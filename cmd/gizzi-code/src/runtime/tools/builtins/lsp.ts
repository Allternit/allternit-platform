import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import path from "path"
import { LSP } from "@/runtime/integrations/lsp"
import DESCRIPTION from "@/runtime/tools/builtins/lsp.txt"
import { Instance } from "@/runtime/context/project/instance"
import { pathToFileURL } from "url"
import { assertExternalDirectory } from "@/runtime/tools/builtins/external-directory"
import { Filesystem } from "@/shared/util/filesystem"
import { WorkspaceEdit } from "@/runtime/tools/builtins/workspace-edit"

const operations = [
  "goToDefinition",
  "findReferences",
  "hover",
  "documentSymbol",
  "workspaceSymbol",
  "goToImplementation",
  "prepareCallHierarchy",
  "incomingCalls",
  "outgoingCalls",
  "rename",
] as const

const NO_SERVER_MESSAGE =
  "No language server is available for this file type, so it cannot be renamed semantically. Use the edit tool (or multiedit/apply_patch) to change each occurrence instead, after finding them with grep."

/** The identifier under a 0-based position, for the rename summary. */
export function identifierAt(content: string, line: number, character: number): string | undefined {
  const text = content.split("\n")[line]?.replace(/\r$/, "")
  if (text === undefined) return undefined
  const word = /[\p{L}\p{N}_$]/u
  let start = Math.min(character, text.length)
  let end = start
  while (start > 0 && word.test(text[start - 1])) start--
  while (end < text.length && word.test(text[end])) end++
  return end > start ? text.slice(start, end) : undefined
}

/**
 * Apply one rename's WorkspaceEdit through the shared edit path (permission
 * prompt, FileTime locks, file events, LSP diagnostics). Exported for tests.
 */
export async function applyRename(
  ctx: Tool.Context,
  input: { file: string; line: number; character: number; newName: string; result: LSP.RenameResult },
) {
  const { files, skipped } = WorkspaceEdit.collect(input.result.edit ?? {})
  const fileContent = await Filesystem.readText(input.file).catch(() => "")
  const oldName =
    input.result.placeholder ??
    (input.result.range
      ? fileContent.slice(
          WorkspaceEdit.offsetAt(fileContent, input.result.range.start),
          WorkspaceEdit.offsetAt(fileContent, input.result.range.end),
        )
      : identifierAt(fileContent, input.line, input.character)) ??
    "symbol"

  // A rename that touches files outside the project is refused whole: applying
  // only the inside part would leave the code half-renamed.
  const outside = [...files.keys()].filter((filePath) => !Instance.containsPath(filePath))
  if (outside.length) {
    return {
      title: `rename ${oldName} -> ${input.newName}`,
      metadata: { result: [], outside } as Record<string, any>,
      output: `Rename refused: the language server wants to edit files outside the project, so nothing was changed:\n${outside.map((x) => `- ${x}`).join("\n")}`,
    }
  }

  const changes: WorkspaceEdit.FileChange[] = []
  for (const [filePath, edits] of files) {
    const before = await Filesystem.readText(filePath)
    const after = WorkspaceEdit.applyTextEdits(before, edits)
    if (after === before) continue
    changes.push({ filePath, before, after, edits: edits.length })
  }

  if (changes.length === 0) {
    return {
      title: `rename ${oldName} -> ${input.newName}`,
      metadata: { result: [], skipped } as Record<string, any>,
      output:
        `The language server returned no text changes for renaming \`${oldName}\` to \`${input.newName}\`.` +
        (skipped.length ? `\nNot applied (file operations are not supported):\n${skipped.map((x) => `- ${x}`).join("\n")}` : ""),
    }
  }

  const applied = await WorkspaceEdit.apply(ctx, changes)
  const total = changes.reduce((sum, change) => sum + (change.edits ?? 0), 0)
  const lines = changes.map(
    (change) => `- ${path.relative(Instance.worktree, change.filePath)} (${change.edits} edit${change.edits === 1 ? "" : "s"})`,
  )
  let output = `Renamed \`${oldName}\` to \`${input.newName}\`: ${total} edit${total === 1 ? "" : "s"} in ${changes.length} file${changes.length === 1 ? "" : "s"}\n${lines.join("\n")}`
  if (skipped.length) {
    output += `\n\nNot applied (file create/rename/delete operations are not supported; do them by hand if needed):\n${skipped.map((x) => `- ${x}`).join("\n")}`
  }
  output += applied.diagnosticsOutput

  return {
    title: `rename ${oldName} -> ${input.newName} (${changes.length} file${changes.length === 1 ? "" : "s"})`,
    metadata: {
      result: [],
      diff: applied.diff,
      files: applied.files,
      diagnostics: applied.diagnostics,
      skipped,
    } as Record<string, any>,
    output,
  }
}

