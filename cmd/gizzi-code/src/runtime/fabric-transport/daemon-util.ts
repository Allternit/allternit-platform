/**
 * Shared daemon helpers for gizzi's queue workers: the fabric-transport
 * worker (worker.ts / worker-daemon-entry.ts) and the self-hosted
 * `gizzi worker` (runtime/selfhost/worker.ts).
 *
 *   - JSON logs, one event per line: `{ts, level, event, ...fields}`
 *     (errors to stderr, everything else to stdout).
 *   - Exponential backoff with jitter: 1s → 2s → … → 30s cap, reset on the
 *     next success.
 */

export type DaemonLevel = "info" | "warn" | "error"
export type DaemonLog = (level: DaemonLevel, event: string, fields?: Record<string, unknown>) => void

export function makeJsonLogger(): DaemonLog {
  return (level, event, fields) => {
    const line = JSON.stringify({ ts: new Date().toISOString(), level, event, ...(fields ?? {}) })
    if (level === "error") console.error(line)
    else console.log(line)
  }
}

export interface Backoff {
  /** The wait the next failure will use (before jitter). */
  readonly currentMs: number
  /** Wait the current backoff (+ up to 250 ms jitter), then double it up to the cap. */
  wait(): Promise<void>
  /** Back to the starting delay (call after a success). */
  reset(): void
}

export function createBackoff(opts: { startMs?: number; maxMs?: number; jitterMs?: number } = {}): Backoff {
  const start = opts.startMs ?? 1000
  const max = opts.maxMs ?? 30_000
  const jitter = opts.jitterMs ?? 250
  let current = start
  return {
    get currentMs() {
      return current
    },
    async wait() {
      const ms = current + Math.floor(Math.random() * jitter)
      current = Math.min(current * 2, max)
      await new Promise((resolve) => setTimeout(resolve, ms))
    },
    reset() {
      current = start
    },
  }
}
