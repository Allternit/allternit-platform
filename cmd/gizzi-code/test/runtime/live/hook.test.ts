// @ts-nocheck
import { describe, expect, test } from "bun:test"
import { mkdtempSync, mkdirSync, appendFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

process.env.ALLTERNIT_FACTORY_HOME = mkdtempSync(join(tmpdir(), "gizzi-live-inbox-"))
delete process.env.ALLTERNIT_LIVE_SESSION
const { inboxDir, inboxKey, markRead, readPending, unreadNotice } = await import("../../../src/runtime/live/inbox")
const { formatFor, runHook, takesMail } = await import("../../../src/cli/commands/hook")
const { toolList, rosterText } = await import("../../../src/runtime/live/team-mcp")

function deposit(key: string, id: string, from = "eoj/other-claude") {
  mkdirSync(inboxDir(), { recursive: true })
  const line = JSON.stringify({ id, from, envelope: `<allternit-message id="${id}">hi</allternit-message>`, at: "now" })
  appendFileSync(join(inboxDir(), `${key}.ndjson`), line + "\n")
}

describe("gizzi hook", () => {
  test("prints pending mail in the harness's JSON and marks it read only afterwards", async () => {
    deposit("proj-codex", "hop_1")
    const first = await runHook("codex", "post-tool-use", { cwd: "/work/proj" })
    expect(JSON.parse(first.out).hookSpecificOutput.hookEventName).toBe("PostToolUse")
    expect(first.out).toContain("hop_1")
    // Not read until the caller says stdout was written.
    expect((await readPending("proj-codex")).length).toBe(1)
    await markRead(first.key, first.ids)
    expect((await readPending("proj-codex")).length).toBe(0)
    expect((await runHook("codex", "post-tool-use", { cwd: "/work/proj" })).out).toBeUndefined()
  })

  test("harness formats and events that cannot take mail", () => {
    expect(JSON.parse(formatFor("codex", "stop", "x")).decision).toBe("block")
    expect(JSON.parse(formatFor("cursor", "stop", "x")).followup_message).toBe("x")
    expect(JSON.parse(formatFor("copilot", "tool", "x")).additionalContext).toBe("x")
    expect(takesMail("cursor", "user-prompt-submit")).toBe(false)
    expect(takesMail("codex", "doing")).toBe(false)
    expect(inboxKey("eoj/my repo-claude")).toBe("my-repo-claude")
  })
})

describe("team tools", () => {
  test("check_inbox is hidden for push sessions; the unread notice names senders", () => {
    expect(toolList({}).map((t) => t.name)).toContain("check_inbox")
    expect(toolList({ CLAUDE_CODE_MESSAGING_SOCKET: "/tmp/s" }).map((t) => t.name)).not.toContain("check_inbox")
    expect(unreadNotice([{ id: "1", from: "a", envelope: "", at: "" }, { id: "2", from: "b", envelope: "", at: "" }])).toBe(
      "[Allternit] 2 unread from a, b. Call check_inbox.",
    )
    expect(rosterText([{ address: "eoj/x-claude", harness: "claude", status: "busy", doing: "tests" }])).toContain("eoj/x-claude")
  })
})
