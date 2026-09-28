import z from "zod/v4"
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { SessionRetry } from "@/runtime/session/retry"
import { Config } from "@/runtime/context/config/config"
import { ProviderQuotas, type QuotaWindow } from "@/runtime/providers/quota"
import { Log } from "@/shared/util/log"

/**
 * Landing before a usage limit (P3.17). A session about to run into a
 * provider's 5-hour / weekly / rate limit — or that just hit one — pauses at
 * a clean point instead of failing, and resumes on its own when the limit
 * resets. Works for any provider gizzi can read a limit from:
 *  - quota windows (`providers/quota`: Kimi, OpenRouter, Codex, …),
 *  - a limit reply from a CLI harness ("You've hit your session limit ·
 *    resets 7:40pm (America/Chicago)"),
 *  - a rate limit with a long retry-after.
 * Switching to another model is never automatic: `resume(…, { model })` is
 * the explicit "Resume now on …".
 */
export namespace SessionPause {
  const log = Log.create({ service: "session.pause" })

  export const DEFAULT_LAND_AT = 0.95
  /** Rate-limit waits shorter than this stay ordinary retries. */
  export const MIN_PAUSE_MS = 2 * 60_000
  /** Resume a little after the stated reset, so the provider has rolled over. */
  const RESET_GRACE_MS = 60_000
  const MAX_TIMER_MS = 2_147_483_647

  export type Paused = NonNullable<Session.Info["paused"]>

  export const Event = {
    Paused: BusEvent.define(
      "session.paused",
      z.object({ sessionID: z.string(), until: z.number(), limit: z.string(), reason: z.string(), providerID: z.string().optional() }),
    ),
    Resumed: BusEvent.define(
      "session.resumed",
      z.object({ sessionID: z.string(), early: z.boolean(), model: z.object({ providerID: z.string(), modelID: z.string() }).optional() }),
    ),
  }

  const timers = new Map<string, ReturnType<typeof setTimeout>>()

  // ── Reading a limit ─────────────────────────────────────────────────────

  /** Offset (ms) of `tz` from UTC at `epoch`. */
  function tzOffset(epoch: number, tz: string): number {
    const parts = new Intl.DateTimeFormat("en-US", {
      timeZone: tz,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }).formatToParts(new Date(epoch))
    const get = (t: string) => Number(parts.find((p) => p.type === t)?.value)
    const asUTC = Date.UTC(get("year"), get("month") - 1, get("day"), get("hour"), get("minute"), get("second"))
    return asUTC - epoch
  }

  /** Wall-clock time in `tz` → epoch ms. */
  function zoned(y: number, mo: number, d: number, h: number, mi: number, tz: string): number {
    const guess = Date.UTC(y, mo, d, h, mi)
    return guess - tzOffset(guess - tzOffset(guess, tz), tz)
  }

  function validTz(tz: string | undefined): string | undefined {
    if (!tz) return
    try {
      new Intl.DateTimeFormat("en-US", { timeZone: tz })
      return tz
    } catch {
      return
    }
  }

