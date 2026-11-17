// @ts-nocheck
/**
 * sleep in the Gizzi TUI: a thin adapter over the shared core
 * (runtime/primitives/sleep.ts), the same one the runtime builtin uses, with
 * the same description (sleep.txt). Replaces the old PROACTIVE/KAIROS stub.
 *
 * Wakes early on input: a prompt or bash command the user queues while it
 * waits (a subagent's sleep wakes on commands addressed to that agent). It
 * does not cancel the turn: query.ts drains the queued message into the same
 * turn after a sleep, so the model reads it and answers. Esc still cancels.
 *
 * Permission: allowed, like the runtime (loop/agent.ts `sleep: "allow"`).
 */
import { z } from 'zod/v4'
import { Sleep } from '@/runtime/primitives/sleep'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { getCommandQueue, subscribeToCommandQueue } from '../../utils/messageQueueManager.js'
import { DESCRIPTION, PROMPT, SLEEP_TOOL_NAME } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage, renderToolUseProgressMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    seconds: z.number().min(Sleep.MIN_SECONDS).max(Sleep.MAX_SECONDS).describe('How long to wait, 1 to 900 seconds'),
    reason: z.string().max(Sleep.MAX_REASON_CHARS).describe('Why you are waiting, shown to the user'),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    slept_seconds: z.number(),
    woke: z.enum(['timer', 'user_input', 'cancelled']),
    reason: z.string(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>

export type Output = z.infer<OutputSchema>
export type SleepProgress = { type: 'sleep_progress'; seconds: number; reason: string }

export const SleepTool = buildTool({
  name: SLEEP_TOOL_NAME,
  searchHint: 'wait a short time for a build or deploy, waking when the user writes',
  maxResultSizeChars: 1_000,
  userFacingName: () => 'Wait',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isReadOnly() {
    return true
  },
  isConcurrencySafe() {
    return true
  },
  toAutoClassifierInput(input) {
    return `${input.seconds}s ${input.reason}`
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  async checkPermissions(input) {
    return { behavior: 'allow', updatedInput: input }
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return {
      tool_use_id: toolUseID,
      type: 'tool_result',
      content: `Slept ${output.slept_seconds} seconds. Woke: ${output.woke}.`,
    }
  },
  renderToolUseMessage,
  renderToolUseProgressMessage,
  renderToolResultMessage,
  async call({ seconds, reason }, context, _canUseTool, _parentMessage, onProgress) {
    const agentId = context.agentId
    const isInput = cmd =>
      agentId ? cmd.agentId === agentId : cmd.agentId === undefined && (cmd.mode === 'prompt' || cmd.mode === 'bash')
    // Commands already waiting when the sleep starts don't wake it.
    const known = new Set(getCommandQueue())
    onProgress?.({ toolUseID: `sleep-${Date.now()}`, data: { type: 'sleep_progress', seconds: Sleep.clampSeconds(seconds), reason } })
    const result = await Sleep.wait({
      seconds,
      abort: context.abortController.signal,
      watchUserInput: wake => {
        if (getCommandQueue().some(cmd => !known.has(cmd) && isInput(cmd))) {
          queueMicrotask(wake)
          return () => {}
        }
        return subscribeToCommandQueue(() => {
          if (getCommandQueue().some(cmd => !known.has(cmd) && isInput(cmd))) wake()
        })
      },
    })
    return { data: { ...result, reason } }
  },
} satisfies ToolDef<InputSchema, Output>)
