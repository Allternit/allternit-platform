// Offline calibration: dataset (harvested from real shadow logs) -> held-out
// split (time-based) -> temperature fit on the EARLIER part -> metrics + Q22
// gate on the LATER part -> manifest written ONLY if the gate passes.
// One manifest per exact scope (model/revision/runtime/question/candidate schema),
// because a manifest binds to its scope fingerprint. Agreement with another
// model is never computed or accepted here.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type { CalibrationScope, DecisionCalibrationManifestV1 } from "./contract.ts";
import { evaluateQ22Gate, DEFAULT_MIN, GATE } from "./gate.ts";
import { buildManifest, scopeFingerprint } from "./manifest.ts";
import { applyTemperature, fitTemperature, wilsonUpper, type Sample } from "./metrics.ts";
import type { DatasetRow } from "./shadow.ts";

export interface CalibrateOptions {
  primitive: string; model: string;
  /** Fraction of the NEWEST rows held out. Default 0.4. */
  holdout?: number;
  /** Also require upper confidence bounds within the gate limits. */
  strict?: boolean;
  now?: Date; datasetRef?: string;
}

export interface ScopeReport {
  scope: CalibrationScope; scope_fingerprint: string;
  n_total: number; n_fit: number; n_held_out: number;
  fit_window: [string, string] | null; held_out_window: [string, string] | null;
  temperature: number | null; auto_min_confidence: number | null;
  manifest: DecisionCalibrationManifestV1 | null;
  gate_passed: boolean; failures: string[];
  needs: string;
}
export interface CalibrationReport {
  primitive: string; model: string; created_at: string; rows_considered: number;
  passed: boolean; manifests_written: string[]; scopes: ScopeReport[]; notes: string[];
}

const toSample = (r: DatasetRow, T: number): Sample => ({
  probs: applyTemperature(r.readout.probs, T), label: r.label_index,
  ...(r.readout.probs_flipped ? { probs_flipped: applyTemperature(r.readout.probs_flipped, T) } : {}),
  ...(r.readout.probs_reordered ? { probs_reordered: applyTemperature(r.readout.probs_reordered, T) } : {}),
});

/** Lowest calibrated confidence whose FIT-set auto-act subset has a Wilson upper error bound <= the gate limit. */
export function chooseAutoMin(fit: Sample[], fallback = 0.95): number {
  for (let t = 0.5; t <= 0.9951; t += 0.005) {
    const auto = fit.filter((s) => Math.max(...s.probs) >= t);
    if (auto.length < 30) break;
    const wrong = auto.filter((s) => s.probs.indexOf(Math.max(...s.probs)) !== s.label).length;
    if (wilsonUpper(wrong, auto.length) <= GATE.auto_act_error_max) return Math.round(t * 1000) / 1000;
  }
  return fallback;
}

