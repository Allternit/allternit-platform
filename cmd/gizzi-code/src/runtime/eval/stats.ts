// Seeded percentile bootstrap, a port of cmd/allternit-api/src/agency_api/evals/stats.rs.
// Same outcomes and same seed give the same interval on every run.
// The PRNG is mulberry32, so numbers differ from the Rust SplitMix64 for the same seed.

export const DEFAULT_RESAMPLES = 2000
export const MAX_RESAMPLES = 20_000

export function mulberry32(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

export function rate(outcomes: boolean[]): number | null {
  return outcomes.length ? outcomes.filter(Boolean).length / outcomes.length : null
}

export function round4(x: number) {
  return Math.round(x * 10_000) / 10_000
}

/** Percentile bootstrap 95% interval of the pass rate. null for no cases. */
export function bootstrapCi95(
  outcomes: boolean[],
  seed = 1,
  resamples = DEFAULT_RESAMPLES,
): { low: number; high: number; method: string; resamples: number; seed: number } | null {
  const n = outcomes.length
  if (n === 0) return null
  const b = Math.min(Math.max(resamples, 100), MAX_RESAMPLES)
  const rnd = mulberry32(seed)
  const rates: number[] = []
  for (let i = 0; i < b; i++) {
    let hit = 0
    for (let j = 0; j < n; j++) if (outcomes[Math.floor(rnd() * n)]) hit++
    rates.push(hit / n)
  }
  rates.sort((x, y) => x - y)
  const lo = rates[Math.min(Math.floor(0.025 * b), b - 1)]!
  const hi = rates[Math.min(Math.max(Math.ceil(0.975 * b) - 1, 0), b - 1)]!
  return { low: round4(lo), high: round4(hi), method: "percentile_bootstrap", resamples: b, seed }
}
