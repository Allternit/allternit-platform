import { expect, test } from "bun:test"
import { mkdtemp, rm, readFile, writeFile } from "node:fs/promises"
import { join } from "node:path"
import { benchmark } from "./benchmark"
import { main } from "./cli"
import { MockBackend } from "./backends"
import { generateFixtures, sourcePatch } from "./fixtures"
import { MockBugFixGraph, NakedRunner, SystemRunner, evidencePass } from "./runners"
import { ROOT, applyPatch, command, evaluate, materialize } from "./workspace"
import { CRITERIA, type Backend, type GraphEvidence, type Task } from "./types"

const budget = { maxTokens: 100000, maxCalls: 2, timeoutMs: 30000 }
const taskFor = (repo: string): Task => ({ fixtureId: "test", report: "fix", repo, files: generateFixtures(1)[0].files,
  editablePaths: ["src/lib.ts"], backendId: "test-backbone", budget })
test("offline paired benchmark emits JSON/table, measured tests and non-production gates", async () => {
  const output = await mkdtemp(join(ROOT, ".tmp-wp13-report-"))
  try {
    const report = await benchmark({ count: 4, seed: 13, backend: new MockBackend("test-backbone"), graph: new MockBugFixGraph(), budget, output })
    expect(report.eligible).toBe(false)
    expect(report.summary.naked.fixRate.estimate).toBe(3 / 4)
    expect(report.summary.system.fixRate.estimate).toBe(1)
    expect(report.summary.lift.fixRate.estimate).toBeCloseTo(1 / 4)
    expect(report.summary.system.regressionRate.estimate).toBe(0)
    expect(report.summary.system.receiptsPassRate!.estimate).toBe(1)
    expect(report.rows.find(x => x.category === "async-await" && x.mode === "system")!.calls).toBe(2)
    expect(report.rows.filter(x => x.mode === "naked").every(x => x.calls === 1)).toBe(true)
    expect(report.rows.every(x => x.wallMs > 0 && x.verificationWallMs > 0 && x.usage?.estimated)).toBe(true)
    expect(JSON.parse(await readFile(join(output, "report.json"), "utf8"))).toEqual(JSON.parse(JSON.stringify(report)))
    expect(await readFile(join(output, "report.md"), "utf8")).toContain("HARNESS ONLY")
  } finally { await rm(output, { recursive: true, force: true }) }
}, 300000)
test("independent evaluator detects protected-file tampering and genuine regressions", async () => {
  const dir = await mkdtemp(join(ROOT, ".tmp-wp13-integrity-"))
  try {
    const fixture = generateFixtures(1)[0], repo = join(dir, "candidate")
    await materialize(fixture, repo)
    const repaired = fixture.truth.fixedSource.replace("value * 2", "value * 3")
    expect(await applyPatch(repo, sourcePatch(fixture.files["src/lib.ts"], repaired))).toBe(true)
    await writeFile(join(repo, "tests/regression.test.ts"), "import { test } from 'vitest'; test('fake pass', () => {})\n")
    const result = await evaluate(fixture, repo, join(dir, "trusted"))
    expect(result.target.passed).toBe(true)
    expect(result.regression.passed).toBe(false)
    expect(result.integrity).toBe(false)
    expect(await applyPatch(repo, sourcePatch(repaired, repaired, "../escape.ts"))).toBe(false)
    expect(await applyPatch(repo, sourcePatch(repaired, repaired, "tests/bug.test.ts"))).toBe(false)
    expect(await applyPatch(repo, fixture.truth.patch)).toBe(false)
  } finally { await rm(dir, { recursive: true, force: true }) }
}, 30000)
test("receipts must be validated, verifier owned and satisfy every BUG_FIX criterion", () => {
  const evidence: GraphEvidence = { runId: "run", runReceiptId: "rr", mutationReceiptIds: ["mr"], verificationReceiptIds: ["vr"],
    workerId: "worker", verifierId: "verifier", verdict: "PASS", receiptsValidated: true, completionOwner: "verifier",
    criteria: Object.fromEntries(CRITERIA.map(x => [x, true])) as GraphEvidence["criteria"], telemetryComplete: true }
  expect(evidencePass(evidence)).toBe(true)
  expect(evidencePass({ ...evidence, verifierId: "worker" })).toBe(false)
  expect(evidencePass({ ...evidence, receiptsValidated: false })).toBe(false)
  expect(evidencePass({ ...evidence, verificationReceiptIds: [] })).toBe(false)
  expect(evidencePass({ ...evidence, runId: 123 as any })).toBe(false)
  expect(evidencePass({ ...evidence, criteria: { ...evidence.criteria, requirements_satisfied: false } })).toBe(false)
  expect(evidencePass(null)).toBe(false)
})
test("budgets stop extra model calls and prevent backend substitution", async () => {
  const dir = await mkdtemp(join(ROOT, ".tmp-wp13-budgets-"))
  try {
    await materialize(generateFixtures(1)[0], dir)
    const backend = new MockBackend("test-backbone"), task = taskFor(dir)
    const tiny = await new NakedRunner(backend).run({ ...task, budget: { ...budget, maxTokens: 1 } })
    expect(tiny.error).toContain("token budget exceeded")
    expect(tiny.patchAccepted).toBe(false)
    const graph = new MockBugFixGraph()
    graph.execute = async ctx => { for (let i = 0; i < 3; i++) await ctx.model.complete({ backendId: ctx.task.backendId, files: ctx.task.files, report: "fix", attempt: i, budget }); return null as any }
    const runner = new SystemRunner(backend, graph, async () => { throw new Error("unreachable") })
    const capped = await runner.run(task)
    expect(capped.calls).toBe(2)
    expect(capped.error).toContain("model-call budget exhausted")
    const substituted = await new NakedRunner(backend).run({ ...task, backendId: "other" })
    expect(substituted.calls).toBe(0)
    expect(substituted.error).toContain("substitution")
    const failure: Backend = { id: backend.id, kind: "http", complete: async () => { throw new Error("offline") } }
    expect((await new NakedRunner(failure).run(task)).usage).toBeNull()
    graph.execute = async ctx => { ctx.accountUsage({ input: 100001, output: 0, reasoning: 0, cacheRead: 0, cacheWrite: 0, estimated: false }); return null as any }
    const extra = await runner.run(task)
    expect(extra.error).toContain("token budget exceeded")
    expect(extra.usage!.input).toBe(100001)
  } finally { await rm(dir, { recursive: true, force: true }) }
})
test("CLI rejects implicit real graphs and malformed options before any model calls", async () => {
  await expect(main(["run", "--model-id", "be.test", "--backend", "http", "--gizzi-url", "http://gizzi.invalid"])).rejects.toThrow("graph-adapter")
  await expect(main(["run"])).rejects.toThrow("model-id")
  await expect(main(["run", "--unknown", "value"])).rejects.toThrow()
  await expect(main(["run", "--count", "NaN"])).rejects.toThrow()
})
test("gate references must be auditable strings or explicitly absent", async () => {
  const graph = { ...new MockBugFixGraph(), execute: async () => null, gates: { wp10: true, wp12: null } } as any
  await expect(benchmark({ count: 1, seed: 13, backend: new MockBackend("test-backbone"), graph, budget, output: join(ROOT, ".tmp-wp13/unused") })).rejects.toThrow("gate attestations")
})
test("owned command timeouts terminate their process group", async () => {
  const result = await command(["node", "-e", "require('child_process').spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {stdio: 'inherit'}); setInterval(() => {}, 1000)"], ROOT, 100)
  expect(result.passed).toBe(false)
  expect(result.output).toContain("Timeout")
}, 5000)
test("model swaps keep the same graph and production telemetry fails closed", async () => {
  const dir = await mkdtemp(join(ROOT, ".tmp-wp13-swap-"))
  try {
    const fixture = generateFixtures(1)[0]
    const graph = new MockBugFixGraph()
    const passed = { target: { passed: true, exitCode: 0, output: "" }, regression: { passed: true, exitCode: 0, output: "" }, integrity: true }
    for (const id of ["test-backbone-a", "test-backbone-b"]) {
      const repo = join(dir, id)
      await materialize(fixture, repo)
      const result = await new SystemRunner(new MockBackend(id), graph, async () => passed).run({ ...taskFor(repo), backendId: id })
      expect(result.error).toBeUndefined()
      expect(evidencePass(result.evidence)).toBe(true)
    }
    const production = { ...graph, production: true, gates: { wp10: "landing-test", wp12: "gate-test" }, execute: async () => null }
    const missing = await new SystemRunner(new MockBackend("test-backbone"), production, async () => passed).run(taskFor(dir))
    expect(missing.usage).toBeNull()
    expect(missing.evidence).toBeNull()
  } finally { await rm(dir, { recursive: true, force: true }) }
})
