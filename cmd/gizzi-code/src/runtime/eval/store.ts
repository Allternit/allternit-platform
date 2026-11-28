// Case store for the Gizzi eval loop (CONTRACT.md sections 1, 3 and 6).
// Cases live under <Global.Path.data>/eval/suites/<suite>/cases/<id>/. Writes are atomic.
import crypto from "crypto"
import fs from "fs/promises"
import path from "path"
import { Global } from "@/runtime/context/global"
import { Instance } from "@/runtime/context/project/instance"
import { Session } from "@/runtime/session"
import { SessionTrace } from "@/runtime/session/trace"
import { Installation } from "@/shared/installation"
import { redactPrivate, type TraceEntry } from "./redact"
import { gradeAll, type GraderSpec as RustGrader, type Trajectory as RustTrajectory } from "./graders"
import { toTrajectory } from "./trajectory"
import { copyOut } from "./workspace"
import {
  CASE_LIMIT,
  CASE_SCHEMA,
  Case,
  EvalErrorCode,
  MAX_NOTE_CHARS,
  MAX_TITLE_CHARS,
  MAX_TURNS,
  SUITE_SCHEMA,
  Suite,
  type CaseSummary,
  type GraderSpec,
} from "./schema"

export namespace EvalStore {
  export type ErrorCode = (typeof EvalErrorCode.options)[number]

  export class EvalError extends Error {
    constructor(
      public readonly code: ErrorCode,
      message: string,
    ) {
      super(message)
      this.name = "EvalError"
    }
  }

  export const DEFAULT_SUITE = "my-cases"
  const DEFAULT_TITLE = "My test cases"
  const LATENCY_FLOOR_MS = 60_000

  export function root() {
    return path.join(Global.Path.data, "eval")
  }
  const suitesDir = () => path.join(root(), "suites")
  const suiteDir = (suite: string) => path.join(suitesDir(), safeSegment(suite))
  const casesDir = (suite: string) => path.join(suiteDir(suite), "cases")

  function safeSegment(s: string) {
    if (!/^[A-Za-z0-9_.-]{1,64}$/.test(s) || s === "." || s === "..") throw new EvalError("case_not_found", "That suite name is not valid.")
    return s
  }

  const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
  function ulid() {
    let t = Date.now()
    let time = ""
    for (let i = 0; i < 10; i++) {
      time = CROCKFORD[t % 32] + time
      t = Math.floor(t / 32)
    }
    let rand = ""
    for (const b of crypto.randomBytes(16).subarray(0, 16)) rand += CROCKFORD[b % 32]
    return time + rand
  }

  async function writeAtomic(file: string, data: string) {
    await fs.mkdir(path.dirname(file), { recursive: true })
    const tmp = `${file}.${process.pid}.${crypto.randomBytes(4).toString("hex")}.tmp`
    await fs.writeFile(tmp, data)
    await fs.rename(tmp, file)
  }

  async function readJson(file: string): Promise<unknown | undefined> {
    try {
      return JSON.parse(await fs.readFile(file, "utf8"))
    } catch {
      return undefined
    }
  }

  async function readSuite(suite: string): Promise<Suite | undefined> {
    const raw = await readJson(path.join(suiteDir(suite), "suite.json"))
    const parsed = Suite.safeParse(raw)
    return parsed.success ? parsed.data : undefined
  }

  async function dirBytes(dir: string): Promise<number> {
    let total = 0
    for (const e of await fs.readdir(dir, { withFileTypes: true }).catch(() => [])) {
      const p = path.join(dir, e.name)
      total += e.isDirectory() ? await dirBytes(p) : (await fs.stat(p).catch(() => null))?.size ?? 0
    }
    return total
  }

  async function caseIds(suite: string): Promise<string[]> {
    const entries = await fs.readdir(casesDir(suite), { withFileTypes: true }).catch(() => [])
    return entries.filter((e) => e.isDirectory()).map((e) => e.name)
  }

  async function summarize(c: Case, dir: string): Promise<CaseSummary> {
    const last = (await readJson(path.join(dir, "last_result.json"))) as
      | { last_result?: "pass" | "fail"; last_run_at?: number }
      | undefined
    return {
      id: c.id,
      suite: c.suite,
      title: c.title,
      label: c.label,
      saved_at: c.origin.saved_at,
      needs_check: c.needs_check,
      last_result: last?.last_result ?? null,
      ...(last?.last_run_at ? { last_run_at: last.last_run_at } : {}),
      bytes: await dirBytes(dir),
      workspace_kind: c.workspace.kind,
      session_id: c.origin.session_id,
    }
  }

