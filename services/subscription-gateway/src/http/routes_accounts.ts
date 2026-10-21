// /v1/accounts — create/list/status/disconnect plus P3 connect: activating an
// account's lane (launch + probe) through the worker pool. Bots never hold
// accounts:manage — enforced at token issue (§A6.2).
import { randomUUID } from "node:crypto";
import { rmSync } from "node:fs";
import { Router, type Request, type Response } from "express";
import { z } from "zod";
import { providerIdSchema, type Account } from "@allternit/subscription-fabric-contracts";
import {
  accountHasActiveTasks,
  deleteAccount,
  ensurePreferredAccount,
  getAccount,
  listAccounts,
  setPreferredAccount,
  upsertAccount,
} from "../store/queries.js";
import { firefoxProfileFor, readFirefoxCookies, type ImportableCookie, type SessionStorageKey } from "../worker/login_browser.js";
import { callerOf, requireScope, type GatewayDeps } from "./server.js";

const patchAccountSchema = z
  .object({ preferred: z.literal(true).optional(), label: z.string().trim().min(1).max(60).optional() })
  .strict();

const connectSchema = z.object({
  account_id: z.string().min(1).optional(),
  provider: providerIdSchema,
  // Defaults to the provider's name ("ChatGPT").
  label: z.string().min(1).optional(),
  plan: z.string().nullable().optional(),
  // Open the login window in the same call (Settings: one-step add).
  login: z.boolean().optional(),
});

// The providers people pick from in Settings; each gains an adapter in turn.
const KNOWN_PROVIDERS: Record<string, string> = { chatgpt: "ChatGPT", claude: "Claude", kimi: "Kimi", google: "Gemini", microsoft: "Copilot" };

export function providerName(provider: string): string {
  return KNOWN_PROVIDERS[provider] ?? provider.charAt(0).toUpperCase() + provider.slice(1);
}

export type UserActionKind = "login" | "wait" | "contact" | "none";

// What the person can do about each session state, in their words.
export function userActionFor(account: Pick<Account, "provider" | "session_health">): {
  kind: UserActionKind;
  label: string;
} {
  const name = providerName(account.provider);
  switch (account.session_health) {
    case "ready":
      return { kind: "none", label: "" };
    case "auth_required":
      return { kind: "login", label: "Log in" };
    case "challenge_presented":
      return { kind: "login", label: "Finish the check in the login window" };
    case "profile_locked":
      return { kind: "wait", label: "Busy in another window; try again shortly" };
    case "degraded":
      return { kind: "wait", label: "Working, but slower than usual" };
    case "provider_down":
      return { kind: "wait", label: `${name} is down right now` };
    case "ui_drift":
      return { kind: "wait", label: `${name} changed its page; Allternit is updating` };
    case "account_restricted":
      return { kind: "contact", label: `${name} restricted this account; check it on ${name}'s site` };
  }
}

export function accountView(account: Account) {
  return {
    ...account,
    identity: account.identity ?? null,
    usage: account.usage ?? null,
    user_action: userActionFor(account),
  };
}

export type LoginStateName = "none" | "waiting" | "signed_in" | "failed" | "closed";

interface LoginSession {
  state: LoginStateName;
  detail: string | null;
  opened_at: string | null;
  baseline: Map<string, string>;
  timer: ReturnType<typeof setInterval> | null;
  finishing: boolean;
  // Signed in, but the provider showed its verification check to the
  // adapter's window: re-probe (non-spending) until it's cleared.
  challengeUntil?: number;
}

export interface AccountsRouterOptions {
  loginPollMs?: number; // default 2000
  challengePollMs?: number; // default 5000
  challengeWaitMs?: number; // default 10 min
  readLoginCookies?: (firefoxProfile: string) => ImportableCookie[]; // tests
  readLoginStorage?: (profile: string, entries: SessionStorageKey[]) => Map<string, string>; // tests
}

