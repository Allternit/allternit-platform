/**
 * resource (Primitives Wave D, D1 recall): id-based views of large tool output
 * and MCP resources. Core in runtime/primitives/resource.ts. Every client;
 * permission allow (loop/agent.ts).
 *
 * The core bounds its own text (Resource.MAX_TEXT_BYTES), so the result sets
 * `truncated: false` and Tool.define does not spill it into another saved
 * output.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Resource } from "@/runtime/primitives/resource"
import DESCRIPTION from "@/runtime/tools/builtins/resource.txt"

export const ResourceTool = Tool.define("resource", {
  description: DESCRIPTION,
  parameters: Resource.Input,
  async execute(params, ctx) {
    const base = { action: params.action as string, ref: params.ref, truncated: false }
    try {
      const r = await Resource.run(ctx.sessionID, params, ctx.abort)
      return {
        title: params.ref ? `${params.action} ${params.ref}` : "list refs",
        output: r.text ?? "",
        metadata: {
          ...base,
          error: false,
          entries: r.entries?.length,
          total_lines: r.total_lines,
          next_offset: r.next_offset,
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
        },
      }
    }
  },
})
