import { describe, expect, test } from "bun:test"
import { PET_OFFER_FIXTURES } from "../../src/cli/ui/ink-app/pet/petOffers.gen"
import {
  buildFixPrompt,
  createOfferGate,
  evaluatePetFailure,
  matchPetOffer,
} from "../../src/cli/ui/ink-app/pet/petOffers"

describe("pet offers matcher", () => {
  for (const f of PET_OFFER_FIXTURES) {
    test(`fixture: ${f.name}`, () => {
      const m = matchPetOffer(f.text, { dialog: f.dialog })
      expect(m?.kind ?? null).toBe(f.expected)
      if (f.rule) expect(m?.ruleId).toBe(f.rule)
    })
  }

  test("matched line is the whole trimmed line, cut to 240", () => {
    const m = matchPetOffer(`ok\nnpm error code 1 ${"x".repeat(400)}\nmore`)
    expect(m?.line.startsWith("npm error code 1")).toBe(true)
    expect(m?.line.length).toBe(240)
  })
})

describe("pet offers gate", () => {
  test("cooldown per directory, never the same line twice", () => {
    const g = createOfferGate()
    expect(g.allow("/a", "boom", 0, 1000)).toBe(true)
    expect(g.allow("/a", "other", 500, 1000)).toBe(false) // cooldown
    expect(g.allow("/b", "other", 500, 1000)).toBe(true) // other directory
    expect(g.allow("/a", "boom", 5000, 1000)).toBe(false) // same line again
    expect(g.allow("/a", "new", 5000, 1000)).toBe(true)
  })

  test("muted: still recorded for /fix, but no bubble", () => {
    const r = evaluatePetFailure({ cwd: "/m", text: "npm error code 1\nnpm error path /x", needMatch: false, enabled: false })
    expect(r?.offer).toBeUndefined()
    expect(buildFixPrompt(r?.failure)).toContain("npm error code 1")
  })

  test("agent Bash output without a rule match makes no offer", () => {
    expect(evaluatePetFailure({ cwd: "/n", text: "all good", needMatch: true, enabled: true })).toBeUndefined()
  })

  test("user shell failure with no rule falls back to run_failed copy", () => {
    const r = evaluatePetFailure({ cwd: "/o", text: "weird thing", needMatch: false, enabled: true })
    expect(r?.offer).toBe("That run failed. Fix it? Type /fix")
  })
})
