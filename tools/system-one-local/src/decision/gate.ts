// The Q22 go-live gate, as code (07-decisions Q22). One primitive at a time.
// LLM agreement is deliberately NOT an input: the manifest field for it must be false.
import type { DecisionCalibrationManifestV1 } from "./contract.ts";

export const GATE = { ece_max: 0.05, auto_act_error_max: 0.05 } as const;
/** "Statistically meaningful held-out sample" floors; raise per primitive, never lower. */
export const DEFAULT_MIN = { held_out_n: 300, auto_act_n: 100 } as const;

export interface GateOptions {
  minHeldOutN?: number; minAutoActN?: number;
  /** Also require the UPPER confidence bounds (not just point estimates) to be within the limits. */
  strictBounds?: boolean;
}
export interface GateResult { passed: boolean; failures: string[] }

export function evaluateQ22Gate(m: DecisionCalibrationManifestV1, o: GateOptions = {}): GateResult {
  const f: string[] = [];
  const h = m.held_out, x = m.metrics;
  const minN = Math.max(o.minHeldOutN ?? DEFAULT_MIN.held_out_n, DEFAULT_MIN.held_out_n);
  const minAuto = Math.max(o.minAutoActN ?? DEFAULT_MIN.auto_act_n, DEFAULT_MIN.auto_act_n);
  if ((m.gate.agreement_with_other_model_used as unknown) === true) f.push("agreement with another model was used as a criterion");
  if (x.ece === null || !(x.ece <= GATE.ece_max)) f.push(`held-out ECE ${x.ece} > ${GATE.ece_max}`);
  if (!(h.n >= minN)) f.push(`held-out n ${h.n} < ${minN}`);
  if (!(h.auto_act_n >= minAuto)) f.push(`auto-act subset n ${h.auto_act_n} < ${minAuto}`);
  if (!(h.auto_act_error_rate <= GATE.auto_act_error_max)) f.push(`auto-act error ${h.auto_act_error_rate} > ${GATE.auto_act_error_max}`);
  if (!Number.isFinite(h.ece_upper_bound) || !Number.isFinite(h.auto_act_error_upper_bound)) f.push("confidence bounds missing");
  if (o.strictBounds) {
    if (h.ece_upper_bound > GATE.ece_max) f.push(`ECE upper bound ${h.ece_upper_bound} > ${GATE.ece_max}`);
    if (h.auto_act_error_upper_bound > GATE.auto_act_error_max) f.push(`auto-act error upper bound ${h.auto_act_error_upper_bound} > ${GATE.auto_act_error_max}`);
  }
  if (m.gate.reversible_only !== true) f.push("reversible_only must be true");
  return { passed: f.length === 0, failures: f };
}
