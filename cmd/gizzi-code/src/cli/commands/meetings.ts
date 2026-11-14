/**
 * `gizzi meetings` — Allternit Meetings from the terminal (cloud-api
 * `/api/v1/meetings`, `/meeting-sessions`, `/me/meeting-settings`; shapes in
 * docs/meetings/CONTRACT.md). Same words for Gizzi's state as the web.
 */
import type { Argv } from "yargs"
import * as fs from "fs"
import * as os from "os"
import * as path from "path"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { cloudInferenceToken, cloudOrigin } from "@/runtime/providers/discovery/allternit-cloud"
import { Installation } from "@/shared/installation"
import { END_REASON_LABELS, describeApiError, gizziLabel, mmss, parseCallUrl, parseSse } from "@/cli/commands/meetings-model"

type Meeting = {
  id: string
  title: string
  platform_label: string
  starts_at: string | null
  gizzi: string
  people_label?: string
  latest_session_id: string | null
  join_url?: string
  page_url?: string
  auto_join?: boolean | null
}
type Session = {
  id: string
  meeting_id: string
  state: string
  gizzi: string
  end_reason: string | null
  speak_replies?: boolean
  listening?: boolean
  live_at: string | null
  joined_at: string | null
  ended_at: string | null
  summary: string | null
  decisions: string[]
  action_items: { text: string; owner: string | null; due: string | null }[]
}
type Segment = { seq: number; speaker: string | null; start_ms: number; text: string }

class Failure extends Error {}

async function token(): Promise<string> {
  const direct = await cloudInferenceToken()
  if (direct) return direct
  try {
    const dir = process.env.GIZZI_CONFIG_DIR ?? path.join(os.homedir(), ".config", "gizzi-code")
    const session = JSON.parse(fs.readFileSync(path.join(dir, "session.json"), "utf8"))
    if (session?.accessToken) return session.accessToken
  } catch {}
  throw new Failure(describeApiError(401).replace("did not accept your credentials", "needs you signed in to Allternit Cloud"))
}

