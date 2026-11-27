/**
 * The sessions view's model of the engine's live sessions
 * (`GET /api/factory/sessions`, factory/engine/src/agents/live_sessions.rs).
 *
 * Every field is read defensively: older engines map blocked to idle and send
 * no `terminalId` / `steer` / `blockedSince`; newer ones send all three. The
 * view works with both.
 *
 * Free of React and Ink so bun tests can drive it with a stubbed fetch.
 */
import path from 'node:path'
import { findEngineUrl, lastMeaningfulLines } from '@/cli/factory/live'
import { fetchWithPlatformToken } from '@/runtime/bots/platform-api'
import { gatewayBase } from '@/shared/constants/allternitGateway'

export type SessionStatus = 'working' | 'blocked' | 'idle' | 'done'
export type SteerRoute = 'terminal' | 'delivery' | 'none'

export interface LiveSessionRow {
  /** `person/session`; the send route's `to`. Unique per session. */
  address: string
  name: string
  /** Runtime the session runs (gizzi, claude, codex, agy…). */
  runtime: string
  cwd: string
  folder: string
  status: SessionStatus
  /** The engine's status word as sent, for debugging and docs. */
  rawStatus: string
  doing: string | null
  /** RFC 3339 time the session entered its current status. */
  since: string | null
  blockedSince: string | null
  sessionId: string
  paneId: string | null
  botId: string | null
  terminalId: string | null
  steer: SteerRoute
  machineName: string
  /** Where it runs when it is not in an Allternit pane (Terminal, iTerm2…). */
  app: string | null
}

const WORKING = new Set(['busy', 'working', 'running', 'thinking'])
const IDLE = new Set(['idle', 'waiting', 'ready'])
const DONE = new Set(['done', 'offline', 'exited', 'stopped', 'finished'])

export function normStatus(raw: unknown, blockedSince?: unknown): SessionStatus {
  const s = typeof raw === 'string' ? raw.toLowerCase() : ''
  if (s === 'blocked') return 'blocked'
  if (WORKING.has(s)) return 'working'
  if (DONE.has(s)) return 'done'
  // An engine that knows blocked but sent another word still says when it started.
  if (typeof blockedSince === 'string' && blockedSince && !IDLE.has(s)) return 'blocked'
  return 'idle'
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v : null
}

/** One engine row → the view's row; null for a row with no address. */
export function toRow(raw: unknown): LiveSessionRow | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const address = str(r.address)
  if (!address) return null
  const holders = Array.isArray(r.holders) ? (r.holders as Record<string, unknown>[]) : []
  const holderTerminal = holders.map(h => str(h?.terminalId)).find(Boolean) ?? null
  const terminalId = str(r.terminalId) ?? holderTerminal
  const steerRaw = str(r.steer)
  // Before the engine sent `steer`, every listed session took the send route.
  const steer: SteerRoute = steerRaw === 'terminal' || steerRaw === 'delivery' || steerRaw === 'none' ? steerRaw : 'delivery'
  const cwd = str(r.cwd) ?? ''
  const harness = str(r.harness) ?? 'unknown'
  return {
    address,
    name: str(r.name) ?? address.split('/').pop() ?? address,
    runtime: harness.replace(/-tui$/, ''),
    cwd,
    folder: str(r.repo) ? path.basename(String(r.repo)) : cwd ? path.basename(cwd) : '',
    status: normStatus(r.status, r.blockedSince),
    rawStatus: str(r.status) ?? '',
    doing: str(r.doing),
    since: str(r.since),
    blockedSince: str(r.blockedSince),
    sessionId: str(r.sessionId) ?? '',
    paneId: str(r.paneId),
    botId: str(r.botId),
    terminalId,
    steer,
    machineName: str(r.machineName) ?? '',
    app: holders.map(h => str(h?.app)).find(Boolean) ?? null,
  }
}

export function toRows(doc: unknown): LiveSessionRow[] {
  const list = Array.isArray(doc) ? doc : Array.isArray((doc as { sessions?: unknown })?.sessions) ? (doc as { sessions: unknown[] }).sessions : []
  const rows: LiveSessionRow[] = []
  for (const item of list) {
    const row = toRow(item)
    if (row) rows.push(row)
  }
  return rows
}

/** Short age: 12s, 4m, 3h, 2d. */
export function ageText(since: string | null, now = Date.now()): string {
  if (!since) return ''
  const t = Date.parse(since)
  if (!Number.isFinite(t)) return ''
  const s = Math.max(0, Math.floor((now - t) / 1000))
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) return `${Math.floor(s / 3600)}h`
  return `${Math.floor(s / 86400)}d`
}

export const STATUS_COLOR: Record<SessionStatus, 'suggestion' | 'warning' | 'subtle' | 'success'> = {
  working: 'suggestion',
  blocked: 'warning',
  idle: 'subtle',
  done: 'success',
}

/** Same list (by the fields the view draws)? Lets the poller skip re-renders. */
export function sameRows(a: LiveSessionRow[], b: LiveSessionRow[]): boolean {
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!
    const y = b[i]!
    if (
      x.address !== y.address ||
      x.status !== y.status ||
      x.doing !== y.doing ||
      x.since !== y.since ||
      x.steer !== y.steer ||
      x.terminalId !== y.terminalId ||
      x.paneId !== y.paneId
    )
      return false
  }
  return true
}

