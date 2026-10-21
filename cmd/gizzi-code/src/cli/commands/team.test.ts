import { describe, expect, test } from "bun:test"
import { formatRoster } from "./team-format"

describe("formatRoster", () => {
  test("empty roster explains how to get teammates", () => {
    expect(formatRoster({ people: [] })[0]).toContain("Invite teammates")
  })
  test("people → machines → sessions with doing line", () => {
    const out = formatRoster({
      people: [{ display: "Eoj", me: true, unread: 2, held: 0, machines: [{ name: "MacBook", online: true, bots: [], sessions: [{ address: "eoj/a-claude", harness: "claude", status: "busy", doing: "fixing tests", tier: "push" }] }] }],
    }).join("\n")
    expect(out).toContain("Eoj (you)  [2 unread]")
    expect(out).toContain("● MacBook")
    expect(out).toContain("eoj/a-claude  claude  busy  push")
    expect(out).toContain("fixing tests")
  })
})
