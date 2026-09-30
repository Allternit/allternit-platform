import { random } from "./fixtures"
import { tokenCount, type Category, type GraphEvidence, type Usage } from "./types"

export interface Observation {
  fixtureId: string; category: Category; mode: "naked" | "system"; backendId: string
  fixed: boolean; regression: boolean; integrity: boolean; completed: boolean
  receiptsPass: boolean | null; verifierPass: boolean | null
  usage: Usage | null; calls: number; wallMs: number; runnerWallMs: number; verificationWallMs: number
  evidence: GraphEvidence | null; patchAccepted: boolean; error?: string
  checks: { target: boolean; regression: boolean }
}
export interface Interval { estimate: number; low: number; high: number }
export function wilson(success: number, total: number): Interval {
  if (!Number.isInteger(total) || total < 1 || !Number.isInteger(success) || success < 0 || success > total) throw new Error("invalid binomial sample")
  const p = success / total, z = 1.959963984540054, d = 1 + z * z / total
  const center = (p + z * z / (2 * total)) / d
  const width = z * Math.sqrt(p * (1 - p) / total + z * z / (4 * total * total)) / d
  return { estimate: p, low: Math.max(0, center - width), high: Math.min(1, center + width) }
}
const mean = (xs: number[]) => xs.reduce((sum, x) => sum + x, 0) / xs.length
/** Paired percentile bootstrap: resample fixture pairs, not independent mode samples. */
export function bootstrap(deltas: number[], seed: number, samples = 2000): Interval {
  if (!deltas.length || deltas.some(x => !Number.isFinite(x)) || !Number.isInteger(samples) || samples < 100) throw new Error("invalid bootstrap sample")
  const rng = random(seed), draws: number[] = []
  for (let i = 0; i < samples; i++) {
    let sum = 0
    for (let j = 0; j < deltas.length; j++) sum += deltas[Math.floor(rng() * deltas.length)]
    draws.push(sum / deltas.length)
  }
  draws.sort((a, b) => a - b)
  return { estimate: mean(deltas), low: draws[Math.floor(samples * 0.025)], high: draws[Math.ceil(samples * 0.975) - 1] }
}
function aggregate(rows: Observation[]) {
  const rate = (key: "fixed" | "regression" | "completed") => wilson(rows.filter(x => x[key]).length, rows.length)
  const evidenceRate = (key: "receiptsPass" | "verifierPass") => rows.every(x => x[key] === null) ? null : wilson(rows.filter(x => x[key]).length, rows.length)
  return { n: rows.length, fixRate: rate("fixed"), regressionRate: rate("regression"), completionRate: rate("completed"),
    receiptsPassRate: evidenceRate("receiptsPass"), verifierPassRate: evidenceRate("verifierPass"),
    meanTokens: rows.every(x => x.usage !== null) ? mean(rows.map(x => tokenCount(x.usage!))) : null,
    meanWallMs: mean(rows.map(x => x.wallMs)), errors: rows.filter(x => x.error).length,
    estimatedUsage: rows.some(x => x.usage?.estimated), missingUsage: rows.filter(x => x.usage === null).length }
}
export function summarize(rows: Observation[], seed: number, samples = 2000) {
  const ids = [...new Set(rows.map(x => x.fixtureId))]
  if (!ids.length) throw new Error("empty benchmark")
  const pairs = ids.map(id => {
    const pair = rows.filter(x => x.fixtureId === id)
    const naked = pair.find(x => x.mode === "naked"), system = pair.find(x => x.mode === "system")
    if (pair.length !== 2 || !naked || !system || naked.backendId !== system.backendId) throw new Error("incomplete or unpaired benchmark")
    return { naked, system }
  })
  if (new Set(rows.map(x => x.backendId)).size !== 1) throw new Error("benchmark must use one backbone")
  return {
    naked: aggregate(pairs.map(x => x.naked)), system: aggregate(pairs.map(x => x.system)),
    lift: {
      fixRate: bootstrap(pairs.map(x => Number(x.system.fixed) - Number(x.naked.fixed)), seed, samples),
      regressionRate: bootstrap(pairs.map(x => Number(x.system.regression) - Number(x.naked.regression)), seed, samples),
      completionRate: bootstrap(pairs.map(x => Number(x.system.completed) - Number(x.naked.completed)), seed, samples),
      tokens: pairs.every(x => x.naked.usage && x.system.usage) ? bootstrap(pairs.map(x => tokenCount(x.system.usage!) - tokenCount(x.naked.usage!)), seed, samples) : null,
      wallMs: bootstrap(pairs.map(x => x.system.wallMs - x.naked.wallMs), seed, samples),
    },
  }
}
export type Summary = ReturnType<typeof summarize>
export function markdown(summary: Summary, eligible: boolean, reasons: string[]): string {
  const pct = (x: Interval) => `${(x.estimate * 100).toFixed(1)}% [${(x.low * 100).toFixed(1)}, ${(x.high * 100).toFixed(1)}]`
  const delta = (x: Interval) => `${(x.estimate * 100).toFixed(1)} pp [${(x.low * 100).toFixed(1)}, ${(x.high * 100).toFixed(1)}]`
  const numeric = (x: Interval | null) => x ? `${x.estimate.toFixed(1)} [${x.low.toFixed(1)}, ${x.high.toFixed(1)}]` : "unavailable"
  const num = (x: number | null) => x === null ? "unavailable" : x.toFixed(1)
  return `# Tiny-profile system-lift benchmark\n\nStatus: ${eligible ? "ELIGIBLE (adapter gate attestations recorded)" : "HARNESS ONLY — NOT VALID SYSTEM-LIFT EVIDENCE"}\n\n` +
    reasons.map(x => `- ${x}\n`).join("") +
    "\n| Metric | Naked | System | System − naked (95% CI) |\n|---|---:|---:|---:|\n" +
    `| Fixtures | ${summary.naked.n} | ${summary.system.n} | paired |\n` +
    `| Fix rate (target + regression tests) | ${pct(summary.naked.fixRate)} | ${pct(summary.system.fixRate)} | ${delta(summary.lift.fixRate)} |\n` +
    `| Regression rate | ${pct(summary.naked.regressionRate)} | ${pct(summary.system.regressionRate)} | ${delta(summary.lift.regressionRate)} |\n` +
    `| Completed | ${pct(summary.naked.completionRate)} | ${pct(summary.system.completionRate)} | ${delta(summary.lift.completionRate)} |\n` +
    `| Receipt pass | n/a | ${summary.system.receiptsPassRate ? pct(summary.system.receiptsPassRate) : "n/a"} | n/a |\n` +
    `| Verifier pass | n/a | ${summary.system.verifierPassRate ? pct(summary.system.verifierPassRate) : "n/a"} | n/a |\n` +
    `| Mean tokens | ${num(summary.naked.meanTokens)} | ${num(summary.system.meanTokens)} | ${numeric(summary.lift.tokens)} |\n` +
    `| Mean wall ms | ${num(summary.naked.meanWallMs)} | ${num(summary.system.meanWallMs)} | ${numeric(summary.lift.wallMs)} |\n` +
    `| Runner errors | ${summary.naked.errors} | ${summary.system.errors} | n/a |\n\n` +
    "Rates use Wilson 95% intervals. Lift uses a seeded paired percentile bootstrap. Positive fix lift is better; negative regression/token/time deltas are better. Mock tokens are estimates. These small synthetic fixture families do not establish general coding performance.\n"
}
