// gizzi eval case ... (eval loop CONTRACT.md section 8). Handlers are plain functions so tests can call them.
import type { Argv } from "yargs"
import fs from "fs/promises"
import path from "path"
import { cmd } from "@/cli/commands/cmd"
import { bootstrap } from "@/cli/bootstrap"
import { EvalStore } from "@/runtime/eval/store"
import { redactForExport, redactPrivate, scanWorkspaceTar, type TraceEntry } from "@/runtime/eval/redact"
import { EvalRunner } from "@/runtime/eval/runner"
import { estimate as estimateRun, loadSuite, type Estimate } from "@/runtime/eval/estimate"
import * as EvalJobs from "@/runtime/eval/jobs"
import { mulberry32 } from "@/runtime/eval/stats"
import { recordedTrajectory } from "@/runtime/eval/trajectory"
import { SessionTrace } from "@/runtime/session/trace"

type Out = (line: string) => void

/** CLI-only problems (bad flags, blocked export). Engine problems stay EvalStore.EvalError. */
export class EvalCliError extends Error {}
const stdout: Out = (line) => console.log(line)

export function needsCheckReason(label: "good" | "bad", agrees: boolean): string {
  if (agrees) return ""
  return label === "good"
    ? "the saved chat did not pass its own checks"
    : "the checks pass this chat, so add a check that says what went wrong"
}

export async function runSave(
  args: { sessionID: string; good?: boolean; bad?: boolean; note?: string; through?: string; suite?: string; json?: boolean },
  out: Out = stdout,
) {
  if (!!args.good === !!args.bad) throw new EvalCliError("Pick one: --good or --bad.")
  const saved = await EvalStore.saveCase({
    sessionID: args.sessionID,
    label: args.good ? "good" : "bad",
    note: args.note,
    throughMessageID: args.through,
    suite: args.suite,
    surface: "cli",
  })
  if (args.json) return out(JSON.stringify(saved, null, 2))
  out(`Saved test case ${saved.case.id} (${saved.case.suite})`)
  out(
    saved.self_test.agrees_with_label
      ? "Self-test: agrees"
      : `Needs a check: ${needsCheckReason(saved.case.label, false)}`,
  )
}

export async function runList(args: { session?: string; suite?: string; json?: boolean }, out: Out = stdout) {
  const cases = await EvalStore.listCases({ sessionID: args.session, suite: args.suite })
  if (args.json) return out(JSON.stringify({ cases }, null, 2))
  if (cases.length === 0) return out("No test cases saved yet.")
  const rows = [["ID", "LABEL", "TITLE", "LAST RESULT", "NEEDS CHECK"]].concat(
    cases.map((c) => [c.id, c.label, c.title, c.last_result ?? "-", c.needs_check ? "yes" : "no"]),
  )
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)))
  for (const r of rows) out(r.map((v, i) => v.padEnd(widths[i]!)).join("  ").trimEnd())
}

export async function runShow(args: { id: string; json?: boolean }, out: Out = stdout) {
  const c = await EvalStore.getCase(args.id)
  if (args.json) return out(JSON.stringify({ case: c }, null, 2))
  out(`${c.id} (${c.suite})`)
  out(`Title: ${c.title}`)
  out(`Label: ${c.label === "good" ? "went well" : "went wrong"}`)
  if (c.note) out(`Note: ${c.note}`)
  out(`Needs a check: ${c.needs_check ? "yes" : "no"}`)
  out(`Saved: ${new Date(c.origin.saved_at).toISOString()} from ${c.origin.surface}`)
  out(`Workspace: ${c.workspace.kind}`)
  out(`Turns: ${c.turns.length}`)
}

export async function runRemove(args: { id: string }, out: Out = stdout) {
  await EvalStore.deleteCase(args.id)
  out(`Removed test case ${args.id}`)
}

