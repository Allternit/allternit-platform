// The Q26 S1 go-live gate, as code (07-decisions Q26, amends Q22).
//
// Per binding = decision bank × backend × revision × prompt/label schema × option set
// (the manifest scope fingerprint):
//   - temperature fitted per (bank, type, option count) on split A (tune);
//   - auto-act threshold τ chosen on split A by fixed-sequence Learn-Then-Test:
//     walk τ downward while the Clopper–Pearson upper bound on auto-act error ≤ ε;
//   - certified on the untouched split B (cert): CP upper bound (δ = 0.05) ≤ ε;
//   - non-inferior to the incumbent decider on the same auto-act rows;
//   - per-class floors; coverage ≥ 30%;
//   - ECE (equal-mass, bootstrap CI, auto-act region only) is reported as a
//     SECONDARY check and never decides on its own.
// ε by consequence: 10% where a mistake costs one cheap retry, 5% default;
// permission/money/client banks never auto-act (S1 may only tighten them).
// Sample minimums: ≥300 tuning and ≥500 certification rows per binding.
// A bank is eligible to go live only when one of its bindings passes; every
// other bank stays in shadow. LLM agreement is never an input.
import type { CalibrationScope, DecisionCalibrationManifestV1 } from "./contract.ts";
import { scopeFingerprint } from "./manifest.ts";
import { accuracy, applyTemperature, brier, fitTemperature, macroF1, nll, type Sample } from "./metrics.ts";
import type { DatasetRow } from "./shadow.ts";

export type Consequence = "cheap_retry" | "default" | "never";
export const Q26 = {
  delta: 0.05,
  epsilon: { cheap_retry: 0.10, default: 0.05 } as Record<Exclude<Consequence, "never">, number>,
  coverage_min: 0.30,
  min_tune_n: 300,
  min_cert_n: 500,
  /** Non-inferiority margin vs the incumbent (one-sided 95% upper bound on the paired error difference). */
  noninferiority_margin: 0.02,
  /** Per-class floor: auto-act error of any predicted class with ≥ class_min_n rows must be ≤ class_floor_factor × ε. */
  class_min_n: 20, class_floor_factor: 2,
} as const;

/** Banks that touch permission, money or client decisions never auto-act. Name-based default; a policy file can only add "never". */
const NEVER_RE = /(permission|approval|consequential|money|payment|billing|invoice|charge|spend|refund|client|customer_comm|send_email|guard)/i;

/**
 * Incumbent coverage: "required" = every auto-act row must carry x-incumbent; "none" = no
 * incumbent decider; "partial" = the incumbent answers only some rows (e.g. the executed
 * operation's target, not the speculative ones), so rows without x-incumbent are skipped.
 */
export type IncumbentPolicy = "required" | "none" | "partial";
export type QuestionPolicy = IncumbentPolicy | { incumbent?: IncumbentPolicy; why?: string };
export interface BankPolicy {
  consequence?: Consequence;
  incumbent?: IncumbentPolicy;
  /** Per-question overrides (question_id → incumbent policy); "*" matches any question, "*_x" any id ending in "_x". */
  questions?: Record<string, QuestionPolicy>;
  min_tune_n?: number;
  min_cert_n?: number;
  why?: string;
}
export type Q26Policy = Record<string, BankPolicy>;

/** Incumbent policy for one question of a bank: exact question id, then suffix wildcard, then "*", then the bank. */
export function incumbentPolicyFor(pol: BankPolicy | undefined, questionId: string | undefined): IncumbentPolicy {
  const qs = pol?.questions ?? {};
  const pick = (v: QuestionPolicy | undefined) => (typeof v === "string" ? v : v?.incumbent);
  const q = questionId ?? "";
  const suffix = Object.keys(qs).find((k) => k.startsWith("*") && k.length > 1 && q.endsWith(k.slice(1)));
  return pick(qs[q]) ?? (suffix ? pick(qs[suffix]) : undefined) ?? pick(qs["*"]) ?? pol?.incumbent ?? "required";
}

export function consequenceFor(bank: string, policy: Q26Policy = {}): Consequence {
  if (NEVER_RE.test(bank)) return "never";
  return policy[bank]?.consequence ?? "default";
}

