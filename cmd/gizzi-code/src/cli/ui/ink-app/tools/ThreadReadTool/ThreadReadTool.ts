/**
 * thread_read / thread_recent / thread_wait in the Gizzi TUI: thin adapters
 * over the shared past-chats core (runtime/primitives/past-chats.ts), the same
 * one the runtime builtins use. Output is labelled as records of an earlier
 * chat, never instructions (PastChats.RECORD_NOTICE).
 */
import { z } from 'zod/v4'
import { PastChats } from '@/runtime/primitives/past-chats'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  THREAD_READ_PROMPT,
  THREAD_READ_TOOL_NAME,
  THREAD_RECENT_PROMPT,
  THREAD_RECENT_TOOL_NAME,
  THREAD_WAIT_PROMPT,
  THREAD_WAIT_TOOL_NAME,
} from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const outputSchema = lazySchema(() =>
  z.object({
    summary: z.string(),
    text: z.string(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** The notice, then the result as JSON. */
function asRecord(result: unknown): string {
  return `${PastChats.RECORD_NOTICE}\n\n${JSON.stringify(result)}`
}

function mapResult(output: Output, toolUseID: string) {
  return { tool_use_id: toolUseID, type: 'tool_result' as const, content: output.text }
}

const readSchema = lazySchema(() => PastChats.ReadInput)
const recentSchema = lazySchema(() => PastChats.RecentInput)
const waitSchema = lazySchema(() => PastChats.WaitInput)

export const ThreadReadTool = buildTool({
  name: THREAD_READ_TOOL_NAME,
  searchHint: "read a window of one of the user's earlier chats",
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Read chat',
  get inputSchema() {
    return readSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  toAutoClassifierInput: input => input.thread ?? '',
  async description() {
    return THREAD_READ_PROMPT
  },
  async prompt() {
    return THREAD_READ_PROMPT
  },
  mapToolResultToToolResultBlockParam: mapResult,
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    try {
      const r = await PastChats.read(getSessionId(), input, context.abortController?.signal)
      if (r.status === 'not_found') {
        return {
          data: {
            summary: 'Chat not found',
            text: `No chat with id ${input.thread} that you can read (it may not exist, belong to someone else, or be incognito).`,
          },
        }
      }
      const name = r.thread.title || 'chat'
      const summary = r.status === 'remote' ? `${name}: summary only (on another computer)` : `Read ${name}`
      return { data: { summary, text: asRecord(r) } }
    } catch (error) {
      return { data: { summary: 'Chat not read', text: messageOf(error) } }
    }
  },
} satisfies ToolDef<any, Output>)

export const ThreadRecentTool = buildTool({
  name: THREAD_RECENT_TOOL_NAME,
  searchHint: "list the user's most recent chats",
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Recent chats',
  get inputSchema() {
    return recentSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  toAutoClassifierInput: input => input.project ?? '',
  async description() {
    return THREAD_RECENT_PROMPT
  },
  async prompt() {
    return THREAD_RECENT_PROMPT
  },
  mapToolResultToToolResultBlockParam: mapResult,
  renderToolUseMessage: () => '',
  renderToolResultMessage,
  async call(input, context) {
    try {
      const r = await PastChats.recent(getSessionId(), input)
      const n = r.threads.length
      return { data: { summary: `${n} recent chat${n === 1 ? '' : 's'}`, text: asRecord(r) } }
    } catch (error) {
      return { data: { summary: 'Recent chats unavailable', text: messageOf(error) } }
    }
  },
} satisfies ToolDef<any, Output>)

export const ThreadWaitTool = buildTool({
  name: THREAD_WAIT_TOOL_NAME,
  searchHint: 'wait for a chat to finish a turn or need the user',
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Wait for chat',
  get inputSchema() {
    return waitSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  toAutoClassifierInput: input => input.threads?.map(t => t.thread).join(' ') ?? '',
  async description() {
    return THREAD_WAIT_PROMPT
  },
  async prompt() {
    return THREAD_WAIT_PROMPT
  },
  mapToolResultToToolResultBlockParam: mapResult,
  renderToolUseMessage: input => `${input.threads?.length ?? 0} chat(s), ${input.timeout_s}s`,
  renderToolResultMessage,
  async call(input, context) {
    try {
      const r = await PastChats.wait(getSessionId(), input, context.abortController?.signal)
      const summary = r.woke
        ? `${r.woke.title}: ${r.woke.reason === 'needs_user' ? 'needs you' : 'turn finished'}`
        : 'No thread finished'
      return { data: { summary, text: asRecord(r) } }
    } catch (error) {
      return { data: { summary: 'Wait failed', text: messageOf(error) } }
    }
  },
} satisfies ToolDef<any, Output>)
