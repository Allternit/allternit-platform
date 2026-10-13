// Website/desktop subscriptions queue extra turns: on a UI-bridge lane (maxParallel 1, one window per account) a
// second concurrent turn waits in order and reports "queued … (position N)" instead of failing or racing the first;
// a full queue answers RATE_LIMITED; cancel takes a waiting turn out of line; a too-long wait gives up.
import { describe, expect, it } from "vitest";
import {
  BaseAaiProvider, fail, ok,
  type AaiResult, type AgentCapabilityManifest, type CursoredEvent, type EventsInput, type EventsResult, type MessageInput, type MessageResult,
  type OpenContextInput, type OpenContextResult,
} from "@allternit/agent-gateway";
import { agentCapabilityManifestSchema } from "@allternit/subscription-fabric-contracts";
import { AaiHost } from "../src/aai/registry.js";

const MANIFEST: AgentCapabilityManifest = agentCapabilityManifestSchema.parse({
  vendor: "grok", adapterId: "ui-app", lane: "ui_bridge", guarantee: "best_effort",
  context: { supported: true, resume: false, parallel: false, maxParallel: 1, isolation: "shared" },
  messaging: { send: true, stream: true, steer: false, interrupt: false, cancel: true },
  memory: { read: false, write: false, snapshot: false, opaque: true },
  tools: { tools: false, mcp: false, plugins: false, connectors: false },
  tasks: { list: false, schedule: false, cancel: false, background: false },
  approvals: { read: false, respond: false, exact: false },
  computer: { view: false, control: false, takeover: false },
  artifacts: { read: false, write: false, export: false },
  events: { native: false, polling: true, transcriptDerived: true, replay: true },
  runtime: { alwaysOn: false, localRequired: true, cloud: false },
});

const deferred = () => { let resolve!: () => void; const promise = new Promise<void>((r) => (resolve = r)); return { promise, resolve }; };

/** One app window: refuses (CONTEXT_BUSY) anything while a reply runs, like grok-bot/chatgpt-dots/claude-desktop. */
class OneWindowApp extends BaseAaiProvider {
  readonly adapterId = "ui-app";
  log: string[] = [];
  cancels = 0;
  gates = new Map<string, ReturnType<typeof deferred>>();
  private sending = 0;
  private n = 0;
  private events_ = new Map<string, CursoredEvent[]>();
  gate(corr: string) { if (!this.gates.has(corr)) this.gates.set(corr, deferred()); return this.gates.get(corr)!; }
  async capabilities(): Promise<AaiResult<AgentCapabilityManifest>> { return ok(MANIFEST); }
  async contextOpen(_i: OpenContextInput): Promise<AaiResult<OpenContextResult>> {
    if (this.sending) return fail("CONTEXT_BUSY", "still answering in another conversation", { retryable: true });
    const id = `ctx-${++this.n}`;
    this.events_.set(id, []);
    this.log.push(`open ${id}`);
    return ok({ contextId: id, isolation: "shared", guarantee: "best_effort", resumed: false });
  }
  async contextMessage(i: MessageInput): Promise<AaiResult<MessageResult>> {
    if (this.sending) { this.log.push(`RACE ${i.correlationId}`); return fail("CONTEXT_BUSY", "still answering", { retryable: true }); }
    this.sending += 1;
    this.log.push(`start ${i.correlationId}`);
    try {
      await this.gate(i.correlationId).promise;
      this.log.push(`end ${i.correlationId}`);
      return ok({ messageId: `m-${i.correlationId}`, correlationId: i.correlationId, reply: `re: ${i.text}`, guarantee: "best_effort" });
    } finally { this.sending -= 1; }
  }
  async contextCancel(): Promise<AaiResult<{ confirmed: boolean }>> { this.cancels += 1; return ok({ confirmed: true }); }
  async events(i: EventsInput): Promise<AaiResult<EventsResult>> {
    const evs = (this.events_.get(i.contextId) ?? []).filter((e) => Number(e.cursor) > Number(i.cursor ?? 0));
    return ok({ events: evs, nextCursor: evs.at(-1)?.cursor ?? String(i.cursor ?? "0") });
  }
}

const binding = { id: "b1", botId: "bot-1", type: "vendor" as const, mode: "hosted" as const, adapterId: "ui-app", externalAgentId: "ui-agent", state: "BOUND" as const };
const setup = (queue: { maxWaitMs?: number; maxQueued?: number } = {}) => {
  const app = new OneWindowApp();
  const host = new AaiHost([], { queue: { busyRetryMs: 10, ...queue } }).register({ provider: app });
  return { app, host };
};
const openCtx = async (host: AaiHost) => {
  const r = await host.call("agent.context.open", binding, { agentId: "ui-agent" });
  if (!r.ok) throw new Error(r.error.humanMessage);
  return (r.value as { contextId: string }).contextId;
};
const until = async (pred: () => boolean) => { const t = Date.now(); while (!pred()) { if (Date.now() - t > 2000) throw new Error("timed out"); await new Promise((r) => setTimeout(r, 5)); } };
type Wire = { type: string; cursor: string; payload: { kind?: string; state?: string; position?: number; detail?: string } };
const read = async (host: AaiHost, contextId: string) => {
  const r = await host.call("agent.events", binding, { contextId });
  if (!r.ok) throw new Error(r.error.humanMessage);
  return (r.value as { events: Wire[] }).events;
};

