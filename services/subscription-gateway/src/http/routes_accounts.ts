// /v1/accounts — create/list/status/disconnect plus P3 connect: activating an
// account's lane (launch + probe) through the worker pool. Bots never hold
// accounts:manage — enforced at token issue (§A6.2).
import { randomUUID } from "node:crypto";
import { Router, type Request, type Response } from "express";
import { z } from "zod";
import { providerIdSchema, type Account } from "@allternit/subscription-fabric-contracts";
import { getAccount, listAccounts, upsertAccount } from "../store/queries.js";
import { callerOf, requireScope, type GatewayDeps } from "./server.js";

const connectSchema = z.object({
  account_id: z.string().min(1).optional(),
  provider: providerIdSchema,
  label: z.string().min(1),
  plan: z.string().nullable().optional(),
});

export function accountsRouter(deps: GatewayDeps): Router {
  const router = Router();

  router.get("/v1/accounts", requireScope("accounts:manage"), (_req: Request, res: Response) => {
    res.json(listAccounts(deps.db));
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
      });
    }
  );

  router.post("/v1/accounts", requireScope("accounts:manage"), (req: Request, res: Response) => {
    const parsed = connectSchema.safeParse(req.body);
    if (!parsed.success) {
      res.status(400).json({ error: "invalid_account", detail: parsed.error.issues });
      return;
    }
    const caller = callerOf(req);
    const accountId = parsed.data.account_id ?? randomUUID();
    const account: Account = {
      account_id: accountId,
      provider: parsed.data.provider,
      label: parsed.data.label,
      plan: parsed.data.plan ?? null,
      plan_observed_at: null,
      profile_ref: `profiles/${accountId}`,
      session_health: "auth_required",
      enabled: true,
    };
    upsertAccount(deps.db, account);
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
    res.status(201).json(account);
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
      } catch (err) {
        res.status(502).json({
          error: "activation_failed",
          detail: err instanceof Error ? err.message : String(err),
        });
        return;
      }
      res.json(getAccount(deps.db, req.params.id));
    }
  );

  // Login mode: providers that sign in through Google refuse automated
  // Chrome, so the human signs in inside a plain Firefox window on the
  // account's own Firefox profile. The adapter's Chrome is closed so the next
  // connect relaunches it and imports the new session. Never automates the
  // login itself and never sees credentials.
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
          detail: "no login browser configured (install Firefox or set SUBS_GATEWAY_LOGIN_BROWSER)",
        });
        return;
      }
      const origin = deps.adapterRegistry?.adapters.find(
        (a) => a.manifest.provider === account.provider
      )?.manifest.origins[0];
      if (!origin) {
        res.status(409).json({ error: "no_adapter_for_provider", provider: account.provider });
        return;
      }
      const lane = { provider: account.provider, account_id: account.account_id };
      await deps.pool.deactivate(lane);
      try {
        await deps.loginBrowser.open(
          account.account_id,
          deps.pool.userDataDirFor(account.profile_ref),
          origin
        );
      } catch (err) {
        res.status(502).json({
          error: "login_browser_failed",
          detail: err instanceof Error ? err.message : String(err),
        });
        return;
      }
      res.json({
        account_id: account.account_id,
        status: "login_window_open",
        browser: "firefox",
        next: `Sign in in the Firefox window, then POST /v1/accounts/${account.account_id}/connect`,
      });
    }
  );

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
      res.json(getAccount(deps.db, req.params.id));
    }
  );

  return router;
}