export const LspTool = Tool.define("lsp", {
  description: DESCRIPTION,
  parameters: z.object({
    operation: z.enum(operations).describe("The LSP operation to perform"),
    filePath: z.string().describe("The absolute or relative path to the file"),
    line: z.number().int().min(1).describe("The line number (1-based, as shown in editors)"),
    character: z.number().int().min(1).describe("The character offset (1-based, as shown in editors)"),
    query: z
      .string()
      .optional()
      .describe("Search string for workspaceSymbol (ignored by all other operations)"),
    newName: z
      .string()
      .optional()
      .describe("The new name for the symbol at the position (required for rename, ignored by all other operations)"),
  }),
  execute: async (args, ctx) => {
    const file = path.isAbsolute(args.filePath) ? args.filePath : path.join(Instance.workdir, args.filePath)
    await assertExternalDirectory(ctx, file)

    await ctx.ask({
      permission: "lsp",
      patterns: ["*"],
      always: ["*"],
      metadata: {},
    })
    const uri = pathToFileURL(file).href
    const position = {
      file,
      line: args.line - 1,
      character: args.character - 1,
    }

    const relPath = path.relative(Instance.worktree, file)
    const title = `${args.operation} ${relPath}:${args.line}:${args.character}`

    const exists = await Filesystem.exists(file)
    if (!exists) {
      throw new Error(`File not found: ${file}`)
    }

    const available = await LSP.hasClients(file)
    if (!available) {
      if (args.operation === "rename") {
        return { title, metadata: { result: [] as unknown[] } as Record<string, any>, output: NO_SERVER_MESSAGE }
      }
      throw new Error("No LSP server available for this file type.")
    }

    await LSP.touchFile(file, true)

    if (args.operation === "rename") {
      const newName = args.newName?.trim()
      if (!newName) throw new Error("rename requires newName: the new name for the symbol at the position.")
      const results = await LSP.rename({ ...position, newName })
      const success = results.find((item) => item.edit)
      if (!success) {
        const reason = results.find((item) => item.error)?.error
        return {
          title,
          metadata: { result: results as unknown[] } as Record<string, any>,
          output: results.length
            ? `Could not rename: ${reason ?? "the language server returned no edits."} Use the edit tool to change the occurrences instead.`
            : NO_SERVER_MESSAGE,
        }
      }
      return applyRename(ctx, { file, line: position.line, character: position.character, newName, result: success })
    }

    const result: unknown[] = await (async () => {
      switch (args.operation) {
        case "goToDefinition":
          return LSP.definition(position)
        case "findReferences":
          return LSP.references(position)
        case "hover":
          return LSP.hover(position)
        case "documentSymbol":
          return LSP.documentSymbol(uri)
        case "workspaceSymbol":
          return LSP.workspaceSymbol(args.query ?? "")
        case "goToImplementation":
          return LSP.implementation(position)
        case "prepareCallHierarchy":
          return LSP.prepareCallHierarchy(position)
        case "incomingCalls":
          return LSP.incomingCalls(position)
        case "outgoingCalls":
          return LSP.outgoingCalls(position)
        default:
          return []
      }
    })()

    const output = (() => {
      if (result.length === 0) return `No results found for ${args.operation}`
      return JSON.stringify(result, null, 2)
    })()

    return {
      title,
      metadata: { result } as Record<string, any>,
      output,
    }
  },
})
