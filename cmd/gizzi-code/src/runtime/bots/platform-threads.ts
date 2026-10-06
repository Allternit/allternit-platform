/**
 * Client for the platform's durable bot threads (`/api/v1/threads`, server
 * `cmd/allternit-api/src/thread_routes.rs`, spec BOT_THREAD_PARITY_SPEC.md
 * P3.1–P3.4). One thread object for every surface: Desktop's Threads panel,
 * the TUI pet HUD, and `gizzi agents bot threads` all read and steer the same rows.
 *
 * A thread spans context generations, each backed by one gizzi session
 * (`currentSessionId`). A turn is posted to the current session through
 * `/api/v1/agent-sessions/:id/messages`; afterwards the caller re-reads the
 * thread and follows `currentSessionId`, because a handoff may have moved it
 * to a fresh generation. Types mirror allternit-ai `src/lib/bots/threads.ts`.
 */
import { platformRequest, type PlatformRequestOptions } from "@/runtime/bots/platform-api"

export type ThreadStatus =
  | "queued"
  | "planning"
  | "working"
  | "blocked"
  | "needs_you"
  | "review"
  | "done"
  | "failed"
  | "paused"
  | "idle"

export interface PlatformThread {
  id: string
  botId: string
  projectId: string | null
  kind: "standing" | "task"
  incognito: boolean
  title: string
  status: ThreadStatus
  currentSessionId: string | null
  generation: number
  /** Share of the current context window in use (0–1). */
  contextUsed: number | null
  lastActivityAt: string
  createdAt: string
}

export interface CreateThreadInput {
  botId: string
  title: string
  kind?: "standing" | "task"
  incognito?: boolean
  createdBy?: string
}

export interface ThreadMessage {
  id: string
  role: "user" | "assistant" | string
  content: string
  timestamp?: string
  metadata?: {
    handoff?: unknown
    /** Raw gizzi parts; the handoff seed's text part carries `metadata.handoff`. */
    parts?: Array<{ type?: string; metadata?: { handoff?: unknown } | null }>
  }
}

/** Where a session continued from, as gizzi tags the seed of a fresh window (spec P3.16). */
export interface HandoffMeta {
  from: string
  generation?: number
  reason?: string
}

function asHandoff(value: unknown): HandoffMeta | null {
  const h = value as Partial<HandoffMeta> | null | undefined
  return h && typeof h.from === "string" ? (h as HandoffMeta) : null
}

/**
 * The handoff a message seeds, or null. The seed is the checkpoint gizzi
 * writes as the first user message of the new window; surfaces hide it and
 * draw the rip in its place.
 */
export function messageHandoff(message: ThreadMessage): HandoffMeta | null {
  const direct = asHandoff(message.metadata?.handoff)
  if (direct) return direct
  for (const part of message.metadata?.parts ?? []) {
    const h = asHandoff(part?.metadata?.handoff)
    if (h) return h
  }
  return null
}

/** A reply from `POST /agent-sessions/:id/messages` (API `transform_message`). */
export interface ThreadTurnReply extends Omit<ThreadMessage, "metadata"> {
  metadata?: ThreadMessage["metadata"] & {
    error?: unknown
    telemetry?: { usage?: { inputTokens?: number; outputTokens?: number; cacheReadTokens?: number; cacheWriteTokens?: number } }
  }
}

export interface ModelRef {
  providerID: string
  modelID: string
}

const LOOKUP: PlatformRequestOptions = { timeoutMs: 10_000 }
const enc = encodeURIComponent

export const threadApi = {
  list: (q: { botId?: string; includeIncognito?: boolean } = {}, options = LOOKUP) => {
    const p = new URLSearchParams()
    if (q.botId) p.set("botId", q.botId)
    if (q.includeIncognito) p.set("includeIncognito", "true")
    const qs = p.toString()
    return platformRequest<{ threads: PlatformThread[] }>("GET", `/api/v1/threads${qs ? `?${qs}` : ""}`, undefined, options).then(
      (r) => r.threads,
    )
  },
  get: (id: string, options = LOOKUP) =>
    platformRequest<{ thread: PlatformThread }>("GET", `/api/v1/threads/${enc(id)}`, undefined, options).then((r) => r.thread),
  create: (input: CreateThreadInput, options = LOOKUP) =>
    platformRequest<{ thread: PlatformThread }>("POST", "/api/v1/threads", input, options).then((r) => r.thread),
  resolve: (id: string, status: "done" | "failed", options = LOOKUP) =>
    platformRequest<{ thread: PlatformThread }>("POST", `/api/v1/threads/${enc(id)}/resolve`, { status }, options).then(
      (r) => r.thread,
    ),
  usage: (id: string, body: { tokensUsed: number; contextWindow?: number; model?: string }, options = LOOKUP) =>
    platformRequest<{ contextUsed: number | null; shouldHandoff: boolean; reason: string | null; handedOff?: boolean }>(
      "POST",
      `/api/v1/threads/${enc(id)}/usage`,
      body,
      options,
    ),
  handoff: (id: string, body: { summary: string; reason?: string; model?: string; contextWindow?: number }, options = LOOKUP) =>
    platformRequest<{ thread: PlatformThread }>("POST", `/api/v1/threads/${enc(id)}/handoff`, body, options).then(
      (r) => r.thread,
    ),
}

