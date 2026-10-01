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

  it("without the gateway task client every call is VENDOR_UNAVAILABLE", async () => {
    const p = new ClaudeSubscriptionProvider({});
    expect(await p.contextOpen({ agentId: "claude" })).toMatchObject({ ok: false, error: { code: "VENDOR_UNAVAILABLE" } });
  });
});
