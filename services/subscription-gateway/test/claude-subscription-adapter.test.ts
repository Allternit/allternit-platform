import { describe, it, expect } from "vitest";
import { ClaudeSubscriptionProvider, type GatewayTasks } from "../adapters/claude-subscription/provider.js";
import { CLAUDE_SUBSCRIPTION_MANIFEST, CAPABILITIES } from "../adapters/claude-subscription/manifest.js";

function fakeTasks(opts: { ready?: boolean; finish?: (body: Record<string, unknown>) => Record<string, unknown> } = {}) {
  const submitted: Record<string, unknown>[] = [];
  const store = new Map<string, Record<string, unknown>>();
  const tasks: GatewayTasks = {
    accountState: async () => ((opts.ready ?? true) ? { health: "ready", remainingPct: 40 } : null),
    submit: async (body) => {
      submitted.push(body);
      const t = { task_id: `t${submitted.length}`, status: "queued" };
      store.set(t.task_id, { ...t, ...(opts.finish?.(body) ?? { status: "completed", result: { artifact_ids: [], text: `echo: ${body.prompt}` } }) });
      return { status: 202, body: t };
    },
    get: async (id) => ({ status: 200, body: store.get(id)! }),
  };
  return { tasks, submitted };
}
const mk = (o: Parameters<typeof fakeTasks>[0] = {}) => { const f = fakeTasks(o); return { ...f, p: new ClaudeSubscriptionProvider({ tasks: f.tasks, pollMs: 1, sleep: async () => {} }) }; };

