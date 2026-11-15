import { describe, expect, test } from "bun:test"
import { describeVerification, pullExitCode } from "./modelVerification"

const base = { source_used: "publisher", source_url: "x", checked_at: "2026-11-14T00:00:00Z" } as const

describe("pullExitCode", () => {
  test("unverified fails unless allowed", () => {
    const v = { ...base, state: "unverified" } as const
    expect(pullExitCode(v, false)).not.toBe(0)
    expect(pullExitCode(v, true)).toBe(0)
  })
  test("older engine (no field) prints a notice and does not fail", () => {
    expect(pullExitCode(undefined, false)).toBe(0)
    expect(describeVerification(undefined).text).toContain("Not checked by this engine version")
  })
  test("verified states pass", () => {
    expect(pullExitCode({ ...base, state: "publisher_hash" }, false)).toBe(0)
    expect(describeVerification({ ...base, state: "freeweights_signed", manifest_commit: "abc1234ffff" }).text).toBe(
      "✓ Verified by Freeweights (commit abc1234)",
    )
  })
})
