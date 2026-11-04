import z from "zod/v4"
import path from "path"
import fs from "fs/promises"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Question } from "@/runtime/integrations/question"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Identifier } from "@/shared/id/id"
import { Provider } from "@/runtime/providers/provider"
import { Instance } from "@/runtime/context/project/instance"
import { Todo } from "@/runtime/session/todo"
import EXIT_DESCRIPTION from "@/runtime/tools/builtins/plan-exit.txt"
import ENTER_DESCRIPTION from "@/runtime/tools/builtins/plan-enter.txt"

async function getLastModel(sessionID: string) {
  for await (const item of MessageV2.stream(sessionID)) {
    if (item.info.role === "user" && item.info.model) return item.info.model
  }
  return Provider.defaultModel()
}

/**
 * The plan-approval answer that also allows the plan's sends and uploads for
 * this bot thread. allternit-api (`agent_session_routes::PLAN_GRANT_LABEL`)
 * matches this exact label on the person's reply and records a one-task grant.
 */
export const PLAN_GRANT_LABEL = "Approve plan and allow its sends and uploads"

/** The approved steps as the agent's instruction: numbered, with the person's edits called out. */
export function approvedStepsText(steps: Question.PlanStep[], edited: boolean): string {
  if (!steps.length) return ""
  const list = steps.map((s, i) => `${i + 1}. ${s.title}${s.detail ? ` (${s.detail})` : ""}`).join("\n")
  return edited
    ? `\n\nThe user edited the plan before approving it. Run these steps, in this order, instead of the plan file's list:\n${list}\nThe to-do list is set to these steps; mark each one in_progress and completed with todowrite as you go.`
    : `\n\nApproved steps:\n${list}\nThe to-do list is set to these steps; mark each one in_progress and completed with todowrite as you go.`
}

/**
 * A plan's steps from its Markdown: the top-level numbered or bulleted items
 * (checkboxes included). Fewer than two items means the plan is prose, not a
 * list, and the card falls back to the plain question.
 */
export function stepsFromMarkdown(markdown: string): Question.PlanStep[] {
  const numbered: string[] = []
  const bullets: string[] = []
  for (const line of markdown.split("\n")) {
    const m = /^(\d+[.)]|[-*+])\s+(?:\[[ xX]\]\s+)?(.+)$/.exec(line)
    if (!m) continue
    const title = m[2].replace(/\*\*/g, "").trim()
    ;(/^\d/.test(m[1]) ? numbered : bullets).push(title)
  }
  // Numbered items are the steps; bullets count only when nothing is numbered.
  const items = numbered.length >= 2 ? numbered : bullets
  return items.length >= 2 ? (Question.readPlan(items.map((title) => ({ title }))) ?? []) : []
}

function sameSteps(a: Question.PlanStep[], b: Question.PlanStep[]) {
  return a.length === b.length && a.every((s, i) => s.title === b[i].title && (s.detail ?? "") === (b[i].detail ?? ""))
}

export const PlanExitTool = Tool.define("plan_exit", {
  description: EXIT_DESCRIPTION,
  parameters: z.object({
    steps: z
      .array(Question.PlanStep)
      .max(Question.MAX_PLAN_STEPS)
      .optional()
      .describe("The plan's steps in order, one short line each. The user can edit them before approving."),
  }),
  async execute(params, ctx) {
    const session = await Session.get(ctx.sessionID)
    const plan = path.relative(Instance.worktree, Session.plan(session))
    const given = Question.readPlan(params.steps ?? []) ?? []
    const proposed = given.length
      ? given
      : stepsFromMarkdown(await fs.readFile(Session.plan(session), "utf8").catch(() => ""))
    const { answers, plan: editedPlan } = await Question.askWithPlan({
      sessionID: ctx.sessionID,
      questions: [
        {
          question: `Plan at ${plan} is complete. Would you like to switch to the build agent and start implementing?`,
          header: "Build Agent",
          custom: false,
          ...(proposed.length ? { plan: { steps: proposed } } : {}),
          options: [
            { label: "Yes", description: "Switch to build agent and start implementing the plan" },
            {
              label: PLAN_GRANT_LABEL,
              description: "Start building, and let the plan's messages and file uploads go out without asking again for this task (payments, signatures and deletions still ask)",
            },
            { label: "No", description: "Stay with plan agent to continue refining the plan" },
          ],
        },
      ],
      tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
    })

    const answer = answers[0]?.[0]
    if (answer === "No") throw new Question.RejectedError()

    // The steps the person approved (edited or as proposed) become the live to-do list.
    const approved = proposed.length ? (editedPlan?.length ? editedPlan : proposed) : []
    const edited = approved.length > 0 && !sameSteps(approved, proposed)
    if (approved.length) {
      Todo.update({
        sessionID: ctx.sessionID,
        todos: approved.map((s) => ({ content: s.title, status: "pending", priority: "medium" })),
      })
    }

    const model = await getLastModel(ctx.sessionID)

    const userMsg: MessageV2.User = {
      id: Identifier.ascending("message"),
      sessionID: ctx.sessionID,
      role: "user",
      time: {
        created: Date.now(),
      },
      agent: "build",
      model,
    }
    await Session.updateMessage(userMsg)
    await Session.updatePart({
      id: Identifier.ascending("part"),
      messageID: userMsg.id,
      sessionID: ctx.sessionID,
      type: "text",
      text:
        (answer === PLAN_GRANT_LABEL
          ? `The plan at ${plan} has been approved, you can now edit files. Execute the plan. The user also allowed the plan's sends and uploads for this task, so those go out without a new approval (payments, signatures and deletions still ask).`
          : `The plan at ${plan} has been approved, you can now edit files. Execute the plan`) + approvedStepsText(approved, edited),
      synthetic: true,
    } satisfies MessageV2.TextPart)

    return {
      title: "Switching to build agent",
      output: "User approved switching to build agent. Wait for further instructions.",
      metadata: {},
    }
  },
})

export const PlanEnterTool = Tool.define("plan_enter", {
  description: ENTER_DESCRIPTION,
  parameters: z.object({}),
  async execute(_params, ctx) {
    const session = await Session.get(ctx.sessionID)
    const plan = path.relative(Instance.worktree, Session.plan(session))

    const answers = await Question.ask({
      sessionID: ctx.sessionID,
      questions: [
        {
          question: `Would you like to switch to the plan agent and create a plan saved to ${plan}?`,
          header: "Plan Mode",
          custom: false,
          options: [
            { label: "Yes", description: "Switch to plan agent for research and planning" },
            { label: "No", description: "Stay with build agent to continue making changes" },
          ],
        },
      ],
      tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
    })

    const answer = answers[0]?.[0]

    if (answer === "No") throw new Question.RejectedError()

    const model = await getLastModel(ctx.sessionID)

    const userMsg: MessageV2.User = {
      id: Identifier.ascending("message"),
      sessionID: ctx.sessionID,
      role: "user",
      time: {
        created: Date.now(),
      },
      agent: "plan",
      model,
    }
    await Session.updateMessage(userMsg)
    await Session.updatePart({
      id: Identifier.ascending("part"),
      messageID: userMsg.id,
      sessionID: ctx.sessionID,
      type: "text",
      text: "User has requested to enter plan mode. Switch to plan mode and begin planning.",
      synthetic: true,
    } satisfies MessageV2.TextPart)

    return {
      title: "Switching to plan agent",
      output: `User confirmed to switch to plan mode. A new message has been created to switch you to plan mode. The plan file will be at ${plan}. Begin planning.`,
      metadata: {},
    }
  },
})
