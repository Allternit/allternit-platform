/**
 * Recurring schedules for the `automation` tool: a small RFC 5545 RRULE
 * subset turned into a 5-field cron line, plus checks on cron lines.
 *
 * Cron lines are read in the automation's time zone by both schedulers
 * (allternit-api routine_local_scheduler.rs via cron_lite, and the runtime
 * /cron). cron_lite takes numbers only (no JAN/MON names, no L/W/#) and 7 for
 * Sunday, so this file accepts exactly that.
 *
 * Only rules cron can express exactly are accepted. Anything else is refused
 * with the list of supported forms (RRuleError), never approximated.
 *
 * Free of runtime state so the Ink adapter and bun tests can use it.
 */
import { Clock } from "@/runtime/primitives/clock"

export namespace RRule {
  /** Shortest gap allowed between two runs of a recurring automation. */
  export const MIN_INTERVAL_MINUTES = 15

  export type ErrorCode = "invalid" | "unsupported" | "too_frequent"

  export class RRuleError extends Error {
    constructor(
      readonly code: ErrorCode,
      message: string,
    ) {
      super(message)
      this.name = "RRuleError"
    }
  }

  export const SUPPORTED_FORMS = [
    "FREQ=MINUTELY;INTERVAL=n (n divides 60 and is at least 15), optional BYHOUR, BYDAY",
    "FREQ=HOURLY[;INTERVAL=n (n divides 24)][;BYMINUTE=m], optional BYHOUR (without INTERVAL), BYDAY",
    "FREQ=DAILY;BYHOUR=h[;BYMINUTE=m], optional BYDAY",
    "FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=h[;BYMINUTE=m]",
    "FREQ=MONTHLY[;INTERVAL=n (n divides 12)];BYMONTHDAY=d (1-31) or BYDAY=MO..SU (no ordinals);BYHOUR=h[;BYMINUTE=m]",
    "any of these with COUNT=n (number of runs) or UNTIL=YYYYMMDD[THHMMSS[Z]] (not both)",
  ]

  function unsupported(why: string): RRuleError {
    return new RRuleError(
      "unsupported",
      `${why} Supported rrule forms:\n- ${SUPPORTED_FORMS.join("\n- ")}\nOr pass a 5-field cron line instead.`,
    )
  }

  export type Converted = {
    /** 5-field cron line, read in the automation's time zone. */
    cron: string
    /** From COUNT: stop after this many runs. */
    maxRuns?: number
    /** From UNTIL: no run after this instant (ISO 8601, UTC). */
    until?: string
  }

  const DAYS: Record<string, number> = { SU: 0, MO: 1, TU: 2, WE: 3, TH: 4, FR: 5, SA: 6 }
  const KEYS = new Set(["FREQ", "INTERVAL", "BYDAY", "BYHOUR", "BYMINUTE", "BYMONTHDAY", "COUNT", "UNTIL"])
  const FREQS = new Set(["MINUTELY", "HOURLY", "DAILY", "WEEKLY", "MONTHLY"])

  function intList(key: string, value: string, min: number, max: number): number[] {
    const out: number[] = []
    for (const raw of value.split(",")) {
      const item = raw.trim()
      if (!/^-?\d+$/.test(item)) throw new RRuleError("invalid", `${key}=${value}: "${item}" is not a number.`)
      const n = Number(item)
      if (n < 0 && key === "BYMONTHDAY") throw unsupported(`BYMONTHDAY=${item}: counting from the end of the month is not supported.`)
      if (n < min || n > max) throw new RRuleError("invalid", `${key}=${value}: ${n} is outside ${min}-${max}.`)
      if (!out.includes(n)) out.push(n)
    }
    return out.sort((a, b) => a - b)
  }

  function dayList(value: string): number[] {
    const out: number[] = []
    for (const raw of value.split(",")) {
      const item = raw.trim().toUpperCase()
      if (/^[+-]?\d+[A-Z]{2}$/.test(item)) {
        throw unsupported(`BYDAY=${value}: weekdays with a position ("1MO", "-1FR") are not supported.`)
      }
      const day = DAYS[item]
      if (day === undefined) throw new RRuleError("invalid", `BYDAY=${value}: "${raw.trim()}" is not one of MO, TU, WE, TH, FR, SA, SU.`)
      if (!out.includes(day)) out.push(day)
    }
    return out.sort((a, b) => a - b)
  }

  function positive(key: string, value: string): number {
    if (!/^\d+$/.test(value) || Number(value) < 1) throw new RRuleError("invalid", `${key}=${value} must be a whole number of at least 1.`)
    return Number(value)
  }