describe("per-account turn queue (UI-bridge lanes)", () => {
  it("runs two concurrent turns on one account in order; the second reports its place in line", async () => {
    const { app, host } = setup();
    const a = await openCtx(host), b = await openCtx(host);
    const first = host.call("agent.context.message", binding, { contextId: a, correlationId: "A", text: "one" });
    await until(() => app.log.includes("start A"));
    const second = host.call("agent.context.message", binding, { contextId: b, correlationId: "B", text: "two" });
    await new Promise((r) => setTimeout(r, 20));
    expect(app.log).not.toContain("start B");
    expect(host.turns.waiting("ui-app")).toBe(1);

    const queued = await read(host, b);
    expect(queued).toHaveLength(1);
    expect(queued[0]).toMatchObject({ type: "agent.activity.started", payload: { kind: "queued", state: "queued", position: 1, detail: "queued: waiting for your Grok app (position 1)" } });
    expect(await read(host, b)).toHaveLength(0); // each notice is delivered once
    expect(await read(host, a)).toHaveLength(0); // and only on its own context

    app.gate("A").resolve();
    expect((await first).ok).toBe(true);
    await until(() => app.log.includes("start B"));
    expect((await read(host, b))[0]).toMatchObject({ payload: { state: "started", position: 0 } });
    app.gate("B").resolve();
    expect(await second).toMatchObject({ ok: true, value: { reply: "re: two" } });
    expect(app.log).toEqual(["open ctx-1", "open ctx-2", "start A", "end A", "start B", "end B"]);
  });

  it("a replayed message keeps one place in line; positions move up as turns finish", async () => {
    const { app, host } = setup();
    const [a, b, c] = [await openCtx(host), await openCtx(host), await openCtx(host)];
    const first = host.call("agent.context.message", binding, { contextId: a, correlationId: "A", text: "1" });
    await until(() => app.log.includes("start A"));
    const second = host.call("agent.context.message", binding, { contextId: b, correlationId: "B", text: "2" });
    const again = host.call("agent.context.message", binding, { contextId: b, correlationId: "B", text: "2" });
    const third = host.call("agent.context.message", binding, { contextId: c, correlationId: "C", text: "3" });
    await until(() => host.turns.waiting("ui-app") === 2);
    expect((await read(host, c)).map((e) => e.payload.position)).toEqual([2]);
    app.gate("A").resolve();
    await until(() => app.log.includes("start B"));
    expect((await read(host, c)).map((e) => e.payload.position)).toEqual([1]);
    app.gate("B").resolve(); app.gate("C").resolve();
    await Promise.all([first, second, again, third]);
    expect(await again).toEqual(await second);
    expect(app.log.filter((l) => l.startsWith("start"))).toEqual(["start A", "start B", "start C"]);
  });

  it("a full queue answers RATE_LIMITED", async () => {
    const { app, host } = setup({ maxQueued: 1 });
    const [a, b, c] = [await openCtx(host), await openCtx(host), await openCtx(host)];
    const first = host.call("agent.context.message", binding, { contextId: a, correlationId: "A", text: "1" });
    await until(() => app.log.includes("start A"));
    const second = host.call("agent.context.message", binding, { contextId: b, correlationId: "B", text: "2" });
    await until(() => host.turns.waiting("ui-app") === 1);
    const third = await host.call("agent.context.message", binding, { contextId: c, correlationId: "C", text: "3" });
    expect(third).toMatchObject({ ok: false, error: { code: "RATE_LIMITED", retryable: true, details: { queued: 1 } } });
    expect(!third.ok && third.error.humanMessage).toMatch(/already waiting for your Grok app/);
    app.gate("A").resolve(); app.gate("B").resolve();
    expect((await first).ok && (await second).ok).toBe(true);
  });

  it("cancel takes a waiting turn out of line without touching the app", async () => {
    const { app, host } = setup();
    const a = await openCtx(host), b = await openCtx(host);
    const first = host.call("agent.context.message", binding, { contextId: a, correlationId: "A", text: "1" });
    await until(() => app.log.includes("start A"));
    const second = host.call("agent.context.message", binding, { contextId: b, correlationId: "B", text: "2" });
    await until(() => host.turns.waiting("ui-app") === 1);
    expect(await host.call("agent.context.cancel", binding, { contextId: b })).toEqual({ ok: true, value: { confirmed: true } });
    expect(await second).toMatchObject({ ok: false, error: { details: { cancelled: true, queued: true } } });
    expect(app.cancels).toBe(0); // a Stop click would have stopped A's reply
    expect(host.turns.waiting("ui-app")).toBe(0);
    app.gate("A").resolve();
    expect((await first).ok).toBe(true);
    expect(app.log).not.toContain("start B");
    // Cancel of the running turn still reaches the app.
    expect((await host.call("agent.context.cancel", binding, { contextId: a })).ok).toBe(true);
    expect(app.cancels).toBe(1);
  });

  it("gives up after the bounded wait", async () => {
    const { app, host } = setup({ maxWaitMs: 40 });
    const a = await openCtx(host), b = await openCtx(host);
    const first = host.call("agent.context.message", binding, { contextId: a, correlationId: "A", text: "1" });
    await until(() => app.log.includes("start A"));
    const r = await host.call("agent.context.message", binding, { contextId: b, correlationId: "B", text: "2" });
    expect(r).toMatchObject({ ok: false, error: { code: "CONTEXT_BUSY", retryable: true, details: { queueTimeout: true } } });
    app.gate("A").resolve();
    await first;
  });

  it("an open on a shared-window lane waits for the running turn instead of failing", async () => {
    const { app, host } = setup();
    const a = await openCtx(host);
    const first = host.call("agent.context.message", binding, { contextId: a, correlationId: "A", text: "1" });
    await until(() => app.log.includes("start A"));
    const opening = host.call("agent.context.open", binding, { agentId: "ui-agent" });
    await new Promise((r) => setTimeout(r, 20));
    expect(app.log).not.toContain("open ctx-2");
    app.gate("A").resolve();
    expect(await opening).toMatchObject({ ok: true, value: { contextId: "ctx-2" } });
    await first;
  });
});
