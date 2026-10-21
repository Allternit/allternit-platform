/**
 * App-bot routing client (allternit-api `cmd/allternit-api/src/bot_routing.rs`).
 *
 * Bots created in the Allternit app live in allternit-api's `agents` table,
 * not in gizzi's `~/.gizzi/bots` store, so their chats are never gizzi
 * canonical bot sessions. For those chats `message_agent` goes through the
 * API instead: it knows which bot owns the calling session (from the
 * session's metadata, never from model input), who the user's other bots
 * are, and how to start a task thread for the target bot under the caller's
 * thread.
 *
 * Credentials: the internal service token when this runtime has one (cloud
 * and the bundled MCP routes use the same), else the user's platform token
 * (`ALLTERNIT_API_TOKEN` from Desktop, or the `gizzi login` device token).
 * The API checks that the session's bot belongs to that user.
 *
 * Free of CLI/UI imports so bun tests can drive it with a stubbed fetch.
 */
import { platformApiBase, platformToken } from "@/runtime/bots/platform-api"

export interface AppRosterBot {
  id: string
  name: string
  handle?: string | null
  title?: string | null
  tagline?: string | null
  description?: string | null
}

export interface AppRoster {
  caller: { id: string; name: string; threadId?: string | null; projectId?: string | null }
  bots: AppRosterBot[]
  /** One line per teammate, ready for the model. */
  text: string
}

export interface AppRouteResult {
  status: "done" | "started" | "failed"
  threadId: string
  sessionId?: string
  resumed?: boolean
  bot: { id: string; name: string }
  reply?: string
  error?: string
  note?: string
}

export class AppRoutingError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
    this.name = "AppRoutingError"
  }
}

async function headers(): Promise<Record<string, string> | undefined> {
  const internal = process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN?.trim()
  const token = await platformToken().catch(() => undefined)
  if (!internal && !token) return undefined
  return {
    "Content-Type": "application/json",
    Accept: "application/json",
    ...(internal ? { "x-allternit-internal-token": internal } : {}),
    ...(token ? { Authorization: `Bearer ${token}` } : {}),
  }
}

async function post<T>(path: string, body: unknown, timeoutMs: number): Promise<T> {
  const h = await headers()
  if (!h) throw new AppRoutingError(401, "This runtime isn't connected to your Allternit account.")
  const response = await fetch(`${platformApiBase()}/api/v1${path}`, {
    method: "POST",
    headers: h,
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(timeoutMs),
  })
  const text = await response.text().catch(() => "")
  if (!response.ok) {
    let message = text
    try {
      message = (JSON.parse(text) as { error?: string; message?: string }).error ?? message
    } catch {
      // plain-text body
    }
    throw new AppRoutingError(response.status, message || `HTTP ${response.status}`)
  }
  return JSON.parse(text) as T
}

/** The calling app bot and its teammates, or null when the session isn't an app bot's chat. */
export async function fetchAppRoster(sessionID: string, timeoutMs = 3_000): Promise<AppRoster | null> {
  if (!sessionID) return null
  try {
    return await post<AppRoster>("/bot-routing/roster", { sessionId: sessionID }, timeoutMs)
  } catch (error) {
    if (error instanceof AppRoutingError && [401, 403, 404].includes(error.status)) return null
    throw error
  }
}

/** Hand `message` to `target`; waits up to `waitSeconds` for the reply. */
export async function routeToAppBot(
  sessionID: string,
  target: string,
  message: string,
  waitSeconds = 90,
): Promise<AppRouteResult> {
  return post<AppRouteResult>(
    "/bot-routing/message",
    { sessionId: sessionID, target, message, waitSeconds },
    (waitSeconds + 30) * 1000,
  )
}

/** Gating cache: tool assembly runs every turn; the answer rarely changes. */
const TTL_MS = 60_000
const cache = new Map<string, { at: number; ok: boolean }>()

export async function isAppBotSession(sessionID: string): Promise<boolean> {
  if (!sessionID) return false
  const hit = cache.get(sessionID)
  if (hit && Date.now() - hit.at < TTL_MS) return hit.ok
  const ok = (await fetchAppRoster(sessionID, 2_000).catch(() => null)) !== null
  cache.set(sessionID, { at: Date.now(), ok })
  if (cache.size > 500) cache.delete(cache.keys().next().value!)
  return ok
}

/** Tests only. */
export function resetAppBotSessionCache() {
  cache.clear()
}