describe("claude-subscription adapter", () => {
  it("manifest and capabilities validate", () => {
    expect(CLAUDE_SUBSCRIPTION_MANIFEST.adapter_id).toBe("claude-subscription");
    expect(CAPABILITIES.lane).toBe("ui_bridge");
  });

  it("first turn is chat.create, later turns chat.continue on the same thread, routed to the Claude subscription", async () => {
    const { p, submitted } = mk();
    const c = await p.contextOpen({ agentId: "claude", threadId: "th1" });
    expect(c.ok).toBe(true);
    if (!c.ok) return;
    const r1 = await p.contextMessage({ contextId: c.value.contextId, correlationId: "k1", text: "hi" });
    const r2 = await p.contextMessage({ contextId: c.value.contextId, correlationId: "k2", text: "again" });
    expect(r1).toMatchObject({ ok: true, value: { reply: "echo: hi" } });
    expect(r2).toMatchObject({ ok: true, value: { reply: "echo: again" } });
    expect(submitted.map((b) => b.capability)).toEqual(["chat.create", "chat.continue"]);
    expect(submitted[0].thread_id).toBe(c.value.contextId);
    expect(submitted[1].thread_id).toBe(c.value.contextId);
    expect(submitted[0]).toMatchObject({ routing: { provider: "claude" }, initiated_by: { kind: "human", action_id: "k1" } });
    // Replaying a correlation id never sends twice.
    await p.contextMessage({ contextId: c.value.contextId, correlationId: "k1", text: "hi" });
    expect(submitted).toHaveLength(2);
    const ev = await p.events({ contextId: c.value.contextId });
    expect(ev.ok && ev.value.events.map((e) => e.event.type)).toContain("agent.message.completed");
  });

  it("no Ready Claude login: contexts refuse with AUTH_REQUIRED and the agent reads blocked", async () => {
    const { p } = mk({ ready: false });
    expect(await p.contextOpen({ agentId: "claude" })).toMatchObject({ ok: false, error: { code: "AUTH_REQUIRED" } });
    const l = await p.list();
    expect(l.ok && l.value[0].state).toBe("blocked");
  });

  it("task failures map to AAI codes and keep the vendor's words", async () => {
    const { p } = mk({ finish: () => ({ status: "failed", error: { class: "rate_limited", detail: "You've hit your limit.", cooldown_s: 60, user_action: null } }) });
    const c = await p.contextOpen({ agentId: "claude" });
    if (!c.ok) throw new Error("open");
    expect(await p.contextMessage({ contextId: c.value.contextId, correlationId: "x", text: "hi" })).toMatchObject({ ok: false, error: { code: "RATE_LIMITED", humanMessage: expect.stringContaining("hit your limit") } });
    const { p: p2 } = mk({ finish: () => ({ status: "needs_user", error: { class: "verification_challenge", detail: "Verify you are human", user_action: "Solve it in the window" } }) });
    const c2 = await p2.contextOpen({ agentId: "claude" });
    if (!c2.ok) throw new Error("open");
    expect(await p2.contextMessage({ contextId: c2.value.contextId, correlationId: "y", text: "hi" })).toMatchObject({ ok: false, error: { code: "LANE_BLOCKED" } });
  });

  it("a 0% usage reading is a limit until its reset time, then stale", async () => {
    const at = (ms: number) => new Date(Date.now() + ms).toISOString();
    const mkState = (resetsAt: string | null) => new ClaudeSubscriptionProvider({ tasks: { ...fakeTasks().tasks, accountState: async () => ({ health: "ready", remainingPct: 0, resetsAt }) }, pollMs: 1, sleep: async () => {} });
    expect(await mkState(at(30 * 60_000)).contextOpen({ agentId: "claude" })).toMatchObject({ ok: false, error: { code: "RATE_LIMITED" } });
    expect(await mkState(null).contextOpen({ agentId: "claude" })).toMatchObject({ ok: false, error: { code: "RATE_LIMITED" } });
    expect((await mkState(at(-5 * 60_000)).contextOpen({ agentId: "claude" })).ok).toBe(true);
  });

  it("a context survives a gateway restart: continue on its mapping, or start over if there is none", async () => {
    const { tasks, submitted } = fakeTasks();
    const p1 = new ClaudeSubscriptionProvider({ tasks, pollMs: 1, sleep: async () => {} });
    const c = await p1.contextOpen({ agentId: "claude" });
    if (!c.ok) throw new Error("open");
    await p1.contextMessage({ contextId: c.value.contextId, correlationId: "a", text: "first" });
    const p2 = new ClaudeSubscriptionProvider({ tasks, pollMs: 1, sleep: async () => {} }); // restarted gateway
    const r = await p2.contextMessage({ contextId: c.value.contextId, correlationId: "b", text: "second" });
    expect(r.ok).toBe(true);
    expect(submitted.map((b) => b.capability)).toEqual(["chat.create", "chat.continue"]);
    expect(submitted[1].thread_id).toBe(c.value.contextId);
    // No mapping on the gateway (409 thread_not_mapped): the turn starts a new conversation instead of failing.
    const unmapped = fakeTasks();
    const orig = unmapped.tasks.submit;
    unmapped.tasks.submit = async (b) => (b.capability === "chat.continue" ? { status: 409, body: { error: "thread_not_mapped" } } : orig(b));
    const p3 = new ClaudeSubscriptionProvider({ tasks: unmapped.tasks, pollMs: 1, sleep: async () => {} });
    expect((await p3.contextMessage({ contextId: "cs-gone-1-x", correlationId: "c", text: "hi" })).ok).toBe(true);
    expect((await p3.contextMessage({ contextId: "not-ours", correlationId: "d", text: "x" })).ok).toBe(false);
  });

  it("without the gateway task client every call is VENDOR_UNAVAILABLE", async () => {
    const p = new ClaudeSubscriptionProvider({});
    expect(await p.contextOpen({ agentId: "claude" })).toMatchObject({ ok: false, error: { code: "VENDOR_UNAVAILABLE" } });
  });
});

