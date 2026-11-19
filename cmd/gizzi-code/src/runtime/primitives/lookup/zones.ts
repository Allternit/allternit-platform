/**
 * lookup kind `time`: the same moment across zones, with local maths over the
 * IANA database Intl ships (no network). "now" uses ctx.now and the turn's
 * zone (ctx.timeZone); a reference like "9am Chicago" converts that moment.
 * Day shifts and DST come from Intl, never hand-written offsets.
 */
import { Clock } from "../clock"
import { LIMITS, LookupError, type LookupContext, type LookupResult, type ZonesParams } from "./types"

const CREDIT = "Time zones: IANA database via Intl, computed on this device"

/** Common place names to IANA zones; anything else must be an IANA name. */
const PLACES: Record<string, string> = {
  "new york": "America/New_York", nyc: "America/New_York", boston: "America/New_York", miami: "America/New_York", toronto: "America/Toronto",
  chicago: "America/Chicago", dallas: "America/Chicago", houston: "America/Chicago", minneapolis: "America/Chicago", "mexico city": "America/Mexico_City",
  denver: "America/Denver", phoenix: "America/Phoenix", "los angeles": "America/Los_Angeles", la: "America/Los_Angeles",
  "san francisco": "America/Los_Angeles", seattle: "America/Los_Angeles", vancouver: "America/Vancouver", anchorage: "America/Anchorage",
  honolulu: "Pacific/Honolulu", "sao paulo": "America/Sao_Paulo", "buenos aires": "America/Argentina/Buenos_Aires", bogota: "America/Bogota",
  london: "Europe/London", dublin: "Europe/Dublin", lisbon: "Europe/Lisbon", paris: "Europe/Paris", berlin: "Europe/Berlin",
  madrid: "Europe/Madrid", rome: "Europe/Rome", amsterdam: "Europe/Amsterdam", zurich: "Europe/Zurich", stockholm: "Europe/Stockholm",
  athens: "Europe/Athens", istanbul: "Europe/Istanbul", moscow: "Europe/Moscow", cairo: "Africa/Cairo", lagos: "Africa/Lagos",
  nairobi: "Africa/Nairobi", johannesburg: "Africa/Johannesburg", dubai: "Asia/Dubai", karachi: "Asia/Karachi", mumbai: "Asia/Kolkata",
  delhi: "Asia/Kolkata", kolkata: "Asia/Kolkata", dhaka: "Asia/Dhaka", bangkok: "Asia/Bangkok", singapore: "Asia/Singapore",
  "hong kong": "Asia/Hong_Kong", shanghai: "Asia/Shanghai", beijing: "Asia/Shanghai", taipei: "Asia/Taipei", seoul: "Asia/Seoul",
  tokyo: "Asia/Tokyo", manila: "Asia/Manila", jakarta: "Asia/Jakarta", sydney: "Australia/Sydney", melbourne: "Australia/Melbourne",
  perth: "Australia/Perth", auckland: "Pacific/Auckland", utc: "UTC", gmt: "UTC",
}

/** Map a place or IANA name to a zone; undefined when unknown. */
export function zoneFor(name: string): string | undefined {
  const raw = name.trim()
  const direct = Clock.validZone(raw.replace(/\s+/g, "_"))
  if (direct && raw.includes("/")) return direct
  const key = raw.toLowerCase().replace(/\s+/g, " ")
  return PLACES[key] ?? (key === "utc" ? "UTC" : undefined)
}

function offsetMinutes(zone: string, at: Date): number {
  const p = wallParts(zone, at)
  const asUtc = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second)
  return Math.round((asUtc - Math.floor(at.getTime() / 1000) * 1000) / 60000)
}

function wallParts(zone: string, at: Date) {
  const f = new Intl.DateTimeFormat("en-US", {
    timeZone: zone, hourCycle: "h23", year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric", second: "numeric",
  })
  const o: Record<string, number> = {}
  for (const part of f.formatToParts(at)) if (part.type !== "literal") o[part.type] = Number(part.value)
  return { year: o.year!, month: o.month!, day: o.day!, hour: o.hour!, minute: o.minute!, second: o.second! }
}

function fmtOffset(min: number): string {
  const sign = min < 0 ? "-" : "+"
  const a = Math.abs(min)
  return `UTC${sign}${String(Math.floor(a / 60)).padStart(2, "0")}:${String(a % 60).padStart(2, "0")}`
}

