/**
 * resource (Primitives Wave D, D1 recall): id-based views of large tool output
 * and MCP resources. Core in runtime/primitives/resource.ts. Every client;
 * permission allow (loop/agent.ts).
 *
 * W1 stub: final id and parameters; lane R fills the execute body.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Resource } from "@/runtime/primitives/resource"
import DESCRIPTION from "@/runtime/tools/builtins/resource.txt"

export const ResourceTool = Tool.define("resource", {
  description: DESCRIPTION,
  parameters: Resource.Input,
  async execute(params, ctx) {
    try {
      const r = await Resource.run(ctx.sessionID, params, ctx.abort)
      return {
        title: params.ref ? `${params.action} ${params.ref}` : params.action,
        output: r.text ?? JSON.stringify(r.entries ?? []),
        metadata: { action: params.action as string, ref: params.ref },
      }
    } catch (error) {
      return {
        title: "Resource unavailable",
        output: error instanceof Error ? error.message : String(error),
        metadata: { action: params.action as string, ref: params.ref },
      }
    }
  },
})
