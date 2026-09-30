// Types mirroring the frozen Kernel ABI 1.0.0 decision contracts
// (spec/Contracts/kernel/v1/schemas/decision.schema.json). No vendor or model
// names belong in this file; concrete model ids are manifest DATA only.

export type DecisionOperation = "BELIEF" | "CHOICE" | "SCORE" | "RANK" | "SUBSET" | "ESTIMATE" | "GATE" | "VERIFY" | "PAIR_SCORE";
export type ConfidenceSemantics = "CALIBRATED" | "RELATIVE_SET" | "UNCALIBRATED";
export type CalibrationLevel = "RAW" | "L0" | "L1" | "L2" | "NONE";
export type ThresholdAction = "AUTO" | "REVIEW" | "ESCALATE" | "REJECT";
export type LatencyClass = "REALTIME" | "INTERACTIVE" | "BACKGROUND" | "BATCH";

export interface Candidate {
  candidate_id: string;
  label?: string | null;
  payload?: unknown;
  is_unknown?: boolean;
  extensions?: Record<string, unknown>;
}

export interface DecisionRequestV1 {
  envelope: Record<string, unknown>;
  operation: DecisionOperation;
  state_projection_ref: string;
  instructions: string;
  question_id?: string | null;
  decision_bank_id: string;
  candidates?: Candidate[];
  scale?: string[];
  constraints?: Record<string, unknown>[];
  calibration_domain?: string | null;
  threshold_profile_id?: string | null;
  max_latency_ms?: number | null;
  latency_class?: LatencyClass | null;
  extensions?: Record<string, unknown>;
}

export interface DecisionResultV1 {
  envelope: Record<string, unknown>;
  operation: DecisionOperation;
  answer: unknown;
  probabilities?: Record<string, number> | null;
  confidence: number;
  confidence_semantics: ConfidenceSemantics;
  calibration_level_served: CalibrationLevel;
  calibration_id?: string | null;
  calibration_fingerprint?: string | null;
  feasible?: boolean | null;
  constraint_violations?: unknown[];
  evidence_refs: unknown[];
  backend_id?: string | null;
  model_impl?: string | null;
  threshold_action: ThresholdAction;
  latency_ms: number;
  abstained?: boolean;
  extensions?: Record<string, unknown>;
}

export interface CalibrationScope {
  backend_id: string;
  model_ref: string;
  model_revision: string;
  tokenizer_id: string;
  quantization: string;
  runtime_backend: string;
  question_id: string;
  candidate_schema_hash: string;
  candidate_set_hash: string;
  threshold_profile: string;
  readout_point?: string | null;
}

export interface CalibrationMetrics {
  ece: number | null; brier: number | null; nll: number | null; accuracy: number | null; f1: number | null;
  coverage_at_risk: number | null; flip_sensitivity: number | null; order_sensitivity: number | null;
}

export interface HeldOut {
  n: number; ci_level: number; ci_method: string; ece_upper_bound: number;
  auto_act_n: number; auto_act_error_rate: number; auto_act_error_upper_bound: number;
  dataset_ref?: string | null;
}

export interface CoverageRegion {
  min_candidates?: number; max_candidates?: number; calibration_domain?: string;
  /** Lowest calibrated confidence seen in held-out data among covered inputs. */
  min_confidence?: number;
}

export interface DecisionCalibrationManifestV1 {
  schema_id: "allternit.kernel.DecisionCalibrationManifestV1";
  schema_version: string;
  manifest_id: string;
  primitive_id: string;
  scope: CalibrationScope;
  scope_fingerprint: string;
  metrics: CalibrationMetrics;
  held_out: HeldOut;
  coverage_region?: CoverageRegion;
  gate: { ece_max: 0.05; auto_act_error_max: 0.05; reversible_only: true; passed: boolean; agreement_with_other_model_used?: false };
  created_at?: string;
  extensions?: { "x-temperature"?: number; "x-auto_min_confidence"?: number; [k: string]: unknown };
}
