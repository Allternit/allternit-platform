/**
 * The user's clock: which time zone the user is in, and the time there.
 *
 * Shared core for the `current_time` tool (runtime builtin and Ink adapter)
 * and the environment section of the system prompt (session/system.ts). The
 * automation primitive reuses the same resolver.
 *
 * Zone priority (first valid one wins; an invalid name falls through):
 *   1. argument     the tool call's `timezone`
 *   2. client       this turn's `metadata.time_zone` (every client sends it)
 *   3. last_client  the last zone any client sent to this runtime, kept in
 *                   `<state>/client-time-zone.json` so it survives restarts
 *                   and covers turns without a client (routines, workers)
 *   4. process      this machine's zone; on a cloud computer it is usually
 *                   not the user's, so callers label it that way
 */
import fs from "fs/promises"
import path from "path"
import { Global } from "@/runtime/context/global"

export namespace Clock {
  export type Source = "argument" | "client" | "last_client" | "process"
  export type Resolved = { zone: string; source: Source }
  export type Now = { iso: string; time_zone: string; utc_offset: string; weekday: string }

  export const STATE_FILE_NAME = "client-time-zone.json"
  /** IANA-shaped names only ("America/Chicago", "UTC", "Etc/GMT+5", "EST5EDT"), never "+05:00" offsets. */
  const IANA_SHAPE = /^[A-Za-z][A-Za-z0-9_]*(\/[A-Za-z0-9_+\-]+){0,2}$/

  export function stateFile(): string {
    return path.join(Global.Path.state, STATE_FILE_NAME)
  }

  /**
   * `value` trimmed when it is a valid IANA zone (at most 64 characters, one
   * Intl knows), else undefined. Kept as sent, not canonicalized: ICU would
   * turn "Asia/Kolkata" into "Asia/Calcutta".
   */
  export function validZone(value: unknown): string | undefined {
    if (typeof value !== "string") return undefined
    const zone = value.trim()
    if (!zone || zone.length > 64 || !IANA_SHAPE.test(zone)) return undefined
    try {
      new Intl.DateTimeFormat("en-US", { timeZone: zone })
      return zone
    } catch {
      return undefined
    }
  }

  export function processZone(): string {
    return validZone(Intl.DateTimeFormat().resolvedOptions().timeZone) ?? "UTC"
  }

  // undefined: not read yet; null: no client has sent a zone.
  let lastClient: string | null | undefined

  async function readLastClient(): Promise<string | undefined> {
    if (lastClient === undefined) {
      const raw = await fs.readFile(stateFile(), "utf8").catch(() => undefined)
      let zone: string | undefined
      try {
        zone = raw ? validZone(JSON.parse(raw)?.zone) : undefined
      } catch {
        zone = undefined
      }
      lastClient = zone ?? null
    }
    return lastClient ?? undefined
  }

  /**
   * Keep the zone a client sent with a turn. Writes the state file only when
   * the zone changes. An invalid or missing value is ignored. Never throws.
   */
  export async function rememberClientZone(value: unknown): Promise<void> {
    const zone = validZone(value)
    if (!zone) return
    if ((await readLastClient()) === zone) return
    lastClient = zone
    try {
      const file = stateFile()
      await fs.mkdir(path.dirname(file), { recursive: true })
      const tmp = `${file}.${process.pid}.tmp`
      await fs.writeFile(tmp, JSON.stringify({ zone, at: new Date().toISOString() }) + "\n")
      await fs.rename(tmp, file)
    } catch {
      // Best effort: the in-memory value still holds for this process.
    }
  }

  /** Forget the in-memory copy so the next read goes to the state file (tests). */
  export function resetCache(): void {
    lastClient = undefined
  }

  /** The zone a turn's user message carries (`metadata.time_zone`), if valid. */
  export function turnZoneOf(metadata: Record<string, unknown> | undefined): string | undefined {
    return validZone(metadata?.["time_zone"])
  }

  /** The zone to use, by the priority in the file header. */
  export async function resolveTimeZone(input: { argument?: string; turnZone?: string } = {}): Promise<Resolved> {
    const argument = validZone(input.argument)
    if (argument) return { zone: argument, source: "argument" }
    const turn = validZone(input.turnZone)
    if (turn) return { zone: turn, source: "client" }
    const last = await readLastClient()
    if (last) return { zone: last, source: "last_client" }
    return { zone: processZone(), source: "process" }
  }

  function parts(zone: string, at: Date) {
    const values: Record<string, string> = {}
    const format = new Intl.DateTimeFormat("en-US", {
      timeZone: zone,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      weekday: "long",
    })
    for (const part of format.formatToParts(at)) values[part.type] = part.value
    return values
  }

  function offset(minutes: number): string {
    const sign = minutes < 0 ? "-" : "+"
    const abs = Math.abs(minutes)
    return `${sign}${String(Math.floor(abs / 60)).padStart(2, "0")}:${String(abs % 60).padStart(2, "0")}`
  }

  /** The time in `zone` (a valid IANA name) at `at` (default: now). */
  export function now(zone: string, at: Date = new Date()): Now {
    const p = parts(zone, at)
    const year = Number(p.year)
    const month = Number(p.month)
    const day = Number(p.day)
    const hour = Number(p.hour) % 24
    const minute = Number(p.minute)
    const second = Number(p.second)
    const wall = Date.UTC(year, month - 1, day, hour, minute, second)
    const utc = Math.floor(at.getTime() / 1000) * 1000
    const utcOffset = offset(Math.round((wall - utc) / 60000))
    const pad = (n: number, width = 2) => String(n).padStart(width, "0")
    return {
      iso: `${pad(year, 4)}-${pad(month)}-${pad(day)}T${pad(hour)}:${pad(minute)}:${pad(second)}${utcOffset}`,
      time_zone: zone,
      utc_offset: utcOffset,
      weekday: p.weekday,
    }
  }

  /** The date in `zone` in `Date.toDateString()` form ("Tue Nov 17 2026"); no time, so the prompt prefix stays cacheable. */
  export function dateString(zone: string, at: Date = new Date()): string {
    const values: Record<string, string> = {}
    const format = new Intl.DateTimeFormat("en-US", {
      timeZone: zone,
      weekday: "short",
      month: "short",
      day: "2-digit",
      year: "numeric",
    })
    for (const part of format.formatToParts(at)) values[part.type] = part.value
    return `${values.weekday} ${values.month} ${values.day} ${values.year}`
  }
}
