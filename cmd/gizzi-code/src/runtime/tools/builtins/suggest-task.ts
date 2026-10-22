/**
 * suggest_task: a bot suggests work and waits for its user's go-ahead.
 *
 * Only in an app bot's own chat (bots made in the Allternit app; gated in
 * SessionPrompt.resolveTools with isAppBotSession, like message_agent).
 * allternit-api opens a task thread for this bot in status `proposed`; the
 * user sees it on the bot's stage, in its Threads and in the Inbox, and
 * answers "Go ahead" (the work runs in that thread and the result is posted
 * back into this chat) or "Not now". Nothing runs before that.
 * See cmd/allternit-api/src/bot_suggestions.rs.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { AppRoutingError, suggestAppTask } from "@/runtime/bots/app-bot-routing"

const DESCRIPTION = `Suggest a task to your user and wait for their go-ahead.

Use it when you notice work worth doing that your user didn't ask for (a
follow-up, a fix, something coming up). The task waits on your screen and in
their Inbox until they answer. "Go ahead" starts it in its own thread and the
result is posted back here; "Not now" closes it. Do not start the work
yourself before they answer.

- title: a short name for the task (under 80 characters).
- objective: what you would do and what they get, in one or two sentences.
- why: optional, the reason you're suggesting it now.

You can have a few suggestions waiting at once; suggesting the same title
again returns the open one.`

export const SuggestTaskTool = Tool.define("suggest_task", {
  description: DESCRIPTION,
  parameters: z.object({
    title: z.string().describe("Short name for the task"),
    objective: z.string().describe("What you would do and what the user gets"),
    why: z.string().optional().describe("Why you're suggesting it now"),
  }),
  async execute(params, ctx) {
    try {
      const r = await suggestAppTask(ctx.sessionID, { title: params.title, objective: params.objective, why: params.why })
      return {
        title: `Suggested: ${params.title}`,
        output: r.existing
          ? `That suggestion is already waiting for your user (thread ${r.threadId}).`
          : `Suggested (thread ${r.threadId}). It waits for your user's go-ahead; the result comes back to this chat when it's done.`,
        metadata: { threadId: r.threadId, status: r.status, existing: r.existing },
      }
    } catch (error) {
      const message = error instanceof AppRoutingError ? error.message : (error as Error).message
      return {
        title: "Not suggested",
        output: message,
        metadata: { threadId: undefined as string | undefined, status: "refused", existing: false },
      }
    }
  },
})