export async function runExport(args: { id: string; out: string }, out: Out = stdout) {
  const c = await EvalStore.getCase(args.id)
  const src = path.join(EvalStore.root(), "suites", c.suite, "cases", c.id)
  const tar = path.join(src, "workspace.tar.gz")
  const hasTar = await fs.stat(tar).then(() => true, () => false)
  if (hasTar) {
    const { hits } = await scanWorkspaceTar(tar)
    if (hits.length > 0) {
      const names = hits.map((h) => `${h.file} (${h.kind})`).join(", ")
      throw new EvalCliError(`Export stopped: ${hits.length} files look like secrets: ${names}`)
    }
  }
  const trace = JSON.parse(await fs.readFile(path.join(src, "trace.json"), "utf8")) as TraceEntry[]
  const red = redactForExport(trace, c.turns)
  const dest = path.resolve(args.out, c.id)
  await fs.mkdir(dest, { recursive: true })
  const caseFile = { ...c, turns: red.turns.map((t) => ({ role: "user" as const, text: t.text })) }
  await fs.writeFile(path.join(dest, "case.json"), JSON.stringify(caseFile, null, 2))
  await fs.writeFile(path.join(dest, "trace.json"), JSON.stringify(red.entries))
  if (hasTar) await fs.copyFile(tar, path.join(dest, "workspace.tar.gz"))
  await fs.writeFile(path.join(dest, "redaction_report.json"), JSON.stringify(red.report, null, 2))
  out(`Exported test case ${c.id} to ${dest}`)
}

// ---------------------------------------------------------------------------
// gizzi eval run | estimate | report | baseline write (CONTRACT.md section 8)
// ---------------------------------------------------------------------------

export const REPORT_SCHEMA = "allternit.eval_report.v1"
export const BASELINE_SCHEMA = "allternit.eval_baseline.v1"

// Copy of compareReports and baselineFrom from scripts/agency-eval-gate.mjs. The CLI is typechecked
// and bundled without allowJs, so it cannot import that .mjs. test/eval/cli.test.ts imports the
// script and asserts both copies give the same answer, so they cannot drift apart unnoticed.
export function compareReports(report: any, baseline: any, opts: { maxDropPp?: number } = {}) {
  if (report?.schema !== REPORT_SCHEMA) throw new Error(`report schema must be ${REPORT_SCHEMA}`)
  if (baseline?.schema !== BASELINE_SCHEMA) throw new Error(`baseline schema must be ${BASELINE_SCHEMA}`)
  const limit = opts.maxDropPp ?? baseline.max_drop_pp ?? 2
  const failures: string[] = []
  const notes: string[] = []
  const suites = report.suites ?? {}
  for (const [name, base] of Object.entries<any>(baseline.suites ?? {})) {
    const cur = suites[name]
    if (!cur) {
      failures.push(`${name}: missing from the report`)
      continue
    }
    if (typeof cur.pass_rate !== "number") {
      failures.push(`${name}: nothing graded`)
      continue
    }
    const drop = (base.pass_rate - cur.pass_rate) * 100
    if (drop > limit + 1e-9) {
      failures.push(
        `${name}: pass rate ${(cur.pass_rate * 100).toFixed(1)}% is ${drop.toFixed(1)} pp below the baseline ${(base.pass_rate * 100).toFixed(1)}% (allowed ${limit} pp); failed: ${(cur.failed_cases ?? []).join(", ") || "n/a"}`,
      )
    } else if (cur.pass_rate > base.pass_rate + 1e-9) {
      notes.push(`${name}: pass rate improved to ${(cur.pass_rate * 100).toFixed(1)}% (refresh the baseline with baseline write)`)
    }
  }
  for (const [name, cur] of Object.entries<any>(suites)) {
    const high = cur.high_stakes_failures ?? []
    if (high.length > 0) failures.push(`${name}: high-stakes (all-of-k) case(s) failed: ${high.join(", ")}`)
    if (!(name in (baseline.suites ?? {}))) notes.push(`${name}: not in the baseline (not gated on pass rate yet)`)
  }
  return { ok: failures.length === 0, failures, notes }
}

