// @ts-nocheck
import { test, expect, afterEach } from "bun:test"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { spawnSync } from "child_process"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionTrace } from "../../src/runtime/session/trace"
import { Identifier } from "../../src/shared/id/id"
import { EvalStore } from "../../src/runtime/eval/store"
import { runSave, runList, runRemove, runExport, runRun, sampleIds, selectCases, enrichReport, compareReports, baselineFrom, EvalCliError } from "../../src/cli/commands/eval"
import { parseTestcaseArgs } from "../../src/cli/ui/ink-app/commands/testcase/testcase"
import { tmpdir } from "../fixture/fixture"

const created: string[] = []
afterEach(async () => {
  for (const id of created.splice(0)) await EvalStore.deleteCase(id).catch(() => {})
})

async function makeSession(prompt = "Fix the failing test") {
  const session = await Session.create({ title: "eval cli test" })
  const t0 = Date.now()
  const messageID = Identifier.ascending("message")
  await Session.updateMessage({
    id: messageID,
    sessionID: session.id,
    role: "user",
    time: { created: t0 },
    agent: "build",
    model: { providerID: "test", modelID: "test" },
  })
  await Session.updatePart({ id: Identifier.ascending("part"), sessionID: session.id, messageID, type: "text", text: prompt })
  SessionTrace.append({ sessionID: session.id, kind: "request.started", data: { agent: "build" }, time: t0 })
  SessionTrace.append({ sessionID: session.id, kind: "request.completed", data: { cost: 0.01, tokens: { input: 1, output: 1 } }, time: t0 + 2000 })
  return session
}

async function inProject<T>(fn: () => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return Instance.provide({ directory: tmp.path, fn })
}

const collect = () => {
  const lines: string[] = []
  return { lines, out: (l: string) => lines.push(l) }
}

test("save prints the id, list shows it, rm removes it", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const saved = collect()
    await runSave({ sessionID: s.id, good: true, note: "works" }, saved.out)
    const id = /Saved test case (case_\w+) \(my-cases\)/.exec(saved.lines[0])?.[1]
    expect(id).toBeDefined()
    created.push(id!)
    expect(saved.lines[1]).toMatch(/^(Self-test: agrees|Needs a check: .+)$/)

    const listed = collect()
    await runList({ session: s.id }, listed.out)
    const row = listed.lines.find((l) => l.startsWith(id!))
    expect(row).toContain("Fix the failing test")
    expect(listed.lines[0]).toContain("NEEDS CHECK")

    const removed = collect()
    await runRemove({ id: id! }, removed.out)
    expect(removed.lines[0]).toBe(`Removed test case ${id}`)
    const after = collect()
    await runList({ session: s.id }, after.out)
    expect(after.lines).toEqual(["No test cases saved yet."])
  })
})

test("save needs exactly one label and shows a plain error", async () => {
  await expect(runSave({ sessionID: "x" }, () => {})).rejects.toThrow(EvalCliError)
  await expect(runSave({ sessionID: "x", good: true, bad: true }, () => {})).rejects.toThrow("Pick one")
  await inProject(async () => {
    await expect(runSave({ sessionID: "ses_missing", bad: true }, () => {})).rejects.toThrow("That session was not found.")
  })
})

test("export refuses when a planted key is in the workspace tar, and writes a clean copy otherwise", async () => {
  await inProject(async () => {
    const s = await makeSession()
    const saved = await EvalStore.saveCase({ sessionID: s.id, label: "bad", surface: "cli" })
    created.push(saved.case.id)
    const caseDir = path.join(EvalStore.root(), "suites", "my-cases", "cases", saved.case.id)
    const work = await fs.mkdtemp(path.join(os.tmpdir(), "eval-cli-"))
    try {
      const src = path.join(work, "src")
      await fs.mkdir(src)
      await fs.writeFile(path.join(src, ".env"), "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwx1234\n")
      expect(spawnSync("tar", ["-czf", path.join(caseDir, "workspace.tar.gz"), "-C", src, "."]).status).toBe(0)

      const dest = path.join(work, "out")
      await expect(runExport({ id: saved.case.id, out: dest }, () => {})).rejects.toThrow(/Export stopped: 1 files look like secrets: .*\.env/)
      expect(await fs.stat(path.join(dest, saved.case.id)).catch(() => null)).toBeNull()

      await fs.writeFile(path.join(src, ".env"), "NAME=demo\n")
      expect(spawnSync("tar", ["-czf", path.join(caseDir, "workspace.tar.gz"), "-C", src, "."]).status).toBe(0)
      const done = collect()
      await runExport({ id: saved.case.id, out: dest }, done.out)
      const files = (await fs.readdir(path.join(dest, saved.case.id))).sort()
      expect(files).toEqual(["case.json", "redaction_report.json", "trace.json", "workspace.tar.gz"])
    } finally {
      await fs.rm(work, { recursive: true, force: true })
    }
  })
})

