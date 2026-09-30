/* Generated from spec/Contracts/kernel/v1/schemas (ABI 1.0.0) by scripts/regenerate.sh. DO NOT EDIT. */

/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "Id".
 */
export type Id = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "StateVersion".
 */
export type StateVersion = number;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ContentHash".
 */
export type ContentHash = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "SemVer".
 */
export type SemVer = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ResourceRef".
 */
export type ResourceRef = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "SensitivityClass".
 */
export type SensitivityClass = "PUBLIC" | "INTERNAL" | "RESTRICTED" | "SECRET";
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AbiVersion".
 */
export type AbiVersion = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "Timestamp".
 */
export type Timestamp = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "TrustClass".
 */
export type TrustClass = "PUBLIC" | "INTERNAL" | "RESTRICTED" | "SECRET" | "UNTRUSTED";
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "LogicalModelId".
 */
export type LogicalModelId = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CapabilityId".
 */
export type CapabilityId = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "TransferMode".
 */
export type TransferMode = "IDENTITY" | "PREFIX_REUSE" | "TRANSLATED_KV" | "SEMANTIC_REBUILD";
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "PrimitiveId".
 */
export type PrimitiveId = string;
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "LifecycleState".
 */
export type LifecycleState =
  | "DECLARED"
  | "ADMITTED"
  | "READY"
  | "LEASED"
  | "SPAWNED"
  | "RUNNING"
  | "OUTPUT_READY"
  | "VERIFYING"
  | "COMMITTED"
  | "CONTINUE"
  | "REPLAN"
  | "NEEDS_HUMAN"
  | "WAITING"
  | "CANCELLING"
  | "CLOSED";
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "RunStatus".
 */
export type RunStatus =
  | "accepted"
  | "running"
  | "waiting"
  | "needs_attention"
  | "paused"
  | "cancelling"
  | "completed"
  | "partial"
  | "failed"
  | "cancelled";

/**
 * Codegen bundle of Allternit Kernel ABI 1.0.0. Not normative; see schemas/.
 */
