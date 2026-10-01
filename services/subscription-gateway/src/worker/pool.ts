// P3 — worker pool: per-lane runtime manager keyed by (provider, account_id)
// (§A8 worker ownership; reuses the supervisor's WorkerKey). activate() is the
// single seam that turns an account row into a live browser runtime:
// reconcile-first via the supervisor, launch once (idempotent), then a
// non-spending probe() mapped onto SessionHealth. Browsers are launched
// through an injectable Launcher so tests never touch a real browser.
import { existsSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, type BrowserContext } from "playwright";
import {
  createPageLease,
  createResolver,
  type SdkAdapterRuntime,
  type SdkPageLease,
  type SdkSelectorResolver,
  type SelectorPack,
} from "@allternit/subscription-adapter-sdk";
import type {
  AccountObservation,
  AdapterManifest,
  ArtifactSink,
  ExecutionContext,
  PageLease,
  ProbeResult,
  RedactingLogger,
  SelectorResolver,
  SessionHealth,
  SubscriptionAdapter,
  TaskAttempt,
} from "@allternit/subscription-fabric-contracts";
import type { AdapterRegistry, LoadedAdapter } from "../adapters/registry.js";
import type { EventLog } from "../events/log.js";
import type { Db } from "../store/db.js";
import { accountHasActiveTasks, getAccount, upsertAccount } from "../store/queries.js";
import { workerKeyId, type WorkerKey, type WorkerSupervisor } from "./supervisor.js";

export type LaneKey = WorkerKey;

// What the worker layer needs from a resident lane: exactly the RunRequest
// adapter/page/makeResolver triple that runAttempt() consumes, plus the
// non-spending probe and a close for shutdown.
export interface LaneRuntime {
  adapter: SubscriptionAdapter;
  page: PageLease;
  makeResolver: (page: PageLease) => SelectorResolver;
  probe(): Promise<ProbeResult>;
  // Who is signed in / usage / plan, read after a ready probe (optional:
  // adapters that can't observe them leave the account's fields as they were).
  readAccount?(): Promise<AccountObservation>;
  readPlan?(): Promise<string | null>;
  close(): Promise<void>;
  // False once the browser behind this runtime is gone (crashed, killed, or
  // its window closed). Optional so fakes without a browser stay alive.
  isAlive?(): boolean;
}

export type Launcher = (
  lane: LaneKey,
  manifest: AdapterManifest,
  profileRef: string
) => Promise<LaneRuntime>;

export interface WorkerPoolDeps {
  db: Db;
  registry: AdapterRegistry;
  supervisor: WorkerSupervisor;
  launch?: Launcher;
  // Base dir for relative profile_refs (default launcher only); main.ts wires
  // config.stateDir. Tests always inject `launch`.
  profilesDir?: string;
  log?: EventLog; // challenge transitions surface one account-scoped ledger event
  logger?: (line: string) => void;
  sessionImport?: PlaywrightLauncherDeps["sessionImport"];
  // After a verification check appears: how often the pool looks again, and
  // for how long (defaults 15 s, 10 min).
  challengeRecheckMs?: number;
  challengeRecheckForMs?: number;
  // Close a lane's Chrome after this long with no task in flight for its
  // account (0/undefined = keep resident). The next task relaunches it, so
  // computers that sleep or share RAM only run Chrome while it is needed.
  laneIdleCloseMs?: number;
  idleSweepMs?: number;
  now?: () => number;
}


interface LaneEntry {
  runtime: LaneRuntime;
  health: SessionHealth;
}

// Probe → SessionHealth. Challenge outranks auth (a challenge page has no
// logged-in probe); a passed auth state with vanished critical locators is UI
// drift; a probe that never completed (navigation/page failure) is
// provider_down (handled by activate's catch, not here).
export function healthFromProbe(result: ProbeResult): SessionHealth {
  if (result.ok) return "ready";
  const failed = new Set(
    result.checks.filter((c) => c.critical && !c.ok).map((c) => c.key)
  );
  if (failed.has("challenge")) return "challenge_presented";
  if (failed.has("auth.state")) return "auth_required";
  return "ui_drift";
}

// fix #6 shape, kept adapter-agnostic here: a profile held by another process
// is profile_locked, not a crash. (Same pattern the web adapter matches.)
const PROFILE_LOCK = /SingletonLock|user data directory is already in use|profile (is )?locked/i;

const NULL_LOGGER: RedactingLogger = {
  debug: () => {},
  info: () => {},
  warn: () => {},
  error: () => {},
};

