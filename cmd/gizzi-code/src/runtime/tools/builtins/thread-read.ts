/**
 * Past-chat tools (Primitives Wave D, D1 recall): thread_read, thread_recent
 * and thread_wait. Core in runtime/primitives/past-chats.ts. Every client,
 * not bot-gated (thread_update / thread_close stay bot-only). Permission:
 * allow (loop/agent.ts).
 *
 * W1 stub: final ids and parameters; lane T fills the execute bodies and the
 * output text.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { PastChats } from "@/runtime/primitives/past-chats"
import DESCRIPTION from "@/runtime/tools/builtins/thread-read.txt"

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export const ThreadReadTool = Tool.define("thread_read", {
  description: DESCRIPTION,
  parameters: PastChats.ReadInput,
  async execute(params, ctx) {
    try {
      const r = await PastChats.read(ctx.sessionID, params, ctx.abort)
      return {
        title: `Read ${r.thread.title}`,
        output: JSON.stringify(r),
        metadata: { threadId: r.thread.id as string | undefined, status: r.status as string },
      }
    } catch (error) {
      return { title: "Chat not read", output: messageOf(error), metadata: { threadId: undefined as string | undefined, status: "error" } }
    }
  },
})

const RECENT_DESCRIPTION = `List the user's most recent chats (threads), newest first, with title, status, goal, summary and last activity. Filter by time (before / after) or project. Use thread_read to open one.`

export const ThreadRecentTool = Tool.define("thread_recent", {
  description: RECENT_DESCRIPTION,
  parameters: PastChats.RecentInput,
  async execute(params, ctx) {
    try {
      const r = await PastChats.recent(ctx.sessionID, params)
      return { title: `${r.threads.length} recent chat${r.threads.length === 1 ? "" : "s"}`, output: JSON.stringify(r), metadata: { count: r.threads.length } }
    } catch (error) {
      return { title: "Recent chats unavailable", output: messageOf(error), metadata: { count: 0 } }
    }
  },
})

const WAIT_DESCRIPTION = `Wait for one of up to 8 threads to finish a turn or need the user. Returns the first that does; on timeout, the latest status line of each. timeout_s 0 is a snapshot. Pass each target's cursor back as after_cursor so the same event is not returned twice.`

export const ThreadWaitTool = Tool.define("thread_wait", {
  description: WAIT_DESCRIPTION,
  parameters: PastChats.WaitInput,
  async execute(params, ctx) {
    try {
      const r = await PastChats.wait(ctx.sessionID, params, ctx.abort)
      return {
        title: r.woke ? `${r.woke.title}: ${r.woke.reason === "needs_user" ? "needs you" : "turn finished"}` : "No thread finished",
        output: JSON.stringify(r),
        metadata: { woke: (r.woke?.thread ?? null) as string | null, timedOut: r.timed_out },
      }
    } catch (error) {
      return { title: "Wait failed", output: messageOf(error), metadata: { woke: null as string | null, timedOut: false } }
    }
  },
})

/** Past-chat tools: every session (Wave D decision 8). */
export const PAST_CHAT_TOOL_IDS = [ThreadReadTool.id, ThreadRecentTool.id, ThreadWaitTool.id] as const
