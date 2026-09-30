// DecisionMotif library (CL-156). A motif is a named, reusable way of asking the
// decision runtime a question: it fixes the operation and safe defaults, and the
// caller supplies the state ref, instructions and candidates. Motifs carry no
// thresholds (those are ThresholdProfile policy).
import type { Candidate, DecisionOperation, DecisionRequestV1, LatencyClass } from "./contract.ts";

export type MotifId = "ROUTE" | "TRIAGE" | "RANK" | "GATE" | "JUDGE" | "LABEL" | "REFLEX" | "CONFIDENCE_GATE" | "BRANCH_PRUNE" | "COMPLETION_GATE";

export interface DecisionMotif {
  id: MotifId;
  operation: DecisionOperation;
  latency_class: LatencyClass;
  /** Needs an explicit "none of the above" candidate so the model can abstain. */
  needs_unknown: boolean;
  summary: string;
}

export const MOTIFS: Record<MotifId, DecisionMotif> = {
  ROUTE: { id: "ROUTE", operation: "CHOICE", latency_class: "INTERACTIVE", needs_unknown: true, summary: "pick one destination from a closed set" },
  TRIAGE: { id: "TRIAGE", operation: "CHOICE", latency_class: "INTERACTIVE", needs_unknown: true, summary: "assign an urgency/category bucket" },
  RANK: { id: "RANK", operation: "RANK", latency_class: "BACKGROUND", needs_unknown: false, summary: "order candidates best-first" },
  GATE: { id: "GATE", operation: "GATE", latency_class: "INTERACTIVE", needs_unknown: false, summary: "yes/no: may this proceed" },
  JUDGE: { id: "JUDGE", operation: "SCORE", latency_class: "BACKGROUND", needs_unknown: false, summary: "ordinal quality rating against a rubric" },
  LABEL: { id: "LABEL", operation: "CHOICE", latency_class: "BATCH", needs_unknown: true, summary: "assign one label from a closed vocabulary" },
  REFLEX: { id: "REFLEX", operation: "CHOICE", latency_class: "REALTIME", needs_unknown: true, summary: "fast lane: event to pre-authorized action, not a bypass" },
  CONFIDENCE_GATE: { id: "CONFIDENCE_GATE", operation: "GATE", latency_class: "INTERACTIVE", needs_unknown: false, summary: "is the upstream answer trustworthy enough to keep" },
  BRANCH_PRUNE: { id: "BRANCH_PRUNE", operation: "SUBSET", latency_class: "BACKGROUND", needs_unknown: false, summary: "keep a subset of candidate branches" },
  COMPLETION_GATE: { id: "COMPLETION_GATE", operation: "VERIFY", latency_class: "INTERACTIVE", needs_unknown: false, summary: "is the task done per the stated criteria" },
};

export interface MotifParams {
  envelope: Record<string, unknown>;
  state_projection_ref: string;
  instructions: string;
  decision_bank_id: string;
  question_id: string;
  candidates?: Candidate[];
  scale?: string[];
  calibration_domain?: string;
  threshold_profile_id?: string;
}

export const UNKNOWN_CANDIDATE: Candidate = { candidate_id: "unknown", label: "none of the above", is_unknown: true };

export function buildRequest(id: MotifId, p: MotifParams): DecisionRequestV1 {
  const m = MOTIFS[id];
  let candidates = p.candidates;
  if (["CHOICE", "RANK", "SUBSET"].includes(m.operation) && (!candidates || candidates.length < 2)) throw new Error(`${id} needs at least 2 candidates`);
  if (m.operation === "SCORE" && (!p.scale || p.scale.length < 2)) throw new Error(`${id} needs a scale of at least 2 levels`);
  if (m.needs_unknown && candidates && !candidates.some((c) => c.is_unknown)) candidates = [...candidates, UNKNOWN_CANDIDATE];
  return {
    envelope: p.envelope,
    operation: m.operation,
    state_projection_ref: p.state_projection_ref,
    instructions: p.instructions,
    question_id: p.question_id,
    decision_bank_id: p.decision_bank_id,
    ...(candidates ? { candidates } : {}),
    ...(p.scale ? { scale: p.scale } : {}),
    calibration_domain: p.calibration_domain ?? null,
    threshold_profile_id: p.threshold_profile_id ?? null,
    latency_class: m.latency_class,
    extensions: { "x-motif": id },
  };
}
