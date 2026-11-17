/**
 * agent_batch core: one worker per CSV row, each replying with a JSON object
 * checked against an output schema. Shared by the runtime builtin
 * (tools/builtins/agent-batch.ts) and an Ink adapter; the worker itself is
 * injected (BatchWorker), so this file holds no session state and bun tests
 * drive it with a fake worker.
 *
 * - Scheduling reuses agent_swarm's AdaptiveRunBatch (burst, ramp, provider
 *   rate-limit requeue, per-row timeout, cancellation).
 * - A reply that fails the schema gets exactly one retry, told what was wrong.
 * - Sample mode runs the first 1-3 rows, writes nothing, and estimates the
 *   full run's tokens, cost and time.
 * - Run mode appends each row to the output CSV as it finishes (input columns
 *   + schema columns + `_status` + `_error`) and records it in a progress
 *   sidecar (`<output>.progress.json`). Running the same call again keeps
 *   the rows that finished ok, drops the failed ones from the file, and runs
 *   everything else.
 * - Paths follow send_user_message's rules: symlinks resolved, inside the
 *   project, the session's working folder or the scratchpad, never a
 *   credential or git control file.
 */
import { createHash } from "node:crypto"
import fs from "fs/promises"
import path from "path"
import { AdaptiveRunBatch, isProviderRateLimit, type AdaptiveRunBatchOptions, type AdaptiveRunLauncher } from "@/runtime/agents/adaptive-run-batch"
import { isCredentialPath } from "@/runtime/tools/guard/permission/credential-paths"
import { BOM, CsvError, formatRow, neutralizeFormula, parseCsv } from "./csv"
import { replyInstructions, retryInstructions, validateReply, type CellValue, type Field } from "./schema"

export const DEFAULT_CONCURRENCY = 4
export const MAX_CONCURRENCY = 8
export const DEFAULT_ROW_SECONDS = 300
export const MAX_ROW_SECONDS = 1800
export const DEFAULT_ROWS = 200
export const MAX_ROWS = 1000
export const SAMPLE_ROWS = 3
export const MAX_CSV_BYTES = 25 * 1024 * 1024
export const STATUS_COLUMN = "_status"
export const ERROR_COLUMN = "_error"

/** Tools a worker never gets: no fan-out, no talking to the user, no waiting, no scheduling. */
export const WORKER_DENIED_TOOLS = [
  "task",
  "agent_swarm",
  "agent_batch",
  "batch",
  "todowrite",
  "todoread",
  "question",
  "send_user_message",
  "sleep",
  "automation",
] as const

// ---------------------------------------------------------------------------
// Paths

export interface PathContext {
  /** Folders the CSV and the output may be in (send_user_message's allowedRoots). */
  roots: string[]
  /** Relative paths resolve against this (default: the first root). */
  cwd?: string
}

function inside(root: string, child: string): boolean {
  const rel = path.relative(root, child)
  return rel !== "" && !path.isAbsolute(rel) && rel.split(path.sep)[0] !== ".."
}

async function realRoots(roots: string[]): Promise<string[]> {
  return Promise.all(roots.filter(Boolean).map((r) => fs.realpath(r).catch(() => path.resolve(r))))
}

function absolute(given: string, ctx: PathContext): string {
  const cwd = ctx.cwd ?? ctx.roots[0] ?? process.cwd()
  return path.isAbsolute(given) ? path.normalize(given) : path.resolve(cwd, given)
}

const OUTSIDE =
  "is outside the project, the session's working folder and the scratchpad (symlinks are followed). Copy the CSV into the project or the scratchpad first."

/** The input CSV: an existing file inside a root, not a credential, at most 25 MB. Returns its real path. */
export async function resolveCsvPath(given: string, ctx: PathContext): Promise<string> {
  const abs = absolute(given, ctx)
  if (isCredentialPath(abs)) throw new Error(`${given} looks like a credential or git control file, so it can't be used.`)
  let real: string
  try {
    real = await fs.realpath(abs)
  } catch {
    throw new Error(`There's no file at ${abs}.`)
  }
  if (isCredentialPath(real)) throw new Error(`${given} points to a credential or git control file, so it can't be used.`)
  const roots = await realRoots(ctx.roots)
  if (!roots.some((root) => inside(root, real))) throw new Error(`${given} ${OUTSIDE}`)
  const stat = await fs.stat(real)
  if (!stat.isFile()) throw new Error(`${given} is not a file.`)
  if (stat.size > MAX_CSV_BYTES) {
    throw new Error(`${given} is ${(stat.size / (1024 * 1024)).toFixed(1)} MB; a batch CSV can be at most ${MAX_CSV_BYTES / (1024 * 1024)} MB.`)
  }
  return real
}

