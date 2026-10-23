import path from "path"
import { Log } from "@/shared/util/log"
import { Config } from "@/runtime/context/config/config"
import { ModelJobs } from "@/runtime/context/config/model-jobs"
import { ReviewVerdict } from "./verdict"
import type { Reviewer } from "./index"

const log = Log.create({ service: "review.goal" })

/** Goal round boundary hook. Called by the session loop after a goal
 * continuation was accounted (so budgets were already enforced) and before the
 * next continuation is synthesized. Runs project checks + the reviewer and
 * returns feedback for the next round, plus an escalation model after two
 * rounds in a row with the same failures. Kept separate from GoalEngine so
 * the goal lifecycle itself is unchanged. */
export namespace GoalRoundChecks {
  export const CHECK_TIMEOUT_MS = 10 * 60 * 1000
  const OUTPUT_TAIL = 4000

  export interface CheckResult {
    command: string
    ok: boolean
    output: string
  }

  export interface RoundResult {
    checks: CheckResult[]
    review: ReviewVerdict.Record | null
    /** Text appended to the next continuation; undefined when everything passed. */
    feedback?: string
    /** Model to switch the session to (models.jobs.goalEscalate). */
    escalate?: ModelJobs.ModelRef
    noProgressRounds: number
  }

  interface State {
    fingerprint?: string
    repeats: number
    escalated: boolean
    lastReview?: ReviewVerdict.Record
  }
  const state = new Map<string, State>()

  export function reset(sessionID: string) {
    state.delete(sessionID)
  }

  /** Detect checks: config `goal.checks`, else package.json typecheck/test, else Cargo.toml → cargo check. */
  export async function detectChecks(cwd: string, configured?: string[]): Promise<string[]> {
    if (configured) return configured.filter((c) => c.trim())
    const pkg = await Bun.file(path.join(cwd, "package.json"))
      .json()
      .catch(() => undefined)
    if (pkg?.scripts && typeof pkg.scripts === "object") {
      const exists = (file: string) => Bun.file(path.join(cwd, file)).exists()
      const runner = (await exists("bun.lock")) || (await exists("bun.lockb"))
        ? "bun run"
        : (await exists("pnpm-lock.yaml"))
          ? "pnpm run"
          : (await exists("yarn.lock"))
            ? "yarn"
            : "npm run"
      const checks: string[] = []
      if (typeof pkg.scripts.typecheck === "string") checks.push(`${runner} typecheck`)
      const test = pkg.scripts.test
      if (typeof test === "string" && !/no test specified/i.test(test)) checks.push(`${runner} test`)
      if (checks.length) return checks
    }
    if (await Bun.file(path.join(cwd, "Cargo.toml")).exists()) return ["cargo check"]
    return []
  }

  export async function runCheck(command: string, cwd: string, abort?: AbortSignal): Promise<CheckResult> {
    const proc = Bun.spawn(["sh", "-c", command], { cwd, stdout: "pipe", stderr: "pipe", env: process.env })
    const kill = () => proc.kill()
    abort?.addEventListener("abort", kill)
    const timer = setTimeout(kill, CHECK_TIMEOUT_MS)
    try {
      const [stdout, stderr, code] = await Promise.all([
        new Response(proc.stdout).text(),
        new Response(proc.stderr).text(),
        proc.exited,
      ])
      const output = `${stdout}\n${stderr}`.trim()
      return { command, ok: code === 0, output: output.slice(-OUTPUT_TAIL) }
    } finally {
      clearTimeout(timer)
      abort?.removeEventListener("abort", kill)
    }
  }

  export function feedbackText(checks: CheckResult[], review: ReviewVerdict.Shape | null): string | undefined {
    const failed = checks.filter((c) => !c.ok)
    const reviewBad = review && review.verdict !== "PASS"
    if (!failed.length && !reviewBad) return undefined
    const lines = ["Round checks found problems. Fix them in this round:"]
    for (const check of failed) lines.push("", `\`${check.command}\` failed:`, "```", check.output, "```")
    if (review && reviewBad) {
      lines.push("", `Independent review: ${review.verdict}. ${review.summary}`)
      if (review.findings.length) lines.push(ReviewVerdict.formatFindings(review.findings))
      if (review.unverified.length) lines.push(...review.unverified.map((u) => `- not verified: ${u.what} — ${u.why}`))
    }
    return lines.join("\n")
  }

  export interface Deps {
    review: (input: Reviewer.RunInput) => Promise<ReviewVerdict.Record | null>
    runCheck: typeof runCheck
    goalConfig: () => Promise<{ checks?: string[]; review?: boolean } | undefined>
    escalationModel: () => Promise<ModelJobs.ModelRef | undefined>
  }

  async function defaultDeps(): Promise<Deps> {
    const { Reviewer } = await import("./index")
    return {
      review: Reviewer.runReview,
      runCheck,
      goalConfig: async () => (await Config.get()).goal,
      escalationModel: () => ModelJobs.jobModel("goalEscalate"),
    }
  }

  export async function run(
    input: { sessionID: string; cwd: string; abort?: AbortSignal },
    deps?: Partial<Deps>,
  ): Promise<RoundResult> {
    const d = { ...(await defaultDeps()), ...deps } as Deps
    const cfg = await d.goalConfig().catch(() => undefined)
    const checks: CheckResult[] = []
    for (const command of await detectChecks(input.cwd, cfg?.checks)) {
      if (input.abort?.aborted) break
      checks.push(await d.runCheck(command, input.cwd, input.abort))
    }
    let review: ReviewVerdict.Record | null = null
    if (cfg?.review !== false && !input.abort?.aborted) {
      const previous = state.get(input.sessionID)?.lastReview
      review = await d
        .review({ sessionID: input.sessionID, cwd: input.cwd, abort: input.abort, previous })
        .catch((error) => {
          log.warn("goal round review failed", { sessionID: input.sessionID, error })
          return null
        })
    }

    const failedChecks = checks.filter((c) => !c.ok).map((c) => `check:${c.command}`)
    const failing = failedChecks.length > 0 || (review !== null && review.verdict !== "PASS")
    const current = state.get(input.sessionID) ?? { repeats: 0, escalated: false }
    if (!failing) {
      state.set(input.sessionID, { repeats: 0, escalated: current.escalated, lastReview: review ?? current.lastReview })
      return { checks, review, noProgressRounds: 0 }
    }
    const fingerprint = ReviewVerdict.fingerprint(
      review ?? { verdict: "PASS", findings: [], unverified: [] },
      failedChecks,
    )
    const repeats = current.fingerprint === fingerprint ? current.repeats + 1 : 0
    let escalate: ModelJobs.ModelRef | undefined
    let escalated = current.escalated
    // Two rounds in a row with the same failures = no progress → escalate once.
    if (repeats >= 1 && !escalated) {
      escalate = await d.escalationModel().catch(() => undefined)
      if (escalate) escalated = true
    }
    state.set(input.sessionID, { fingerprint, repeats, escalated, lastReview: review ?? current.lastReview })
    return { checks, review, feedback: feedbackText(checks, review), escalate, noProgressRounds: repeats }
  }
}
