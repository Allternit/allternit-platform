/**
 * The TUI's side of the Allternit Sleep triggers (runtime/memory/sleep/triggers.ts):
 * the session id, the prompt-cache TTL this session's requests use, and the
 * exit hook. Main thread only: callers pass the turn's agentId and a
 * subagent (any agentId) sends nothing.
 */
import { getPromptCache1hAllowlist, getPromptCache1hEligible, getSessionId } from '../../bootstrap/state.js'
import { registerCleanup } from '../../utils/cleanupRegistry.js'
import { promptCacheTtlSeconds } from '../../../../../shared/util/cache-countdown.js'
import { SleepTriggers } from '../../../../../runtime/memory/sleep/triggers.js'

let exitHooked = false

function sessionKey(): string {
  return String(getSessionId() ?? '') || 'cli'
}

/** Same TTL rule as the cache countdown (main REPL thread source). */
function cacheTtl(): number {
  return promptCacheTtlSeconds({
    eligible: getPromptCache1hEligible(),
    allowlist: getPromptCache1hAllowlist(),
    source: 'repl_main_thread',
  })
}

/** A main-thread prompt starts: cancel the idle timer, `resume` after an `idle`. */
export function sleepPromptStarted(agentId: string | undefined): void {
  if (agentId) return
  if (!exitHooked) {
    exitHooked = true
    // Graceful shutdown runs cleanups in parallel under a 2 s cap; endAll caps itself at 500 ms.
    registerCleanup(() => SleepTriggers.endAll())
  }
  SleepTriggers.promptArrived(sessionKey(), cacheTtl())
}

/** A main-thread turn finished (or was stopped): start the idle timer. */
export function sleepTurnEnded(agentId: string | undefined): void {
  if (agentId) return
  SleepTriggers.turnEnded(sessionKey(), cacheTtl())
}

/** Compaction starts: fire-and-forget `pre_compact`; returns at once. */
export function sleepPreCompact(agentId: string | undefined): void {
  if (agentId) return
  SleepTriggers.preCompact(sessionKey())
}
