/**
 * Allternit Sleep triggers: Gizzi tells allternit-api when a session is a
 * good moment to sleep (POST /api/v1/memory/sleep/signal). No scheduled
 * jobs: every signal follows something the session itself did.
 *
 *  - idle         once, when no new prompt arrived for the prompt-cache TTL
 *                 after a turn ended (the cache is cold by then, so a cycle
 *                 costs the person nothing in re-caching).
 *  - resume       fire-and-forget, when a prompt arrives after `idle` was sent,
 *                 so a running cycle stops cleanly.
 *  - pre_compact  fire-and-forget at compaction. Never awaited: compaction
 *                 proceeds with the API slow or down.
 *  - session_end  best effort at process exit, bounded (500 ms, else skipped).
 *
 * Only sent when signed in and Sleep is on for the account (./client
 * `sleepEnabled`, one cached status check per process); off, 404 or any
 * error sends nothing. Nothing here ever throws into a turn.
 *
 * Callers (main thread only, never subagents):
 *  - runtime/session (Desktop and other hosts): prompt.ts `prompt()` →
 *    promptArrived, Stop → turnEnded, status.ts idle → sessionIdle,
 *    compaction.ts PreCompact → preCompact, `gizzi serve` shutdown → endAll.
 *  - the ink-app TUI: query.ts `query()` start/finally → promptArrived /
 *    turnEnded, the compact paths → preCompact, graceful shutdown → endAll.
 *
 * The clock and transport are injectable so tests drive the timer and the
 * network edge directly.
 */
import { PROMPT_CACHE_TTL_S } from "@/shared/util/cache-countdown"
import { Log } from "@/shared/util/log"
import type { SleepSignal, SleepSignalEvent } from "./client"

export interface SleepClock {
  setTimeout(fn: () => void, ms: number): unknown
  clearTimeout(handle: unknown): void
}

export type SleepTransport = (signal: SleepSignal, options: { timeoutMs: number }) => Promise<unknown>

export interface SleepTriggerDeps {
  clock?: SleepClock
  transport?: SleepTransport
  /** Signed in and Sleep on for the account. Errors count as off. */
  enabled?: () => Promise<boolean>
  log?: (message: string) => void
}

export const SESSION_END_TIMEOUT_MS = 500
const SIGNAL_TIMEOUT_MS = 5_000

interface SessionState {
  ttl: number
  timer?: unknown
  idleSent: boolean
}

const realClock: SleepClock = {
  setTimeout(fn, ms) {
    const t = setTimeout(fn, ms)
    // A pending idle timer must never keep a finished process alive.
    ;(t as { unref?: () => void }).unref?.()
    return t
  },
  clearTimeout(handle) {
    clearTimeout(handle as ReturnType<typeof setTimeout>)
  },
}

// Loaded lazily so importing the triggers (session/status.ts does) stays light.
const realTransport: SleepTransport = async (signal, { timeoutMs }) =>
  (await import("./client")).sendSleepSignal(signal, timeoutMs)
const realEnabled = async () => (await import("./client")).sleepEnabled()

