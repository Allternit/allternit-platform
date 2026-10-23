import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Question } from "@/runtime/integrations/question"
import DESCRIPTION from "@/runtime/tools/builtins/question.txt"

export const QuestionTool = Tool.define("question", {
  description: DESCRIPTION,
  parameters: z.object({
    questions: z.array(Question.Info.omit({ custom: true })).describe("Questions to ask"),
  }),
  async execute(params, ctx) {
    const answers = await Question.ask({
      sessionID: ctx.sessionID,
      questions: params.questions,
      tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
    })

    const formatted = params.questions
      .map((q, i) => `"${q.question}"="${Question.formatAnswer(q, answers[i]) || "Unanswered"}"`)
      .join(", ")

    return {
      title: `Asked ${params.questions.length} question${params.questions.length > 1 ? "s" : ""}`,
      output: `User has answered your questions: ${formatted}. You can now continue with the user's answers in mind.`,
      metadata: {
        answers,
      },
    }
  },
})
