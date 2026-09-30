// Calibration + decision-quality metrics. Pure, deterministic, unit-tested.
export interface Sample {
  /** Probabilities over the classes (sum ~1). */
  probs: number[];
  /** Index of the true class. */
  label: number;
  /** Probabilities after an option-polarity flip, mapped back to original class order. */
  probs_flipped?: number[];
  /** Probabilities after candidate reordering, mapped back to original class order. */
  probs_reordered?: number[];
}

const argmax = (p: number[]) => p.reduce((b, v, i) => (v > p[b] ? i : b), 0);
const conf = (s: Sample) => Math.max(...s.probs);
const eps = 1e-12;

export function ece(samples: Sample[], bins = 10): number {
  if (!samples.length) return 0;
  const acc = Array.from({ length: bins }, () => ({ n: 0, c: 0, ok: 0 }));
  for (const s of samples) {
    const c = conf(s);
    const b = acc[Math.min(bins - 1, Math.floor(c * bins))];
    b.n++; b.c += c; b.ok += argmax(s.probs) === s.label ? 1 : 0;
  }
  return acc.reduce((t, b) => t + (b.n ? (b.n / samples.length) * Math.abs(b.ok / b.n - b.c / b.n) : 0), 0);
}

export const accuracy = (ss: Sample[]) => (ss.length ? ss.filter((s) => argmax(s.probs) === s.label).length / ss.length : 0);

export function brier(ss: Sample[]): number {
  if (!ss.length) return 0;
  return ss.reduce((t, s) => t + s.probs.reduce((a, p, i) => a + (p - (i === s.label ? 1 : 0)) ** 2, 0), 0) / ss.length;
}

export const nll = (ss: Sample[]) => (ss.length ? ss.reduce((t, s) => t - Math.log(Math.max(s.probs[s.label], eps)), 0) / ss.length : 0);

export function macroF1(ss: Sample[]): number {
  if (!ss.length) return 0;
  const k = Math.max(...ss.map((s) => s.probs.length));
  const f: number[] = [];
  for (let c = 0; c < k; c++) {
    let tp = 0, fp = 0, fn = 0;
    for (const s of ss) {
      const p = argmax(s.probs);
      if (p === c && s.label === c) tp++; else if (p === c) fp++; else if (s.label === c) fn++;
    }
    if (tp + fp + fn === 0) continue; // class absent everywhere
    f.push((2 * tp) / (2 * tp + fp + fn));
  }
  return f.length ? f.reduce((a, b) => a + b, 0) / f.length : 0;
}

/** Largest fraction of samples that can be answered (highest confidence first) with error rate <= risk. */
export function coverageAtRisk(ss: Sample[], risk = 0.05): number {
  if (!ss.length) return 0;
  const sorted = [...ss].sort((a, b) => conf(b) - conf(a));
  let wrong = 0, best = 0;
  sorted.forEach((s, i) => {
    if (argmax(s.probs) !== s.label) wrong++;
    if (wrong / (i + 1) <= risk) best = i + 1;
  });
  return best / ss.length;
}

const changed = (ss: Sample[], pick: (s: Sample) => number[] | undefined) => {
  const have = ss.filter((s) => pick(s));
  return have.length ? have.filter((s) => argmax(pick(s)!) !== argmax(s.probs)).length / have.length : 0;
};
export const flipSensitivity = (ss: Sample[]) => changed(ss, (s) => s.probs_flipped);
export const orderSensitivity = (ss: Sample[]) => changed(ss, (s) => s.probs_reordered);

/** Wilson score upper bound for k errors in n trials (z=1.645 → one-sided 95%). */
export function wilsonUpper(k: number, n: number, z = 1.645): number {
  if (n <= 0) return 1;
  const p = k / n, z2 = z * z;
  return Math.min(1, (p + z2 / (2 * n) + z * Math.sqrt((p * (1 - p)) / n + z2 / (4 * n * n))) / (1 + z2 / n));
}

/** Seeded bootstrap upper bound (95th pct) on ECE. */
export function eceUpperBound(ss: Sample[], resamples = 200, seed = 1337): number {
  if (!ss.length) return 1;
  let x = seed >>> 0;
  const rnd = () => ((x = (Math.imul(x, 1664525) + 1013904223) >>> 0) / 2 ** 32);
  const vals: number[] = [];
  for (let r = 0; r < resamples; r++) vals.push(ece(ss.map(() => ss[Math.floor(rnd() * ss.length)])));
  vals.sort((a, b) => a - b);
  return vals[Math.min(vals.length - 1, Math.ceil(0.95 * vals.length) - 1)];
}

/** Temperature scaling (probs → softmax(log p / T)); grid-search T minimizing held-out NLL. */
export function applyTemperature(probs: number[], T: number): number[] {
  const z = probs.map((p) => Math.log(Math.max(p, eps)) / T);
  const m = Math.max(...z);
  const e = z.map((v) => Math.exp(v - m));
  const s = e.reduce((a, b) => a + b, 0);
  return e.map((v) => v / s);
}
export function fitTemperature(fitSet: Sample[]): number {
  let best = 1, bestNll = Infinity;
  for (let T = 0.25; T <= 5.0001; T += 0.05) {
    const v = nll(fitSet.map((s) => ({ ...s, probs: applyTemperature(s.probs, T) })));
    if (v < bestNll) { bestNll = v; best = T; }
  }
  return Math.round(best * 100) / 100;
}
