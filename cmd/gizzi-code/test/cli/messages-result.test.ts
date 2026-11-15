// @ts-nocheck
import { describe, expect, test } from "bun:test"
import { formatMessagesResult, SEARCH_MAX_ROWS } from "../../src/cli/ui/ink-app/tools/MCPTool/messagesResult"

describe("messages tool results", () => {
  test("held send shows recipient, text and the approval line", () => {
    const v = formatMessagesResult(JSON.stringify({ held: true, inboxId: "approval:1" }), { to: "Mom", text: "On my way" })
    expect(v.kind).toBe("held")
    expect(v.lines[0]).toBe("Waiting for your approval, here or on your phone")
    expect(v.lines.join("\n")).toContain("To: Mom")
    expect(v.lines.join("\n")).toContain("On my way")
  })
  test("draft shows to, name, text and the phone line", () => {
    const v = formatMessagesResult([{ type: "text", text: JSON.stringify({ type: "message_draft", to: "+1555", toName: "Sam", text: "Hi" }) }])
    expect(v.kind).toBe("draft")
    expect(v.lines[0]).toContain("Sam (+1555)")
    expect(v.lines).toContain("Draft only. Send it from your phone.")
  })
  test("search is compact, capped, marked outside, first line only", () => {
    const rows = Array.from({ length: 12 }, (_, i) => ({ sender: `s${i}`, time: 1700000000, text: `line one\nline two ${i}` }))
    const v = formatMessagesResult(JSON.stringify(rows))
    expect(v.kind).toBe("search")
    expect(v.lines[0]).toContain("outside content")
    expect(v.lines.length).toBe(1 + SEARCH_MAX_ROWS + 1)
    expect(v.lines[1]).not.toContain("line two")
    expect(v.lines.at(-1)).toBe("and 4 more")
  })
  test("other output is left alone", () => {
    expect(formatMessagesResult("plain text")).toBeNull()
    expect(formatMessagesResult(JSON.stringify({ ok: true }))).toBeNull()
  })
})
