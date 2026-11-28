// Live eval run jobs: an in-process queue (PLAN.md D10, CONTRACT.md section 6).
// A job runs the cases of a suite one by one with runLive, one attempt at a time,
// then writes a report in the same shape replay writes (extra.mode = "live").
import crypto from "crypto"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { EvalStore } from "./store"
import { EvalRunner } from "./runner"
import { runLive, passRule, type Attempt, type LiveOptions } from "./live"
import { bootstrapCi95, rate } from "./stats"
import { getEstimate, loadSuite, matches } from "./estimate"
import { REPORT_SCHEMA, type PerCase, type Report } from "./schema"

export const MAX_RUNS = 50
/** Price check: stop when the probe says the run costs more than this times the estimate. */
export const PRICE_CHECK_FACTOR = 1.5

export type RunState = "queued" | "running" | "done" | "failed" | "cancelled"

export interface RunView {
  run_id: string
  mode: "replay" | "live"
  suite: string
  state: RunState
  progress: { done: number; total: number }
  report?: Report
  error?: string
  started_at: number
  finished_at?: number
}

export class JobError extends Error {
  constructor(
    public readonly code: "estimate_required" | "run_not_found",
    message: string,
  ) {
    super(message)
    this.name = "JobError"
  }
}

export interface StartInput {
  suite?: string
  suiteDir?: string
  caseIds?: string[]
  k?: number
  model?: string
  estimateId?: string
  seed?: number
  /** Test and tool hooks passed to runLive (stub binary, trajectory source, timeouts). */
  live?: Partial<Pick<LiveOptions, "gizziArgv" | "trajectoryFor" | "timeoutMs" | "testsTimeoutMs">>
  now?: () => number
}

interface Job {
  view: RunView
  abort: AbortController
  done: Promise<void>
}

/** Defaults for runLive in every job. Tests point this at a stub binary. */
export const liveDefaults: NonNullable<StartInput["live"]> = {}

const jobs = new Map<string, Job>()

const runsDir = () => path.join(EvalStore.root(), "runs")

function newId() {
  return `run_${Date.now().toString(36)}${crypto.randomBytes(4).toString("hex")}`
}

async function persist(v: RunView) {
  const dir = path.join(runsDir(), v.run_id)
  await fs.mkdir(dir, { recursive: true })
  await fs.writeFile(path.join(dir, "run.json"), JSON.stringify(v, null, 2)).catch(() => {})
}

function prune() {
  const finished = [...jobs.values()].filter((j) => j.view.state !== "queued" && j.view.state !== "running")
  finished.sort((a, b) => b.view.started_at - a.view.started_at)
  for (const j of finished.slice(MAX_RUNS)) jobs.delete(j.view.run_id)
}

export async function start(input: StartInput): Promise<{ run_id: string }> {
  const now = input.now ?? Date.now
  const k = Math.max(1, Math.floor(input.k ?? 1))
  const loaded = await loadSuite(input)
  const est = getEstimate(input.estimateId, now())
  if (!est || !matches(est, { suite: loaded.suite.id, caseIds: loaded.cases.map((c) => c.id), k, model: input.model })) {
    throw new JobError("estimate_required", "Get a price first.")
  }

  const run_id = newId()
  const abort = new AbortController()
  const view: RunView = {
    run_id,
    mode: "live",
    suite: loaded.suite.id,
    state: "queued",
    progress: { done: 0, total: loaded.cases.length * k },
    started_at: now(),
  }
  const job: Job = { view, abort, done: Promise.resolve() }
  jobs.set(run_id, job)
  prune()
  await persist(view)

  job.done = (async () => {
    view.state = "running"
    await persist(view)
    try {
      const rows: PerCase[] = []
      const failed: string[] = []
      const high: string[] = []
      const outcomes: boolean[] = []
      const outRoot = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-eval-run-"))
      let probe: { perAttempt: number; tokens: number } | undefined
      for (const lc of loaded.cases) {
        const attempts: Attempt[] = []
        for (let n = 1; n <= k; n++) {
          if (abort.signal.aborted) break
          const r = await runLive({
            ...liveDefaults,
            ...input.live,
            case: lc.case,
            caseDir: lc.caseDir,
            k: 1,
            model: input.model,
            signal: abort.signal,
            outDir: path.join(outRoot, lc.id, `a${n}`),
          })
          if (abort.signal.aborted) break
          attempts.push(...r.attempts.map((a) => ({ ...a, n })))
          view.progress.done++

          // Price check. With no price in the table the first attempt runs alone and its real
          // charge is read from the trajectory. Cost per attempt times all attempts must stay
          // within 1.5x of what the estimate implies: usd_est when there is one, otherwise the
          // probe's real price per token times tokens_est (so a probe that burns over 1.5x the
          // expected tokens stops the run). Over that, the job fails with "price_check".
          if (est.probe_required && !probe && attempts.length === 1 && lc === loaded.cases[0] && n === 1) {
            const a = attempts[0]!
            probe = { perAttempt: a.trajectory.cost_usd, tokens: a.trajectory.tokens }
            const extrapolated = probe.perAttempt * est.attempts
            const expected = est.usd_est ?? (probe.tokens > 0 ? (probe.perAttempt / probe.tokens) * est.tokens_est : extrapolated)
            if (extrapolated > PRICE_CHECK_FACTOR * expected) {
              view.state = "failed"
              view.error = "price_check"
              view.finished_at = now()
              await persist(view)
              return
            }
          }
        }
        if (abort.signal.aborted) break
        const passes = attempts.filter((a) => a.pass).length
        const pass = passRule(lc.case.pass_rule, attempts.map((a) => a.pass))
        outcomes.push(pass)
        if (!pass) {
          failed.push(lc.id)
          if ((lc.case as any).stakes === "high") high.push(lc.id)
        }
        rows.push({
          id: lc.id,
          passes,
          attempts: attempts.length,
          pass,
          cost: attempts.reduce((s, a) => s + a.cost_usd, 0),
          tokens: attempts.reduce((s, a) => s + a.tokens, 0),
          ms: attempts.reduce((s, a) => s + a.ms, 0),
        })
      }
      if (abort.signal.aborted) {
        view.state = "cancelled"
        view.finished_at = now()
        await persist(view)
        await fs.rm(outRoot, { recursive: true, force: true }).catch(() => {})
        return
      }

      const seed = input.seed ?? 1
      const ci = bootstrapCi95(outcomes, seed)
      const attemptsAll = rows.reduce((s, r) => s + r.attempts, 0)
      const report = {
        schema: REPORT_SCHEMA,
        seed,
        generated_by: "gizzi eval live",
        suites: {
          [loaded.suite.id]: {
            cases: loaded.cases.length,
            graded: rows.length,
            skipped: 0,
            passed: outcomes.filter(Boolean).length,
            pass_rate: rate(outcomes),
            attempt_pass_rate: attemptsAll ? rows.reduce((s, r) => s + r.passes, 0) / attemptsAll : null,
            ci95: ci,
            high_stakes_failures: high,
            failed_cases: failed,
          },
        },
        extra: { mode: "live", brain: input.model ?? est.model ?? "default", per_case: rows, ci95: ci },
      } as unknown as Report
      // writeRun names its own folder and stamps each case with its result. Keep that, under this job's id.
      const written = await EvalRunner.writeRun(report)
      await fs.rm(path.join(runsDir(), run_id, "report.json"), { force: true })
      await fs.rename(path.join(runsDir(), written, "report.json"), path.join(runsDir(), run_id, "report.json"))
      await fs.rm(path.join(runsDir(), written), { recursive: true, force: true })
      view.report = report
      view.state = "done"
      view.finished_at = now()
      await persist(view)
      await fs.rm(outRoot, { recursive: true, force: true }).catch(() => {})
    } catch (e) {
      view.state = "failed"
      view.error = String(e instanceof Error ? e.message : e)
      view.finished_at = now()
      await persist(view)
    }
  })()
  return { run_id }
}

