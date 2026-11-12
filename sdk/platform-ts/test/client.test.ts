// Tests against a mocked HTTP server. Run: node --test test/*.test.ts (Node 22.6+ strips the types).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type IncomingMessage, type ServerResponse, type Server } from "node:http";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  AllternitPlatform,
  AuthenticationError,
  ConflictError,
  InvalidRequestError,
  NotFoundError,
  PermissionError,
  RateLimitError,
  InternalServerError,
  APIError,
  APITimeoutError,
} from "../src/index.ts";

type Seen = { method: string; url: string; headers: IncomingMessage["headers"]; body: unknown };
const seen: Seen[] = [];
let server: Server;
let base = "";

function json(res: ServerResponse, status: number, body: unknown, headers: Record<string, string> = {}) {
  res.writeHead(status, { "content-type": "application/json", ...headers });
  res.end(JSON.stringify(body));
}

const msg = (content: string) => ({
  id: "cmsg_1", object: "conversation.message", conversation_id: "conv_1", role: "assistant",
  content, status: "completed", error: null, created_at: "2026-10-07T00:00:00Z",
});

before(async () => {
  server = createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const body = raw ? JSON.parse(raw) : undefined;
      seen.push({ method: req.method!, url: req.url!, headers: req.headers, body });
      const url = new URL(req.url!, "http://x");
      const p = url.pathname;
      if (p === "/v1/agents" && req.method === "POST") return json(res, 201, { id: "agent_1", object: "agent", ...body });
      if (p === "/v1/agents" && req.method === "GET") {
        const after = url.searchParams.get("after");
        if (!after) return json(res, 200, { data: [{ id: "agent_1" }, { id: "agent_2" }], has_more: true, next_cursor: "c2" });
        if (after === "c2") return json(res, 200, { data: [{ id: "agent_3" }], has_more: false, next_cursor: null });
      }
      if (p.startsWith("/v1/errors/")) {
        const status = Number(p.split("/").pop());
        return json(res, status, { error: { type: "x_error", code: `code_${status}`, message: `boom ${status}`, param: status === 400 ? "name" : null } },
          { "x-request-id": "req_123", ...(status === 429 ? { "retry-after": "7" } : {}) });
      }
      if (p === "/v1/agents/agent_missing") return json(res, 404, { error: { type: "not_found_error", code: "agent_not_found", message: "No such agent.", param: null } });
      if (p === "/v1/slow") return setTimeout(() => json(res, 200, {}), 500);
      if (p === "/v1/conversations/conv_1/messages") {
        if (!body.stream) return json(res, 200, msg("Hi there"));
        res.writeHead(200, { "content-type": "text/event-stream" });
        if (body.content === "fail") {
          res.write('event: message.delta\ndata: {"delta":"Hal"}\n\n');
          res.end('event: error\ndata: {"error":{"type":"api_error","code":"turn_failed","message":"the agent\'s turn failed"}}\n\n');
          return;
        }
        res.write(": keep-alive\n\n");
        res.write('event: message.delta\ndata: {"delta":"Hi "}\n\n');
        res.write('event: message.delta\r\ndata: {"delta":"there"}\r\n\r\n');
        res.end(`event: message.completed\ndata: ${JSON.stringify(msg("Hi there"))}\n\n`);
        return;
      }
      if (p === "/v1/decisions" && req.method === "POST") return json(res, 200, { id: "dec_1", object: "decision", answers: [{ name: "ok", type: "predicate", probability: 0.9, confidence: 0.9 }] });
      if (p === "/v1/responses" && req.method === "POST") {
        if (!body.stream) return json(res, 200, { id: "resp_1", object: "response", status: "completed", output: [] });
        res.writeHead(200, { "content-type": "text/event-stream" });
        res.end('event: response.output_text.delta\ndata: {"delta":"Hi"}\n\nevent: response.completed\ndata: {"id":"resp_1"}\n\n');
        return;
      }
      if (p === "/v1/messages" && req.method === "POST") return json(res, 200, { id: "mdl_1", type: "message", role: "assistant", content: [{ type: "text", text: "Hi" }], stop_reason: "end_turn" });
      if (p === "/v1/texts" && req.method === "POST") return json(res, 201, { id: "msg_1", object: "text", status: "simulated" });
      if (p === "/v1/conversations/conv_1/items" && req.method === "GET") return json(res, 200, { data: [{ id: "item_1", object: "item" }], has_more: false, next_cursor: null });
      if (p === "/v1/approvals/apr_1" && req.method === "POST") return json(res, 200, { id: "apr_1", object: "approval", status: "approved" });
      json(res, 404, { error: { type: "not_found_error", code: "route", message: "no route", param: null } });
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
  const addr = server.address();
  base = `http://127.0.0.1:${typeof addr === "object" && addr ? addr.port : 0}`;
});

