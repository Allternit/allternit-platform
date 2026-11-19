/**
 * end_conversation in the Gizzi TUI: prompt-gap contract stub (G3). Same id, input
 * schema and description as the runtime builtin
 * (runtime/tools/builtins/end-conversation.ts), and enabled only while that module's
 * `available` is true, so flipping it there turns on both. tools.ts already
 * registers this adapter; G7-9 (PR11g) builds it out in this folder and never
 * edits tools.ts.
 */
import { available, EndConversationTool as RuntimeEndConversationTool } from '@/runtime/tools/builtins/end-conversation'
import { END_CONVERSATION_TOOL_ID, EndConversationInput } from '@/runtime/primitives/gap-types'
import DESCRIPTION_TXT from '@/runtime/tools/builtins/end-conversation.txt'
import { getSessionId } from '../../bootstrap/state'
import { buildTool, type ToolDef } from '../../Tool.js'

export const END_CONVERSATION_TOOL_NAME = END_CONVERSATION_TOOL_ID

type InputSchema = typeof EndConversationInput
type Output = { ok: boolean; message: string; ended?: boolean }

/** What the TUI shows once the chat has ended. */
export const CHAT_ENDED_NOTICE = 'This chat has ended. Start a new one with /clear.'

let endedSession: string | null = null
/** True once end_conversation succeeded in this TUI session; /clear switches to a new session id, which clears it. */
export function isChatEnded(): boolean {
  return endedSession !== null && endedSession === getSessionId()
}

export const EndConversationTool = buildTool({
  name: END_CONVERSATION_TOOL_ID,
  maxResultSizeChars: 2_000,
  userFacingName: () => 'End chat',
  get inputSchema(): InputSchema {
    return EndConversationInput
  },
  isEnabled() {
    return available
  },
  async description() {
    return DESCRIPTION_TXT
  },
  async prompt() {
    return DESCRIPTION_TXT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.message }
  },
  renderToolUseMessage() {
    return ''
  },
  async call(input, context) {
    const sessionID = getSessionId()
    try {
      const tool = await RuntimeEndConversationTool.init()
      const out = await tool.execute(input, {
        sessionID,
        messageID: String(context.agentId ?? sessionID),
        agent: 'gizzi',
        abort: context.abortController.signal,
        extra: {},
        messages: [],
        metadata() {},
        async ask() {},
      })
      const ok = out.metadata.ok === true
      if (ok) endedSession = sessionID
      return { data: { ok, ended: ok, message: ok ? `${out.output}\n${CHAT_ENDED_NOTICE}` : out.output } }
    } catch (e) {
      // A terminal chat may have no runtime session row (the TUI keeps its own transcript).
      // End it locally under the same rules: abuse only after a warning; a request is the person's own.
      if (input.reason === 'user_requested' || input.warned === true) {
        endedSession = sessionID
        return { data: { ok: true, ended: true, message: `The chat has ended.\n${CHAT_ENDED_NOTICE}` } }
      }
      return { data: { ok: false, message: e instanceof Error ? e.message : 'Could not end the chat.' } }
    }
  },
} satisfies ToolDef<InputSchema, Output>)
