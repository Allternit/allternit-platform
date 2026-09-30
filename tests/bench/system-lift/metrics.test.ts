import { expect, test } from "bun:test"
import { bootstrap, markdown, summarize, wilson, type Observation } from "./metrics"
import { emptyUsage } from "./types"

const row = (id: string, mode: "naked" | "system", fixed: boolean): Observation => ({
  fixtureId: id, category: "off-by-one", mode, backendId: "test-backbone", fixed, regression: false,
  integrity: true, completed: fixed, receiptsPass: mode === "system" ? fixed : null, verifierPass: mode === "system" ? fixed : null,
  usage: { ...emptyUsage(), input: 10, output: 2 }, calls: 1, wallMs: 10, runnerWallMs: 8, verificationWallMs: 2,
  evidence: null, patchAccepted: fixed, checks: { target: fixed, regression: true },
})
test("Wilson includes uncertainty at zero and one", () => {
  expect(wilson(0, 10).low).toBeCloseTo(0)
  expect(wilson(0, 10).high).toBeCloseTo(0.27753, 4)
  expect(wilson(10, 10).low).toBeCloseTo(0.72247, 4)
  expect(wilson(10, 10).high).toBeCloseTo(1)
  expect(() => wilson(0, 0)).toThrow()
})
test("bootstrap is seeded and preserves paired outcomes", () => {
  expect(bootstrap([0, 1, -1, 1], 13)).toEqual(bootstrap([0, 1, -1, 1], 13))
  expect(bootstrap([0, 0], 99)).toEqual({ estimate: 0, low: 0, high: 0 })
  const summary = summarize([row("a", "naked", false), row("a", "system", true), row("b", "system", true), row("b", "naked", true)], 13)
  expect(summary.lift.fixRate.estimate).toBe(0.5)
  expect(summary.lift.fixRate.low).toBe(0)
  expect(summary.lift.fixRate.high).toBe(1)
  expect(summary.naked.receiptsPassRate).toBeNull()
  expect(markdown(summary, false, ["offline"])).toContain("HARNESS ONLY")
  expect(markdown(summary, false, [])).toContain("50.0 pp")
})
test("unpaired, duplicate, swapped backbones and missing usage fail honestly", () => {
  expect(() => summarize([row("a", "naked", false)], 1)).toThrow()
  expect(() => summarize([row("a", "naked", false), row("a", "naked", true)], 1)).toThrow()
  expect(() => summarize([row("a", "naked", false), { ...row("a", "system", true), backendId: "different" }], 1)).toThrow()
  const summary = summarize([{ ...row("a", "naked", false), usage: null }, row("a", "system", true)], 1)
  expect(summary.naked.meanTokens).toBeNull()
  expect(summary.lift.tokens).toBeNull()
})