// ---------- exact binomial bounds ----------
function logChoose(n: number, k: number) { return lgamma(n + 1) - lgamma(k + 1) - lgamma(n - k + 1); }
function lgamma(x: number): number {
  // Lanczos approximation (g=7, n=9).
  const c = [0.99999999999980993, 676.5203681218851, -1259.1392167224028, 771.32342877765313, -176.61502916214059, 12.507343278686905, -0.13857109526572012, 9.9843695780195716e-6, 1.5056327351493116e-7];
  if (x < 0.5) return Math.log(Math.PI / Math.sin(Math.PI * x)) - lgamma(1 - x);
  x -= 1;
  let a = c[0];
  const t = x + 7.5;
  for (let i = 1; i < 9; i++) a += c[i] / (x + i);
  return 0.5 * Math.log(2 * Math.PI) + (x + 0.5) * Math.log(t) - t + Math.log(a);
}
export function binomCdf(k: number, n: number, p: number): number {
  if (p <= 0) return 1; if (p >= 1) return k >= n ? 1 : 0;
  let s = 0;
  for (let i = 0; i <= k; i++) s += Math.exp(logChoose(n, i) + i * Math.log(p) + (n - i) * Math.log1p(-p));
  return Math.min(1, s);
}
/** One-sided Clopper–Pearson upper bound: the p with P(X ≤ k | n, p) = δ. n = 0 → 1. */
export function clopperPearsonUpper(k: number, n: number, delta: number = Q26.delta): number {
  if (n <= 0) return 1;
  if (k >= n) return 1;
  let lo = k / n, hi = 1;
  for (let i = 0; i < 60; i++) { const mid = (lo + hi) / 2; if (binomCdf(k, n, mid) > delta) lo = mid; else hi = mid; }
  return hi;
}

// ---------- helpers ----------
const argmax = (p: number[]) => p.indexOf(Math.max(...p));
const conf = (p: number[]) => Math.max(...p);
const toSample = (r: DatasetRow, T: number): Sample => ({ probs: applyTemperature(r.readout.probs, T), label: r.label_index });

/** Equal-mass ECE (Roelofs et al. 2022 recommendation for small n). */
export function eceEqualMass(ss: Sample[], bins = 10): number {
  if (!ss.length) return NaN;
  const s = [...ss].sort((a, b) => conf(a.probs) - conf(b.probs));
  const per = Math.max(1, Math.ceil(s.length / bins));
  let e = 0;
  for (let i = 0; i < s.length; i += per) {
    const b = s.slice(i, i + per);
    const c = b.reduce((t, x) => t + conf(x.probs), 0) / b.length;
    const a = b.filter((x) => argmax(x.probs) === x.label).length / b.length;
    e += (b.length / s.length) * Math.abs(c - a);
  }
  return e;
}
function bootstrapUpper(ss: Sample[], f: (x: Sample[]) => number, resamples = 200, seed = 4242): number {
  if (!ss.length) return NaN;
  let x = seed >>> 0;
  const rnd = () => ((x = (Math.imul(x, 1664525) + 1013904223) >>> 0) / 2 ** 32);
  const vals: number[] = [];
  for (let r = 0; r < resamples; r++) vals.push(f(Array.from({ length: ss.length }, () => ss[Math.floor(rnd() * ss.length)])));
  vals.sort((a, b) => a - b);
  return vals[Math.floor(0.95 * (vals.length - 1))];
}

/** Fixed-sequence Learn-Then-Test on split A: the lowest τ (walking down from 0.99) whose CP upper bound stays ≤ ε. */
export function chooseTau(tune: Sample[], epsilon: number, delta: number = Q26.delta): number | null {
  let best: number | null = null;
  for (let t = 0.99; t >= 0.5 - 1e-9; t -= 0.01) {
    const tau = Math.round(t * 100) / 100;
    const auto = tune.filter((s) => conf(s.probs) >= tau);
    if (!auto.length) continue;
    const wrong = auto.filter((s) => argmax(s.probs) !== s.label).length;
    if (clopperPearsonUpper(wrong, auto.length, delta) <= epsilon) best = tau; else break;
  }
  return best;
}

export interface BindingReport {
  bank: string; type: string; k: number; scope: CalibrationScope; scope_fingerprint: string;
  consequence: Consequence; epsilon: number | null;
  n_tune: number; n_cert: number; temperature: number | null; tau: number | null;
  cert: { auto_n: number; errors: number; error_rate: number; error_upper: number; coverage: number } | null;
  noninferiority: { checked: boolean; n: number; s1_only_wrong: number; incumbent_only_wrong: number; diff_upper: number | null; reason?: string } | null;
  per_class: { label: number; auto_n: number; error_rate: number; floor: number; ok: boolean }[];
  secondary: { ece_equal_mass: number | null; ece_upper_95: number | null; accuracy: number; brier: number; nll: number; macro_f1: number } | null;
  passed: boolean; failures: string[];
  manifest: DecisionCalibrationManifestV1 | null;
}
export interface Q26Report {
  gate: "Q26"; created_at: string; delta: number;
  banks: Record<string, { eligible: boolean; consequence: Consequence; epsilon: number | null; passing_bindings: string[]; failing_bindings: number }>;
  temperatures: { bank: string; type: string; k: number; n: number; temperature: number }[];
  bindings: BindingReport[];
  notes: string[];
}
export interface Q26Options { policy?: Q26Policy; now?: Date; datasetRef?: string }

