/**
 * Steer signal (Primitives Wave B, lane L1): wakes a `wait_agents` call when
 * a message is queued for the waiting session.
 *
 * SessionPrompt calls `notify` where a message queues behind the running turn
 * (the queuedTurns bump in runLoop). That bump also fires when a background
 * task's completion is delivered to the parent, so `wait_agents` re-reads its
 * targets on a wake and reports "settled" before "steer".
 *
 * Per process, keyed by session id (ids are unique across instances). A
 * notify with nobody waiting is dropped: `wait` only sees notifies made after
 * it was called.
 */
export namespace SteerSignal {
  const waiters = new Map<string, Set<() => void>>()

  /** A message was queued for `sessionID` while its turn runs. Never throws. */
  export function notify(sessionID: string): void {
    const set = waiters.get(sessionID)
    if (!set) return
    waiters.delete(sessionID)
    for (const wake of set) {
      try {
        wake()
      } catch {}
    }
  }

  /** Resolves on the next `notify(sessionID)` after the call; rejects when `abort` fires. */
  export function wait(sessionID: string, abort: AbortSignal): Promise<void> {
    if (abort.aborted) return Promise.reject(abort.reason ?? new DOMException("Aborted", "AbortError"))
    return new Promise<void>((resolve, reject) => {
      const remove = () => {
        const set = waiters.get(sessionID)
        if (!set) return
        set.delete(wake)
        if (!set.size) waiters.delete(sessionID)
      }
      const onAbort = () => {
        remove()
        reject(abort.reason ?? new DOMException("Aborted", "AbortError"))
      }
      const wake = () => {
        abort.removeEventListener("abort", onAbort)
        resolve()
      }
      let set = waiters.get(sessionID)
      if (!set) waiters.set(sessionID, (set = new Set()))
      set.add(wake)
      abort.addEventListener("abort", onAbort, { once: true })
    })
  }

  /** How many waits are open for a session (tests and diagnostics). */
  export function waiting(sessionID: string): number {
    return waiters.get(sessionID)?.size ?? 0
  }
}
