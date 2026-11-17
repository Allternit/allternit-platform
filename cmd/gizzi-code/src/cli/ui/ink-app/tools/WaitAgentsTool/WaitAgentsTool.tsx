// @ts-nocheck
/**
 * wait_agents in the Gizzi TUI (Primitives Wave B, lane L6): the same tool
 * id, parameters and result shape as the runtime builtin, over Ink's own
 * background agents (local_agent tasks in AppState.tasks).
 *
 * Wakes when a watched agent settles (completed, failed, stopped or
 * interrupted), when the user queues a new message (steer), or at the
 * timeout. AppState has no change hook a tool can reach, so agent states are
 * checked every POLL_MS; the command queue has a signal and wakes it at once.
 */
import React from 'react'
import { z } from 'zod/v4'
import { MessageResponse } from '../../components/MessageResponse'
import { Text } from '../../ink'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  getCommandQueue,
  subscribeToCommandQueue,
} from '../../utils/messageQueueManager.js'
import {
  WAIT_AGENTS_TIMEOUT_MS,
  type SettledAgent,
  type WaitAgentsResult,
} from '@/runtime/primitives/wave-b-types'
import {
  agentRef,
  agentStatus,
  agentSummary,
  agentTasks,
  isAgentRunning,
  resolveAgent,
} from '../AgentControlTool/agents.js'
import { DESCRIPTION, PROMPT, WAIT_AGENTS_TOOL_NAME } from './prompt.js'

const POLL_MS = 500

const inputSchema = lazySchema(() =>
  z.strictObject({
    targets: z
      .array(z.string().min(1).max(200))
      .max(50)
      .optional()
      .describe('Agent ids or name paths to wait on. Leave out to wait on every running agent.'),
    timeout_ms: z
      .number()
      .int()
      .min(WAIT_AGENTS_TIMEOUT_MS.min)
      .max(WAIT_AGENTS_TIMEOUT_MS.max)
      .optional()
      .describe(`How long to wait, in milliseconds (default ${WAIT_AGENTS_TIMEOUT_MS.default})`),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

export type Output = WaitAgentsResult

/** A message the user typed (not a task notification, not a system prompt), for the main thread. */
function isUserMessage(cmd): boolean {
  return cmd.mode === 'prompt' && !cmd.origin && !cmd.isMeta && cmd.agentId === undefined
}

function title(result: Output): string {
  if (result.woke === 'settled')
    return `${result.settled.length} agent${result.settled.length === 1 ? '' : 's'} finished`
  if (result.woke === 'steer') return 'New message from the user'
  return 'Still running'
}

export const WaitAgentsTool = buildTool({
  name: WAIT_AGENTS_TOOL_NAME,
  searchHint: 'wait for background agents to finish instead of polling',
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Wait for agents',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  isReadOnly() {
    return true
  },
  isConcurrencySafe() {
    return true
  },
  toAutoClassifierInput(input) {
    return (input.targets ?? []).join(' ')
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
  renderToolUseMessage(input) {
    return input.targets?.length ? input.targets.join(', ') : 'all running agents'
  },
  renderToolResultMessage(output: Output) {
    return (
      <MessageResponse>
        <Text>
          {title(output)}
          {output.running.length ? <Text dimColor> · {output.running.length} still running</Text> : null}
        </Text>
      </MessageResponse>
    )
  },
  async call({ targets, timeout_ms }, { getAppState, abortController, agentId }) {
    const timeout = timeout_ms ?? WAIT_AGENTS_TIMEOUT_MS.default
    const start = getAppState()
    const explicit = Boolean(targets?.length)
    const ids = explicit
      ? [...new Set(targets!.map(t => resolveAgent(start, t)))]
      : agentTasks(start).filter(isAgentRunning).map(t => t.id)

    const lastSeen = new Map(ids.map(id => [id, start.tasks[id]]))
    const settledOf = (id: string): SettledAgent => {
      const task = lastSeen.get(id)
      return {
        id,
        name: agentRef(getAppState(), id).name,
        status: task ? agentStatus(task) : 'cancelled',
        summary: task ? agentSummary(task) : 'No longer tracked.',
      }
    }
    const runningRefs = (list: string[]) => list.map(id => agentRef(getAppState(), id))

    const watched = ids.filter(id => isAgentRunning(start.tasks[id]))
    if (watched.length === 0) {
      const result: Output = { woke: 'settled', settled: explicit ? ids.map(settledOf) : [], running: [] }
      return { data: result }
    }

    // Steer only reaches the main thread: subagents don't get the user's typing.
    const steerable = agentId === undefined
    const queuedAtStart = new Set(getCommandQueue().filter(isUserMessage))

    return await new Promise<{ data: Output }>((resolve, reject) => {
      let done = false
      let timer: ReturnType<typeof setTimeout> | undefined
      let poll: ReturnType<typeof setInterval> | undefined
      let unsubscribe: (() => void) | undefined

      const finish = (result: Output | Error) => {
        if (done) return
        done = true
        clearTimeout(timer)
        clearInterval(poll)
        unsubscribe?.()
        abortController.signal.removeEventListener('abort', onAbort)
        if (result instanceof Error) reject(result)
        else resolve({ data: result })
      }

      const check = () => {
        const state = getAppState()
        const settled: string[] = []
        const running: string[] = []
        for (const id of watched) {
          const task = state.tasks[id]
          if (task) lastSeen.set(id, task)
          if (task && isAgentRunning(task)) running.push(id)
          else settled.push(id)
        }
        if (settled.length) finish({ woke: 'settled', settled: settled.map(settledOf), running: runningRefs(running) })
        return running
      }

      const checkSteer = () => {
        if (!steerable) return
        const fresh = getCommandQueue().some(cmd => isUserMessage(cmd) && !queuedAtStart.has(cmd))
        if (fresh) finish({ woke: 'steer', settled: [], running: runningRefs(check()) })
      }

      const onAbort = () =>
        finish(abortController.signal.reason instanceof Error ? abortController.signal.reason : new Error('Aborted'))

      if (abortController.signal.aborted) return onAbort()
      abortController.signal.addEventListener('abort', onAbort, { once: true })
      unsubscribe = subscribeToCommandQueue(checkSteer)
      poll = setInterval(() => {
        check()
        checkSteer()
      }, POLL_MS)
      timer = setTimeout(() => finish({ woke: 'timeout', settled: [], running: runningRefs(watched) }), timeout)
      check()
    })
  },
} satisfies ToolDef<InputSchema, Output>)
