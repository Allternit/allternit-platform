// Claude Managed Agents adapter: SSE streaming (stream-first, live deltas, queued->processed gate), mid-stream
// disconnect with cursor backfill, reconnect exhaustion, custom tool round trip + needs_attention, session listing and
// re-adoption after a restart, and the bad-environment error. All offline against the fake SDK client.
import { describe, expect, it } from "vitest";
import { APIConnectionError } from "@anthropic-ai/sdk";
import type { AaiResult, CursoredEvent } from "@allternit/agent-gateway";
import { FakeManagedAgents } from "../adapters/claude-managed-agents/fixtures/fake-client.js";
import { RECORDED_CUSTOM_TOOL_PAUSE, at, ev } from "../adapters/claude-managed-agents/fixtures/recorded.js";
import { AGENT_ID, makeProvider } from "../adapters/claude-managed-agents/fixtures/rig.js";
import type { MaEvent } from "../adapters/claude-managed-agents/client.js";

const val = <T>(r: AaiResult<T>): T => { if (!r.ok) throw new Error(`${r.error.code}: ${r.error.humanMessage}`); return r.value; };
const tick = async (n = 5) => { for (let i = 0; i < n; i++) await new Promise((r) => setImmediate(r)); };
const ofType = (evs: CursoredEvent[], t: string) => evs.filter((e) => e.event.type === t);
/** The queued echo the fake stored for the last user.message, re-sighted as processed (client-patterns #2). */
const processEcho = (fake: FakeManagedAgents, sid: string) => {
  const echo = [...fake.sessions.get(sid)!.events].reverse().find((e) => e.type === "user.message" && e.processed_at == null)!;
  fake.emit(sid, { ...echo, processed_at: at(fake.t()) });
  return echo;
};
const msg = (fake: FakeManagedAgents, text: string): MaEvent => ev.agentMessage(fake.id(), fake.t(), text);

describe("streaming", () => {
  it("opens the stream before sending, emits live (non-buffered) deltas mid-turn, and waits past a stale idle for a queued message", async () => {
    const fake = new FakeManagedAgents(); fake.manual = true;
    const p = makeProvider(fake);
    const ctx = val(await p.contextOpen({ agentId: AGENT_ID }));
    const sid = ctx.contextId;
    let settled = false;
    const pending = p.contextMessage({ contextId: sid, correlationId: "live-1", text: "stream please" }).then((r) => { settled = true; return r; });
    await fake.streamOpen(sid);
    expect(fake.calls.indexOf("events.stream")).toBeLessThan(fake.calls.indexOf("events.send")); // stream-first
    await tick();
    // the previous turn's idle arrives while our message is still queued: must not end our turn
    fake.emit(sid, ev.idle(fake.id(), fake.t(), "end_turn"));
    await tick();
    expect(settled).toBe(false);
    processEcho(fake, sid);
    fake.emit(sid, ev.running(fake.id(), fake.t()));
    fake.emit(sid, msg(fake, "part one"));
    await tick();
    const mid = val(await p.events({ contextId: sid })).events;
    const delta = ofType(mid, "agent.message.delta").at(-1)!;
    expect(delta.event.payload).toMatchObject({ text: "part one", buffered: false });
    expect(delta.event.correlationId).toBe("live-1");
    expect(settled).toBe(false); // still streaming
    fake.emit(sid, msg(fake, "part two"));
    fake.emit(sid, ev.idle(fake.id(), fake.t(), "end_turn"));
    const r = val(await pending);
    expect(r.reply).toBe("part one\n\npart two");
    await tick();
    expect(fake.openStreams(sid)).toBe(0); // the stream is closed once the turn is over
    const all = val(await p.events({ contextId: sid })).events;
    expect(new Set(all.map((e) => e.event.remoteEventId)).size).toBe(all.length);
  });

  it("a mid-stream disconnect reconnects and backfills from the cursor: nothing lost, nothing repeated, no full re-read", async () => {
    const fake = new FakeManagedAgents(); fake.manual = true;
    let sleeping!: () => void; const slept = new Promise<void>((r) => { sleeping = r; });
    let release!: () => void; const gate = new Promise<void>((r) => { release = r; });
    const p = makeProvider(fake, { sleep: () => { sleeping(); return gate; } });
    const sid = val(await p.contextOpen({ agentId: AGENT_ID })).contextId;
    fake.dropStreamAfter = 3; // echo(queued), echo(processed), running ... then the connection drops
    const pending = p.contextMessage({ contextId: sid, correlationId: "drop-1", text: "long job" });
    await fake.streamOpen(sid); await tick();
    processEcho(fake, sid);
    fake.emit(sid, ev.running(fake.id(), fake.t()));
    await slept; // the pump saw the drop and is backing off
    expect(fake.openStreams(sid)).toBe(0);
    fake.emit(sid, msg(fake, "written while disconnected")); // only reachable through history
    const listsBefore = fake.listParams.length;
    release();
    await fake.streamOpen(sid); await tick();
    fake.emit(sid, msg(fake, "after reconnect"));
    fake.emit(sid, ev.idle(fake.id(), fake.t(), "end_turn"));
    const r = val(await pending);
    expect(r.reply).toBe("written while disconnected\n\nafter reconnect");
    expect(fake.calls.filter((c) => c === "events.stream").length).toBe(2);
    expect(fake.listParams.slice(listsBefore).every((q) => typeof q?.["created_at[gte]"] === "string")).toBe(true);
    const evs = val(await p.events({ contextId: sid })).events;
    expect(new Set(evs.map((e) => e.event.remoteEventId)).size).toBe(evs.length);
    const deltas = ofType(evs, "agent.message.delta").map((e) => e.event.payload);
    expect(deltas).toEqual([
      expect.objectContaining({ text: "written while disconnected", buffered: true }),
      expect.objectContaining({ text: "after reconnect", buffered: false }),
    ]);
  });

  it("gives up after maxReconnects: the message fails as VENDOR_UNAVAILABLE and health.changed is logged", async () => {
    const fake = new FakeManagedAgents(); fake.manual = true;
    const p = makeProvider(fake, { maxReconnects: 1 });
    const sid = val(await p.contextOpen({ agentId: AGENT_ID })).contextId;
    fake.beta.sessions.events.stream = async () => { throw new APIConnectionError({ message: "no route" }); };
    const r = await p.contextMessage({ contextId: sid, correlationId: "x", text: "hi" });
    expect(!r.ok && r.error.code).toBe("VENDOR_UNAVAILABLE");
    const evs = val(await p.events({ contextId: sid })).events;
    expect(ofType(evs, "agent.health.changed").at(-1)?.event.payload).toMatchObject({ status: "degraded", errorType: "stream_lost" });
  });
});

