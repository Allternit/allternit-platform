import { describe, expect, it } from "vitest";
import { approvalStatusLabel, summarizeProposedAction } from "../approvals";

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