async function raw(method: string, route: string, body?: unknown, signal?: AbortSignal, accept = "application/json"): Promise<Response> {
  const t = await token()
  let res: Response
  try {
    res = await fetch(`${cloudOrigin()}/api/v1${route}`, {
      method,
      headers: {
        Authorization: `Bearer ${t}`,
        Accept: accept,
        "X-Allternit-Client": `gizzi-code/${Installation.VERSION}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: signal ?? AbortSignal.timeout(15_000),
    })
  } catch (e) {
    throw new Failure(`Could not reach Allternit Cloud: ${e instanceof Error ? e.message : String(e)}`)
  }
  if (!res.ok) {
    const b = (await res.json().catch(() => undefined)) as { error?: unknown; message?: string } | undefined
    if (res.status === 409 && b?.error === "zoom_not_connected")
      throw new Failure(`Connect Zoom once, then try again: ${ZOOM_SETTINGS_URL}\nZoom only lets Gizzi in while you are in the call.`)
    throw new Failure(describeApiError(res.status, b))
  }
  return res
}

async function api<T>(method: string, route: string, body?: unknown): Promise<T> {
  const res = await raw(method, route, body)
  if (res.status === 204) return undefined as T
  return (await res.json()) as T
}

async function guard(fn: () => Promise<unknown>): Promise<void> {
  try {
    await fn()
  } catch (e) {
    UI.error((e as Error).message)
    process.exitCode = 1
  }
}

const json = (v: unknown) => process.stdout.write(JSON.stringify(v, null, 2) + "\n")

function pad(s: string, n: number): string {
  return s.length >= n ? s.slice(0, n - 1) + "…" : s.padEnd(n)
}

/** Meeting by full id or unique id prefix, across the ranges the API serves. */
async function findMeeting(key: string): Promise<Meeting> {
  const seen = new Map<string, Meeting>()
  for (const range of ["now", "upcoming", "past"]) {
    const r = await api<{ meetings: Meeting[] }>("GET", `/meetings?range=${range}&limit=200`)
    for (const m of r.meetings ?? []) seen.set(m.id, m)
  }
  const hits = [...seen.values()].filter((m) => m.id === key || m.id.startsWith(key))
  const exact = hits.find((m) => m.id === key)
  if (exact) return exact
  if (hits.length === 1) return hits[0]
  throw new Failure(hits.length ? `"${key}" matches ${hits.length} meetings; use more of the id.` : `No meeting with id "${key}". Run \`gizzi meetings list\`.`)
}

async function sessionOf(key: string): Promise<{ meeting: Meeting; session: Session }> {
  const meeting = await findMeeting(key)
  if (!meeting.latest_session_id) throw new Failure("Gizzi has not joined this meeting yet.")
  const r = await api<{ session: Session }>("GET", `/meeting-sessions/${meeting.latest_session_id}`)
  return { meeting, session: r.session }
}

function when(iso: string | null): string {
  if (!iso) return "no time"
  const d = new Date(iso)
  const day = d.toDateString() === new Date().toDateString() ? "Today" : d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })
  return `${day} ${d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`
}

const ZOOM_SETTINGS_URL = "https://ai.allternit.com/settings/meetings"
const ZOOM_NOTE = "Zoom: Gizzi joins as \"Gizzi (Allternit)\" only while you are in the call, so you must be in it. Connect Zoom once in Settings > Meetings."
const isZoom = (m: { platform_label: string }) => /zoom/i.test(m.platform_label)

export function formatMeetings(list: Meeting[]): string {
  if (!list.length) return "No meetings."
  return [
    `${pad("WHEN", 20)} ${pad("TITLE", 34)} ${pad("PLATFORM", 16)} ${pad("GIZZI", 21)} ID`,
    ...list.map((m) => `${pad(when(m.starts_at), 20)} ${pad(m.title || "(untitled)", 34)} ${pad(m.platform_label, 16)} ${pad(gizziLabel(m.gizzi), 21)} ${m.id.slice(0, 8)}`),
    ...(list.some(isZoom) ? ["", ZOOM_NOTE] : []),
  ].join("\n")
}

const ListCommand = cmd({
  command: "list",
  aliases: ["ls"],
  describe: "your meetings and Gizzi's state in each",
  builder: (y: Argv) =>
    y
      .option("range", { type: "string", choices: ["today", "week", "past"], default: "today" })
      .option("json", { type: "boolean", describe: "print JSON" }),
  handler: (args) => guard(async () => {
    const r = await api<{ meetings: Meeting[] }>("GET", `/meetings?range=${args.range}`)
    if (args.json) return json(r)
    process.stdout.write(formatMeetings(r.meetings ?? []) + "\n")
  }),
})

const AddCommand = cmd({
  command: "add <url>",
  describe: "add a call link (Meet, Zoom or Teams); optionally send Gizzi",
  builder: (y: Argv) =>
    y
      .positional("url", { type: "string", demandOption: true })
      .option("now", { type: "boolean", describe: "Gizzi joins right away" })
      .option("at-start", { type: "boolean", describe: "Gizzi joins when the meeting starts" })
      .conflicts("now", "at-start")
      .option("json", { type: "boolean" }),
  handler: (args) => guard(async () => {
    const url = String(args.url)
    if (!parseCallUrl(url)) throw new Failure("That link is not a Google Meet, Zoom or Teams call link.")
    const meeting = await api<Meeting>("POST", "/meetings", { url })
    let session: Session | undefined
    if (args.now || args["at-start"]) session = await api<Session>("POST", `/meetings/${meeting.id}/sessions`, { when: args.now ? "now" : "start" })
    if (args.json) return json({ meeting, session })
    process.stdout.write(`Added: ${meeting.title || meeting.join_url} (${meeting.platform_label}) · ${meeting.id}\n`)
    if (session) process.stdout.write(`Gizzi: ${gizziLabel(session.gizzi)}\n`)
    if (isZoom(meeting)) process.stdout.write(`${ZOOM_NOTE}\n`)
  }),
})

const JoinCommand = cmd({
  command: "join <id>",
  describe: "send Gizzi to a meeting (at its start, or --now)",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }).option("now", { type: "boolean" }),
  handler: (args) => guard(async () => {
    const m = await findMeeting(String(args.id))
    const s = await api<Session>("POST", `/meetings/${m.id}/sessions`, { when: args.now ? "now" : "start" })
    process.stdout.write(`${m.title || m.id}: ${gizziLabel(s.gizzi)}\n`)
  }),
})

const AutoCommand = cmd({
  command: "auto <id> <onoff>",
  describe: "turn Gizzi's auto-join for one meeting on or off",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }).positional("onoff", { type: "string", choices: ["on", "off"], demandOption: true }),
  handler: (args) => guard(async () => {
    const m = await findMeeting(String(args.id))
    const r = await api<Meeting>("PATCH", `/meetings/${m.id}`, { auto_join: args.onoff === "on" })
    process.stdout.write(`${m.title || m.id}: auto-join ${args.onoff}; Gizzi ${gizziLabel(r.gizzi)}\n`)
  }),
})

const StatusCommand = cmd({
  command: "status <id>",
  describe: "Gizzi's state in a meeting, time in the call, and why it ended",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }).option("json", { type: "boolean" }),
  handler: (args) => guard(async () => {
    const { meeting, session } = await sessionOf(String(args.id))
    if (args.json) return json({ meeting, session })
    const start = session.live_at ?? session.joined_at
    const end = session.ended_at ? Date.parse(session.ended_at) : Date.now()
    const lines = [`${meeting.title || meeting.id}`, `  state:   ${gizziLabel(session.gizzi)} (${session.state})`]
    if (start) lines.push(`  elapsed: ${mmss(end - Date.parse(start))}`)
    if (session.end_reason) lines.push(`  ended:   ${END_REASON_LABELS[session.end_reason] ?? session.end_reason}`)
    process.stdout.write(lines.join("\n") + "\n")
  }),
})