describe("custom tools", () => {
  it("surfaces agent.custom_tool_use + needs_attention, and context.message({ toolResult }) sends user.custom_tool_result", async () => {
    const fake = new FakeManagedAgents();
    const p = makeProvider(fake);
    const sid = val(await p.contextOpen({ agentId: AGENT_ID })).contextId;
    const first = val(await p.contextMessage({ contextId: sid, correlationId: "q1", text: "use the custom tool" }));
    expect(first.reply).toBeUndefined(); // paused, not a silent hang
    const evs = val(await p.events({ contextId: sid })).events;
    const call = ofType(evs, "agent.tool.called").find((e) => e.event.payload?.custom)!;
    expect(call.event.payload).toMatchObject({ phase: "use", custom: true, name: "lookup_order", input: { order: "1234" }, awaiting: "allternit" });
    const toolUseId = String(call.event.payload!.toolUseId);
    const attention = ofType(evs, "agent.task.updated").find((e) => e.event.payload?.kind === "needs_attention")!;
    expect(attention.event.payload).toMatchObject({ reason: "custom_tool_result_required", customToolCalls: [{ toolUseId, name: "lookup_order" }] });

    const sends: unknown[] = [];
    const orig = fake.beta.sessions.events.send;
    fake.beta.sessions.events.send = (id, params) => { sends.push(params.events[0]); return orig(id, params); };
    const answer = val(await p.contextMessage({ contextId: sid, correlationId: "q1-tool", text: "shipped yesterday", toolResult: { toolUseId } }));
    expect(answer.reply).toBe("Tool said: shipped yesterday");
    expect(sends).toEqual([{ type: "user.custom_tool_result", custom_tool_use_id: toolUseId, content: [{ type: "text", text: "shipped yesterday" }] }]);
    const after = val(await p.events({ contextId: sid })).events;
    const result = ofType(after, "agent.tool.called").find((e) => e.event.payload?.phase === "result" && e.event.payload?.custom)!;
    expect(result.event.payload).toMatchObject({ toolUseId, text: "shipped yesterday", isError: false });
    expect(ofType(after, "agent.message.completed").at(-1)?.event.correlationId).toBe("q1-tool");

    const again = await p.contextMessage({ contextId: sid, correlationId: "q1-tool-2", text: "again", toolResult: { toolUseId } });
    expect(!again.ok && again.error.code).toBe("SYNC_CONFLICT");
    const ghost = await p.contextMessage({ contextId: sid, correlationId: "q1-ghost", text: "?", toolResult: { toolUseId: "sevt_nope" } });
    expect(!ghost.ok && ghost.error.code).toBe("UNKNOWN");
  });

  it("a restarted gateway sees a pending custom tool call after adoption and can answer it", async () => {
    const fake = new FakeManagedAgents();
    fake.seedSession("sesn_custom", AGENT_ID, RECORDED_CUSTOM_TOOL_PAUSE);
    const p = makeProvider(fake);
    val(await p.contextOpen({ agentId: AGENT_ID, adoptContextId: "sesn_custom" }));
    const evs = val(await p.events({ contextId: "sesn_custom" })).events;
    expect(ofType(evs, "agent.task.updated").find((e) => e.event.payload?.kind === "needs_attention")?.event.payload)
      .toMatchObject({ customToolCalls: [{ toolUseId: "sevt_c3" }] });
    const r = val(await p.contextMessage({ contextId: "sesn_custom", correlationId: "c", text: "in transit", toolResult: { toolUseId: "sevt_c3" } }));
    expect(r.reply).toBe("Tool said: in transit");
  });
});

