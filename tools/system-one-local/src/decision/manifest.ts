import { createHash } from "node:crypto";
import type { CalibrationScope, DecisionCalibrationManifestV1, DecisionRequestV1 } from "./contract.ts";
import { accuracy, brier, coverageAtRisk, ece, eceUpperBound, flipSensitivity, macroF1, nll, orderSensitivity, wilsonUpper, type Sample } from "./metrics.ts";
import { evaluateQ22Gate, GATE } from "./gate.ts";

export function canonicalJson(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonicalJson).join(",")}]`;
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o).sort().filter((k) => o[k] !== undefined).map((k) => `${JSON.stringify(k)}:${canonicalJson(o[k])}`).join(",")}}`;
  }
  return JSON.stringify(v);
}
export const sha256Hash = (s: string) => `sha256:${createHash("sha256").update(s).digest("hex")}`;

export function scopeFingerprint(scope: CalibrationScope): string {
  return sha256Hash(canonicalJson({ ...scope, readout_point: scope.readout_point ?? null }));
}

/** Hash of the candidate SCHEMA (operation, scale, constraints), independent of which candidates are present. */
export function candidateSchemaHash(req: Pick<DecisionRequestV1, "operation" | "scale" | "constraints">): string {
  return sha256Hash(canonicalJson({ operation: req.operation, scale: req.scale ?? null, constraints: req.constraints ?? [] }));
}
/** Hash of the concrete candidate set (ids, order-insensitive). */
export function candidateSetHash(req: Pick<DecisionRequestV1, "candidates">): string {
  return sha256Hash(canonicalJson([...(req.candidates ?? [])].map((c) => c.candidate_id).sort()));
}

export type BindResult = { ok: true } | { ok: false; reason: string };

/** A manifest may serve a request only if the runtime scope matches it exactly and the fingerprint is genuine. */
export function checkBinding(m: DecisionCalibrationManifestV1, runtime: CalibrationScope): BindResult {
  if (scopeFingerprint(m.scope) !== m.scope_fingerprint) return { ok: false, reason: "scope_fingerprint does not match manifest scope (tampered or stale)" };
  const keys = Object.keys(m.scope) as (keyof CalibrationScope)[];
  for (const k of new Set([...keys, ...(Object.keys(runtime) as (keyof CalibrationScope)[])])) {
    if ((m.scope[k] ?? null) !== (runtime[k] ?? null)) return { ok: false, reason: `scope mismatch on ${k}` };
  }
  return { ok: true };
}

export interface BuildOptions {
  manifest_id: string; primitive_id: string; scope: CalibrationScope;
  /** Held-out samples, disjoint from any fit set. probs must already be the calibrated readout. */
  heldOut: Sample[]; autoMinConfidence: number; temperature?: number; dataset_ref?: string;
  coverage?: { min_candidates?: number; max_candidates?: number; calibration_domain?: string };
  now?: Date;
}

export function buildManifest(o: BuildOptions): DecisionCalibrationManifestV1 {
  const ss = o.heldOut;
  const auto = ss.filter((s) => Math.max(...s.probs) >= o.autoMinConfidence);
  const wrong = auto.filter((s) => s.probs.indexOf(Math.max(...s.probs)) !== s.label).length;
  const m: DecisionCalibrationManifestV1 = {
    schema_id: "allternit.kernel.DecisionCalibrationManifestV1",
    schema_version: "1.0.0",
    manifest_id: o.manifest_id,
    primitive_id: o.primitive_id,
    scope: o.scope,
    scope_fingerprint: scopeFingerprint(o.scope),
    metrics: {
      ece: ece(ss), brier: brier(ss), nll: nll(ss), accuracy: accuracy(ss), f1: macroF1(ss),
      coverage_at_risk: coverageAtRisk(ss, GATE.auto_act_error_max),
      flip_sensitivity: flipSensitivity(ss), order_sensitivity: orderSensitivity(ss),
    },
    held_out: {
      n: ss.length, ci_level: 0.95, ci_method: "bootstrap-ece/wilson-error",
      ece_upper_bound: eceUpperBound(ss),
      auto_act_n: auto.length,
      auto_act_error_rate: auto.length ? wrong / auto.length : 0,
      auto_act_error_upper_bound: wilsonUpper(wrong, auto.length),
      dataset_ref: o.dataset_ref ?? null,
    },
    coverage_region: { ...o.coverage, min_confidence: o.autoMinConfidence },
    gate: { ece_max: 0.05, auto_act_error_max: 0.05, reversible_only: true, passed: false, agreement_with_other_model_used: false },
    created_at: (o.now ?? new Date()).toISOString(),
    extensions: { "x-temperature": o.temperature ?? 1, "x-auto_min_confidence": o.autoMinConfidence },
  };
  m.gate.passed = evaluateQ22Gate(m).passed;
  return m;
}
