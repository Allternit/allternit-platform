/**
 * Pure parts of `gizzi agents adopt`: the session matcher, the plain-language
 * error sentences and the step lines. No network, no process access.
 */

export interface AdoptHolder {
  pid: number
  app: "Terminal" | "iTerm2" | "Allternit" | "Ghostty" | "other" | string
  terminalId?: string
  paneId?: string
  tty?: string
  startedAt?: string
}

export type Movable = "now" | "busy" | "open_elsewhere" | "no_resume" | "here"
export type MoveState = "waiting_idle" | "stopping" | "starting" | "done" | "failed"

export interface AdoptJob {
  jobId: string
  state: MoveState
  terminalId?: string
  code?: string
  error?: string
}

/** One row of `GET /api/v1/live-sessions`; every adopt field is optional (older runtimes omit them). */
export interface LiveSession {
  address: string
  name?: string
  harness: string
  sessionId?: string
  status?: string
  cwd?: string
  pid?: number
  holders?: AdoptHolder[]
  carriedFlags?: string[]
  canCopy?: boolean
  external?: boolean
  movable?: Movable
  move?: AdoptJob
}

export type MatchResult =
  | { kind: "one"; session: LiveSession }
  | { kind: "none" }
  | { kind: "many"; matches: LiveSession[] }

/**
 * Resolve what the user typed: a session id or address (exact, case-insensitive)
 * first, then a name, then a prefix of the id, address or name. A query that
 * matches more than one row at the first level that matches anything is ambiguous.
 */
export function matchSession(sessions: LiveSession[], query: string): MatchResult {
  const q = query.trim().toLowerCase()
  if (!q) return { kind: "none" }
  const lc = (s?: string) => (s ?? "").toLowerCase()
  const levels: Array<(s: LiveSession) => boolean> = [
    (s) => lc(s.sessionId) === q || lc(s.address) === q,
    (s) => lc(s.name) === q,
    (s) => lc(s.sessionId).startsWith(q) || lc(s.address).startsWith(q) || lc(s.name).startsWith(q),
  ]
  for (const hit of levels) {
    const matches = sessions.filter(hit)
    if (matches.length === 1) return { kind: "one", session: matches[0] }
    if (matches.length > 1) return { kind: "many", matches }
  }
  return { kind: "none" }
}

export function holderLine(h: AdoptHolder): string {
  return `${h.app} pid ${h.pid}${h.tty ? ` on ${h.tty}` : ""}`
}

/** The command the session resumes with in Allternit (display only; the runtime builds the real one). */
export function resumeCommand(s: LiveSession): string {
  const flags = s.carriedFlags?.length ? ` ${s.carriedFlags.join(" ")}` : ""
  return `${s.harness} --resume ${s.sessionId ?? "<id>"}${flags}`
}

/** A reason to refuse before any request, or null when a move may be tried. */
export function refuseMove(s: LiveSession): string | null {
  const label = s.name || s.address
  switch (s.movable) {
    case "here":
      return `${label} is already in Allternit.`
    case "open_elsewhere":
      return `${label} is open in more than one place: ${(s.holders ?? []).map(holderLine).join("; ")}. Close the others first, then run this again.`
    case "no_resume":
      return `${label} can't be resumed in another terminal. Use --new to start a fresh session in the same folder.`
    default:
      return null
  }
}

export function stepLine(job: AdoptJob, name: string, pid?: number): string | null {
  switch (job.state) {
    case "waiting_idle":
      return `Waiting for ${name} to go idle…`
    case "stopping":
      return `Stopping ${pid ? `pid ${pid}` : "the old process"}…`
    case "starting":
      return "Starting in Allternit…"
    case "done":
      return `Done: terminal ${job.terminalId ?? "(unknown)"}`
    default:
      return null
  }
}

const SENTENCES: Record<string, string> = {
  session_open_elsewhere: "That session is open in more than one place. Close the others first.",
  already_here: "That session is already in Allternit.",
  adopt_in_progress: "A move for this session is already running.",
  holder_changed: "The session changed while you were deciding. Run the command again.",
  still_busy: "The agent is still working. Try again when it is idle.",
  did_not_exit: "The old process did not exit, so nothing was moved. Close it yourself, then try again.",
  too_late: "It is too late to cancel: the old process has already been stopped.",
  no_resume: "This tool can't resume a session in another terminal. Use --new.",
  no_copy: "This tool can't copy a session. Use --new or move it.",
}

/** One plain sentence for an adopt error response. A 404 means an older runtime without the route. */
export function errorSentence(status: number, code?: string, message?: string): string {
  if (status === 404) return "This Allternit runtime is too old for adopt; update Allternit Desktop."
  if (code === "start_failed") return `Allternit could not start the session. ${message ?? ""}`.trim()
  if (code && SENTENCES[code]) return SENTENCES[code]
  return message || `The runtime answered ${status}${code ? ` (${code})` : ""}.`
}