describe("session listing + adoption", () => {
  it("snapshot lists the agent's sessions; a restarted provider re-adopts one by id and rebuilds the same log", async () => {
    const fake = new FakeManagedAgents();
    const p = makeProvider(fake);
    const sid = val(await p.contextOpen({ agentId: AGENT_ID, threadId: "thread-9", title: "Orders" })).contextId;
    val(await p.contextMessage({ contextId: sid, correlationId: "k", text: "hello" }));
    const before = val(await p.events({ contextId: sid })).events;

    const q = makeProvider(fake); // gateway restart
    const snap = val(await q.snapshot({ agentId: AGENT_ID }));
    const listed = (snap.fields.sessions as Array<Record<string, unknown>>).find((s) => s.contextId === sid);
    expect(listed).toMatchObject({ status: "idle", title: "Orders", threadId: "thread-9", adopted: false, archived: false });
    const adopted = val(await q.contextOpen({ agentId: AGENT_ID, adoptContextId: sid }));
    expect(adopted).toMatchObject({ contextId: sid, resumed: true });
    const rebuilt = val(await q.events({ contextId: sid })).events;
    expect(rebuilt.map((e) => [e.cursor, e.event.type, e.event.remoteEventId]).slice(1)).toEqual(before.map((e) => [e.cursor, e.event.type, e.event.remoteEventId]).slice(1));
    expect(rebuilt[0].event.threadId).toBe("thread-9"); // thread recovered from session metadata
    expect((val(await q.listSessions({ agentId: AGENT_ID }))).find((s) => s.contextId === sid)?.adopted).toBe(true);
  });

  it("adopting a session that is still running resumes the stream until the turn ends", async () => {
    const fake = new FakeManagedAgents(); fake.manual = true;
    fake.seedSession("sesn_run", AGENT_ID, [ev.userMessage("sevt_r1", 1, "big task"), ev.running("sevt_r2", 2), ev.agentMessage("sevt_r3", 3, "working...")], "running");
    const p = makeProvider(fake);
    val(await p.contextOpen({ agentId: AGENT_ID, adoptContextId: "sesn_run" }));
    await fake.streamOpen("sesn_run"); await tick();
    fake.emit("sesn_run", ev.agentMessage("sevt_r4", 4, "done"));
    fake.emit("sesn_run", ev.idle("sevt_r5", 5, "end_turn"));
    await tick();
    expect(fake.openStreams("sesn_run")).toBe(0);
    const texts = ofType(val(await p.events({ contextId: "sesn_run" })).events, "agent.message.completed").map((e) => e.event.payload?.text);
    expect(texts).toEqual(["working...", "done"]);
  });
});

describe("environment", () => {
  it("an unknown or archived environment id yields a clear error naming the available ones", async () => {
    const missing = await makeProvider(new FakeManagedAgents(), { environmentId: "env_nope" }).contextOpen({ agentId: AGENT_ID });
    expect(!missing.ok && missing.error.humanMessage).toMatch(/env_nope .*was not found.*env_fake \(Fake cloud\)/);
    const archived = await makeProvider(new FakeManagedAgents(), { environmentId: "env_old" }).contextOpen({ agentId: AGENT_ID });
    expect(!archived.ok && archived.error.humanMessage).toMatch(/env_old .*is archived/);
  });
});
