// GET /aai/events/stream: SSE over agent.events against the offline fake OpenClaw gateway — framing (event: aai,
// id: <cursor>), live delivery, Last-Event-ID resume, heartbeats, error termination and teardown on disconnect.
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { AaiHost } from "../src/aai/registry.js";
import { AaiContextStore } from "../src/aai/context-store.js";
import { createServer } from "../src/http/server.js";
import { issueToken } from "../src/security/tokens.js";
import { OpenClawProvider } from "../adapters/openclaw/index.js";
import { startFakeOpenClaw, type FakeOpenClaw } from "../adapters/openclaw/fixtures/fake-server.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

const AGENT = "openclaw:openclaw";
const binding = { id: "b1", botId: "bot-1", type: "vendor" as const, mode: "hosted" as const, adapterId: "openclaw", externalAgentId: AGENT, state: "BOUND" as const };

let fake: FakeOpenClaw;
beforeAll(async () => { fake = await startFakeOpenClaw(); });
afterAll(async () => { await fake.close(); });

let dir: string, deps: TestDeps, host: AaiHost, server: Server, base: string, tok: string, calls: number;
beforeEach(async () => {
  dir = tmpStateDir();
  deps = makeDeps(dir);
  tok = issueToken(deps.db, "api", "test", ["tasks:submit", "tasks:read"]).token;
  host = new AaiHost([], { contexts: new AaiContextStore(deps.db) }).register({ provider: new OpenClawProvider({ baseUrl: fake.url }) });
  calls = 0;
  const call = host.call.bind(host);
  host.call = (...a: Parameters<AaiHost["call"]>) => { if (a[0] === "agent.events") calls += 1; return call(...a); };
  const app = createServer({ ...deps, aai: host, aaiStream: { pollMs: 20, heartbeatMs: 40 } });
  server = await new Promise<Server>((r) => { const s = app.listen(0, "127.0.0.1", () => r(s)); });
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
afterEach(async () => {
  server.closeAllConnections?.();
  await new Promise<void>((r) => server.close(() => r()));
  deps.cleanup(); cleanupDir(dir);
});

interface Frame { event?: string; id?: string; data?: string; comment?: string }
/** Opens the stream and parses frames as they arrive. */
async function stream(query: string, headers: Record<string, string> = {}) {
  const ac = new AbortController();
  const res = await fetch(`${base}/aai/events/stream?${query}`, { headers: { authorization: `Bearer ${tok}`, ...headers }, signal: ac.signal });
  const frames: Frame[] = [];
  let ended = false;
  const done = (async () => {
    if (!res.body) return;
    const reader = res.body.getReader(), dec = new TextDecoder();
    let buf = "";
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += dec.decode(value, { stream: true });
        let i: number;
        while ((i = buf.indexOf("\n\n")) >= 0) {
          const f: Frame = {};
          for (const line of buf.slice(0, i).split("\n")) {
            if (line.startsWith(":")) f.comment = line.slice(1).trim();
            else { const k = line.indexOf(":"); (f as Record<string, string>)[line.slice(0, k)] = line.slice(k + 1).trim(); }
          }
          frames.push(f);
          buf = buf.slice(i + 2);
        }
      }
    } catch { /* aborted */ }
    ended = true;
  })();
  const until = async (pred: () => boolean, ms = 3000) => {
    const t = Date.now();
    while (!pred()) { if (Date.now() - t > ms) throw new Error(`timed out; frames: ${JSON.stringify(frames)}`); await new Promise((r) => setTimeout(r, 10)); }
  };
  const aai = () => frames.filter((f) => f.event === "aai").map((f) => ({ id: f.id!, ev: JSON.parse(f.data!) as { type: string; cursor: string; payload?: { reply?: string } } }));
  return { res, frames, aai, until, ended: () => ended, close: async () => { ac.abort(); await done; } };
}

async function openWithTurn(): Promise<string> {
  const o = await host.call("agent.context.open", binding, { agentId: AGENT });
  if (!o.ok) throw new Error(o.error.humanMessage);
  const contextId = (o.value as { contextId: string }).contextId;
  const m = await host.call("agent.context.message", binding, { contextId, correlationId: "c1", text: "hi" });
  if (!m.ok) throw new Error(m.error.humanMessage);
  return contextId;
}

describe("GET /aai/events/stream", () => {
  it("requires a bearer token and a context", async () => {
    expect((await fetch(`${base}/aai/events/stream?adapterId=openclaw&contextId=x`)).status).toBe(401);
    expect((await fetch(`${base}/aai/events/stream?adapterId=openclaw`, { headers: { authorization: `Bearer ${tok}` } })).status).toBe(400);
  });

  it("streams events with cursor ids, live turns, heartbeats, and resumes after Last-Event-ID", async () => {
    const contextId = await openWithTurn();
    const s = await stream(`adapterId=openclaw&contextId=${contextId}`);
    expect(s.res.headers.get("content-type")).toMatch(/text\/event-stream/);
    await s.until(() => s.aai().some((e) => e.ev.type === "agent.message.completed"));
    for (const e of s.aai()) expect(e.id).toBe(e.ev.cursor);
    expect(s.aai()[0].ev.type).toBe("agent.context.opened");
    const seen = s.aai().length;

    // A turn sent while connected arrives live.
    const m = await host.call("agent.context.message", binding, { contextId, correlationId: "c2", text: "again" });
    expect(m.ok).toBe(true);
    await s.until(() => s.aai().slice(seen).some((e) => e.ev.type === "agent.message.completed"));
    expect(s.aai().slice(seen).find((e) => e.ev.type === "agent.message.completed")!.ev.payload!.reply).toBe("echo[2]: again");
    await s.until(() => s.frames.some((f) => f.comment === "hb"));
    const ids = s.aai().map((e) => Number(e.id));
    expect(ids).toEqual([...ids].sort((a, b) => a - b)); // in order, no repeats
    expect(new Set(ids).size).toBe(ids.length);
    await s.close();

    // Reconnect: Last-Event-ID wins over ?cursor and only later events are sent.
    const mid = s.aai()[seen - 1].id;
    const r = await stream(`adapterId=openclaw&contextId=${contextId}&cursor=0`, { "last-event-id": mid });
    await r.until(() => r.aai().some((e) => e.ev.type === "agent.message.completed"));
    expect(r.aai().every((e) => Number(e.id) > Number(mid))).toBe(true);
    expect(r.aai().map((e) => e.id)).toEqual(s.aai().slice(seen).map((e) => e.id));
    await r.close();
  });

  it("accepts an explicit binding and stops polling when the client disconnects", async () => {
    const contextId = await openWithTurn();
    const s = await stream(`binding=${encodeURIComponent(JSON.stringify(binding))}&contextId=${contextId}`);
    await s.until(() => s.aai().length > 0);
    await s.until(() => calls >= 3);
    await s.close();
    await new Promise((r) => setTimeout(r, 60));
    const after = calls;
    await new Promise((r) => setTimeout(r, 150));
    expect(calls).toBe(after);
  });

  it("sends a non-retryable read failure as event: error and ends the stream", async () => {
    const s = await stream("adapterId=openclaw&contextId=oc-unknown");
    await s.until(() => s.ended());
    const err = s.frames.find((f) => f.event === "error");
    expect(JSON.parse(err!.data!)).toMatchObject({ code: "CONTEXT_NOT_FOUND" });
  });
});