export interface AllternitKernelAbi {
  [k: string]: unknown;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AgentStateV1".
 */
export interface AgentStateV1 {
  schema_id: "allternit.kernel.AgentStateV1";
  schema_version: string;
  identity: {
    run_id: Id;
    session_id: Id;
    task_id: Id;
    state_version: StateVersion;
  };
  work_ref?: StateVersionRef | null;
  user_intent: {
    objective: string;
    task_ir_ref?: Id | null;
  };
  acceptance: {}[];
  constraints: Id[];
  environment: {
    workspace_root: string;
    repo_id?: string | null;
    languages?: string[];
    fingerprint?: ContentHash | null;
  };
  repository?: {} | null;
  active_packs: {
    pack_id: Id;
    pack_version: SemVer;
  }[];
  plan: {};
  graph_cursor: {
    graph_id: Id;
    graph_version: number;
    frontier?: Id[];
    completed_nodes?: Id[];
    failed_nodes?: Id[];
  };
  hypotheses?: {
    hypothesis_id: Id;
    statement: string;
    status: "OPEN" | "SUPPORTED" | "REFUTED" | "ABANDONED";
    evidence_refs?: EvidenceRef[];
  }[];
  candidate_targets?: {}[];
  selected_targets?: ResourceRef[];
  context_index?: Id[];
  relevant_context?: Id[];
  generated_artifacts?: ArtifactRef[];
  mutations?: Id[];
  command_history?: Id[];
  diagnostics?: {}[];
  verification?: Id[];
  unresolved_failures?: {}[];
  evidence: EvidenceRef[];
  permissions: {
    grant_ids: Id[];
    authority_profile?: ("read-only" | "code-safe" | "code-write") | null;
  };
  budgets: ResourceBudget;
  model_state?: {};
  subgoals?: {}[];
  completion: {
    coverage: {
      criterion_id: Id;
      satisfied: boolean;
      evidence_refs?: EvidenceRef[];
    }[];
    latest_decision_ref?: Id | null;
  };
  instruction_predicates: {
    predicate_id: Id;
    instruction_ref: Id;
    condition: string;
    pinned?: boolean;
  }[];
  sensitivity_labels: {
    resource: ResourceRef;
    class: SensitivityClass;
  }[];
  retrieval_snapshot_refs: {
    snapshot_id: Id;
    fingerprint: ContentHash;
  }[];
  user_visible_status?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "StateVersionRef".
 */
export interface StateVersionRef {
  store: "AGENT_STATE" | "WORK_LEDGER";
  run_id: Id;
  state_version: StateVersion;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ExtensionMap".
 */
export interface ExtensionMap {
  [k: string]: unknown;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "EvidenceRef".
 */
export interface EvidenceRef {
  kind:
    | "STATE"
    | "FILE"
    | "SYMBOL"
    | "TOOL_RECEIPT"
    | "ACTION_RECEIPT"
    | "MUTATION_RECEIPT"
    | "VERIFICATION_RECEIPT"
    | "POLICY_RECEIPT"
    | "SPAWN_RECEIPT"
    | "NODE_OUTPUT"
    | "TRACE"
    | "HUMAN"
    | "ATTENTION_RESOLUTION"
    | "DECISION_RESULT";
  ref: Id;
  content_hash?: ContentHash | null;
  claim?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ArtifactRef".
 */
export interface ArtifactRef {
  artifact_id: Id;
  content_hash: ContentHash;
  mime_type?: string | null;
  uri?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ResourceBudget".
 */
export interface ResourceBudget {
  max_cost_units?: number | null;
  max_tokens?: number | null;
  max_wall_ms?: number | null;
  max_tool_calls?: number | null;
  max_attempts?: number | null;
  max_spawns?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AttentionPolicyV1".
 */
export interface AttentionPolicyV1 {
  schema_id: "allternit.kernel.AttentionPolicyV1";
  schema_version: string;
  channels: string[];
  quiet_hours?: {} | null;
  interrupt_quiet_hours_at?: "critical" | "never";
  max_per_hour: number;
  max_per_day: number;
  dedupe_window_seconds: number;
  escalate_after_seconds?: number | null;
  defaults_version: string;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AttentionRequestV1".
 */
export interface AttentionRequestV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.AttentionRequestV1";
  };
  attention_id: Id;
  run_id: Id;
  campaign_id?: Id | null;
  kind: "approval" | "informational";
  reason:
    | "approval_required"
    | "clarification_needed"
    | "budget_exhausted"
    | "verification_inconclusive"
    | "low_confidence"
    | "unsafe_resume"
    | "review_requested";
  urgency: "low" | "normal" | "high" | "critical";
  expected_loss: string | null;
  topic: string;
  summary?: string | null;
  requested_channel?: string | null;
  dedupe_key: string;
  expires_at?: Timestamp | null;
  on_expiry: "reject" | "none";
  /**
   * @minItems 1
   */
  response_options: [
    "approval" | "rejection" | "data" | "message",
    ...("approval" | "rejection" | "data" | "message")[]
  ];
  policy_decision_id?: Id | null;
  evidence_refs: EvidenceRef[];
  status: "open" | "resolved" | "expired" | "withdrawn";
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AbiEnvelopeV1".
 */
export interface AbiEnvelopeV1 {
  abi_version: AbiVersion;
  schema_id: string;
  schema_version: SemVer;
  run_id: Id;
  session_id: Id;
  task_id: Id;
  graph_id?: Id | null;
  node_id?: Id | null;
  state_version: StateVersion;
  work_state_version?: StateVersion | null;
  created_at: Timestamp;
  producer: ComponentIdentity;
  trace_id: Id;
  parent_trace_id?: Id | null;
  /**
   * @minItems 1
   */
  provenance: [Provenance, ...Provenance[]];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ComponentIdentity".
 */
export interface ComponentIdentity {
  component_id: Id;
  component_version: string;
  kind:
    | "RUNTIME"
    | "PRIMITIVE"
    | "DECISION_BACKEND"
    | "MODEL_COMPONENT"
    | "TOOL"
    | "VERIFIER"
    | "POLICY_ENGINE"
    | "HUMAN"
    | "ADAPTER";
  logical_class?: string;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "Provenance".
 */
export interface Provenance {
  source_type: "USER" | "FILE" | "TOOL" | "MODEL" | "POLICY" | "SYSTEM" | "CACHE" | "RETRIEVAL";
  source_id: string;
  trust_class: TrustClass;
  content_hash?: ContentHash | null;
  observed_at?: Timestamp | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AttentionResolutionV1".
 */
export interface AttentionResolutionV1 {
  schema_id: "allternit.kernel.AttentionResolutionV1";
  schema_version: string;
  attention_id: Id;
  type: "approval" | "rejection" | "data" | "message" | "expired" | "withdrawn";
  outcome: "approved" | "rejected" | "answered" | "none";
  resolved_by?: Id | null;
  value?: unknown;
  resolved_at: Timestamp;
  receipt_id: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CompositeModelBundleV1".
 */
export interface CompositeModelBundleV1 {
  schema_id: "allternit.kernel.CompositeModelBundleV1";
  schema_version: string;
  logical_model_id: LogicalModelId;
  bundle_version: SemVer;
  /**
   * @minItems 1
   */
  required_abi_versions: [AbiVersion, ...AbiVersion[]];
  primitive_packs: Id[];
  compute_graphs: Id[];
  decision_banks: Id[];
  model_components: Id[];
  adapters?: Id[];
  decision_backends?: Id[];
  shared_backbone_id?: Id | null;
  decision_head_ids?: Id[];
  calibration_manifest_ids?: Id[];
  router_policy: Id;
  context_policy: Id;
  verification_policy: Id;
  completion_policies?: CompletionPolicyRef[];
  deployment_profiles: string[];
  eval_profile?: Id | null;
  hashes?: {
    [k: string]: ContentHash;
  };
  signatures: ReceiptSignature[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CompletionPolicyRef".
 */
export interface CompletionPolicyRef {
  policy_id: Id;
  policy_version: number;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ReceiptSignature".
 */
export interface ReceiptSignature {
  alg: "ed25519";
  key_id: string;
  value: string;
  domain: "allternit.receipt.v1";
  jwks_url?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AgentBundleManifestV1".
 */
export interface AgentBundleManifestV1 {
  schema_id: "allternit.kernel.AgentBundleManifestV1";
  schema_version: string;
  name: string;
  version: string;
  stable: boolean;
  /**
   * @minItems 1
   */
  profiles: ["read-only" | "code-safe" | "code-write", ...("read-only" | "code-safe" | "code-write")[]];
  bundle_ref: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "WakeTrigger".
 */
export interface WakeTrigger {
  kind: "TIMER" | "EVENT" | "DEPENDENCY" | "MANUAL" | "ATTENTION_RESOLVED";
  interval_seconds?: number | null;
  at?: Timestamp | null;
  event_type?: string | null;
  dependency_ref?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "WakePolicyV1".
 */
export interface WakePolicyV1 {
  schema_id: "allternit.kernel.WakePolicyV1";
  schema_version: string;
  /**
   * @minItems 1
   */
  triggers: [WakeTrigger, ...WakeTrigger[]];
  min_interval_seconds: number;
  max_wakes_per_day: number;
  skip_if_run_active: boolean;
  until?: Timestamp | null;
  defaults_version: string;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "WaitGateV1".
 */
export interface WaitGateV1 {
  schema_id: "allternit.kernel.WaitGateV1";
  schema_version: string;
  gate_id: Id;
  kind: "TIMER" | "EVENT" | "DEPENDENCY" | "MANUAL" | "ATTENTION";
  wake_key: string;
  not_before?: Timestamp | null;
  expires_at?: Timestamp | null;
  on_expiry?: "FAIL_NODE" | "CONTINUE" | "NEEDS_HUMAN";
  status: "ARMED" | "FIRED" | "EXPIRED" | "CANCELLED";
  attention_id?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "WakeEventV1".
 */
export interface WakeEventV1 {
  schema_id: "allternit.kernel.WakeEventV1";
  schema_version: string;
  wake_id: Id;
  wake_key: string;
  source: "TIMER" | "EVENT" | "DEPENDENCY" | "MANUAL" | "ATTENTION";
  fired_at: Timestamp;
  /**
   * @minItems 1
   */
  targets: [
    {
      kind: "WAIT_GATE" | "CAMPAIGN";
      id: Id;
    },
    ...{
      kind: "WAIT_GATE" | "CAMPAIGN";
      id: Id;
    }[]
  ];
  aggregated_count?: number | null;
  scheduling_enabled?: boolean;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CampaignV1".
 */
export interface CampaignV1 {
  schema_id: "allternit.kernel.CampaignV1";
  schema_version: string;
  campaign_id: Id;
  objective: string;
  graph_template: Id;
  agent_state_ref: StateVersionRef | null;
  budget: {
    per_run: ResourceBudget;
    max_cost_units?: number | null;
    max_runs?: number | null;
    on_exhaustion: "REQUEST_ATTENTION" | "STOP";
  };
  wake_policy: WakePolicyV1;
  attention_policy: AttentionPolicyV1;
  completion_criteria: CompletionPolicyRef;
  next_wake: Timestamp | null;
  last_wake?: Timestamp | null;
  owner: Id;
  status: "active" | "sleeping" | "running" | "needs_attention" | "paused" | "completed" | "failed" | "cancelled";
  active_run_ids?: Id[];
  scheduling: "disabled_pending_golive" | "enabled";
  defaults_version: string;
  work_state_version?: StateVersion | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CapabilityRequestV1".
 */
export interface CapabilityRequestV1 {
  schema_id: "allternit.kernel.CapabilityRequestV1";
  schema_version: string;
  capability: CapabilityId;
  task_shape?: string | null;
  language?: string | null;
  modality: "TEXT" | "CODE" | "IMAGE" | "MULTI";
  min_context_tokens?: number | null;
  required_output_schema?: string | null;
  quality_floor?: number | null;
  latency_class: "INTERACTIVE" | "NORMAL" | "BACKGROUND";
  trust_requirement: TrustClass;
  state_transfer_preference?: ("NATIVE_KV" | "TRANSLATED_KV" | "PREFIX" | "SEMANTIC") | null;
  budget: ResourceBudget;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ModelCapabilityProfileV1".
 */
export interface ModelCapabilityProfileV1 {
  schema_id: "allternit.kernel.ModelCapabilityProfileV1";
  schema_version: string;
  model_id: Id;
  locality?: ("LOCAL" | "PRIVATE_REMOTE" | "PUBLIC_REMOTE") | null;
  capabilities: {
    capability: CapabilityId;
    score: number;
    measured_range?: {} | null;
  }[];
  context_limit: number;
  output_schemas?: string[];
  trust_classes: TrustClass[];
  hardware_backends?: string[];
  residency: "PINNED" | "HOT" | "WARM" | "COLD" | "REMOTE";
  memory_footprint_mb?: number | null;
  benchmark_profile?: Id | null;
  state_transfer?: Id[];
  hidden_state_access: boolean;
  early_exit_support: boolean;
  logits_access: boolean;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ModelPoolEntryV1".
 */
export interface ModelPoolEntryV1 {
  schema_id: "allternit.kernel.ModelPoolEntryV1";
  schema_version: string;
  backend_id: Id;
  cognitive_roles: ("S0" | "S1" | "S2" | "S3")[];
  modes: (
    | "M0.DETERMINISTIC"
    | "M1.LOGIT_READOUT"
    | "M2.CALIBRATED_READOUT"
    | "M3.HIDDEN_HEAD"
    | "M4.DEDICATED_DECIDER"
    | "M5.GENERATIVE"
    | "M6.DEEP_SOLVER"
  )[];
  capabilities: CapabilityId[];
  trust_tags?: string[];
  confidence_estimate: number;
  latency_ms: number;
  cost: number;
  residency: "PINNED" | "HOT" | "WARM" | "COLD" | "REMOTE";
  backbone_id?: Id | null;
  model_revision?: string | null;
  runtime?: string | null;
  quantization?: string | null;
  layer_stop?: number | null;
  readout_head_id?: Id | null;
  calibration_manifest_id?: Id | null;
  memory_mb?: number | null;
  load_latency_ms?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ExecutionPlanV1".
 */
export interface ExecutionPlanV1 {
  schema_id: "allternit.kernel.ExecutionPlanV1";
  schema_version: string;
  plan_id: Id;
  node_id?: Id | null;
  cognitive_role: "S0" | "S1" | "S2" | "S3";
  capability_id: CapabilityId;
  execution_mode:
    | "M0.DETERMINISTIC"
    | "M1.LOGIT_READOUT"
    | "M2.CALIBRATED_READOUT"
    | "M3.HIDDEN_HEAD"
    | "M4.DEDICATED_DECIDER"
    | "M5.GENERATIVE"
    | "M6.DEEP_SOLVER";
  backend_id: Id;
  backbone_id?: Id | null;
  model_revision?: string | null;
  runtime?: string | null;
  quantization?: string | null;
  layer_start?: number | null;
  layer_stop?: number | null;
  expected_forward_fraction?: number | null;
  readout_head_id?: Id | null;
  calibration_manifest_id?: Id | null;
  calibration_level?: ("RAW" | "L0" | "L1" | "L2") | null;
  context_projection_id?: Id | null;
  cache_strategy?: ("REUSE" | "EXTEND" | "REBUILD" | "SEMANTIC_RECONSTRUCT") | null;
  state_transfer_strategy?: TransferMode | null;
  residency_requirement?: ("PINNED" | "HOT" | "WARM" | "COLD" | "REMOTE") | null;
  confidence_floor: number;
  latency_budget_ms?: number | null;
  cost_budget?: number | null;
  memory_budget_mb?: number | null;
  fallback_chain: Id[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ErrorV1".
 */
export interface ErrorV1 {
  family:
    | "INPUT"
    | "STATE"
    | "CONTEXT"
    | "ROUTING"
    | "DECISION"
    | "TOOL"
    | "POLICY"
    | "MUTATION"
    | "VERIFY"
    | "TRANSFER"
    | "BUDGET"
    | "SYSTEM";
  code: string;
  message: string;
  retryable: boolean;
  receipt_id?: Id | null;
  details?: {};
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CompletionCriterionV1".
 */
export interface CompletionCriterionV1 {
  schema_id: "allternit.kernel.CompletionCriterionV1";
  schema_version: string;
  criterion_id: string;
  criterion_version: number;
  description?: string | null;
  verifier_kind:
    | "parse"
    | "format"
    | "lint"
    | "typecheck"
    | "unit_test"
    | "integration_test"
    | "build"
    | "semantic_rule"
    | "requirement"
    | "security"
    | "regression"
    | "diff_review"
    | "human";
  verifier_semantics: string;
  /**
   * @minItems 1
   */
  evidence_kinds: [string, ...string[]];
  built_in: boolean;
  deprecated?: boolean;
  blocking_default?: boolean;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CompletionPolicyV1".
 */
export interface CompletionPolicyV1 {
  schema_id: "allternit.kernel.CompletionPolicyV1";
  schema_version: string;
  policy_id: Id;
  policy_version: number;
  task_type: string;
  /**
   * @minItems 1
   */
  require: [
    {
      criterion_id: string;
      criterion_version: number;
      blocking: boolean;
      params?: {};
    },
    ...{
      criterion_id: string;
      criterion_version: number;
      blocking: boolean;
      params?: {};
    }[]
  ];
  allow_partial: boolean;
  l2882_predicate: [
    "acceptance_criteria_satisfied",
    "required_verifications_pass",
    "no_unresolved_blocking_failures",
    "required_artifacts_exist",
    "policy_obligations_satisfied",
    "completion_evidence_durable"
  ];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CompletionProposalV1".
 */
export interface CompletionProposalV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.CompletionProposalV1";
  };
  proposal_id: Id;
  proposer_node_id: Id;
  proposer_role?: ("S0" | "S1" | "S2" | "S3") | null;
  claimed_criteria: {
    criterion_id: string;
    claim: "SATISFIED" | "UNSATISFIED" | "UNKNOWN";
  }[];
  evidence_refs: EvidenceRef[];
  confidence?: number | null;
  rationale?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CompletionDecisionV1".
 */
export interface CompletionDecisionV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.CompletionDecisionV1";
  } & {
    producer?: {
      kind?: "RUNTIME" | "VERIFIER";
    };
  };
  decision_id: Id;
  proposal_id: Id | null;
  policy: CompletionPolicyRef;
  outcome: "COMPLETE" | "PARTIAL" | "CONTINUE" | "ESCALATE" | "FAIL";
  predicate_results: {
    acceptance_criteria_satisfied: boolean;
    required_verifications_pass: boolean;
    no_unresolved_blocking_failures: boolean;
    required_artifacts_exist: boolean;
    policy_obligations_satisfied: boolean;
    completion_evidence_durable: boolean;
  };
  criteria_results: {
    criterion_id: string;
    result: "PASS" | "FAIL" | "INCONCLUSIVE" | "ERROR";
    /**
     * @minItems 1
     */
    receipt_refs: [Id, ...Id[]];
  }[];
  decided_by: "SYSTEM";
  verifier_ids?: Id[];
  attention_id?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ContextChunkV1".
 */
export interface ContextChunkV1 {
  schema_id: "allternit.kernel.ContextChunkV1";
  schema_version: string;
  chunk_id: Id;
  kind:
    | "USER_TURN"
    | "FILE"
    | "SYMBOL"
    | "TOOL_INPUT"
    | "TOOL_OUTPUT"
    | "DIAGNOSTIC"
    | "PLAN"
    | "INSTRUCTION"
    | "DIFF"
    | "SUMMARY"
    | "RECEIPT"
    | "MEMORY"
    | "OTHER";
  content_ref: string;
  content_hash: ContentHash;
  source_ref: Provenance;
  trust_class: TrustClass;
  sensitivity: SensitivityClass;
  created_at: Timestamp;
  supersedes?: Id[];
  pinned_conditions?: Id[];
  freshness?: number | null;
  dependency_distance?: number | null;
  token_estimates?: {
    [k: string]: number;
  };
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ContextVisibilityDecisionV1".
 */
export interface ContextVisibilityDecisionV1 {
  schema_id: "allternit.kernel.ContextVisibilityDecisionV1";
  schema_version: string;
  chunk_id: Id;
  visibility: "HIDE" | "SHORT" | "LONG" | "FULL";
  relevance: number;
  reason_code: string;
  compression_method?: string | null;
  evidence_pointers?: Id[];
  estimated_tokens?: number | null;
  assembly_order?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ContextProjectionV1".
 */
export interface ContextProjectionV1 {
  schema_id: "allternit.kernel.ContextProjectionV1";
  schema_version: string;
  projection_id: Id;
  target_capability: CapabilityId;
  target_model_family?: string | null;
  selected: ContextVisibilityDecisionV1[];
  compiled_tokens_estimate: number;
  budget_tokens?: number | null;
  instruction_fragments?: Id[];
  retrieval_snapshot_id?: Id | null;
  context_fingerprint: ContentHash;
  cache_strategy?: ("REUSE" | "EXTEND" | "REBUILD" | "SEMANTIC_RECONSTRUCT") | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "Candidate".
 */
export interface Candidate {
  candidate_id: Id;
  label?: string | null;
  payload?: unknown;
  is_unknown?: boolean;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "DecisionRequestV1".
 */
export interface DecisionRequestV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.DecisionRequestV1";
  };
  operation: "BELIEF" | "CHOICE" | "SCORE" | "RANK" | "SUBSET" | "ESTIMATE" | "GATE" | "VERIFY" | "PAIR_SCORE";
  state_projection_ref: Id;
  instructions: string;
  question_id?: Id | null;
  decision_bank_id: Id;
  candidates?: Candidate[];
  scale?: string[];
  constraints?: {}[];
  calibration_domain?: string | null;
  threshold_profile_id?: Id | null;
  max_latency_ms?: number | null;
  latency_class?: ("REALTIME" | "INTERACTIVE" | "BACKGROUND" | "BATCH") | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "DecisionResultV1".
 */
export interface DecisionResultV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.DecisionResultV1";
  };
  operation: "BELIEF" | "CHOICE" | "SCORE" | "RANK" | "SUBSET" | "ESTIMATE" | "GATE" | "VERIFY" | "PAIR_SCORE";
  answer: unknown;
  probabilities?: {
    [k: string]: number;
  } | null;
  confidence: number;
  confidence_semantics: "CALIBRATED" | "RELATIVE_SET" | "UNCALIBRATED";
  calibration_level_served: "RAW" | "L0" | "L1" | "L2" | "NONE";
  calibration_id?: Id | null;
  calibration_fingerprint?: ContentHash | null;
  feasible?: boolean | null;
  constraint_violations?: {}[];
  evidence_refs: EvidenceRef[];
  backend_id?: Id | null;
  model_impl?: ComponentIdentity | null;
  threshold_action: "AUTO" | "REVIEW" | "ESCALATE" | "REJECT";
  latency_ms: number;
  abstained?: boolean;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "DecisionDeploymentUnitV1".
 */
export interface DecisionDeploymentUnitV1 {
  schema_id: "allternit.kernel.DecisionDeploymentUnitV1";
  schema_version: string;
  model_ref: Id;
  model_revision: string;
  tokenizer_id: string;
  quantization: string;
  runtime_backend: string;
  readout_point: string | null;
  schema_hash: ContentHash;
  candidate_set_hash: ContentHash;
  calibration_artifact_id: Id | null;
  threshold_profile: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "DecisionBackendProfileV1".
 */
export interface DecisionBackendProfileV1 {
  schema_id: "allternit.kernel.DecisionBackendProfileV1";
  schema_version: string;
  backend_id: Id;
  kind: "rules" | "schema_encoder" | "contrastive_ranker" | "decoder_readout" | "specialist" | "remote_api";
  /**
   * @minItems 1
   */
  decision_modes: [
    "BELIEF" | "CHOICE" | "SCORE" | "RANK" | "SUBSET" | "ESTIMATE" | "GATE" | "VERIFY" | "PAIR_SCORE",
    ...("BELIEF" | "CHOICE" | "SCORE" | "RANK" | "SUBSET" | "ESTIMATE" | "GATE" | "VERIFY" | "PAIR_SCORE")[]
  ];
  probability_semantics: "CALIBRATED" | "RELATIVE_SET" | "UNCALIBRATED";
  state_cacheability?: boolean;
  candidate_cacheability?: boolean;
  candidate_count_curve?: {}[];
  max_candidates?: number | null;
  max_state_tokens?: number | null;
  domain_heads?: Id[];
  fine_tune_support?: boolean;
  calibration_profile?: {
    ece?: number | null;
    brier?: number | null;
    bins?: number | null;
  } | null;
  trust_location: "LOCAL" | "PRIVATE_REMOTE" | "PUBLIC_REMOTE";
  deployment_unit_id?: Id | null;
  latency_class?: ("REALTIME" | "INTERACTIVE" | "BACKGROUND" | "BATCH") | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "DecisionReadoutProfileV1".
 */
export interface DecisionReadoutProfileV1 {
  schema_id: "allternit.kernel.DecisionReadoutProfileV1";
  schema_version: string;
  profile_id: Id;
  backend_id: Id;
  readout_kind:
    "RAW_LOGIT" | "DEBIASED_LOGIT" | "CALIBRATED_LOGIT" | "HIDDEN_STATE_HEAD" | "CONTRASTIVE" | "SCHEMA_ENCODER";
  source_layer?: number | null;
  early_exit_supported?: boolean;
  question_id: Id;
  schema_hash: ContentHash;
  candidate_set_hash?: ContentHash | null;
  calibration_level: "RAW" | "L0" | "L1" | "L2" | "AUTO";
  calibration_artifact_id?: Id | null;
  head_artifact_id?: Id | null;
  confidence_semantics?: "CALIBRATED" | "RELATIVE_SET" | "UNCALIBRATED";
  max_options?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "DecisionCalibrationManifestV1".
 */
export interface DecisionCalibrationManifestV1 {
  schema_id: "allternit.kernel.DecisionCalibrationManifestV1";
  schema_version: string;
  manifest_id: Id;
  primitive_id: PrimitiveId;
  scope: {
    backend_id: Id;
    model_ref: Id;
    model_revision: string;
    tokenizer_id: string;
    quantization: string;
    runtime_backend: string;
    question_id: Id;
    candidate_schema_hash: ContentHash;
    candidate_set_hash: ContentHash;
    threshold_profile: Id;
    readout_point?: string | null;
  };
  scope_fingerprint: ContentHash;
  metrics: {
    ece: number | null;
    brier: number | null;
    nll: number | null;
    accuracy: number | null;
    f1: number | null;
    coverage_at_risk: number | null;
    flip_sensitivity: number | null;
    order_sensitivity: number | null;
  };
  held_out: {
    n: number;
    ci_level: number;
    ci_method: string;
    ece_upper_bound: number;
    auto_act_n: number;
    auto_act_error_rate: number;
    auto_act_error_upper_bound: number;
    dataset_ref?: Id | null;
  };
  coverage_region?: {};
  gate: {
    ece_max: 0.05;
    auto_act_error_max: 0.05;
    reversible_only: true;
    passed: boolean;
    agreement_with_other_model_used?: false;
  };
  created_at?: Timestamp;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ArtifactRecordV1".
 */
export interface ArtifactRecordV1 {
  schema_id: "allternit.kernel.ArtifactRecordV1";
  schema_version: string;
  artifact_id: Id;
  kind: "decision_readout_profile" | "decision_head" | "calibration_manifest" | "adapter" | "routing_profile";
  content_hash: ContentHash;
  scope_fingerprint: ContentHash;
  metadata?: {};
  payload_ref?: string | null;
  created_at: Timestamp;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ExecutionEnvironmentV1".
 */
export interface ExecutionEnvironmentV1 {
  schema_id: "allternit.kernel.ExecutionEnvironmentV1";
  schema_version: string;
  environment_id: Id;
  allowed_env_keys: string[];
  secret_refs: Id[];
  cwd: string;
  tmp_scope: string;
  filesystem_scope: ResourceRef[];
  inherit_process_env: false;
  network_policy_id?: Id | null;
  compiled_by?: ComponentIdentity | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "NetworkAccessPolicyV1".
 */
export interface NetworkAccessPolicyV1 {
  schema_id: "allternit.kernel.NetworkAccessPolicyV1";
  schema_version: string;
  policy_id: Id;
  network_default: "DENY" | "ALLOWLIST";
  domains: string[];
  private_ip_policy: "DENY";
  redirect_policy: {
    max_redirects: number;
    scope: "SAME_DOMAIN" | "ALLOWLIST";
  };
  dns_rebind_policy: "PIN_RESOLUTION";
  defaults_version: string;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "RetryPolicy".
 */
export interface RetryPolicy {
  max_attempts: number;
  backoff_ms?: number | null;
  /**
   * @minItems 1
   */
  must_change: ["EVIDENCE" | "STRATEGY" | "CONTEXT" | "MODEL", ...("EVIDENCE" | "STRATEGY" | "CONTEXT" | "MODEL")[]];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "OnFailure".
 */
export interface OnFailure {
  strategy: "RETRY" | "FALLBACK" | "ROLLBACK" | "ESCALATE" | "FAIL";
  target?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "GraphNodeV1".
 */
export interface GraphNodeV1 {
  node_id: Id;
  primitive_id: PrimitiveId;
  node_kind: "COMPUTE" | "POLICY" | "VERIFY" | "WAIT" | "CONTROL";
  cognitive_role?: ("S0" | "S1" | "S2" | "S3") | null;
  allowed_modes?: (
    | "M0.DETERMINISTIC"
    | "M1.LOGIT_READOUT"
    | "M2.CALIBRATED_READOUT"
    | "M3.HIDDEN_HEAD"
    | "M4.DEDICATED_DECIDER"
    | "M5.GENERATIVE"
    | "M6.DEEP_SOLVER"
  )[];
  capability_request?: CapabilityRequestV1 | null;
  inputs: string[];
  outputs: string[];
  read_set: ResourceRef[];
  write_set: ResourceRef[];
  lock_scope: ResourceRef[];
  context_projection_id?: Id | null;
  wait_gate?: WaitGateV1 | null;
  output_refs?: Id[];
  preconditions?: string[];
  postconditions?: string[];
  verifier?:
    | (
        | "PARSE"
        | "FORMAT"
        | "LINT"
        | "TYPECHECK"
        | "UNIT_TEST"
        | "INTEGRATION_TEST"
        | "BUILD"
        | "SEMANTIC_RULE"
        | "REQUIREMENT"
        | "SECURITY"
        | "REGRESSION"
        | "DIFF_REVIEW"
        | "HUMAN"
      )
    | null;
  retry_policy?: RetryPolicy | null;
  timeout_ms?: number | null;
  budget?: ResourceBudget | null;
  parallel_group?: string | null;
  rollback?: PrimitiveId | null;
  on_failure: OnFailure;
  evidence_required?: string[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "GraphEdgeV1".
 */
export interface GraphEdgeV1 {
  from: Id;
  to: Id;
  when?: string | null;
  priority?: number;
  edge_kind?: "NORMAL" | "FAILURE" | "LOOP";
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ComputeGraphIRV1".
 */
export interface ComputeGraphIRV1 {
  schema_id: "allternit.kernel.ComputeGraphIRV1";
  schema_version: string;
  graph_id: Id;
  task_type: string;
  task_id?: Id | null;
  graph_version: number;
  /**
   * @minItems 1
   */
  nodes: [GraphNodeV1, ...GraphNodeV1[]];
  edges: GraphEdgeV1[];
  /**
   * @minItems 1
   */
  entry_nodes: [Id, ...Id[]];
  /**
   * @minItems 1
   */
  completion_nodes: [Id, ...Id[]];
  global_budget: ResourceBudget;
  failure_policy: {
    on_unhandled: "FAIL" | "ESCALATE" | "NEEDS_HUMAN";
    max_total_retries?: number | null;
  };
  invariants?: string[];
  completion_policy?: CompletionPolicyRef | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "UnifiedDiffV1".
 */
export interface UnifiedDiffV1 {
  diff_ref: Id;
  diff_hash: ContentHash;
  base_hashes: {
    [k: string]: ContentHash;
  };
  files?: string[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "MutationRequestV1".
 */
export interface MutationRequestV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.MutationRequestV1";
  };
  target: ResourceRef;
  mutation_type:
    | "CREATE"
    | "DELETE"
    | "MOVE"
    | "RENAME"
    | "REPLACE_RANGE"
    | "REPLACE_SYMBOL"
    | "INSERT_NODE"
    | "DELETE_NODE"
    | "APPLY_PATCH"
    | "CODEMOD";
  canonical_change: UnifiedDiffV1;
  source_representation: "UNIFIED_DIFF" | "SEARCH_REPLACE";
  source_ref?: Id | null;
  expected_base_hash?: ContentHash | null;
  /**
   * @minItems 1
   */
  write_set: [ResourceRef, ...ResourceRef[]];
  invariants?: string[];
  rollback_plan?: Id | null;
  policy_decision_id: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "MutationReceiptV1".
 */
export interface MutationReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.MutationReceiptV1";
  };
  mutation_id: Id;
  target: ResourceRef;
  status: "APPLIED" | "REJECTED" | "ROLLED_BACK" | "FAILED";
  before_hash?: ContentHash | null;
  after_hash?: ContentHash | null;
  canonical_change: UnifiedDiffV1;
  structural_parse: "PASS" | "FAIL" | "NOT_APPLICABLE";
  rollback_ref?: Id | null;
  changed_symbols?: string[];
  affected_resources?: ResourceRef[];
  state_version_before: StateVersion;
  state_version_after: StateVersion;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CapabilityGrant".
 */
export interface CapabilityGrant {
  grant_id: Id;
  capability: string;
  resources: ResourceRef[];
  limits?: {};
  expires_at: Timestamp;
  task_id?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "PolicyCheckV1".
 */
export interface PolicyCheckV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.PolicyCheckV1";
  };
  check_id: Id;
  principal: Id;
  task_id: Id;
  proposed_action: {
    action_class: "READ" | "WRITE" | "EXECUTE" | "NETWORK" | "FINANCIAL" | "PUBLISH" | "PERMISSION_CHANGE" | "SPAWN";
    primitive_id: PrimitiveId;
    tool_id?: Id | null;
  };
  resources: ResourceRef[];
  data_classes: SensitivityClass[];
  external_effects: (
    | "NONE"
    | "READ"
    | "WORKSPACE_WRITE"
    | "EXECUTE"
    | "NETWORK"
    | "EXTERNAL_WRITE"
    | "FINANCIAL"
    | "PUBLISH"
    | "PERMISSION_CHANGE"
  )[];
  spend?: ResourceBudget | null;
  irreversible_class: "REVERSIBLE" | "COMPENSATABLE" | "IRREVERSIBLE";
  human_approval_state: "NOT_REQUIRED" | "REQUIRED_PENDING" | "APPROVED" | "REJECTED" | "EXPIRED";
  cognitive_risk?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "PolicyDecisionV1".
 */
export interface PolicyDecisionV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.PolicyDecisionV1";
  };
  check_id: Id;
  decision: "ALLOW" | "ALLOW_WITH_LIMITS" | "ASK" | "DENY";
  grants: CapabilityGrant[];
  limits: {}[];
  reason_codes: string[];
  expires_at?: Timestamp | null;
  receipt_id: Id;
  hard_policy: boolean;
  precedence: "HARD_DENY" | "HARD_ALLOW" | "SOFT" | "COGNITIVE" | "FAIL_CLOSED";
  attention_id?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AuthorityProfileV1".
 */
export interface AuthorityProfileV1 {
  schema_id: "allternit.kernel.AuthorityProfileV1";
  schema_version: string;
  profile_id: "read-only" | "code-safe" | "code-write";
  profile_version: number;
  capabilities: string[];
  denied_capabilities: string[];
  filesystem_scope: ResourceRef[];
  network_policy_id: Id;
  approval_requirements: {
    action_class: string;
    requires: "ASK" | "DENY";
  }[];
  defaults_version?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "PrimitiveDescriptorV1".
 */
export interface PrimitiveDescriptorV1 {
  schema_id: "allternit.kernel.PrimitiveDescriptorV1";
  schema_version: string;
  primitive_id: PrimitiveId;
  aliases?: string[];
  plugin_id?: Id | null;
  /**
   * @minItems 1
   */
  class: ["D" | "S" | "G" | "V" | "P" | "M" | "R", ...("D" | "S" | "G" | "V" | "P" | "M" | "R")[]];
  authority: "PROPOSE" | "DECIDE" | "VERIFY" | "AUTHORIZE";
  default_role?: ("S0" | "S1" | "S2" | "S3") | null;
  input_schema: string;
  output_schema: string;
  read_set: ResourceRef[];
  write_set: ResourceRef[];
  side_effect_class:
    | "NONE"
    | "READ"
    | "WORKSPACE_WRITE"
    | "EXECUTE"
    | "NETWORK"
    | "EXTERNAL_WRITE"
    | "FINANCIAL"
    | "PUBLISH"
    | "PERMISSION_CHANGE";
  reversibility: "REVERSIBLE" | "COMPENSATABLE" | "IRREVERSIBLE";
  determinism: "DETERMINISTIC" | "PROBABILISTIC" | "GENERATIVE";
  idempotency: "IDEMPOTENT" | "DEDUPLICATED" | "NON_IDEMPOTENT";
  evidence_required: string[];
  failure_modes: string[];
  fallback?: PrimitiveId[];
  capability_requirements?: CapabilityId[];
  timeout_ms?: number | null;
  eval_ref?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "PrimitiveMaturityV1".
 */
export interface PrimitiveMaturityV1 {
  schema_id: "allternit.kernel.PrimitiveMaturityV1";
  schema_version: string;
  primitive_id: PrimitiveId;
  stage: "UNTRAINED" | "BOOTSTRAP_L0" | "CALIBRATED_L1" | "SPECIALIZED_L2" | "MONITORED_PRODUCTION" | "INVALIDATED";
  observations: number;
  labels: number;
  active_profile_id: Id | null;
  last_fit_at?: Timestamp | null;
  drift_status?: "UNKNOWN" | "STABLE" | "WATCH" | "INVALIDATE";
  fallback_profile_id?: Id | null;
  authority_stage: "SHADOW" | "AUTO_ACT_REVERSIBLE" | "AUTO_ACT_EXPANDED";
  calibration_manifest_id?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "PrimitiveRegistryV1".
 */
export interface PrimitiveRegistryV1 {
  schema_id: "allternit.kernel.PrimitiveRegistryV1";
  schema_version: string;
  registry_version: SemVer;
  source?: string;
  count: number;
  class_legend?: {};
  /**
   * @minItems 1
   */
  primitives: [
    {
      id: PrimitiveId;
      alias: string;
      extra_aliases?: string[];
      family: string;
      /**
       * @minItems 1
       */
      ledger_rows: [string, ...string[]];
      class?: ("D" | "S" | "G" | "V" | "P" | "M" | "R")[] | null;
      note?: string | null;
    },
    ...{
      id: PrimitiveId;
      alias: string;
      extra_aliases?: string[];
      family: string;
      /**
       * @minItems 1
       */
      ledger_rows: [string, ...string[]];
      class?: ("D" | "S" | "G" | "V" | "P" | "M" | "R")[] | null;
      note?: string | null;
    }[]
  ];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ReceiptChainV1".
 */
export interface ReceiptChainV1 {
  receipt_id: Id;
  run_id: Id;
  index: number;
  prev_hash: ContentHash | null;
  content_hash: ContentHash;
  schema_id: string;
  schema_version: SemVer;
  domain: "allternit.receipt.v1";
  canonicalization: "RFC8785-JCS";
  signature?: ReceiptSignature | null;
  supersedes?: Id | null;
  idempotency_key?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ActionReceiptV1".
 */
export interface ActionReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.ActionReceiptV1";
  };
  chain: ReceiptChainV1;
  action_id: Id;
  effect_class:
    | "NONE"
    | "READ"
    | "WORKSPACE_WRITE"
    | "EXECUTE"
    | "NETWORK"
    | "EXTERNAL_WRITE"
    | "FINANCIAL"
    | "PUBLISH"
    | "PERMISSION_CHANGE";
  idempotency_key: string;
  target?: string | null;
  status: "INTENDED" | "COMMITTED" | "FAILED" | "COMPENSATED" | "UNKNOWN";
  policy_decision_id: Id;
  tool_receipt_id?: Id | null;
  external_ref?: string | null;
  result_hash?: ContentHash | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "PolicyReceiptV1".
 */
export interface PolicyReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.PolicyReceiptV1";
  };
  chain: ReceiptChainV1;
  check_id: Id;
  decision_id: Id;
  decision: "ALLOW" | "ALLOW_WITH_LIMITS" | "ASK" | "DENY";
  precedence: "HARD_DENY" | "HARD_ALLOW" | "SOFT" | "COGNITIVE" | "FAIL_CLOSED";
  reason_codes: string[];
  evaluated_rules: {
    rule_id: Id;
    rule_version?: string | null;
    outcome: "MATCH" | "NO_MATCH" | "ERROR";
  }[];
  judge_ref?: Id | null;
  grant_ids?: Id[];
  attention_id?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "SpawnReceiptV1".
 */
export interface SpawnReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.SpawnReceiptV1";
  };
  chain: ReceiptChainV1;
  spawn_id: Id;
  parent_node_id?: Id | null;
  executor_class: string;
  admission_decision_id: Id;
  lease_id: Id;
  environment_id: Id;
  network_policy_id?: Id | null;
  authority_profile: "read-only" | "code-safe" | "code-write";
  policy_gate_injected?: true;
  process_ref?: string | null;
  subgoal_id?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "RunReceiptV1".
 */
export interface RunReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.RunReceiptV1";
  };
  chain: ReceiptChainV1;
  task_id: Id;
  graph_id?: Id | null;
  final_state_version: StateVersion;
  final_work_state_version: StateVersion;
  completion_status: "COMPLETE" | "PARTIAL" | "FAILED" | "CANCELLED" | "WAITING_APPROVAL";
  completion_decision_id: Id | null;
  acceptance_results: {
    criterion_id: Id;
    satisfied: boolean;
    evidence_refs?: EvidenceRef[];
  }[];
  mutation_receipts?: Id[];
  verification_receipts?: Id[];
  tool_receipts?: Id[];
  policy_receipts?: Id[];
  action_receipts?: Id[];
  models_used?: ComponentIdentity[];
  total_cost_units?: number | null;
  total_latency_ms?: number | null;
  unresolved_items?: string[];
  user_result_ref?: Id | null;
  replay_fingerprint: ContentHash;
  retries: number;
  implementation_resolution: {
    node_id: Id;
    backend_id: Id;
    execution_mode:
      | "M0.DETERMINISTIC"
      | "M1.LOGIT_READOUT"
      | "M2.CALIBRATED_READOUT"
      | "M3.HIDDEN_HEAD"
      | "M4.DEDICATED_DECIDER"
      | "M5.GENERATIVE"
      | "M6.DEEP_SOLVER";
    calibration_level?: ("RAW" | "L0" | "L1" | "L2") | null;
    layer_stop?: number | null;
  }[];
  router_telemetry: {
    context_build_tokens?: number | null;
    prefill_tokens?: number | null;
    transfer_latency_ms?: number | null;
    effective_route_cost?: number | null;
    reuse_mode?: TransferMode | null;
    provider_trust_class?: TrustClass | null;
  };
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "TraceEventV1".
 */
export interface TraceEventV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.TraceEventV1";
  };
  event_type: string;
  primitive_id?: PrimitiveId | null;
  capability_id?: CapabilityId | null;
  backend_id?: Id | null;
  model_ref?: Id | null;
  tool_id?: Id | null;
  cognitive_role?: ("S0" | "S1" | "S2" | "S3") | null;
  execution_mode?:
    | (
        | "M0.DETERMINISTIC"
        | "M1.LOGIT_READOUT"
        | "M2.CALIBRATED_READOUT"
        | "M3.HIDDEN_HEAD"
        | "M4.DEDICATED_DECIDER"
        | "M5.GENERATIVE"
        | "M6.DEEP_SOLVER"
      )
    | null;
  calibration_level?: ("RAW" | "L0" | "L1" | "L2") | null;
  layer_stop?: number | null;
  latency_ms?: number | null;
  input_tokens?: number | null;
  output_tokens?: number | null;
  memory_bytes?: number | null;
  cache_hit?: boolean | null;
  kv_transfer_mode?: TransferMode | null;
  confidence?: number | null;
  cost_units?: number | null;
  status: "SUCCEEDED" | "FAILED" | "SKIPPED" | "STARTED";
  state_version_before?: StateVersion | null;
  state_version_after?: StateVersion | null;
  receipt_refs?: Id[];
  error?: ErrorV1 | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CassetteV1".
 */
export interface CassetteV1 {
  schema_id: "allternit.kernel.CassetteV1";
  schema_version: string;
  cassette_id: Id;
  run_id: Id;
  graph_id: Id;
  graph_version: number;
  abi_version: AbiVersion;
  /**
   * @minItems 1
   */
  entries: [
    {
      seq: number;
      node_id: Id;
      boundary: "DECISION" | "TOOL" | "CAPABILITY" | "POLICY" | "VERIFICATION" | "MUTATION" | "WAKE" | "ATTENTION";
      primitive_id?: PrimitiveId | null;
      request_hash: ContentHash;
      recorded_result_hash: ContentHash;
      result_ref: Id;
      effectful?: boolean;
      branch_taken?: string | null;
      receipt_ids?: Id[];
    },
    ...{
      seq: number;
      node_id: Id;
      boundary: "DECISION" | "TOOL" | "CAPABILITY" | "POLICY" | "VERIFICATION" | "MUTATION" | "WAKE" | "ATTENTION";
      primitive_id?: PrimitiveId | null;
      request_hash: ContentHash;
      recorded_result_hash: ContentHash;
      result_ref: Id;
      effectful?: boolean;
      branch_taken?: string | null;
      receipt_ids?: Id[];
    }[]
  ];
  run_receipt_hash: ContentHash;
  created_at?: Timestamp | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "DivergenceReportV1".
 */
export interface DivergenceReportV1 {
  schema_id: "allternit.kernel.DivergenceReportV1";
  schema_version: string;
  report_id: Id;
  cassette_id: Id;
  replay_run_id: Id;
  verdict: "IDENTICAL" | "EXPECTED_DIVERGENCE" | "UNEXPECTED_DIVERGENCE";
  divergences: {
    seq: number;
    node_id: Id;
    kind:
      | "BRANCH"
      | "RESULT_HASH"
      | "NODE_ORDER"
      | "MISSING_ENTRY"
      | "EXTRA_ENTRY"
      | "POLICY_OUTCOME"
      | "BACKEND_RESOLUTION";
    expected: boolean;
    recorded?: string | null;
    replayed?: string | null;
    explanation?: string | null;
  }[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "CognitiveStateDescriptorV1".
 */
export interface CognitiveStateDescriptorV1 {
  schema_id: "allternit.kernel.CognitiveStateDescriptorV1";
  schema_version: string;
  state_id: Id;
  model_ref: Id;
  model_revision: string;
  context_fingerprint: ContentHash;
  state_type: "NATIVE_KV" | "TRANSLATED_KV" | "PREFIX_CACHE" | "LATENT" | "NONE";
  token_span?: number | null;
  position_encoding?: string | null;
  storage_ref?: string | null;
  quality_profile?: {} | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "StateTransferRequestV1".
 */
export interface StateTransferRequestV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.StateTransferRequestV1";
  };
  source_state_id: Id;
  target_model_ref: Id;
  required_context_fingerprint: ContentHash;
  max_quality_loss?: number | null;
  max_latency_ms?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "StateTransferReceiptV1".
 */
export interface StateTransferReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.StateTransferReceiptV1";
  };
  mode: TransferMode;
  target_state_id?: Id | null;
  latency_ms: number;
  fidelity_estimate?: number | null;
  validation_ref?: Id | null;
  fallback_used: boolean;
  fallback_reason?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "TaskIRV1".
 */
export interface TaskIRV1 {
  schema_id: "allternit.kernel.TaskIRV1";
  schema_version: string;
  task_id: Id;
  task_type: string;
  objective: string;
  requested_outputs?: {
    kind: string;
    description?: string | null;
    required?: boolean;
  }[];
  acceptance_criteria: {
    criterion_id: Id;
    text: string;
    origin: "EXPLICIT" | "INFERRED";
    required?: boolean;
  }[];
  constraints: {
    constraint_id: Id;
    text: string;
    hardness: "HARD" | "SOFT";
  }[];
  ambiguities?: {
    ambiguity_id: Id;
    text: string;
    resolution: "UNRESOLVED" | "DEFAULTED" | "CLARIFIED" | "ESCALATED";
  }[];
  allowed_scope: ResourceRef[];
  forbidden_scope: ResourceRef[];
  risk_class: "LOW" | "MEDIUM" | "HIGH" | "CRITICAL" | "UNKNOWN";
  interaction_mode: "AUTONOMOUS" | "APPROVAL_GATED" | "RECOMMEND_ONLY";
  freshness_requirement?: string | null;
  latency_budget_ms?: number | null;
  cost_budget?: ResourceBudget | null;
  privacy_class: TrustClass;
  completion_policy: CompletionPolicyRef;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ToolInvocationV1".
 */
export interface ToolInvocationV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.ToolInvocationV1";
  };
  invocation_id?: Id | null;
  tool_id: Id;
  operation_id: string;
  arguments: {};
  argument_provenance: {
    [k: string]: "STATE" | "RETRIEVAL" | "POLICY" | "GENERATED" | "USER";
  };
  read_set: ResourceRef[];
  write_set: ResourceRef[];
  effect_class:
    | "NONE"
    | "READ"
    | "WORKSPACE_WRITE"
    | "EXECUTE"
    | "NETWORK"
    | "EXTERNAL_WRITE"
    | "FINANCIAL"
    | "PUBLISH"
    | "PERMISSION_CHANGE";
  policy_decision_id: Id;
  idempotency_key?: string | null;
  timeout_ms: number;
  environment_id: Id;
  network_policy_id?: Id | null;
  sandbox_ref?: Id | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ToolReceiptV1".
 */
export interface ToolReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.ToolReceiptV1";
  };
  invocation_id: Id;
  tool_id: Id;
  operation_id: string;
  started_at: Timestamp;
  finished_at: Timestamp;
  exit_class: "SUCCESS" | "FAILURE" | "PARTIAL" | "TIMEOUT" | "DENIED" | "CANCELLED";
  exit_code?: number | null;
  stdout_ref?: Id | null;
  stderr_ref?: Id | null;
  outputs?: ArtifactRef[];
  observed_reads?: ResourceRef[];
  observed_writes?: ResourceRef[];
  diagnostics?: {}[];
  policy_receipt_id: Id;
  content_hashes: ContentHash[];
  error?: ErrorV1 | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "RecoveryDecisionV1".
 */
export interface RecoveryDecisionV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.RecoveryDecisionV1";
  };
  failure_class_ref: Id;
  action: "RETRY" | "WAIT" | "CHANGE_ARGUMENT" | "CHANGE_TOOL" | "ESCALATE" | "FAIL";
  attempt: number;
  changed: ("EVIDENCE" | "STRATEGY" | "CONTEXT" | "MODEL")[];
  hard_limit_reached?: boolean;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "VerificationRequestV1".
 */
export interface VerificationRequestV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.VerificationRequestV1";
  };
  subject: {
    kind: "ARTIFACT" | "MUTATION_RECEIPT" | "CLAIM" | "COMPLETION_PROPOSAL";
    ref: Id;
  };
  verifier:
    | "PARSE"
    | "FORMAT"
    | "LINT"
    | "TYPECHECK"
    | "UNIT_TEST"
    | "INTEGRATION_TEST"
    | "BUILD"
    | "SEMANTIC_RULE"
    | "REQUIREMENT"
    | "SECURITY"
    | "REGRESSION"
    | "DIFF_REVIEW"
    | "HUMAN";
  criteria: Id[];
  evidence_inputs?: EvidenceRef[];
  required_confidence?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "VerificationReceiptV1".
 */
export interface VerificationReceiptV1 {
  envelope: AbiEnvelopeV1 & {
    schema_id?: "allternit.kernel.VerificationReceiptV1";
  };
  verification_id: Id;
  subject_ref: Id;
  verifier:
    | "PARSE"
    | "FORMAT"
    | "LINT"
    | "TYPECHECK"
    | "UNIT_TEST"
    | "INTEGRATION_TEST"
    | "BUILD"
    | "SEMANTIC_RULE"
    | "REQUIREMENT"
    | "SECURITY"
    | "REGRESSION"
    | "DIFF_REVIEW"
    | "HUMAN";
  result: "PASS" | "FAIL" | "INCONCLUSIVE" | "ERROR";
  findings: {}[];
  evidence_refs: EvidenceRef[];
  confidence?: number | null;
  deterministic: boolean;
  retryable: boolean;
  generated_followups?: Id[];
  criteria_results?: {
    criterion_id: Id;
    result: "PASS" | "FAIL" | "INCONCLUSIVE" | "ERROR";
  }[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "WorkNodeLifecycleV1".
 */
export interface WorkNodeLifecycleV1 {
  schema_id: "allternit.kernel.WorkNodeLifecycleV1";
  schema_version: string;
  run_id: Id;
  node_id: Id;
  attempt: number;
  state: LifecycleState;
  close_outcome?: ("COMMITTED" | "PARTIAL" | "FAILED" | "CANCELLED") | null;
  lease_id?: Id | null;
  wait_gate_id?: Id | null;
  attention_id?: Id | null;
  work_state_version: StateVersion;
  agent_state_ref: StateVersionRef;
  updated_at?: Timestamp;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "WorkNodeTransitionV1".
 */
export interface WorkNodeTransitionV1 {
  schema_id: "allternit.kernel.WorkNodeTransitionV1";
  schema_version: string;
  run_id: Id;
  node_id: Id;
  from: LifecycleState;
  to: LifecycleState;
  guard:
    | "ADMISSION_OK"
    | "DEPS_MET"
    | "LEASE_ACQUIRED"
    | "SPAWN_OK"
    | "HEARTBEAT"
    | "OUTPUT_RECORDED"
    | "VERIFY_PASS"
    | "VERIFY_FAIL"
    | "COMPLETION_DECIDED"
    | "REPLAN_ACCEPTED"
    | "ATTENTION_OPENED"
    | "ATTENTION_RESOLVED"
    | "WAIT_GATE_ARMED"
    | "WAKE"
    | "LEASE_EXPIRED"
    | "BUDGET_STOP"
    | "CANCEL_REQUESTED"
    | "EFFECTS_SETTLED"
    | "UNRECOVERABLE";
  evidence_refs?: EvidenceRef[];
  work_state_version_before: StateVersion;
  work_state_version_after: StateVersion;
  actor: "SYSTEM" | "EXECUTOR" | "HUMAN";
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "WorkRunRecordV1".
 */
export interface WorkRunRecordV1 {
  schema_id: "allternit.kernel.WorkRunRecordV1";
  schema_version: string;
  run_id: Id;
  status: RunStatus;
  work_state_version: StateVersion;
  agent_state_ref: StateVersionRef;
  campaign_id?: Id | null;
  attempts?: number;
  budget_usage?: ResourceBudget | null;
  billable_stopped?: boolean;
  resolved: {
    agent: string;
    logical_model_id: LogicalModelId;
    authority_profile: "read-only" | "code-safe" | "code-write";
    workspace?: string | null;
    budget: ResourceBudget;
    completion_policy: CompletionPolicyRef;
    defaults_version: string;
  };
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "NodeOutputV1".
 */
export interface NodeOutputV1 {
  schema_id: "allternit.kernel.NodeOutputV1";
  schema_version: string;
  dag_id: Id;
  node_id: Id;
  artifact_id: Id;
  mime_type: string;
  hash: ContentHash;
  producer: ComponentIdentity;
  state_version: StateVersion;
  trust_class: TrustClass;
  sensitivity: SensitivityClass;
  verification_status: "UNVERIFIED" | "PASS" | "FAIL" | "INCONCLUSIVE" | "ERROR";
  receipt_ref: Id;
  created_at: Timestamp;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "LeaseV1".
 */
export interface LeaseV1 {
  schema_id: "allternit.kernel.LeaseV1";
  schema_version: string;
  lease_id: Id;
  holder: Id;
  host: string;
  process_or_session: string;
  /**
   * @minItems 1
   */
  scope: [ResourceRef, ...ResourceRef[]];
  acquired_at: Timestamp;
  heartbeat_at: Timestamp;
  heartbeat_interval_ms: number;
  expires_after_ms: number;
  reclaim_policy: "REQUEUE" | "RESUME_FROM_CHECKPOINT" | "FAIL_NODE" | "NEEDS_HUMAN";
  state_version: StateVersion;
  defaults_version?: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AdmissionRequestV1".
 */
export interface AdmissionRequestV1 {
  schema_id: "allternit.kernel.AdmissionRequestV1";
  schema_version: string;
  request_id: Id;
  run_id: Id;
  node_id: Id;
  executor_class: string;
  requested_scope: ResourceRef[];
  budget?: ResourceBudget | null;
  priority?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "AdmissionDecisionV1".
 */
export interface AdmissionDecisionV1 {
  schema_id: "allternit.kernel.AdmissionDecisionV1";
  schema_version: string;
  request_id: Id;
  decision: "ADMIT" | "QUEUE" | "REJECT";
  spawn_cap_key: string;
  cap_limit?: number | null;
  cap_in_use?: number | null;
  probe_id?: Id | null;
  reason_codes: string[];
  retry_after_ms?: number | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitKernelAbi`'s JSON-Schema
 * via the `definition` "ExecutorProbeV1".
 */
export interface ExecutorProbeV1 {
  schema_id: "allternit.kernel.ExecutorProbeV1";
  schema_version: string;
  probe_id: Id;
  executor_class: string;
  available: boolean;
  auth_state: "AUTHENTICATED" | "UNAUTHENTICATED" | "EXPIRED" | "UNKNOWN";
  capacity?: number | null;
  in_use?: number | null;
  probed_at: Timestamp;
  error?: ErrorV1 | null;
  extensions?: ExtensionMap;
}
