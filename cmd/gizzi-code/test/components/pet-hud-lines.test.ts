import { describe, expect, test } from "bun:test"
import {
  earlierWindowAtTop,
  hudLinesFromMessages,
  maxOffset,
  visibleLines,
  type HudLine,
} from "../../src/cli/ui/ink-app/pet/hudLines"

// The pet HUD's Thread scrollback (spec P3.16): scrolling up past a rip reads
// the window it continued from, like Desktop's "Show the earlier conversation".
describe("pet HUD thread lines", () => {
  const seed = {
    id: "m0",
    role: "user",
    content: "[checkpoint: window 1] Decided: ship Friday.",
    timestamp: "2026-09-28T10:00:00Z",
    metadata: { parts: [{ type: "text", metadata: { handoff: { from: "ses_gen1", generation: 1, reason: "threshold" } } }] },
  }

  test("a window's seed becomes a rip that remembers the earlier window", () => {
    const lines = hudLinesFromMessages([
      seed,
      { id: "m1", role: "user", content: "status?" },
      { id: "m2", role: "assistant", content: "[No text content]" },
      { id: "m3", role: "assistant", content: "On track." },
    ])
    expect(lines).toEqual([
      { role: "rip", generation: 2, reason: "threshold", at: "2026-09-28T10:00:00Z", from: "ses_gen1" },
      { role: "user", content: "status?" },
      { role: "assistant", content: "On track." },
    ])
  })

  test("scrolls by offset from the newest line", () => {
    const lines: HudLine[] = ["a", "b", "c", "d", "e", "f"].map((content) => ({ role: "user", content }))
    const text = (ls: HudLine[]) => ls.map((l) => (l as { content: string }).content).join("")
    expect(text(visibleLines(lines, 0, 4))).toBe("cdef")
    expect(text(visibleLines(lines, 2, 4))).toBe("abcd")
    expect(maxOffset(lines, 4)).toBe(2)
    expect(maxOffset(lines.slice(0, 3), 4)).toBe(0)
  })

  test("only an unloaded rip at the top has an earlier window to load", () => {
    const lines = hudLinesFromMessages([seed, { id: "m1", role: "user", content: "hi" }])
    expect(earlierWindowAtTop(lines, new Set())).toBe("ses_gen1")
    expect(earlierWindowAtTop(lines, new Set(["ses_gen1"]))).toBeNull()
    expect(earlierWindowAtTop(lines.slice(1), new Set())).toBeNull()
    expect(earlierWindowAtTop([], new Set())).toBeNull()
  })
})