/** `<folder>/<name>.results.csv` next to the input. */
export function defaultOutputPath(csvPath: string): string {
  const parsed = path.parse(csvPath)
  return path.join(parsed.dir, `${parsed.name}.results.csv`)
}

export function progressPath(outputPath: string): string {
  return `${outputPath}.progress.json`
}

async function lstat(p: string) {
  return fs.lstat(p).catch(() => undefined)
}

/**
 * The output CSV: a .csv inside a root, in a folder that exists, not the
 * input, not a credential, not a folder; neither it nor its progress file a
 * symlink out of the roots. Returns the real path (it may not exist yet).
 */
export async function resolveOutputPath(given: string | undefined, csvReal: string, ctx: PathContext): Promise<string> {
  const abs = given ? absolute(given, ctx) : defaultOutputPath(csvReal)
  const shown = given ?? abs
  if (path.extname(abs).toLowerCase() !== ".csv") throw new Error(`output_csv_path must end in .csv (got ${shown}).`)
  if (isCredentialPath(abs)) throw new Error(`${shown} looks like a credential or git control file, so it can't be written.`)
  let parent: string
  try {
    parent = await fs.realpath(path.dirname(abs))
  } catch {
    throw new Error(`The folder for ${shown} doesn't exist. Use an existing folder.`)
  }
  let target = path.join(parent, path.basename(abs))
  const st = await lstat(target)
  if (st?.isSymbolicLink()) target = await fs.realpath(target).catch(() => target)
  if (st && (await fs.stat(target).catch(() => undefined))?.isDirectory()) throw new Error(`${shown} is a folder.`)
  if (isCredentialPath(target)) throw new Error(`${shown} points to a credential or git control file, so it can't be written.`)
  const roots = await realRoots(ctx.roots)
  if (!roots.some((root) => inside(root, target))) throw new Error(`${shown} ${OUTSIDE.replace("Copy the CSV into", "Write the output in")}`)
  if (target === csvReal) throw new Error("output_csv_path is the input CSV. Pick another file so the input stays as it is.")
  if ((await lstat(progressPath(target)))?.isSymbolicLink()) {
    throw new Error(`${progressPath(target)} is a symlink, so the progress file can't be written there.`)
  }
  return target
}

export function hashText(text: string): string {
  return createHash("sha256").update(text).digest("hex")
}

// ---------------------------------------------------------------------------
// Plan

export type BatchRow = {
  /** 0-based position among the data rows. */
  index: number
  /** The line the row starts on in the CSV. */
  line: number
  /** The id_column value, or the 1-based data row number. */
  id: string
  /** The input fields, in header order. */
  values: string[]
  record: Record<string, string>
}

export type BatchPlan = {
  header: string[]
  fields: Field[]
  instruction: string
  idColumn?: string
  /** Output columns: input + schema + _status + _error. */
  columns: string[]
  /** The rows this batch covers (the first max_rows). */
  rows: BatchRow[]
  /** Data rows in the CSV. */
  totalRows: number
  /** Rows past max_rows, not covered. */
  overLimit: number
  bom: boolean
}

const PLACEHOLDER = /\{([^{}\r\n]{1,128})\}/g
const NAME_LIKE = /^[\w .\-]+$/

/**
 * The instruction's `{column}` placeholders. A name that matches a header is
 * filled in; a plain name that matches none is an error (likely a typo);
 * anything else in braces (JSON in an example, say) is left as written.
 */
export function checkInstruction(instruction: string, header: readonly string[]): string[] {
  const used = new Set<string>()
  const unknown = new Set<string>()
  for (const m of instruction.matchAll(PLACEHOLDER)) {
    const name = m[1]!.trim()
    if (header.includes(name)) used.add(name)
    else if (NAME_LIKE.test(name)) unknown.add(name)
  }
  if (unknown.size) {
    throw new Error(
      `The instruction uses ${[...unknown].map((n) => `{${n}}`).join(", ")}, but the CSV has no such column. Columns: ${header.join(", ")}.`,
    )
  }
  if (!used.size) {
    throw new Error(`Put at least one {column} placeholder in the instruction so each row gets its own task. Columns: ${header.join(", ")}.`)
  }
  return [...used]
}

