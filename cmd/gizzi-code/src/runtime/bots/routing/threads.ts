/**
 * Past-chat routes (Primitives Wave D, D1 recall). Server: allternit-api
 * `thread_read_tools.rs`, which works in every session the caller owns (not
 * only bot chats), maps a thread to its gizzi session, checks the owner and
 * never returns an incognito chat. Whose session it is comes from the
 * session id, never from model input.
 *
 * - `POST /bot-routing/thread/resolve` `{sessionId, thread}` → which session
 *   holds a thread's transcript and whether it lives on this computer.
 * - `POST /bot-routing/thread/recent` `{sessionId, limit, before?, after?, project?}`
 *   → the owner's most recent threads.
 * - `POST /bot-routing/thread/wait` `{sessionId, targets, timeoutS}` → the
 *   first target that finished a turn or now needs the user (bot_events).
 *
 * W1 stub: final signatures; lane T fills the shapes and error handling.
 * Free of runtime state and UI imports so bun tests can stub `fetch`.
 */
import { botRoutingPost, type AppThreadSummary } from "@/runtime/bots/app-bot-routing"

/** Where a thread's transcript lives, from the caller's point of view. */
export interface ResolvedThread {
  id: string
  title: string
  status: string
  /** The gizzi session that holds the transcript, when there is one. */
  sessionId: string | null
  /** True when that session is in this runtime's local store. */
  local: boolean
  /** The computer the transcript lives on when it is not local (display name). */
  computer?: string | null
  summary?: string | null
  lastActivityAt?: string | null
}

export async function resolveThread(sessionID: string, input: { thread: string }): Promise<{ thread: ResolvedThread }> {
  return botRoutingPost("/bot-routing/thread/resolve", { sessionId: sessionID, ...input }, 10_000)
}

export interface RecentThreadsRequest {
  limit: number
  before?: string
  after?: string
  project?: string
}

export async function recentThreads(sessionID: string, input: RecentThreadsRequest): Promise<{ threads: AppThreadSummary[] }> {
  return botRoutingPost("/bot-routing/thread/recent", { sessionId: sessionID, ...input }, 10_000)
}

export interface ThreadWaitTarget {
  thread: string
  afterCursor?: string
}

/** One target's latest state, as a status line. */
export interface ThreadWaitStatus {
  thread: string
  title: string
  status: string
  statusLine?: string | null
  /** Pass back as `afterCursor` so the same event is not delivered twice. */
  cursor: string
}

export interface ThreadWaitResult {
  /** The first target that finished a turn or needs the user; null on timeout or a snapshot. */
  woke: (ThreadWaitStatus & { reason: "turn_finished" | "needs_user" }) | null
  timedOut: boolean
  statuses: ThreadWaitStatus[]
}

/** Long-polls up to `timeoutS` (0 = snapshot); the HTTP timeout leaves room for the server to answer. */
export async function waitThreads(sessionID: string, input: { targets: ThreadWaitTarget[]; timeoutS: number }): Promise<ThreadWaitResult> {
  return botRoutingPost("/bot-routing/thread/wait", { sessionId: sessionID, ...input }, (input.timeoutS + 15) * 1000)
}