export function createSleepTriggers(deps: SleepTriggerDeps = {}) {
  const clock = deps.clock ?? realClock
  const transport = deps.transport ?? realTransport
  const enabled = deps.enabled ?? realEnabled
  const log = deps.log ?? (() => {})
  const sessions = new Map<string, SessionState>()

  const validTtl = (ttl: number | undefined, fallback: number) =>
    typeof ttl === "number" && Number.isFinite(ttl) && ttl > 0 ? ttl : fallback

  async function deliver(sessionID: string, event: SleepSignalEvent, ttl: number, timeoutMs: number): Promise<void> {
    try {
      if (!(await enabled())) return
      await transport({ session_id: sessionID, event, cache_ttl_s: ttl }, { timeoutMs })
    } catch (error) {
      log(`[sleep] ${event} signal skipped: ${error instanceof Error ? error.message : String(error)}`)
    }
  }

  /** Fire and forget: never awaited, never throws. */
  function fire(sessionID: string, event: SleepSignalEvent, ttl: number): void {
    void deliver(sessionID, event, ttl, SIGNAL_TIMEOUT_MS)
  }

  function clearTimer(s: SessionState): void {
    if (s.timer === undefined) return
    clock.clearTimeout(s.timer)
    s.timer = undefined
  }

  function arm(sessionID: string, s: SessionState): void {
    clearTimer(s)
    if (s.idleSent) return // one idle per quiet stretch; the next prompt re-arms it
    s.timer = clock.setTimeout(() => {
      s.timer = undefined
      if (s.idleSent) return
      s.idleSent = true
      fire(sessionID, "idle", s.ttl)
    }, s.ttl * 1000)
  }

  return {
    /** A new prompt arrived: cancel the idle timer; after an `idle`, send `resume`. */
    promptArrived(sessionID: string, ttlSeconds?: number): void {
      try {
        const s = sessions.get(sessionID)
        if (!s) {
          sessions.set(sessionID, { ttl: validTtl(ttlSeconds, PROMPT_CACHE_TTL_S), idleSent: false })
          return
        }
        if (ttlSeconds !== undefined) s.ttl = validTtl(ttlSeconds, s.ttl)
        clearTimer(s)
        if (s.idleSent) {
          s.idleSent = false
          fire(sessionID, "resume", s.ttl)
        }
      } catch (error) {
        log(`[sleep] promptArrived failed: ${String(error)}`)
      }
    },

    /** A turn finished (Stop): start the idle timer for the cache TTL. */
    turnEnded(sessionID: string, ttlSeconds?: number): void {
      try {
        let s = sessions.get(sessionID)
        if (!s) {
          s = { ttl: validTtl(ttlSeconds, PROMPT_CACHE_TTL_S), idleSent: false }
          sessions.set(sessionID, s)
        } else if (ttlSeconds !== undefined) {
          s.ttl = validTtl(ttlSeconds, s.ttl)
        }
        arm(sessionID, s)
      } catch (error) {
        log(`[sleep] turnEnded failed: ${String(error)}`)
      }
    },

    /**
     * The session went idle (status bus). Only sessions this process saw a
     * prompt for count, which keeps subagent/child sessions out.
     */
    sessionIdle(sessionID: string): void {
      const s = sessions.get(sessionID)
      if (s && s.timer === undefined) arm(sessionID, s)
    },

    /** Compaction is starting: signal and return at once. Never blocks compaction. */
    preCompact(sessionID: string): void {
      const s = sessions.get(sessionID)
      if (!s) return
      fire(sessionID, "pre_compact", s.ttl)
    },

    /** Process exit: `session_end` for every known session, bounded; resolves within `timeoutMs`. */
    endAll(timeoutMs = SESSION_END_TIMEOUT_MS): Promise<void> {
      const all = [...sessions.entries()]
      sessions.clear()
      if (all.length === 0) return Promise.resolve()
      for (const [, s] of all) clearTimer(s)
      const sends = Promise.all(all.map(([id, s]) => deliver(id, "session_end", s.ttl, timeoutMs))).then(() => {})
      let handle: unknown
      const cap = new Promise<void>((resolve) => {
        handle = clock.setTimeout(resolve, timeoutMs)
      })
      return Promise.race([sends, cap]).finally(() => clock.clearTimeout(handle))
    },

    /** Test/introspection: whether an idle timer is pending for the session. */
    pending(sessionID: string): boolean {
      return sessions.get(sessionID)?.timer !== undefined
    },
  }
}

export type SleepTriggers = ReturnType<typeof createSleepTriggers>

const log = Log.create({ service: "memory-sleep" })

/** The process-wide triggers (real clock, real API, the runtime logger). */
export const SleepTriggers: SleepTriggers = createSleepTriggers({ log: (message) => log.info(message) })
