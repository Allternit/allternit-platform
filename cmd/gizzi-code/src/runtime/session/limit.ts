import { Session } from "@/runtime/session"
import { Config } from "@/runtime/context/config/config"
import { ProviderQuotas, type QuotaWindow } from "@/runtime/providers/quota"
import { Log } from "@/shared/util/log"

/**
 * Where a session stands against its model's usage limit — the state behind
 * the "Approaching usage limit" / "Usage limit reached · Wrapping up" /
 * "Wrapped up past your usage limit" strip in every client:
 *
 *   ok → approaching (≥ warn_at) → wrapping_up (≥ land_at mid-turn)
 *      → wrapped (turn finished cleanly, session paused) | paused (held or hit)
 *
 * The share between land_at and the provider's hard wall is the wrap-up
 * budget, for every provider including Allternit Cloud — nothing runs past
 * the limit. Published as `Session.Info.limit` (session.updated), next to
 * `paused` which still owns when the session resumes.
 */
export namespace SessionLimit {
  const log = Log.create({ service: "session.limit" })

  export const DEFAULT_WARN_AT = 0.8
  export const DEFAULT_LAND_AT = 0.95 // same default as SessionPause.DEFAULT_LAND_AT
  export const DEFAULT_WRAP_UP_STEPS = 3

  export type Limit = NonNullable<Session.Info["limit"]>
  export type State = Limit["state"]

  export interface Thresholds {
    warnAt: number
    landAt: number
    wrapUpSteps: number
  }

  export async function thresholds(): Promise<Thresholds> {
    const cfg = (await Config.get()).limits
    const landAt = cfg?.land_at ?? DEFAULT_LAND_AT
    return {
      landAt,
      warnAt: Math.min(cfg?.warn_at ?? DEFAULT_WARN_AT, landAt),
      wrapUpSteps: cfg?.wrap_up_steps ?? DEFAULT_WRAP_UP_STEPS,
    }
  }

  /** Model-scoped windows (Claude's weekly Opus / Sonnet) only bind their model family. */
  export function appliesTo(window: QuotaWindow, modelID: string | undefined): boolean {
    const scoped = window.id.match(/-(opus|sonnet|haiku)$/)
    if (!scoped) return true
    return Boolean(modelID && modelID.toLowerCase().includes(scoped[1]))
  }

  /** The window closest to its limit that still applies to this model. */
  export function tightest(windows: QuotaWindow[], modelID?: string, now = Date.now()): QuotaWindow | undefined {
    return windows
      .filter((w) => appliesTo(w, modelID) && (!w.resetAt || Date.parse(w.resetAt) > now))
      .sort((a, b) => b.usedRatio - a.usedRatio)[0]
  }

  export type Reading =
    | { state: "ok"; window?: QuotaWindow }
    | { state: "approaching"; window: QuotaWindow }
    /** Past land_at with a known reset: time to wrap up and land. */
    | { state: "land"; window: QuotaWindow & { resetAt: string } }

  export function classify(windows: QuotaWindow[], t: Thresholds, modelID?: string, now = Date.now()): Reading {
    // Landing needs a reset time to resume at; with several windows spent,
    // land until the last of them resets.
    const landed = windows
      .filter((w) => appliesTo(w, modelID) && w.usedRatio >= t.landAt && w.resetAt && Date.parse(w.resetAt) > now)
      .sort((a, b) => Date.parse(b.resetAt!) - Date.parse(a.resetAt!))[0]
    if (landed) return { state: "land", window: landed as QuotaWindow & { resetAt: string } }
    const w = tightest(windows, modelID, now)
    if (!w || w.usedRatio < t.warnAt) return { state: "ok", window: w }
    return { state: "approaching", window: w }
  }

  export function snapshot(state: State, providerID: string, window: QuotaWindow | undefined, source?: string, now = Date.now()): Limit {
    const label = window ? `${source && source !== providerID ? `${source} ` : ""}${window.label.toLowerCase()} limit` : "usage limit"
    const resetAt = window?.resetAt ? Date.parse(window.resetAt) : undefined
    return {
      state,
      providerID,
      windowID: window?.id ?? "limit",
      label,
      usedRatio: window?.usedRatio ?? 1,
      ...(resetAt && Number.isFinite(resetAt) ? { resetAt } : {}),
      at: now,
    }
  }

  // Last published value per session: writes only on a real change, so a
  // turn's per-step checks don't flood session.updated.
  const last = new Map<string, Limit | null>()

  function same(a: Limit | null | undefined, b: Limit | null): boolean {
    if (!a || !b) return !a && !b
    return (
      a.state === b.state &&
      a.providerID === b.providerID &&
      a.windowID === b.windowID &&
      a.resetAt === b.resetAt &&
      Math.round(a.usedRatio * 100) === Math.round(b.usedRatio * 100)
    )
  }

  /** Publish the session's limit state (null / ok clears it). */
  export function set(sessionID: string, next: Limit | null, current?: Limit | null) {
    const value = next && next.state === "ok" ? null : next
    const prev = last.has(sessionID) ? last.get(sessionID) : current
    if (same(prev, value)) return
    last.set(sessionID, value)
    try {
      Session.setLimit({ sessionID, limit: value })
    } catch (error) {
      log.warn("failed to publish limit state", { sessionID, error })
    }
  }

  export function clear(sessionID: string) {
    set(sessionID, null)
  }

  /** The limit state for a session that just paused (held, hit, or wrapped up). */
  export function landed(
    sessionID: string,
    p: { until: number; limit: string; providerID?: string; reason: string },
    state: "wrapped" | "paused",
    window?: QuotaWindow,
  ) {
    set(sessionID, {
      state,
      providerID: p.providerID ?? "",
      windowID: window?.id ?? p.reason,
      label: p.limit,
      usedRatio: window?.usedRatio ?? 1,
      resetAt: p.until,
      at: Date.now(),
    })
  }

  /**
   * Read the provider's windows (cached reads + live response headers) and
   * publish approaching / ok. Returns the landing window when the session
   * is past land_at, leaving the wrapping/paused transition to the caller.
   */
  export async function check(
    session: Pick<Session.Info, "id" | "limit">,
    providerID: string,
    modelID?: string,
  ): Promise<(QuotaWindow & { resetAt: string; source: string }) | undefined> {
    const result = await ProviderQuotas.get(providerID).catch(() => undefined)
    if (result?.status !== "ok") return
    const t = await thresholds()
    const reading = classify(result.quota.windows, t, modelID)
    const current = last.has(session.id) ? last.get(session.id) : session.limit
    if (reading.state === "land") return { ...reading.window, source: result.quota.source }
    // Don't downgrade a wrap-up / pause from here; resume and reset clear those.
    if (current && (current.state === "wrapping_up" || current.state === "wrapped" || current.state === "paused")) {
      if (reading.state !== "ok") return
      if (current.resetAt && current.resetAt > Date.now()) return
    }
    set(session.id, snapshot(reading.state, providerID, reading.window, result.quota.source), session.limit)
    return
  }

  /** Tests only. */
  export function reset() {
    last.clear()
  }
}
