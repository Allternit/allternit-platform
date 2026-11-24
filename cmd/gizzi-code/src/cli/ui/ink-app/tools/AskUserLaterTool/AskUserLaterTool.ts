/**
 * ask_user_later in the Gizzi TUI: a thin adapter over the shared runtime
 * logic (Question.askLater in runtime/integrations/question, the same store
 * and limits the runtime builtin runtime/tools/builtins/ask-user-later.ts
 * uses). The ask is persisted and the tool returns at once; the REPL shows
 * the question as a block with "answer later with /answer <id>".
 *
 * When this terminal is a live engine session the ask is filed by the engine
 * (POST /api/v1/questions/async) under that session: `question.asked` goes
 * out live, the user's other devices show the card and get the push, and
 * their answer comes back through the live inbox as the next prompt. If the
 * engine can't take it the ask is written here and devices see it on their
 * next /v1/questions poll. Otherwise
 * it is filed under this terminal's session and answered with /answer, which
 * the REPL hook (hooks/useAskUserLaterAnswers.ts) turns into the next turn.
 */
import { z } from 'zod/v4'
import { parameters } from '@/runtime/tools/builtins/ask-user-later'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { ASK_USER_LATER_TOOL_NAME, DESCRIPTION, PROMPT } from './prompt.js'
import { getLiveSession } from '../../services/liveSession.js'
import { logForDebugging } from '../../utils/debug.js'
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

const ENGINE_TIMEOUT_MS = 5_000
const PUSH_VALUES = new Set<Output['push']>(['sent', 'not_requested', 'rate_limited', 'unpaired', 'unavailable'])

/** Same mapping as the runtime builtin. */
function pushOf(result: { pushed?: boolean; reason?: string }): Output['push'] {
  if (result.pushed) return 'sent'
  if (result.reason === 'rate_limited') return 'rate_limited'
  if (result.reason === 'unpaired') return 'unpaired'
  if (result.reason === 'incognito') return 'not_requested'
  return 'unavailable'
}

/**
 * Files the ask through the attached engine (POST /api/v1/questions/async,
 * the same path as the runtime tool: persist, publish `question.asked`, push).
 * Undefined when the engine can't take it (not running, an older engine
 * without the route, a session it doesn't know), so the caller writes it here.
 * The pending-ask limit is the engine's answer, so it is thrown, not retried.
 */
async function askThroughEngine(input: {
  sessionID: string
  questions: unknown[]
  assumption: string
}): Promise<{ request: { id: string; sessionID: string; assumption?: string }; push: Output['push'] } | undefined> {
  let res: Response
  try {
    res = await fetch(`${getLiveSession().engineURL}/api/v1/questions/async`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(input),
      signal: AbortSignal.timeout(ENGINE_TIMEOUT_MS),
    })
  } catch (error) {
    logForDebugging(`[ask_user_later] engine unreachable, filing the ask here: ${error}`)
    return undefined
  }
  const body = (await res.json().catch(() => ({}))) as any
  if (res.status === 429) throw new Error(String(body?.error ?? 'Too many questions are waiting for the user.'))
  if (!res.ok || !body?.request?.id) {
    logForDebugging(`[ask_user_later] engine refused the ask (${res.status}), filing it here`)
    return undefined
  }
  return { request: body.request, push: PUSH_VALUES.has(body.push) ? body.push : 'unavailable' }
}

/** No engine: the ask goes straight into this runtime's store (answered here with /answer). */
async function askHere(input: {
  sessionID: string
  questions: any[]
  assumption: string
  live: boolean
}): Promise<{ request: { id: string; sessionID: string; assumption?: string }; push: Output['push'] }> {
  const { Question } = await import('@/runtime/integrations/question')
  let request
  try {
    request = await withRuntime(() =>
      Question.askLater({ sessionID: input.sessionID, questions: input.questions, assumption: input.assumption }),
    )
  } catch (error) {
    if (error instanceof Question.AsyncLimitError) throw new Error(error.message)
    throw error
  }
  // A push only helps when the user can open the thread elsewhere (a live engine session).
  let push: Output['push'] = 'unavailable'
  if (input.live) {
    try {
      const { notifyUser } = await import('@/runtime/bots/app-bot-routing')
      const { UserMessage } = await import('@/runtime/primitives/user-message')
      push = pushOf(
        await notifyUser(request.sessionID, {
          kind: 'question',
          level: 'needs_you',
          preview: UserMessage.preview(input.questions[0].question),
          ref: request.id,
        }),
      )
    } catch {
      // No platform credential, API not running, timeout: the block is still in the terminal.
      push = 'unavailable'
    }
  }
  return { request, push }
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
    const live = askIsLive()
    const sessionID = askSessionID()
    // Attached to an engine: file the ask there, so `question.asked` reaches every device live.
    const { request, push } =
      (live && (await askThroughEngine({ sessionID, questions, assumption }))) ||
      (await askHere({ sessionID, questions, assumption, live }))
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
