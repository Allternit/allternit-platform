import { performance } from "node:perf_hooks"
import { mkdir, mkdtemp, rm, writeFile, readFile } from "node:fs/promises"
import { cpus, platform, arch } from "node:os"
import { join, resolve } from "node:path"
import { createHash } from "node:crypto"
import { generateFixtures, random } from "./fixtures"
import { DEPENDENCIES, evaluate, materialize, ROOT, runTests } from "./workspace"
import { NakedRunner, SystemRunner, evidencePass } from "./runners"
import { markdown, summarize, type Observation } from "./metrics"
import { CRITERIA, type Backend, type Budget, type BugFixGraph, type Fixture, type Task } from "./types"

export interface BenchmarkConfig {
  count: number; seed: number; backend: Backend; graph: BugFixGraph; budget: Budget
  output: string; bootstrapSamples?: number
}
export async function benchmark(config: BenchmarkConfig) {
  const { backend, graph, budget } = config
  if (!backend.id) throw new Error("backend ID required")
  if (!graph.gates || typeof graph.revision !== "string" || !graph.revision.trim() || typeof graph.production !== "boolean" ||
    [graph.gates.wp10, graph.gates.wp12].some(x => x !== null && (typeof x !== "string" || !x.trim()))) throw new Error("invalid graph metadata or gate attestations")
  if (![budget.maxTokens, budget.maxCalls, budget.timeoutMs].every(x => Number.isSafeInteger(x) && x > 0)) throw new Error("positive integer budgets required")
  const output = resolve(config.output)
  if (!output.startsWith(ROOT + "/")) throw new Error("benchmark output must be inside this worktree")
  const fixtures = generateFixtures(config.count, config.seed), rows: Observation[] = []
  const scratch = await mkdtemp(join(ROOT, ".tmp-wp13-bench-"))
  const orders: { fixtureId: string; modes: string[] }[] = []
  const rng = random(config.seed)
  let verifyCount = 0
  try {
    for (const fixture of fixtures) {
      const baselineRepo = join(scratch, fixture.id, "baseline")
      await materialize(fixture, baselineRepo)
      const baseline = await runTests(baselineRepo)
      if (baseline.target.passed || !baseline.regression.passed) throw new Error(`invalid fixture or test infrastructure: ${fixture.id}\n${baseline.regression.output}`)
      const runners = {
        naked: new NakedRunner(backend),
        system: new SystemRunner(backend, graph, async task => evaluate(fixture, task.repo, join(scratch, fixture.id, `graph-check-${verifyCount++}`))),
      }
      const modes: ("naked" | "system")[] = rng() < 0.5 ? ["naked", "system"] : ["system", "naked"]
      orders.push({ fixtureId: fixture.id, modes })
      for (const mode of modes) {
        const repo = join(scratch, fixture.id, mode)
        await materialize(fixture, repo)
        const task: Task = Object.freeze({ fixtureId: fixture.id, report: fixture.report, repo, files: Object.freeze({ ...fixture.files }),
          editablePaths: [fixture.truth.file], backendId: backend.id, budget: Object.freeze({ ...budget }) })
        const start = performance.now(), result = await runners[mode].run(task)
        const runnerWallMs = performance.now() - start
        const verificationStart = performance.now()
        const verified = await evaluate(fixture, repo, join(scratch, fixture.id, `${mode}-final`))
        const verificationWallMs = performance.now() - verificationStart
        const fixed = verified.target.passed && verified.regression.passed && verified.integrity
        const receiptsPass = mode === "system" ? evidencePass(result.evidence) && verified.integrity && !result.error : null
        const verifierPass = mode === "system" ? receiptsPass && fixed : null
        rows.push({ fixtureId: fixture.id, category: fixture.category, mode, backendId: backend.id,
          fixed, regression: !verified.regression.passed, integrity: verified.integrity,
          completed: fixed && !result.error && (mode === "naked" ? result.patchAccepted : verifierPass === true),
          receiptsPass, verifierPass, usage: result.usage, calls: result.calls,
          wallMs: runnerWallMs + verificationWallMs, runnerWallMs, verificationWallMs,
          patchAccepted: result.patchAccepted, evidence: result.evidence, error: result.error,
          checks: { target: verified.target.passed, regression: verified.regression.passed } })
      }
    }
    const reasons: string[] = []
    if (backend.kind === "mock") reasons.push("Deterministic mock backend; synthetic token estimates.")
    if (!graph.production) reasons.push("Offline graph simulation; WP10 production graph not attached.")
    if (!graph.gates.wp10) reasons.push("WP10 landing attestation missing.")
    if (!graph.gates.wp12) reasons.push("WP12 alpha-gate attestation missing.")
    if (rows.some(x => x.usage === null || x.usage.estimated)) reasons.push("Measured token telemetry unavailable for at least one run.")
    if (rows.some(x => !x.integrity)) reasons.push("Protected evaluation files were modified.")
    if (rows.some(x => x.error?.includes("budget"))) reasons.push("A configured budget was exceeded or exhausted.")
    const summary = summarize(rows, config.seed, config.bootstrapSamples)
    const report = { schemaVersion: "1.0.0", profile: "tiny", eligible: reasons.length === 0, reasons,
      createdAt: new Date().toISOString(), config: { count: config.count, seed: config.seed, backendId: backend.id,
        backendKind: backend.kind, graphId: graph.id, graphRevision: graph.revision, gates: graph.gates, budget,
        bootstrapSamples: config.bootstrapSamples ?? 2000 },
      environment: { platform: platform(), arch: arch(), cpu: cpus()[0]?.model, logicalCpus: cpus().length,
        runtime: `bun ${Bun.version}`, vitest: JSON.parse(await readFile(join(DEPENDENCIES, "vitest/package.json"), "utf8")).version },
      definitions: { fix: "trusted target and regression tests pass with protected-file integrity",
        regression: "previously passing regression suite fails", completion: "fix plus runner success; system requires verifier-owned validated receipts",
        tokens: "input + output + reasoning + cache read + cache write; generator calls plus adapter-accounted auxiliary cognition",
        wall: "runner including graph verification + independent final evaluation; excludes fixture setup/baseline",
        criteria: CRITERIA, confidence: "Wilson 95% rates; paired seeded percentile bootstrap 95% lift" },
      fixtures: fixtures.map(f => ({ id: f.id, seed: f.seed, category: f.category,
        groundTruthSha256: createHash('sha256').update(JSON.stringify(f.truth)).digest('hex') })),
      orders, rows, summary }
    await mkdir(output, { recursive: true })
    await writeFile(join(output, "report.json"), JSON.stringify(report, null, 2) + "\n")
    await writeFile(join(output, "report.md"), markdown(summary, report.eligible, reasons))
    return report
  } finally { await rm(scratch, { recursive: true, force: true }) }
}

/** Standalone generator: ground truth lives beside repos, never inside them. */
export async function exportFixtures(fixtures: Fixture[], output: string) {
  const base = resolve(output)
  if (!base.startsWith(ROOT + "/")) throw new Error("fixture output must be inside this worktree")
  await mkdir(base, { recursive: true })
  for (const fixture of fixtures) await materialize(fixture, join(base, fixture.id))
  await writeFile(join(base, "ground-truth.json"), JSON.stringify(fixtures.map(f => ({ id: f.id, category: f.category, report: f.report, truth: f.truth })), null, 2) + "\n")
}
