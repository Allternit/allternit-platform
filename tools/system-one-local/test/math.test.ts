import { describe, expect, test } from "bun:test";
import { confidence, labelDistribution, normalize, roundDistribution, voteDistribution, weightedScore } from "../src/math.ts";

describe("confidence = (n·max − 1)/(n − 1)", () => {
  test("uniform → 0", () => {
    expect(confidence([1 / 3, 1 / 3, 1 / 3])).toBeCloseTo(0, 10);
    expect(confidence([0.5, 0.5])).toBeCloseTo(0, 10);
  });
  test("one-hot → 1", () => expect(confidence([0, 1, 0, 0])).toBe(1));
  test("0.88/0.12/0 → 0.82 (docs show 0.81 from unrounded probs)", () => expect(confidence([0.88, 0.12, 0])).toBeCloseTo(0.82, 10));
  test("docs explorer default 0.9/0.06/0.04 → 0.85", () => expect(confidence([0.9, 0.06, 0.04])).toBeCloseTo(0.85, 10));
  test("clamped to [0,1]", () => {
    expect(confidence([0.2, 0.2, 0.2, 0.2, 0.2])).toBeGreaterThanOrEqual(0);
    expect(confidence([1.2, 0])).toBe(1);
  });
});

describe("weightedScore = Σ i·p_i", () => {
  test("docs example 0/0.95/0.05 → 1.05", () => expect(weightedScore([0, 0.95, 0.05])).toBeCloseTo(1.05, 10));
  test("can land between levels", () => expect(weightedScore([0.5, 0, 0.5])).toBeCloseTo(1, 10));
  test("top level", () => expect(weightedScore([0, 0, 0, 1])).toBe(3));
});

describe("labelDistribution (top-k → renormalized over valid labels)", () => {
  const lp = (p: number) => Math.log(p);
  test("renormalizes over valid labels only, ignoring junk tokens", () => {
    const d = labelDistribution(
      [{ token: "A", logprob: lp(0.6) }, { token: "The", logprob: lp(0.2) }, { token: "B", logprob: lp(0.2) }],
      ["A", "B"],
    )!;
    expect(d.probs[0]).toBeCloseTo(0.75, 10);
    expect(d.probs[1]).toBeCloseTo(0.25, 10);
    expect(d.labelMass).toBeCloseTo(0.8, 10);
  });
  test("sums leading-space and case variants", () => {
    const d = labelDistribution(
      [{ token: "Yes", logprob: lp(0.4) }, { token: " yes", logprob: lp(0.2) }, { token: "No", logprob: lp(0.2) }],
      ["Yes", "No"], { caseInsensitive: true },
    )!;
    expect(d.probs[0]).toBeCloseTo(0.75, 10);
  });
  test("letters are case-sensitive (lowercase 'a' is a word, not label A)", () => {
    const d = labelDistribution([{ token: "a", logprob: lp(0.5) }, { token: "B", logprob: lp(0.5) }], ["A", "B"])!;
    expect(d.probs[1]).toBeGreaterThan(0.9);
  });
  test("unseen labels get a floor ≤ min shown prob and ≤ leftover share", () => {
    const d = labelDistribution([{ token: "A", logprob: lp(0.9) }, { token: "B", logprob: lp(0.05) }], ["A", "B", "C"])!;
    // leftover = 0.05, min shown = 0.05, 1 unseen → floor 0.05; total 1.0
    expect(d.probs[2]).toBeCloseTo(0.05, 10);
    expect(d.probs.reduce((a, b) => a + b)).toBeCloseTo(1, 10);
    expect(d.seen).toBe(2);
  });
  test("no valid label in top-k → null (caller must not invent a distribution)", () => {
    expect(labelDistribution([{ token: "The", logprob: -0.1 }], ["A", "B"])).toBeNull();
  });
});

describe("helpers", () => {
  test("normalize handles zero sum as uniform", () => expect(normalize([0, 0])).toEqual([0.5, 0.5]));
  test("voteDistribution counts valid labels only", () => {
    expect(voteDistribution(["A", "A", "B", null, "Z"], ["A", "B"])).toEqual([2 / 3, 1 / 3]);
  });
  test("roundDistribution sums to exactly 1", () => {
    const r = roundDistribution([1 / 3, 1 / 3, 1 / 3]);
    expect(r.reduce((a, b) => a + b)).toBeCloseTo(1, 12);
  });
});
