import { describe, test, expect, beforeAll, afterAll } from "bun:test"
import chalk from "../../src/shared/util/chalk"
import {
  ColorDiff,
  ColorFile,
  getSyntaxTheme,
  listSyntaxThemes,
} from "../../src/vendor/color-diff-napi"
import { tokenizeLine, resolveLanguage, mapThemeName, wrapSpans } from "../../src/vendor/color-diff-napi/syntax"
import { StructuredDiff } from "../../src/cli/ui/ink-app/components/StructuredDiff"
import { HighlightedCode } from "../../src/cli/ui/ink-app/components/HighlightedCode"

// Colors follow the terminal's level; pin truecolor (tests run without a TTY).
let savedLevel = 0
beforeAll(() => {
  savedLevel = chalk.level
  chalk.level = 3
})
afterAll(() => {
  chalk.level = savedLevel
})

const ANSI_RE = /\x1b\[[0-9;?]*[a-zA-Z]/g
const stripAnsi = (s: string) => s.replace(ANSI_RE, "")
const printableLen = (s: string) => stripAnsi(s).length

const PATCH = {
  oldStart: 10,
  oldLines: 3,
  newStart: 10,
  newLines: 3,
  lines: [" context();", '-const bad = "old";', '+const good = "new";', "+added();", ""].map((l, i) => (i === 4 ? " " : l)),
}

describe("color-diff-napi vendor shim", () => {
  test("color-math mode: diff/similar/closest/map", () => {
    const cd = new ColorDiff()
    expect(cd.diff({ r: 255, g: 0, b: 0 }, { r: 255, g: 0, b: 0 })).toBeCloseTo(0)
    expect(cd.similar({ r: 255, g: 0, b: 0 }, { r: 250, g: 5, b: 5 })).toBe(true)
    expect(cd.closest({ r: 255, g: 0, b: 0 }, [])).toBeNull()
    expect(cd.closest({ r: 255, g: 0, b: 0 }, [{ r: 0, g: 0, b: 255 }])).toEqual({ r: 0, g: 0, b: 255 })
    expect(cd.map({ r: 255, g: 0, b: 0 }, [{ r: 0, g: 0, b: 255 }, { r: 255, g: 0, b: 0 }])).toBe(1)
  })

  test("custom threshold is honored", () => {
    const strict = new ColorDiff(0.1)
    expect(strict.similar({ r: 255, g: 0, b: 0 }, { r: 250, g: 5, b: 5 })).toBe(false)
  })

  test("diff-render construction accepts (patch, firstLine, filePath, fileContent)", () => {
    const patch = { oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-a", "+b"] }
    // Regression: gizzi-code 2.0.5 crashed with
    // "new ColorDiff(...).render is not a function" (TypeError) because the
    // TS port only implemented the color-math API.
    let instance: ColorDiff
    expect(() => {
      instance = new ColorDiff(patch, null, "foo.ts", "a\nb")
    }).not.toThrow()
    expect(typeof (instance! as unknown as { render?: unknown }).render).toBe("function")
  })

  test("render() emits guttered, tinted, syntax-highlighted lines within width", () => {
    const patch = {
      oldStart: 10, oldLines: 2, newStart: 10, newLines: 2,
      lines: [" context();", '-const bad = "old";', '+const good = "new";'],
    }
    const lines = new ColorDiff(patch, null, "foo.ts", "").render("dark", 80, false)
    expect(Array.isArray(lines)).toBe(true)
    expect(lines!.length).toBe(3)
    for (const l of lines!) {
      expect(printableLen(l)).toBeLessThanOrEqual(80)
    }
    // Gutter (Claude Code layout): 2-digit number + space + marker + space =
    // 5 cells for lines 10-11. Context lines show the new-file number; removed
    // lines the old-file number.
    expect(stripAnsi(lines![0]!).startsWith("10   ")).toBe(true)
    expect(stripAnsi(lines![1]!).startsWith("11 - ")).toBe(true)
    expect(stripAnsi(lines![2]!).startsWith("11 + ")).toBe(true)
    // The gutter carries the row tint, marker in the theme's diff word color.
    expect(lines![1]!.startsWith("\x1b[48;2;122;41;54m\x1b[38;2;179;89;107m11 - ")).toBe(true)
    expect(lines![2]!.startsWith("\x1b[48;2;34;92;43m\x1b[38;2;56;166;96m11 + ")).toBe(true)
    // Diff background tints (dark theme: added rgb(34,92,43), removed rgb(122,41,54)).
    expect(lines![1]).toContain("48;2;122;41;54")
    expect(lines![2]).toContain("48;2;34;92;43")
    // Keyword syntax color for `const` (dark-plus #569cd6).
    expect(lines![2]).toContain("38;2;86;156;214")
    // String literal syntax color (#ce9178).
    expect(lines![2]).toContain("38;2;206;145;120")
  })

  test("render() respects dim and light theme palettes", () => {
    const patch = { oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-const x = 1;", "+const y = 2;"] }
    const dark = new ColorDiff(patch, null, "a.ts", "").render("dark", 80, true)!
    const light = new ColorDiff(patch, null, "a.ts", "").render("light", 80, false)!
    // Dimmed dark removed/added backgrounds.
    expect(dark[0]).toContain("48;2;105;72;77")
    expect(dark[1]).toContain("48;2;71;88;74")
    // Light theme backgrounds.
    expect(light[0]).toContain("48;2;255;168;180")
    expect(light[1]).toContain("48;2;105;219;124")
    // Light keyword color (#0000ff) instead of dark (#569cd6 = 86;156;214).
    expect(light[1]).toContain("38;2;0;0;255")
    expect(dark[1]).toContain("38;2;86;156;214")
  })

  test("render() wraps long lines and pads tinted backgrounds to content width", () => {
    const longLine = "+" + "x".repeat(60) + ' "str"'
    const patch = { oldStart: 1, oldLines: 0, newStart: 1, newLines: 1, lines: [longLine] }
    const lines = new ColorDiff(patch, null, "a.ts", "").render("dark", 40, false)!
    expect(lines.length).toBeGreaterThan(1)
    for (const l of lines) expect(printableLen(l)).toBeLessThanOrEqual(40)
    // Every wrapped row of an added line carries the added background.
    for (const l of lines) expect(l).toContain("48;2;34;92;43")
    // Continuation rows have a blank gutter.
    expect(stripAnsi(lines[1]!).startsWith("    ")).toBe(true)
  })

  test("render() follows the terminal color level", () => {
    const patch = { oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-const x = 1;", "+const y = 2;"] }
    try {
      // Apple Terminal before macOS 26: 256 colors, never 24-bit.
      chalk.level = 2
      const l256 = new ColorDiff(patch, null, "a.ts", "").render("dark", 80, false)!
      expect(l256.join("")).not.toContain("38;2;")
      expect(l256.join("")).not.toContain("48;2;")
      expect(l256[1]).toContain("48;5;")
      // Color off: plain text, same layout.
      chalk.level = 0
      const plain = new ColorDiff(patch, null, "a.ts", "").render("dark", 80, false)!
      expect(plain.join("")).not.toContain("\x1b[")
      expect(plain).toEqual(["1 - const x = 1;", "1 + const y = 2;"])
    } finally {
      chalk.level = 3
    }
  })

  test("render() handles no-newline marker lines", () => {
    const patch = { oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-a", "\\ No newline at end of file", "+b"] }
    const lines = new ColorDiff(patch, null, "a.txt", "").render("dark", 80, false)!
    expect(lines.length).toBe(3)
    expect(stripAnsi(lines[1]!)).toContain("No newline at end of file")
  })

  test("render() returns null in color-math mode and for bad widths", () => {
    expect(new ColorDiff().render("dark", 80, false)).toBeNull()
    const patch = { oldStart: 1, oldLines: 1, newStart: 1, newLines: 1, lines: ["-a", "+b"] }
    expect(new ColorDiff(patch, null, "a.ts", "").render("dark", 5, false)).toBeNull()
  })

  test("ColorFile render-mode: guttered highlighted lines within width", () => {
    const code = 'const a = 1\nconsole.log("hi")\n'
    const lines = new ColorFile(code, "foo.ts").render("dark", 60, false)
    expect(Array.isArray(lines)).toBe(true)
    expect(lines!.length).toBe(3)
    expect(stripAnsi(lines![0]!).startsWith(" 1 ")).toBe(true)
    expect(printableLen(lines![0]!)).toBeLessThanOrEqual(60)
    // `const` keyword color.
    expect(lines![0]).toContain("38;2;86;156;214")
    // console.log call: function color (#dcdcaa).
    expect(lines![1]).toContain("38;2;220;220;170")
  })

  test("ColorFile render-mode: wraps long lines with blank continuation gutter", () => {
    const code = "x".repeat(100)
    const lines = new ColorFile(code, "a.txt").render("dark", 40, false)!
    expect(lines.length).toBeGreaterThan(1)
    for (const l of lines) expect(printableLen(l)).toBeLessThanOrEqual(40)
    expect(stripAnsi(lines[1]!).startsWith("   ")).toBe(true)
  })

  test("ColorFile palette mode is unchanged", () => {
    expect(ColorFile.hexToRgb("ff0000")).toEqual({ r: 255, g: 0, b: 0 })
    expect(ColorFile.hexToRgb("f00")).toEqual({ r: 255, g: 0, b: 0 })
    const cf = new ColorFile()
    cf.add({ r: 1, g: 2, b: 3 })
    expect(cf.getColors()).toEqual([{ r: 1, g: 2, b: 3 }])
    expect(cf.render("dark", 80, false)).toBeNull()
    cf.clear()
    expect(cf.getColors()).toEqual([])
  })

  test("tokenizer: language rules mark strings/keywords/comments", () => {
    const js = tokenizeLine('const s = "hi" // note', "typescript")
    expect(js.find((t) => t.text === "const")?.type).toBe("keyword")
    expect(js.find((t) => t.text === '"hi"')?.type).toBe("string")
    expect(js.find((t) => t.text.startsWith("//"))?.type).toBe("comment")

    const py = tokenizeLine("def f(x):  # comment", "python")
    expect(py.find((t) => t.text === "def")?.type).toBe("keyword")
    expect(py.find((t) => t.text.startsWith("#"))?.type).toBe("comment")

    const sh = tokenizeLine('echo "$HOME" # c', "shell")
    expect(sh.find((t) => t.text === '"$HOME"')?.type).toBe("string")
    expect(sh.find((t) => t.text === "echo")?.type).toBe("keyword")
  })

  test("resolveLanguage: by extension and shebang", () => {
    expect(resolveLanguage("a/b.ts")).toBe("typescript")
    expect(resolveLanguage("x.py")).toBe("python")
    expect(resolveLanguage("noext", "#!/usr/bin/env python3")).toBe("python")
    expect(resolveLanguage("noext", "#!/bin/bash")).toBe("shell")
    expect(resolveLanguage("mystery.xyz")).toBe("text")
  })

  test("mapThemeName: theme families", () => {
    expect(mapThemeName("dark").syntaxThemeName).toBe("dark-plus")
    expect(mapThemeName("light").syntaxThemeName).toBe("light-plus")
    expect(mapThemeName("dark-daltonized").palette.added).toEqual([0, 68, 102])
    expect(mapThemeName("light-daltonized").palette.added).toEqual([153, 204, 255])
    expect(mapThemeName("dark-ansi").palette.ansi).toBe(true)
    expect(mapThemeName("").syntaxThemeName).toBe("dark-plus")
  })

  test("wrapSpans: hard-breaks overlong spans and splits at width", () => {
    const rows = wrapSpans([{ text: "aaa bbb ccc" }], 4)
    expect(rows.map((r) => r.map((s) => s.text).join(""))).toEqual(["aaa ", "bbb ", "ccc"])
    const long = wrapSpans([{ text: "abcdefgh" }], 3)
    expect(long.map((r) => r.map((s) => s.text).join(""))).toEqual(["abc", "def", "gh"])
  })

  test("getSyntaxTheme maps TUI theme names and built-ins", () => {
    expect(listSyntaxThemes().length).toBeGreaterThan(0)
    expect(getSyntaxTheme("monokai")?.name).toBe("Monokai")
    expect(getSyntaxTheme("no-such-theme")).toBeNull()
    // TUI theme names resolve to the nearest built-in palette.
    const dark = getSyntaxTheme("dark") as any
    expect(dark.name).toBe("Dark+")
    expect(dark.theme).toBe("Dark+")
    expect(dark.source).toBe("built-in")
    expect((getSyntaxTheme("light") as any).name).toBe("Light+")
    expect(getSyntaxTheme("dark-daltonized")).not.toBeNull()
  })

  test("StructuredDiff/HighlightedCode components import (module graph loads)", () => {
    // memo() wraps the components, so exports are objects, not functions.
    expect(typeof StructuredDiff).toMatch(/function|object/)
    expect(typeof HighlightedCode).toMatch(/function|object/)
    expect(StructuredDiff).toBeTruthy()
    expect(HighlightedCode).toBeTruthy()
  })
})