const typeOf = (r: DatasetRow) => ((r as any).laya?.question?.type as string | undefined) ?? r.operation.toUpperCase();

/**
 * Run Q26. `tune` and `cert` must be disjoint (cert untouched by any fitting); rows carry the
 * readout of the exact backend revision being certified (e.g. a fine-tuned checkpoint's
 * scored-tune/scored-cert files, or ledger rows for the revision that served them).
 */
export function runQ26(tune: DatasetRow[], cert: DatasetRow[], o: Q26Options = {}): Q26Report {
  const now = o.now ?? new Date();
  const policy = o.policy ?? {};
  const notes: string[] = [];
  const certIds = new Set(cert.map((r) => r.decision_id));
  const overlap = tune.filter((r) => certIds.has(r.decision_id)).length;
  if (overlap) throw new Error(`tune and cert splits overlap on ${overlap} decisions: certification must be untouched`);
  for (const r of [...tune, ...cert]) if (r.readout.probs.length !== r.options.length) throw new Error(`row ${r.decision_id}: probs/options length mismatch`);

  // Temperatures per (bank, type, k), fit on split A only.
  const tKey = (r: DatasetRow) => `${r.primitive_id}\u0000${typeOf(r)}\u0000${r.options.length}\u0000${r.scope.backend_id}\u0000${r.scope.model_revision}`;
  const tGroups = new Map<string, DatasetRow[]>();
  for (const r of tune) { const k = tKey(r); if (!tGroups.has(k)) tGroups.set(k, []); tGroups.get(k)!.push(r); }
  const temps = new Map<string, number>();
  const temperatures: Q26Report["temperatures"] = [];
  for (const [k, rs] of tGroups) {
    const T = fitTemperature(rs.map((r) => toSample(r, 1)));
    temps.set(k, T);
    temperatures.push({ bank: rs[0].primitive_id, type: typeOf(rs[0]), k: rs[0].options.length, n: rs.length, temperature: T });
  }

  // Gate per binding (scope fingerprint within a bank).
  const bKey = (r: DatasetRow) => `${r.primitive_id}\u0000${scopeFingerprint(r.scope)}`;
  const byBinding = new Map<string, { tune: DatasetRow[]; cert: DatasetRow[] }>();
  for (const [split, rs] of [["tune", tune], ["cert", cert]] as const) for (const r of rs) {
    const k = bKey(r);
    if (!byBinding.has(k)) byBinding.set(k, { tune: [], cert: [] });
    byBinding.get(k)![split].push(r);
  }
  const bindings: BindingReport[] = [];
  for (const { tune: tu, cert: ce } of byBinding.values()) {
    const any = tu[0] ?? ce[0];
    const bank = any.primitive_id, fp = scopeFingerprint(any.scope);
    const cons = consequenceFor(bank, policy);
    const eps = cons === "never" ? null : Q26.epsilon[cons];
    const pol = policy[bank] ?? {};
    const minTune = Math.max(Q26.min_tune_n, pol.min_tune_n ?? 0), minCert = Math.max(Q26.min_cert_n, pol.min_cert_n ?? 0);
    const rep: BindingReport = {
      bank, type: typeOf(any), k: any.options.length, scope: any.scope, scope_fingerprint: fp, consequence: cons, epsilon: eps,
      n_tune: tu.length, n_cert: ce.length, temperature: null, tau: null, cert: null, noninferiority: null, per_class: [], secondary: null,
      passed: false, failures: [], manifest: null,
    };
    bindings.push(rep);
    if (cons === "never") { rep.failures.push("permission/money/client bank: never auto-acts (S1 may only tighten)"); continue; }
    if (tu.length < minTune) rep.failures.push(`tuning split n=${tu.length} < ${minTune}`);
    if (ce.length < minCert) rep.failures.push(`certification split n=${ce.length} < ${minCert}`);
    if (rep.failures.length) continue;
    const T = temps.get(tKey(tu[0])) ?? 1;
    rep.temperature = T;
    const tau = chooseTau(tu.map((r) => toSample(r, T)), eps!);
    rep.tau = tau;
    if (tau === null) { rep.failures.push(`no threshold on split A keeps the CP upper error bound ≤ ${eps}`); continue; }
    const cs = ce.map((r) => toSample(r, T));
    const autoIdx = cs.map((s, i) => (conf(s.probs) >= tau ? i : -1)).filter((i) => i >= 0);
    const wrong = autoIdx.filter((i) => argmax(cs[i].probs) !== cs[i].label);
    const errUpper = clopperPearsonUpper(wrong.length, autoIdx.length);
    const coverage = autoIdx.length / cs.length;
    rep.cert = { auto_n: autoIdx.length, errors: wrong.length, error_rate: autoIdx.length ? wrong.length / autoIdx.length : 0, error_upper: errUpper, coverage };
    if (!(errUpper <= eps!)) rep.failures.push(`split B auto-act error upper bound ${errUpper.toFixed(4)} > ε ${eps}`);
    if (!(coverage >= Q26.coverage_min)) rep.failures.push(`coverage ${coverage.toFixed(3)} < ${Q26.coverage_min}`);

    // Non-inferiority vs the incumbent on the same auto-act rows (paired; McNemar-style normal bound).
    const incPol = (i: number) => incumbentPolicyFor(pol, ce[i].question?.question_id);
    const covered = autoIdx.filter((i) => incPol(i) !== "none");
    if (autoIdx.length && !covered.length) rep.noninferiority = { checked: false, n: 0, s1_only_wrong: 0, incumbent_only_wrong: 0, diff_upper: null, reason: "policy: bank has no incumbent decider" };
    else {
      const missing = covered.filter((i) => ce[i].incumbent == null && incPol(i) === "required");
      const withInc = covered.filter((i) => ce[i].incumbent != null);
      if (missing.length) {
        rep.noninferiority = { checked: false, n: withInc.length, s1_only_wrong: 0, incumbent_only_wrong: 0, diff_upper: null, reason: `incumbent answer missing on ${missing.length} auto-act rows` };
        rep.failures.push("non-inferiority not provable: incumbent answers missing (log x-incumbent, or set policy incumbent=none)");
      } else if (!withInc.length && covered.length) {
        rep.noninferiority = { checked: false, n: 0, s1_only_wrong: 0, incumbent_only_wrong: 0, diff_upper: null, reason: "policy: partial incumbent, none on the auto-act rows" };
      } else {
        let b = 0, c = 0;
        for (const i of withInc) {
          const s1Wrong = argmax(cs[i].probs) !== cs[i].label, incWrong = ce[i].incumbent !== ce[i].label;
          if (s1Wrong && !incWrong) b++; else if (!s1Wrong && incWrong) c++;
        }
        const n = withInc.length || 1;
        const diffUpper = (b - c) / n + 1.645 * Math.sqrt(b + c) / n;
        rep.noninferiority = { checked: true, n: withInc.length, s1_only_wrong: b, incumbent_only_wrong: c, diff_upper: diffUpper };
        if (diffUpper > Q26.noninferiority_margin) rep.failures.push(`inferior to the incumbent: paired error-difference upper bound ${diffUpper.toFixed(4)} > ${Q26.noninferiority_margin}`);
      }
    }
    // Per-class floors (by predicted class in the auto-act region).
    const floor = Q26.class_floor_factor * eps!;
    for (let c = 0; c < any.options.length; c++) {
      const rows = autoIdx.filter((i) => argmax(cs[i].probs) === c);
      if (rows.length < Q26.class_min_n) continue;
      const er = rows.filter((i) => cs[i].label !== c).length / rows.length;
      rep.per_class.push({ label: c, auto_n: rows.length, error_rate: er, floor, ok: er <= floor });
      if (er > floor) rep.failures.push(`class ${any.options[c]} auto-act error ${er.toFixed(3)} > floor ${floor}`);
    }
    // Secondary only: ECE in the auto-act region with a bootstrap CI.
    const autoS = autoIdx.map((i) => cs[i]);
    const e = eceEqualMass(autoS);
    rep.secondary = { ece_equal_mass: Number.isFinite(e) ? e : null, ece_upper_95: autoS.length ? bootstrapUpper(autoS, (x) => eceEqualMass(x)) : null,
      accuracy: accuracy(cs), brier: brier(cs), nll: nll(cs), macro_f1: macroF1(cs) };
    if (rep.secondary.ece_upper_95 !== null && rep.secondary.ece_upper_95 > 0.05) notes.push(`${bank}: secondary check — auto-act ECE upper bound ${rep.secondary.ece_upper_95.toFixed(3)} > 0.05 (reported, not gating)`);

    rep.passed = rep.failures.length === 0;
    if (rep.passed) rep.manifest = q26Manifest(rep, ce.length, autoS, now, o.datasetRef);
  }
  const banks: Q26Report["banks"] = {};
  for (const b of bindings) {
    const cur = banks[b.bank] ?? { eligible: false, consequence: b.consequence, epsilon: b.epsilon, passing_bindings: [], failing_bindings: 0 };
    if (b.passed) { cur.eligible = true; cur.passing_bindings.push(b.scope_fingerprint); } else cur.failing_bindings++;
    banks[b.bank] = cur;
  }
  if (!bindings.length) notes.push("no rows: nothing to certify");
  return { gate: "Q26", created_at: now.toISOString(), delta: Q26.delta, banks, temperatures, bindings, notes };
}

