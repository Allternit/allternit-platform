/**
 * ask_user_later: a question whose turn does not wait. The ask is persisted
 * (Question.askLater), shown on every surface as a card with the assumption,
 * and pushed to the user's devices; the tool returns at once. The answer
 * starts a new turn in the thread (Question.reply). A subagent's ask belongs
 * to its root session. Every client gets it (not bot-gated). The blocking
 * `question` tool is unchanged.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Question } from "@/runtime/integrations/question"
import { notifyUser } from "@/runtime/bots/app-bot-routing"
import { UserMessage } from "@/runtime/primitives/user-message"
import DESCRIPTION from "@/runtime/tools/builtins/ask-user-later.txt"

export const parameters = z.object({
  questions: z
    .array(Question.Info.omit({ plan: true }))
    .min(1)
    .max(Question.ASYNC_MAX_QUESTIONS)
    .describe(`1 to ${Question.ASYNC_MAX_QUESTIONS} questions to ask`),
  assumption: z
    .string()
    .trim()
    .min(1)
    .max(Question.ASYNC_MAX_ASSUMPTION)
    .describe("What you will do until the user answers, in one line"),
})

export type Metadata = {
  ask_id: string
  /** The thread the ask belongs to (the root session). */
  sessionID: string
  questions: Question.Info[]
  assumption: string
  push: UserMessage.Push
}

function pushOf(result: { pushed?: boolean; reason?: string }): UserMessage.Push {
  if (result.pushed) return "sent"
  if (result.reason === "rate_limited") return "rate_limited"
  if (result.reason === "unpaired") return "unpaired"
  if (result.reason === "incognito") return "not_requested"
  return "unavailable"
}

/**
 * Files an ask: persists it and publishes `question.asked` (Question.askLater),
 * then pushes it to the user's devices. The one path behind the tool and the
 * engine route (POST /api/v1/questions/async) the Gizzi TUI calls, so a
 * terminal ask reaches every device live. Throws Question.AsyncLimitError.
 */
export async function fileAsk(input: {
  sessionID: string
  questions: Question.Info[]
  assumption: string
}): Promise<{ request: Question.Request; push: UserMessage.Push }> {
  const request = await Question.askLater(input)
  let push: UserMessage.Push
  try {
    const result = await notifyUser(request.sessionID, {
      kind: "question",
      level: "needs_you",
      preview: UserMessage.preview(input.questions[0]!.question),
      ref: request.id,
    })
    push = pushOf(result)
  } catch {
    // No platform credential, API not running, timeout: the card is still in the thread.
    push = "unavailable"
  }
  return { request, push }
}

export const AskUserLaterTool = Tool.define("ask_user_later", {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    await ctx.ask({
      permission: "ask_user_later",
      patterns: ["*"],
      always: ["*"],
      metadata: { questions: params.questions.length },
    })
    const { request, push } = await fileAsk({
      sessionID: ctx.sessionID,
      questions: params.questions,
      assumption: params.assumption,
    }).catch((error) => {
      if (error instanceof Question.AsyncLimitError) throw new Error(error.message)
      throw error
    })
    const metadata: Metadata = {
      ask_id: request.id,
      sessionID: request.sessionID,
      questions: params.questions,
      assumption: request.assumption ?? params.assumption,
      push,
    }
    const n = params.questions.length
    return {
      title: `Asked for later: ${params.questions[0]!.header || `${n} question${n === 1 ? "" : "s"}`}`,
      output: `Asked (ask_id ${request.id}); the answer will arrive as a new message. Continue with your stated assumption.`,
      metadata,
    }
  },
})
