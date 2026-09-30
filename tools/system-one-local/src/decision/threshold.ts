// Threshold Policy (CL-158). Thresholds are policy/config, NEVER set by the model.
// A profile maps a CALIBRATED confidence to a ThresholdAction.
import type { ThresholdAction } from "./contract.ts";

export interface ThresholdProfile {
  profile_id: string;
  /** confidence >= auto_min -> AUTO (only if every other gate also holds). */
  auto_min: number;
  /** review_min <= confidence < auto_min -> REVIEW. */
  review_min: number;
  /** reject_below <= confidence < review_min -> ESCALATE; below reject_below -> REJECT. */
  reject_below: number;
  /** AUTO is only ever allowed for reversible / low-consequence decisions. */
  reversible_only: true;
}

export function validateProfile(p: ThresholdProfile): string[] {
  const e: string[] = [];
  for (const k of ["auto_min", "review_min", "reject_below"] as const) if (!(p[k] >= 0 && p[k] <= 1)) e.push(`${k} outside [0,1]`);
  if (!(p.reject_below <= p.review_min && p.review_min <= p.auto_min)) e.push("need reject_below <= review_min <= auto_min");
  if (p.reversible_only !== true) e.push("reversible_only must be true");
  return e;
}

export function actionFor(p: ThresholdProfile, confidence: number): ThresholdAction {
  if (confidence >= p.auto_min) return "AUTO";
  if (confidence >= p.review_min) return "REVIEW";
  if (confidence >= p.reject_below) return "ESCALATE";
  return "REJECT";
}

/** Conservative default: AUTO only at >= 0.95 calibrated confidence. */
export const DEFAULT_PROFILE: ThresholdProfile = { profile_id: "threshold.default", auto_min: 0.95, review_min: 0.6, reject_below: 0.3, reversible_only: true };
