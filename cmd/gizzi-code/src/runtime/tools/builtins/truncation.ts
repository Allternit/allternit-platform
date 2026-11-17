import fs from "fs/promises"
import path from "path"
import { Global } from "@/runtime/context/global"
import { Identifier } from "@/shared/id/id"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import type { Agent } from "@/runtime/loop/agent"
import { Scheduler } from "@/runtime/automation/scheduler"
import { Filesystem } from "@/shared/util/filesystem"
import { Glob } from "@/shared/util/glob"
import { Log } from "@/shared/util/log"

const log = Log.create({ service: "truncation" })

export namespace Truncate {
  export const MAX_LINES = 2000
  export const MAX_BYTES = 50 * 1024
  export const DIR = path.join(Global.Path.data, "tool-output")
  export const GLOB = path.join(DIR, "*")
  const RETENTION_MS = 7 * 24 * 60 * 60 * 1000 // 7 days
  const HOUR_MS = 60 * 60 * 1000

  /**
   * A saved output's id (`tool_<ulid>`). The `resource` tool resolves
   * `out:<id>` to DIR/<id> only when the id matches this and the session's
   * own tool parts name it (runtime/primitives/resource.ts).
   */
  export const ID = /^tool_[0-9A-Za-z]{8,64}$/

  /** The session-scoped ref the truncation notice names: `out:<id>`. */
  export function outRef(id: string): string {
    return `out:${id}`
  }

  export type Result = { content: string; truncated: false } | { content: string; truncated: true; outputPath: string }

  export type ModelContentItem = {
    type: string
    text?: string
    resource?: { blob?: unknown }
    [key: string]: unknown
  }

  export interface Options {
    maxLines?: number
    maxBytes?: number
    direction?: "head" | "tail"
  }

  /**
   * Applies a spill result to provider-native structured tool content. Some
   * adapters prefer this array over the normalized string output, so leaving
   * its text untouched would silently bypass truncation. Media is retained;
   * textual resource content is available in the spill file.
   */
  export function modelContent(content: ModelContentItem[], result: Result): ModelContentItem[] {
    if (!result.truncated) return content
    return [
      { type: "text", text: result.content },
      ...content.filter(
        (item) => item.type === "image" || (item.type === "resource" && item.resource?.blob !== undefined),
      ),
    ]
  }

  export function init() {
    Scheduler.register({
      id: "tool.truncation.cleanup",
      interval: HOUR_MS,
      run: cleanup,
      scope: "global",
    })
  }

  export async function cleanup() {
    const cutoff = Identifier.timestamp(Identifier.create("tool", false, Date.now() - RETENTION_MS))
    const entries = await Glob.scan("tool_*", { cwd: DIR, include: "file" }).catch(() => [] as string[])
    for (const entry of entries) {
      if (Identifier.timestamp(entry) >= cutoff) continue
      await fs.unlink(path.join(DIR, entry)).catch((e) => log.debug("Failed to clean up tool temp file", { entry, error: e }))
    }
  }

  function hasTaskTool(agent?: Agent.Info): boolean {
    if (!agent?.permission) return false
    const rule = PermissionNext.evaluate("task", "*", agent.permission)
    return rule.action !== "deny"
  }

  export async function output(text: string, options: Options = {}, agent?: Agent.Info): Promise<Result> {
    const maxLines = options.maxLines ?? MAX_LINES
    const maxBytes = options.maxBytes ?? MAX_BYTES
    const direction = options.direction ?? "head"
    const lines = text.split("\n")
    const totalBytes = Buffer.byteLength(text, "utf-8")

    if (lines.length <= maxLines && totalBytes <= maxBytes) {
      return { content: text, truncated: false }
    }

    const out: string[] = []
    let i = 0
    let bytes = 0
    let hitBytes = false

    if (direction === "head") {
      for (i = 0; i < lines.length && i < maxLines; i++) {
        const size = Buffer.byteLength(lines[i], "utf-8") + (i > 0 ? 1 : 0)
        if (bytes + size > maxBytes) {
          hitBytes = true
          break
        }
        out.push(lines[i])
        bytes += size
      }
    } else {
      for (i = lines.length - 1; i >= 0 && out.length < maxLines; i--) {
        const size = Buffer.byteLength(lines[i], "utf-8") + (out.length > 0 ? 1 : 0)
        if (bytes + size > maxBytes) {
          hitBytes = true
          break
        }
        out.unshift(lines[i])
        bytes += size
      }
    }

    const removed = hitBytes ? totalBytes - bytes : lines.length - out.length
    const unit = hitBytes ? "bytes" : "lines"
    const preview = out.join("\n")

    const id = Identifier.ascending("tool")
    const filepath = path.join(DIR, id)
    await Filesystem.write(filepath, text)

    const ref = outRef(id)
    const hint = hasTaskTool(agent)
      ? `The tool call succeeded but the output was truncated. Full output saved as ref ${ref} (file: ${filepath}).\nUse the Task tool to have an explore agent process it with the resource tool (find with a pattern, read with offset/limit) or Grep and Read on the file. Do NOT read the full output yourself - delegate to save context.`
      : `The tool call succeeded but the output was truncated. Full output saved as ref ${ref} (file: ${filepath}).\nUse the resource tool on ${ref} (find with a pattern, read with offset/limit), or Grep/Read with offset/limit on the file.`
    const message =
      direction === "head"
        ? `${preview}\n\n...${removed} ${unit} truncated...\n\n${hint}`
        : `...${removed} ${unit} truncated...\n\n${hint}\n\n${preview}`

    return { content: message, truncated: true, outputPath: filepath }
  }
}
