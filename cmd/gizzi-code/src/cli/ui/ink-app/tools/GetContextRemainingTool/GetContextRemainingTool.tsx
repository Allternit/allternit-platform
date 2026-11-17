// @ts-nocheck
/**
 * get_context_remaining in the Gizzi TUI (Primitives Wave B, lane L6): the
 * runtime tool id and result shape, measured the way Ink's own auto-compact
 * measures (room.ts). `compactions` counts the compact boundaries in this
 * session's transcript; a subagent counts only its own (none are written).
 */
import React from 'react'
import { z } from 'zod/v4'
import { MessageResponse } from '../../components/MessageResponse'
import { Text } from '../../ink'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { isCompactBoundaryMessage } from '../../utils/messages.js'
import type { ContextRemainingResult } from '@/runtime/primitives/wave-b-types'
import { readTranscript } from '../HistoryRecallTool/transcript.js'
import { DESCRIPTION, GET_CONTEXT_REMAINING_TOOL_NAME, PROMPT } from './prompt.js'
import { contextRoom } from './room.js'

const inputSchema = lazySchema(() => z.strictObject({}))
type InputSchema = ReturnType<typeof inputSchema>

export type Output = ContextRemainingResult

function percentLeft(output: Output): number {
  return output.window > 0 ? Math.round((output.remaining / output.window) * 100) : 0
}

export const GetContextRemainingTool = buildTool({
  name: GET_CONTEXT_REMAINING_TOOL_NAME,
  searchHint: 'how much context window is left before compaction',
  maxResultSizeChars: 10_000,
  userFacingName: () => 'Context left',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  isReadOnly() {
    return true
  },
  isConcurrencySafe() {
    return true
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: JSON.stringify(output) }
  },
  renderToolUseMessage() {
    return ''
  },
  renderToolResultMessage(output: Output) {
    return (
      <MessageResponse>
        <Text>
          {percentLeft(output)}% of context left
          <Text dimColor> · {output.until_compaction.toLocaleString('en-US')} tokens before compaction</Text>
        </Text>
      </MessageResponse>
    )
  },
  async call(_input, { messages, options, agentId }) {
    const room = contextRoom(messages, options.mainLoopModel)
    const inMemory = messages.filter(isCompactBoundaryMessage).length
    // The main thread's older boundaries live only in the transcript.
    const compactions = agentId === undefined ? Math.max(inMemory, (await readTranscript()).compactions) : inMemory
    const result: Output = {
      window: room.window,
      used: room.used,
      remaining: room.remaining,
      until_compaction: room.until_compaction,
      compactions,
    }
    return { data: result }
  },
} satisfies ToolDef<InputSchema, Output>)