function q26Manifest(r: BindingReport, nCert: number, autoS: Sample[], now: Date, datasetRef?: string): DecisionCalibrationManifestV1 {
  const c = r.cert!;
  return {
    schema_id: "allternit.kernel.DecisionCalibrationManifestV1", schema_version: "1.0.0",
    manifest_id: `cal.q26.${r.bank}.${r.scope_fingerprint.slice(7, 15)}.${now.toISOString().replace(/[-:.TZ]/g, "").slice(0, 14)}`,
    primitive_id: r.bank, scope: r.scope, scope_fingerprint: r.scope_fingerprint,
    metrics: { ece: r.secondary?.ece_equal_mass ?? null, brier: r.secondary!.brier, nll: r.secondary!.nll, accuracy: r.secondary!.accuracy, f1: r.secondary!.macro_f1,
      coverage_at_risk: c.coverage, flip_sensitivity: null, order_sensitivity: null } as any,
    held_out: { n: nCert, ci_level: 1 - Q26.delta, ci_method: "clopper-pearson (split B) / bootstrap-ece (secondary)",
      ece_upper_bound: r.secondary?.ece_upper_95 ?? NaN, auto_act_n: c.auto_n, auto_act_error_rate: c.error_rate, auto_act_error_upper_bound: c.error_upper, dataset_ref: datasetRef ?? null },
    coverage_region: { min_candidates: r.k, max_candidates: r.k, min_confidence: r.tau! },
    gate: { ece_max: 0.05, auto_act_error_max: 0.05, reversible_only: true, passed: true, agreement_with_other_model_used: false },
    created_at: now.toISOString(),
    extensions: { "x-temperature": r.temperature!, "x-auto_min_confidence": r.tau!, "x-gate": "Q26",
      "x-q26": { epsilon: r.epsilon, delta: Q26.delta, consequence: r.consequence, tau: r.tau, n_tune: r.n_tune, n_cert: nCert, auto_n: c.auto_n,
        error_upper: c.error_upper, coverage: c.coverage, noninferiority: r.noninferiority, type: r.type, k: r.k } },
  };
}