  function defaultGraders(
    t: RustTrajectory,
    label: "good" | "bad",
    quick: string[] | undefined,
  ): GraderSpec[] {
    const slow = label === "bad" && !!quick?.includes("too_slow")
    const mul = slow ? 0.5 : 2
    const graders: GraderSpec[] = [{ type: "status", equals: "completed" }]
    // A plan brain records $0, and a $0 ceiling would fail any metered rerun.
    if (t.cost_usd > 0) graders.push({ type: "cost_ceiling", max_usd: t.cost_usd * mul })
    graders.push({
      type: "latency_ceiling",
      max_ms: slow ? t.latency_ms * mul : Math.max(t.latency_ms * mul, LATENCY_FLOOR_MS),
    })
    // "Broke a file" adds no grader: forbidding the write tool would forbid every
    // edit, and the trace alone cannot tell a bad write from a good one. The case
    // usually lands as needs_check, and a file check (checks[]) says what "fixed" means.
    return graders
  }

  export async function saveCase(input: {
    sessionID: string
    label: "good" | "bad"
    note?: string
    throughMessageID?: string
    quickCheck?: Case["quick_check"]
    suite?: string
    surface: Case["origin"]["surface"]
  }): Promise<{ case: CaseSummary; self_test: { passed: boolean; agrees_with_label: boolean }; needs_check: boolean }> {
    const suite = safeSegment(input.suite ?? DEFAULT_SUITE)
    const session = await Session.get(input.sessionID).catch(() => undefined)
    if (!session) throw new EvalError("session_not_found", "That session was not found.")
    const messages = await Session.messages({ sessionID: input.sessionID })

    // Turns: user text up to through_message_id, at most 10.
    const rawTurns: { role: "user"; text: string }[] = []
    for (const m of messages) {
      if (m.info.role === "user") {
        const text = m.parts
          .filter((p) => p.type === "text" && !p.synthetic && !p.ignored)
          .map((p) => (p as { text: string }).text)
          .join("\n")
          .trim()
        if (text) rawTurns.push({ role: "user", text })
      }
      if (input.throughMessageID && m.info.id === input.throughMessageID) break
    }
    const turns = rawTurns.slice(0, MAX_TURNS)
    if (turns.length === 0) throw new EvalError("empty_session", "This chat has no messages to save.")

    if ((await caseIds(suite)).length >= CASE_LIMIT)
      throw new EvalError("case_limit", `A suite holds ${CASE_LIMIT} cases. Delete one first.`)

    // Trace: page through the whole session, cut at through_message_id, redact.
    let all: SessionTrace.Entry[] = []
    for (let after = 0; ; ) {
      const page = SessionTrace.list({ sessionID: input.sessionID, after, limit: 5000 })
      all.push(...page)
      if (page.length < 5000) break
      after = page[page.length - 1]!.sequence
    }
    if (input.throughMessageID) {
      const through = input.throughMessageID
      all = all.filter((e) => !e.messageID || e.messageID <= through)
    }
    const red = redactPrivate(all as unknown as TraceEntry[], turns)
    const entries = red.entries as unknown as SessionTrace.Entry[]
    const trajectory = toTrajectory(entries)
    const quick = input.label === "bad" ? input.quickCheck : undefined
    const graders = defaultGraders(trajectory, input.label, quick)
    const graded = gradeAll(trajectory as unknown as RustTrajectory, graders as unknown as RustGrader[])
    const passed = graded.pass
    const agrees = input.label === "good" ? passed : !passed
    const needsCheck = !agrees

    const id = `case_${ulid()}`
    const dir = path.join(casesDir(suite), id)
    await fs.mkdir(dir, { recursive: true })
    try {
      const workspace = await Instance.provide({
        directory: session.directory,
        fn: () => copyOut({ projectID: session.projectID }, messages, dir),
      })
      const title = (red.turns[0]?.text ?? turns[0]!.text).split("\n")[0]!.trim().slice(0, MAX_TITLE_CHARS) || "Saved chat"
      const note = input.note?.trim().slice(0, MAX_NOTE_CHARS)
      const caseFile = Case.parse({
        schema: CASE_SCHEMA,
        id,
        suite,
        title,
        label: input.label,
        ...(note ? { note } : {}),
        ...(quick && quick.length ? { quick_check: quick } : {}),
        split: "dev",
        origin: {
          session_id: input.sessionID,
          through_message_id: input.throughMessageID ?? null,
          saved_at: Date.now(),
          gizzi_version: Installation.VERSION,
          surface: input.surface,
        },
        turns: red.turns.map((t) => ({ role: "user" as const, text: t.text })),
        workspace,
        checks: [],
        effects: "block",
        graders,
        k: 1,
        pass_rule: "all_of_k",
        recorded: {
          trajectory,
          grades: graded.grades.map((g) => ({ type: g.type, pass: g.pass, detail: g.detail })),
          passed,
        },
        needs_check: needsCheck,
        redaction: red.report,
      })
      await writeAtomic(path.join(dir, "trace.json"), JSON.stringify(entries))
      await writeAtomic(path.join(dir, "case.json"), JSON.stringify(caseFile, null, 2))

      const suiteFile =
        (await readSuite(suite)) ??
        ({ schema: SUITE_SCHEMA, id: suite, title: DEFAULT_TITLE, runner: "gizzi", cases: [] } as Suite)
      suiteFile.cases.push({ id, source: { kind: "session", path: `cases/${id}/case.json` } })
      await writeAtomic(path.join(suiteDir(suite), "suite.json"), JSON.stringify(suiteFile, null, 2))

      return {
        case: await summarize(caseFile, dir),
        self_test: { passed, agrees_with_label: agrees },
        needs_check: needsCheck,
      }
    } catch (e) {
      await fs.rm(dir, { recursive: true, force: true }).catch(() => {})
      throw e
    }
  }

