// Applying computed edits (LSP WorkspaceEdits from `lsp` rename, symbol range
// replacements from `symbol_edit`) through the same path the edit tools use:
// external-directory + sandbox checks, one `edit` permission prompt listing
// every file, FileTime locks, File.Event.Edited + FileWatcher events, then
// LSP touchFile + diagnostics. Undo comes from the session's step snapshots
// (SessionProcessor tracks the worktree with Snapshot.track/patch around every
// step), exactly as for edit/apply_patch, so `/rewind` restores these writes.

import fs from "fs"
import * as path from "path"
import { fileURLToPath } from "url"
import { createTwoFilesPatch, diffLines } from "diff"
import type { Tool } from "@/runtime/tools/builtins/tool"
import { Bus } from "@/shared/bus"
import { File } from "@/shared/file"
import { FileWatcher } from "@/shared/file/watcher"
import { FileTime } from "@/shared/file/time"
import { Filesystem } from "@/shared/util/filesystem"
import { Instance } from "@/runtime/context/project/instance"
import { LSP } from "@/runtime/integrations/lsp"
import type { Snapshot } from "@/runtime/session/snapshot"
import { assertExternalDirectory, assertSandboxWriteAllowed } from "@/runtime/tools/builtins/external-directory"
import { trimDiff } from "@/runtime/tools/builtins/edit"

const MAX_DIAGNOSTICS_PER_FILE = 20


/**
 * Project-relative path for display and permission patterns. Language
 * servers report real paths, so a project opened through a symlink (macOS
 * /var → /private/var, a linked checkout) is compared against its real path too.
 */
function fromWorktree(file: string): string {
  const root = Instance.worktree
  if (file === root || file.startsWith(root + path.sep)) return path.relative(root, file)
  const real = (p: string) => {
    try {
      return fs.realpathSync(p)
    } catch {
      return p
    }
  }
  const realRoot = real(root)
  const realFile = real(file)
  if (realFile === realRoot || realFile.startsWith(realRoot + path.sep)) return path.relative(realRoot, realFile)
  return path.relative(root, file)
}

export namespace WorkspaceEdit {
  export type Position = { line: number; character: number }
  export type Range = { start: Position; end: Position }
  export type TextEdit = { range: Range; newText: string }

  /** LSP WorkspaceEdit — `changes` and/or `documentChanges`. */
  export type Edit = {
    changes?: Record<string, Array<TextEdit | { range: Range; snippet?: { value: string } }>>
    documentChanges?: Array<Record<string, any>>
  }

  export type Collected = {
    /** Absolute file path → its text edits, in the order the server sent them. */
    files: Map<string, TextEdit[]>
    /** Resource operations (create/rename/delete) that were not applied. */
    skipped: string[]
  }

  function uriToPath(uri: string) {
    return uri.startsWith("file:") ? fileURLToPath(uri) : uri
  }

  function textOf(edit: any): string {
    if (typeof edit?.newText === "string") return edit.newText
    if (typeof edit?.snippet?.value === "string") return edit.snippet.value
    return ""
  }

  /**
   * Flatten a WorkspaceEdit into per-file text edits. Per the LSP spec,
   * `documentChanges` wins over `changes` when both are present. Resource
   * operations are reported in `skipped`, never applied.
   */
  export function collect(edit: Edit): Collected {
    const files = new Map<string, TextEdit[]>()
    const skipped: string[] = []
    const add = (uri: string, edits: any[]) => {
      const file = uriToPath(uri)
      const list = files.get(file) ?? []
      for (const item of edits ?? []) {
        if (!item?.range) continue
        list.push({ range: item.range, newText: textOf(item) })
      }
      if (list.length) files.set(file, list)
    }

    if (Array.isArray(edit.documentChanges) && edit.documentChanges.length) {
      for (const change of edit.documentChanges) {
        if (typeof change?.kind === "string") {
          if (change.kind === "rename") skipped.push(`rename ${uriToPath(change.oldUri)} -> ${uriToPath(change.newUri)}`)
          else skipped.push(`${change.kind} ${uriToPath(change.uri)}`)
          continue
        }
        if (change?.textDocument?.uri) add(change.textDocument.uri, change.edits)
      }
      return { files, skipped }
    }

    for (const [uri, edits] of Object.entries(edit.changes ?? {})) add(uri, edits as any[])
    return { files, skipped }
  }

  /** Offset of an LSP position (UTF-16 columns, same unit as JS strings). Clamps past-the-end values. */
  export function offsetAt(content: string, position: Position): number {
    let offset = 0
    for (let line = 0; line < position.line; line++) {
      const next = content.indexOf("\n", offset)
      if (next === -1) return content.length
      offset = next + 1
    }
    let lineEnd = content.indexOf("\n", offset)
    if (lineEnd === -1) lineEnd = content.length
    if (lineEnd > offset && content[lineEnd - 1] === "\r") lineEnd--
    return Math.min(offset + Math.max(0, position.character), lineEnd)
  }