export function fillInstruction(instruction: string, record: Readonly<Record<string, string>>): string {
  return instruction.replace(PLACEHOLDER, (whole, raw: string) => {
    const name = raw.trim()
    return Object.hasOwn(record, name) ? record[name]! : whole
  })
}

/** The whole first message a worker gets for its row. */
export function rowPrompt(plan: Pick<BatchPlan, "instruction" | "fields">, row: BatchRow): string {
  return [
    fillInstruction(plan.instruction, row.record),
    "",
    "You are one worker in a batch, working on this one row only. Don't ask the user anything; nobody will answer.",
    replyInstructions(plan.fields),
  ].join("\n")
}

export function clampRows(maxRows: number | undefined): number {
  return Math.min(Math.max(1, Math.floor(maxRows ?? DEFAULT_ROWS)), MAX_ROWS)
}

/** Parse the CSV and check it against the schema, id column and instruction. Throws a sentence the model can act on. */
export function prepareBatch(input: {
  text: string
  instruction: string
  fields: Field[]
  idColumn?: string
  maxRows?: number
}): BatchPlan {
  const table = parseCsv(input.text)
  if (!table.rows.length) throw new CsvError("The CSV has a header row but no data rows.")
  const lowerHeader = new Map(table.header.map((h) => [h.toLowerCase(), h]))
  for (const reserved of [STATUS_COLUMN, ERROR_COLUMN]) {
    if (lowerHeader.has(reserved)) throw new Error(`The CSV already has a "${reserved}" column; rename it, since the output adds its own.`)
  }
  const clash = input.fields.filter((f) => lowerHeader.has(f.name.toLowerCase())).map((f) => f.name)
  if (clash.length) {
    throw new Error(`output_schema field${clash.length > 1 ? "s" : ""} ${clash.join(", ")} already ${clash.length > 1 ? "are" : "is"} a CSV column. Rename the field.`)
  }
  checkInstruction(input.instruction, table.header)

  const limit = clampRows(input.maxRows)
  const used = table.rows.slice(0, limit)
  const idIndex = input.idColumn === undefined ? -1 : table.header.indexOf(input.idColumn)
  if (input.idColumn !== undefined && idIndex < 0) {
    throw new Error(`id_column "${input.idColumn}" is not a column. Columns: ${table.header.join(", ")}.`)
  }
  const seen = new Map<string, number>()
  const rows = used.map((values, index): BatchRow => {
    const line = table.lines[index]!
    let id = String(index + 1)
    if (idIndex >= 0) {
      id = values[idIndex]!.trim()
      if (!id) throw new Error(`Row on line ${line} has an empty ${input.idColumn}. Every row needs an id.`)
      const earlier = seen.get(id)
      if (earlier !== undefined) throw new Error(`${input.idColumn} "${id}" appears twice (lines ${earlier} and ${line}). Ids must be unique.`)
      seen.set(id, line)
    }
    const record = Object.fromEntries(table.header.map((h, i) => [h, values[i]!]))
    return { index, line, id, values, record }
  })
  return {
    header: table.header,
    fields: input.fields,
    instruction: input.instruction,
    idColumn: input.idColumn,
    columns: [...table.header, ...input.fields.map((f) => f.name), STATUS_COLUMN, ERROR_COLUMN],
    rows,
    totalRows: table.rows.length,
    overLimit: table.rows.length - rows.length,
    bom: table.bom,
  }
}

// ---------------------------------------------------------------------------
// Workers

export type WorkerTurn = { text: string; tokens?: number; cost?: number }

export interface WorkerHandle {
  /** The worker's session id. */
  runID: string
  /** One turn with the worker; the second call continues the same conversation. */
  send(prompt: string): Promise<WorkerTurn>
  /** Re-run the last turn after a provider rate limit. */
  retry?(): Promise<WorkerTurn>
  /** Called once the row is settled (ok, failed, timed out or cancelled). */
  close?(): void | Promise<void>
}

export interface BatchWorker {
  /** Start (or, with retryRunID, reopen) the worker for one row. Must stop work when `signal` aborts. */
  start(input: { row: BatchRow; signal: AbortSignal; retryRunID?: string }): Promise<WorkerHandle>
}

export type RowOutcome = {
  row: BatchRow
  status: "ok" | "failed"
  values?: Record<string, CellValue>
  error?: string
  tokens: number
  cost: number
  ms: number
  runID?: string
}

