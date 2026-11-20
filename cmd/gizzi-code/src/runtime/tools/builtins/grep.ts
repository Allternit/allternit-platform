import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Filesystem } from "@/shared/util/filesystem"
import { Ripgrep } from "@/shared/file/ripgrep"

import DESCRIPTION from "@/runtime/tools/builtins/grep.txt"
import { Instance } from "@/runtime/context/project/instance"
import path from "path"
import { assertExternalDirectory } from "@/runtime/tools/builtins/external-directory"

const MAX_LINE_LENGTH = 2000

export const GrepTool = Tool.define("grep", {
  description: DESCRIPTION,
  parameters: z.object({
    pattern: z.string().describe("The regex pattern to search for in file contents"),
    path: z.string().optional().describe("The directory to search in. Defaults to the current working directory."),
    include: z.string().optional().describe('File pattern to include in the search (e.g. "*.js", "*.{ts,tsx}")'),
    output_mode: z
      .enum(["content", "files_with_matches", "count"])
      .optional()
      .describe(
        'What to return. "content" (default) lists matching lines with line numbers. "files_with_matches" lists only file paths. "count" gives the number of matching lines per file.',
      ),
  }),
  async execute(params, ctx) {
    if (!params.pattern) {
      throw new Error("pattern is required")
    }

    await ctx.ask({
      permission: "grep",
      patterns: [params.pattern],
      always: ["*"],
      metadata: {
        pattern: params.pattern,
        path: params.path,
        include: params.include,
        output_mode: params.output_mode,
      },
    })

    let searchPath = params.path ?? Instance.workdir
    searchPath = path.isAbsolute(searchPath) ? searchPath : path.resolve(Instance.workdir, searchPath)
    await assertExternalDirectory(ctx, searchPath, { kind: "directory" })

    const rgPath = await Ripgrep.filepath()
    const args = ["-nH", "--hidden", "--no-messages", "--field-match-separator=|", "--regexp", params.pattern]
    if (params.include) {
      args.push("--glob", params.include)
    }
    args.push(searchPath)

    const proc = Bun.spawn([rgPath, ...args], {
      stdout: "pipe",
      stderr: "pipe",
      signal: ctx.abort,
    })

    const output = await new Response(proc.stdout).text()
    const errorOutput = await new Response(proc.stderr).text()
    const exitCode = await proc.exited

    // Exit codes: 0 = matches found, 1 = no matches, 2 = errors (but may still have matches)
    // With --no-messages, we suppress error output but still get exit code 2 for broken symlinks etc.
    // Only fail if exit code is 2 AND no output was produced
    if (exitCode === 1 || (exitCode === 2 && !output.trim())) {
      return {
        title: params.pattern,
        metadata: { matches: 0, truncated: false },
        output: "No files found",
      }
    }

    if (exitCode !== 0 && exitCode !== 2) {
      throw new Error(`ripgrep failed: ${errorOutput}`)
    }

    const hasErrors = exitCode === 2

    // Handle both Unix (\n) and Windows (\r\n) line endings
    const lines = output.trim().split(/\r?\n/)
    const matches = []

    for (const line of lines) {
      if (!line) continue

      const [filePath, lineNumStr, ...lineTextParts] = line.split("|")
      if (!filePath || !lineNumStr || lineTextParts.length === 0) continue

      const lineNum = parseInt(lineNumStr, 10)
      const lineText = lineTextParts.join("|")

      const stats = Filesystem.stat(filePath)
      if (!stats) continue

      matches.push({
        path: filePath,
        modTime: stats.mtime.getTime(),
        lineNum,
        lineText,
      })
    }

    matches.sort((a, b) => b.modTime - a.modTime)

    const mode = params.output_mode ?? "content"
    if (mode !== "content" && matches.length > 0) {
      const counts = new Map<string, number>()
      for (const match of matches) counts.set(match.path, (counts.get(match.path) ?? 0) + 1)
      const files = [...counts.entries()]
      const body =
        mode === "count" ? files.map(([file, n]) => `${file}: ${n}`) : files.map(([file]) => file)
      const header =
        mode === "count"
          ? `Found ${matches.length} matches in ${files.length} files`
          : `Found ${files.length} files`
      const outLines = [header, ...body]
      if (hasErrors) outLines.push("", "(Some paths were inaccessible and skipped)")
      return {
        title: params.pattern,
        metadata: { matches: matches.length, truncated: false },
        output: outLines.join("\n"),
      }
    }

    const limit = 100
    const truncated = matches.length > limit
    const finalMatches = truncated ? matches.slice(0, limit) : matches

    if (finalMatches.length === 0) {
      return {
        title: params.pattern,
        metadata: { matches: 0, truncated: false },
        output: "No files found",
      }
    }

    const totalMatches = matches.length
    const outputLines = [`Found ${totalMatches} matches${truncated ? ` (showing first ${limit})` : ""}`]

    let currentFile = ""
    for (const match of finalMatches) {
      if (currentFile !== match.path) {
        if (currentFile !== "") {
          outputLines.push("")
        }
        currentFile = match.path
        outputLines.push(`${match.path}:`)
      }
      const truncatedLineText =
        match.lineText.length > MAX_LINE_LENGTH ? match.lineText.substring(0, MAX_LINE_LENGTH) + "..." : match.lineText
      outputLines.push(`  Line ${match.lineNum}: ${truncatedLineText}`)
    }

    if (truncated) {
      outputLines.push("")
      outputLines.push(
        `(Results truncated: showing ${limit} of ${totalMatches} matches (${totalMatches - limit} hidden). Consider using a more specific path or pattern.)`,
      )
    }

    if (hasErrors) {
      outputLines.push("")
      outputLines.push("(Some paths were inaccessible and skipped)")
    }

    return {
      title: params.pattern,
      metadata: {
        matches: totalMatches,
        truncated,
      },
      output: outputLines.join("\n"),
    }
  },
})
