import { describe, expect, it } from "vitest";
import { formatPct, hasOutcomes, topAnswer } from "../decisions";

describe("topAnswer", () => {
  it("formats predicates", () => {
    expect(topAnswer({ name: "a", type: "predicate", probability: 0.92 })).toBe("yes 92%");
    expect(topAnswer({ name: "a", type: "predicate", probability: 0.1 })).toBe("no 90%");
  });
  it("formats choices with the chosen probability", () => {
    expect(
      topAnswer({ name: "d", type: "choice", choice: "billing", confidence: 0.998, probabilities: [{ value: "billing", probability: 0.998 }, { value: "shipping", probability: 0.002 }] }),
    ).toBe("billing 99.8%");
  });
  it("formats scores with the most likely label", () => {
    expect(
      topAnswer({ name: "u", type: "score", score: 0.98, probabilities: [{ label: "low", value: 0, probability: 0.015 }, { label: "high", value: 2, probability: 0.98 }] }),
    ).toBe("high · 0.98");
  });
  it("names refusals", () => {
    expect(topAnswer({ name: "x", type: "refusal", reason: "budget_exhausted" })).toBe("Budget exhausted");
  });
  it("formats percents and outcomes", () => {
    expect(formatPct(0.5)).toBe("50%");
    expect(hasOutcomes({ outcomes: {} })).toBe(false);
    expect(hasOutcomes({ outcomes: { a: { status: "success" } } })).toBe(true);
  });
});
