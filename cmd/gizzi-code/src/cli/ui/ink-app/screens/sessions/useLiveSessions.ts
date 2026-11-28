/**
 * One poller of the engine's live sessions for the whole TUI, shared by the
 * sessions view (`/sessions`) and the REPL footer's blocked badge.
 *
 * - Every 2 s while the view is open, every 6 s for the footer alone, and
 *   every 30 s while the engine is not answering.
 * - A failed poll keeps the last good list (no flicker); the error shows next
 *   to it. A poll that changes nothing the view draws does not re-render.
 * - Blocked notices live here, not in a component, so a transition rings once
 *   whichever screens are mounted: terminal bell + OSC 9 "<name> needs you".
 *   `sessionsBlockedNotice: false` in the global config turns them off.
 */
import * as React from 'react'
import { useContext } from 'react'
import { TerminalWriteContext } from '../../ink/useTerminalNotification'
import { getGlobalConfig } from '../../utils/config'
import {
  blockedNoticeBytes,
  captureLines,
  listSessions,
  newlyBlocked,
  sameRows,
  type LiveSessionRow,
} from './liveSessions'

export interface SessionsSnapshot {
  rows: LiveSessionRow[]
  /** Last output lines per address (pane sessions only). */
  lines: Record<string, string[]>
  error: string | null
  loaded: boolean
  blockedCount: number
}

export const FAST_MS = 2000
export const BACKGROUND_MS = 6000
export const OFFLINE_MS = 30000
const MAX_CAPTURES = 12

export interface PollerDeps {
  list: () => Promise<LiveSessionRow[]>
  capture: (row: LiveSessionRow) => Promise<string[]>
  noticeEnabled: () => boolean
}

type Want = { fast: boolean; lines: boolean }

export class SessionsPoller {
  snapshot: SessionsSnapshot = { rows: [], lines: {}, error: null, loaded: false, blockedCount: 0 }
  private listeners = new Map<(s: SessionsSnapshot) => void, Want>()
  private writers = new Set<(data: string) => void>()
  private prevBlocked: Set<string> | null = null
  private timer: ReturnType<typeof setTimeout> | null = null
  private running = false

  constructor(private deps: PollerDeps) {}

  subscribe(listener: (s: SessionsSnapshot) => void, want: Want): () => void {
    this.listeners.set(listener, want)
    listener(this.snapshot)
    // Opening the view polls now instead of waiting out the background interval.
    if (want.fast || !this.timer) this.schedule(0)
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  addWriter(write: (data: string) => void): () => void {
    this.writers.add(write)
    return () => this.writers.delete(write)
  }

  stop(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
  }

  private wants(): Want {
    let fast = false
    let lines = false
    for (const w of this.listeners.values()) {
      fast ||= w.fast
      lines ||= w.lines
    }
    return { fast, lines }
  }

  private schedule(ms: number): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = setTimeout(() => {
      this.timer = null
      void this.tick()
    }, ms)
  }

  private emit(next: SessionsSnapshot): void {
    this.snapshot = next
    for (const l of this.listeners.keys()) l(next)
  }

  /** One poll. Public so tests can drive it without timers. */
  async tick(): Promise<void> {
    if (this.running) return
    this.running = true
    const want = this.wants()
    let ok = true
    try {
      const rows = await this.deps.list()
      this.notice(rows)
      let lines = this.snapshot.lines
      if (want.lines) lines = await this.readLines(rows)
      const blockedCount = rows.filter(r => r.status === 'blocked').length
      const prev = this.snapshot
      if (!prev.loaded || prev.error !== null || !sameRows(prev.rows, rows) || lines !== prev.lines || prev.blockedCount !== blockedCount) {
        this.emit({ rows, lines, error: null, loaded: true, blockedCount })
      }
    } catch (err) {
      ok = false
      const error = (err as Error)?.message || String(err)
      if (error !== this.snapshot.error || !this.snapshot.loaded) this.emit({ ...this.snapshot, error, loaded: true })
    } finally {
      this.running = false
    }
    if (this.listeners.size > 0) {
      const now = this.wants()
      this.schedule(now.fast ? FAST_MS : ok ? BACKGROUND_MS : OFFLINE_MS)
    }
  }

  private notice(rows: LiveSessionRow[]): void {
    const { blocked, fresh } = newlyBlocked(this.prevBlocked, rows)
    this.prevBlocked = blocked
    if (fresh.length === 0 || !this.deps.noticeEnabled()) return
    const write = this.writers.values().next().value as ((d: string) => void) | undefined
    if (!write) return
    for (const r of fresh) write(blockedNoticeBytes(r.name))
  }

  private async readLines(rows: LiveSessionRow[]): Promise<Record<string, string[]>> {
    const prev = this.snapshot.lines
    const targets = rows.filter(r => r.paneId && r.botId && r.status !== 'done').slice(0, MAX_CAPTURES)
    const next: Record<string, string[]> = {}
    let changed = Object.keys(prev).length !== targets.length
    await Promise.all(
      targets.map(async r => {
        let got = prev[r.address] ?? []
        try {
          got = await this.deps.capture(r)
        } catch {
          // keep the last lines we had
        }
        const old = prev[r.address]
        if (old && old.length === got.length && old.every((l, i) => l === got[i])) next[r.address] = old
        else {
          next[r.address] = got
          changed = true
        }
      }),
    )
    return changed ? next : prev
  }
}

let shared: SessionsPoller | null = null

export function sharedPoller(): SessionsPoller {
  shared ??= new SessionsPoller({
    list: listSessions,
    capture: row => captureLines(row, 3),
    noticeEnabled: () => getGlobalConfig().sessionsBlockedNotice !== false,
  })
  return shared
}

/** Lets the shared poller ring the terminal from whatever screen is mounted. */
function useNoticeWriter(poller: SessionsPoller): void {
  const writeRaw = useContext(TerminalWriteContext)
  React.useEffect(() => (writeRaw ? poller.addWriter(writeRaw) : undefined), [poller, writeRaw])
}

/** The sessions view: fast polls plus last output lines. */
export function useLiveSessions(): SessionsSnapshot & { refresh: () => void } {
  const poller = sharedPoller()
  const [snap, setSnap] = React.useState<SessionsSnapshot>(poller.snapshot)
  useNoticeWriter(poller)
  React.useEffect(() => poller.subscribe(setSnap, { fast: true, lines: true }), [poller])
  const refresh = React.useCallback(() => void poller.tick(), [poller])
  return { ...snap, refresh }
}

/** The REPL footer: how many sessions are blocked right now (0 hides the badge). */
export function useBlockedSessionsCount(): number {
  const poller = sharedPoller()
  const [count, setCount] = React.useState(poller.snapshot.blockedCount)
  useNoticeWriter(poller)
  React.useEffect(() => poller.subscribe(s => setCount(s.blockedCount), { fast: false, lines: false }), [poller])
  return count
}
