/**
 * A session against its usage limit (spec P3.17 + wrap-up). gizzi warns as a
 * window nears its limit, wraps a running turn up at a clean point once it
 * crosses `limits.land_at`, and resumes on its own when the limit resets;
 * resuming sooner on another model is always an explicit choice.
 *
 * Terminal twin of allternit-ai `src/lib/agents/session-pause.ts` (the web
 * composer's limit strip): the same copy — "Approaching usage limit",
 * "Usage limit reached · Wrapping up", "Wrapped up past your usage limit",
 * "Paused until 7:40 PM · Claude 5-hour limit · resumes on its own" — so the
 * pet HUD, the REPL and Desktop read alike.
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

/** `metadata.limit` — where the session stands against its usage limit. */
export interface SessionLimitState {
  state: "ok" | "approaching" | "wrapping_up" | "wrapped" | "paused"
  providerID: string
  windowID: string
  /** e.g. "Claude 5-hour limit", "Allternit Cloud monthly limit". */
  label: string
  /** 0–1 share of the window used. */
  usedRatio: number
  /** Epoch ms when the window resets. */
  resetAt?: number
  at: number
}

/** What a surface shows: the limit state plus the pause that owns resuming. */
export interface SessionLimitView {
  state: Exclude<SessionLimitState["state"], "ok">
  limit: SessionLimitState | null
  paused: SessionPaused | null
}

export function limitStateOf(metadata: unknown, now = Date.now()): SessionLimitState | null {
  const l = (metadata as { limit?: Partial<SessionLimitState> | null } | null | undefined)?.limit
  if (!l || typeof l.state !== "string" || l.state === "ok" || typeof l.label !== "string") return null
  // A reset window no longer binds, whatever state was last published.
  if (typeof l.resetAt === "number" && l.resetAt <= now && l.state !== "wrapping_up") return null
  return l as SessionLimitState
}

/** The limit view in a session's metadata, or null when nothing needs showing. */
export function limitViewOf(metadata: unknown, now = Date.now()): SessionLimitView | null {
  const paused = pausedOf(metadata, now)
  const limit = limitStateOf(metadata, now)
  if (paused) return { state: limit?.state === "wrapped" ? "wrapped" : "paused", limit, paused }
  if (!limit || (limit.state !== "approaching" && limit.state !== "wrapping_up")) return null
  return { state: limit.state, limit, paused: null }
}

function resetsAt(view: SessionLimitView, now: number): string {
  const at = view.paused?.until ?? view.limit?.resetAt
  return at ? ` · Resets at ${untilLabel(at, now)}` : ""
}

/**
 * One line per state, matching the web composer strip:
 * "Approaching usage limit · 84% of Claude 5-hour limit · Resets at 11:55 AM",
 * "Usage limit reached · Wrapping up · Resets at 11:55 AM",
 * "Wrapped up past your usage limit · Resets at 11:55 AM", or the pause line.
 */
export function limitLine(view: SessionLimitView, now = Date.now()): string {
  switch (view.state) {
    case "approaching": {
      const l = view.limit!
      const label = limitLabel({ until: 0, limit: l.label, providerID: l.providerID })
      return `Approaching usage limit · ${Math.round(l.usedRatio * 100)}% of ${label}${resetsAt(view, now)}`
    }
    case "wrapping_up":
      return `Usage limit reached · Wrapping up${resetsAt(view, now)}`
    case "wrapped":
      return `Wrapped up past your usage limit${resetsAt(view, now)}`
    case "paused":
      return view.paused ? pausedLine(view.paused, now) : `Usage limit reached${resetsAt(view, now)}`
  }
}

/** The glyph a terminal surface puts before the line. */
export function limitGlyph(view: SessionLimitView): string {
  return view.state === "approaching" ? "◔ " : view.state === "wrapping_up" ? "⚠ " : view.state === "wrapped" ? "✓ " : "⏸ "
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

/** The platform session's limit view (approaching / wrapping up / wrapped / paused), or null. */
export async function getSessionLimit(sessionId: string, options = LOOKUP): Promise<SessionLimitView | null> {
  const session = await platformRequest<{ metadata?: unknown }>(
    "GET",
    `/api/v1/agent-sessions/${encodeURIComponent(sessionId)}`,
    undefined,
    options,
  )
  return limitViewOf(session?.metadata)
}

/** Resume now. With a model, continue on it ("Resume now on …"), only ever on the user's press. */
export async function resumeSession(sessionId: string, model?: ModelRef, options = LOOKUP): Promise<void> {
  await platformRequest("POST", `/api/v1/agent-sessions/${encodeURIComponent(sessionId)}/resume`, model ? { model } : {}, options)
}