export function baselineFrom(report: any, previous?: any) {
  const suites: Record<string, { pass_rate: number; graded: number }> = {}
  for (const [name, cur] of Object.entries<any>(report.suites ?? {})) {
    if (typeof cur.pass_rate === "number") suites[name] = { pass_rate: cur.pass_rate, graded: cur.graded }
  }
  return {
    schema: BASELINE_SCHEMA,
    description: previous?.description ?? "Pass-rate baseline for gizzi eval compare and scripts/gizzi-eval-gate.mjs.",
    max_drop_pp: previous?.max_drop_pp ?? 2.0,
    suites,
    // Per-case outcomes so a compare can name each case that flipped. compareReports ignores this.
    extra: {
      per_case: (report.extra?.per_case ?? []).map((r: any) => ({
        id: r.id,
        pass: !!r.pass,
        ...(r.split ? { split: r.split } : {}),
        ...(r.title ? { title: r.title } : {}),
        ...(r.note ? { note: r.note } : {}),
        ...(r.grader_details ? { grader_details: r.grader_details } : {}),
      })),
    },
  }
}

/** Cases that pass in one report and fail in the other. Needs per_case on both sides. */
export function flippedCases(report: any, baseline: any): { id: string; was: boolean; now: boolean }[] {
  const was = new Map<string, boolean>((baseline?.extra?.per_case ?? []).map((r: any) => [r.id, !!r.pass]))
  const out: { id: string; was: boolean; now: boolean }[] = []
  for (const r of report?.extra?.per_case ?? []) {
    if (was.has(r.id) && was.get(r.id) !== !!r.pass) out.push({ id: r.id, was: was.get(r.id)!, now: !!r.pass })
  }
  return out
}

export type Split = "dev" | "holdout" | "all"

export interface SelectArgs {
  suite?: string
  suiteDir?: string
  split?: Split
  fraction?: number
  seed?: number
  /** Case ids to drop after the split and fraction picks. */
  skipCases?: string[]
}

/** What the report needs to say about each case, read from case.json. */
export interface CaseInfo {
  split: string
  title: string
  note?: string
}

/** A seeded sample: the same ids come back for the same ids, fraction and seed. Sorted result. */
export function sampleIds(ids: string[], fraction: number, seed: number): string[] {
  const sorted = [...ids].sort()
  if (!(fraction < 1)) return sorted
  const n = Math.max(1, Math.ceil(sorted.length * fraction))
  const rnd = mulberry32(seed)
  for (let i = sorted.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1))
    ;[sorted[i], sorted[j]] = [sorted[j]!, sorted[i]!]
  }
  return sorted.slice(0, n).sort()
}

/**
 * Picks the case ids for a run. Returns undefined ids when nothing narrows the suite, so a replay
 * of the whole suite also counts cases that are skipped. The engine only applies fraction to replay,
 * so the CLI picks the ids for both modes.
 */
export async function selectCases(a: SelectArgs): Promise<{ ids?: string[]; labels: Map<string, string>; info: Map<string, CaseInfo> }> {
  if (!a.suite && !a.suiteDir) throw new EvalCliError("Pick a suite with --suite or --suite-dir.")
  const split = a.split ?? "all"
  if (!["dev", "holdout", "all"].includes(split)) throw new EvalCliError("--split must be dev, holdout or all.")
  const fraction = a.fraction ?? 1
  if (!(fraction > 0 && fraction <= 1)) throw new EvalCliError("--fraction must be above 0 and at most 1.")
  const loaded = await loadSuite({ suite: a.suite, suiteDir: a.suiteDir }).catch((e) => {
    throw new EvalCliError(e instanceof Error ? e.message : String(e))
  })
  const labels = new Map<string, string>(loaded.cases.map((c) => [c.id, c.case.label]))
  const info = new Map<string, CaseInfo>(
    loaded.cases.map((c) => [c.id, { split: (c.case as any).split ?? "dev", title: c.case.title, ...(c.case.note ? { note: c.case.note } : {}) }]),
  )
  const skip = new Set(a.skipCases ?? [])
  if (split === "all" && fraction >= 1 && skip.size === 0) return { labels, info }
  const pool = loaded.cases.filter((c) => split === "all" || c.case.split === split).map((c) => c.id)
  if (pool.length === 0) throw new EvalCliError(`No cases in the ${split} split.`)
  const ids = sampleIds(pool, fraction, a.seed ?? 1).filter((id) => !skip.has(id))
  if (ids.length === 0) throw new EvalCliError("Every selected case was skipped.")
  return { ids, labels, info }
}

