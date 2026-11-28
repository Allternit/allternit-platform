import { describe, expect, test } from "bun:test"
import { appleScriptEscape, attachCommand, buildLaunch, formatLaunch, pickTerminal, shellQuote } from "./terminalLauncher"

const cmd = ["gizzi", "agents", "attach", "term_abc"]

describe("terminalLauncher", () => {
  test("shellQuote keeps hostile text one literal word", () => {
    expect(shellQuote("a b; rm -rf /")).toBe("'a b; rm -rf /'")
    expect(shellQuote("it's")).toBe(`'it'\\''s'`)
    expect(shellQuote("safe-1.2/x")).toBe("safe-1.2/x")
  })
  test("appleScriptEscape escapes quotes and backslashes", () => {
    expect(appleScriptEscape('a"b\\c')).toBe('a\\"b\\\\c')
  })
  test("Terminal.app uses do script", () => {
    const l = buildLaunch("terminal", cmd)
    expect(l.argv[0]).toBe("osascript")
    expect(l.argv.join(" ")).toContain('tell application "Terminal" to do script "gizzi agents attach term_abc"')
  })
  test("cwd is quoted into the AppleScript shell line", () => {
    const l = buildLaunch("terminal", cmd, "/tmp/my dir")
    expect(l.argv[l.argv.length - 1]).toContain("cd '/tmp/my dir' && gizzi agents attach term_abc")
  })
  test("iTerm2 creates a window with the command", () => {
    expect(buildLaunch("iterm", cmd).argv.join(" ")).toContain('create window with default profile command "gizzi agents attach term_abc"')
  })
  test("Ghostty uses open -na with -e and a working directory", () => {
    expect(buildLaunch("ghostty", cmd, "/w").argv).toEqual(["open", "-na", "Ghostty", "--args", "--working-directory=/w", "-e", ...cmd])
  })
  test("Windows Terminal uses wt.exe new-tab", () => {
    expect(buildLaunch("wt", cmd, "C:\\w").argv).toEqual(["wt.exe", "new-tab", "--startingDirectory", "C:\\w", "--", ...cmd])
  })
  test("Linux terminals", () => {
    expect(buildLaunch("gnome-terminal", cmd).argv).toEqual(["gnome-terminal", "--", ...cmd])
    expect(buildLaunch("konsole", cmd).argv).toEqual(["konsole", "-e", ...cmd])
    expect(buildLaunch("x-terminal-emulator", cmd).argv.slice(0, 2)).toEqual(["x-terminal-emulator", "-e"])
    expect(buildLaunch("alacritty", cmd).argv.slice(0, 2)).toEqual(["alacritty", "-e"])
    expect(buildLaunch("kitty", cmd).argv).toEqual(["kitty", ...cmd])
    expect(buildLaunch("foot", cmd, "/w")).toEqual({ argv: ["foot", ...cmd], cwd: "/w" })
  })
  test("pick: override, TERM_PROGRAM, OS default", () => {
    expect(pickTerminal({ platform: "darwin", env: { TERM_PROGRAM: "iTerm.app" } })).toBe("iterm")
    expect(pickTerminal({ platform: "darwin", env: { TERM_PROGRAM: "ghostty" } })).toBe("ghostty")
    expect(pickTerminal({ platform: "darwin", env: { TERM_PROGRAM: "Apple_Terminal" } })).toBe("terminal")
    expect(pickTerminal({ platform: "darwin", env: { TERM_PROGRAM: "vscode" } })).toBe("terminal")
    expect(pickTerminal({ platform: "win32", env: {} })).toBe("wt")
    expect(pickTerminal({ platform: "linux", env: { TERMINAL: "/usr/bin/kitty" } })).toBe("kitty")
    expect(pickTerminal({ platform: "linux", env: {}, has: (p) => p === "konsole" })).toBe("konsole")
    expect(pickTerminal({ platform: "linux", env: {}, override: "foot" })).toBe("foot")
    expect(() => pickTerminal({ platform: "linux", env: {} })).toThrow()
    expect(() => pickTerminal({ platform: "linux", env: {}, override: "nope" })).toThrow()
  })
  test("hostile ids are quoted, not interpolated", () => {
    const l = buildLaunch("terminal", attachCommand("terminal", "x'; rm -rf ~ #"))
    expect(l.argv[l.argv.length - 1]).toContain("rm -rf ~ #")
    expect(l.argv[l.argv.length - 1]).toContain("'x'")
  })
  test("attach commands", () => {
    expect(attachCommand("terminal", "t1")).toEqual(["gizzi", "agents", "attach", "t1"])
    expect(attachCommand("engine", "ses_1")).toEqual(["gizzi", "--session", "ses_1"])
    expect(formatLaunch({ argv: ["a", "b c"], cwd: "/x" })).toBe("(cwd /x) a 'b c'")
  })
})
