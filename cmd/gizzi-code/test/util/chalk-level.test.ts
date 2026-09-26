import { describe, expect, test } from "bun:test"
import { detectTTYLevel } from "../../src/shared/util/chalk"

const SONOMA = "23.6.0"
const TAHOE = "25.0.0"

describe("detectTTYLevel", () => {
  test("Apple Terminal before macOS 26 gets 256 colors, not truecolor", () => {
    expect(detectTTYLevel({ TERM: "xterm-256color", TERM_PROGRAM: "Apple_Terminal" }, "darwin", SONOMA)).toBe(2)
  })

  test("Apple Terminal on macOS 26+ gets truecolor", () => {
    expect(detectTTYLevel({ TERM: "xterm-256color", TERM_PROGRAM: "Apple_Terminal" }, "darwin", TAHOE)).toBe(3)
  })

  test("COLORTERM=truecolor wins", () => {
    expect(detectTTYLevel({ TERM: "xterm-256color", COLORTERM: "truecolor" }, "darwin", SONOMA)).toBe(3)
    expect(detectTTYLevel({ TERM: "xterm", COLORTERM: "24bit" }, "linux", "")).toBe(3)
  })

  test("known truecolor terminals", () => {
    expect(detectTTYLevel({ TERM_PROGRAM: "iTerm.app", TERM: "xterm-256color" }, "darwin", SONOMA)).toBe(3)
    expect(detectTTYLevel({ TERM: "xterm-ghostty" }, "darwin", SONOMA)).toBe(3)
    expect(detectTTYLevel({ TERM: "xterm-kitty" }, "linux", "")).toBe(3)
  })

  test("plain 256-color TERM without other hints gets 256 colors", () => {
    expect(detectTTYLevel({ TERM: "xterm-256color" }, "linux", "")).toBe(2)
    expect(detectTTYLevel({ TERM: "tmux-256color" }, "darwin", SONOMA)).toBe(2)
  })

  test("basic terminals get 16 colors", () => {
    expect(detectTTYLevel({ TERM: "xterm" }, "linux", "")).toBe(1)
    expect(detectTTYLevel({ TERM: "screen" }, "linux", "")).toBe(1)
  })

  test("windows terminals get truecolor", () => {
    expect(detectTTYLevel({}, "win32", "10.0.22631")).toBe(3)
  })
})
