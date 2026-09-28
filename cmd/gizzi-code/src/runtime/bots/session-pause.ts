/**
 * A session paused before a usage limit (spec P3.17). gizzi lands the turn
 * at a clean point and resumes on its own when the limit resets; resuming
 * sooner on another model is always an explicit choice, never automatic.
 *
 * Terminal twin of allternit-ai `src/lib/agents/session-pause.ts`: the same
 * copy ("Paused until 7:40 PM · Claude 5-hour limit · resumes on its own")
 * so the pet HUD, the REPL and Desktop read alike.
 */
import { platformRequest, type PlatformRequestOptions } from "@/runtime/bots/platform-api"
import type { ModelRef } from "@/runtime/bots/platform-threads"

export interface SessionPaused {
  /** Epoch ms when the limit resets. */
  until: number
  /** e.g. "5-hour limit", "Kimi For Coding weekly limit", "rate limit". */
  limit: string
  providerID?: string
  reason?: "quota" | "rate_limit" | "limit_hit" | string
}

/** The pause carried in a session's metadata, if it is still in effect. */
export function pausedOf(metadata: unknown, now = Date.now()): SessionPaused | null {
  const p = (metadata as { paused?: Partial<SessionPaused> | null } | null | undefined)?.paused
  if (!p || typeof p.until !== "number" || typeof p.limit !== "string") return null
  return p.until > now ? (p as SessionPaused) : null
}

/** "7:40 PM" today, "Sat 9:00 AM" this week, else "Oct 3, 9:00 AM". */
export function untilLabel(until: number, now = Date.now()): string {
  const t = new Date(until)
  const n = new Date(now)
  const time = t.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
  if (t.toDateString() === n.toDateString()) return time
  if (until - now < 6 * 86_400_000) return `${t.toLocaleDateString([], { weekday: "short" })} ${time}`
  return `${t.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`
}

/** The limit with its provider named and capitalized: "Claude 5-hour limit". */
export function limitLabel(p: SessionPaused): string {
  const text = p.limit.trim()
  const provider = p.providerID?.replace(/-cli$/, "")
  const named =
    provider && !text.toLowerCase().includes(provider.toLowerCase())
      ? `${provider.charAt(0).toUpperCase()}${provider.slice(1)} ${text}`
      : text
  return named.charAt(0).toUpperCase() + named.slice(1)
}

/** One line for any surface: "Paused until 7:40 PM · Claude 5-hour limit · resumes on its own". */
export function pausedLine(p: SessionPaused, now = Date.now()): string {
  return `Paused until ${untilLabel(p.until, now)} · ${limitLabel(p)} · resumes on its own`
}

const LOOKUP: PlatformRequestOptions = { timeoutMs: 10_000 }

/** The platform session's pause, or null when it isn't paused. */
export async function getSessionPaused(sessionId: string, options = LOOKUP): Promise<SessionPaused | null> {
  const session = await platformRequest<{ metadata?: unknown }>(
    "GET",
    `/api/v1/agent-sessions/${encodeURIComponent(sessionId)}`,
    undefined,
    options,
  )
  return pausedOf(session?.metadata)
}

/** Resume now. With a model, continue on it ("Resume now on …"), only ever on the user's press. */
export async function resumeSession(sessionId: string, model?: ModelRef, options = LOOKUP): Promise<void> {
  await platformRequest("POST", `/api/v1/agent-sessions/${encodeURIComponent(sessionId)}/resume`, model ? { model } : {}, options)
}