after(() => new Promise<void>((r) => server.close(() => r())));

const client = () => new AllternitPlatform({ apiKey: "alt_test_abc", baseUrl: base });

test("sends the bearer key and JSON body", async () => {
  const agent = await client().agents.create({ account_id: "acct_1", name: "Front desk" });
  assert.equal(agent.id, "agent_1");
  const last = seen.at(-1)!;
  assert.equal(last.headers.authorization, "Bearer alt_test_abc");
  assert.equal(last.headers["content-type"], "application/json");
  assert.deepEqual(last.body, { account_id: "acct_1", name: "Front desk" });
});

test("reads the key from ALLTERNIT_API_KEY and refuses to start without one", async () => {
  const prev = process.env.ALLTERNIT_API_KEY;
  try {
    process.env.ALLTERNIT_API_KEY = "alt_test_fromenv";
    await new AllternitPlatform({ baseUrl: base }).agents.get("agent_missing").catch(() => undefined);
    assert.equal(seen.at(-1)!.headers.authorization, "Bearer alt_test_fromenv");
    delete process.env.ALLTERNIT_API_KEY;
    assert.throws(() => new AllternitPlatform({ baseUrl: base }), /No API key/);
  } finally {
    if (prev === undefined) delete process.env.ALLTERNIT_API_KEY; else process.env.ALLTERNIT_API_KEY = prev;
  }
});

test("adds a random Idempotency-Key to every POST, and uses yours when given", async () => {
  const c = client();
  await c.agents.create({ account_id: "a", name: "n" });
  const k1 = seen.at(-1)!.headers["idempotency-key"] as string;
  await c.agents.create({ account_id: "a", name: "n" });
  const k2 = seen.at(-1)!.headers["idempotency-key"] as string;
  assert.match(k1, /^[0-9a-f-]{36}$/);
  assert.notEqual(k1, k2);
  await c.agents.create({ account_id: "a", name: "n" }, { idempotencyKey: "mine-1" });
  assert.equal(seen.at(-1)!.headers["idempotency-key"], "mine-1");
  await c.agents.list();
  assert.equal(seen.at(-1)!.headers["idempotency-key"], undefined, "GET carries no idempotency key");
});

test("maps error statuses to typed errors", async () => {
  const c = client();
  const cases: Array<[number, Function]> = [
    [400, InvalidRequestError], [401, AuthenticationError], [403, PermissionError], [404, NotFoundError],
    [409, ConflictError], [422, InvalidRequestError], [429, RateLimitError], [503, InternalServerError], [418, APIError],
  ];
  for (const [status, cls] of cases) {
    const err = await c.http.request({ method: "GET", path: `/v1/errors/${status}` }).then(() => null, (e) => e);
    assert.ok(err instanceof cls, `status ${status} -> ${cls.name}, got ${err?.constructor?.name}`);
    assert.ok(err instanceof APIError);
    assert.equal(err.status, status);
    assert.equal(err.code, `code_${status}`);
    assert.equal(err.message, `boom ${status}`);
    assert.equal(err.requestId, "req_123");
    if (status === 400) assert.equal(err.param, "name");
    if (status === 429) assert.equal((err as RateLimitError).retryAfter, 7);
  }
  const nf = await c.agents.get("agent_missing").catch((e) => e);
  assert.ok(nf instanceof NotFoundError);
  assert.equal(nf.code, "agent_not_found");
  assert.equal(nf.type, "not_found_error");
});

test("times out", async () => {
  const err = await client().http.request({ method: "GET", path: "/v1/slow", options: { timeout: 50 } }).catch((e) => e);
  assert.ok(err instanceof APITimeoutError);
});

