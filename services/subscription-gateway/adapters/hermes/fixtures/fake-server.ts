// Offline fake Hermes gateway: a real local HTTP server speaking the OpenAI-compatible subset the adapter uses.
// Shapes follow the OpenAI chat-completions spec (which Hermes's gateway mirrors); no Hermes code or network involved.
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

export interface FakeHermesOptions {
  models?: string[] | "absent";       // "absent" => GET /v1/models answers 404
  token?: string;                      // when set, requests without the matching bearer get 401
  fault?: "rate_limit" | "unauthorized" | "not_found" | "server_error";
  chunkDelayMs?: number;               // gap between SSE chunks
  chunks?: number;                     // chunks per reply (default 3)
  jsonOnly?: boolean;                  // ignore stream:true and answer plain JSON
}
export interface FakeHermes {
  url: string; close(): Promise<void>;
  requests: Array<{ path: string; body?: { model?: string; messages?: Array<{ role: string; content: string }>; stream?: boolean }; auth?: string }>;
  /** number of chat requests whose client connection closed before the reply finished */
  aborted: number;
}

const readBody = (req: IncomingMessage) => new Promise<string>((res) => { let b = ""; req.on("data", (d) => (b += d)); req.on("end", () => res(b)); });

export async function startFakeHermes(opts: FakeHermesOptions = {}): Promise<FakeHermes> {
  const state: FakeHermes = { url: "", requests: [], aborted: 0, close: async () => undefined };
  const server: Server = createServer(async (req: IncomingMessage, res: ServerResponse) => {
    const raw = req.method === "POST" ? await readBody(req) : "";
    const path = req.url ?? "";
    const body = raw ? JSON.parse(raw) : undefined;
    state.requests.push({ path, body, auth: req.headers.authorization });
    const json = (code: number, o: unknown, h: Record<string, string> = {}) => { res.writeHead(code, { "content-type": "application/json", ...h }); res.end(JSON.stringify(o)); };
    if (opts.fault === "rate_limit") return json(429, { error: "slow down" }, { "retry-after": "2" });
    if (opts.fault === "unauthorized") return json(401, { error: "bad token" });
    if (opts.fault === "not_found") return json(404, { error: "not found" });
    if (opts.fault === "server_error") return json(500, { error: "boom" });
    if (opts.token && req.headers.authorization !== `Bearer ${opts.token}`) return json(401, { error: "unauthorized" });
    if (path === "/v1/models") {
      if (opts.models === "absent") return json(404, { error: "no models endpoint" });
      return json(200, { object: "list", data: (opts.models ?? ["hermes-agent"]).map((id) => ({ id, object: "model" })) });
    }
    if (path === "/v1/chat/completions" && req.method === "POST") {
      const users = (body.messages as Array<{ role: string; content: string }>).filter((m) => m.role === "user");
      // Echo reveals how much history the client replayed, so tests can prove isolation and continuity.
      const reply = `echo[${users.length}]: ${users[users.length - 1]?.content ?? ""}`;
      if (!body.stream || opts.jsonOnly) return json(200, { choices: [{ message: { role: "assistant", content: reply } }] });
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      const n = opts.chunks ?? 3, size = Math.ceil(reply.length / n);
      let done = false;
      res.on("close", () => { if (!done) state.aborted += 1; });
      for (let k = 0; k < n; k++) {
        if (res.destroyed) return;
        res.write(`data: ${JSON.stringify({ choices: [{ delta: { content: reply.slice(k * size, (k + 1) * size) } }] })}\n\n`);
        if (opts.chunkDelayMs) await new Promise((r) => setTimeout(r, opts.chunkDelayMs));
      }
      done = true;
      res.write("data: [DONE]\n\n"); res.end();
      return;
    }
    json(404, { error: "unknown route" });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  state.url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  state.close = () => new Promise<void>((r) => { server.closeAllConnections?.(); server.close(() => r()); });
  return state;
}