/** Adds split, title, note and grader_details to each per_case row. Cost is already USD (trace cost_usd). */
export function enrichReport(report: any, info: Map<string, CaseInfo>) {
  const rows = report?.extra?.per_case
  if (!Array.isArray(rows)) return report
  for (const r of rows) {
    const i = info.get(r.id)
    r.split = i?.split ?? "dev"
    r.title = i?.title ?? r.id
    if (i?.note) r.note = i.note
    r.grader_details = (r.grades ?? []).filter((g: any) => !g.pass).map((g: any) => ({ type: g.type, detail: g.detail }))
  }
  return report
}

const fmtTokens = (n: number) => n.toLocaleString("en-US")

export function priceText(e: Estimate): string {
  if (e.billing === "plan") return "Uses your plan"
  if (e.billing === "metered" && e.usd_est !== undefined) return `About $${e.usd_est.toFixed(2)}`
  return "price unknown, one run first"
}

export function estimateLine(e: Estimate): string {
  return `${e.attempts} runs · about ${fmtTokens(e.tokens_est)} tokens · about ${e.minutes_est} min · ${priceText(e)}`
}

function printTable(out: Out, rows: string[][]) {
  const widths = rows[0]!.map((_, i) => Math.max(...rows.map((r) => r[i]!.length)))
  for (const r of rows) out(r.map((v, i) => v.padEnd(widths[i]!)).join("  ").trimEnd())
}