test("/testcase saves as bad when no label is given", () => {
  expect(parseTestcaseArgs("")).toEqual({ label: "bad", note: undefined })
  expect(parseTestcaseArgs("good It was fine")).toEqual({ label: "good", note: "It was fine" })
  expect(parseTestcaseArgs("it deleted my file")).toEqual({ label: "bad", note: "it deleted my file" })
  expect(parseTestcaseArgs("bad")).toEqual({ label: "bad", note: undefined })
})

// ---- gizzi eval run (eval loop 2.5) ----

const SHARED = path.resolve(import.meta.dir, "../../../../domains/agent-evals/gizzi/suites/shared.v1")

/** A temp suite holding copies of the shared good and bad session cases. */
async function tempSuite(edit?: (id: string, c: any) => void) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "eval-run-suite-"))
  const ids = ["case_shared_session_good", "case_shared_session_bad"]
  for (const id of ids) {
    const to = path.join(dir, "cases", id)
    await fs.mkdir(to, { recursive: true })
    const c = JSON.parse(await fs.readFile(path.join(SHARED, "cases", id, "case.json"), "utf8"))
    edit?.(id, c)
    await fs.writeFile(path.join(to, "case.json"), JSON.stringify(c))
    await fs.copyFile(path.join(SHARED, "cases", id, "trace.json"), path.join(to, "trace.json"))
  }
  await fs.writeFile(
    path.join(dir, "suite.json"),
    JSON.stringify({
      schema: "allternit.eval_suite.v1",
      id: "shared.v1",
      title: "temp",
      runner: "gizzi",
      cases: ids.map((id) => ({ id, source: { kind: "session", path: `cases/${id}/case.json` } })),
    }),
  )
  return dir
}

async function dropRun(lines: string[]) {
  const id = lines.map((l) => /^Run (run_\w+)$/.exec(l)?.[1]).find(Boolean)
  if (id) await fs.rm(path.join(EvalStore.root(), "runs", id), { recursive: true, force: true })
}