const NOOP_SINK: ArtifactSink = {
  begin: async () => "artifact-noop",
  write: async () => {},
  commit: async () => {},
  fail: async () => {},
};

export class WorkerPool {
  private readonly launch: Launcher;
  private readonly lanes = new Map<string, LaneEntry>();
  private readonly activations = new Map<string, Promise<LaneRuntime>>();
  // Lanes showing a verification check: a non-spending re-probe notices when
  // the person has cleared it, so the account turns ready by itself.
  private readonly challengeWatches = new Map<string, ReturnType<typeof setInterval>>();
  private readonly lastUsed = new Map<string, number>();
  private readonly laneKeys = new Map<string, LaneKey>();
  private idleSweep: ReturnType<typeof setInterval> | null = null;

  constructor(private readonly deps: WorkerPoolDeps) {
    if (deps.laneIdleCloseMs && deps.laneIdleCloseMs > 0) {
      this.idleSweep = setInterval(() => void this.closeIdleLanes(), deps.idleSweepMs ?? 60_000);
      this.idleSweep.unref?.();
    }
    this.launch =
      deps.launch ??
      createPlaywrightLauncher({
        registry: deps.registry,
        profilesDir: deps.profilesDir ?? process.cwd(),
        sessionImport: deps.sessionImport,
        logger: deps.logger,
      });
  }

  // Login mode: close the lane's Chrome so the profile is free and the next
  // activate relaunches (importing any new login-browser session).
  async deactivate(lane: LaneKey): Promise<void> {
    const id = workerKeyId(lane);
    this.stopChallengeWatch(id);
    await this.activations.get(id)?.catch(() => {});
    const entry = this.lanes.get(id);
    this.lanes.delete(id);
    if (entry) await entry.runtime.close().catch(() => {});
  }

  // Absolute Chrome user-data dir for an account's profile_ref.
  userDataDirFor(profileRef: string): string {
    return isAbsolute(profileRef) ? profileRef : join(this.deps.profilesDir ?? process.cwd(), profileRef);
  }

  private now(): number {
    return this.deps.now?.() ?? Date.now();
  }

  /** Close resident lanes idle past laneIdleCloseMs. Returns the closed lane ids. */
  async closeIdleLanes(): Promise<string[]> {
    const idleMs = this.deps.laneIdleCloseMs ?? 0;
    if (idleMs <= 0) return [];
    const closed: string[] = [];
    for (const id of [...this.lanes.keys()]) {
      const lane = this.laneKeys.get(id);
      if (!lane || this.activations.has(id) || this.challengeWatches.has(id)) continue;
      if (accountHasActiveTasks(this.deps.db, lane.account_id)) {
        this.lastUsed.set(id, this.now());
        continue;
      }
      if (this.now() - (this.lastUsed.get(id) ?? 0) < idleMs) continue;
      await this.deactivate(lane);
      this.lastUsed.delete(id);
      closed.push(id);
      this.deps.logger?.(`subscription-gateway: closed idle browser for ${id}`);
    }
    return closed;
  }

  // A dead runtime is not resident: callers get null (activate relaunches it).
  runtimeFor(lane: LaneKey): LaneRuntime | null {
    this.lastUsed.set(workerKeyId(lane), this.now());
    const runtime = this.lanes.get(workerKeyId(lane))?.runtime ?? null;
    return runtime && runtime.isAlive?.() === false ? null : runtime;
  }

  healthFor(lane: LaneKey): SessionHealth | null {
    return this.lanes.get(workerKeyId(lane))?.health ?? null;
  }

  // Reconcile lookup for the supervisor: only resident runtimes answer. With
  // no runtime the reconcile sweep takes its safe ambiguous path (needs_user,
  // never resubmits) — this is the post-restart boot order (reconcile-first,
  // browser second).
  adapterFor(adapterId: string): SubscriptionAdapter | undefined {
    for (const entry of this.lanes.values()) {
      if (entry.runtime.adapter.manifest.adapter_id === adapterId) return entry.runtime.adapter;
    }
    return undefined;
  }

  // Read-only watch-page-style ctx for adapter.reconcile against the resident
  // lane page. Never reconciles against a different account's browser: no
  // resident runtime for the attempt's lane is a loud error, not a fallback.
  reconcileCtx(attempt: TaskAttempt, adapter: SubscriptionAdapter): ExecutionContext {
    const runtime = this.runtimeFor({
      provider: adapter.manifest.provider,
      account_id: attempt.account_id,
    });
    if (!runtime) {
      throw new Error(
        `no resident runtime for ${adapter.manifest.provider}:${attempt.account_id} — cannot reconcile`
      );
    }
    return {
      signal: new AbortController().signal,
      page: runtime.page,
      artifacts: NOOP_SINK,
      pacing: { beforeAction: async () => {}, beforeTask: async () => {} },
      selectors: runtime.makeResolver(runtime.page),
      log: NULL_LOGGER,
      attempt,
      markSubmitted: async () => {
        throw new Error("reconcile contexts are read-only: markSubmitted is not available");
      },
    };
  }

