/**
 * tool_search: load deferred tools for the next step (Primitives Wave B).
 * Replaces select_tools and covers built-in families and MCP tools alike.
 * Every client; permission allow. Always loaded itself.
 *
 * Core: primitives/tool-families.ts (ToolFamilies.search, lane L3). L3 may
 * swap the static description for one that lists the deferred tool names.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { ToolFamilies } from "@/runtime/primitives/tool-families"
import { TOOL_SEARCH_TOOL_ID, type ToolSearchResult } from "@/runtime/primitives/wave-b-types"
import DESCRIPTION from "@/runtime/tools/builtins/tool-search.txt"

export const parameters = z.object({
  query: z
    .string()
    .min(1)
    .max(500)
    .describe('Keywords for what you need (such as "schedule reminder"), or "select:name1,name2" for exact tool names'),
})

export const ToolSearchTool = Tool.define<typeof parameters, ToolSearchResult>(TOOL_SEARCH_TOOL_ID, {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    await ctx.ask({
      permission: TOOL_SEARCH_TOOL_ID,
      patterns: ["*"],
      always: ["*"],
      metadata: { query: params.query },
    })
    const result = await ToolFamilies.search({ sessionID: ctx.sessionID, messages: ctx.messages, query: params.query })
    const title = result.loaded.length
      ? `Loaded ${result.loaded.length} tool${result.loaded.length === 1 ? "" : "s"}`
      : "No new tools loaded"
    return { title, output: JSON.stringify(result), metadata: result }
  },
})
