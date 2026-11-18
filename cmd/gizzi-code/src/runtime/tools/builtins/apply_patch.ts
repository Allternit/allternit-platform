import z from "zod/v4"
import * as path from "path"
import * as fs from "fs/promises"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Bus } from "@/shared/bus"
import { FileWatcher } from "@/shared/file/watcher"
import { Instance } from "@/runtime/context/project/instance"
import { Patch } from "@/runtime/integrations/patch"
import { createTwoFilesPatch, diffLines } from "diff"
import { assertExternalDirectory, assertSandboxWriteAllowed, FolderGrants } from "@/runtime/tools/builtins/external-directory"
import { trimDiff } from "@/runtime/tools/builtins/edit"
import { LSP } from "@/runtime/integrations/lsp"
import { Filesystem } from "@/shared/util/filesystem"
import DESCRIPTION from "@/runtime/tools/builtins/apply_patch.txt"
import { File } from "@/shared/file"

const PatchParams = z.object({
  patchText: z.string().describe("The full patch text that describes all changes to be made"),
})

export const ApplyPatchTool = Tool.define("apply_patch", {
  description: DESCRIPTION,
  parameters: PatchParams,
  async execute(params, ctx) {
    if (!params.patchText) {
      throw new Error("patchText is required")
    }

    // Parse the patch to get hunks
    let hunks: Patch.Hunk[]
    try {
      const parseResult = Patch.parsePatch(params.patchText)
      hunks = parseResult.hunks
    } catch (error) {
      throw new Error(`apply_patch verification failed: ${error}`)
    }

    if (hunks.length === 0) {
      const normalized = params.patchText.replace(/\r\n/g, "\n").replace(/\r/g, "\n").trim()
      if (normalized === "*** Begin Patch\n*** End Patch") {
        throw new Error("patch rejected: empty patch")
      }
      throw new Error("apply_patch verification failed: no hunks found")
    }

    // Validate file paths and check permissions
    const fileChanges: Array<{
      filePath: string
      oldContent: string
      newContent: string
      type: "add" | "update" | "delete" | "move"
      movePath?: string
      diff: string
      additions: number
      deletions: number
      /** A delete in a folder granted without request_delete: moved to `_to_delete/` instead. */
      trash?: FolderGrants.TrashPlan
    }> = []

    let totalDiff = ""

    for (const hunk of hunks) {
      const filePath = path.resolve(Instance.workdir, hunk.path)
      await assertExternalDirectory(ctx, filePath)
      assertSandboxWriteAllowed(ctx, filePath)

      switch (hunk.type) {
        case "add": {
          const oldContent = ""
          const newContent =
            hunk.contents.length === 0 || hunk.contents.endsWith("\n") ? hunk.contents : `${hunk.contents}\n`
          const diff = trimDiff(createTwoFilesPatch(filePath, filePath, oldContent, newContent))

          let additions = 0
          let deletions = 0
          for (const change of diffLines(oldContent, newContent)) {
            if (change.added) additions += change.count || 0
            if (change.removed) deletions += change.count || 0
          }

          fileChanges.push({
            filePath,
            oldContent,
            newContent,
            type: "add",
            diff,
            additions,
            deletions,
          })

          totalDiff += diff + "\n"
          break
        }

        case "update": {
          // Check if file exists for update
          const stats = await fs.stat(filePath).catch(() => null)
          if (!stats || stats.isDirectory()) {
            throw new Error(`apply_patch verification failed: Failed to read file to update: ${filePath}`)
          }

          const oldContent = await fs.readFile(filePath, "utf-8")
          let newContent = oldContent

          // Apply the update chunks to get new content
          try {
            const fileUpdate = Patch.deriveNewContentsFromChunks(filePath, hunk.chunks)
            newContent = fileUpdate.content
          } catch (error) {
            throw new Error(`apply_patch verification failed: ${error}`)
          }

          const diff = trimDiff(createTwoFilesPatch(filePath, filePath, oldContent, newContent))

          let additions = 0
          let deletions = 0
          for (const change of diffLines(oldContent, newContent)) {
            if (change.added) additions += change.count || 0
            if (change.removed) deletions += change.count || 0
          }

          const movePath = hunk.move_path ? path.resolve(Instance.workdir, hunk.move_path) : undefined
          await assertExternalDirectory(ctx, movePath)
          assertSandboxWriteAllowed(ctx, movePath)

          fileChanges.push({
            filePath,
            oldContent,
            newContent,
            type: hunk.move_path ? "move" : "update",
            movePath,
            diff,
            additions,
            deletions,
          })

          totalDiff += diff + "\n"
          break
        }

        case "delete": {
          const contentToDelete = await fs.readFile(filePath, "utf-8").catch((error) => {
            throw new Error(`apply_patch verification failed: ${error}`)
          })
          const deleteDiff = trimDiff(createTwoFilesPatch(filePath, filePath, contentToDelete, ""))

          const deletions = contentToDelete.split("\n").length

          // In a folder granted with folder_access but not request_delete the
          // file moves to <root>/_to_delete/ (decision 10), like bash deletes.
          // Planned here so a symlink escape refuses before anything changes.
          const disposition = FolderGrants.disposition(ctx.sessionID, filePath)
          const trash = disposition.kind === "trash" ? FolderGrants.planTrash(disposition.grant.root, filePath) : undefined

          fileChanges.push({
            filePath,
            oldContent: contentToDelete,
            newContent: "",
            type: "delete",
            diff: deleteDiff,
            additions: 0,
            deletions,
            ...(trash ? { trash } : {}),
          })

          totalDiff += deleteDiff + "\n"
          break
        }
      }
    }

    // Build per-file metadata for UI rendering (used for both permission and result)
    const files = fileChanges.map((change) => ({
      filePath: change.filePath,
      relativePath: path.relative(Instance.worktree, change.movePath ?? change.filePath),
      type: change.type,
      diff: change.diff,
      before: change.oldContent,
      after: change.newContent,
      additions: change.additions,
      deletions: change.deletions,
      movePath: change.movePath,
    }))

    // Check permissions if needed
    const relativePaths = fileChanges.map((c) => path.relative(Instance.worktree, c.filePath))
    await ctx.ask({
      permission: "edit",
      patterns: relativePaths,
      always: ["*"],
      metadata: {
        filepath: relativePaths.join(", "),
        diff: totalDiff,
        files,
      },
    })

    // Apply the changes
    const updates: Array<{ file: string; event: "add" | "change" | "unlink" }> = []
    const trashed = new Map<string, string>()

    for (const change of fileChanges) {
      const edited = change.type === "delete" ? undefined : (change.movePath ?? change.filePath)
      switch (change.type) {
        case "add":
          // Create parent directories (recursive: true is safe on existing/root dirs)
          await fs.mkdir(path.dirname(change.filePath), { recursive: true })
          await fs.writeFile(change.filePath, change.newContent, "utf-8")
          updates.push({ file: change.filePath, event: "add" })
          break

        case "update":
          await fs.writeFile(change.filePath, change.newContent, "utf-8")
          updates.push({ file: change.filePath, event: "change" })
          break

        case "move":
          if (change.movePath) {
            // Create parent directories (recursive: true is safe on existing/root dirs)
            await fs.mkdir(path.dirname(change.movePath), { recursive: true })
            await fs.writeFile(change.movePath, change.newContent, "utf-8")
            await fs.unlink(change.filePath)
            updates.push({ file: change.filePath, event: "unlink" })
            updates.push({ file: change.movePath, event: "add" })
          }
          break

        case "delete":
          if (change.trash) {
            const dest = FolderGrants.applyTrash(change.trash)
            trashed.set(change.filePath, dest)
            updates.push({ file: change.filePath, event: "unlink" })
            updates.push({ file: dest, event: "add" })
            break
          }
          await fs.unlink(change.filePath)
          updates.push({ file: change.filePath, event: "unlink" })
          break
      }

      if (edited) {
        await Bus.publish(File.Event.Edited, {
          file: edited,
          sessionID: ctx.sessionID,
        })
      }
    }

    // Publish file change events
    for (const update of updates) {
      await Bus.publish(FileWatcher.Event.Updated, update)
    }

    // Notify LSP of file changes and collect diagnostics
    for (const change of fileChanges) {
      if (change.type === "delete") continue
      const target = change.movePath ?? change.filePath
      await LSP.touchFile(target, true)
    }
    const diagnostics = await LSP.diagnostics()

    // Generate output summary
    const summaryLines = fileChanges.map((change) => {
      if (change.type === "add") {
        return `A ${path.relative(Instance.worktree, change.filePath)}`
      }
      if (change.type === "delete") {
        const dest = trashed.get(change.filePath)
        const line = `D ${path.relative(Instance.worktree, change.filePath)}`
        return dest ? `${line} (moved to ${dest}, not deleted)` : line
      }
      const target = change.movePath ?? change.filePath
      return `M ${path.relative(Instance.worktree, target)}`
    })
    let output = `Success. Updated the following files:\n${summaryLines.join("\n")}`
    if (trashed.size) {
      output += `\n\nThe folder was granted without delete permission, so deleted files moved to ${FolderGrants.TRASH_DIR}/ inside it. Tell the person where they are. To delete for real, call folder_access with action request_delete for the folder.`
    }

    // Report LSP errors for changed files
    const MAX_DIAGNOSTICS_PER_FILE = 20
    for (const change of fileChanges) {
      if (change.type === "delete") continue
      const target = change.movePath ?? change.filePath
      const normalized = Filesystem.normalizePath(target)
      const issues = diagnostics[normalized] ?? []
      const errors = issues.filter((item) => item.severity === 1)
      if (errors.length > 0) {
        const limited = errors.slice(0, MAX_DIAGNOSTICS_PER_FILE)
        const suffix =
          errors.length > MAX_DIAGNOSTICS_PER_FILE ? `\n... and ${errors.length - MAX_DIAGNOSTICS_PER_FILE} more` : ""
        output += `\n\nLSP errors detected in ${path.relative(Instance.worktree, target)}, please fix:\n<diagnostics file="${target}">\n${limited.map(LSP.Diagnostic.pretty).join("\n")}${suffix}\n</diagnostics>`
      }
    }

    return {
      title: output,
      metadata: {
        diff: totalDiff,
        files,
        diagnostics,
      },
      output,
    }
  },
})