  /**
   * UNTIL as an instant. A date alone means the end of that day, and a time
   * without Z is local time; both are read in `timezone`.
   */
  function untilInstant(value: string, timezone: string): string {
    const m = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})(Z)?)?$/.exec(value)
    if (!m) throw new RRuleError("invalid", `UNTIL=${value} must look like 20261231, 20261231T235900 or 20261231T235900Z.`)
    const [, y, mo, d, h, mi, s, z] = m
    const local = `${y}-${mo}-${d}T${h ?? "23"}:${mi ?? "59"}`
    if (z) {
      const at = new Date(`${y}-${mo}-${d}T${h}:${mi}:${s}Z`)
      if (Number.isNaN(at.getTime()) || at.toISOString().slice(0, 10) !== `${y}-${mo}-${d}`) {
        throw new RRuleError("invalid", `UNTIL=${value} is not a real date.`)
      }
      return at.toISOString()
    }
    const instant = localToInstant(local, timezone)
    if ("error" in instant) throw new RRuleError("invalid", `UNTIL=${value}: ${instant.error}`)
    const seconds = h ? Number(s) : 59
    return new Date(instant.at.getTime() + seconds * 1000).toISOString()
  }

  const list = (values: number[] | undefined) => (values && values.length ? values.join(",") : "*")

  /**
   * Convert an RRULE (with or without the "RRULE:" prefix) to cron. `timezone`
   * reads a floating UNTIL. Throws RRuleError: `invalid` for a malformed rule,
   * `unsupported` for a valid rule cron can't express, `too_frequent` when two
   * runs could fall less than MIN_INTERVAL_MINUTES apart.
   */
  export function rruleToCron(rrule: string, timezone = "UTC"): Converted {
    const text = rrule.trim().replace(/^RRULE:/i, "")
    if (!text) throw new RRuleError("invalid", "The rrule is empty.")
    if (/^(DTSTART|EXDATE|RDATE)/im.test(text) || text.includes("\n")) {
      throw unsupported("Only one RRULE line is accepted (no DTSTART, EXDATE or RDATE).")
    }
    const fields = new Map<string, string>()
    for (const part of text.split(";")) {
      if (!part.trim()) continue
      const eq = part.indexOf("=")
      if (eq < 1) throw new RRuleError("invalid", `"${part}" is not KEY=VALUE.`)
      const key = part.slice(0, eq).trim().toUpperCase()
      const value = part.slice(eq + 1).trim()
      if (!KEYS.has(key)) throw unsupported(`${key} is not supported.`)
      if (fields.has(key)) throw new RRuleError("invalid", `${key} appears twice.`)
      fields.set(key, value)
    }
    const freq = fields.get("FREQ")?.toUpperCase()
    if (!freq) throw new RRuleError("invalid", "FREQ is missing.")
    if (!FREQS.has(freq)) throw unsupported(`FREQ=${freq} is not supported.`)

    const interval = fields.has("INTERVAL") ? positive("INTERVAL", fields.get("INTERVAL")!) : 1
    const byDay = fields.has("BYDAY") ? dayList(fields.get("BYDAY")!) : undefined
    const byHour = fields.has("BYHOUR") ? intList("BYHOUR", fields.get("BYHOUR")!, 0, 23) : undefined
    const byMinute = fields.has("BYMINUTE") ? intList("BYMINUTE", fields.get("BYMINUTE")!, 0, 59) : undefined
    const byMonthDay = fields.has("BYMONTHDAY") ? intList("BYMONTHDAY", fields.get("BYMONTHDAY")!, 1, 31) : undefined
    if (byDay && byMonthDay) {
      throw unsupported("BYDAY and BYMONTHDAY together are not supported (cron would run on either, not both).")
    }
    if (fields.has("COUNT") && fields.has("UNTIL")) throw new RRuleError("invalid", "Use COUNT or UNTIL, not both.")

    let minute: string
    let hour: string
    let dom = list(byMonthDay)
    let month = "*"
    const dow = list(byDay)

    switch (freq) {
      case "MINUTELY": {
        if (byMinute) throw unsupported("BYMINUTE with FREQ=MINUTELY is not supported; use FREQ=HOURLY;BYMINUTE=… instead.")
        if (interval < MIN_INTERVAL_MINUTES) {
          throw new RRuleError("too_frequent", `Every ${interval} minute${interval === 1 ? "" : "s"} is too often: recurring scheduled tasks run at most every ${MIN_INTERVAL_MINUTES} minutes.`)
        }
        if (interval >= 60 || 60 % interval !== 0) {
          throw unsupported(`INTERVAL=${interval} with FREQ=MINUTELY must divide 60 (15, 20 or 30); for longer gaps use FREQ=HOURLY.`)
        }
        minute = `*/${interval}`
        hour = list(byHour)
        break
      }
      case "HOURLY": {
        if (interval > 1 && (interval >= 24 || 24 % interval !== 0)) {
          throw unsupported(`INTERVAL=${interval} with FREQ=HOURLY must divide 24 (2, 3, 4, 6, 8 or 12).`)
        }
        if (interval > 1 && byHour) throw unsupported("INTERVAL and BYHOUR together are not supported with FREQ=HOURLY.")
        minute = list(byMinute ?? [0])
        hour = interval > 1 ? `*/${interval}` : list(byHour)
        break
      }
      case "DAILY":
      case "WEEKLY":
      case "MONTHLY": {
        if (!byHour) {
          throw new RRuleError("invalid", `FREQ=${freq} needs BYHOUR (and BYMINUTE, default 0) so the run time is clear.`)
        }
        if (freq === "WEEKLY" && !byDay) throw new RRuleError("invalid", "FREQ=WEEKLY needs BYDAY (for example BYDAY=MO,TH).")
        if (freq === "MONTHLY" && !byDay && !byMonthDay) {
          throw new RRuleError("invalid", "FREQ=MONTHLY needs BYMONTHDAY (for example BYMONTHDAY=1) or BYDAY.")
        }
        if (freq === "DAILY" && byMonthDay) throw unsupported("BYMONTHDAY with FREQ=DAILY is not supported; use FREQ=MONTHLY.")
        if (interval > 1) {
          if (freq !== "MONTHLY") {
            throw unsupported(`INTERVAL=${interval} with FREQ=${freq} can't be expressed as a fixed schedule ("every other day/week" is not supported).`)
          }
          if (interval >= 12 || 12 % interval !== 0) {
            throw unsupported(`INTERVAL=${interval} with FREQ=MONTHLY must divide 12 (2, 3, 4 or 6).`)
          }
          month = `1-12/${interval}`
        }
        minute = list(byMinute ?? [0])
        hour = list(byHour)
        break
      }
      default:
        throw unsupported(`FREQ=${freq} is not supported.`)
    }

    const cron = [minute, hour, dom, month, dow].join(" ")
    assertMinInterval(cron)
    const out: Converted = { cron }
    if (fields.has("COUNT")) out.maxRuns = positive("COUNT", fields.get("COUNT")!)
    if (fields.has("UNTIL")) out.until = untilInstant(fields.get("UNTIL")!, timezone)
    return out
  }

  /* ------------------------------------------------------------------------ */
  /* Cron                                                                     */
  /* ------------------------------------------------------------------------ */

  const CRON_FIELDS: Array<{ name: string; min: number; max: number }> = [
    { name: "minute", min: 0, max: 59 },
    { name: "hour", min: 0, max: 23 },
    { name: "day of month", min: 1, max: 31 },
    { name: "month", min: 1, max: 12 },
    { name: "day of week", min: 0, max: 7 },
  ]

  function cronField(value: string, field: { name: string; min: number; max: number }): Set<number> {
    const out = new Set<number>()
    for (const part of value.split(",")) {
      const m = /^(\*|(\d+)(?:-(\d+))?)(?:\/(\d+))?$/.exec(part)
      if (!m) {
        throw new RRuleError(
          "invalid",
          `Cron ${field.name} "${part}" is not valid. Use numbers, ranges (1-5), steps (*/15) and lists (1,15); names like MON or JAN are not accepted.`,
        )
      }
      const step = m[4] === undefined ? 1 : Number(m[4])
      if (step < 1) throw new RRuleError("invalid", `Cron ${field.name} "${part}": the step must be at least 1.`)
      let lo = field.min
      let hi = field.max
      if (m[1] !== "*") {
        lo = Number(m[2])
        hi = m[3] === undefined ? (m[4] === undefined ? lo : field.max) : Number(m[3])
      }
      if (lo < field.min || hi > field.max || lo > hi) {
        throw new RRuleError("invalid", `Cron ${field.name} "${part}" is outside ${field.min}-${field.max}.`)
      }
      for (let n = lo; n <= hi; n += step) out.add(n)
    }
    return out
  }

  export type ParsedCron = { minutes: number[]; hours: number[]; daysOfMonth: number[]; months: number[]; daysOfWeek: number[] }

  /** Parse and range-check a 5-field cron line (cron_lite syntax). Throws RRuleError `invalid`. */
  export function parseCron(cron: string): ParsedCron {
    const parts = cron.trim().split(/\s+/)
    if (parts.length !== 5 || !parts[0]) {
      throw new RRuleError(
        "invalid",
        `A cron line has 5 fields (minute hour day-of-month month day-of-week), for example "0 9 * * 1-5"; got ${parts[0] ? parts.length : 0}.`,
      )
    }
    const sets = parts.map((p, i) => cronField(p, CRON_FIELDS[i]!))
    const sorted = (s: Set<number>) => [...s].sort((a, b) => a - b)
    const dow = sets[4]!
    if (dow.has(7)) {
      dow.delete(7)
      dow.add(0)
    }
    return {
      minutes: sorted(sets[0]!),
      hours: sorted(sets[1]!),
      daysOfMonth: sorted(sets[2]!),
      months: sorted(sets[3]!),
      daysOfWeek: sorted(dow),
    }
  }

  /**
   * The smallest gap in minutes two runs of `cron` could have, computed
   * conservatively: every run time of day is assumed to occur on two days in
   * a row, so the gap across midnight always counts.
   */
  export function smallestGapMinutes(cron: string): number {
    const parsed = parseCron(cron)
    const times: number[] = []
    for (const h of parsed.hours) for (const m of parsed.minutes) times.push(h * 60 + m)
    times.sort((a, b) => a - b)
    if (times.length === 1) return 24 * 60
    let gap = times[0]! + 24 * 60 - times[times.length - 1]!
    for (let i = 1; i < times.length; i++) gap = Math.min(gap, times[i]! - times[i - 1]!)
    return gap
  }

  /** Throws RRuleError `too_frequent` when runs of `cron` could be under MIN_INTERVAL_MINUTES apart. */
  export function assertMinInterval(cron: string): void {
    const gap = smallestGapMinutes(cron)
    if (gap < MIN_INTERVAL_MINUTES) {
      throw new RRuleError(
        "too_frequent",
        `"${cron}" could run ${gap} minute${gap === 1 ? "" : "s"} apart; recurring scheduled tasks run at most every ${MIN_INTERVAL_MINUTES} minutes.`,
      )
    }
  }

  /** Check a cron line the model passed: 5 fields, cron_lite syntax, at least MIN_INTERVAL_MINUTES apart. Returns it normalized (single spaces). */
  export function validateCron(cron: string): string {
    const normalized = cron.trim().split(/\s+/).join(" ")
    if (normalized.startsWith("@")) {
      throw new RRuleError("invalid", `"${normalized}" shortcuts are not accepted; write the 5 fields, for example "0 9 * * *".`)
    }
    parseCron(normalized)
    assertMinInterval(normalized)
    return normalized
  }

  /* ------------------------------------------------------------------------ */
  /* Local wall time → instant                                                */
  /* ------------------------------------------------------------------------ */

  export type Instant = { ok: true; at: Date } | { ok: false; error: string; code: "invalid" | "nonexistent" }

  const LOCAL = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/

  function offsetMinutes(zone: string, at: Date): number {
    const m = /^([+-])(\d{2}):(\d{2})$/.exec(Clock.now(zone, at).utc_offset)
    if (!m) return 0
    const minutes = Number(m[2]) * 60 + Number(m[3])
    return m[1] === "-" ? -minutes : minutes
  }

  /**
   * A local wall time ("YYYY-MM-DDTHH:mm", no offset) in an IANA zone as an
   * instant. A time the clocks skip (spring forward) is `nonexistent`; a time
   * they repeat (fall back) is the first of the two.
   */
  export function localToInstant(local: string, zone: string): Instant {
    const m = LOCAL.exec(local)
    if (!m) {
      return { ok: false, code: "invalid", error: `"${local}" must be local time as YYYY-MM-DDTHH:mm, with no offset or Z (the zone goes in timezone).` }
    }
    const [y, mo, d, h, mi] = m.slice(1).map(Number) as [number, number, number, number, number]
    const wall = Date.UTC(y, mo - 1, d, h, mi)
    const check = new Date(wall)
    if (
      check.getUTCFullYear() !== y ||
      check.getUTCMonth() !== mo - 1 ||
      check.getUTCDate() !== d ||
      check.getUTCHours() !== h ||
      check.getUTCMinutes() !== mi
    ) {
      return { ok: false, code: "invalid", error: `"${local}" is not a real date and time.` }
    }
    const day = 24 * 60 * 60 * 1000
    const offsets = new Set([-day, 0, day].map((shift) => offsetMinutes(zone, new Date(wall + shift))))
    const matches: number[] = []
    for (const offset of offsets) {
      const at = wall - offset * 60_000
      if (Clock.now(zone, new Date(at)).iso.slice(0, 16) === local) matches.push(at)
    }
    if (!matches.length) {
      return {
        ok: false,
        code: "nonexistent",
        error: `${local} doesn't exist in ${zone}: the clocks skip it for daylight saving time. Ask the user for another time (for example an hour later).`,
      }
    }
    return { ok: true, at: new Date(Math.min(...matches)) }
  }
}

export const rruleToCron = RRule.rruleToCron
export const RRuleError = RRule.RRuleError
