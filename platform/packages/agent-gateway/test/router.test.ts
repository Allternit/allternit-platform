import { describe, expect, it } from "vitest";
import type { BotExecutionBinding } from "@allternit/subscription-fabric-contracts";
import { AaiRouter, MemoryProvider, runConformance, withFaults } from "../src";

const binding = (over: Partial<BotExecutionBinding> = {}): BotExecutionBinding => ({
  id: "b1", botId: "mem-agent", type: "vendor", mode: "hosted", adapterId: "memory-test", state: "READY", ...over,
});

describe("AaiRouter", () => {
  it("routes by binding and returns first result for replayed correlationId (no duplicate work)", async () => {
    const inner = new MemoryProvider("mem-agent", 2, { doubleExecute: true }); // provider would double-execute
    const r = new AaiRouter().register(inner).bind(binding());
    const o = await r.contextOpen({ agentId: "mem-agent" });
    if (!o.ok) throw new Error("open");
    const a = await r.contextMessage({ contextId: o.value.contextId, correlationId: "c", text: "x" });
    const b = await r.contextMessage({ contextId: o.value.contextId, correlationId: "c", text: "x" });
    expect(b).toBe(a);
    const ev = await r.events({ contextId: o.value.contextId });
    expect(ev.ok && ev.value.events.filter((e) => e.event.type === "agent.message.completed")).toHaveLength(1);
  });

  it("enforces maxParallel with CONTEXT_BUSY even if the provider does not", async () => {
    const r = new AaiRouter().register(new MemoryProvider("mem-agent", 2, { ignoreMaxParallel: true })).bind(binding());
    const opens = await Promise.all([1, 2, 3].map(() => r.contextOpen({ agentId: "mem-agent" })));
    expect(opens.filter((x) => x.ok)).toHaveLength(2);
    expect(opens.filter((x) => !x.ok && x.error.code === "CONTEXT_BUSY")).toHaveLength(1);
  });

  it("never auto-resolves an approval without a human actor", async () => {
    const r = new AaiRouter().register(new MemoryProvider("mem-agent", 2, { autoApprove: true })).bind(binding());
    const res = await r.approvals({ op: "respond", approvalId: "appr-1", decision: "approve", actor: { type: "system", id: "coordinator" } });
    expect(!res.ok && res.error.code).toBe("APPROVAL_REQUIRED");
  });

  it("returns AAIError for missing adapter, dead bindings, unsupported ops and raw throws", async () => {
    const router = new AaiRouter().register(withFaults(new MemoryProvider(), { contextOpen: "throw" }));
    const code = async (b: BotExecutionBinding, f: (p: ReturnType<AaiRouter["bind"]>) => Promise<{ ok: boolean; error?: { code: string } }>) => (await f(router.bind(b))).error?.code;
    expect(await code(binding({ adapterId: "nope" }), (p) => p.list())).toBe("UNSUPPORTED");
    expect(await code(binding({ state: "NEEDS_AUTH" }), (p) => p.list())).toBe("AUTH_REQUIRED");
    expect(await code(binding({ state: "DISABLED" }), (p) => p.list())).toBe("LANE_BLOCKED");
    expect(await code(binding(), (p) => p.contextOpen({ agentId: "mem-agent" }))).toBe("UNKNOWN");
    expect(await code(binding(), (p) => p.computer({ contextId: "x", op: "frame" }))).toBe("UNSUPPORTED");
  });

  it("a healthy provider behind the router still passes conformance", async () => {
    const router = new AaiRouter().register(new MemoryProvider());
    const rep = await runConformance(router.bind(binding()), { agentId: "mem-agent", approvalId: "appr-1" });
    expect(rep.areas.filter((a) => a.status === "fail").map((a) => a.reasons)).toEqual([]);
  });
});