const fmtMs = (ms: number) => (ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`)

export function printReport(report: any, out: Out, labels?: Map<string, string>) {
  const rows = [["CASE", "LABEL", "PASS", "PASSES", "TOKENS", "TIME"]]
  for (const r of report.extra?.per_case ?? []) {
    rows.push([r.id, r.label ?? labels?.get(r.id) ?? "-", r.pass ? "yes" : "no", `${r.passes}/${r.attempts}`, fmtTokens(r.tokens ?? 0), fmtMs(r.ms ?? 0)])
  }
  if (rows.length > 1) printTable(out, rows)
  for (const [name, s] of Object.entries<any>(report.suites ?? {})) {
    const ci = s.ci95 ? ` (95% range ${s.ci95.low} to ${s.ci95.high})` : ""
    out(`${name}: ${s.passed}/${s.graded} passed, pass rate ${s.pass_rate ?? "n/a"}${ci}${s.skipped ? `, ${s.skipped} skipped` : ""}`)
  }
}

export interface RunDeps {
  isTTY: boolean
  confirm: (question: string) => Promise<boolean>
  estimate: typeof estimateRun
  pollMs: number
  /** Passed to the live job (test hook: stub binary, trajectory source). */
  live?: EvalJobs.StartInput["live"]
}

async function ttyConfirm(question: string): Promise<boolean> {
  const readline = await import("readline/promises")
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout })
  try {
    return /^y(es)?$/i.test((await rl.question(question)).trim())
  } finally {
    rl.close()
  }
}

const realRunDeps = (): RunDeps => ({ isTTY: !!process.stdin.isTTY, confirm: ttyConfirm, estimate: estimateRun, pollMs: 1000 })

export interface RunArgs extends SelectArgs {
  mode?: string
  k?: number
  model?: string
  json?: boolean
  yes?: boolean
  recordTraces?: boolean
  baseline?: string
}

export async function runEstimate(
  args: Omit<RunArgs, "mode"> & { seed?: number },
  out: Out = stdout,
  deps: Partial<RunDeps> = {},
): Promise<Estimate> {
  const d = { ...realRunDeps(), ...deps }
  const sel = await selectCases(args)
  const est = await d.estimate({ suite: args.suite, suiteDir: args.suiteDir, caseIds: sel.ids, k: args.k, model: args.model })
  out(args.json ? JSON.stringify(est, null, 2) : estimateLine(est))
  return est
}

export async function runRun(args: RunArgs, out: Out = stdout, deps: Partial<RunDeps> = {}): Promise<{ report: any; ok: boolean }> {
  const d = { ...realRunDeps(), ...deps }
  if (args.mode !== "replay" && args.mode !== "live") throw new EvalCliError("Pick a mode: --mode replay or --mode live.")
  const sel = await selectCases(args)
  const seed = args.seed ?? 1
  let report: any
  let run_id: string | undefined
  let tracePack: { sessionID: string; entries: TraceEntry[] }[] = []

  if (args.mode === "replay") {
    report = await EvalRunner.replay({ suite: args.suite, suiteDir: args.suiteDir, caseIds: sel.ids, seed })
    run_id = await EvalJobs.recordReplay(report, Object.keys(report.suites)[0] ?? "").catch(() => undefined)
  } else {
    const est = await d.estimate({ suite: args.suite, suiteDir: args.suiteDir, caseIds: sel.ids, k: args.k, model: args.model })
    // Progress and the price go to stderr with --json so stdout stays one JSON document.
    const say: Out = args.json ? (l) => console.error(l) : out
    say(estimateLine(est))
    if (!args.yes) {
      if (!d.isTTY) throw new EvalCliError("This run uses your model. Add --yes to start it without asking.")
      if (!(await d.confirm("Start this run? [y/N] "))) throw new EvalCliError("Run cancelled. Nothing was started.")
    }
    const live: NonNullable<EvalJobs.StartInput["live"]> = { ...d.live }
    if (args.recordTraces) {
      const inner = live.trajectoryFor ?? ((id: string) => recordedTrajectory(id))
      live.trajectoryFor = async (sessionID) => {
        const entries = SessionTrace.list({ sessionID, limit: 5_000 }) as unknown as TraceEntry[]
        tracePack.push({ sessionID, entries: redactPrivate(entries, []).entries })
        return inner(sessionID)
      }
    }
    const started = await EvalJobs.start({
      suite: args.suite,
      suiteDir: args.suiteDir,
      caseIds: est.case_ids,
      k: est.k,
      model: args.model,
      estimateId: est.estimate_id,
      seed,
      live,
    })
    run_id = started.run_id
    const onSigint = () => EvalJobs.cancel(started.run_id)
    process.once("SIGINT", onSigint)
    let last = -1
    try {
      for (;;) {
        const v = await EvalJobs.get(started.run_id)
        if (v.progress.done !== last) {
          last = v.progress.done
          say(`Progress: ${v.progress.done}/${v.progress.total} runs`)
        }
        if (v.state === "done") {
          report = v.report
          break
        }
        if (v.state === "failed")
          throw new EvalCliError(v.error === "price_check" ? "Stopped: the first run cost more than the price shown." : `The run failed: ${v.error ?? "unknown error"}`)
        if (v.state === "cancelled") throw new EvalCliError("The run was cancelled.")
        await new Promise((r) => setTimeout(r, d.pollMs))
      }
    } finally {
      process.off("SIGINT", onSigint)
    }
    if (args.recordTraces && run_id) {
      // Attempts run one after another in case order, so row i owns the next `attempts` traces.
      const dir = path.join(EvalStore.root(), "runs", run_id, "traces")
      await fs.mkdir(dir, { recursive: true })
      const index: { case: string; attempt: number; file: string }[] = []
      let at = 0
      for (const row of report.extra?.per_case ?? []) {
        for (let n = 1; n <= row.attempts; n++) {
          const t = tracePack[at++]
          if (!t) continue
          const file = `${row.id}.a${n}.trace.json`
          await fs.writeFile(path.join(dir, file), JSON.stringify(t.entries))
          index.push({ case: row.id, attempt: n, file })
        }
      }
      await fs.writeFile(path.join(dir, "index.json"), JSON.stringify(index, null, 2))
    }
  }

  enrichReport(report, sel.info)

  let ok = true
  let flips: ReturnType<typeof flippedCases> = []
  let cmp: ReturnType<typeof compareReports> | undefined
  if (args.baseline) {
    const base = JSON.parse(await fs.readFile(args.baseline, "utf8").catch(() => { throw new EvalCliError(`Baseline file not found: ${args.baseline}`) }))
    cmp = compareReports(report, base)
    flips = flippedCases(report, base)
    ok = cmp.ok
  }

  if (args.json) {
    out(JSON.stringify(report, null, 2))
    if (cmp) {
      for (const f of cmp.failures) console.error(`FAIL: ${f}`)
      for (const f of flips) console.error(`Flipped: ${f.id} ${f.was ? "passed" : "failed"} before, ${f.now ? "passes" : "fails"} now`)
    }
  } else {
    if (run_id) out(`Run ${run_id}`)
    printReport(report, out, sel.labels)
    if (cmp) {
      for (const n of cmp.notes) out(`Note: ${n}`)
      for (const f of flips) out(`Flipped: ${f.id} ${f.was ? "passed" : "failed"} before, ${f.now ? "passes" : "fails"} now`)
      for (const f of cmp.failures) out(`FAIL: ${f}`)
      out(ok ? "Compared with the baseline: PASS" : "Compared with the baseline: FAIL")
    }
  }
  return { report, ok }
}

export async function runReport(args: { runID: string; json?: boolean }, out: Out = stdout) {
  const v = await EvalJobs.get(args.runID).catch((e) => {
    throw new EvalCliError(e instanceof Error ? e.message : String(e))
  })
  if (!v.report) throw new EvalCliError(`That run has no report yet (state: ${v.state}).`)
  if (args.json) return out(JSON.stringify(v.report, null, 2))
  out(`Run ${v.run_id} (${v.mode}, ${v.state})`)
  printReport(v.report, out)
}

export async function runBaselineWrite(args: { run: string; out: string }, out: Out = stdout) {
  const v = await EvalJobs.get(args.run).catch((e) => {
    throw new EvalCliError(e instanceof Error ? e.message : String(e))
  })
  if (!v.report) throw new EvalCliError(`That run has no report yet (state: ${v.state}).`)
  const previous = await fs.readFile(args.out, "utf8").then((t) => JSON.parse(t), () => undefined)
  // Best effort: a run of a --suite-dir folder may not resolve by id, then the rows keep what the run stored.
  if (v.report.extra?.per_case?.some((r: any) => !r.split)) {
    const sel = await selectCases({ suite: v.suite }).catch(() => undefined)
    if (sel) enrichReport(v.report, sel.info)
  }
  const baseline = baselineFrom(v.report, previous)
  if (Object.keys(baseline.suites).length === 0) throw new EvalCliError("That run graded nothing, so there is nothing to save.")
  await fs.mkdir(path.dirname(path.resolve(args.out)), { recursive: true })
  await fs.writeFile(args.out, JSON.stringify(baseline, null, 2) + "\n")
  out(`Wrote baseline ${args.out} from run ${args.run}`)
}

// Errors print the plain message and exit 1.
function guarded<A>(fn: (args: A) => Promise<void>) {
  return async (args: A) => {
    try {
      await bootstrap(process.cwd(), () => fn(args))
    } catch (err) {
      console.error(err instanceof EvalStore.EvalError || err instanceof EvalCliError ? err.message : `Something went wrong: ${String(err)}`)
      process.exit(1)
    }
  }
}

const CaseSaveCommand = cmd({
  command: "save <sessionID>",
  describe: "save a chat as a test case",
  builder: (y: Argv) =>
    y
      .positional("sessionID", { type: "string", demandOption: true })
      .option("good", { type: "boolean", describe: "the chat went well" })
      .option("bad", { type: "boolean", describe: "the chat went wrong" })
      .option("note", { type: "string", describe: "what should have happened" })
      .option("through", { type: "string", describe: "keep the chat up to this message id" })
      .option("suite", { type: "string", describe: "suite name (default my-cases)" })
      .option("json", { type: "boolean", describe: "print the route response" }),
  handler: guarded((a: any) => runSave(a)),
})

const CaseListCommand = cmd({
  command: "list",
  describe: "list saved test cases",
  builder: (y: Argv) =>
    y
      .option("session", { type: "string" })
      .option("suite", { type: "string" })
      .option("json", { type: "boolean" }),
  handler: guarded((a: any) => runList(a)),
})

const CaseShowCommand = cmd({
  command: "show <id>",
  describe: "show one test case",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }).option("json", { type: "boolean" }),
  handler: guarded((a: any) => runShow(a)),
})

const CaseRmCommand = cmd({
  command: "rm <id>",
  describe: "delete a test case",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }),
  handler: guarded((a: any) => runRemove(a)),
})

const CaseExportCommand = cmd({
  command: "export <id>",
  describe: "copy a test case to a folder with strict redaction",
  builder: (y: Argv) =>
    y.positional("id", { type: "string", demandOption: true }).option("out", { type: "string", demandOption: true, describe: "folder to write into" }),
  handler: guarded((a: any) => runExport(a)),
})

const CaseCommand = cmd({
  command: "case",
  describe: "saved test cases",
  builder: (y: Argv) =>
    y.command(CaseSaveCommand).command(CaseListCommand).command(CaseShowCommand).command(CaseRmCommand).command(CaseExportCommand).demandCommand(),
  async handler() {},
})

const selectOptions = (y: Argv) =>
  y
    .option("suite", { type: "string", describe: "suite id in your test cases" })
    .option("suite-dir", { type: "string", describe: "path to a suite folder" })
    .option("k", { type: "number", describe: "attempts per case" })
    .option("model", { type: "string" })
    .option("fraction", { type: "number", describe: "share of the cases to run, 0 to 1" })
    .option("seed", { type: "number", describe: "seed for the sample and the report ranges" })
    .option("split", { type: "string", choices: ["dev", "holdout", "all"], describe: "which cases to use" })
    .option("skip-case", { type: "string", array: true, describe: "leave this case id out (repeatable)" })
    .option("json", { type: "boolean" })

const RunCommand = cmd({
  command: "run",
  describe: "run a suite of test cases",
  builder: (y: Argv) =>
    selectOptions(y)
      .option("mode", { type: "string", choices: ["replay", "live"], demandOption: true, describe: "replay is free, live uses your model" })
      .option("yes", { type: "boolean", describe: "start a live run without asking" })
      .option("record-traces", { type: "boolean", describe: "keep each attempt's redacted trace next to the report" })
      .option("baseline", { type: "string", describe: "compare with this baseline file; exit 1 on a drop" }),
  handler: guarded(async (a: any) => {
    const r = await runRun({ ...a, suiteDir: a.suiteDir ?? a["suite-dir"], skipCases: a.skipCase ?? a["skip-case"], recordTraces: a.recordTraces ?? a["record-traces"] })
    if (!r.ok) process.exit(1)
  }),
})

const EstimateCommand = cmd({
  command: "estimate",
  describe: "show what a live run would cost, without running it",
  builder: (y: Argv) => selectOptions(y),
  handler: guarded(async (a: any) => {
    await runEstimate({ ...a, suiteDir: a.suiteDir ?? a["suite-dir"], skipCases: a.skipCase ?? a["skip-case"] })
  }),
})

const ReportCommand = cmd({
  command: "report <runID>",
  describe: "show the results of a run",
  builder: (y: Argv) => y.positional("runID", { type: "string", demandOption: true }).option("json", { type: "boolean" }),
  handler: guarded((a: any) => runReport(a)),
})

const BaselineWriteCommand = cmd({
  command: "write",
  describe: "save a run as the baseline to compare against",
  builder: (y: Argv) =>
    y.option("run", { type: "string", demandOption: true }).option("out", { type: "string", demandOption: true, describe: "baseline file to write" }),
  handler: guarded((a: any) => runBaselineWrite(a)),
})

const BaselineCommand = cmd({
  command: "baseline",
  describe: "baseline files for comparing runs",
  builder: (y: Argv) => y.command(BaselineWriteCommand).demandCommand(),
  async handler() {},
})

export const EvalCommand = cmd({
  command: "eval",
  describe: "test cases and checks for Gizzi",
  builder: (y: Argv) => y.command(CaseCommand).command(RunCommand).command(EstimateCommand).command(ReportCommand).command(BaselineCommand).demandCommand(),
  async handler() {},
})
