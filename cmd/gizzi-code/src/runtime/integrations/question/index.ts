import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"
import z from "zod/v4"

export namespace Question {
  const log = Log.create({ service: "question" })

  const Leaf = z.object({
    label: z.string().describe("Display text (1-5 words, concise)"),
    description: z.string().describe("Explanation of choice, or what happens if picked"),
    recommended: z.boolean().optional().describe("Your pick (at most one per list)"),
    caution: z.string().optional().describe("A short cost or risk of this choice, shown highlighted (e.g. 'Skips 1,284 tests')"),
  })

  export const FollowUp = z.object({
    question: z.string().describe("What to settle next if this option is picked"),
    options: z.array(Leaf).describe("Choices for the follow-up"),
  })
  export type FollowUp = z.infer<typeof FollowUp>

  export const Option = Leaf.extend({
    then: FollowUp.optional().describe("Decision tree: a follow-up question shown when this option is picked"),
  })

  export type Option = z.infer<typeof Option>

  export const Info = z
    .object({
      question: z.string().describe("Complete question"),
      header: z.string().describe("Very short label (max 30 chars)"),
      options: z.array(Option).describe("Available choices"),
      multiple: z.boolean().optional().describe("Allow selecting multiple choices"),
      custom: z.boolean().optional().describe("Allow typing a custom answer (default: true)"),
      risk: z.enum(["low", "medium", "high"]).optional().describe("Decision tree: how much is at stake"),
      context: z.string().optional().describe("Decision tree: the facts behind the question, one or two short lines"),
    })

  export type Info = z.infer<typeof Info>

  /** A decision tree: any option has a follow-up, or the question carries risk/context. */
  export function isTree(q: Info) {
    return !q.multiple && (q.options.some((o) => o.then) || !!q.risk || !!q.context)
  }

  /** One answer as text: a tree answer is a path ("Roll back → Post on the status page"). */
  export function formatAnswer(q: Info, answer: Answer | undefined) {
    if (!answer?.length) return ""
    return answer.join(isTree(q) ? " → " : ", ")
  }

  export const Request = z
    .object({
      id: Identifier.schema("question"),
      sessionID: Identifier.schema("session"),
      questions: z.array(Info).describe("Questions to ask"),
      tool: z
        .object({
          messageID: z.string(),
          callID: z.string(),
        })
        .optional(),
    })
    
  export type Request = z.infer<typeof Request>

  export const Answer = z.array(z.string())
  export type Answer = z.infer<typeof Answer>

  export const Reply = z.object({
    answers: z
      .array(Answer)
      .describe("User answers in order of questions (each answer is an array of selected labels)"),
  })
  export type Reply = z.infer<typeof Reply>

  export const Event = {
    Asked: BusEvent.define("question.asked", Request),
    Replied: BusEvent.define(
      "question.replied",
      z.object({
        sessionID: z.string(),
        requestID: z.string(),
        answers: z.array(Answer),
      }),
    ),
    Rejected: BusEvent.define(
      "question.rejected",
      z.object({
        sessionID: z.string(),
        requestID: z.string(),
      }),
    ),
  }

  const state = Instance.state(async () => {
    const pending: Record<
      string,
      {
        info: Request
        resolve: (answers: Answer[]) => void
        reject: (e: any) => void
      }
    > = {}

    return {
      pending,
    }
  })

  export async function ask(input: {
    sessionID: string
    questions: Info[]
    tool?: { messageID: string; callID: string }
  }): Promise<Answer[]> {
    const s = await state()
    const id = Identifier.ascending("question")

    log.info("asking", { id, questions: input.questions.length })

    return new Promise<Answer[]>((resolve, reject) => {
      const info: Request = {
        id,
        sessionID: input.sessionID,
        questions: input.questions,
        tool: input.tool,
      }
      s.pending[id] = {
        info,
        resolve,
        reject,
      }
      Bus.publish(Event.Asked, info)
    })
  }

  export async function reply(input: { requestID: string; answers: Answer[] }): Promise<void> {
    const s = await state()
    const existing = s.pending[input.requestID]
    if (!existing) {
      log.warn("reply for unknown request", { requestID: input.requestID })
      return
    }
    delete s.pending[input.requestID]

    log.info("replied", { requestID: input.requestID, answers: input.answers })

    Bus.publish(Event.Replied, {
      sessionID: existing.info.sessionID,
      requestID: existing.info.id,
      answers: input.answers,
    })

    existing.resolve(input.answers)
  }

  export async function reject(requestID: string): Promise<void> {
    const s = await state()
    const existing = s.pending[requestID]
    if (!existing) {
      log.warn("reject for unknown request", { requestID })
      return
    }
    delete s.pending[requestID]

    log.info("rejected", { requestID })

    Bus.publish(Event.Rejected, {
      sessionID: existing.info.sessionID,
      requestID: existing.info.id,
    })

    existing.reject(new RejectedError())
  }

  export class RejectedError extends Error {
    constructor() {
      super("The user dismissed this question")
    }
  }

  export async function list() {
    return state().then((x) => Object.values(x.pending).map((x) => x.info))
  }
}