  /**
   * Apply text edits to a document. Edits are applied bottom-up (by start
   * offset, descending) so earlier offsets stay valid; edits at the same
   * position keep the server's order in the result. Overlapping edits throw.
   */
  export function applyTextEdits(content: string, edits: TextEdit[]): string {
    const resolved = edits
      .map((edit, index) => ({
        start: offsetAt(content, edit.range.start),
        end: offsetAt(content, edit.range.end),
        text: edit.newText,
        index,
      }))
      .sort((a, b) => b.start - a.start || b.end - a.end || b.index - a.index)

    let result = content
    let floor = Infinity
    for (const edit of resolved) {
      if (edit.end > floor) throw new Error("Overlapping edits in the workspace edit; nothing was applied.")
      result = result.slice(0, edit.start) + edit.text + result.slice(edit.end)
      floor = edit.start
    }
    return result
  }

  export type FileChange = {
    filePath: string
    before: string
    after: string
    /** Number of edits that produced `after` (for the summary). */
    edits?: number
  }

  export type Applied = {
    diff: string
    files: Array<{
      filePath: string
      relativePath: string
      type: "update"
      diff: string
      before: string
      after: string
      additions: number
      deletions: number
    }>
    filediff: Snapshot.FileDiff[]
    diagnostics: Awaited<ReturnType<typeof LSP.diagnostics>>
    /** `<diagnostics>` blocks for files with new errors, or "". */
    diagnosticsOutput: string
  }

  function normalizeLineEndings(text: string) {
    return text.replaceAll("\r\n", "\n")
  }

  /**
   * Write `changes` to disk the way edit/apply_patch do. Every path must be
   * inside the project (assertExternalDirectory) and the sandbox. Asks the
   * `edit` permission once with every file. With `assertRead`, each file must
   * have been read this session and be unchanged since (FileTime.assert, as
   * edit.ts requires); in every case a file that changed after `before` was
   * read is refused rather than overwritten.
   */
  export async function apply(
    ctx: Tool.Context,
    changes: FileChange[],
    options: { assertRead?: boolean } = {},
  ): Promise<Applied> {
    for (const change of changes) {
      await assertExternalDirectory(ctx, change.filePath)
      assertSandboxWriteAllowed(ctx, change.filePath)
    }

    const files: Applied["files"] = []
    const filediff: Snapshot.FileDiff[] = []
    let diff = ""
    for (const change of changes) {
      const fileDiff = trimDiff(
        createTwoFilesPatch(
          change.filePath,
          change.filePath,
          normalizeLineEndings(change.before),
          normalizeLineEndings(change.after),
        ),
      )
      let additions = 0
      let deletions = 0
      for (const part of diffLines(change.before, change.after)) {
        if (part.added) additions += part.count || 0
        if (part.removed) deletions += part.count || 0
      }
      files.push({
        filePath: change.filePath,
        relativePath: fromWorktree(change.filePath),
        type: "update",
        diff: fileDiff,
        before: change.before,
        after: change.after,
        additions,
        deletions,
      })
      filediff.push({ file: change.filePath, before: change.before, after: change.after, additions, deletions })
      diff += fileDiff + "\n"
    }

    const relativePaths = files.map((file) => file.relativePath)
    await ctx.ask({
      permission: "edit",
      patterns: relativePaths,
      always: ["*"],
      metadata: {
        filepath: relativePaths.join(", "),
        diff,
        files,
      },
    })

    for (const change of changes) {
      await FileTime.withLock(change.filePath, async () => {
        if (options.assertRead) await FileTime.assert(ctx.sessionID, change.filePath)
        const current = await Filesystem.readText(change.filePath)
        if (current !== change.before) {
          throw new Error(
            `File ${change.filePath} changed while the edit was being prepared. Nothing more was written; run the tool again.`,
          )
        }
        await Filesystem.write(change.filePath, change.after)
        await Bus.publish(File.Event.Edited, { file: change.filePath, sessionID: ctx.sessionID })
        await Bus.publish(FileWatcher.Event.Updated, { file: change.filePath, event: "change" })
        FileTime.read(ctx.sessionID, change.filePath)
      })
    }

    for (const change of changes) await LSP.touchFile(change.filePath, true)
    const diagnostics = await LSP.diagnostics()

    let diagnosticsOutput = ""
    for (const change of changes) {
      const issues = diagnostics[Filesystem.normalizePath(change.filePath)] ?? []
      const errors = issues.filter((item) => item.severity === 1)
      if (errors.length === 0) continue
      const limited = errors.slice(0, MAX_DIAGNOSTICS_PER_FILE)
      const suffix =
        errors.length > MAX_DIAGNOSTICS_PER_FILE ? `\n... and ${errors.length - MAX_DIAGNOSTICS_PER_FILE} more` : ""
      diagnosticsOutput += `\n\nLSP errors detected in ${fromWorktree(change.filePath)}, please fix:\n<diagnostics file="${change.filePath}">\n${limited.map(LSP.Diagnostic.pretty).join("\n")}${suffix}\n</diagnostics>`
    }

    return { diff, files, filediff, diagnostics, diagnosticsOutput }
  }
}