const TranscriptCommand = cmd({
  command: "transcript <id>",
  describe: "the transcript with speaker labels; --follow keeps listening",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }).option("follow", { alias: "f", type: "boolean" }),
  handler: (args) => guard(async () => {
    const { session } = await sessionOf(String(args.id))
    let after = 0
    const show = (s: Segment) => process.stdout.write(`[${mmss(s.start_ms)}] ${s.speaker || "Unknown"}: ${s.text}\n`)
    for (;;) {
      const r = await api<{ segments: Segment[]; next_after_seq: number }>("GET", `/meeting-sessions/${session.id}/transcript?after_seq=${after}&limit=2000`)
      r.segments.forEach((s) => {
        show(s)
        after = Math.max(after, s.seq)
      })
      if (r.segments.length < 2000) break
    }
    if (!args.follow) return
    const res = await raw("GET", `/meeting-sessions/${session.id}/events`, undefined, AbortSignal.timeout(8 * 3600_000), "text/event-stream")
    const reader = res.body!.getReader()
    const dec = new TextDecoder()
    let buf = ""
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buf += dec.decode(value, { stream: true })
      const p = parseSse(buf)
      buf = p.rest
      for (const ev of p.events) {
        let d: any
        try { d = JSON.parse(ev.data) } catch { continue }
        if (ev.event === "meeting.transcript.delta") for (const s of d.segments as Segment[]) if (s.seq > after) { show(s); after = s.seq }
        if (ev.event === "meeting.ended") return void process.stdout.write(`-- meeting ended: ${END_REASON_LABELS[d.end_reason] ?? d.end_reason}\n`)
      }
    }
  }),
})

const RecapCommand = cmd({
  command: "recap <id>",
  describe: "summary, decisions and action items",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }).option("json", { type: "boolean" }),
  handler: (args) => guard(async () => {
    const { meeting, session } = await sessionOf(String(args.id))
    if (args.json) return json({ id: meeting.id, title: meeting.title, summary: session.summary, decisions: session.decisions, action_items: session.action_items })
    if (!session.summary && !session.decisions?.length && !session.action_items?.length) throw new Failure("No recap yet. It appears once the meeting ends.")
    const out = [meeting.title || meeting.id, "", session.summary ?? ""]
    if (session.decisions?.length) out.push("", "Decisions", ...session.decisions.map((d) => `  - ${d}`))
    if (session.action_items?.length)
      out.push("", "Action items", ...session.action_items.map((a) => `  - ${a.text}${a.owner ? ` (${a.owner})` : ""}${a.due ? ` due ${a.due}` : ""}`))
    process.stdout.write(out.join("\n") + "\n")
  }),
})

const LeaveCommand = cmd({
  command: "leave <id>",
  describe: "tell Gizzi to leave the call",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }),
  handler: (args) => guard(async () => {
    const { session } = await sessionOf(String(args.id))
    const r = await api<Session>("POST", `/meeting-sessions/${session.id}/control`, { action: "leave" })
    process.stdout.write(`Gizzi: ${gizziLabel(r.gizzi)}\n`)
  }),
})

const SettingsCommand = cmd({
  command: "settings",
  describe: "show or change your Meetings settings",
  builder: (y: Argv) => y.option("auto-join", { type: "string", choices: ["off", "all", "all_except_1on1"], describe: "which calendar meetings Gizzi joins" }).option("json", { type: "boolean" }),
  handler: (args) => guard(async () => {
    const s = args["auto-join"]
      ? await api<Record<string, unknown>>("PUT", "/me/meeting-settings", { auto_join: args["auto-join"] })
      : await api<Record<string, unknown>>("GET", "/me/meeting-settings")
    if (args.json) return json(s)
    process.stdout.write(Object.entries(s).map(([k, v]) => `${pad(k, 20)} ${v}`).join("\n") + "\n")
  }),
})

export const MeetingsCommand = cmd({
  command: "meetings",
  describe: "Allternit Meetings: see calls, send Gizzi, read transcripts and recaps",
  builder: (yargs: Argv) =>
    yargs
      .command(ListCommand)
      .command(AddCommand)
      .command(JoinCommand)
      .command(AutoCommand)
      .command(StatusCommand)
      .command(TranscriptCommand)
      .command(RecapCommand)
      .command(LeaveCommand)
      .command(SettingsCommand)
      .demandCommand(1, "Specify a meetings command (list, add, join, auto, status, transcript, recap, leave, settings)"),
  handler: () => {},
})
