import { describe, expect, test } from "bun:test"
import chalk, { nearestAnsi256 } from "../../src/shared/util/chalk"
import { dotMode } from "../../src/shared/util/dot-progress"
import { DOT_FPS, ToolPrinter, dotBandFrame, dotProgressText, fitMiddle, progressDetailOf, shouldAnimateDots, normalizeTool, shortPath } from "../../src/cli/commands/run-print"
import { Highlight } from "../../src/shared/util/highlight"
import { TurnSummary } from "../../src/shared/util/turn-summary"
import { patchFiles, unwrapShell } from "../../src/shared/util/cli-tools"

const plain = (s: string) => s.replace(/\u001b\[[0-9;]*m/g, "")

function capture(fn: (p: ToolPrinter) => void): string[] {
  const lines: string[] = []
  const level = chalk.level
  chalk.level = 0
  try {
    fn(new ToolPrinter((l) => lines.push(l)))
  } finally {
    chalk.level = level
  }
  return lines
}

describe("gizzi run tool lines", () => {
  // Regression: claude-cli tools printed "Read(Read)" / "Edit(false)", each twice.
  test("a CLI brain's Read and Edit print one header each with the real path", () => {
    const cwd = process.cwd()
    const lines = capture((p) => {
      const read = { id: "r", tool: "Read", state: { status: "running" as const, input: { file_path: `${cwd}/src/a.ts` } } }
      p.update(read)
      p.update({ ...read, state: { ...read.state, status: "completed", output: "x", title: "Read" } })
      const edit = { id: "e", tool: "Edit", state: { status: "running" as const, input: { file_path: `${cwd}/nope.ts`, old_string: "a", new_string: "b", replace_all: false } } }
      p.update(edit)
      p.update({ ...edit, state: { ...edit.state, status: "completed", output: "ok", title: "Edit" } })
    })
    const headers = lines.filter((l) => l.startsWith("⏺"))
    expect(headers).toEqual(["⏺ Read src/a.ts", "⏺ Edit nope.ts  +1 −1"])
    expect(lines.join("\n")).not.toContain("false")
  })

  test("names and argument keys from CLI brains map to the engine's", () => {
    expect(normalizeTool("MultiEdit", { file_path: "/a" })).toMatchObject({ name: "edit", input: { filePath: "/a" } })
    expect(normalizeTool("exec_command", { command: ["bun", "test"] })).toMatchObject({ name: "bash", input: { command: "bun test" } })
  })

  // Codex tools: shell calls arrive wrapped in zsh -lc, edits as fileChange patches.
  test("codex shell wrapper is removed and a patch prints as an Edit with counts", () => {
    expect(unwrapShell("/bin/zsh -lc 'cat calc.ts'")).toBe("cat calc.ts")
    expect(unwrapShell(`/bin/zsh -lc "echo \\"hi\\""`)).toBe('echo "hi"')
    const changes = [{ path: "calc.ts", kind: { type: "update" }, diff: "@@ -6 +6 @@\n-  return a + b\n+  return a - b\n" }]
    expect(patchFiles(changes)[0]).toMatchObject({ path: "calc.ts", kind: "update", added: 1, removed: 1 })
    const lines = capture((p) =>
      p.update({ id: "p", tool: "patch_apply", state: { status: "completed", input: { changes }, output: "" } }),
    )
    expect(lines.filter((l) => l.startsWith("⏺"))).toEqual(["⏺ Edit calc.ts  +1 −1"])
  })

  test("paths are cwd-relative and cut in the middle only when too long", () => {
    expect(shortPath(`${process.cwd()}/src/x.ts`)).toBe("src/x.ts")
    expect(fitMiddle("short.ts", 20)).toBe("short.ts")
    const cut = fitMiddle("a/very/long/path/to/some/deeply/nested/file.ts", 20)
    expect(cut).toHaveLength(20)
    expect(cut.endsWith("file.ts")).toBe(true)
  })
})

describe("shared highlighter", () => {
  test("pass, fail, paths and counts", () => {
    const level = chalk.level
    chalk.level = 2
    try {
      expect(Highlight.line("(fail) sub")).toContain("38;5;167")
      expect(Highlight.line(" 1 pass")).toContain("38;5;114")
      expect(Highlight.line(" 0 fail")).toBe(" 0 fail")
      expect(Highlight.line("see src/a.ts:3")).toContain("1;38;5;74msrc/a.ts")
      expect(Highlight.line("https://x.dev/a/b.ts")).toBe("https://x.dev/a/b.ts")
    } finally {
      chalk.level = level
    }
  })

  test("NO_COLOR level leaves text alone", () => {
    const level = chalk.level
    chalk.level = 0
    try {
      expect(Highlight.text("✓ ok src/a.ts +1 -2")).toBe("✓ ok src/a.ts +1 -2")
    } finally {
      chalk.level = level
    }
  })
})

describe("256-color diff backgrounds", () => {
  // Regression: Apple Terminal showed added lines in grey 237 (the prompt
  // background) and removed lines in plum 89.
  test("the dark theme's diff tints keep their hue", () => {
    expect(nearestAnsi256(34, 92, 43)).toBe(22)
    expect(nearestAnsi256(122, 41, 54)).toBe(88)
    expect(nearestAnsi256(55, 55, 55)).toBe(237)
  })
})

describe("closing line", () => {
  test("done lists files and counts; stopped never says Done", () => {
    const done = plain(TurnSummary.done({ durationMs: 42000, files: [{ path: "a.ts", added: 48, removed: 12 }], toolCount: 12, model: "m" }))
    expect(done).toBe("Done in 42s · 1 file +48 −12 · 12 tools · m")
    const stopped = plain(TurnSummary.stopped({ durationMs: 1000, cause: "API Error: Not logged in\nmore" }))
    expect(stopped).toBe("✕ Stopped after 1.0s · nothing changed · Not logged in")
  })
})

describe("braille dot progress", () => {
  const braille = /^[\u2800-\u28FF]+$/
  test("frame is pure, the requested width, and all braille", () => {
    for (const w of [1, 8, 20]) expect(dotBandFrame(w, 1.3)).toHaveLength(w)
    expect(dotBandFrame(16, 2.5)).toBe(dotBandFrame(16, 2.5))
    expect(dotBandFrame(16, 2.5)).toMatch(braille)
  })
  test("the band moves between frames and loops", () => {
    expect(dotBandFrame(16, 0.2)).not.toBe(dotBandFrame(16, 0.5))
    expect(dotBandFrame(16, 0)).toBe(dotBandFrame(16, (16 + 6) / 14))
  })
  test("a real fraction fills the left and the band sweeps only the rest", () => {
    const f = dotBandFrame(10, 0.4, 0.5)
    expect(f.slice(0, 5)).toBe("\u28FF".repeat(5))
    expect(dotBandFrame(10, 0.4, 1)).toBe("\u28FF".repeat(10))
  })
  test("no time draws the still frame", () => {
    expect(dotBandFrame(6, undefined)).toBe("\u2802".repeat(6))
  })
  test("text shows percent only for a real fraction", () => {
    expect(dotProgressText({ label: "Drawing frame 12 of 180" }, 14.9)).toBe("Drawing frame 12 of 180 \u00B7 14s")
    expect(dotProgressText({ label: "Encoding", fraction: 0.4 }, 3)).toBe("Encoding \u00B7 3s \u00B7 40%")
    expect(progressDetailOf({ progress: "Queued" })).toEqual({ label: "Queued" })
  })
  test("static under NO_COLOR, non-TTY, reduced motion", () => {
    expect(shouldAnimateDots({}, true)).toBe(true)
    expect(shouldAnimateDots({ NO_COLOR: "1" }, true)).toBe(false)
    expect(shouldAnimateDots({}, false)).toBe(false)
    expect(shouldAnimateDots({ GIZZI_REDUCED_MOTION: "1" }, true)).toBe(false)
  })
  test("printer: animated line is erased and the result row replaces it", async () => {
    const lines: string[] = []
    const raw: string[] = []
    const p = new ToolPrinter((l) => lines.push(l), { animate: true, sink: { write: (s) => raw.push(s) } })
    const part = { id: "m", tool: "media_generate", state: { status: "running" as const, input: { prompt: "a fox" }, metadata: { progress_detail: { label: "Drawing", fraction: 0.5 } } } }
    p.update(part)
    await new Promise((r) => setTimeout(r, 1000 / DOT_FPS + 60))
    expect(raw.some((s) => /[\u2800-\u28FF]/.test(s) && s.includes("50%"))).toBe(true)
    p.update({ ...part, state: { ...part.state, status: "completed", output: "saved fox.png", metadata: {} } })
    expect(raw[raw.length - 1]).toBe("\r\u001b[2K")
    const n = raw.length
    await new Promise((r) => setTimeout(r, 200))
    expect(raw.length).toBe(n)
    expect(plain(lines.join("\n"))).toContain("saved fox.png")
  })
  test("printer: static mode prints a line per stage, no timer", () => {
    const lines: string[] = []
    const p = new ToolPrinter((l) => lines.push(l), { animate: false })
    const part = { id: "m", tool: "media_generate", state: { status: "running" as const, input: { prompt: "x" }, metadata: { progress_detail: { label: "Queued" } } } }
    p.update(part)
    p.update(part)
    expect(lines.filter((l) => plain(l).includes("Queued"))).toHaveLength(1)
  })
  test("Ink app: still when reduced motion, NO_COLOR or not a TTY; animate otherwise", () => {
    expect(dotMode(false, {}, true)).toBe("animate")
    expect(dotMode(true, {}, true)).toBe("still")
    expect(dotMode(false, { NO_COLOR: "1" }, true)).toBe("still")
    expect(dotMode(false, {}, false)).toBe("still")
    expect(dotProgressText({ label: "Queued" })).toBe("Queued")
  })
})
