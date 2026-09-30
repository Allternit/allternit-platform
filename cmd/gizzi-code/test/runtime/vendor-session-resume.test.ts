import { describe, expect, test } from "bun:test"
import {
  acpCanLoadSession,
  claudeSessionFlags,
  claudeSessionIdFromEvent,
} from "@/runtime/drivers/cli-session-flags"

describe("vendor session capture", () => {
  test("claude session_id is read from system init and result events", () => {
    expect(claudeSessionIdFromEvent({ type: "system", subtype: "init", session_id: "abc" })).toBe("abc")
    expect(claudeSessionIdFromEvent({ type: "result", session_id: "def" })).toBe("def")
  })
  test("other events and empty ids are ignored", () => {
    expect(claudeSessionIdFromEvent({ type: "assistant", session_id: "x" })).toBeUndefined()
    expect(claudeSessionIdFromEvent({ type: "system", session_id: "" })).toBeUndefined()
    expect(claudeSessionIdFromEvent(null)).toBeUndefined()
  })
})

describe("vendor session reuse", () => {
  test("claude gets --resume only when an id was captured", () => {
    expect(claudeSessionFlags({})).not.toContain("--resume")
    const flags = claudeSessionFlags({ vendorSessionId: "abc" })
    expect(flags.slice(flags.indexOf("--resume"))[1]).toBe("abc")
  })
  test("ACP resume requires the loadSession capability", () => {
    expect(acpCanLoadSession({ loadSession: true })).toBe(true)
    expect(acpCanLoadSession({ loadSession: false })).toBe(false)
    expect(acpCanLoadSession(undefined)).toBe(false)
  })
})