/** Router-side re-check of a stored Q26 manifest (never trusts gate.passed alone). */
export function evaluateQ26Manifest(m: DecisionCalibrationManifestV1): { passed: boolean; failures: string[] } {
  const f: string[] = [];
  const q = m.extensions?.["x-q26"] as Record<string, any> | undefined;
  if (m.extensions?.["x-gate"] !== "Q26" || !q) return { passed: false, failures: ["not a Q26 manifest"] };
  if ((m.gate.agreement_with_other_model_used as unknown) === true) f.push("agreement with another model was used as a criterion");
  if (m.gate.reversible_only !== true) f.push("reversible_only must be true");
  if (consequenceFor(m.primitive_id) === "never" || q.consequence === "never") f.push("permission/money/client bank never auto-acts");
  if (!(typeof q.epsilon === "number" && q.epsilon <= Q26.epsilon.cheap_retry)) f.push("ε missing or above 10%");
  if (q.consequence === "default" && q.epsilon > Q26.epsilon.default) f.push("ε above 5% for a default-consequence bank");
  if (!(q.delta <= Q26.delta)) f.push(`δ ${q.delta} > ${Q26.delta}`);
  if (!(q.error_upper <= q.epsilon)) f.push(`auto-act error upper bound ${q.error_upper} > ε ${q.epsilon}`);
  if (!(q.coverage >= Q26.coverage_min)) f.push(`coverage ${q.coverage} < ${Q26.coverage_min}`);
  if (!(q.n_cert >= Q26.min_cert_n)) f.push(`certification n ${q.n_cert} < ${Q26.min_cert_n}`);
  if (!(q.n_tune >= Q26.min_tune_n)) f.push(`tuning n ${q.n_tune} < ${Q26.min_tune_n}`);
  if (m.gate.passed !== true) f.push("manifest.gate.passed=false");
  return { passed: f.length === 0, failures: f };
}