export type RunOptions = {
  worker: BatchWorker
  maxConcurrency?: number
  /** Per row, in seconds (default 300, max 1800). */
  maxRuntimeSeconds?: number
  signal?: AbortSignal
  /** Called after each row is recorded. */
  onProgress?: (progress: { done: number; failed: number; total: number; skipped: number }) => void
  /** Scheduler tuning (tests shorten the ramp). */
  scheduler?: Omit<AdaptiveRunBatchOptions, "maxConcurrency">
}

function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(signal.reason ?? new Error("aborted"))
  return new Promise<T>((resolve, reject) => {
    const onAbort = () => reject(signal.reason ?? new Error("aborted"))
    signal.addEventListener("abort", onAbort, { once: true })
    promise.then(
      (v) => {
        signal.removeEventListener("abort", onAbort)
        resolve(v)
      },
      (e) => {
        signal.removeEventListener("abort", onAbort)
        reject(e)
      },
    )
  })
}

const message = (error: unknown) => (error instanceof Error ? error.message : String(error))

export function clampConcurrency(n: number | undefined): number {
  return Math.min(Math.max(1, Math.floor(n ?? DEFAULT_CONCURRENCY)), MAX_CONCURRENCY)
}

export function clampRowSeconds(n: number | undefined): number {
  return Math.min(Math.max(1, Math.floor(n ?? DEFAULT_ROW_SECONDS)), MAX_ROW_SECONDS)
}

/**
 * Run rows through workers. `record` is called once per finished row (ok or
 * failed), one at a time, in finishing order. A row the user cancelled is
 * never recorded, so a re-run picks it up.
 */
async function runRows(
  plan: BatchPlan,
  rows: BatchRow[],
  options: RunOptions,
  record: (outcome: RowOutcome) => Promise<void>,
): Promise<{ outcomes: RowOutcome[]; cancelled: boolean; writeError?: unknown }> {
  const stop = new AbortController()
  const onParentAbort = () => stop.abort(options.signal?.reason)
  if (options.signal?.aborted) stop.abort(options.signal.reason)
  else options.signal?.addEventListener("abort", onParentAbort, { once: true })

  const seconds = clampRowSeconds(options.maxRuntimeSeconds)
  const outcomes: RowOutcome[] = []
  const recorded = new Set<number>()
  let writeError: unknown
  let chain = Promise.resolve()
  const save = (outcome: RowOutcome) => {
    if (recorded.has(outcome.row.index)) return chain
    recorded.add(outcome.row.index)
    outcomes.push(outcome)
    chain = chain.then(() => record(outcome)).catch((error) => {
      if (writeError === undefined) writeError = error
      stop.abort(error)
    })
    return chain
  }

  const launcher: AdaptiveRunLauncher<BatchRow, RowOutcome> = {
    async launch(task, context) {
      const row = task.data
      const startedAt = Date.now()
      const handle = await options.worker.start({ row, signal: context.signal, retryRunID: context.retryRunID })
      context.onReady()
      const completion = (async (): Promise<RowOutcome> => {
        let tokens = 0
        let cost = 0
        const take = (turn: WorkerTurn) => {
          tokens += turn.tokens ?? 0
          cost += turn.cost ?? 0
          return turn.text
        }
        let outcome: RowOutcome
        try {
          const first = context.retryRunID && handle.retry ? handle.retry() : handle.send(rowPrompt(plan, row))
          let check = validateReply(plan.fields, take(await abortable(first, context.signal)))
          if (!check.ok) {
            const again = validateReply(plan.fields, take(await abortable(handle.send(retryInstructions(check.error)), context.signal)))
            check = again.ok ? again : { ok: false, error: `${again.error} (still wrong after one retry)` }
          }
          outcome = check.ok
            ? { row, status: "ok", values: check.values, tokens, cost, ms: Date.now() - startedAt, runID: handle.runID }
            : { row, status: "failed", error: `Output didn't match the schema: ${check.error}`, tokens, cost, ms: Date.now() - startedAt, runID: handle.runID }
        } catch (error) {
          if (stop.signal.aborted) throw error // cancelled: the row stays unfinished
          if (isProviderRateLimit(error)) throw error // the scheduler requeues it
          const timedOut = context.signal.aborted
          outcome = {
            row,
            status: "failed",
            error: timedOut ? `Timed out after ${seconds} s.` : message(error),
            tokens,
            cost,
            ms: Date.now() - startedAt,
            runID: handle.runID,
          }
        }
        await save(outcome)
        return outcome
      })().finally(() => Promise.resolve(handle.close?.()).catch(() => {}))
      return { runID: handle.runID, completion }
    },
  }

  const concurrency = clampConcurrency(options.maxConcurrency)
  const batch = new AdaptiveRunBatch(
    launcher,
    rows.map((row) => ({ data: row, timeoutMs: seconds * 1000, signal: stop.signal })),
    { initialBurst: concurrency, ...options.scheduler, maxConcurrency: concurrency },
  )
  const results = await batch.run()
  options.signal?.removeEventListener("abort", onParentAbort)

  // Rows that failed outside a worker's reply (could not start, rate-limited to the end).
  if (!stop.signal.aborted) {
    for (const result of results) {
      if (result.status !== "failed" || recorded.has(result.task.data.index)) continue
      await save({ row: result.task.data, status: "failed", error: result.error ?? "The worker failed.", tokens: 0, cost: 0, ms: 0, runID: result.runID })
    }
  }
  await chain
  return { outcomes, cancelled: stop.signal.aborted && writeError === undefined, writeError }
}

