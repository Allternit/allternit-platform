// §A6.1 — express app factory + transport guards. UDS default (0600), TCP
// optional and always token-authed, strict Host validation (DNS-rebinding
// defense), Origin rejected unless explicitly allowlisted, never CORS *.
import express, { type Express, type Request, type RequestHandler } from "express";
import { chmodSync, mkdirSync, unlinkSync } from "node:fs";
import { dirname } from "node:path";
import type { Server } from "node:http";
import type { Config } from "../config.js";
import type { Db } from "../store/db.js";
import type { KeychainBackend } from "../security/keychain.js";
import { verifyToken, type TokenScope, type VerifiedCaller } from "../security/tokens.js";
import type { EventLog } from "../events/log.js";
import type { CallerOutbox } from "../events/outbox.js";
import type { SseHub } from "../events/sse.js";
import type { Notifier } from "../events/notify.js";
import type { CapabilityRouter } from "@allternit/subscription-fabric-contracts";
import type { AdapterRegistry } from "../adapters/registry.js";
import type { Scheduler } from "../queue/scheduler.js";
import type { WorkerPool } from "../worker/pool.js";
import type { LoginBrowser } from "../worker/login_browser.js";
import { tasksRouter } from "./routes_tasks.js";
import { eventsRouter } from "./routes_events.js";
import { artifactsRouter } from "./routes_artifacts.js";
import { accountsRouter, type AccountsRouterOptions } from "./routes_accounts.js";
import { capabilitiesRouter } from "./routes_capabilities.js";
import { catalogRouter } from "./routes_catalog.js";
import { statsRouter } from "./routes_stats.js";

export interface GatewayDeps {
  db: Db;
  config: Config;
  keychain: KeychainBackend;
  log: EventLog;
  outbox: CallerOutbox;
  hub: SseHub;
  notifier: Notifier;
  router?: CapabilityRouter;
  scheduler?: Scheduler; // P3 — POST /v1/tasks enqueues; workers drain per key
  adapterRegistry?: AdapterRegistry; // P3 — live /v1/capabilities view
  pool?: WorkerPool; // P3 activation — POST /v1/accounts/:id/connect activates lanes
  loginBrowser?: LoginBrowser; // login mode — POST /v1/accounts/:id/login
  accountsOptions?: AccountsRouterOptions; // login watcher poll + cookie reader (tests)
  allowedOrigins?: string[]; // default: empty — every Origin is rejected
  version?: string;
}

export interface AuthedRequest extends Request {
  caller: VerifiedCaller;
}

export function callerOf(req: Request): VerifiedCaller {
  return (req as AuthedRequest).caller;
}

// §A6.1 — Host must be loopback, with no port or the port the connection
// actually arrived on (UDS carries the bare `localhost` placeholder).
function hostGuard(): RequestHandler {
  return (req, res, next) => {
    const host = req.headers.host ?? "";
    const m = /^([^:]+)(?::(\d+))?$/.exec(host);
    const name = m?.[1];
    const port = m?.[2] ? Number(m[2]) : undefined;
    const loopback = name === "127.0.0.1" || name === "localhost";
    const portOk = port === undefined || port === req.socket.localPort;
    if (!loopback || !portOk) {
      res.status(403).json({ error: "forbidden_host" });
      return;
    }
    next();
  };
}

function originGuard(allowedOrigins: string[]): RequestHandler {
  return (req, res, next) => {
    const origin = req.headers.origin;
    if (origin !== undefined && !allowedOrigins.includes(origin)) {
      res.status(403).json({ error: "forbidden_origin" });
      return;
    }
    next();
  };
}

function bearerAuth(db: Db): RequestHandler {
  return (req, res, next) => {
    const header = req.headers.authorization ?? "";
    const m = /^Bearer\s+(.+)$/.exec(header);
    const caller = m ? verifyToken(db, m[1]) : null;
    if (!caller) {
      res.status(401).json({ error: "unauthorized" });
      return;
    }
    (req as AuthedRequest).caller = caller;
    next();
  };
}

export function requireScope(...scopes: TokenScope[]): RequestHandler {
  return (req, res, next) => {
    const caller = callerOf(req);
    if (!scopes.some((s) => caller.scopes.includes(s))) {
      res.status(403).json({ error: "forbidden_scope", required: scopes });
      return;
    }
    next();
  };
}

export function createServer(deps: GatewayDeps): Express {
  const app = express();
  app.disable("x-powered-by");
  // 5 MB: image.generate may carry a reference photo inline (base64).
  app.use(express.json({ limit: "5mb" }));
  app.use(hostGuard());
  app.use(originGuard(deps.allowedOrigins ?? []));

  app.get("/v1/health", (_req, res) => {
    res.json({
      ok: true,
      name: "subscription-gateway",
      version: deps.version ?? "0.1.0",
    });
  });

  app.use(bearerAuth(deps.db));
  app.use(tasksRouter(deps));
  app.use(eventsRouter(deps));
  app.use(artifactsRouter(deps));
  app.use(accountsRouter(deps, deps.accountsOptions));
  app.use(capabilitiesRouter(deps));
  app.use(catalogRouter(deps));
  app.use(statsRouter(deps));

  app.use((_req, res) => res.status(404).json({ error: "not_found" }));
  return app;
}

// UDS default transport: unlink any stale socket, bind, then chmod 0600.
export function listenUds(app: Express, udsPath: string): Promise<Server> {
  mkdirSync(dirname(udsPath), { recursive: true });
  try {
    unlinkSync(udsPath);
  } catch {
    // no stale socket — fine
  }
  return new Promise((resolve, reject) => {
    const server = app.listen(udsPath, () => {
      chmodSync(udsPath, 0o600);
      resolve(server);
    });
    server.on("error", reject);
  });
}

export function listenTcp(app: Express, host: string, port: number): Promise<Server> {
  return new Promise((resolve, reject) => {
    const server = app.listen(port, host, () => resolve(server));
    server.on("error", reject);
  });
}

export function closeServer(server: Server): Promise<void> {
  return new Promise((resolve, reject) => {
    server.close((err) => (err ? reject(err) : resolve()));
  });
}
