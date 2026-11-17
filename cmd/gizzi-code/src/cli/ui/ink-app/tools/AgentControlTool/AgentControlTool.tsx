// @ts-nocheck
/**
 * agent_control in the Gizzi TUI (Primitives Wave B, lane L6): the runtime
 * tool id and result shapes over Ink's own background agents.
 *
 * - list: AppState.tasks local_agent rows (flat: Ink agents can't start agents).
 * - interrupt: kills the agent's turn (killAsyncAgent); its transcript stays
 *   on disk, and it reports as interrupted, not failed.
 * - resume / followup: a running agent gets the message queued for its next
 *   step (queuePendingMessage); a stopped or finished one is restarted from
 *   its transcript in the background (resumeAgentBackground). Ink ships no
 *   SendMessage tool, so this is the terminal's follow-up path.
 * - close: not an action here; TaskStop ends an agent for good.
 */
import React from 'react'
import { z } from 'zod/v4'
import { MessageResponse } from '../../components/MessageResponse'
import { Text } from '../../ink'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import {
  appendMessageToLocalAgent,
  killAsyncAgent,
  queuePendingMessage,
} from '../../tasks/LocalAgentTask/LocalAgentTask.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { createUserMessage } from '../../utils/messages.js'
import type { AgentControlResult, AgentNode } from '@/runtime/primitives/wave-b-types'
import {
  agentActivity,
  agentRef,
  agentStatus,
  agentTasks,
  clearInterrupted,
  isAgentRunning,
  markInterrupted,
  resolveAgent,
} from './agents.js'
import { AGENT_CONTROL_TOOL_NAME, DESCRIPTION, PROMPT } from './prompt.js'

const ACTIONS = ['list', 'interrupt', 'resume', 'followup'] as const
const RESUME_MESSAGE = 'Continue the task from where you stopped.'

const inputSchema = lazySchema(() =>
  z.strictObject({
    action: z.enum(ACTIONS).describe('list, interrupt, resume or followup'),
    target: z.string().min(1).max(200).optional().describe('An agent id or name path; every action but list needs it'),
    message: z.string().min(1).max(20_000).optional().describe('followup: the message to send'),
    path_prefix: z.string().min(1).max(200).optional().describe('list: only agents whose name path starts with this'),
    reason: z.string().max(500).optional().describe('interrupt: why, shown to the user'),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

export type Output = AgentControlResult

function title(output: Output): string {
  switch (output.action) {
    case 'list':
      return `${output.agents.length} agent${output.agents.length === 1 ? '' : 's'}`
    case 'interrupt':
      return output.outcome === 'interrupted' ? `Interrupted ${output.target.path ?? output.target.id}` : 'Was not running'
    case 'resume':
      return output.outcome === 'resumed' ? `Resumed ${output.target.path ?? output.target.id}` : 'Already running'
    case 'followup':
      return output.outcome === 'steered'
        ? `Sent to ${output.target.path ?? output.target.id}`
        : `Started ${output.target.path ?? output.target.id} again`
    default:
      return ''
  }
}

export const AgentControlTool = buildTool({
  name: AGENT_CONTROL_TOOL_NAME,
  searchHint: 'list, interrupt, resume or message background agents',
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Agent control',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  isReadOnly(input) {
    return input?.action === 'list'
  },
  isConcurrencySafe(input) {
    return input?.action === 'list'
  },
  toAutoClassifierInput(input) {
    return [input.action, input.target, input.message].filter(Boolean).join(' ')
  },
  async validateInput(input) {
    if (input.action !== 'list' && !input.target)
      return { result: false, message: `agent_control ${input.action} needs a target.`, errorCode: 1 }
    if (input.action === 'followup' && !input.message?.trim())
      return { result: false, message: 'agent_control followup needs a message.', errorCode: 1 }
    return { result: true }
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
    return [input.action, input.target].filter(Boolean).join(' ')
  },
  renderToolResultMessage(output: Output) {
    return (
      <MessageResponse>
        <Text>{title(output)}</Text>
      </MessageResponse>
    )
  },
  async call(input, toolUseContext, canUseTool) {
    const { getAppState } = toolUseContext
    // In-process teammates get a no-op setAppState; this reaches the root store.
    const setAppState = toolUseContext.setAppStateForTasks ?? toolUseContext.setAppState
    const state = getAppState()

    if (input.action === 'list') {
      const parent = getSessionId()
      const agents: (AgentNode & { activity?: string })[] = agentTasks(state)
        .map(task => {
          const activity = isAgentRunning(task) ? agentActivity(task) : undefined
          return {
            ...agentRef(state, task.id),
            status: agentStatus(task),
            agent: task.agentType ?? null,
            description: task.description,
            depth: 1,
            parent_id: parent,
            ...(activity ? { activity } : {}),
          }
        })
        .filter(node => !input.path_prefix || (node.path ?? '').startsWith(input.path_prefix))
      return { data: { action: 'list', agents } }
    }

    const id = resolveAgent(state, input.target!)
    const target = agentRef(state, id)
    const task = state.tasks[id]

    if (input.action === 'interrupt') {
      if (!isAgentRunning(task)) return { data: { action: 'interrupt', target, outcome: 'not_running' } }
      markInterrupted(id)
      killAsyncAgent(id, setAppState)
      return { data: { action: 'interrupt', target, outcome: 'interrupted' } }
    }

    const message = input.action === 'resume' ? RESUME_MESSAGE : input.message!.trim()
    if (isAgentRunning(task)) {
      if (input.action === 'resume') return { data: { action: 'resume', target, outcome: 'already_running' } }
      queuePendingMessage(id, message, setAppState)
      appendMessageToLocalAgent(id, createUserMessage({ content: message }), setAppState)
      return { data: { action: 'followup', target, outcome: 'steered' } }
    }

    // Loaded on use: resumeAgent imports tools.ts, which imports this tool.
    const { resumeAgentBackground } = await import('../AgentTool/resumeAgent.js')
    await resumeAgentBackground({ agentId: id, prompt: message, toolUseContext, canUseTool })
    clearInterrupted(id)
    return input.action === 'resume'
      ? { data: { action: 'resume', target, outcome: 'resumed' } }
      : { data: { action: 'followup', target, outcome: 'started' } }
  },
} satisfies ToolDef<InputSchema, Output>)
