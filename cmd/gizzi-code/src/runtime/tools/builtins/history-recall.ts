/**
 * history_recall: read this session's compacted history and the sessions it
 * continues (Primitives Wave B). Read-only; every client; permission allow.
 * Results are labelled as data, never instructions.
 * Core: primitives/history-recall.ts (HistoryRecall.run, lane L4).
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { HistoryRecall } from "@/runtime/primitives/history-recall"
import {
  HISTORY_RECALL_ACTIONS,
  HISTORY_RECALL_LIMIT_MAX,
  HISTORY_RECALL_TOOL_ID,
  type HistoryRecallResult,
} from "@/runtime/primitives/wave-b-types"
import DESCRIPTION from "@/runtime/tools/builtins/history-recall.txt"

export const parameters = z.object({
  action: z.enum(HISTORY_RECALL_ACTIONS).describe("windows, search or read"),
  query: z.string().min(1).max(500).optional().describe("search: words to find"),
  window_id: z.string().min(1).max(200).optional().describe("read: which window (from windows or search); search: only this window"),
  item_id: z.string().min(1).max(200).optional().describe("read: one message in the window (from search)"),
  limit: z.number().int().min(1).max(HISTORY_RECALL_LIMIT_MAX).optional().describe(`windows, search: at most ${HISTORY_RECALL_LIMIT_MAX} results`),
})

export type HistoryRecallParams = z.infer<typeof parameters>

/** A model-readable error for input the schema can't express, or undefined when the input is fine. */
export function historyRecallInputError(params: HistoryRecallParams): string | undefined {
  if (params.action === "search" && !params.query) return "history_recall search needs a query."
  if (params.action === "read" && !params.window_id)
    return 'history_recall read needs a window_id. Use action "windows" or "search" to find one.'
  return undefined
}

export function historyRecallTitle(result: HistoryRecallResult): string {
  if (result.action === "windows") return `${result.windows.length} earlier window${result.windows.length === 1 ? "" : "s"}`
  if (result.action === "search") return `${result.hits.length} match${result.hits.length === 1 ? "" : "es"} in earlier history`
  return result.truncated ? "Read earlier history (shortened)" : "Read earlier history"
}

export const HistoryRecallTool = Tool.define<typeof parameters, HistoryRecallResult>(HISTORY_RECALL_TOOL_ID, {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    const error = historyRecallInputError(params)
    if (error) throw new Error(error)
    await ctx.ask({
      permission: HISTORY_RECALL_TOOL_ID,
      patterns: ["*"],
      always: ["*"],
      metadata: { action: params.action, query: params.query },
    })
    const result = await HistoryRecall.run({ sessionID: ctx.sessionID, ...params })
    return { title: historyRecallTitle(result), output: JSON.stringify(result), metadata: result }
  },
})
