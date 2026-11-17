// @ts-nocheck
/**
 * ask_user_later in the Gizzi TUI: a thin adapter over the shared runtime
 * logic (Question.askLater in runtime/integrations/question, the same store
 * and limits the runtime builtin runtime/tools/builtins/ask-user-later.ts
 * uses). The ask is persisted and the tool returns at once; the REPL shows
 * the question as a block with "answer later with /answer <id>".
 *
 * When this terminal is a live engine session the ask is filed under that
 * session, so the user's other devices show the card and get the push, and
 * their answer comes back through the live inbox as the next prompt. Otherwise
 * it is filed under this terminal's session and answered with /answer, which
 * the REPL hook (hooks/useAskUserLaterAnswers.ts) turns into the next turn.
 */
import { z } from 'zod/v4'
import { parameters } from '@/runtime/tools/builtins/ask-user-later'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { ASK_USER_LATER_TOOL_NAME, DESCRIPTION, PROMPT } from './prompt.js'
import { askIsLive, askSessionID, withRuntime } from './session.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => parameters.strict())
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    ask_id: z.string(),
    session_id: z.string(),
    questions: z.array(z.any()),
    assumption: z.string(),
    push: z.enum(['sent', 'not_requested', 'rate_limited', 'unpaired', 'unavailable']),
    live: z.boolean(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>

export type Output = z.infer<OutputSchema>

/** Same mapping as the runtime builtin. */
function pushOf(result: { pushed?: boolean; reason?: string }): Output['push'] {
  if (result.pushed) return 'sent'
  if (result.reason === 'rate_limited') return 'rate_limited'
  if (result.reason === 'unpaired') return 'unpaired'
  if (result.reason === 'incognito') return 'not_requested'
  return 'unavailable'
}

export const AskUserLaterTool = buildTool({
  name: ASK_USER_LATER_TOOL_NAME,
  searchHint: 'ask the user a question without waiting for the answer',
  maxResultSizeChars: 10_000,
  userFacingName: () => 'Question for later',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isConcurrencySafe() {
    return false
  },
  toAutoClassifierInput(input) {
    return input.questions.map(q => q.question).join(' | ')
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return {
      tool_use_id: toolUseID,
      type: 'tool_result',
      content: `Asked (ask_id ${output.ask_id}); the answer will arrive as a new message. Continue with your stated assumption.`,
    }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call({ questions, assumption }) {
    const { Question } = await import('@/runtime/integrations/question')
    const live = askIsLive()
    let request
    try {
      request = await withRuntime(() => Question.askLater({ sessionID: askSessionID(), questions, assumption }))
    } catch (error) {
      if (error instanceof Question.AsyncLimitError) throw new Error(error.message)
      throw error
    }
    // A push only helps when the user can open the thread elsewhere (a live engine session).
    let push: Output['push'] = 'unavailable'
    if (live) {
      try {
        const { notifyUser } = await import('@/runtime/bots/app-bot-routing')
        const { UserMessage } = await import('@/runtime/primitives/user-message')
        push = pushOf(
          await notifyUser(request.sessionID, {
            kind: 'question',
            level: 'needs_you',
            preview: UserMessage.preview(questions[0].question),
            ref: request.id,
          }),
        )
      } catch {
        // No platform credential, API not running, timeout: the block is still in the terminal.
        push = 'unavailable'
      }
    }
    return {
      data: {
        ask_id: request.id,
        session_id: request.sessionID,
        questions,
        assumption: request.assumption ?? assumption,
        push,
        live,
      },
    }
  },
} satisfies ToolDef<InputSchema, Output>)
