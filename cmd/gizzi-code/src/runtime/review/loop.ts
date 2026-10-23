import { Bus } from "@/shared/bus"
import { Identifier } from "@/shared/id/id"
import { Log } from "@/shared/util/log"
import { ReviewEvent } from "./events"
import { ReviewVerdict } from "./verdict"
import type { Reviewer } from "./index"

const log = Log.create({ service: "review.loop" })

/** Review → fix → re-review until PASS, the round limit, an error, or abort. */
export namespace ReviewLoop {
  export const DEFAULT_ROUNDS = 5
  export const MAX_ROUNDS = 12

  export type Stopped = "pass" | "limit" | "error" | "abort" | "nothing"

  export interface Result {
    rounds: number
    stopped: Stopped
    verdict: ReviewVerdict.Record | null
    history: ReviewVerdict.Record[]
    error?: string
  }

  export interface Deps {
    review: (input: Reviewer.RunInput) => Promise<ReviewVerdict.Record | null>
    /** Send a user turn to the coder's session and resolve when that turn is done.
     * Returns "aborted" when the turn was cancelled. */
    sendFix: (sessionID: string, text: string, signal: AbortSignal) => Promise<"done" | "aborted">
    publish: typeof Bus.publish
  }

  const active = new Map<string, AbortController>()

  /** Abort a running loop for this session (also aborted by session abort). */
  export function cancel(sessionID: string) {
    active.get(sessionID)?.abort()
  }

  export function running(sessionID: string) {
    return active.has(sessionID)
  }

  export function clampRounds(rounds?: number) {
    if (!rounds || !Number.isFinite(rounds)) return DEFAULT_ROUNDS
    return Math.max(1, Math.min(MAX_ROUNDS, Math.trunc(rounds)))
  }

  export function fixPrompt(verdict: ReviewVerdict.Shape): string {
    const lines = ["Fix these review findings:"]
    if (verdict.findings.length) lines.push(ReviewVerdict.formatFindings(verdict.findings))
    if (verdict.unverified.length) {
      lines.push(
        "",
        "The reviewer could not verify the following. Make them verifiable (add or run a check) or fix them:",
        ...verdict.unverified.map((u) => `- ${u.what} — ${u.why}`),
      )
    }
    lines.push("", "An independent reviewer will check again after this turn.")
    return lines.join("\n")
  }

  async function defaultSendFix(sessionID: string, text: string, signal: AbortSignal): Promise<"done" | "aborted"> {
    const { SessionPrompt } = await import("@/runtime/session/prompt")
    const onAbort = () => SessionPrompt.cancel(sessionID)
    signal.addEventListener("abort", onAbort)
    try {
      // Same path the server's POST /session/:id/message route uses.
      const result = await SessionPrompt.prompt({
        sessionID,
        messageID: Identifier.ascending("message"),
        parts: [{ type: "text", text }],
      } as any)
      const error = (result.info as any)?.error
      if (signal.aborted || error?.name === "MessageAbortedError") return "aborted"
      if (error) throw new Error(error.data?.message ?? error.name ?? "Fix turn failed")
      return "done"
    } finally {
      signal.removeEventListener("abort", onAbort)
    }
  }

  async function defaultDeps(): Promise<Deps> {
    const { Reviewer } = await import("./index")
    return { review: Reviewer.runReview, sendFix: defaultSendFix, publish: Bus.publish }
  }

  export async function reviewUntilPass(
    input: { sessionID: string; cwd: string; maxRounds?: number; instruction?: string; abort?: AbortSignal },
    deps?: Partial<Deps>,
  ): Promise<Result> {
    const resolved = { ...(await defaultDeps()), ...deps } as Deps
    const maxRounds = clampRounds(input.maxRounds)
    const controller = new AbortController()
    const onOuterAbort = () => controller.abort()
    input.abort?.addEventListener("abort", onOuterAbort)
    if (input.abort?.aborted) controller.abort()
    active.get(input.sessionID)?.abort()
    active.set(input.sessionID, controller)

    const history: ReviewVerdict.Record[] = []
    let verdict: ReviewVerdict.Record | null = null
    let rounds = 0
    let stopped: Stopped = "limit"
    let error: string | undefined

    try {
      for (let round = 1; round <= maxRounds; round++) {
        if (controller.signal.aborted) {
          stopped = "abort"
          break
        }
        rounds = round
        verdict = await resolved.review({
          sessionID: input.sessionID,
          cwd: input.cwd,
          instruction: input.instruction,
          previous: verdict ?? undefined,
          round,
          maxRounds,
          abort: controller.signal,
        })
        if (!verdict) {
          stopped = "nothing"
          break
        }
        history.push(verdict)
        if (verdict.verdict === "PASS") {
          stopped = "pass"
          break
        }
        if (round === maxRounds) {
          stopped = "limit"
          break
        }
        if (controller.signal.aborted) {
          stopped = "abort"
          break
        }
        resolved.publish(ReviewEvent.Fixing, { sessionID: input.sessionID, round, maxRounds })
        const outcome = await resolved.sendFix(input.sessionID, fixPrompt(verdict), controller.signal)
        if (outcome === "aborted") {
          stopped = "abort"
          break
        }
      }
    } catch (err) {
      if (controller.signal.aborted) stopped = "abort"
      else {
        stopped = "error"
        error = err instanceof Error ? err.message : String(err)
        log.warn("review loop failed", { sessionID: input.sessionID, error })
      }
    } finally {
      input.abort?.removeEventListener("abort", onOuterAbort)
      if (active.get(input.sessionID) === controller) active.delete(input.sessionID)
    }

    resolved.publish(ReviewEvent.LoopFinished, {
      sessionID: input.sessionID,
      rounds,
      stopped,
      verdict: verdict?.verdict ?? null,
      ...(error ? { error } : {}),
    })
    return { rounds, stopped, verdict, history, ...(error ? { error } : {}) }
  }
}