/** Resolves when the job has stopped. For tests. */
export async function wait(run_id: string) {
  await jobs.get(run_id)?.done
}

export function cancel(run_id: string): boolean {
  const job = jobs.get(run_id)
  if (!job) return false
  if (job.view.state === "queued" || job.view.state === "running") job.abort.abort()
  return true
}

/** Save a finished replay report as a run so it shows in the run list. */
export async function recordReplay(report: Report, suite: string): Promise<string> {
  const run_id = await EvalRunner.writeRun(report)
  const t = Date.now()
  await persist({ run_id, mode: "replay", suite, state: "done", progress: { done: 1, total: 1 }, started_at: t, finished_at: t })
  return run_id
}

async function readDisk(run_id: string): Promise<RunView | undefined> {
  if (!/^run_[0-9a-z]+$/.test(run_id)) return undefined
  const dir = path.join(runsDir(), run_id)
  const read = async (f: string) => JSON.parse(await fs.readFile(path.join(dir, f), "utf8"))
  const report: Report | undefined = await read("report.json").catch(() => undefined)
  const meta: RunView | undefined = await read("run.json").catch(() => undefined)
  if (!meta && !report) return undefined
  const t = (await fs.stat(dir).catch(() => undefined))?.mtimeMs ?? 0
  const v: RunView = meta ?? {
    run_id,
    mode: report!.extra?.mode ?? "replay",
    suite: Object.keys(report!.suites)[0] ?? "",
    state: "done",
    progress: { done: 1, total: 1 },
    started_at: t,
    finished_at: t,
  }
  return report ? { ...v, report } : v
}

export async function get(run_id: string): Promise<RunView> {
  const v = jobs.get(run_id)?.view ?? (await readDisk(run_id))
  if (!v) throw new JobError("run_not_found", "That run was not found.")
  return v
}

export async function list(suite?: string): Promise<RunView[]> {
  const ids = (await fs.readdir(runsDir()).catch(() => [])) as string[]
  const byId = new Map<string, RunView>()
  for (const id of ids) {
    const v = await readDisk(id)
    if (v) byId.set(id, v)
  }
  for (const [id, j] of jobs) byId.set(id, j.view)
  return [...byId.values()]
    .filter((v) => !suite || v.suite === suite)
    .sort((a, b) => b.started_at - a.started_at)
    .slice(0, MAX_RUNS)
}

export const passRateOf = (v: RunView): number | undefined => {
  const s = v.report ? Object.values(v.report.suites)[0] : undefined
  return s?.pass_rate ?? undefined
}

/** The newest finished run of a suite, for GET /suites. */
export async function lastRun(suite: string): Promise<{ run_id: string; pass_rate: number | null; at: number } | undefined> {
  const v = (await list(suite)).find((r) => r.state === "done" && r.report)
  if (!v) return undefined
  return { run_id: v.run_id, pass_rate: passRateOf(v) ?? null, at: v.finished_at ?? v.started_at }
}