// ---------------------------------------------------------------------------
// Sample

export type Estimate = {
  /** Rows the full run covers. */
  rows: number
  tokens: number
  /** In the provider's currency (USD); null when the sample reported no cost. */
  cost: number | null
  seconds: number
}

export type SampleResult = {
  outcomes: RowOutcome[]
  estimate: Estimate | null
  cancelled: boolean
}

export function estimate(outcomes: RowOutcome[], rows: number, concurrency: number): Estimate | null {
  // Rows a worker actually worked on (not ones that failed before starting).
  const measured = outcomes.filter((o) => o.ms > 0 || o.tokens > 0)
  if (!measured.length) return null
  const avg = (pick: (o: RowOutcome) => number) => measured.reduce((sum, o) => sum + pick(o), 0) / measured.length
  const cost = avg((o) => o.cost)
  return {
    rows,
    tokens: Math.round(avg((o) => o.tokens) * rows),
    cost: cost > 0 ? cost * rows : null,
    seconds: Math.round((avg((o) => o.ms) / 1000) * Math.ceil(rows / clampConcurrency(concurrency))),
  }
}

/** Run the first 1-3 rows, write nothing, and estimate the full run. */
export async function runSample(plan: BatchPlan, options: RunOptions): Promise<SampleResult> {
  const rows = plan.rows.slice(0, SAMPLE_ROWS)
  const { outcomes, cancelled } = await runRows(plan, rows, options, async () => {})
  outcomes.sort((a, b) => a.row.index - b.row.index)
  return { outcomes, estimate: estimate(outcomes, plan.rows.length, clampConcurrency(options.maxConcurrency)), cancelled }
}

// ---------------------------------------------------------------------------
// Run

type ProgressFile = {
  version: 1
  tool: "agent_batch"
  csv_path: string
  csv_sha256: string
  id_column: string | null
  columns: string[]
  /** One entry per data line of the output, in file order. */
  rows: Array<{ id: string; status: "ok" | "failed" }>
  updated_at: string
}

function cell(value: CellValue | undefined): string {
  if (value === undefined || value === null) return ""
  if (typeof value === "string") return neutralizeFormula(value)
  return String(value)
}

async function writeAtomic(file: string, text: string) {
  const tmp = `${file}.${process.pid}.tmp`
  await fs.writeFile(tmp, text)
  await fs.rename(tmp, file)
}

const sameList = (a: readonly string[], b: readonly string[]) => a.length === b.length && a.every((x, i) => x === b[i])

/**
 * Open the output for appending. A fresh output gets its header. An output
 * with a matching progress file is resumed: rows that finished ok stay,
 * failed rows and anything past the last recorded row are dropped (they run
 * again). An output with no progress file is never overwritten.
 */