test("replay on a temp suite prints a results table and a valid report", async () => {
  const dir = await tempSuite()
  try {
    const table = collect()
    const r = await runRun({ mode: "replay", suiteDir: dir }, table.out)
    await dropRun(table.lines)
    expect(table.lines.find((l) => l.startsWith("CASE"))).toContain("PASSES")
    expect(table.lines.find((l) => l.startsWith("case_shared_session_good"))).toMatch(/good\s+yes\s+1\/1\s+2,040/)
    expect(table.lines.find((l) => l.startsWith("case_shared_session_bad"))).toMatch(/bad\s+yes\s+1\/1/)
    expect(r.ok).toBe(true)

    const json = collect()
    await runRun({ mode: "replay", suiteDir: dir, json: true }, json.out)
    const report = JSON.parse(json.lines.join("\n"))
    expect(report.schema).toBe("allternit.eval_report.v1")
    expect(report.suites["shared.v1"]).toMatchObject({ graded: 2, passed: 2, pass_rate: 1, failed_cases: [] })
    expect(report.extra.per_case.map((p: any) => p.id)).toEqual(["case_shared_session_good", "case_shared_session_bad"])
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
})

test("live without --yes on a non-TTY shows the price, then refuses and starts nothing", async () => {
  const dir = await tempSuite()
  try {
    const seen = collect()
    let started = false
    await expect(
      runRun({ mode: "live", suiteDir: dir }, seen.out, {
        isTTY: false,
        estimate: async (input: any) => {
          expect(input.suiteDir).toBe(dir)
          return {
            estimate_id: "est_x", expires_at: Date.now() + 1000, suite: "shared.v1", case_ids: ["a", "b"], cases: 2, k: 1, attempts: 2,
            tokens_est: 4000, minutes_est: 8, billing: "plan", probe_required: false,
          } as any
        },
        confirm: async () => {
          started = true
          return true
        },
      }),
    ).rejects.toThrow("Add --yes")
    expect(seen.lines).toEqual(["2 runs · about 4,000 tokens · about 8 min · Uses your plan"])
    expect(started).toBe(false)
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
})

test("the seeded fraction picks the same ids twice and honours the split", async () => {
  const ids = Array.from({ length: 20 }, (_, i) => `case_${String(i).padStart(2, "0")}`)
  const a = sampleIds(ids, 0.25, 7)
  expect(a).toHaveLength(5)
  expect(sampleIds([...ids].reverse(), 0.25, 7)).toEqual(a)
  expect(sampleIds(ids, 0.25, 8)).not.toEqual(a)

  const dir = await tempSuite((id, c) => {
    if (id.endsWith("bad")) c.split = "holdout"
  })
  try {
    expect((await selectCases({ suiteDir: dir, split: "holdout" })).ids).toEqual(["case_shared_session_bad"])
    expect((await selectCases({ suiteDir: dir, split: "dev", fraction: 1 })).ids).toEqual(["case_shared_session_good"])
    expect((await selectCases({ suiteDir: dir })).ids).toBeUndefined()
    await expect(selectCases({ suiteDir: dir, fraction: 2 })).rejects.toThrow("--fraction")
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
})

test("a baseline compare names the flipped case and fails; the CLI copy matches the gate script", async () => {
  // Make the bad case's graders pass on its recording: a bad case that passes its graders is a failure.
  const dir = await tempSuite((id, c) => {
    if (id.endsWith("bad")) c.graders = [{ type: "status", equals: "failed" }]
  })
  const base = await fs.mkdtemp(path.join(os.tmpdir(), "eval-run-base-"))
  const cleanDir = await tempSuite()
  try {
    const good = collect()
    const first = await runRun({ mode: "replay", suiteDir: cleanDir }, good.out)
    await dropRun(good.lines)
    const baselineFile = path.join(base, "baseline.json")
    await fs.writeFile(baselineFile, JSON.stringify(baselineFrom(first.report)))

    const out = collect()
    const r = await runRun({ mode: "replay", suiteDir: dir, baseline: baselineFile }, out.out)
    await dropRun(out.lines)
    expect(r.ok).toBe(false)
    expect(out.lines).toContain("Flipped: case_shared_session_bad passed before, fails now")
    expect(out.lines.some((l) => l.startsWith("FAIL: shared.v1: pass rate 50.0% is 50.0 pp below"))).toBe(true)

    const gate = await import("../../../../scripts/agency-eval-gate.mjs")
    const baseline = JSON.parse(await fs.readFile(baselineFile, "utf8"))
    expect(compareReports(r.report, baseline)).toEqual(gate.compareReports(r.report, baseline))
    expect(compareReports(first.report, baseline)).toEqual(gate.compareReports(first.report, baseline))
    expect(baselineFrom(first.report).suites).toEqual(gate.baselineFrom(first.report).suites)
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
    await fs.rm(base, { recursive: true, force: true })
    await fs.rm(cleanDir, { recursive: true, force: true })
  }
})

test("--skip-case removes the case from the selection, and per_case carries split and title", async () => {
  const dir = await tempSuite((id, c) => {
    if (id.endsWith("bad")) {
      c.split = "holdout"
      c.note = "should not delete files"
      c.graders = [{ type: "status", equals: "failed" }]
    }
  })
  try {
    const all = await selectCases({ suiteDir: dir, skipCases: ["case_shared_session_bad"] })
    expect(all.ids).toEqual(["case_shared_session_good"])
    const split = await selectCases({ suiteDir: dir, split: "holdout", skipCases: ["case_shared_session_good"] })
    expect(split.ids).toEqual(["case_shared_session_bad"])
    await expect(selectCases({ suiteDir: dir, skipCases: ["case_shared_session_good", "case_shared_session_bad"] })).rejects.toThrow("skipped")

    const json = collect()
    await runRun({ mode: "replay", suiteDir: dir, json: true, skipCases: ["case_shared_session_good"] }, json.out)
    const report = JSON.parse(json.lines.join("\n"))
    expect(report.extra.per_case.map((p: any) => p.id)).toEqual(["case_shared_session_bad"])
    const row = report.extra.per_case[0]
    expect(row.split).toBe("holdout")
    expect(typeof row.title).toBe("string")
    expect(row.title.length).toBeGreaterThan(0)
    expect(row.note).toBe("should not delete files")
    expect(Array.isArray(row.grader_details)).toBe(true)
    expect(typeof row.cost).toBe("number")
    expect(baselineFrom(report).extra.per_case[0]).toMatchObject({ id: "case_shared_session_bad", split: "holdout", title: row.title })
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
})
