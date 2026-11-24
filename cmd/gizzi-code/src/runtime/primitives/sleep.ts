/**
 * sleep: a short in-turn wait that ends early when the user writes again.
 *
 * Shared core for the `sleep` tool. The wait ends on the first of:
 *   timer       the requested seconds passed
 *   user_input  a new user message was queued for this session
 *   cancelled   the turn was aborted
 * `slept_seconds` is the real time waited (rounded to 0.1 s), not the request.
 *
 * The user-input signal is injected (`watchUserInput`) so tests need no bus;
 * the tool passes a Bus subscription to `message.updated` for user messages
 * other than the ones the turn already had.
 */
export namespace Sleep {
  export const MIN_SECONDS = 1
  export const MAX_SECONDS = 900
  export const MAX_REASON_CHARS = 120

  export type Woke = "timer" | "user_input" | "cancelled"
  export type Result = { slept_seconds: number; woke: Woke }

  /** Start watching; call `wake` on a new user message; return the stop function. */
  export type WatchUserInput = (wake: () => void) => () => void

  export function clampSeconds(seconds: number): number {
    return Math.min(MAX_SECONDS, Math.max(MIN_SECONDS, Math.round(seconds)))
  }

  export async function wait(input: {
    seconds: number
    abort?: AbortSignal
    watchUserInput?: WatchUserInput
    now?: () => number
  }): Promise<Result> {
    const now = input.now ?? Date.now
    const start = now()
    const done = (woke: Woke): Result => ({
      slept_seconds: Math.round(((now() - start) / 1000) * 10) / 10,
      woke,
    })
    if (input.abort?.aborted) return done("cancelled")

    return new Promise<Result>((resolve) => {
      let stopWatch: (() => void) | undefined
      let timer: ReturnType<typeof setTimeout> | undefined
      const finish = (woke: Woke) => {
        if (timer) clearTimeout(timer)
        stopWatch?.()
        input.abort?.removeEventListener("abort", onAbort)
        resolve(done(woke))
      }
      const onAbort = () => finish("cancelled")
      input.abort?.addEventListener("abort", onAbort, { once: true })
      timer = setTimeout(() => finish("timer"), clampSeconds(input.seconds) * 1000)
      stopWatch = input.watchUserInput?.(() => finish("user_input"))
    })
  }
}
