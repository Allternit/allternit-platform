// Boot — order matters: config → secret-store gate (D3/D15: refuse without
// it) → store+migrations → events/http wiring → UDS (+TCP if enabled) → log binds.
import { readFileSync } from "node:fs";
import type { Server } from "node:http";
import { homedir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import type { Express } from "express";
import type { RedactingLogger } from "@allternit/subscription-fabric-contracts";
import { loadConfig, stallTimeoutFor, type Config } from "./config.js";
import { loadAdapterRegistry, type AdapterRegistry } from "./adapters/registry.js";
import { openDatabase, type Db } from "./store/db.js";
import { listAccounts, preferredReadyAccount } from "./store/queries.js";
import { EventLog } from "./events/log.js";
import { SseHub } from "./events/sse.js";
import { CallerOutbox } from "./events/outbox.js";
import { Notifier } from "./events/notify.js";
import {
  KeychainUnavailable,
  requireKeychain,
  selectKeychainBackend,
  type KeychainBackend,
} from "./security/keychain.js";
import { ensureCliToken } from "./security/tokens.js";
import { createAaiHost } from "./aai/registry.js";
import { closeServer, createServer, listenTcp, listenUds } from "./http/server.js";
import { createScheduler } from "./queue/scheduler.js";
import { FabricRouter } from "./router/resolve.js";
import type { DispatchDeps } from "./router/dispatch.js";
import { WorkerSupervisor } from "./worker/supervisor.js";
import { WorkerPool } from "./worker/pool.js";
import {
  createChromeLoginBrowser,
  createFirefoxLoginBrowser,
  importFirefoxSessionIfNewer,
} from "./worker/login_browser.js";
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

  const app = createServer({
    aai: createAaiHost(config, process.env, deps.fetchImpl, {
      subscriptionProfile: async (provider) => {
        const account = preferredReadyAccount(listAccounts(db), provider);
        if (!account) return null;
        await pool.deactivate({ provider: account.provider, account_id: account.account_id });
        return { dir: pool.userDataDirFor(account.profile_ref), account_id: account.account_id };
      },
    }),
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
      supervisor.shutdown();
      await pool.shutdown();
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

const invokedDirectly =
  typeof process.argv[1] === "string" &&
  import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  void main();
}