  // Idempotent: a resident ready runtime is returned as-is; a resident
  // not-ready runtime is re-probed WITHOUT relaunching (this is how the CLI's
  // "run connect again after logging in" re-drives the probe). Concurrent
  // calls single-flight on the same activation.
  async activate(lane: LaneKey): Promise<LaneRuntime> {
    const id = workerKeyId(lane);
    this.lastUsed.set(id, this.now());
    this.laneKeys.set(id, { provider: lane.provider, account_id: lane.account_id });
    const pending = this.activations.get(id);
    if (pending) return pending;
    let existing = this.lanes.get(id);
    // A dead browser is never re-probed (every probe would throw "target
    // closed" → provider_down forever); drop it so doActivate relaunches.
    // Closed inside the single-flight activation so concurrent callers share
    // one relaunch.
    let dead: LaneRuntime | null = null;
    if (existing && existing.runtime.isAlive?.() === false) {
      this.lanes.delete(id);
      dead = existing.runtime;
      existing = undefined;
    }
    if (existing && existing.health === "ready") return existing.runtime;
    const activation = (async () => {
      if (dead) {
        await dead.close().catch(() => {});
        this.deps.logger?.(`subscription-gateway: worker runtime for ${id} was dead; relaunching`);
      }
      return this.doActivate(lane, existing?.runtime ?? null);
    })().finally(() => this.activations.delete(id));
    this.activations.set(id, activation);
    return activation;
  }

  private async doActivate(lane: LaneKey, resident: LaneRuntime | null): Promise<LaneRuntime> {
    const id = workerKeyId(lane);
    const account = getAccount(this.deps.db, lane.account_id);
    if (!account) throw new Error(`account ${lane.account_id} not found`);
    const loaded = this.deps.registry.adapters.find((a) => a.manifest.provider === lane.provider);
    if (!loaded) throw new Error(`no adapter registered for provider ${lane.provider}`);

    // §A8 recovery rule: sent_unconfirmed attempts reconcile BEFORE the lane
    // is served (and before a fresh browser launches).
    await this.deps.supervisor.ensureWorker(lane);

    let runtime = resident;
    if (!runtime) {
      try {
        runtime = await this.launch(lane, loaded.manifest, account.profile_ref);
      } catch (err) {
        if (PROFILE_LOCK.test(err instanceof Error ? err.message : String(err))) {
          this.persistHealth(lane, "profile_locked");
        }
        throw err;
      }
      this.deps.logger?.(`subscription-gateway: worker runtime launched for ${id}`);
    }

    let health: SessionHealth;
    try {
      health = healthFromProbe(await runtime.probe());
    } catch (err) {
      health = "provider_down";
      this.deps.logger?.(
        `subscription-gateway: probe for ${id} threw: ${err instanceof Error ? err.message : String(err)}`
      );
    }
    this.persistHealth(lane, health);
    this.deps.logger?.(`subscription-gateway: probe ${id} → ${health}`);
    if (health === "ready") await this.observeAccount(lane, runtime);
    // Critical #5 — a challenge is stop + surface, never auto-retried (the
    // watch below only re-probes, to notice the person cleared it). One
    // account-scoped ledger event per transition into challenge_presented
    // (SSE fan-out via the hub; the notifier pattern needs no extra call).
    const previous = this.lanes.get(id)?.health;
    this.lanes.set(id, { runtime, health });
    if (health === "challenge_presented") this.watchChallenge(lane);
    if (health === "challenge_presented" && previous !== "challenge_presented") {
      this.deps.log?.append({
        task_id: `account:${lane.account_id}`,
        kind: "needs_user",
        payload: {
          account_id: lane.account_id,
          reason: "challenge",
          message:
            "Provider presented a verification interstitial — solve it in the Sessions window; it is never auto-retried",
        },
        callers: [],
      });
    }
    return runtime;
  }

