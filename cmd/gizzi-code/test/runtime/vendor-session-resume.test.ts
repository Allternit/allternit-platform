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

import {
  codexThreadRequest,
  opencodeResumeFlags,
  qwenResumeFlags,
  vendorSessionIdFromEvent,
} from "@/runtime/drivers/cli-session-flags"

describe("codex / opencode / qwen resume", () => {
  test("qwen: session_id from stream-json, --resume on reuse", () => {
    expect(vendorSessionIdFromEvent("qwen-cli", { type: "system", session_id: "q1" })).toBe("q1")
    expect(qwenResumeFlags("q1")).toEqual(["--resume", "q1"])
    expect(qwenResumeFlags()).toEqual([])
  })
  test("opencode: sessionID on any event, --session on reuse", () => {
    expect(vendorSessionIdFromEvent("opencode", { type: "text", sessionID: "ses_1" })).toBe("ses_1")
    expect(vendorSessionIdFromEvent("opencode", { type: "text" })).toBeUndefined()
    expect(opencodeResumeFlags("ses_1")).toEqual(["--session", "ses_1"])
    expect(opencodeResumeFlags()).toEqual([])
  })
  test("unlisted CLIs capture nothing", () => {
    expect(vendorSessionIdFromEvent("cursor-agent", { type: "system", session_id: "x" })).toBeUndefined()
  })
  test("codex: thread/resume with threadId when an id exists, else thread/start", () => {
    const p = { cwd: "/w" }
    expect(codexThreadRequest(undefined, p)).toEqual({ method: "thread/start", params: p })
    expect(codexThreadRequest("t1", p)).toEqual({ method: "thread/resume", params: { cwd: "/w", threadId: "t1" } })
  })
})