  export async function listSuites() {
    const ids = (await fs.readdir(suitesDir(), { withFileTypes: true }).catch(() => []))
      .filter((e) => e.isDirectory())
      .map((e) => e.name)
    const out: { id: string; title: string; cases: number }[] = []
    for (const id of ids) {
      const s = await readSuite(id)
      out.push({ id, title: s?.title ?? id, cases: (await caseIds(id)).length })
    }
    return out
  }

  export async function listCases(input: { sessionID?: string; suite?: string } = {}): Promise<CaseSummary[]> {
    const suites = input.suite ? [safeSegment(input.suite)] : (await listSuites()).map((s) => s.id)
    const out: CaseSummary[] = []
    for (const suite of suites) {
      for (const id of await caseIds(suite)) {
        const dir = path.join(casesDir(suite), id)
        const parsed = Case.safeParse(await readJson(path.join(dir, "case.json")))
        if (!parsed.success) continue
        if (input.sessionID && parsed.data.origin.session_id !== input.sessionID) continue
        out.push(await summarize(parsed.data, dir))
      }
    }
    return out.sort((a, b) => b.saved_at - a.saved_at)
  }

  async function locate(id: string): Promise<{ suite: string; dir: string } | undefined> {
    if (!/^case_[0-9A-Za-z]+$/.test(id)) return undefined
    for (const s of await listSuites()) {
      const dir = path.join(casesDir(s.id), id)
      if (await fs.stat(dir).then((st) => st.isDirectory()).catch(() => false)) return { suite: s.id, dir }
    }
    return undefined
  }

  export async function getCase(id: string): Promise<Case> {
    const at = await locate(id)
    const parsed = at ? Case.safeParse(await readJson(path.join(at.dir, "case.json"))) : undefined
    if (!parsed?.success) throw new EvalError("case_not_found", "That test case was not found.")
    return parsed.data
  }

  export async function deleteCase(id: string): Promise<void> {
    const at = await locate(id)
    if (!at) throw new EvalError("case_not_found", "That test case was not found.")
    await fs.rm(at.dir, { recursive: true, force: true })
    const suite = await readSuite(at.suite)
    if (suite) {
      suite.cases = suite.cases.filter((c) => c.id !== id)
      await writeAtomic(path.join(suiteDir(at.suite), "suite.json"), JSON.stringify(suite, null, 2))
    }
  }

  /** Remember the result of the latest run on a case (kept beside case.json). */
  export async function setLastResult(id: string, result: "pass" | "fail", at: number = Date.now()): Promise<void> {
    const found = await locate(id)
    if (!found) throw new EvalError("case_not_found", "That test case was not found.")
    await writeAtomic(path.join(found.dir, "last_result.json"), JSON.stringify({ last_result: result, last_run_at: at }))
  }
}