/** The instant whose wall clock in `zone` reads the given fields (DST gaps resolve forward). */
export function instantFromWall(zone: string, y: number, mo: number, d: number, h: number, mi: number): Date {
  const guess = Date.UTC(y, mo - 1, d, h, mi)
  let t = guess - offsetMinutes(zone, new Date(guess)) * 60000
  t = guess - offsetMinutes(zone, new Date(t)) * 60000
  return new Date(t)
}

interface Reference {
  at: Date
  /** Zone the reference was written in, when one was named. */
  zone?: string
  label: string
}

function parseReference(query: string | undefined, ctx: LookupContext): Reference {
  const now = ctx.now?.() ?? new Date()
  const q = (query ?? "").trim()
  if (!q || /^now$/i.test(q)) return { at: now, label: "now" }
  // Optional leading date, a time, then an optional place or zone.
  const m = q.match(/^(?:(\d{4})-(\d{2})-(\d{2})[ T,]*)?(\d{1,2})(?::(\d{2}))?\s*(?:(am|pm)\b)?(?:\s*(Z|UTC)\b)?\s*(?:(?:in|at)\b)?\s*(.*)$/i)
  if (!m || (!m[6] && !m[5] && !m[1])) throw new LookupError("time", `Could not read "${q}" as a time. Use "now", "9am Chicago", or "2026-03-08 14:30 America/New_York".`)
  let hour = Number(m[4])
  const minute = m[5] ? Number(m[5]) : 0
  const mer = m[6]?.toLowerCase()
  if (mer) {
    if (hour < 1 || hour > 12) throw new LookupError("time", `"${q}" has an invalid hour.`)
    hour = (hour % 12) + (mer === "pm" ? 12 : 0)
  }
  if (hour > 23 || minute > 59) throw new LookupError("time", `"${q}" is not a valid time of day.`)
  const place = m[8] ? m[8].trim() : ""
  const zone = m[7] ? "UTC" : place ? zoneFor(place) : (ctx.timeZone && Clock.validZone(ctx.timeZone)) || undefined
  if (place && !zone) throw new LookupError("time", `"${place}" is not a zone this lookup knows. Use an IANA name such as America/Chicago.`)
  const base = zone ?? "UTC"
  let y: number, mo: number, d: number
  if (m[1]) {
    y = Number(m[1]); mo = Number(m[2]); d = Number(m[3])
  } else {
    const p = wallParts(base, now)
    y = p.year; mo = p.month; d = p.day
  }
  const at = instantFromWall(base, y, mo, d, hour, minute)
  return { at, zone: base, label: q }
}

function dateLabel(zone: string, at: Date): string {
  return new Intl.DateTimeFormat("en-US", { timeZone: zone, weekday: "short", year: "numeric", month: "short", day: "numeric" }).format(at)
}

function timeLabel(zone: string, at: Date): string {
  const p = wallParts(zone, at)
  return `${String(p.hour).padStart(2, "0")}:${String(p.minute).padStart(2, "0")}`
}

export async function lookupZones(params: ZonesParams, ctx: LookupContext): Promise<LookupResult> {
  const names = (params.zones ?? []).map((z) => z.trim()).filter(Boolean)
  if (names.length === 0) throw new LookupError("time", "Name at least one zone or place, such as America/Chicago or Tokyo.")
  if (names.length > LIMITS.zonesMax) throw new LookupError("time", `At most ${LIMITS.zonesMax} zones per lookup.`)
  const resolved = names.map((n) => {
    const zone = zoneFor(n)
    if (!zone) throw new LookupError("time", `"${n}" is not a zone this lookup knows. Use an IANA name such as Asia/Tokyo.`)
    return { place: n, zone }
  })
  const ref = parseReference(params.query, ctx)
  const rows = resolved.map(({ place, zone }) => ({
    place,
    zone,
    time: timeLabel(zone, ref.at),
    date: dateLabel(zone, ref.at),
    offset: fmtOffset(offsetMinutes(zone, ref.at)),
  }))
  const refText = ref.label === "now" ? `now (${ref.at.toISOString()})` : `${ref.label} (${ref.at.toISOString()})`
  const text =
    `Same moment, ${refText}:\n` + rows.map((r) => `- ${r.place} (${r.zone}): ${r.time}, ${r.date}, ${r.offset}`).join("\n") + `\n${CREDIT}.`
  return {
    text,
    credit: CREDIT,
    card: {
      version: 1,
      template: "time-zones",
      title: "Time zones",
      summary: rows.map((r) => `${r.place} ${r.time}`).join(" · "),
      data: { reference: refText, zones: rows, credit: CREDIT },
    },
  }
}