  // Best effort: a failed read never changes health or blocks the lane.
  private async observeAccount(lane: LaneKey, runtime: LaneRuntime): Promise<void> {
    const observed = runtime.readAccount ? await runtime.readAccount().catch(() => null) : null;
    const plan = runtime.readPlan ? await runtime.readPlan().catch(() => null) : null;
    if (!observed && !plan) return;
    const fresh = getAccount(this.deps.db, lane.account_id);
    if (!fresh) return;
    upsertAccount(this.deps.db, {
      ...fresh,
      identity: observed?.identity ?? fresh.identity ?? null,
      usage: observed?.usage ?? fresh.usage ?? null,
      agents: observed?.agents ?? fresh.agents ?? null,
      ...(plan ? { plan, plan_observed_at: new Date().toISOString() } : {}),
    });
  }

  private persistHealth(lane: LaneKey, health: SessionHealth): void {
    const fresh = getAccount(this.deps.db, lane.account_id);
    if (!fresh) return;
    upsertAccount(this.deps.db, { ...fresh, session_health: health });
  }

  // Looks again (probe only: nothing is clicked, solved or resent) until the
  // check is gone, the lane closes (e.g. a login window took the profile), or
  // the time runs out. One watch per lane.
  private watchChallenge(lane: LaneKey): void {
    const id = workerKeyId(lane);
    if (this.challengeWatches.has(id)) return;
    const until = Date.now() + (this.deps.challengeRecheckForMs ?? 10 * 60_000);
    let busy = false;
    const timer = setInterval(() => {
      if (busy) return;
      if (Date.now() > until || !this.runtimeFor(lane)) {
        this.stopChallengeWatch(id);
        return;
      }
      busy = true;
      void this.activate(lane)
        .catch(() => {})
        .finally(() => {
          busy = false;
          if (this.healthFor(lane) !== "challenge_presented") this.stopChallengeWatch(id);
        });
    }, this.deps.challengeRecheckMs ?? 15_000);
    timer.unref?.();
    this.challengeWatches.set(id, timer);
  }

  private stopChallengeWatch(id: string): void {
    const timer = this.challengeWatches.get(id);
    if (timer) clearInterval(timer);
    this.challengeWatches.delete(id);
  }

  async shutdown(): Promise<void> {
    if (this.idleSweep) clearInterval(this.idleSweep);
    for (const id of [...this.challengeWatches.keys()]) this.stopChallengeWatch(id);
    const entries = [...this.lanes.values()];
    this.lanes.clear();
    await Promise.allSettled(entries.map((e) => e.runtime.close()));
  }
}

// ---------------------------------------------------------------------------
// Default launcher: persistent, headed system Chrome under the account's
// profile_ref; the adapter instance comes from the adapter package itself.
// ---------------------------------------------------------------------------

// Adapter package convention (P3): <dir>/adapter.ts (or a precompiled
// adapter.js) exports `createAdapter()` — a zero-arg factory returning the
// SubscriptionAdapter — or a default export of the same shape.
async function instantiateAdapter(loaded: LoadedAdapter): Promise<SubscriptionAdapter> {
  let moduleUrl: string | null = null;
  for (const name of ["adapter.js", "adapter.ts"]) {
    const candidate = join(loaded.dir, name);
    if (existsSync(candidate)) {
      moduleUrl = pathToFileURL(candidate).href;
      break;
    }
  }
  if (!moduleUrl) {
    throw new Error(`adapter ${loaded.manifest.adapter_id} has no adapter module under ${loaded.dir}`);
  }
  const mod = (await import(moduleUrl)) as Record<string, unknown>;
  const factory = (mod.createAdapter ?? mod.default) as unknown;
  if (typeof factory !== "function") {
    throw new Error(
      `adapter ${loaded.manifest.adapter_id} module must export createAdapter() (or a default factory)`
    );
  }
  const adapter = factory() as SubscriptionAdapter;
  if (
    !adapter ||
    adapter.manifest?.adapter_id !== loaded.manifest.adapter_id ||
    typeof adapter.attach !== "function" ||
    typeof adapter.execute !== "function" ||
    typeof adapter.probe !== "function"
  ) {
    throw new Error(`adapter module at ${moduleUrl} did not produce adapter ${loaded.manifest.adapter_id}`);
  }
  return adapter;
}

export interface PlaywrightLauncherDeps {
  registry: AdapterRegistry;
  // Login mode: import a session the human established in the login browser
  // (Firefox) into this Chrome context before the first navigation.
  sessionImport?: (userDataDir: string, context: BrowserContext) => Promise<number>;
  logger?: (line: string) => void;
  // Base dir for relative profile_refs (e.g. config.stateDir →
  // <stateDir>/profiles/<account_id>). Absolute profile_refs pass through.
  profilesDir: string;
  // How long a probe waits for the page to settle before judging it (default
  // 8 s). Single-page apps (Kimi) draw the signed-in UI after
  // domcontentloaded; judging at once reads a signed-in page as logged out.
  probeSettleMs?: number;
}