async function openOutput(plan: BatchPlan, input: { outputPath: string; csvPath: string; csvHash: string }) {
  const sidecar = progressPath(input.outputPath)
  const outputExists = Boolean(await lstat(input.outputPath))
  const sidecarExists = Boolean(await lstat(sidecar))
  const header = (plan.bom ? BOM : "") + formatRow(plan.columns)
  const progress: ProgressFile = {
    version: 1,
    tool: "agent_batch",
    csv_path: input.csvPath,
    csv_sha256: input.csvHash,
    id_column: plan.idColumn ?? null,
    columns: plan.columns,
    rows: [],
    updated_at: new Date().toISOString(),
  }
  const startOver = `Move it and ${path.basename(sidecar)} aside to start over, or pick another output_csv_path.`

  if (outputExists && !sidecarExists) {
    throw new Error(`${input.outputPath} already exists and isn't from an earlier agent_batch run (no ${path.basename(sidecar)}), so it won't be overwritten. Pick another output_csv_path.`)
  }
  if (outputExists) {
    let earlier: ProgressFile
    try {
      earlier = JSON.parse(await fs.readFile(sidecar, "utf8"))
    } catch {
      throw new Error(`${sidecar} can't be read. ${startOver}`)
    }
    if (earlier.tool !== "agent_batch" || earlier.version !== 1 || !Array.isArray(earlier.rows)) {
      throw new Error(`${sidecar} isn't an agent_batch progress file. ${startOver}`)
    }
    if (!sameList(earlier.columns ?? [], plan.columns) || (earlier.id_column ?? null) !== progress.id_column) {
      throw new Error(`${input.outputPath} is from a batch with other columns or another id_column. ${startOver}`)
    }
    if (!plan.idColumn && earlier.csv_sha256 !== input.csvHash) {
      throw new Error(`The CSV changed since the earlier run and there's no id_column, so its rows can't be matched to the output. Set id_column, or ${startOver.charAt(0).toLowerCase()}${startOver.slice(1)}`)
    }
    let table
    try {
      table = parseCsv(await fs.readFile(input.outputPath, "utf8"))
    } catch (error) {
      throw new Error(`${input.outputPath} can't be read to resume (${message(error)}). ${startOver}`)
    }
    if (!sameList(table.header, plan.columns)) throw new Error(`${input.outputPath} has other columns than this batch. ${startOver}`)
    let body = ""
    const n = Math.min(table.rows.length, earlier.rows.length)
    for (let i = 0; i < n; i++) {
      const entry = earlier.rows[i]!
      if (entry.status !== "ok") continue
      body += formatRow(table.rows[i]!)
      progress.rows.push({ id: String(entry.id), status: "ok" })
    }
    await writeAtomic(input.outputPath, header + body)
  } else {
    await fs.writeFile(input.outputPath, header)
  }
  await writeAtomic(sidecar, JSON.stringify(progress, null, 2))

  const finished = new Set(progress.rows.map((r) => r.id))
  return {
    finished,
    async append(outcome: RowOutcome) {
      const values = [
        ...outcome.row.values,
        ...plan.fields.map((f) => cell(outcome.values?.[f.name])),
        outcome.status,
        outcome.error ? neutralizeFormula(outcome.error) : "",
      ]
      await fs.appendFile(input.outputPath, formatRow(values))
      progress.rows.push({ id: outcome.row.id, status: outcome.status })
      progress.updated_at = new Date().toISOString()
      await writeAtomic(sidecar, JSON.stringify(progress, null, 2))
    },
  }
}

export type BatchResult = {
  done: number
  failed: number
  /** Rows that finished ok on an earlier run and were not run again. */
  skipped: number
  /** Rows not finished because the run was cancelled. */
  remaining: number
  total: number
  overLimit: number
  cancelled: boolean
  tokens: number
  cost: number
  output_csv_path: string
  progress_path: string
  /** The first few failures, for the summary. */
  failures: Array<{ id: string; error: string }>
}

/** Run every row not already finished ok, appending each to the output as it finishes. */
export async function runBatch(
  plan: BatchPlan,
  options: RunOptions & { outputPath: string; csvPath: string; csvHash: string },
): Promise<BatchResult> {
  const output = await openOutput(plan, options)
  const todo = plan.rows.filter((row) => !output.finished.has(row.id))
  const skipped = plan.rows.length - todo.length
  let done = 0
  let failed = 0
  const { outcomes, cancelled, writeError } = await runRows(plan, todo, options, async (outcome) => {
    await output.append(outcome)
    if (outcome.status === "ok") done++
    else failed++
    options.onProgress?.({ done, failed, total: plan.rows.length, skipped })
  })
  if (writeError !== undefined) throw new Error(`Writing ${options.outputPath} failed: ${message(writeError)}. Finished rows are kept; run the same call again to resume.`)
  return {
    done,
    failed,
    skipped,
    remaining: todo.length - done - failed,
    total: plan.rows.length,
    overLimit: plan.overLimit,
    cancelled,
    tokens: outcomes.reduce((sum, o) => sum + o.tokens, 0),
    cost: outcomes.reduce((sum, o) => sum + o.cost, 0),
    output_csv_path: options.outputPath,
    progress_path: progressPath(options.outputPath),
    failures: outcomes
      .filter((o) => o.status === "failed")
      .slice(0, 5)
      .map((o) => ({ id: o.row.id, error: o.error ?? "" })),
  }
}
