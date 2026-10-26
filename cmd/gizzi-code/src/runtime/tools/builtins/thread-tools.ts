/**
 * Thread tools (W3, "threads as war rooms"): a bot keeps its own thread
 * accurate. Only in an app bot's chat (gated in SessionPrompt.resolveTools
 * with isAppBotSession, like suggest_task). The thread is always the calling
 * session's own; the server (allternit-api thread_bot_tools.rs) checks that.
 *
 * - thread_update: rename / restate the goal, or flag that the owner is needed.
 * - thread_close: done or dropped. Closing revokes "allowed for this task"
 *   grants and stops reminders set in the thread (thread_closeout.rs).
 * - thread_search: the owner's threads, including closed and archived ones.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { AppRoutingError, closeAppThread, searchAppThreads, updateAppThread } from "@/runtime/bots/app-bot-routing"

function messageOf(error: unknown): string {
  return error instanceof AppRoutingError ? error.message : (error as Error).message
}

const UPDATE_DESCRIPTION = `Update the thread you are working in.

- title / goal: when the user changes what this thread is for, rename it (a
  short name, under 80 characters) and restate the goal in a sentence, so their
  rail shows what you're doing now. Your main chat keeps its name.
- needs_you: when you can't go on without the user, give a short reason. Use
  urgency "urgent" only for time-critical things (an outage, money at risk, a
  person waiting on the owner today): it reaches the owner during quiet hours
  and repeats until opened. Everything else is "normal".`

export const ThreadUpdateTool = Tool.define("thread_update", {
  description: UPDATE_DESCRIPTION,
  parameters: z.object({
    title: z.string().optional().describe("New short name for the thread"),
    goal: z.string().optional().describe("What the thread is for now, in a sentence"),
    needs_you: z
      .object({
        reason: z.string().describe("What you need from the user, in a short line"),
        urgency: z.enum(["urgent", "normal"]).optional().describe('"urgent" only for time-critical asks'),
      })
      .optional()
      .describe("Flag that the user is needed before you can go on"),
  }),
  async execute(params, ctx) {
    try {
      const r = await updateAppThread(ctx.sessionID, {
        title: params.title,
        goal: params.goal,
        needsYou: params.needs_you ? { reason: params.needs_you.reason, urgency: params.needs_you.urgency } : undefined,
      })
      return {
        title: params.needs_you ? "Asked for you" : `Thread: ${r.thread.title}`,
        output: params.needs_you
          ? `The thread now shows that it needs the user: ${params.needs_you.reason}`
          : `Thread updated. It's now called "${r.thread.title}".`,
        metadata: { threadId: r.thread.id as string | undefined, status: r.thread.status },
      }
    } catch (error) {
      return { title: "Thread not updated", output: messageOf(error), metadata: { threadId: undefined as string | undefined, status: "refused" } }
    }
  },
})

const CLOSE_DESCRIPTION = `Close the thread you are working in.

- outcome "dropped": the user said to stop, drop, cancel or forget this task.
  The thread is archived out of their rail.
- outcome "done": the work is finished and the user has what they asked for.
- note: one line on how it ended (what was done, or why it was dropped).

Closing ends any "allowed for this task" permissions and stops reminders and
routines set in this thread. The transcript stays; you can still answer
questions about it afterwards. Your main chat never closes.`

export const ThreadCloseTool = Tool.define("thread_close", {
  description: CLOSE_DESCRIPTION,
  parameters: z.object({
    outcome: z.enum(["done", "dropped"]).describe("done = finished; dropped = the user called it off"),
    note: z.string().describe("One line on how it ended"),
  }),
  async execute(params, ctx) {
    try {
      const r = await closeAppThread(ctx.sessionID, { outcome: params.outcome, note: params.note })
      return {
        title: params.outcome === "done" ? `Closed: ${r.thread.title}` : `Dropped: ${r.thread.title}`,
        output:
          params.outcome === "done"
            ? "Thread closed as done. Its reminders and task permissions have stopped."
            : "Thread dropped and archived. Its reminders and task permissions have stopped.",
        metadata: { threadId: r.thread.id as string | undefined, status: r.thread.status },
      }
    } catch (error) {
      return { title: "Thread not closed", output: messageOf(error), metadata: { threadId: undefined as string | undefined, status: "refused" } }
    }
  },
})

const SEARCH_DESCRIPTION = `Search the user's past and current threads (open, closed and archived) by
what they were about. Returns each match's title, goal, status and summary.
Use it when the user asks about earlier work ("what happened with the venue?").`

export const ThreadSearchTool = Tool.define("thread_search", {
  description: SEARCH_DESCRIPTION,
  parameters: z.object({
    query: z.string().describe("Words the thread's title, goal or summary would contain"),
  }),
  async execute(params, ctx) {
    try {
      const threads = await searchAppThreads(ctx.sessionID, params.query)
      if (threads.length === 0) {
        return { title: "No threads found", output: `No threads match "${params.query}".`, metadata: { count: 0 } }
      }
      const lines = threads.map((t) => {
        const state = t.archived ? "archived" : t.closed ? `closed (${t.status})` : t.status
        const bits = [`- ${t.title} [${state}] id ${t.id}`]
        if (t.botName) bits.push(`  bot: ${t.botName}`)
        if (t.goal) bits.push(`  goal: ${t.goal}`)
        if (t.summary) bits.push(`  summary: ${t.summary}`)
        if (t.statusLine) bits.push(`  last status: ${t.statusLine}`)
        bits.push(`  last active: ${t.lastActivityAt}`)
        return bits.join("\n")
      })
      return { title: `${threads.length} thread${threads.length === 1 ? "" : "s"}`, output: lines.join("\n"), metadata: { count: threads.length } }
    } catch (error) {
      return { title: "Search failed", output: messageOf(error), metadata: { count: 0 } }
    }
  },
})

export const THREAD_TOOL_IDS = [ThreadUpdateTool.id, ThreadCloseTool.id, ThreadSearchTool.id] as const
