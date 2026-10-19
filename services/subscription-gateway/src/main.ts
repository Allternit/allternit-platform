// Boot — order matters: config → secret-store gate (D3/D15: refuse without
// it) → store+migrations → events/http wiring → UDS (+TCP if enabled) → log binds.
import { readFileSync, realpathSync } from "node:fs";
import { request as httpRequest, type Server } from "node:http";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { Express } from "express";
import type { RedactingLogger } from "@allternit/subscription-fabric-contracts";
import { loadConfig, stallTimeoutFor, type Config } from "./config.js";
import { loadAdapterRegistry, type AdapterRegistry } from "./adapters/registry.js";
import { openDatabase, type Db } from "./store/db.js";
import { BROWSER_LANES_NEED_CHROME, BROWSER_LANES_OFF, browserLanesEnabled, isLocalDesktop, setBrowserLanesCheck } from "./local_desktop.js";
import { detectGoogleChrome } from "./worker/hidden_chrome.js";
import { listAccounts, preferredReadyAccount } from "./store/queries.js";
import { EventLog } from "./events/log.js";
import { SseHub, createSubscriber } from "./events/sse.js";
import { CallerOutbox } from "./events/outbox.js";
import { Notifier } from "./events/notify.js";
import {
  KeychainUnavailable,
  requireKeychain,
  selectKeychainBackend,
  type KeychainBackend,
} from "./security/keychain.js";
import { ensureCliToken, CLI_TOKEN_ACCOUNT } from "./security/tokens.js";
import { createAaiHost } from "./aai/registry.js";
import { closeServer, createServer, listenTcp, listenUds } from "./http/server.js";
import { createScheduler } from "./queue/scheduler.js";
import { FabricRouter } from "./router/resolve.js";
import type { DispatchDeps } from "./router/dispatch.js";
import { DesktopPhoneGateway, linkedPhones } from "./phone/gateway.js";
import { RouteDirector, type VendorLaneInfo } from "./routes/director.js";
import { AppRouteTasks } from "./routes/app-route-tasks.js";
import { routesForAdapterManifest, routesForAgentManifest } from "./routes/registry.js";
import { WorkerSupervisor } from "./worker/supervisor.js";
import { WorkerPool } from "./worker/pool.js";
import {
  createChromeLoginBrowser,
  createFirefoxLoginBrowser,
  importFirefoxSessionIfNewer,
} from "./worker/login_browser.js";
import { startCanary } from "./worker/canary.js";
import { startDrain } from "./worker/drain.js";
import { createWatchScheduler } from "./worker/detach.js";
import { createActivityTracker } from "./worker/progress.js";

export interface BootDeps {
  env?: NodeJS.ProcessEnv;
  keychain?: KeychainBackend;
  fetchImpl?: typeof fetch;
  logger?: (line: string) => void;
  exit?: (code: number) => void;
}

export interface RunningGateway {
  config: Config;
  db: Db;
  app: Express;
  servers: Server[];
  log: EventLog;
  outbox: CallerOutbox;
  hub: SseHub;
  notifier: Notifier;
  adapterRegistry: AdapterRegistry;
  supervisor: WorkerSupervisor;
  pool: WorkerPool;
  close(): Promise<void>;
}

function packageVersion(): string {
  const raw = readFileSync(new URL("../package.json", import.meta.url), "utf8");
  return (JSON.parse(raw) as { version?: string }).version ?? "0.1.0";
}

// Adapter ctx.log → the gateway's line log (the worker redacts first).
function lineLogger(line: (l: string) => void): RedactingLogger {
  const emit = (level: string) => (message: string, fields?: Record<string, unknown>) =>
    line(`subscription-gateway: adapter ${level}: ${message}${fields ? ` ${JSON.stringify(fields)}` : ""}`);
  return { debug: () => {}, info: emit("info"), warn: emit("warn"), error: emit("error") };
}

