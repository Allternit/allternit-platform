import { describe, expect, it } from "vitest";
import { MemoryProvider, runConformance, type MemoryDefects } from "../src";

const fx = { agentId: "mem-agent", approvalId: "appr-1" };
const status = (r: Awaited<ReturnType<typeof runConformance>>) => Object.fromEntries(r.areas.map((a) => [a.area, a.status]));

describe("conformance harness discriminates", () => {
  it("passes a healthy in-memory provider", async () => {
    const r = await runConformance(new MemoryProvider(), fx);
    expect(r.areas.filter((a) => a.status === "fail").map((a) => a.reasons)).toEqual([]);
    expect(r.ok).toBe(true);
  });

  it("FAILS a provider that contaminates contexts, double-executes replays and auto-approves", async () => {
    const r = await runConformance(new MemoryProvider("mem-agent", 2, { contaminate: true, doubleExecute: true, autoApprove: true }), fx);
    expect(r.ok).toBe(false);
    const s = status(r);
    expect(s.isolation).toBe("fail");
    expect(s.idempotency).toBe("fail");
    expect(s.approvals).toBe("fail");
    expect(r.areas.find((a) => a.area === "isolation")!.reasons.join()).toMatch(/token/);
  });

  const single: Array<[keyof MemoryDefects, string]> = [
    ["contaminate", "isolation"], ["doubleExecute", "idempotency"], ["autoApprove", "approvals"],
    ["ignoreCancel", "cancellation"],
  ];
  it("letting more conversations exist than maxParallel is allowed (the limit is on what runs at once)", async () => {
    const r = await runConformance(new MemoryProvider("mem-agent", 2, { ignoreMaxParallel: true }), fx);
    expect(status(r).parallelism).toBe("pass");
  });
  for (const [defect, area] of single) {
    it(`defect ${defect} fails only via ${area}`, async () => {
      const r = await runConformance(new MemoryProvider("mem-agent", 2, { [defect]: true }), fx);
      expect(status(r)[area]).toBe("fail");
    });
  }

  it("fails a provider that throws raw errors", async () => {
    const { withFaults } = await import("../src");
    const p = withFaults(new MemoryProvider(), { contextOpen: "throw" });
    const r = await runConformance(p, fx);
    expect(r.ok).toBe(false);
    expect(r.areas.find((a) => a.area === "context")!.reasons.join()).toMatch(/threw raw/);
  });

  it("fails a provider that answers ok for something it does not declare", async () => {
    const p = new MemoryProvider();
    (p as unknown as { computer: () => Promise<unknown> }).computer = async () => ({ ok: true, value: { frame: "x" } });
    const r = await runConformance(p, fx);
    expect(status(r).computer).toBe("fail");
  });
});