// ---------------------------------------------------------------- blocked notice

/**
 * Addresses that became blocked between two polls. A session that stays
 * blocked is not listed again; one that leaves blocked and comes back is.
 * The first poll (prev === null) only records state, so opening Gizzi with a
 * session already blocked does not ring.
 */
export function newlyBlocked(prev: ReadonlySet<string> | null, rows: LiveSessionRow[]): { blocked: Set<string>; fresh: LiveSessionRow[] } {
  const blocked = new Set(rows.filter(r => r.status === 'blocked').map(r => r.address))
  if (prev === null) return { blocked, fresh: [] }
  return { blocked, fresh: rows.filter(r => r.status === 'blocked' && !prev.has(r.address)) }
}

/** The terminal bytes for one blocked notice: bell, then an OSC 9 desktop notice. */
export function blockedNoticeBytes(name: string): string {
  const text = `${name} needs you`.replace(/[\x00-\x1f\x07]/g, ' ')
  return `\x07\x1b]9;${text}\x07`
}

// ---------------------------------------------------------------- engine client

let engineBase: string | null = null

async function engine(): Promise<string> {
  if (engineBase) return engineBase
  const found = await findEngineUrl()
  if (!found) {
    throw new Error(
      "The Factory engine isn't serving on this computer. Open Allternit Desktop, or run `allternit-factory serve`.",
    )
  }
  engineBase = found
  return found
}

async function engineJson(p: string, init: RequestInit & { timeoutMs?: number } = {}): Promise<unknown> {
  const base = await engine()
  let res: Response
  try {
    res = await fetch(`${base}${p}`, {
      ...init,
      headers: { 'content-type': 'application/json', ...(init.headers as Record<string, string> | undefined) },
      signal: AbortSignal.timeout(init.timeoutMs ?? 8000),
    })
  } catch (err) {
    engineBase = null // re-probe the ports next time
    throw err
  }
  const text = await res.text()
  let body: unknown = text
  try {
    body = text ? JSON.parse(text) : null
  } catch {
    // plain text
  }
  if (!res.ok) {
    const b = body as { fact?: string; error?: string; action?: string } | string
    const fact = typeof b === 'string' ? b || `HTTP ${res.status}` : (b?.fact ?? b?.error ?? `HTTP ${res.status}`)
    const action = typeof b === 'object' && b?.action ? ` — ${b.action}` : ''
    throw new Error(`${fact}${action}`)
  }
  return body
}

export async function listSessions(): Promise<LiveSessionRow[]> {
  return toRows(await engineJson('/api/factory/sessions'))
}

/** The last `n` meaningful lines of a pane session's screen; [] when it has no bot pane. */
export async function captureLines(row: LiveSessionRow, n = 3): Promise<string[]> {
  if (!row.botId || !row.paneId) return []
  const doc = (await engineJson(`/api/factory/agents/${encodeURIComponent(row.botId)}/capture?lines=60`, { timeoutMs: 4000 })) as {
    text?: unknown
  }
  return typeof doc?.text === 'string' ? lastMeaningfulLines(doc.text, n) : []
}

export interface SteerResult {
  via: 'terminal' | 'delivery'
  state: string
}

/**
 * Send `text` to a session, once. `steer === "terminal"` types it into the
 * session's terminal (allternit-api `POST /terminal/:id/input`, with a CR so
 * it submits); otherwise the engine's send route picks the delivery tier.
 */
export async function steerSession(row: LiveSessionRow, text: string): Promise<SteerResult> {
  if (row.steer === 'none') throw new Error(`${row.name} can't be steered from here`)
  if (row.steer === 'terminal' && row.terminalId) {
    const url = `${gatewayBase()}/terminal/${encodeURIComponent(row.terminalId)}/input`
    const init: RequestInit = {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: `${text}\r` }),
      signal: AbortSignal.timeout(8000),
    }
    const res = (await fetchWithPlatformToken(url, init)) ?? (await fetch(url, init))
    if (!res.ok) throw new Error(`The terminal refused the message (HTTP ${res.status})`)
    return { via: 'terminal', state: 'typed' }
  }
  const doc = (await engineJson('/api/factory/send', {
    method: 'POST',
    body: JSON.stringify({ to: row.address, text }),
    timeoutMs: 30000,
  })) as { state?: string; tier?: string | null }
  return { via: 'delivery', state: [doc?.state ?? 'sent', doc?.tier].filter(Boolean).join(' · ') }
}

/** Typed confirm: stop only when the typed text is exactly the session's name. */
export function confirmMatches(row: Pick<LiveSessionRow, 'name'>, typed: string): boolean {
  return typed.trim() === row.name
}

/** `n`: the runtime and first task from the prompt line ("codex fix the tests"). */
export function parseNewSession(line: string): { cli: string; prompt: string } | null {
  const m = line.trim().match(/^(\S+)(?:\s+(.*))?$/)
  if (!m) return null
  return { cli: m[1]!.toLowerCase(), prompt: (m[2] ?? '').trim() }
}

/** The shell form of `gizzi spawn`, shown when the command can't run from here. */
export function spawnCommandText(cli: string, prompt: string): string {
  return `gizzi spawn ${cli}${prompt ? ` ${JSON.stringify(prompt)}` : ''} --window`
}
