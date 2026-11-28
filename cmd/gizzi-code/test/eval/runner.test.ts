// @ts-nocheck
import { test, expect } from "bun:test"
import fs from "fs/promises"
import path from "path"
// @ts-ignore plain JS module in the repo's scripts folder
import { compareReports } from "../../../../scripts/agency-eval-gate.mjs"
import { EvalRunner } from "../../src/runtime/eval/runner"
import { bootstrapCi95 } from "../../src/runtime/eval/stats"
import { tmpdir } from "../fixture/fixture"

const ok = [
  { sequence: 1, sessionID: "s", kind: "request.started", data: { agent: "build" }, time: 1000 },
  { sequence: 2, sessionID: "s", kind: "request.completed", data: { cost: 0.01, tokens: { input: 1, output: 1 } }, time: 2000 },
]
const broken = [
  { sequence: 1, sessionID: "s", kind: "request.started", data: { agent: "build" }, time: 1000 },
  { sequence: 2, sessionID: "s", kind: "request.failed", data: {}, time: 2000 },
]
const graders = [{ type: "status", equals: "completed" }]

async function writeSuite(root: string, cases: { id: string; label: string; trace?: unknown[] }[]) {
  const refs = []
  for (const c of cases) {
    const dir = path.join(root, "cases", c.id)
    await fs.mkdir(dir, { recursive: true })
    await fs.writeFile(path.join(dir, "case.json"), JSON.stringify({ id: c.id, label: c.label, graders }))
    if (c.trace) await fs.writeFile(path.join(dir, "trace.json"), JSON.stringify(c.trace))
    refs.push({ id: c.id, source: { kind: c.trace ? "session" : "workspace_task", path: `cases/${c.id}/case.json` } })
  }
  await fs.writeFile(
    path.join(root, "suite.json"),
    JSON.stringify({ schema: "allternit.eval_suite.v1", id: "t.v1", title: "t", runner: "gizzi", cases: refs }),
  )
}

test("good passing case and bad failing recording both pass; no-trace case is skipped", async () => {
  await using tmp = await tmpdir()
  await writeSuite(tmp.path, [
    { id: "g1", label: "good", trace: ok },
    { id: "b1", label: "bad", trace: broken },
    { id: "w1", label: "good" },
  ])
  const r = await EvalRunner.replay({ suiteDir: tmp.path })
  const s = r.suites["t.v1"]
  expect(s.graded).toBe(2)
  expect(s.skipped).toBe(1)
  expect(s.pass_rate).toBe(1)
  expect(s.failed_cases).toEqual([])
  const bad = r.extra.per_case.find((p) => p.id === "b1")
  expect(bad.pass).toBe(true)
  expect(bad.expected_fail).toBe(true)
  expect(bad.grades.some((g) => !g.pass)).toBe(true)
})

test("a good case whose recording fails counts as a failure", async () => {
  await using tmp = await tmpdir()
  await writeSuite(tmp.path, [
    { id: "g1", label: "good", trace: ok },
    { id: "g2", label: "good", trace: broken },
  ])
  const r = await EvalRunner.replay({ suiteDir: tmp.path })
  expect(r.suites["t.v1"].pass_rate).toBe(0.5)
  expect(r.suites["t.v1"].failed_cases).toEqual(["g2"])
})

test("report works with the real gate: matching baseline passes, higher baseline fails", async () => {
  await using tmp = await tmpdir()
  await writeSuite(tmp.path, [
    { id: "g1", label: "good", trace: ok },
    { id: "g2", label: "good", trace: broken },
  ])
  const r = await EvalRunner.replay({ suiteDir: tmp.path })
  const base = (pass_rate: number) => ({
    schema: "allternit.eval_baseline.v1",
    max_drop_pp: 2,
    suites: { "t.v1": { pass_rate } },
  })
  expect(compareReports(r, base(0.5)).ok).toBe(true)
  const worse = compareReports(r, base(1))
  expect(worse.ok).toBe(false)
  expect(worse.failures[0]).toContain("t.v1")
})

test("the same seed gives the same ci95", async () => {
  const outcomes = [true, true, false, true, false, true, true, false]
  expect(bootstrapCi95(outcomes, 7)).toEqual(bootstrapCi95(outcomes, 7))
  const ci = bootstrapCi95(outcomes, 7)
  expect(ci.low).toBeLessThanOrEqual(0.625)
  expect(ci.high).toBeGreaterThanOrEqual(0.625)
  await using tmp = await tmpdir()
  await writeSuite(tmp.path, [
    { id: "g1", label: "good", trace: ok },
    { id: "g2", label: "good", trace: broken },
  ])
  const a = await EvalRunner.replay({ suiteDir: tmp.path, seed: 3 })
  const b = await EvalRunner.replay({ suiteDir: tmp.path, seed: 3 })
  expect(a.suites["t.v1"].ci95).toEqual(b.suites["t.v1"].ci95)
})
