/**
 * Paired statistics for measured loops.
 *
 * A measured loop compares a control arm (metric before the change) with a
 * treatment arm (metric after the change) over n paired runs. The verdict is
 * decided against a keep bar that is fixed before the loop starts and against
 * the noise floor measured from unchanged (A/A) runs.
 */

export type MetricDirection = "lower" | "higher"
export type ArmVerdict = "KEEP" | "MINOR" | "DROP" | "INCONCLUSIVE"

export interface ArmStatsInput {
  /** One entry per round; null means the instrument failed that round. */
  control: Array<number | null>
  treatment: Array<number | null>
  direction: MetricDirection
  /** Fractional improvement required to keep, e.g. 0.05 = 5%. */
  keepBar: number
  /** Fractional A/A noise floor from `noiseFloor()`. */
  noise: number
  minPairs: number
  seed?: number
}

export interface ArmStatsResult {
  verdict: ArmVerdict
  why: string
  pairs: number
  /** Median fractional improvement (positive = better). */
  delta: number
  ci: [number, number] | null
  wins: number
}

/** Fractional improvement of treatment over control; positive is better. */
export function improvement(control: number, treatment: number, direction: MetricDirection): number {
  const base = Math.abs(control) || 1e-12
  return direction === "lower" ? (control - treatment) / base : (treatment - control) / base
}

export function median(xs: number[]): number {
  if (xs.length === 0) return 0
  const s = [...xs].sort((a, b) => a - b)
  const m = s.length >> 1
  return s.length % 2 ? s[m]! : (s[m - 1]! + s[m]!) / 2
}

function mulberry32(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/** Seeded bootstrap 95% confidence interval of the median. */
export function bootstrapCI(xs: number[], seed = 1, resamples = 2000): [number, number] {
  if (xs.length === 0) return [0, 0]
  const rnd = mulberry32(seed)
  const meds: number[] = []
  for (let i = 0; i < resamples; i++) {
    const sample: number[] = []
    for (let j = 0; j < xs.length; j++) sample.push(xs[Math.floor(rnd() * xs.length)]!)
    meds.push(median(sample))
  }
  meds.sort((a, b) => a - b)
  return [meds[Math.floor(0.025 * (resamples - 1))]!, meds[Math.ceil(0.975 * (resamples - 1))]!]
}

/**
 * Noise floor from runs of an unchanged tree: the median absolute fractional
 * difference between neighbouring runs.
 */
export function noiseFloor(runs: Array<number | null>): number {
  const ok = runs.filter((x): x is number => x !== null)
  const diffs: number[] = []
  for (let i = 1; i < ok.length; i++) {
    const base = Math.abs(ok[i - 1]!) || 1e-12
    diffs.push(Math.abs(ok[i]! - ok[i - 1]!) / base)
  }
  return median(diffs)
}

export function armStats(input: ArmStatsInput): ArmStatsResult {
  const rounds = Math.max(input.control.length, input.treatment.length)
  const failed = (arm: Array<number | null>) => {
    let n = 0
    for (let i = 0; i < rounds; i++) if (arm[i] == null) n++
    return n
  }
  const cf = failed(input.control)
  const tf = failed(input.treatment)
  const deltas: number[] = []
  for (let i = 0; i < rounds; i++) {
    const c = input.control[i]
    const t = input.treatment[i]
    if (c != null && t != null) deltas.push(improvement(c, t, input.direction))
  }
  const delta = median(deltas)
  const wins = deltas.filter((d) => d > 0).length
  const base = { pairs: deltas.length, delta, wins }

  // 1. Reliability: a change that breaks the instrument is not an improvement.
  if (tf - cf >= 2) {
    return { ...base, ci: null, verdict: "DROP", why: `reliability: instrument failed ${tf}/${rounds} rounds after the change vs ${cf}/${rounds} before` }
  }
  if (tf - cf === 1) {
    return { ...base, ci: null, verdict: "INCONCLUSIVE", why: `reliability: one extra failed round after the change (${tf} vs ${cf}); add rounds` }
  }
  // 2. Enough data.
  if (deltas.length < input.minPairs) {
    return { ...base, ci: null, verdict: "INCONCLUSIVE", why: `need ${input.minPairs} complete pairs, have ${deltas.length}` }
  }
  // 3. Effect size against the bar, the noise floor and the interval.
  const ci = bootstrapCI(deltas, input.seed ?? 1)
  if (delta <= 0 || ci[1] <= 0) {
    return { ...base, ci, verdict: "DROP", why: "median change is not an improvement" }
  }
  if (wins === deltas.length && delta >= input.keepBar && delta > input.noise && ci[0] > 0) {
    return { ...base, ci, verdict: "KEEP", why: "met the keep bar, beat the noise floor, won every pair" }
  }
  if (ci[0] > 0) {
    return { ...base, ci, verdict: "MINOR", why: "real improvement that misses the keep bar or lost a pair" }
  }
  return { ...base, ci, verdict: "INCONCLUSIVE", why: "inside the noise; add rounds or drop it" }
}
