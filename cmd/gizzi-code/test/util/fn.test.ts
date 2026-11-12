import { describe, expect, test } from "bun:test"
import { z } from "zod/v4"
import { fn } from "../../src/shared/util/fn"

// Regression: `gizzi teleport <claude-session-id>` failed with "must start with ses"
// because Session.get threw synchronously and skipped its `.catch(() => undefined)`.
describe("fn", () => {
  const id = z.string().startsWith("ses")

  test("async callback: a bad input rejects, so .catch handles it", async () => {
    const get = fn(id, async (s) => ({ id: s }))
    expect(await get("7f7e5d0f-claude-id").catch(() => undefined)).toBeUndefined()
    expect(await get("ses_1")).toEqual({ id: "ses_1" })
  })

  test("sync callback: a bad input still throws synchronously", () => {
    const get = fn(id, (s) => s.length)
    expect(() => get("nope")).toThrow()
    expect(get("ses_12")).toBe(6)
  })
})
