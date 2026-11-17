/**
 * get_context_remaining: how much of the context window is left
 * (Primitives Wave B). Read-only; every client; permission allow.
 * Core: primitives/context-remaining.ts (ContextRemaining.read, lane L4).
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { ContextRemaining } from "@/runtime/primitives/context-remaining"
import { CONTEXT_REMAINING_TOOL_ID, type ContextRemainingResult } from "@/runtime/primitives/wave-b-types"
import DESCRIPTION from "@/runtime/tools/builtins/context-remaining.txt"

export const parameters = z.object({})

export function contextRemainingTitle(result: ContextRemainingResult): string {
  const percent = result.window > 0 ? Math.round((result.remaining / result.window) * 100) : 0
  return `${percent}% of context left`
}

export const GetContextRemainingTool = Tool.define<typeof parameters, ContextRemainingResult>(CONTEXT_REMAINING_TOOL_ID, {
  description: DESCRIPTION,
  parameters,
  async execute(_params, ctx) {
    await ctx.ask({
      permission: CONTEXT_REMAINING_TOOL_ID,
      patterns: ["*"],
      always: ["*"],
      metadata: {},
    })
    const result = await ContextRemaining.read({ sessionID: ctx.sessionID, model: ctx.extra?.model })
    return { title: contextRemainingTitle(result), output: JSON.stringify(result), metadata: result }
  },
})