export function accountsRouter(deps: GatewayDeps, opts: AccountsRouterOptions = {}): Router {
  const router = Router();
  const readCookies =
    opts.readLoginCookies ??
    ((profile: string) => (deps.loginBrowser ? deps.loginBrowser.readCookies(profile) : readFirefoxCookies(profile)));
  const readStorage =
    opts.readLoginStorage ??
    ((profile: string, entries: SessionStorageKey[]) => deps.loginBrowser?.readStorage?.(profile, entries) ?? new Map<string, string>());
  const loginProfileFor = (userDataDir: string) =>
    deps.loginBrowser ? deps.loginBrowser.profileFor(userDataDir) : firefoxProfileFor(userDataDir);
  const logins = new Map<string, LoginSession>();

  const manifestFor = (provider: string) =>
    deps.adapterRegistry?.adapters.find((a) => a.manifest.provider === provider)?.manifest;

  // The provider's signed-in cookies (and localStorage session keys) in the
  // account's login-browser profile.
  const sessionCookies = (account: Account): Map<string, string> => {
    const out = new Map<string, string>();
    const manifest = manifestFor(account.provider);
    const names = manifest?.auth.session_cookies ?? [];
    const storage = manifest?.auth.session_storage ?? [];
    if (!deps.pool) return out;
    if (storage.length > 0) {
      const udd = deps.pool.userDataDirFor(account.profile_ref);
      for (const [k, v] of readStorage(loginProfileFor(udd), storage)) out.set(k, v);
    }
    if (names.length === 0) return out;
    // Every origin: a sign-in can land on another of the provider's domains
    // (kimi.com → kimi.ai).
    const hosts = manifest!.origins.map((o) => new URL(o).hostname);
    const profile = loginProfileFor(deps.pool.userDataDirFor(account.profile_ref));
    for (const c of readCookies(profile)) {
      const domain = c.domain.replace(/^\./, "");
      if (!hosts.some((host) => host === domain || host.endsWith(`.${domain}`))) continue;
      if (names.some((n) => c.name.startsWith(n))) out.set(c.name, c.value);
    }
    return out;
  };

  const loginState = (accountId: string) => {
    const s = logins.get(accountId);
    // After a bot check the tracker waits while the adapter keeps probing; once the
    // account itself reads ready, the login is done even if the watch never flipped it
    // (Eoj, 2026-10-04: the app kept saying "waiting" after the check was cleared).
    if (s && s.state === "waiting" && getAccount(deps.db, accountId)?.session_health === "ready") {
      stopWatch(s);
      s.state = "signed_in";
      s.detail = null;
    }
    const done = s?.state === "signed_in" || s?.state === "failed";
    const account = done ? getAccount(deps.db, accountId) : null;
    return {
      state: s?.state ?? ("none" as LoginStateName),
      detail: s?.detail ?? null,
      opened_at: s?.opened_at ?? null,
      account: account ? accountView(account) : null,
    };
  };

  const stopWatch = (s: LoginSession) => {
    if (s.timer) clearInterval(s.timer);
    s.timer = null;
  };

  // Login done: close the login browser so its cookie store is flushed, relaunch the
  // adapter's Chrome (it imports the new session) and probe.
  const finishLogin = async (account: Account, s: LoginSession) => {
    s.finishing = true;
    stopWatch(s);
    try {
      await deps.loginBrowser?.close(account.account_id);
      const lane = { provider: account.provider, account_id: account.account_id };
      await deps.pool!.deactivate(lane);
      await deps.pool!.activate(lane);
      const fresh = getAccount(deps.db, account.account_id);
      if (fresh?.session_health === "ready") {
        s.state = "signed_in";
        s.detail = null;
      } else if (fresh?.session_health === "challenge_presented") {
        // The adapter's window shows the check; the person clears it there
        // and this keeps probing until the account is ready.
        s.state = "waiting";
        s.detail = `Finish the security check in the ${providerName(account.provider)} window on the computer`;
        s.challengeUntil = Date.now() + (opts.challengeWaitMs ?? 10 * 60_000);
        s.timer = setInterval(() => void challengeTick(account.account_id), opts.challengePollMs ?? 5000);
        s.timer.unref?.();
      } else {
        s.state = "failed";
        s.detail = fresh ? userActionFor(fresh).label || fresh.session_health : "account removed";
      }
    } catch (err) {
      s.state = "failed";
      s.detail = err instanceof Error ? err.message : String(err);
    } finally {
      s.finishing = false;
    }
  };

  const challengeTick = async (accountId: string) => {
    const s = logins.get(accountId);
    if (!s || s.state !== "waiting" || s.finishing || s.challengeUntil === undefined) return;
    const account = getAccount(deps.db, accountId);
    if (!account) {
      stopWatch(s);
      logins.delete(accountId);
      return;
    }
    s.finishing = true;
    try {
      await deps.pool!.activate({ provider: account.provider, account_id: accountId });
    } catch {
      // provider_down for a moment; the next tick probes again
    } finally {
      s.finishing = false;
    }
    const fresh = getAccount(deps.db, accountId);
    if (fresh?.session_health === "ready") {
      stopWatch(s);
      s.state = "signed_in";
      s.detail = null;
    } else if (fresh?.session_health !== "challenge_presented" && fresh?.session_health !== "provider_down") {
      stopWatch(s);
      s.state = "failed";
      s.detail = fresh ? userActionFor(fresh).label || fresh.session_health : "account removed";
    } else if (Date.now() > s.challengeUntil) {
      stopWatch(s);
      s.state = "failed";
      s.detail = "The security check wasn't finished in time.";
    }
  };

  const tick = async (accountId: string) => {
    const s = logins.get(accountId);
    if (!s || s.state !== "waiting" || s.finishing) return;
    if (!deps.loginBrowser?.isOpen(accountId)) {
      s.state = "closed";
      s.detail = "The login window was closed before signing in.";
      stopWatch(s);
      return;
    }
    const account = getAccount(deps.db, accountId);
    if (!account) {
      stopWatch(s);
      logins.delete(accountId);
      return;
    }
    let now: Map<string, string>;
    try {
      now = sessionCookies(account);
    } catch {
      return; // cookie store mid-write; next tick
    }
    const signedIn = [...now].some(([name, value]) => s.baseline.get(name) !== value);
    if (signedIn) await finishLogin(account, s);
  };

  // Opens the login window and starts watching for the sign-in.
  const openLogin = async (account: Account) => {
    const origin = manifestFor(account.provider)?.origins[0];
    if (!origin) return { error: 409, body: { error: "no_adapter_for_provider", provider: account.provider } };
    const lane = { provider: account.provider, account_id: account.account_id };
    const previous = logins.get(account.account_id);
    if (previous) stopWatch(previous);
    let baseline = new Map<string, string>();
    try {
      baseline = sessionCookies(account);
    } catch {
      // unreadable now: any session cookie seen later counts as a sign-in
    }
    // Already signed in (the profile holds a session): a probe settles it
    // without a login window.
    if (baseline.size > 0) {
      try {
        await deps.pool!.activate(lane);
      } catch {
        // fall through to the login window
      }
      if (getAccount(deps.db, account.account_id)?.session_health === "ready") {
        logins.set(account.account_id, {
          state: "signed_in",
          detail: null,
          opened_at: new Date().toISOString(),
          baseline,
          timer: null,
          finishing: false,
        });
        return { error: null, body: null };
      }
    }
    await deps.pool!.deactivate(lane);
    try {
      await deps.loginBrowser!.open(account.account_id, deps.pool!.userDataDirFor(account.profile_ref), origin);
    } catch (err) {
      return {
        error: 502,
        body: { error: "login_browser_failed", detail: err instanceof Error ? err.message : String(err) },
      };
    }
    const s: LoginSession = {
      state: "waiting",
      detail: null,
      opened_at: new Date().toISOString(),
      baseline,
      timer: null,
      finishing: false,
    };
    s.timer = setInterval(() => void tick(account.account_id), opts.loginPollMs ?? 2000);
    s.timer.unref?.();
    logins.set(account.account_id, s);
    return { error: null, body: null };
  };

  router.get("/v1/providers", requireScope("accounts:manage"), (_req: Request, res: Response) => {
    const registered = new Set((deps.adapterRegistry?.adapters ?? []).map((a) => a.manifest.provider as string));
    const ids = [...new Set([...Object.keys(KNOWN_PROVIDERS), ...registered])];
    res.json(
      ids.map((id) => ({
        id,
        name: providerName(id),
        supported: registered.has(id),
        login_supported: registered.has(id) && Boolean(deps.loginBrowser && deps.pool),
      }))
    );
  });

  router.get("/v1/accounts", requireScope("accounts:manage"), (_req: Request, res: Response) => {
    res.json(listAccounts(deps.db).map(accountView));
  });

  router.get(
    "/v1/accounts/:id/status",
    requireScope("accounts:manage", "tasks:read"),
    (req: Request, res: Response) => {
      const account = getAccount(deps.db, req.params.id);
      if (!account) {
        res.status(404).json({ error: "account_not_found", account_id: req.params.id });
        return;
      }
      res.json({
        account_id: account.account_id,
        session_health: account.session_health,
        enabled: account.enabled,
        plan: account.plan,
        user_action: userActionFor(account),
      });
    }
  );

  router.post("/v1/accounts", requireScope("accounts:manage"), async (req: Request, res: Response) => {
    const parsed = connectSchema.safeParse(req.body);
    if (!parsed.success) {
      res.status(400).json({ error: "invalid_account", detail: parsed.error.issues });
      return;
    }
    // Only providers an adapter is loaded for: an unknown one would sit as a
    // dead auth_required row nothing can ever sign in to.
    const adapters = deps.adapterRegistry?.adapters;
    if (adapters && !adapters.some((a) => a.manifest.provider === parsed.data.provider)) {
      res.status(400).json({ error: "unknown_provider", provider: parsed.data.provider });
      return;
    }
    const caller = callerOf(req);
    const accountId = parsed.data.account_id ?? randomUUID();
    const account: Account = {
      account_id: accountId,
      provider: parsed.data.provider,
      label: parsed.data.label ?? providerName(parsed.data.provider),
      plan: parsed.data.plan ?? null,
      plan_observed_at: null,
      profile_ref: `profiles/${accountId}`,
      session_health: "auth_required",
      enabled: true,
    };
    upsertAccount(deps.db, account);
    ensurePreferredAccount(deps.db, account.provider);
    // Account-scoped ledger entry; the events table is task-keyed, so the
    // synthetic task_id is `account:<id>` (documented in the notes).
    deps.log.append({
      task_id: `account:${account.account_id}`,
      kind: "needs_user",
      payload: {
        account_id: account.account_id,
        reason: "auth",
        message: `Account "${account.label}" needs an interactive login in the Sessions window`,
      },
      callers: [caller.caller_id],
    });
    let login: ReturnType<typeof loginState> | null = null;
    if (parsed.data.login) {
      if (!deps.pool || !deps.loginBrowser) {
        login = { state: "failed", detail: "no login browser on this Sessions computer", opened_at: null, account: null };
      } else {
        const opened = await openLogin(account);
        login = opened.error
          ? { state: "failed", detail: String((opened.body as { error: string }).error), opened_at: null, account: null }
          : loginState(account.account_id);
      }
    }
    res.status(201).json({ ...accountView(getAccount(deps.db, account.account_id) ?? account), login });
  });

  router.post(
    "/v1/accounts/:id/connect",
    requireScope("accounts:manage"),
    async (req: Request, res: Response) => {
      const account = getAccount(deps.db, req.params.id);
      if (!account) {
        res.status(404).json({ error: "account_not_found", account_id: req.params.id });
        return;
      }
      if (!account.enabled) {
        res.status(409).json({ error: "account_disabled", account_id: account.account_id });
        return;
      }
      if (!deps.pool) {
        res.status(503).json({ error: "worker_unavailable" });
        return;
      }
      // P3 — the activation seam: reconcile-first, launch the Sessions window
      // once (idempotent), then a non-spending probe. The returned account's
      // session_health is the probe outcome (ready / auth_required /
      // challenge_presented / ui_drift / provider_down); auth walls and
      // challenges leave the window open for the human and are never retried
      // here — re-POST to re-drive the probe after an interactive login.
      // Login mode: a login-browser window still open holds the fresh session
      // in memory — close it first so cookies.sqlite is flushed, then the
      // relaunched Chrome imports it.
      if (deps.loginBrowser?.isOpen(account.account_id)) {
        await deps.loginBrowser.close(account.account_id);
        await deps.pool.deactivate({ provider: account.provider, account_id: account.account_id });
      }
      try {
        await deps.pool.activate({
          provider: account.provider,
          account_id: account.account_id,
        });
        // Activation is idempotent on ready lanes; an explicit reconnect still
        // rereads rendered account bots without reopening or changing the page.
        await deps.pool.refreshAccount?.({ provider: account.provider, account_id: account.account_id });
      } catch (err) {
        res.status(502).json({
          error: "activation_failed",
          detail: err instanceof Error ? err.message : String(err),
        });
        return;
      }
      const fresh = getAccount(deps.db, req.params.id);
      const s = logins.get(req.params.id);
      if (s && s.state === "waiting" && fresh) {
        stopWatch(s);
        s.state = fresh.session_health === "ready" ? "signed_in" : "failed";
        s.detail = fresh.session_health === "ready" ? null : userActionFor(fresh).label || fresh.session_health;
      }
      res.json(fresh ? accountView(fresh) : null);
    }
  );

  // Login mode: providers that sign in through Google refuse automated
  // Chrome, so the human signs in inside a plain, non-automated browser
  // window (Chrome on the account's own profile; Firefox on a side profile
  // whose session is imported). The adapter's Chrome is closed so the next
  // connect relaunches it with the new session. Never automates the login
  // itself and never sees credentials.
  router.post(
    "/v1/accounts/:id/login",
    requireScope("accounts:manage"),
    async (req: Request, res: Response) => {
      const account = getAccount(deps.db, req.params.id);
      if (!account) {
        res.status(404).json({ error: "account_not_found", account_id: req.params.id });
        return;
      }
      if (!account.enabled) {
        res.status(409).json({ error: "account_disabled", account_id: account.account_id });
        return;
      }
      if (!deps.pool || !deps.loginBrowser) {
        res.status(501).json({
          error: "login_browser_unavailable",
          detail: "no login browser configured (install Google Chrome or set SUBS_GATEWAY_LOGIN_BROWSER)",
        });
        return;
      }
      const opened = await openLogin(account);
      if (opened.error) {
        res.status(opened.error).json(opened.body);
        return;
      }
      // The old fields stay for existing callers; the rest is the LoginState
      // the Settings hub polls (GET /v1/accounts/:id/login).
      res.json({
        account_id: account.account_id,
        status: "login_window_open",
        next: `Sign in in the login window; the gateway connects by itself (or POST /v1/accounts/${account.account_id}/connect)`,
        ...loginState(account.account_id),
      });
    }
  );

  router.get("/v1/accounts/:id/login", requireScope("accounts:manage"), (req: Request, res: Response) => {
    if (!getAccount(deps.db, req.params.id)) {
      res.status(404).json({ error: "account_not_found", account_id: req.params.id });
      return;
    }
    res.json(loginState(req.params.id));
  });

  // Cancel: the person closed the viewer. The account stays as it was.
  router.delete(
    "/v1/accounts/:id/login",
    requireScope("accounts:manage"),
    async (req: Request, res: Response) => {
      if (!getAccount(deps.db, req.params.id)) {
        res.status(404).json({ error: "account_not_found", account_id: req.params.id });
        return;
      }
      await deps.loginBrowser?.close(req.params.id);
      const s = logins.get(req.params.id);
      if (s && s.state === "waiting") {
        stopWatch(s);
        s.state = "closed";
        s.detail = null;
      }
      res.json(loginState(req.params.id));
    }
  );

  // Remove an account: sign-out included (its browser profiles are deleted).
  router.delete("/v1/accounts/:id", requireScope("accounts:manage"), async (req: Request, res: Response) => {
    const account = getAccount(deps.db, req.params.id);
    if (!account) {
      res.status(404).json({ error: "account_not_found", account_id: req.params.id });
      return;
    }
    if (accountHasActiveTasks(deps.db, account.account_id)) {
      res.status(409).json({ error: "account_busy", account_id: account.account_id });
      return;
    }
    await deps.loginBrowser?.close(account.account_id);
    const s = logins.get(account.account_id);
    if (s) stopWatch(s);
    logins.delete(account.account_id);
    if (deps.pool) {
      await deps.pool.deactivate({ provider: account.provider, account_id: account.account_id });
      // Only the gateway's own relative profiles are ever deleted.
      if (/^profiles\/[\w-]+$/.test(account.profile_ref)) {
        const dir = deps.pool.userDataDirFor(account.profile_ref);
        rmSync(dir, { recursive: true, force: true });
        rmSync(firefoxProfileFor(dir), { recursive: true, force: true });
      }
    }
    deleteAccount(deps.db, account.account_id);
    ensurePreferredAccount(deps.db, account.provider);
    res.status(204).end();
  });

  // Switch which login of a subscription is used first, or rename it
  // ("Work", "Personal") so two logins of one provider are easy to tell apart.
  router.patch("/v1/accounts/:id", requireScope("accounts:manage"), (req: Request, res: Response) => {
    const parsed = patchAccountSchema.safeParse(req.body);
    if (!parsed.success) {
      res.status(400).json({ error: "invalid_account", detail: parsed.error.issues });
      return;
    }
    const account = getAccount(deps.db, req.params.id);
    if (!account) {
      res.status(404).json({ error: "account_not_found", account_id: req.params.id });
      return;
    }
    if (parsed.data.label !== undefined) upsertAccount(deps.db, { ...account, label: parsed.data.label });
    if (parsed.data.preferred === true) setPreferredAccount(deps.db, account.account_id);
    const fresh = getAccount(deps.db, account.account_id);
    res.json(fresh ? accountView(fresh) : null);
  });

  router.post(
    "/v1/accounts/:id/disconnect",
    requireScope("accounts:manage"),
    (req: Request, res: Response) => {
      const account = getAccount(deps.db, req.params.id);
      if (!account) {
        res.status(404).json({ error: "account_not_found", account_id: req.params.id });
        return;
      }
      upsertAccount(deps.db, { ...account, enabled: false });
      const fresh = getAccount(deps.db, req.params.id);
      res.json(fresh ? accountView(fresh) : null);
    }
  );

  return router;
}
