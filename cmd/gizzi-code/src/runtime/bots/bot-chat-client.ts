/**
 * Network side of the terminal `/bots` chat (see bot-chat-view.ts): the live
 * `/agent-sessions/sync` feed plus the calls that act on a running bot turn
 * (stop it, answer its permission and question prompts). All go through the
 * platform gateway, the same routes Desktop uses.
 *
 * Free of CLI/UI imports so bun tests can drive it with a stubbed fetch.
 */
import { parseSSE, type SyncEvent } from "@/runtime/bots/bot-chat-view"
import { PlatformApiError, platformFetch, platformRequest, PlatformSignedOutError } from "@/runtime/bots/platform-api"

const enc = encodeURIComponent
const ACTION = { timeoutMs: 15_000 }

export interface SyncStreamOptions {
  onEvent: (event: SyncEvent) => void
  /** Connection state, for a "reconnecting…" hint. */
  onStatus?: (status: "open" | "retrying", error?: unknown) => void
  signal: AbortSignal
  /** Test seam: waits between reconnects. */
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>
}

const RETRY_MIN_MS = 500
const RETRY_MAX_MS = 15_000

function defaultSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(done, ms)
    function done() {
      clearTimeout(timer)
      signal.removeEventListener("abort", done)
      resolve()
    }
    signal.addEventListener("abort", done, { once: true })
  })
}

/**
 * Follow the sync feed until `signal` aborts. Reconnects with backoff and
 * resumes from the last event id (`?since=`), so a dropped connection replays
 * the gap instead of losing streamed text. Signed out is final: it throws.
 */
export async function openSyncStream(options: SyncStreamOptions): Promise<void> {
  const { onEvent, onStatus, signal } = options
  const sleep = options.sleep ?? defaultSleep
  let since: string | undefined
  let delay = RETRY_MIN_MS
  while (!signal.aborted) {
    try {
      const path = `/api/v1/agent-sessions/sync${since ? `?since=${enc(since)}` : ""}`
      const response = await platformFetch("GET", path, undefined, { accept: "text/event-stream", signal })
      if (!response.ok || !response.body) {
        const text = await response.text().catch(() => "")
        throw new PlatformApiError(response.status, text || `HTTP ${response.status}`)
      }
      onStatus?.("open")
      delay = RETRY_MIN_MS
      for await (const { id, data } of parseSSE(response.body)) {
        if (id) since = id
        onEvent(data)
      }
    } catch (error) {
      if (signal.aborted) return
      if (error instanceof PlatformSignedOutError) throw error
      if (error instanceof PlatformApiError && (error.status === 401 || error.status === 403)) throw error
      onStatus?.("retrying", error)
    }
    if (signal.aborted) return
    // The server closed the stream or it failed: back off, then resume.
    await sleep(delay, signal)
    delay = Math.min(delay * 2, RETRY_MAX_MS)
  }
}

/** Stop the turn running in this session, whoever started it. */
export function abortSession(sessionId: string): Promise<unknown> {
  return platformRequest("POST", `/api/v1/agent-sessions/${enc(sessionId)}/abort`, {}, ACTION)
}

export type PermissionReply = "once" | "always" | "reject"

/** Answer a bot's permission prompt (gateway relays to gizzi `/v1/permission/:id/reply`). */
export function replyPermission(requestId: string, reply: PermissionReply, message?: string): Promise<unknown> {
  return platformRequest(
    "POST",
    `/api/v1/permissions/${enc(requestId)}/reply`,
    { reply, ...(message ? { message } : {}) },
    ACTION,
  )
}

/** Answer a bot's question: one list of chosen labels per question, in order. */
export function replyQuestion(requestId: string, answers: string[][]): Promise<unknown> {
  return platformRequest("POST", `/api/v1/questions/${enc(requestId)}/reply`, { answers }, ACTION)
}

export function rejectQuestion(requestId: string): Promise<unknown> {
  return platformRequest("POST", `/api/v1/questions/${enc(requestId)}/reject`, {}, ACTION)
}
