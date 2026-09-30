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

export function createHandler(opts: ServeOptions = {}) {
  const engine = opts.engine ?? new SystemOne();
  const token = opts.token ?? process.env.SYSTEM_ONE_TOKEN;
  const maxInflight = opts.maxInflight ?? Number(process.env.SYSTEM_ONE_MAX_INFLIGHT ?? 8);
  let inflight = 0;
  const decision = opts.decision ?? new DecisionRouter({
    provider: new LocalLogitReadoutProvider(engine, {
      model_ref: engine.config.runtimeModel, model_revision: "unpinned", tokenizer_id: "unknown", quantization: "unknown", runtime_backend: engine.config.runtimeUrl.includes(":11434") ? "ollama" : "openai-compat",
    }),
    manifests: [],
  });

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
      try {
        return json(200, await decision.decide(b.request, b.state, { reversible: b.reversible === true }));
      } catch (e) {
        return err(500, "api_error", (e as Error).message);
      }
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
