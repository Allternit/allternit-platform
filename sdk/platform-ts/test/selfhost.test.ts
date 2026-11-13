// Self-hosted client and custom-tool worker against a mocked Agents Server.
// Run: node --test test/*.test.ts (Node 22.6+ strips the types).
import { test, before, after, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { createServer, type IncomingMessage, type ServerResponse, type Server } from "node:http";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { SelfHostClient, runCustomToolWorker, isTurnEnd } from "../src/index.ts";
import { MAX_TOOL_RESULT_BYTES } from "../src/selfhost/index.ts";

const SECRET = "sess-secret-123";
type Call = { tool_call_id: string; name: string; input: Record<string, unknown> };
const state = {
  claims: [] as unknown[],
  calls: [] as Call[],
  staleOnNext: false,
  results: [] as Array<[string, { output: string; is_error: boolean }]>,
  completed: [] as Array<[string, unknown]>,
  released: [] as string[],
  secretHeaders: [] as Array<string | undefined>,
  lastEventIds: [] as Array<string | undefined>,
  streamRound: 0,
  emptyWaits: 0, // plain 204s to send before the closing one
  workerHeaders: [] as Array<[string | undefined, string | undefined]>,
  completesBeforeClose: 0,
};

function reset(p: Partial<typeof state> = {}) {
  Object.assign(state, {
    claims: [], calls: [], staleOnNext: false, results: [], completed: [], released: [],
    secretHeaders: [], lastEventIds: [], streamRound: 0, emptyWaits: 0, workerHeaders: [], completesBeforeClose: 0, ...p,
  });
}

function json(res: ServerResponse, status: number, body?: unknown) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(body === undefined ? "" : JSON.stringify(body));
}

const claim = (workId = "work_1") => ({
  work_id: workId, session_id: "sess_1", session_secret: SECRET, lease_generation: 3,
  lease_expires_at: "2026-11-12T00:01:00Z",
  environment: { network: "none", allowed_hosts: [], git: {}, mcp: {} },
});

let server: Server;
let base = "";