describe("claude-subscription thinking", () => {
  it("streams reasoning deltas as thinking activity and attaches the thought to the reply", async () => {
    const f = fakeTasks();
    let onEvent: ((e: { kind?: string; payload?: unknown }) => void) | null = null;
    f.tasks.subscribe = (_id, cb) => { onEvent = cb; return () => { onEvent = null; }; };
    const origGet = f.tasks.get;
    let polls = 0;
    f.tasks.get = async (id) => {
      polls += 1;
      if (polls === 1) { onEvent?.({ kind: "reply", payload: { t: "reply", event: { type: "reply.reasoning.delta", delta: "Weighing " } } }); onEvent?.({ kind: "reply", payload: { t: "reply", event: { type: "reply.reasoning.delta", delta: "the options." } } }); }
      return origGet(id);
    };
    const p = new ClaudeSubscriptionProvider({ tasks: f.tasks, pollMs: 1, sleep: async () => {} });
    const c = await p.contextOpen({ agentId: "claude" });
    if (!c.ok) throw new Error("open");
    await p.contextMessage({ contextId: c.value.contextId, correlationId: "t1", text: "decide" });
    const ev = await p.events({ contextId: c.value.contextId });
    const evs = ev.ok ? ev.value.events.map((e) => e.event) : [];
    const acts = evs.filter((e) => e.type === "agent.activity.started" && (e.payload as { kind?: string }).kind === "thinking");
    expect((acts.at(-1)!.payload as { detail: string }).detail).toBe("Weighing the options.");
    const done = evs.find((e) => e.type === "agent.message.completed")!;
    expect((done.payload as { content?: unknown[] }).content).toEqual([{ type: "thinking", text: "Weighing the options." }]);
    expect(onEvent).toBeNull(); // unsubscribed after the turn
  });
});

describe("claude-subscription cursors", () => {
  it("a revived context's events sort after everything a previous process handed out", async () => {
    const { tasks } = fakeTasks();
    const p1 = new ClaudeSubscriptionProvider({ tasks, pollMs: 1, sleep: async () => {} });
    const c = await p1.contextOpen({ agentId: "claude" });
    if (!c.ok) throw new Error("open");
    await p1.contextMessage({ contextId: c.value.contextId, correlationId: "a", text: "one" });
    const before = await p1.events({ contextId: c.value.contextId });
    const held = before.ok ? before.value.nextCursor : "0";
    const p2 = new ClaudeSubscriptionProvider({ tasks, pollMs: 1, sleep: async () => {} });
    await p2.contextMessage({ contextId: c.value.contextId, correlationId: "b", text: "two" });
    const after = await p2.events({ contextId: c.value.contextId, cursor: held });
    expect(after.ok && after.value.events.some((e) => e.event.type === "agent.message.completed")).toBe(true);
  });
});


describe("claude-subscription Projects", () => {
  const PID = "0f6f1c2e-6a3b-4c1d-9e8f-123456789abc";
  it("lists each Claude Project as an agent and starts its chats inside the Project, also after a restart", async () => {
    const f = fakeTasks();
    f.tasks.accountState = async () => ({ health: "ready", remainingPct: 50, agents: [{ id: PID, name: "Allternit Brain", kind: "project" }, { id: "not-a-uuid", name: "junk" }] });
    const p = new ClaudeSubscriptionProvider({ tasks: f.tasks, pollMs: 1, sleep: async () => {} });
    const l = await p.list();
    expect(l.ok && l.value.map((a) => [a.agentId, a.displayName])).toEqual([["claude", "Claude"], [`claude:project:${PID}`, "Allternit Brain"]]);
    expect(await p.identity(`claude:project:${PID}`)).toMatchObject({ ok: true, value: { displayName: "Allternit Brain", lookPack: "claude" } });
    const c = await p.contextOpen({ agentId: `claude:project:${PID}` });
    if (!c.ok) throw new Error("open");
    await p.contextMessage({ contextId: c.value.contextId, correlationId: "p1", text: "hi" });
    expect(f.submitted[0]).toMatchObject({ capability: "chat.create", options: { project_id: PID } });
    // A restarted gateway revives the context with its Project; a new conversation (no mapping) still starts there.
    const unmapped = fakeTasks();
    const orig = unmapped.tasks.submit;
    unmapped.tasks.submit = async (b) => (b.capability === "chat.continue" ? { status: 409, body: { error: "thread_not_mapped" } } : orig(b));
    const p2 = new ClaudeSubscriptionProvider({ tasks: unmapped.tasks, pollMs: 1, sleep: async () => {} });
    await p2.contextMessage({ contextId: c.value.contextId, correlationId: "p2", text: "again" });
    expect(unmapped.submitted.at(-1)).toMatchObject({ capability: "chat.create", options: { project_id: PID } });
    expect(await p.contextOpen({ agentId: "claude:project:nope" })).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND" } });
  });
});
