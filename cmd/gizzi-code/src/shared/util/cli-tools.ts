/**
 * Tool calls that a CLI brain (claude-cli, codex-cli, ACP CLIs) ran itself,
 * turned into what Gizzi needs to show them: the real shell command, and for
 * edits the hunks, +/− counts and the result shape the TUI's Edit/Write
 * components render (diff with line numbers). Used by the TUI's cliBrain
 * path and by the `gizzi run` printer.
 */
import { readFileSync } from "fs"
import { parsePatch, structuredPatch } from "diff"

export type Hunk = { oldStart: number; oldLines: number; newStart: number; newLines: number; lines: string[] }

/**
 * Codex runs every command as `/bin/zsh -lc '<cmd>'` (or bash -lc); show the
 * command, not the wrapper.
 */
export function unwrapShell(command: unknown): string {
  const text = Array.isArray(command) ? command.map(String).join(" ") : String(command ?? "")
  const m = text.match(/^(?:\/(?:usr\/)?bin\/)?(?:ba|z)?sh\s+-l?c\s+(['"])([\s\S]*)\1$/)
  if (!m) return text
  const inner = m[2]!
  return m[1] === "'" ? inner.replace(/'\\''/g, "'") : inner.replace(/\\(["\\$`])/g, "$1")
}

export function hunksFromUnified(diff: string): Hunk[] {
  if (!diff.trim()) return []
  // Codex sends bare hunks ("@@ -1 +1 @@ …") without ---/+++ headers.
  const text = /^(---|diff |Index:)/m.test(diff) ? diff : `--- a\n+++ b\n${diff}`
  try {
    return parsePatch(text).flatMap((p) => p.hunks as Hunk[])
  } catch {
    return []
  }
}

/** Hunks for an old/new string edit, placed at the real line numbers of the file as it is now. */
export function hunksFromStrings(filePath: string, oldString: string, newString: string, now?: string): Hunk[] {
  const patch = structuredPatch(filePath, filePath, oldString, newString, "", "", { context: 0 })
  let offset = 0
  const text = now ?? readText(filePath)
  const at = newString && text ? text.indexOf(newString) : -1
  if (at >= 0) offset = text!.slice(0, at).split("\n").length - 1
  return (patch.hunks as Hunk[]).map((h) => ({ ...h, oldStart: h.oldStart + offset, newStart: h.newStart + offset }))
}

export function countHunks(hunks: Hunk[]): { added: number; removed: number } {
  let added = 0
  let removed = 0
  for (const h of hunks)
    for (const l of h.lines) {
      if (l.startsWith("+")) added++
      else if (l.startsWith("-")) removed++
    }
  return { added, removed }
}

function readText(filePath: string): string | undefined {
  try {
    return readFileSync(filePath, "utf8")
  } catch {
    return undefined
  }
}

export type PatchFile = { path: string; kind: "add" | "delete" | "update"; hunks: Hunk[]; content?: string; added: number; removed: number }

/**
 * Files in a Codex fileChange / apply_patch call. Accepts the app-server
 * array (`[{path, kind, diff}]`) and the older map (`{path: {update|add|delete}}`).
 */
export function patchFiles(changes: unknown): PatchFile[] {
  const entries: Array<[string, Record<string, unknown>]> = Array.isArray(changes)
    ? (changes as Array<Record<string, unknown>>).filter((c) => typeof c?.path === "string").map((c) => [c.path as string, c])
    : changes && typeof changes === "object"
      ? Object.entries(changes as Record<string, Record<string, unknown>>)
      : []
  return entries.map(([path, change]) => {
    const rawKind = change?.kind && typeof change.kind === "object" ? (change.kind as Record<string, unknown>).type : (change?.kind ?? change?.type)
    const nested = (change?.update ?? change?.add ?? change?.delete) as Record<string, unknown> | undefined
    const kind = (rawKind ?? (change?.add ? "add" : change?.delete ? "delete" : "update")) as PatchFile["kind"]
    const diff = [change?.diff, change?.unified_diff, nested?.unified_diff].find((d): d is string => typeof d === "string")
    const content = [change?.content, nested?.content].find((c): c is string => typeof c === "string") ?? (kind === "add" ? diff : undefined)
    if (kind === "add") {
      const lines = (content ?? "").replace(/\n$/, "").split("\n")
      const hunks: Hunk[] = content ? [{ oldStart: 0, oldLines: 0, newStart: 1, newLines: lines.length, lines: lines.map((l) => "+" + l) }] : []
      return { path, kind, hunks, content, added: content ? lines.length : 0, removed: 0 }
    }
    const hunks = diff ? hunksFromUnified(diff) : []
    return { path, kind: kind === "delete" ? "delete" : "update", hunks, ...countHunks(hunks) }
  })
}

/**
 * The result object the TUI's Edit / Write components render, for an edit a
 * CLI brain made itself (no Gizzi tool ran, so there is no toolUseResult).
 * Undefined for anything that isn't an edit.
 */
export function observedEditResult(name: string, input: Record<string, unknown>): Record<string, unknown> | undefined {
  const filePath = typeof input.file_path === "string" ? input.file_path : undefined
  if (!filePath) return undefined
  const now = readText(filePath)
  if (name === "Edit" || name === "MultiEdit") {
    const edits: Array<{ old: string; new: string }> = Array.isArray(input.edits)
      ? (input.edits as Array<Record<string, unknown>>).map((e) => ({ old: String(e.old_string ?? ""), new: String(e.new_string ?? "") }))
      : [{ old: String(input.old_string ?? ""), new: String(input.new_string ?? "") }]
    if (edits.every((e) => !e.old && !e.new)) return undefined
    let original = now ?? ""
    for (const e of edits) if (e.new && original.includes(e.new)) original = original.replace(e.new, e.old)
    return {
      filePath,
      oldString: edits[0]!.old,
      newString: edits[0]!.new,
      originalFile: original,
      structuredPatch: edits.flatMap((e) => hunksFromStrings(filePath, e.old, e.new, now)),
      userModified: false,
      replaceAll: Boolean(input.replace_all),
    }
  }
  if (name === "Write") {
    const content = typeof input.content === "string" ? input.content : (now ?? "")
    return { type: "create", filePath, content, structuredPatch: [], originalFile: null }
  }
  return undefined
}
