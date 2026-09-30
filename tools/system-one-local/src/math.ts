// Pure probability helpers. Everything here is deterministic and unit-tested.

/**
 * TypeSafe-style confidence: (n·max − 1)/(n − 1), clamped to [0, 1].
 * 0 for a uniform distribution, 1 when all mass is on one outcome.
 */
export function confidence(probs: number[]): number {
  const n = probs.length;
  if (n < 2) return 1;
  const max = Math.max(...probs);
  return clamp01((n * max - 1) / (n - 1));
}

/** Probability-weighted level index: Σ i·p_i (levels are 0-indexed). */
export function weightedScore(probs: number[]): number {
  return probs.reduce((acc, p, i) => acc + i * p, 0);
}

export function normalize(values: number[]): number[] {
  const sum = values.reduce((a, b) => a + b, 0);
  if (!(sum > 0)) return values.map(() => 1 / values.length);
  return values.map((v) => v / sum);
}

export interface TopLogprob {
  token: string;
  logprob: number;
}

export interface LabelDistribution {
  /** Probabilities over `labels`, renormalized to sum to 1. */
  probs: number[];
  /** Raw probability mass (before renormalization) that fell on valid labels. */
  labelMass: number;
  /** Labels that appeared in the top-k list. */
  seen: number;
}

/**
 * Turn the next-token top-k logprobs into a distribution over the valid labels.
 *
 * - Token text is trimmed, so " A" and "A" both count for label "A"; variants are summed.
 * - `caseInsensitive` also folds "yes"/"YES" into "Yes" (used for noul).
 * - Labels absent from the top-k are given a floor: each gets at most the smallest
 *   probability shown (a true upper bound, since it would otherwise have been listed),
 *   and together at most the leftover mass 1 − Σ top-k. Then everything is renormalized
 *   over the valid labels only.
 *
 * Returns `null` when no valid label appears in the top-k at all — the caller must
 * not invent a distribution from nothing (it falls back to sampling instead).
 */
export function labelDistribution(
  top: TopLogprob[],
  labels: string[],
  opts: { caseInsensitive?: boolean } = {},
): LabelDistribution | null {
  const key = (s: string) => (opts.caseInsensitive ? s.trim().toLowerCase() : s.trim());
  const index = new Map(labels.map((l, i) => [key(l), i]));
  const mass = new Array(labels.length).fill(0);
  let shownTotal = 0;
  let minShown = 1;
  for (const t of top) {
    const p = Math.exp(t.logprob);
    if (!Number.isFinite(p)) continue;
    shownTotal += p;
    minShown = Math.min(minShown, p);
    const i = index.get(key(t.token));
    if (i !== undefined) mass[i] += p;
  }
  const seen = mass.filter((m) => m > 0).length;
  if (seen === 0) return null;
  const labelMass = mass.reduce((a, b) => a + b, 0);
  const unseen = labels.length - seen;
  if (unseen > 0) {
    const leftover = Math.max(0, 1 - shownTotal);
    const floor = Math.min(minShown, leftover / unseen);
    for (let i = 0; i < mass.length; i++) if (mass[i] === 0) mass[i] = floor;
  }
  return { probs: normalize(mass), labelMass, seen };
}

/** Empirical distribution from k sampled labels (method: "sampled"). */
export function voteDistribution(samples: (string | null)[], labels: string[], opts: { caseInsensitive?: boolean } = {}): number[] {
  const key = (s: string) => (opts.caseInsensitive ? s.trim().toLowerCase() : s.trim());
  const index = new Map(labels.map((l, i) => [key(l), i]));
  const counts = new Array(labels.length).fill(0);
  for (const s of samples) {
    if (s == null) continue;
    const i = index.get(key(s));
    if (i !== undefined) counts[i]++;
  }
  return normalize(counts);
}

export function round(x: number, places = 4): number {
  const f = 10 ** places;
  return Math.round(x * f) / f;
}

/** Round a distribution and push the rounding residue onto the argmax so it still sums to 1. */
export function roundDistribution(probs: number[], places = 4): number[] {
  const r = probs.map((p) => round(p, places));
  const residue = round(1 - r.reduce((a, b) => a + b, 0), places);
  if (residue !== 0) {
    const argmax = r.indexOf(Math.max(...r));
    r[argmax] = round(r[argmax] + residue, places);
  }
  return r;
}

function clamp01(x: number) {
  return Math.max(0, Math.min(1, x));
}
