// HTTP server. Binds 127.0.0.1 ONLY (the host is not configurable on purpose);
// the port is configurable (default 7717).
//
//   POST /v1/systemone   System One contract
//   GET  /v1/models      model list
//   GET  /healthz        liveness
//
// Auth: none by default (loopback only). If SYSTEM_ONE_TOKEN is set, requests
// need `Authorization: Bearer <token>` or get 401.
import { SystemOne } from "./engine.ts";
import { DecisionRouter } from "./decision/router.ts";
import { LocalLogitReadoutProvider } from "./decision/local-provider.ts";
import { ShadowLedger } from "./decision/shadow.ts";
import { BASE_DIR } from "./log.ts";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import { CanaryController } from "./decision/canary.ts";
import { SystemOneError, type ErrorBody } from "./types.ts";

export const HOST = "127.0.0.1";
export const DEFAULT_PORT = 7717;

export interface ServeOptions {
  port?: number;
  engine?: SystemOne;
  token?: string;
  maxInflight?: number;
  /** Canonical ABI decision router. Default: shadow-only, no manifests (refuses uncalibrated S1). */
  decision?: DecisionRouter;
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const err = (status: number, type: ErrorBody["error"]["type"], message: string) =>
  json(status, { error: { type, message } } satisfies ErrorBody);

/**
 * Calibration manifests from ALLTERNIT_S1_MANIFESTS (a JSON array of
 * DecisionCalibrationManifestV1) — the same source the gizzi ModelPool reads,
 * so the pool entry and the runtime agree. Unreadable = none (fail closed).
 */
export function loadManifests(path = process.env.ALLTERNIT_S1_MANIFESTS?.trim()): any[] {
  if (!path) return [];
  try {
    const parsed = JSON.parse(require("node:fs").readFileSync(path, "utf8"));
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/** Shadow ledger is opt-in (ALLTERNIT_S1_SHADOW_DIR, or SYSTEM_ONE_SHADOW_LOG=1 for the default dir) so tests never write to $HOME. */
/** Q29 pinned base revision (same pin as laya/serve-laya.sh and Desktop SystemOneManager). */
export const LAYA_PINNED_REVISION = "55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851";

/**
 * The revision Laya is serving, resolved with Desktop's precedence (env path/revision,
 * then <LAYA_HOME>/config.json {checkpoint}, then the Q29 pin). A local fine-tuned
 * checkpoint is identified by its allternit_checkpoint.json revision. Every revision is a
 * distinct backend identity (Q29), so shadow rows and manifests never mix across checkpoints.
 */
export function layaRevision(env: Record<string, string | undefined> = process.env): string {
  const fromPath = (p: string) => {
    try {
      const m = JSON.parse(readFileSync(join(p, "allternit_checkpoint.json"), "utf8")) as { revision?: unknown };
      if (typeof m.revision === "string" && m.revision) return m.revision;
    } catch { /* not one of ours */ }
    return `local:${basename(p)}`;
  };
  const path = env.ALLTERNIT_LAYA_CHECKPOINT_PATH?.trim() || env.LAYA_CHECKPOINT_PATH?.trim();
  if (path) return fromPath(path);
  const rev = env.ALLTERNIT_LAYA_REVISION?.trim() || env.LAYA_REVISION?.trim();
  if (rev) return rev;
  const home = env.ALLTERNIT_LAYA_HOME?.trim() || env.LAYA_HOME?.trim() || join(homedir(), "Library", "Application Support", "Allternit", "laya");
  const cfgPath = join(home, "config.json");
  if (existsSync(cfgPath)) {
    try {
      const cp = (JSON.parse(readFileSync(cfgPath, "utf8")) as { checkpoint?: { path?: unknown; revision?: unknown } }).checkpoint ?? {};
      if (typeof cp.path === "string" && cp.path.trim()) return fromPath(cp.path.trim());
      if (typeof cp.revision === "string" && cp.revision.trim()) return cp.revision.trim();
    } catch { /* fall through to the pin */ }
  }
  return LAYA_PINNED_REVISION;
}

/** Live mode only: the Q26 canary state (ALLTERNIT_S1_CANARY, default <base>/canary.json). */
export function canaryController(mode: string): CanaryController | undefined {
  if (mode !== "live") return undefined;
  return new CanaryController(process.env.ALLTERNIT_S1_CANARY?.trim() || join(BASE_DIR, "canary.json"));
}

export function shadowLedger(): ShadowLedger | undefined {
  const dir = process.env.ALLTERNIT_S1_SHADOW_DIR?.trim();
  if (dir) return new ShadowLedger(dir);
  if (process.env.SYSTEM_ONE_SHADOW_LOG === "1") return new ShadowLedger(join(BASE_DIR, "shadow"));
  return undefined;
}

export function createHandler(opts: ServeOptions = {}) {
  const engine = opts.engine ?? new SystemOne();
  const token = opts.token ?? process.env.SYSTEM_ONE_TOKEN;
  const maxInflight = opts.maxInflight ?? Number(process.env.SYSTEM_ONE_MAX_INFLIGHT ?? 8);
  let inflight = 0;
  const ledger = shadowLedger();
  const manifests = loadManifests();
  const mode = process.env.ALLTERNIT_S1_MODE === "live" ? "live" : "shadow";
  const canary = canaryController(mode);
  const routerFor = (provider: LocalLogitReadoutProvider) => new DecisionRouter({ provider, manifests, ledger, mode, canary });
  // One router per S1 backend (the routing policy's s1_backend, sent as `backend`).
  // Each has its own backend_id, so shadow rows and calibration never mix (Q22).
  const local = opts.decision ?? routerFor(new LocalLogitReadoutProvider(engine, {
    model_ref: engine.config.runtimeModel, model_revision: "unpinned", tokenizer_id: "unknown", quantization: "unknown", runtime_backend: engine.config.runtimeUrl.includes(":11434") ? "ollama" : "openai-compat",
  }));
  // Re-resolved at most every 10 s, so a Desktop checkpoint swap (setCheckpoint restarts
  // only Laya) changes the identity S1 logs and binds manifests to.
  let layaRev = { value: layaRevision(), at: Date.now() };
  const cachedLayaRevision = () => {
    if (Date.now() - layaRev.at > 10_000) layaRev = { value: layaRevision(), at: Date.now() };
    return layaRev.value;
  };
  const laya = opts.decision ?? routerFor(new LocalLogitReadoutProvider(engine, {
    model_ref: `convaiinnovations/laya/${engine.config.layaModel}`, get model_revision() { return cachedLayaRevision(); }, tokenizer_id: "modernbert", quantization: "none", runtime_backend: "laya-serve",
  }, "backend.laya", `laya:${engine.config.layaModel}`));
  const jev = opts.decision ?? (engine.typesafe ? routerFor(new LocalLogitReadoutProvider(engine, {
    model_ref: "typesafe/jev-latest", model_revision: "remote", tokenizer_id: "unknown", quantization: "unknown", runtime_backend: "typesafe",
  }, "backend.jev_api", "typesafe:jev-latest")) : undefined);
  // "auto" (allternit-api's default when no policy names a backend): Laya while
  // it answers its health check, else the local engine. Re-probed at most every 10 s.
  let layaUp = { ok: false, at: 0 };
  const layaHealthy = async () => {
    if (Date.now() - layaUp.at < 10_000) return layaUp.ok;
    let ok = false;
    try { ok = (await fetch(`${engine.config.layaUrl}/health`, { signal: AbortSignal.timeout(800) })).ok; } catch { ok = false; }
    layaUp = { ok, at: Date.now() };
    return ok;
  };
  const decisionFor = async (backend: unknown): Promise<DecisionRouter | undefined> =>
    backend === "laya_bundled" ? laya : backend === "jev_api" ? jev
      : backend === "auto" && !opts.decision && (await layaHealthy()) ? laya : local;

  return async function handle(req: Request): Promise<Response> {
    const url = new URL(req.url);
    if (url.pathname === "/healthz") return json(200, { ok: true, model: engine.local.model });
    if (token && req.headers.get("authorization") !== `Bearer ${token}`) {
      return err(401, "authentication_error", "missing or invalid bearer token");
    }
    if (url.pathname === "/v1/models" && req.method === "GET") return json(200, engine.models());
    if (url.pathname === "/v1/decision" && req.method === "POST") {
      // Canonical ABI route: body {request: DecisionRequestV1, state: string, reversible?: boolean} -> DecisionResultV1
      let b: any;
      try { b = await req.json(); } catch { return err(422, "invalid_request_error", "body is not valid JSON"); }
      if (!b?.request?.operation || typeof b.state !== "string") return err(422, "invalid_request_error", "need {request: DecisionRequestV1, state: string}");
      const decision = await decisionFor(b.backend);
      if (!decision) return err(401, "authentication_error", "jev_api backend requested but TYPESAFE_API_KEY is not set");
      try {
        return json(200, await decision.decide(b.request, b.state, { reversible: b.reversible === true }));
      } catch (e) {
        return err(500, "api_error", (e as Error).message);
      }
    }
    if (url.pathname === "/v1/decision/outcome" && req.method === "POST") {
      // Ground truth from deterministic code: {decision_id|subject_ref, question_id?, truth, source}
      if (!ledger) return err(409, "invalid_request_error", "shadow ledger disabled (set ALLTERNIT_S1_SHADOW_DIR)");
      let b: any;
      try { b = await req.json(); } catch { return err(422, "invalid_request_error", "body is not valid JSON"); }
      try { return json(200, ledger.recordOutcome({ decision_id: b?.decision_id, subject_ref: b?.subject_ref, question_id: b?.question_id, truth: b?.truth, source: b?.source })); }
      catch (e) { return err(422, "invalid_request_error", (e as Error).message); }
    }
    if (url.pathname !== "/v1/systemone") return err(404, "not_found_error", `no route ${req.method} ${url.pathname}`);
    if (req.method !== "POST") return err(404, "not_found_error", "use POST /v1/systemone");
    if (inflight >= maxInflight) return err(429, "rate_limit_error", `too many concurrent requests (max ${maxInflight}); retry with backoff`);
    let body: unknown;
    try {
      body = await req.json();
    } catch {
      return err(422, "invalid_request_error", "body is not valid JSON");
    }
    inflight++;
    try {
      return json(200, await engine.evaluate(body));
    } catch (e) {
      if (e instanceof SystemOneError) return json(e.status, e.body);
      return err(500, "api_error", (e as Error).message);
    } finally {
      inflight--;
    }
  };
}

export function serve(opts: ServeOptions = {}) {
  const port = opts.port ?? Number(process.env.SYSTEM_ONE_PORT ?? DEFAULT_PORT);
  return Bun.serve({ hostname: HOST, port, fetch: createHandler(opts), idleTimeout: 255 });
}