test("returns pages and auto-pages through every item", async () => {
  const c = client();
  const page = await c.agents.list({ limit: 2 });
  assert.equal(page.has_more, true);
  assert.equal(page.next_cursor, "c2");
  assert.match(seen.at(-1)!.url, /limit=2/);
  const ids: string[] = [];
  for await (const a of c.agents.listAll({ limit: 2 })) ids.push(a.id);
  assert.deepEqual(ids, ["agent_1", "agent_2", "agent_3"]);
  assert.match(seen.at(-1)!.url, /after=c2/);
});

test("sends a message without streaming", async () => {
  const m = await client().conversations.sendMessage("conv_1", { content: "Hello" });
  assert.equal(m.content, "Hi there");
  assert.deepEqual(seen.at(-1)!.body, { content: "Hello" });
});

test("streams deltas then the completed message", async () => {
  const stream = client().conversations.stream("conv_1", { content: "Hello" });
  const events = [];
  for await (const ev of stream) events.push(ev);
  assert.deepEqual(seen.at(-1)!.body, { content: "Hello", stream: true });
  assert.equal(seen.at(-1)!.headers.accept, "text/event-stream");
  assert.deepEqual(events.slice(0, 2), [{ type: "message.delta", delta: "Hi " }, { type: "message.delta", delta: "there" }]);
  assert.equal(events[2].type, "message.completed");
  assert.equal(stream.receivedText, "Hi there");
  assert.equal((await stream.finalMessage()).content, "Hi there");
  const direct = await client().conversations.stream("conv_1", { content: "again" }).finalMessage();
  assert.equal(direct.id, "cmsg_1");
});

test("an error event in the stream throws a typed error", async () => {
  const deltas: string[] = [];
  const err = await (async () => {
    for await (const ev of client().conversations.stream("conv_1", { content: "fail" })) {
      if (ev.type === "message.delta") deltas.push(ev.delta);
    }
  })().catch((e) => e);
  assert.deepEqual(deltas, ["Hal"]);
  assert.ok(err instanceof APIError);
  assert.equal(err.code, "turn_failed");
  assert.equal(err.type, "api_error");
});

test("decisions.create posts typed questions", async () => {
  const d = await client().decisions.create({ input: "Box arrived crushed", questions: [{ name: "ok", type: "predicate", instructions: "Damaged?" }] } as any);
  assert.equal(d.id, "dec_1");
  assert.equal(seen.at(-1)!.url, "/v1/decisions");
  assert.equal((seen.at(-1)!.body as any).questions[0].name, "ok");
});

test("responses.create and createStream", async () => {
  const r = await client().responses.create({ model: "allternit/auto", input: "hi" } as any);
  assert.equal(r.status, "completed");
  const events = [];
  for await (const ev of client().responses.createStream({ model: "allternit/auto", input: "hi" } as any)) events.push(ev);
  assert.equal((seen.at(-1)!.body as any).stream, true);
  assert.equal(events.length, 2);
});

test("messages.create uses the Anthropic shape", async () => {
  const m = await client().messages.create({ model: "allternit/auto", messages: [{ role: "user", content: "hi" }], max_tokens: 64 } as any);
  assert.equal(m.id, "mdl_1");
  assert.equal(seen.at(-1)!.url, "/v1/messages");
});

test("texts.create posts to /v1/texts", async () => {
  const t = await client().texts.create({ number_id: "n1", to: "+14155550123", body: "Hi" } as any);
  assert.equal(t.object, "text");
  assert.equal(seen.at(-1)!.url, "/v1/texts");
});

test("conversations.listItems reads the items page", async () => {
  const page = await client().conversations.listItems("conv_1");
  assert.equal(page.data[0].id, "item_1");
  assert.match(seen.at(-1)!.url, /^\/v1\/conversations\/conv_1\/items/);
});

test("approvals.resolve sends the decision", async () => {
  const a = await client().approvals.resolve("apr_1", { decision: "approve" });
  assert.equal(a.status, "approved");
  assert.deepEqual(seen.at(-1)!.body, { decision: "approve" });
});

test("the generated code matches the OpenAPI file (generate.py --check)", () => {
  const script = fileURLToPath(new URL("../../../scripts/platform-sdk/generate.py", import.meta.url));
  const out = execFileSync("python3", [script, "--check"], { encoding: "utf8" });
  assert.match(out, /up to date/);
});