/**
 * The bot's standing thread: its long-lived main line of work (a bot's
 * canonical chat becomes one the first time threads are listed). Picks the
 * most recently active standing thread; creates one when the bot has none.
 */
export async function ensureStandingThread(botId: string, title: string): Promise<PlatformThread> {
  const standing = (await threadApi.list({ botId }))
    .filter((t) => t.kind === "standing" && !t.incognito)
    .sort((a, b) => b.lastActivityAt.localeCompare(a.lastActivityAt))
  if (standing[0]) return standing[0]
  return threadApi.create({ botId, title, kind: "standing", createdBy: "user" })
}

/** An incognito ask (spec D3): hidden from lists, no ledger trail, same call Desktop uses. */
export function createIncognitoThread(botId: string, title: string): Promise<PlatformThread> {
  return threadApi.create({ botId, title, kind: "task", incognito: true, createdBy: "user" })
}

export function listSessionMessages(sessionId: string, options = LOOKUP): Promise<ThreadMessage[]> {
  return platformRequest<ThreadMessage[]>("GET", `/api/v1/agent-sessions/${enc(sessionId)}/messages`, undefined, options)
}

/** Parse "provider/model" (the agent record's model) into the API's model ref. */
export function parseModelRef(model: string | null | undefined): ModelRef | undefined {
  if (!model) return undefined
  const slash = model.indexOf("/")
  if (slash <= 0 || slash === model.length - 1) return undefined
  return { providerID: model.slice(0, slash), modelID: model.slice(slash + 1) }
}

/**
 * Post one user turn into the thread's current generation and wait for the
 * bot's reply. Runs as the bot: its model is sent, and the session carries the
 * bot's identity. `source: "terminal"` labels where it was typed.
 */
export async function sendThreadTurn(
  thread: PlatformThread,
  text: string,
  options: { model?: ModelRef; signal?: AbortSignal } = {},
): Promise<ThreadTurnReply> {
  if (!thread.currentSessionId) throw new Error(`Thread ${thread.id} has no live session`)
  return platformRequest<ThreadTurnReply>(
    "POST",
    `/api/v1/agent-sessions/${enc(thread.currentSessionId)}/messages`,
    { text, source: "terminal", ...(options.model ? { metadata: { model: options.model } } : {}) },
    { signal: options.signal, timeoutMs: 15 * 60_000 },
  )
}

/** Context tokens a turn left in the window (what the handoff budget measures). */
export function turnContextTokens(reply: ThreadTurnReply): number {
  const u = reply.metadata?.telemetry?.usage
  if (!u) return 0
  return (u.inputTokens ?? 0) + (u.cacheReadTokens ?? 0) + (u.cacheWriteTokens ?? 0) + (u.outputTokens ?? 0)
}

/**
 * After a turn: report usage and hand off if the server says the window is
 * full, then re-read the thread so the caller follows `currentSessionId` to
 * the newest generation. Handoff failures never lose the turn — the thread
 * as last read is returned.
 */
export async function followThread(
  thread: PlatformThread,
  usage: { tokensUsed: number; contextWindow?: number; model?: string },
): Promise<PlatformThread> {
  try {
    if (usage.tokensUsed > 0) {
      const verdict = await threadApi.usage(thread.id, usage)
      if (verdict.shouldHandoff && !verdict.handedOff) {
        return await threadApi.handoff(thread.id, {
          summary: "",
          reason: verdict.reason ?? "budget",
          model: usage.model,
          contextWindow: usage.contextWindow,
        })
      }
    }
    return await threadApi.get(thread.id)
  } catch {
    return thread
  }
}
