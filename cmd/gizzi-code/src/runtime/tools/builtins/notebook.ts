import z from "zod/v4"
import * as path from "path"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Bus } from "@/shared/bus"
import { File } from "@/shared/file"
import { FileWatcher } from "@/shared/file/watcher"
import { FileTime } from "@/shared/file/time"
import { Filesystem } from "@/shared/util/filesystem"
import { Instance } from "@/runtime/context/project/instance"
import { assertExternalDirectory } from "@/runtime/tools/builtins/external-directory"

interface NotebookCell {
  cell_type: string
  source: string[] | string
  metadata: Record<string, unknown>
  outputs?: unknown[]
  execution_count?: number | null
}

interface Notebook {
  cells: NotebookCell[]
  metadata: Record<string, unknown>
  nbformat: number
  nbformat_minor: number
}

function makeCell(cellType: string, source: string): NotebookCell {
  const lines = source.split("\n").map((line, i, arr) => (i < arr.length - 1 ? line + "\n" : line))
  const cell: NotebookCell = {
    cell_type: cellType,
    source: lines,
    metadata: {},
  }
  if (cellType === "code") {
    cell.outputs = []
    cell.execution_count = null
  }
  return cell
}

function cellSourceString(cell: NotebookCell): string {
  if (Array.isArray(cell.source)) return cell.source.join("")
  return cell.source
}

const DESCRIPTION = `Edits Jupyter notebook (.ipynb) files by inserting, replacing, or deleting cells.

Operations:
- "insert": Insert a new cell at the given cellIndex. Requires cellType and source.
- "replace": Replace the source (and optionally cellType) of an existing cell. Requires source.
- "delete": Delete the cell at the given cellIndex.

The cellIndex is 0-based. For insert, the new cell is placed before the existing cell at that index (use cells.length to append).`

export const NotebookEditTool = Tool.define("notebook_edit", {
  description: DESCRIPTION,
  parameters: z.object({
    filePath: z.string().describe("The absolute path to the .ipynb file"),
    operation: z.enum(["insert", "replace", "delete"]).describe("The operation to perform"),
    cellIndex: z.number().describe("The 0-based cell index"),
    cellType: z
      .enum(["code", "markdown"])
      .optional()
      .describe("The cell type (required for insert, optional for replace)"),
    source: z.string().optional().describe("The new cell content (required for insert and replace)"),
  }),
  async execute(params, ctx) {
    if (!params.filePath) {
      throw new Error("filePath is required")
    }

    const filePath = path.isAbsolute(params.filePath) ? params.filePath : path.join(Instance.workdir, params.filePath)
    await assertExternalDirectory(ctx, filePath)

    if (!filePath.endsWith(".ipynb")) {
      throw new Error(`File must be a .ipynb notebook: ${filePath}`)
    }

    const exists = await Filesystem.exists(filePath)
    if (!exists) {
      throw new Error(`File not found: ${filePath}`)
    }

    await FileTime.assert(ctx.sessionID, filePath)

    const raw = await Filesystem.readText(filePath)
    let notebook: Notebook
    try {
      notebook = JSON.parse(raw)
    } catch {
      throw new Error(`Failed to parse notebook JSON: ${filePath}`)
    }

    if (!Array.isArray(notebook.cells)) {
      throw new Error(`Invalid notebook: missing cells array in ${filePath}`)
    }

    const totalCells = notebook.cells.length
    let summary = ""

    switch (params.operation) {
      case "insert": {
        if (params.source === undefined) {
          throw new Error("source is required for insert operation")
        }
        if (!params.cellType) {
          throw new Error("cellType is required for insert operation")
        }
        if (params.cellIndex < 0 || params.cellIndex > totalCells) {
          throw new Error(`cellIndex ${params.cellIndex} out of range [0, ${totalCells}]`)
        }
        const cell = makeCell(params.cellType, params.source)
        notebook.cells.splice(params.cellIndex, 0, cell)
        summary = `Inserted ${params.cellType} cell at index ${params.cellIndex}. Notebook now has ${notebook.cells.length} cells.`
        break
      }

      case "replace": {
        if (params.source === undefined) {
          throw new Error("source is required for replace operation")
        }
        if (params.cellIndex < 0 || params.cellIndex >= totalCells) {
          throw new Error(`cellIndex ${params.cellIndex} out of range [0, ${totalCells - 1}]`)
        }
        const existing = notebook.cells[params.cellIndex]
        const oldSource = cellSourceString(existing)
        if (params.cellType) {
          existing.cell_type = params.cellType
          if (params.cellType === "code" && existing.outputs === undefined) {
            existing.outputs = []
            existing.execution_count = null
          }
          if (params.cellType === "markdown") {
            delete existing.outputs
            delete existing.execution_count
          }
        }
        const lines = params.source.split("\n").map((line, i, arr) => (i < arr.length - 1 ? line + "\n" : line))
        existing.source = lines
        summary = `Replaced ${existing.cell_type} cell at index ${params.cellIndex}. Old source length: ${oldSource.length}, new source length: ${params.source.length}.`
        break
      }

      case "delete": {
        if (params.cellIndex < 0 || params.cellIndex >= totalCells) {
          throw new Error(`cellIndex ${params.cellIndex} out of range [0, ${totalCells - 1}]`)
        }
        const removed = notebook.cells[params.cellIndex]
        notebook.cells.splice(params.cellIndex, 1)
        summary = `Deleted ${removed.cell_type} cell at index ${params.cellIndex}. Notebook now has ${notebook.cells.length} cells.`
        break
      }
    }

    const newContent = JSON.stringify(notebook, null, 1) + "\n"

    await ctx.ask({
      permission: "edit",
      patterns: [path.relative(Instance.worktree, filePath)],
      always: ["*"],
      metadata: {
        filepath: filePath,
        diff: summary,
      },
    })

    await Filesystem.write(filePath, newContent)
    await Bus.publish(File.Event.Edited, {
      file: filePath,
    })
    await Bus.publish(FileWatcher.Event.Updated, {
      file: filePath,
      event: "change",
    })
    FileTime.read(ctx.sessionID, filePath)

    return {
      title: `${path.relative(Instance.worktree, filePath)}`,
      metadata: {},
      output: summary,
    }
  },
})