export function calibrate(rows: DatasetRow[], o: CalibrateOptions): CalibrationReport {
  const now = o.now ?? new Date();
  const holdout = o.holdout ?? 0.4;
  if (!(holdout > 0 && holdout < 1)) throw new Error("holdout must be in (0,1)");
  const mine = rows.filter((r) => r.primitive_id === o.primitive && r.scope.model_ref === o.model);
  const notes: string[] = [];
  if (rows.length && !mine.length) notes.push(`no rows match primitive=${o.primitive} model=${o.model}`);
  const groups = new Map<string, DatasetRow[]>();
  for (const r of mine) {
    const k = scopeFingerprint(r.scope);
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k)!.push(r);
  }
  const scopes: ScopeReport[] = [];
  for (const [fp, rs] of groups) {
    rs.sort((a, b) => a.ts.localeCompare(b.ts));
    const nHeld = Math.floor(rs.length * holdout);
    const fit = rs.slice(0, rs.length - nHeld), held = rs.slice(rs.length - nHeld);
    const win = (x: DatasetRow[]): [string, string] | null => (x.length ? [x[0].ts, x[x.length - 1].ts] : null);
    const base = { scope: rs[0].scope, scope_fingerprint: fp, n_total: rs.length, n_fit: fit.length, n_held_out: held.length, fit_window: win(fit), held_out_window: win(held) };
    if (fit.length < 30 || held.length < DEFAULT_MIN.held_out_n) {
      scopes.push({ ...base, temperature: null, auto_min_confidence: null, manifest: null, gate_passed: false,
        failures: [`insufficient data: fit n=${fit.length}, held-out n=${held.length} (need held-out >= ${DEFAULT_MIN.held_out_n})`],
        needs: `about ${Math.ceil(DEFAULT_MIN.held_out_n / holdout)} labeled rows minimum for this scope (have ${rs.length}); the auto-act floor of ${DEFAULT_MIN.auto_act_n} needs more if few rows are high-confidence` });
      continue;
    }
    const T = fitTemperature(fit.map((r) => toSample(r, 1)));
    const autoMin = chooseAutoMin(fit.map((r) => toSample(r, T)));
    const heldSamples = held.map((r) => toSample(r, T));
    const m = buildManifest({
      manifest_id: `cal.${o.primitive}.${fp.slice(7, 15)}.${now.toISOString().replace(/[-:.TZ]/g, "").slice(0, 14)}`,
      primitive_id: o.primitive, scope: rs[0].scope, heldOut: heldSamples, autoMinConfidence: autoMin, temperature: T,
      dataset_ref: o.datasetRef, now,
      coverage: { min_candidates: Math.min(...rs.map((r) => r.options.length)), max_candidates: Math.max(...rs.map((r) => r.options.length)) },
    });
    const g = evaluateQ22Gate(m, { strictBounds: o.strict });
    m.gate.passed = g.passed;
    scopes.push({ ...base, temperature: T, auto_min_confidence: autoMin, manifest: m, gate_passed: g.passed, failures: g.failures,
      needs: g.passed ? "" : m.held_out.auto_act_n < DEFAULT_MIN.auto_act_n
        ? "too few high-confidence held-out rows for the auto-act floor; accumulate more labeled data"
        : "calibration/accuracy insufficient on this data: more data will not help unless the readout quality improves" });
  }
  if (!scopes.length) notes.push("nothing to calibrate");
  return { primitive: o.primitive, model: o.model, created_at: now.toISOString(), rows_considered: mine.length,
    passed: scopes.length > 0 && scopes.every((s) => s.gate_passed), manifests_written: [], scopes, notes };
}

/** Atomically append manifests to the ALLTERNIT_S1_MANIFESTS JSON array (temp file + rename). Never truncates on parse error. */
export function appendManifests(path: string, ms: DecisionCalibrationManifestV1[]) {
  let cur: unknown[] = [];
  if (existsSync(path)) {
    const parsed = JSON.parse(readFileSync(path, "utf8"));
    if (!Array.isArray(parsed)) throw new Error(`${path} is not a JSON array; refusing to overwrite`);
    cur = parsed;
  }
  mkdirSync(dirname(path), { recursive: true });
  const tmp = `${path}.${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify([...cur, ...ms], null, 2) + "\n");
  renameSync(tmp, path);
}

/** Run calibrate and persist: manifests only for gate-passing scopes; the report always. */
export function calibrateAndWrite(rows: DatasetRow[], o: CalibrateOptions & { manifestsPath?: string; reportPath: string }): CalibrationReport {
  const rep = calibrate(rows, o);
  const passing = rep.scopes.filter((s) => s.gate_passed && s.manifest).map((s) => s.manifest!);
  if (passing.length) {
    if (!o.manifestsPath) rep.notes.push("gate passed but no manifests path (ALLTERNIT_S1_MANIFESTS / --manifests): nothing written");
    else { appendManifests(o.manifestsPath, passing); rep.manifests_written = passing.map((m) => m.manifest_id); }
  }
  mkdirSync(dirname(o.reportPath), { recursive: true });
  writeFileSync(o.reportPath, JSON.stringify(rep, null, 2) + "\n");
  return rep;
}
