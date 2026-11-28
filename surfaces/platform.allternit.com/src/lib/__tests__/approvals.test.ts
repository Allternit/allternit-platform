import { describe, expect, it } from "vitest";
import { approvalStatusLabel, canDecideInConsole, kernelDeadlineState, shortActionHash, summarizeProposedAction } from "../approvals";

describe("approvals mapping", () => {
  it("labels the five statuses and passes unknown through", () => {
    expect(approvalStatusLabel("pending")).toBe("Pending");
    expect(approvalStatusLabel("consumed")).toBe("Consumed");
    expect(approvalStatusLabel("weird")).toBe("weird");
  });
  it("summarizes proposed_action on one line", () => {
    expect(summarizeProposedAction({ summary: "Text  Sam\nabout Friday" })).toBe("Text Sam about Friday");
    expect(summarizeProposedAction({ tool: "send_text", to: "+1555" })).toBe("send_text");
    expect(summarizeProposedAction({ to: "+1555", n: 2 })).toBe("to: +1555, n: 2");
    expect(summarizeProposedAction({})).toBe("No details");
    expect(summarizeProposedAction({ summary: "x".repeat(300) }, 20)).toHaveLength(20);
  });
});

describe("kernel approvals", () => {
  it("shortens the action hash and drops the sha256 prefix", () => {
    expect(shortActionHash("sha256:0123456789abcdef0123")).toBe("0123456789ab");
    expect(shortActionHash(null)).toBe("—");
  });
  it("reports the deadline state", () => {
    const now = Date.parse("2026-11-28T12:00:00Z");
    expect(kernelDeadlineState({ deadline: "2026-11-28T11:59:00Z" }, "pending", now)).toBe("expired");
    expect(kernelDeadlineState({ deadline: "2026-11-28T12:05:00Z" }, "pending", now)).toBe("open");
    expect(kernelDeadlineState({ deadline: null }, "pending", now)).toBe("none");
  });
  it("never lets the console decide a kernel row", () => {
    expect(canDecideInConsole({ source: "kernel", status: "pending" })).toBe(false);
    expect(canDecideInConsole({ source: "twin", status: "pending" })).toBe(true);
    expect(canDecideInConsole({ source: "twin", status: "approved" })).toBe(false);
  });
});