before(async () => {
  server = createServer((req: IncomingMessage, res: ServerResponse) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const body = raw ? JSON.parse(raw) : {};
      const path = new URL(req.url ?? "/", "http://x").pathname;
      const id = path.split("/")[3]!;
      if (/\/(claim|renew|heartbeat|complete|release)$/.test(path)) {
        state.workerHeaders.push([req.headers["x-worker-hostname"] as string | undefined, req.headers["x-worker-version"] as string | undefined]);
      }
      if (req.method === "GET" && path === "/v1/sessions/sess_1/events/stream") {
        state.lastEventIds.push(req.headers["last-event-id"] as string | undefined);
        state.streamRound += 1;
        // Round 1 sends seq 1-2 then drops; round 2 replays 2 (must be skipped) and adds 3.
        const seqs = state.streamRound === 1 ? [1, 2] : [2, 3];
        let frames = ": keepalive\n\n";
        for (const s of seqs) {
          const ev = {
            id: `evt_${s}`, object: "event", session_id: "sess_1", seq: s,
            type: s === 3 ? "status" : "message",
            data: s === 3 ? { status: "idle" } : { role: "assistant", content: `m${s}` },
            created_at: "2026-11-12T00:00:00Z",
          };
          frames += `id: ${s}\nevent: ${ev.type}\ndata: ${JSON.stringify(ev)}\n\n`;
        }
        res.writeHead(200, { "content-type": "text/event-stream" });
        res.end(frames);
        return;
      }
      if (path === "/v1/environments/env_1/work/claim") {
        const c = state.claims.shift();
        return c ? json(res, 200, c) : json(res, 204);
      }
      if (path.endsWith("/tool_calls/next")) {
        state.secretHeaders.push(req.headers["x-session-secret"] as string | undefined);
        if (state.staleOnNext) return json(res, 409, { error: { type: "conflict_error", message: "stale", code: "lease_stale" } });
        const c = state.calls.shift();
        if (c) return json(res, 200, c);
        if (state.emptyWaits > 0) {
          state.emptyWaits -= 1;
          return json(res, 204); // plain 204: empty wait, poll again
        }
        res.writeHead(204, { "x-session-closed": "true" });
        return res.end();
      }
      if (path.startsWith("/v1/tool_calls/") && path.endsWith("/result")) {
        state.secretHeaders.push(req.headers["x-session-secret"] as string | undefined);
        if (Buffer.byteLength(body.output) > MAX_TOOL_RESULT_BYTES) {
          return json(res, 413, { error: { type: "payload_too_large_error", message: "big", code: "result_too_large" } });
        }
        state.results.push([id, body]);
        return json(res, 200, { tool_call_id: id, status: "completed" });
      }
      if (path.endsWith("/complete")) {
        if (state.emptyWaits > 0) state.completesBeforeClose += 1;
        state.completed.push([id, body]);
        return json(res, 200, { work_id: id, lease_generation: body.lease_generation, lease_expires_at: null, status: "completed" });
      }
      if (path.endsWith("/release")) {
        state.released.push(id);
        return json(res, 200, { work_id: id, lease_generation: body.lease_generation, lease_expires_at: null, status: "ready" });
      }
      if (path.endsWith("/renew")) {
        return json(res, 200, { work_id: id, lease_generation: body.lease_generation, lease_expires_at: "2026-11-12T00:01:00Z", status: "claimed" });
      }
      json(res, 404, { error: { type: "not_found_error", message: "no", code: "not_found" } });
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const addr = server.address();
  base = `http://127.0.0.1:${typeof addr === "object" && addr ? addr.port : 0}`;
});

after(() => new Promise<void>((r) => server.close(() => r())));
beforeEach(() => reset());

const client = () => new SelfHostClient({ apiKey: "aek_test", baseUrl: base });

test("drain runs each custom tool call once per id, errors unknown tools, completes", async () => {
  const runs: unknown[] = [];
  reset({
    claims: [claim()],
    calls: [
      { tool_call_id: "tc_1", name: "lookup_order", input: { id: "A1" } },
      { tool_call_id: "tc_2", name: "bash", input: { command: "ls" } },
      { tool_call_id: "tc_1", name: "lookup_order", input: { id: "A1" } }, // handed out again
    ],
  });
  await runCustomToolWorker({
    environmentId: "env_1", client: client(), blockMs: 0, drain: true,
    tools: { lookup_order: (input) => { runs.push(input); return { order: input.id, status: "shipped" }; } },
  });
  assert.deepEqual(runs, [{ id: "A1" }]);
  assert.deepEqual(state.results.map((r) => r[0]), ["tc_1", "tc_2", "tc_1"]);
  assert.deepEqual(JSON.parse(state.results[0]![1].output), { order: "A1", status: "shipped" });
  assert.equal(state.results[1]![1].is_error, true);
  assert.match(state.results[1]![1].output, /No handler for tool "bash"/);
  assert.deepEqual(state.completed, [["work_1", { lease_generation: 3, outcome: "success" }]]);
  assert.ok(state.secretHeaders.every((h) => h === SECRET));
});

test("a plain 204 means poll again; only 204 with X-Session-Closed ends the item", async () => {
  reset({ claims: [claim()], emptyWaits: 3, calls: [{ tool_call_id: "tc_a", name: "t", input: {} }] });
  await runCustomToolWorker({ environmentId: "env_1", client: client(), blockMs: 0, drain: true, tools: { t: () => "ok" } });
  assert.equal(state.emptyWaits, 0); // all three empty waits were polled through
  assert.equal(state.completesBeforeClose, 0);
  assert.equal(state.results.length, 1);
  assert.deepEqual(state.completed, [["work_1", { lease_generation: 3, outcome: "success" }]]);
});

test("claim, renew, complete and release carry worker hostname and version", async () => {
  reset({ claims: [claim()] });
  await runCustomToolWorker({ environmentId: "env_1", client: client(), blockMs: 0, drain: true, hostname: "box-1", tools: {} });
  assert.ok(state.workerHeaders.length >= 2); // claim + complete
  assert.ok(state.workerHeaders.every(([h, v]) => h === "box-1" && /^allternit-platform-ts-worker\//.test(v ?? "")));
  const stop = new AbortController();
  reset({ claims: [claim()], calls: [{ tool_call_id: "tc_r", name: "t", input: {} }] });
  await runCustomToolWorker({ environmentId: "env_1", client: client(), blockMs: 0, hostname: "box-1", signal: stop.signal, tools: { t: () => { stop.abort(); return "ok"; } } });
  assert.deepEqual(state.released, ["work_1"]);
  assert.ok(state.workerHeaders.every(([h]) => h === "box-1")); // includes release
});

test("a thrown handler error becomes an error result", async () => {
  reset({ claims: [claim()], calls: [{ tool_call_id: "tc_9", name: "q", input: {} }] });
  await runCustomToolWorker({
    environmentId: "env_1", client: client(), blockMs: 0, drain: true,
    tools: { q: () => { throw new Error("database down"); } },
  });
  assert.deepEqual(state.results[0]![1], { output: "database down", is_error: true });
});

test("oversize output is cut under 100 KiB", async () => {
  reset({ claims: [claim()], calls: [{ tool_call_id: "tc_big", name: "dump", input: {} }] });
  await runCustomToolWorker({ environmentId: "env_1", client: client(), blockMs: 0, drain: true, tools: { dump: () => "é".repeat(200_000) } });
  const out = state.results[0]![1].output;
  assert.ok(Buffer.byteLength(out) <= MAX_TOOL_RESULT_BYTES);
  assert.match(out, /\[truncated:/);
});

test("a stale lease posts nothing and does not complete", async () => {
  reset({ claims: [claim()], staleOnNext: true });
  await runCustomToolWorker({ environmentId: "env_1", client: client(), blockMs: 0, drain: true, tools: {} });
  assert.deepEqual(state.results, []);
  assert.deepEqual(state.completed, []);
  assert.deepEqual(state.released, []);
});

test("stopping hands the item back with /release and discards the result", async () => {
  const stop = new AbortController();
  reset({ claims: [claim()], calls: [{ tool_call_id: "tc_s", name: "t", input: {} }] });
  await runCustomToolWorker({
    environmentId: "env_1", client: client(), blockMs: 0, signal: stop.signal,
    tools: { t: () => { stop.abort(); return "ok"; } },
  });
  assert.deepEqual(state.results, []);
  assert.deepEqual(state.released, ["work_1"]);
  assert.deepEqual(state.completed, []);
});

test("streamEvents resumes with Last-Event-ID and skips replayed events", async () => {
  const seqs: number[] = [];
  for await (const ev of client().sessions.streamEvents("sess_1", { until: isTurnEnd })) seqs.push(ev.seq);
  assert.deepEqual(seqs, [1, 2, 3]);
  assert.deepEqual(state.lastEventIds, [undefined, "2"]);
});

test("the generated self-hosted code matches selfhost-v1.yaml", () => {
  const script = fileURLToPath(new URL("../../../scripts/platform-sdk/generate.py", import.meta.url));
  const out = execFileSync("python3", [
    script, "--check",
    "--spec", "cmd/allternit-api/openapi/selfhost-v1.yaml",
    "--out", "sdk/platform-ts/src/selfhost",
    "--out-py", "sdk/platform-python/allternit_platform/_selfhost_generated.py",
    "--client-name", "SelfHostClient",
  ], { encoding: "utf8" });
  assert.match(out, /up to date/);
});
