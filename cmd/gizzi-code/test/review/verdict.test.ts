import { describe, expect, test } from "bun:test"
import { ReviewVerdict } from "../../src/runtime/review/verdict"

const pass = { verdict: "PASS", verified: ["types line up"], findings: [], unverified: [], summary: "ok" }

describe("review verdict schema", () => {
  test("accepts a valid PASS and normalizes loose model output", () => {
    expect(ReviewVerdict.parse(pass).ok).toBe(true)
    const loose = ReviewVerdict.parse(
      JSON.stringify({ verdict: "fail", findings: [{ file: "a.ts", line: "12", problem: "x" }], summary: "bad" }),
    )
    expect(loose.ok).toBe(true)
    if (loose.ok) {
      expect(loose.value.verdict).toBe("FAIL")
      expect(loose.value.findings[0]!.line).toBe(12)
    }
  })

  test("FAIL needs a finding and PARTIAL needs an unverified item", () => {
    expect(ReviewVerdict.parse({ ...pass, verdict: "FAIL" }).ok).toBe(false)
    expect(ReviewVerdict.parse({ ...pass, verdict: "PARTIAL" }).ok).toBe(false)
    expect(ReviewVerdict.parse({ ...pass, verdict: "FAIL", findings: [{ file: "", problem: "x" }] }).ok).toBe(false)
  })

  test("repairs once, then falls back to PARTIAL with the reason", async () => {
    let calls = 0
    const repaired = await ReviewVerdict.parseWithRepair({ verdict: "FAIL" }, async () => {
      calls++
      return { ...pass, verdict: "FAIL", findings: [{ file: "a.ts", problem: "broken" }] }
    })
    expect(calls).toBe(1)
    expect(repaired).toMatchObject({ repaired: true, failed: false })
    expect(repaired.value.verdict).toBe("FAIL")

    const failed = await ReviewVerdict.parseWithRepair(undefined, async () => ({ verdict: "MAYBE" }))
    expect(failed.failed).toBe(true)
    expect(failed.value.verdict).toBe("PARTIAL")
    expect(failed.value.unverified.length).toBe(1)
    expect(ReviewVerdict.parse(failed.value).ok).toBe(true)
  })

  test("maps verdicts to gizzi run --review exit codes", () => {
    expect(ReviewVerdict.exitCode("PASS")).toBe(0)
    expect(ReviewVerdict.exitCode(null)).toBe(0)
    expect(ReviewVerdict.exitCode("FAIL")).toBe(3)
    expect(ReviewVerdict.exitCode("PARTIAL")).toBe(4)
  })

  test("json schema has no $schema key and requires the verdict fields", () => {
    const schema = ReviewVerdict.jsonSchema() as any
    expect(schema.$schema).toBeUndefined()
    expect(schema.required).toEqual(expect.arrayContaining(["verdict", "findings", "unverified", "summary"]))
  })
})