  const MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]

  /**
   * A limit stated in a reply: "…limit · resets 7:40pm (America/Chicago)",
   * "…resets Oct 3, 9am (UTC)", "Claude AI usage limit reached|1759012345",
   * "try again in 2 hours 13 minutes". Returns when it resets.
   */
  export function limitFromText(text: string, now = Date.now()): { until: number; limit: string } | undefined {
    const t = text.replace(/\s+/g, " ").trim()
    if (!/(limit|quota)/i.test(t)) return
    const label = /weekly|7-day|week/i.test(t)
      ? "weekly limit"
      : /5-hour|five-hour|session limit|5h/i.test(t)
        ? "5-hour limit"
        : /rate/i.test(t)
          ? "rate limit"
          : "usage limit"

    const epoch = t.match(/limit reached\|(\d{10,13})/i)
    if (epoch) {
      const n = Number(epoch[1])
      return { until: n < 1e12 ? n * 1000 : n, limit: label }
    }

    const inRel = t.match(/(?:try again|resets?) in\s+(?:(\d+)\s*h(?:ours?|rs?)?)?[\s,]*(?:(\d+)\s*m(?:in(?:ute)?s?)?)?/i)
    if (inRel && (inRel[1] || inRel[2])) {
      return { until: now + (Number(inRel[1] ?? 0) * 60 + Number(inRel[2] ?? 0)) * 60_000, limit: label }
    }

    const at = t.match(
      /(?:resets?|try again)(?: at| on)?\s+(?:([A-Za-z]{3})[a-z]*\.? (\d{1,2}),?\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?(?:\s*\(([^)]+)\))?/i,
    )
    if (!at) return
    const tz = validTz(at[6]?.trim()) ?? Intl.DateTimeFormat().resolvedOptions().timeZone
    let hour = Number(at[3])
    const minute = Number(at[4] ?? 0)
    const meridiem = at[5]?.toLowerCase()
    if (meridiem === "pm" && hour < 12) hour += 12
    if (meridiem === "am" && hour === 12) hour = 0
    if (hour > 23 || minute > 59) return

    const off = tzOffset(now, tz)
    const local = new Date(now + off)
    let y = local.getUTCFullYear()
    let mo = local.getUTCMonth()
    let d = local.getUTCDate()
    if (at[1] && at[2]) {
      const m = MONTHS.indexOf(at[1].toLowerCase())
      if (m >= 0) {
        mo = m
        d = Number(at[2])
        if (mo < local.getUTCMonth() - 6) y += 1
      }
    }
    let until = zoned(y, mo, d, hour, minute, tz)
    if (!at[1] && until <= now) until = zoned(y, mo, d + 1, hour, minute, tz)
    return until > now ? { until, limit: label } : undefined
  }

  async function landAt(): Promise<number> {
    const cfg = await Config.get()
    return cfg.limits?.land_at ?? DEFAULT_LAND_AT
  }

  /** A provider window that is (nearly) used up, with when it resets. */
  export async function quotaLanding(providerID: string): Promise<{ until: number; limit: string } | undefined> {
    const result = await ProviderQuotas.get(providerID).catch(() => undefined)
    if (result?.status !== "ok") return
    return windowsLanding(result.quota.source, result.quota.windows, await landAt())
  }

  export function windowsLanding(source: string, windows: QuotaWindow[], threshold: number, now = Date.now()) {
    const hit = windows
      .filter((w) => w.usedRatio >= threshold && w.resetAt && Date.parse(w.resetAt) > now)
      .sort((a, b) => Date.parse(b.resetAt!) - Date.parse(a.resetAt!))[0]
    if (!hit) return
    return { until: Date.parse(hit.resetAt!), limit: `${source} ${hit.label.toLowerCase()} limit` }
  }

  /** A finished turn that ran into a limit: its reply or its error. */
  export function limitFromTurn(message: MessageV2.WithParts | undefined, now = Date.now()) {
    const info = message?.info
    if (!info || info.role !== "assistant") return
    const text = message!.parts
      .filter((p): p is MessageV2.TextPart => p.type === "text")
      .map((p) => p.text)
      .join(" ")
    const stated = text.length < 600 ? limitFromText(text, now) : undefined
    if (stated) return { ...stated, reason: "limit_hit" as const }
    const err = info.error
    if (err && MessageV2.APIError.isInstance(err)) {
      const msg = `${err.data.message ?? ""} ${String(err.data.responseBody ?? "")}`
      const fromBody = limitFromText(msg, now)
      if (fromBody) return { ...fromBody, reason: "limit_hit" as const }
      if (/rate.?limit|429|too many requests/i.test(msg) || err.data.statusCode === 429) {
        const wait = SessionRetry.delay(1, err)
        if (wait >= MIN_PAUSE_MS) return { until: now + wait, limit: "rate limit", reason: "rate_limit" as const }
      }
    }
    return
  }

  // ── Pausing and resuming ────────────────────────────────────────────────

  export function isPaused(session: Session.Info, now = Date.now()): boolean {
    return Boolean(session.paused && session.paused.until > now)
  }

  export function pause(sessionID: string, p: Omit<Paused, "at">) {
    const paused: Paused = { ...p, at: Date.now() }
    Session.setPaused({ sessionID, paused })
    schedule(sessionID, paused)
    Bus.publish(Event.Paused, { sessionID, until: paused.until, limit: paused.limit, reason: paused.reason, providerID: paused.providerID })
    log.info("paused before a limit", { sessionID, until: new Date(paused.until).toISOString(), limit: paused.limit })
    return paused
  }

  function schedule(sessionID: string, paused: Paused) {
    clearTimeout(timers.get(sessionID))
    const wait = Math.max(0, paused.until + RESET_GRACE_MS - Date.now())
    const timer = setTimeout(() => void wake(sessionID), Math.min(wait, MAX_TIMER_MS))
    ;(timer as { unref?: () => void }).unref?.()
    timers.set(sessionID, timer)
  }

  /** Timer fired: check the limit really reset, then resume. */
  async function wake(sessionID: string) {
    timers.delete(sessionID)
    if ((await Config.get()).limits?.auto_resume === false) return
    const session = await Session.get(sessionID).catch(() => undefined)
    if (!session?.paused) return
    if (session.paused.until + RESET_GRACE_MS > Date.now()) return schedule(sessionID, session.paused)
    if (session.paused.providerID) {
      const still = await quotaLanding(session.paused.providerID)
      if (still && still.until > Date.now()) {
        Session.setPaused({ sessionID, paused: { ...session.paused, until: still.until } })
        return schedule(sessionID, { ...session.paused, until: still.until })
      }
    }
    await resume(sessionID).catch((error) => log.warn("automatic resume failed", { sessionID, error }))
  }

  /**
   * Resume a paused session: run the held turn, or tell the model to carry
   * on. `model` is the explicit "Resume now on …" (early, other provider).
   */
  export async function resume(sessionID: string, opts: { model?: { providerID: string; modelID: string } } = {}) {
    const session = await Session.get(sessionID)
    clearTimeout(timers.get(sessionID))
    timers.delete(sessionID)
    const early = Boolean(session.paused && session.paused.until > Date.now())
    if (session.paused) Session.setPaused({ sessionID, paused: null })
    Bus.publish(Event.Resumed, { sessionID, early, ...(opts.model ? { model: opts.model } : {}) })

    const msgs = await MessageV2.filterCompacted(MessageV2.stream(sessionID))
    const last = msgs.at(-1)
    const lastUser = msgs.findLast((m) => m.info.role === "user")?.info as MessageV2.User | undefined
    if (!lastUser) return
    const { SessionPrompt } = await import("@/runtime/session/prompt")
    const model = opts.model ?? lastUser.model
    if (last?.info.role === "user" && !opts.model) {
      // The held turn never ran: run it now.
      return SessionPrompt.loop({ sessionID })
    }
    return SessionPrompt.prompt({
      sessionID,
      agent: lastUser.agent,
      model,
      parts: [
        {
          type: "text",
          synthetic: true,
          text: early
            ? `Continuing on ${model.providerID}/${model.modelID} before the limit reset. Carry on where you left off; don't repeat finished work.`
            : "The usage limit has reset. Continue where you left off; don't repeat finished work.",
        },
      ],
    })
  }

  /** Re-arm timers for sessions that were paused before gizzi restarted. */
  export function restore() {
    for (const session of Session.listPaused()) {
      if (session.paused) schedule(session.id, session.paused)
    }
  }

  /** Tests only. */
  export function clearTimers() {
    for (const t of timers.values()) clearTimeout(t)
    timers.clear()
  }

  export function scheduled(sessionID: string): boolean {
    return timers.has(sessionID)
  }
}
