/**
 * The closing line of a turn, shared by the Ink TUI and `gizzi run`:
 *
 *   Done in 42s · 3 files +48 −12 · 12 tools · claude-sonnet-5
 *       src/auth.ts  src/session.ts  test/auth.test.ts
 *
 * A failed or empty turn gets no "Done" line. It gets:
 *
 *   ✕ Stopped after 1s · nothing changed · <short cause>
 */
import chalk from "@/shared/util/chalk"
import { Highlight } from "@/shared/util/highlight"

export namespace TurnSummary {
  export type FileChange = { path: string; added: number; removed: number }

  const dim = (s: string) => chalk.dim(s)
  const SEP = dim(" · ")

  /** 4321 → "4.3s", 45000 → "45s", 125000 → "2m 5s". */
  export function seconds(ms: number): string {
    const s = Math.max(0, ms) / 1000
    if (s < 10) return `${s.toFixed(1)}s`
    if (s < 60) return `${Math.round(s)}s`
    return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`
  }

  /** Sum per-file changes; the same path edited twice counts once. */
  export function merge(changes: FileChange[]): FileChange[] {
    const byPath = new Map<string, FileChange>()
    for (const c of changes) {
      const prev = byPath.get(c.path)
      byPath.set(c.path, prev ? { path: c.path, added: prev.added + c.added, removed: prev.removed + c.removed } : { ...c })
    }
    return [...byPath.values()]
  }

  function filesSegment(files: FileChange[]): string | undefined {
    if (files.length === 0) return undefined
    const added = files.reduce((n, f) => n + f.added, 0)
    const removed = files.reduce((n, f) => n + f.removed, 0)
    return `${files.length} ${files.length === 1 ? "file" : "files"} ${Highlight.counts(added, removed)}`
  }

  /** The dimmed changed-file list for the line under "Done", or undefined. */
  export function fileList(files: FileChange[], width = 100): string | undefined {
    if (files.length === 0) return undefined
    const parts: string[] = []
    let used = 4
    for (const [i, f] of files.entries()) {
      if (used + f.path.length + 2 > width && i > 0) {
        parts.push(`+${files.length - i} more`)
        break
      }
      parts.push(f.path)
      used += f.path.length + 2
    }
    return dim(`    ${parts.join("  ")}`)
  }

  export function done(input: {
    durationMs: number
    files?: FileChange[]
    toolCount?: number
    model?: string
    /** Extra dim segments after the model (tokens, cost, context, quota). */
    extra?: string[]
  }): string {
    const segments = [`Done in ${seconds(input.durationMs)}`]
    const files = filesSegment(input.files ?? [])
    if (files) segments.push(files)
    const tail: string[] = []
    if (input.toolCount) tail.push(`${input.toolCount} ${input.toolCount === 1 ? "tool" : "tools"}`)
    if (input.model) tail.push(input.model)
    tail.push(...(input.extra ?? []).filter(Boolean))
    return segments.join(SEP) + (tail.length ? SEP + dim(tail.join(" · ")) : "")
  }

  /** First line of an error, without "Error:" noise, cut to fit one line. */
  export function shortCause(error: string | undefined, max = 80): string | undefined {
    if (!error) return undefined
    const first = error
      .split("\n")
      .map((l) => l.trim())
      .find(Boolean)
    if (!first) return undefined
    const clean = first.replace(/^(?:API\s+)?Error:\s*/i, "").replace(/^⎿\s*/, "")
    return clean.length > max ? clean.slice(0, max - 1).trimEnd() + "…" : clean
  }

  /** `width` keeps the line on one row: the cause is cut to what is left. */
  export function stopped(input: { durationMs: number; files?: FileChange[]; cause?: string; width?: number }): string {
    const files = input.files ?? []
    const changed = files.length === 0 ? "nothing changed" : `${files.length} ${files.length === 1 ? "file" : "files"} changed`
    const head = `Stopped after ${seconds(input.durationMs)}`
    const room = (input.width ?? 100) - (2 + head.length + 3 + changed.length + 3)
    const tail = [changed, shortCause(input.cause, Math.max(24, room))].filter(Boolean).join(" · ")
    return `${chalk.rgb(215, 95, 95)("✕")} ${head}${SEP}${dim(tail)}`
  }
}