// Wait until the page shows something decisive: the signed-in marker, a
// verification check, or the pack's optional logged_out_probe. Signed-in
// pages return as soon as they render; the rest wait out the budget.
export async function settleForProbe(
  resolve: (key: string) => Promise<unknown | null>,
  keys: { loggedIn: string },
  budgetMs: number,
  pollMs = 250
): Promise<void> {
  const end = Date.now() + budgetMs;
  for (;;) {
    for (const key of [keys.loggedIn, "challenge", "logged_out_probe"]) {
      if ((await resolve(key).catch(() => null)) !== null) return;
    }
    if (Date.now() >= end) return;
    await new Promise((r) => setTimeout(r, pollMs));
  }
}

export function createPlaywrightLauncher(deps: PlaywrightLauncherDeps): Launcher {
  return async (lane, manifest, profileRef) => {
    const loaded = deps.registry.byId(manifest.adapter_id);
    if (!loaded) throw new Error(`adapter ${manifest.adapter_id} is not in the registry`);
    const adapter = await instantiateAdapter(loaded);
    const pack = (adapter as unknown as { pack?: SelectorPack }).pack;
    if (!pack) {
      throw new Error(`adapter ${manifest.adapter_id} exposes no selector pack (SDK DeclarativeChatAdapter shape required)`);
    }

    const userDataDir = isAbsolute(profileRef) ? profileRef : join(deps.profilesDir, profileRef);
    // Sessions window: headed, real Chrome, persistent profile. Never closed
    // on auth walls or challenges — the human drives it; pool.shutdown() owns
    // the lifecycle.
    const context: BrowserContext = await chromium.launchPersistentContext(userDataDir, {
      channel: "chrome",
      headless: false,
      // After a crash or kill -9 Chrome otherwise floats a "Restore pages?"
      // bubble over the provider page on every relaunch.
      args: ["--hide-crash-restore-bubble", "--start-maximized"],
      // The page follows the window. Playwright's default 1280x720 viewport plus
      // Chrome's toolbars made the window taller than a 720p Sessions display,
      // so its bottom (composer, buttons) sat off-screen in the viewer.
      viewport: null,
    });
    try {
      if (deps.sessionImport) {
        const n = await deps.sessionImport(userDataDir, context);
        if (n > 0) deps.logger?.(`subscription-gateway: imported login-browser session (${n} cookies) for ${lane.account_id}`);
      }
      const page = context.pages()[0] ?? (await context.newPage());
      const runtimeCtx: SdkAdapterRuntime = {
        adapter_id: manifest.adapter_id,
        origins: manifest.origins,
        page,
        navigate: async (url: string) => {
          await page.goto(url);
        },
      };
      await adapter.attach(runtimeCtx);
      const lease = createPageLease(page);
      const makeResolver = (p: PageLease): SdkSelectorResolver =>
        createResolver((p as SdkPageLease).page, pack);
      if (manifest.origins.length > 0) {
        await page.goto(manifest.origins[0], { waitUntil: "domcontentloaded" });
      }
      let closed = false;
      let alive = true;
      context.on("close", () => {
        alive = false;
      });
      return {
        adapter,
        isAlive: () => alive,
        page: lease,
        makeResolver,
        ...(adapter.readAccount
          ? { readAccount: () => adapter.readAccount!(new AbortController().signal) }
          : {}),
        ...(adapter.readPlan ? { readPlan: () => adapter.readPlan!(new AbortController().signal) } : {}),
        async probe(): Promise<ProbeResult> {
          const resolver = makeResolver(lease);
          await settleForProbe(
            (key) => resolver.tryResolveLocator(key),
            { loggedIn: manifest.auth.logged_in_probe },
            deps.probeSettleMs ?? 8000
          );
          const result = await adapter.probe(new AbortController().signal);
          // §A5/Critical #5 — surface a challenge interstitial as a failing
          // critical check so the pool maps it to challenge_presented. Packs
          // without a challenge key resolve to null (no-op).
          const challenge = await makeResolver(lease).tryResolveLocator("challenge");
          if (challenge) {
            result.checks.push({
              key: "challenge",
              critical: true,
              ok: false,
              detail: "challenge interstitial present",
            });
            result.ok = false;
          }
          return result;
        },
        async close(): Promise<void> {
          if (closed) return;
          closed = true;
          await adapter.detach().catch(() => {});
          await context.close().catch(() => {});
        },
      };
    } catch (err) {
      await context.close().catch(() => {});
      throw err;
    }
  };
}