export async function boot(deps: BootDeps = {}): Promise<RunningGateway> {
  const logger = deps.logger ?? ((line: string) => console.log(line));
  const exit = deps.exit ?? ((code: number) => process.exit(code));
  const config = loadConfig(deps.env ?? process.env);

  let keychain: KeychainBackend;
  try {
    keychain = requireKeychain(
      deps.keychain ??
        selectKeychainBackend({ kind: config.keychainBackend, stateDir: config.stateDir })
    );
  } catch (err) {
    if (err instanceof KeychainUnavailable) {
      logger(`subscription-gateway: ${err.message}`);
      exit(1);
    }
    throw err;
  }

  const db = openDatabase(config.dbPath);

  // CLI auth bootstrap (§A6.2): issue + store the cli-token once in the
  // configured secret store; the CLI uses SUBS_GATEWAY_TOKEN or reads the
  // store back (macOS Keychain item / Sessions-machine file).
  const cliToken = ensureCliToken(db, keychain);
  if (cliToken.issued) logger("subscription-gateway: issued cli-token (stored in secret store)");

  // §A7 — adapter registry: manifests validated at boot; invalid = loud fail.
  const adapterRegistry = loadAdapterRegistry(config.adaptersDir);
  logger(`subscription-gateway: ${adapterRegistry.adapters.length} adapter(s) registered`);

  const hub = new SseHub();
  const outbox = new CallerOutbox(db);
  const log = new EventLog(db, hub);
  const notifier = new Notifier({
    apiBase: config.apiBase,
    notificationsDir: join(homedir(), ".allternit", "notifications"),
    fetchImpl: deps.fetchImpl,
    log,
  });
  log.setNotifier(notifier);

  // P3 activation — the worker layer: one supervisor (reconcile-first per
  // lane), one pool (per-lane browser runtimes, lazy — nothing launches at
  // boot), and the drain that turns scheduler enqueues into runAttempt calls.
  // The supervisor reaches pool runtimes through closures because reconcile
  // only runs at activate() time, after both exist.
  const scheduler = createScheduler();
  const router = new FabricRouter();
  let pool!: WorkerPool;
  const supervisor = new WorkerSupervisor({
    db,
    scheduler,
    adapters: (adapterId) => pool.adapterFor(adapterId),
    log,
    makeReconcileCtx: (attempt, adapter) => pool.reconcileCtx(attempt, adapter),
    stallTimeoutS: (capability) => stallTimeoutFor(config, capability),
  });
  setBrowserLanesCheck(() => browserLanesEnabled(config.stateDir));
  // Firefox logins sign in on a separate profile whose session is copied
  // into Chrome at launch; Chrome logins sign in on Chrome's own profile.
  const firefoxLogin = config.loginBrowser !== null && /firefox/i.test(config.loginBrowser);
  pool = new WorkerPool({
    db,
    registry: adapterRegistry,
    supervisor,
    profilesDir: config.stateDir,
    log,
    logger,
    sessionImport: firefoxLogin ? importFirefoxSessionIfNewer : undefined,
    laneIdleCloseMs: config.laneIdleCloseMin * 60_000,
    // Allternit Desktop on the person's own computer: browser lanes only when
    // turned on, in a hidden Google Chrome (local_desktop.ts).
    ...(isLocalDesktop()
      ? {
          hiddenChrome: () => {
            if (!browserLanesEnabled(config.stateDir)) {
              return { refuse: BROWSER_LANES_OFF };
            }
            const executable = detectGoogleChrome();
            return executable
              ? { executable }
              : { refuse: BROWSER_LANES_NEED_CHROME };
          },
        }
      : {}),
  });
  await supervisor.sweepAtBoot();
  const loginBrowser = config.loginBrowser
    ? firefoxLogin
      ? createFirefoxLoginBrowser({ executable: config.loginBrowser })
      : createChromeLoginBrowser({ executable: config.loginBrowser })
    : undefined;
  logger(
    `subscription-gateway: login browser ${config.loginBrowser ?? "unavailable (login mode disabled)"}`
  );
  const watchScheduler = createWatchScheduler();
  const activity = createActivityTracker();
  const dispatch: DispatchDeps = { db, registry: adapterRegistry, router, scheduler };

  const udsJson = (method: string, path: string, body?: unknown): Promise<{ status: number; body: Record<string, unknown> }> =>
    new Promise((resolve, reject) => {
      const token = keychain.get(CLI_TOKEN_ACCOUNT) ?? "";
      const payload = body === undefined ? undefined : JSON.stringify(body);
      const req = httpRequest(
        // x-subs-route-chosen: these turns come from vendor adapters the RouteDirector already picked.
        { socketPath: config.udsPath, path, method, headers: { host: "localhost", authorization: `Bearer ${token}`, "x-subs-route-chosen": "1", ...(payload ? { "content-type": "application/json" } : {}) } },
        (res) => {
          let raw = "";
          res.setEncoding("utf8");
          res.on("data", (c) => (raw += c));
          res.on("end", () => {
            try { resolve({ status: res.statusCode ?? 0, body: raw ? (JSON.parse(raw) as Record<string, unknown>) : {} }); }
            catch { resolve({ status: res.statusCode ?? 0, body: { error: raw.slice(0, 200) } }); }
          });
        },
      );
      req.on("error", reject);
      if (payload) req.write(payload);
      req.end();
    });

  // §4 route router: deterministic per-vendor route selection over the signed-in
  // apps. Lanes come from the YAML adapter registry plus the AAI host's vendor
  // manifests (phone adapters map to android-app / iphone-app-ictrl by id);
  // phone devices come from Allternit Desktop's phone gateway. A phone route is
  // eligible only once its conformance has passed for the current version.
  const aaiHost = createAaiHost(config, process.env, deps.fetchImpl, {
    subscriptionProfile: async (provider) => {
      const account = preferredReadyAccount(listAccounts(db), provider);
      if (!account) return null;
      await pool.deactivate({ provider: account.provider, account_id: account.account_id });
      return { dir: pool.userDataDirFor(account.profile_ref), account_id: account.account_id };
    },
    // Vendor adapters that run their turns as this gateway's own tasks (claude-subscription): the task API over
    // our UDS with the cli-token, so every rule a /v1/tasks caller meets (initiated_by, routing, mapping) applies.
    gatewayTasks: {
      submit: (body) => udsJson("POST", "/v1/tasks", body),
      get: (taskId) => udsJson("GET", `/v1/tasks/${encodeURIComponent(taskId)}`),
      cancel: (taskId) => udsJson("POST", `/v1/tasks/${encodeURIComponent(taskId)}/cancel`, {}),
      // Live task events (reply deltas, reasoning) as the worker logs them; returns unsubscribe.
      subscribe: (taskId, onEvent) =>
        hub.subscribe(taskId, createSubscriber((e) => { try { onEvent(e as { kind?: string; payload?: unknown }); } catch { /* adapter's problem */ } return true; })),
      accountState: async (provider) => {
        const all = listAccounts(db).filter((a) => a.provider === provider && a.enabled);
        const best = preferredReadyAccount(all, provider) ?? all.find((a) => a.preferred) ?? all[0];
        return best ? { health: best.session_health, remainingPct: best.usage?.remaining_pct ?? null, resetsAt: best.usage?.resets_at ?? null, agents: best.agents ?? null } : null;
      },
    },
  }, db);

  const routeDirector = new RouteDirector({
    db,
    lanes: (() => {
      let lanesCache: { at: number; lanes: VendorLaneInfo[] } | null = null;
      return {
      lanes: async (): Promise<VendorLaneInfo[]> => {
        // providers() makes live list()/capabilities() calls to every vendor
        // adapter; eligibility asks per route per turn, so cache briefly.
        const nowMs = Date.now();
        if (lanesCache && nowMs - lanesCache.at < 5_000) return lanesCache.lanes;
        const out: VendorLaneInfo[] = [];
        for (const a of adapterRegistry.adapters) {
          for (const route of routesForAdapterManifest(a.manifest)) {
            out.push({
              vendor: a.manifest.provider,
              adapterId: a.manifest.adapter_id,
              appVersion: a.manifest.adapter_version,
              routes: [route],
              // YAML web lanes are stateless Playwright bridges; reachability
              // is the account's (signedIn), so the lane itself is online.
              online: true,
            });
          }
        }
        for (const p of await aaiHost.providers()) {
          if (!p.manifest) continue;
          for (const route of routesForAgentManifest(p.manifest)) {
            out.push({
              vendor: p.manifest.vendor,
              adapterId: p.adapterId,
              appVersion: "0.0.0",
              routes: [route],
              online: !p.disabled && !p.error,
              ...(p.reliability ? { reliability: p.reliability } : {}),
            });
          }
        }
        lanesCache = { at: nowMs, lanes: out };
        return out;
      },
      };
    })(),
    // Phone routes: the linked phones Allternit Desktop reports (phone.devices
    // over its loopback phone gateway). Empty when Desktop isn't running here.
    phones: (() => {
      const gateway = new DesktopPhoneGateway({ file: process.env.SUBS_GATEWAY_PHONE_GATEWAY_FILE });
      return { linkedDevices: () => linkedPhones(gateway) };
    })(),
    onChoice: (entry) => {
      log.append({
        task_id: `route:${entry.vendor}`,
        kind: "route.choice",
        payload: entry,
        callers: [],
      });
    },
  });
  aaiHost.routeDirector = routeDirector;
  // Model-picker chats (POST /v1/tasks) on the same router (Eoj, 2026-10-17).
  const appRouteTasks = new AppRouteTasks({
    db,
    log,
    host: { call: (op, binding, input) => aaiHost.call(op, binding as never, input) as never },
    director: routeDirector,
  });

  // §2 debug-mode rule, launch-time probe: once vendor adapters have loaded,
  // record which desktop lane applies per app version from the configured
  // transport (cdp only when the shipped app accepts its debug flag
  // unmodified; an ax-configured adapter uses desktop-app-ax instead).
  // SUBS_GATEWAY_<ADAPTER>_TRANSPORT=ax selects the accessibility lane, the
  // convention claude-desktop's aai.ts already uses.
  void aaiHost.vendorsReady.then((ids) => {
    for (const adapterId of ids) {
      void aaiHost.providerManifest(adapterId).then((manifest) => {
        if (!manifest) return;
        const desktopRoutes = routesForAgentManifest(manifest).filter(
          (r) => r === "desktop-app-cdp" || r === "desktop-app-ax"
        );
        if (desktopRoutes.length === 0) return;
        const envKey = `SUBS_GATEWAY_${adapterId.replace(/-/g, "_").toUpperCase()}_TRANSPORT`;
        const transport = process.env[envKey] ?? "cdp";
        routeDirector.recordProbe(
          manifest.vendor, "desktop-app-cdp", "0.0.0", transport !== "ax",
          transport === "ax" ? `${envKey}=ax: the app uses the accessibility lane` : `${envKey} unset: the app must accept its debug flag unmodified`
        );
        routeDirector.recordProbe(
          manifest.vendor, "desktop-app-ax", "0.0.0", transport === "ax",
          transport === "ax" ? `${envKey}=ax: accessibility bridge configured` : `${envKey} unset: debug-port lane applies while the app accepts the flag`
        );
      });
    }
  }).catch(() => { /* vendorsReady already logs its own failure */ });

  const app = createServer({
    aai: aaiHost,
    db,
    config,
    keychain,
    log,
    outbox,
    hub,
    notifier,
    router,
    scheduler,
    adapterRegistry,
    pool,
    loginBrowser,
    routeDirector,
    appRouteTasks,
    version: packageVersion(),
  });

  const servers: Server[] = [];
  servers.push(await listenUds(app, config.udsPath));
  logger(`subscription-gateway: listening on unix:${config.udsPath}`);
  if (config.tcp.enabled) {
    servers.push(await listenTcp(app, config.tcp.host, config.tcp.port));
    logger(
      `subscription-gateway: listening on http://${config.tcp.host}:${config.tcp.port} (token required)`
    );
  }

  // Drain after HTTP is up: queued tasks start flowing to ready lanes.
  const stopDrain = startDrain({
    db,
    scheduler,
    pool,
    makeWorkerDeps: () => ({
      db,
      log,
      artifactsDir: config.artifactsDir,
      supervisor,
      watchScheduler,
      activity,
      dispatch,
      imageChats: config.imageChats,
      logger: lineLogger(logger),
    }),
    logger,
  });

  const stopCanary = startCanary({ db, pool, intervalMs: config.canaryIntervalSec * 1000, logger });

  return {
    config,
    db,
    app,
    servers,
    log,
    outbox,
    hub,
    notifier,
    adapterRegistry,
    supervisor,
    pool,
    async close() {
      stopDrain();
      stopCanary();
      supervisor.shutdown();
      await pool.shutdown();
      await appRouteTasks.settled();
      await notifier.drain();
      await Promise.all(servers.map((s) => closeServer(s)));
      db.pragma("wal_checkpoint(TRUNCATE)");
      db.close();
    },
  };
}

async function main(): Promise<void> {
  const gateway = await boot();
  const shutdown = () => {
    void gateway.close().then(() => process.exit(0));
  };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
}

// Compare real paths: started through a symlink (macOS /var -> /private/var,
// a Windows 8.3 short name) the two spellings differ and the gateway would
// exit 0 without booting.
function invokedAsMain(): boolean {
  if (typeof process.argv[1] !== "string") return false;
  if (import.meta.url === pathToFileURL(process.argv[1]).href) return true;
  try {
    return realpathSync(fileURLToPath(import.meta.url)) === realpathSync(process.argv[1]);
  } catch {
    return false;
  }
}
const invokedDirectly = invokedAsMain();
if (invokedDirectly) {
  void main();
}
