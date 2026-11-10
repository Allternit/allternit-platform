/**
 * Bot pause gate for gizzi's own automatic starts (loop iterations, scheduled
 * agent/cowork routine runs). Reads the runtime pause state through the
 * work/api client, cached ~10 s. Never blocks when the runtime is unreachable
 * or answers 404 (older runtime without the pause route).
 */
import { getBotPause, type BotPause } from "@/cli/work/api"

export const PAUSE_CACHE_MS = 10_000

type Fetcher = () => Promise<BotPause>

let fetcher: Fetcher = getBotPause
let cached: { paused: boolean; at: number } | null = null

/** Test hook: swap the fetcher and clear the cache. */
export function __setPauseFetcher(f: Fetcher | null): void {
  fetcher = f ?? getBotPause
  cached = null
}

/** Refresh the cache when older than PAUSE_CACHE_MS. Never throws. */
export async function refreshBotPause(now: number = Date.now()): Promise<boolean> {
  if (cached && now - cached.at < PAUSE_CACHE_MS) return cached.paused
  let paused = false
  try {
    paused = (await fetcher()).paused === true
  } catch {
    paused = false // unreachable or 404: do not block
  }
  cached = { paused, at: now }
  return paused
}

/** Last known state without a network call (false before the first refresh). */
export function botsPausedCached(): boolean {
  return cached?.paused === true
}

/**
 * Hold while paused: resolves when bots are running again, or when
 * `shouldStop()` turns true. `onHold` is called once when the wait begins.
 */
export async function waitWhilePaused(
  shouldStop: () => boolean,
  onHold?: () => void,
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
): Promise<void> {
  let announced = false
  while (!shouldStop() && (await refreshBotPause())) {
    if (!announced) {
      announced = true
      onHold?.()
    }
    await sleep(PAUSE_CACHE_MS)
  }
}
