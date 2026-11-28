// Replay runner for the Gizzi eval loop (PLAN.md D5, CONTRACT.md section 5).
// Replay uses no model: it grades the stored trace of each case. It tests the
// grading machinery, not prompt changes.
import crypto from "crypto"
import fs from "fs/promises"
import path from "path"
import { SessionTrace } from "@/runtime/session/trace"
import { EvalStore } from "./store"
import { gradeAll, type Grade, type GraderSpec as RustGrader, type Trajectory as RustTrajectory } from "./graders"
import { toTrajectory } from "./trajectory"
import { bootstrapCi95, rate } from "./stats"
import { REPORT_SCHEMA, Suite, type Report } from "./schema"

export namespace EvalRunner {
  type PerCaseReplay = {
    id: string
    label: "good" | "bad"
    pass: boolean
    expected_fail?: boolean
    grades: Grade[]
    // Fields the report's per_case rows carry.
    passes: number
    attempts: number
    cost: number
    tokens: number
    ms: number
  }

  async function readJson(file: string): Promise<any | undefined> {
    try {
      return JSON.parse(await fs.readFile(file, "utf8"))
    } catch {
      return undefined
    }
  }

  export async function replay(input: {
    suiteDir?: string
    suite?: string
    caseIds?: string[]
    seed?: number
  }): Promise<Report> {
    const seed = input.seed ?? 1
    const dir = input.suiteDir ?? (input.suite ? path.join(EvalStore.root(), "suites", input.suite) : undefined)
    if (!dir) throw new Error("replay needs a suite or a suiteDir")
    const parsed = Suite.safeParse(await readJson(path.join(dir, "suite.json")))
    if (!parsed.success) throw new Error(`No valid suite.json in ${dir}`)
    const suite = parsed.data
    const want = input.caseIds ? new Set(input.caseIds) : undefined

    const rows: PerCaseReplay[] = []
    const failed: string[] = []
    const high: string[] = []
    let skipped = 0
    let selected = 0
    for (const ref of suite.cases) {
      if (want && !want.has(ref.id)) continue
      selected++
      const caseFile = path.join(dir, ref.source.path)
      const c = ref.source.kind === "session" ? await readJson(caseFile) : undefined
      const trace = c ? await readJson(path.join(path.dirname(caseFile), "trace.json")) : undefined
      if (!c || !Array.isArray(trace) || !Array.isArray(c.graders)) {
        skipped++
        continue
      }
      const t = toTrajectory(trace as SessionTrace.Entry[])
      const graded = gradeAll(t as unknown as RustTrajectory, c.graders as RustGrader[])
      const label: "good" | "bad" = c.label === "bad" ? "bad" : "good"
      // A bad case describes a failure: it passes in replay while its graders still fail on the recording.
      const pass = label === "good" ? graded.pass : !graded.pass
      rows.push({
        id: ref.id,
        label,
        pass,
        ...(label === "bad" ? { expected_fail: true } : {}),
        grades: graded.grades,
        passes: pass ? 1 : 0,
        attempts: 1,
        cost: t.cost_usd,
        tokens: t.tokens,
        ms: t.latency_ms,
      })
      if (!pass) {
        failed.push(ref.id)
        if (c.stakes === "high") high.push(ref.id)
      }
    }

    const outcomes = rows.map((r) => r.pass)
    const passed = outcomes.filter(Boolean).length
    const ci = bootstrapCi95(outcomes, seed)
    return {
      schema: REPORT_SCHEMA,
      seed,
      generated_by: "gizzi eval replay",
      suites: {
        [suite.id]: {
          cases: selected,
          graded: rows.length,
          skipped,
          passed,
          pass_rate: rate(outcomes),
          attempt_pass_rate: rate(outcomes),
          ci95: ci,
          high_stakes_failures: high,
          failed_cases: failed,
        },
      },
      extra: {
        mode: "replay",
        ci95: ci,
        per_case: rows.map((r) => ({
          id: r.id,
          label: r.label,
          pass: r.pass,
          ...(r.expected_fail ? { expected_fail: true } : {}),
          grades: r.grades,
          passes: r.passes,
          attempts: r.attempts,
          cost: r.cost,
          tokens: r.tokens,
          ms: r.ms,
        })),
        skipped,
      },
    } as unknown as Report
  }

  /** Save a report under runs/<run_id>/report.json and stamp each store case with its result. */
  export async function writeRun(report: Report): Promise<string> {
    const run_id = `run_${Date.now().toString(36)}${crypto.randomBytes(4).toString("hex")}`
    const dir = path.join(EvalStore.root(), "runs", run_id)
    await fs.mkdir(dir, { recursive: true })
    await fs.writeFile(path.join(dir, "report.json"), JSON.stringify(report, null, 2))
    const at = Date.now()
    for (const row of report.extra?.per_case ?? []) {
      // Cases of a suiteDir outside the store are not found; that is fine.
      await EvalStore.setLastResult(row.id, row.pass ? "pass" : "fail", at).catch(() => {})
    }
    return run_id
  }
}
