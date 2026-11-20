/**
 * resource (Primitives Wave D, D1 recall; PR3): id-based views of large tool
 * output, MCP resources and files (attachments, Files-drive), with a local
 * keyword search that returns citable line ranges. Core in
 * runtime/primitives/resource.ts. Every client; permission allow
 * (loop/agent.ts). The result text is fenced as untrusted in message-v2.
 *
 * The core bounds its own text (Resource.MAX_TEXT_BYTES), so the result sets
 * `truncated: false` and Tool.define does not spill it into another saved
 * output.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Resource } from "@/runtime/primitives/resource"
import type { ResourceExtensionMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/resource.txt"

function titleOf(params: Resource.Input): string {
  if (params.action === "search") return `search "${(params.query ?? params.pattern ?? "").slice(0, 60)}"`
  if (params.action === "find" && !params.ref) return `find refs "${(params.pattern ?? "").slice(0, 60)}"`
  return params.ref ? `${params.action} ${params.ref}` : "list refs"
}

export const ResourceTool = Tool.define("resource", {
  description: DESCRIPTION,
  parameters: Resource.Input,
  async execute(params, ctx) {
    const base = { action: params.action as string, ref: params.ref, truncated: false }
    try {
      const r = await Resource.run(ctx.sessionID, params, ctx.abort)
      return {
        title: titleOf(params),
        output: r.text ?? "",
        metadata: {
          ...base,
          error: false,
          entries: r.entries?.length,
          total_lines: r.total_lines,
          next_offset: r.next_offset,
          hits: r.hits as ResourceExtensionMetadata["hits"],
          citations: r.citations as ResourceExtensionMetadata["citations"],
          next_cursor: r.next_cursor,
        },
      }
    } catch (error) {
      if (ctx.abort?.aborted) throw error
      return {
        title: "Resource not read",
        output: error instanceof Error ? error.message : String(error),
        metadata: {
          ...base,
          error: true,
          entries: undefined as number | undefined,
          total_lines: undefined as number | undefined,
          next_offset: undefined as number | undefined,
          hits: undefined as ResourceExtensionMetadata["hits"],
          citations: undefined as ResourceExtensionMetadata["citations"],
          next_cursor: undefined as string | undefined,
        },
      }
    }
  },
})
