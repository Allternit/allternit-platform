/* Generated from spec/Contracts/os/v1/schemas (os contracts 1.0.0) by scripts/regenerate.sh. DO NOT EDIT. */

/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ShellToolResultV1".
 */
export type ShellToolResultV1 =
  | {
      ok: true;
      command_id: Id;
      state_version: number;
    }
  | {
      ok: false;
      error: ErrorCode;
      receipt_id: Id;
    };
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "Id".
 */
export type Id = string;
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ErrorCode".
 */
export type ErrorCode =
  | "bad_request"
  | "unsupported_version"
  | "frame_too_large"
  | "unauthenticated"
  | "forbidden_caller"
  | "denied"
  | "approval_required"
  | "token_invalid"
  | "token_expired"
  | "token_replayed"
  | "hash_mismatch"
  | "budget_exceeded"
  | "not_found"
  | "conflict"
  | "chain_broken"
  | "kernel_unavailable";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ShellAction".
 */
export type ShellAction =
  "open" | "close" | "focus" | "arrange" | "start_run" | "stop_run" | "install_app" | "fork_app";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ResourceRef".
 */
export type ResourceRef = string;
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "TrustClass".
 */
export type TrustClass = "PUBLIC" | "INTERNAL" | "RESTRICTED" | "SECRET" | "UNTRUSTED";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "Sha256Hex".
 */
export type Sha256Hex = string;
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "Surface".
 */
export type Surface = "desktop" | "web" | "phone" | "pwa" | "ios" | "android" | "tui";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ActionClass".
 */
export type ActionClass =
  | "read"
  | "ui_layout"
  | "run_control"
  | "file_write"
  | "network"
  | "app_install"
  | "inbox_prepare"
  | "money"
  | "client_message"
  | "deploy"
  | "secret_new_host"
  | "irreversible";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "Timestamp".
 */
export type Timestamp = string;
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "InboxListResult".
 */
export type InboxListResult = InboxItemV1[];
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "InboxToolInputV1".
 */
export type InboxToolInputV1 =
  | {
      action: "list";
      scope: "needs_you" | "snoozed" | "all";
      limit?: number;
    }
  | {
      action: "summarise";
      ids: Id[];
    }
  | {
      action: "snooze";
      id: Id;
      until: Timestamp;
    }
  | {
      action: "prepare";
      id: Id;
      draft: {};
    };
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "InboxAction".
 */
export type InboxAction = "list" | "summarise" | "snooze" | "prepare";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "DecimalString".
 */
export type DecimalString = string;
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "SemVer".
 */
export type SemVer = string;
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "StateVersion".
 */
export type StateVersion = number;
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "AutonomyLevel".
 */
export type AutonomyLevel = "ask_first" | "ask_risky" | "full";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "KernelResponseV1".
 */
export type KernelResponseV1 =
  | {
      v: 1;
      id: string;
      ok: true;
      body: {};
    }
  | {
      v: 1;
      id: string;
      ok: false;
      error: {
        code: ErrorCode;
        message: string;
        receipt_id: Id | null;
      };
    };
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "KernelCall".
 */
export type KernelCall =
  | "check"
  | "lease.acquire"
  | "lease.heartbeat"
  | "lease.release"
  | "secret.use"
  | "approval.request"
  | "approval.resolve"
  | "approval.redeem"
  | "audit.append"
  | "budget.reserve"
  | "budget.commit"
  | "budget.release"
  | "deploy.admit";
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "HumanOnlyActionClass".
 */
export type HumanOnlyActionClass = "money" | "client_message" | "deploy" | "secret_new_host" | "irreversible";

/**
 * Codegen bundle of AllternitOS contracts os/v1 plus the ABI defs they reference. Not normative; see schemas/.
 */
export interface AllternitOsContracts {
  [k: string]: unknown;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ShellCommandV1".
 */
export interface ShellCommandV1 {
  schema_id: "allternit.os.ShellCommandV1";
  schema_version: string;
  command_id: Id;
  action: ShellAction;
  expected_state_version?: number;
  view_id?: string;
  params?: {};
  instance_id?: string;
  arrangement?: {
    layout: "single" | "split" | "grid" | "free";
    slots: {
      instance_id: string;
      region: string;
    }[];
  };
  task?:
    | TaskIRV1
    | {
        prompt: string;
        profile_id: string;
      };
  run_id?: Id;
  app_source?: {
    app_id: string;
    source: "catalog" | "url";
    ref: string;
    digest: Sha256Hex;
  };
  fork_of?: {
    app_id: string;
    app_version: string;
  };
  new_app_id?: string;
  check_id?: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
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
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
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
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ExtensionMap".
 */
export interface ExtensionMap {
  [k: string]: unknown;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "CompletionPolicyRef".
 */
export interface CompletionPolicyRef {
  policy_id: Id;
  policy_version: number;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ShellStateV1".
 */
export interface ShellStateV1 {
  schema_id: "allternit.os.ShellStateV1";
  schema_version: string;
  state_version: number;
  surface: Surface;
  views: {
    instance_id: string;
    view_id: string;
    focused: boolean;
    region: string;
  }[];
  focused_instance: string | null;
  arrangement: "single" | "split" | "grid" | "free";
  runs: {
    run_id: Id;
    status: string;
  }[];
  installed_apps: {
    app_id: string;
    app_version: string;
    view_id: string;
  }[];
  inbox_counts: {
    needs_you: number;
    snoozed: number;
  };
  protected_open: string | null;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ViewDescriptorV1".
 */
export interface ViewDescriptorV1 {
  schema_id: "allternit.os.ViewDescriptorV1";
  schema_version: string;
  view_id: string;
  kind: "builtin" | "app" | "protected";
  icon?: string;
  app_id?: string;
  app_version?: string;
  entry?: {
    type: "app-host";
    url: string;
  };
  surfaces: Surface[];
  singleton?: boolean;
  capabilities?: string[];
  params_schema?: {};
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "InboxSnoozeResult".
 */
export interface InboxSnoozeResult {
  ok: true;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "InboxItemV1".
 */
export interface InboxItemV1 {
  id: Id;
  kind: "approval" | "informational";
  action_class: ActionClass;
  action_summary: string;
  action_payload: {};
  assignee: Id | "owner";
  deadline: Timestamp;
  urgency: "low" | "normal" | "high" | "critical";
  run_id: Id;
  source_store:
    | "kernel"
    | "factory"
    | "aci"
    | "permission_policy"
    | "gateway"
    | "cowork"
    | "cloud_platform"
    | "task_grant"
    | "sign_request";
  status: "open" | "approved" | "denied" | "expired" | "withdrawn";
  attention_id: Id;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "DeployAdmitBody".
 */
export interface DeployAdmitBody {
  schema_id: "allternit.os.DeployAdmissionV1";
  schema_version: string;
  admission_id?: Id;
  principal: Id;
  target: string;
  artifact_digest: Sha256Hex;
  command_preview: string;
  action_hash?: Sha256Hex;
  status?: "pending_approval" | "admitted" | "denied" | "expired";
  approval_token_id?: Id;
  receipt_id?: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "DeployAdmissionV1".
 */
export interface DeployAdmissionV1 {
  schema_id: "allternit.os.DeployAdmissionV1";
  schema_version: string;
  admission_id: Id;
  principal: Id;
  target: string;
  artifact_digest: Sha256Hex;
  command_preview: string;
  action_hash: Sha256Hex;
  status: "pending_approval" | "admitted" | "denied" | "expired";
  approval_token_id?: Id;
  receipt_id: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "BudgetReleaseBody".
 */
export interface BudgetReleaseBody {
  reservation_id: Id;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "BudgetCommitBody".
 */
export interface BudgetCommitBody {
  reservation_id: Id;
  actual_amount: DecimalString;
  quantity: number;
  unit: string;
  idempotency_key: string;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "BudgetReserveBody".
 */
export interface BudgetReserveBody {
  schema_id: "allternit.os.BudgetReservationV1";
  schema_version: string;
  reservation_id?: Id;
  run_id: Id;
  principal: Id;
  scope: "run" | "day" | "month";
  amount_reserved: DecimalString;
  currency: string;
  status?: "reserved" | "committed" | "released" | "expired";
  expires_at: Timestamp;
  amount_committed?: DecimalString;
  placement_id?: string;
  resource_id?: string;
  usage?: {
    event_type: string;
    quantity: number;
    unit: string;
    measured_at: Timestamp;
  };
  receipt_id?: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "BudgetReservationV1".
 */
export interface BudgetReservationV1 {
  schema_id: "allternit.os.BudgetReservationV1";
  schema_version: string;
  reservation_id: Id;
  run_id: Id;
  principal: Id;
  scope: "run" | "day" | "month";
  amount_reserved: DecimalString;
  currency: string;
  status: "reserved" | "committed" | "released" | "expired";
  expires_at: Timestamp;
  amount_committed?: DecimalString;
  placement_id?: string;
  resource_id?: string;
  usage?: {
    event_type: string;
    quantity: number;
    unit: string;
    measured_at: Timestamp;
  };
  receipt_id: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "AuditAppendBody".
 */
export interface AuditAppendBody {
  run_id: Id;
  domain: "allternit.receipt.v1";
  schema_id: string;
  schema_version: SemVer;
  payload: {};
  idempotency_key: string | null;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ApprovalTokenV1".
 */
export interface ApprovalTokenV1 {
  schema_id: "allternit.os.ApprovalTokenV1";
  schema_version: string;
  token_id: Id;
  request_id: Id;
  action_class: ActionClass;
  action_hash: Sha256Hex;
  principal: Id;
  approved_by: Id;
  issued_at: Timestamp;
  expires_at: Timestamp;
  single_use: true;
  key_id: string;
  signature: string;
  token: string;
  redemption_receipt_id?: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ApprovalRedeemResult".
 */
export interface ApprovalRedeemResult {
  redeemed: true;
  request_id: Id;
  spend_receipt_id: Id;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ApprovalRedeemBody".
 */
export interface ApprovalRedeemBody {
  token: string;
  action_hash: Sha256Hex;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ApprovalResolveBody".
 */
export interface ApprovalResolveBody {
  request_id: Id;
  decision: "approve" | "deny";
  note: string | null;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ApprovalRequestBody".
 */
export interface ApprovalRequestBody {
  schema_id: "allternit.os.ApprovalRequestV1";
  schema_version: string;
  request_id?: Id;
  source_store:
    | "kernel"
    | "factory"
    | "aci"
    | "permission_policy"
    | "gateway"
    | "cowork"
    | "cloud_platform"
    | "task_grant"
    | "sign_request";
  source_ref?: string;
  run_id: Id;
  principal: Id;
  assignee: Id | "owner";
  action_class: ActionClass;
  action_summary: string;
  action_payload: {};
  action_hash?: Sha256Hex;
  deadline: Timestamp;
  on_expiry: "reject";
  attention_id?: Id;
  check_id: Id;
  status?: "open" | "approved" | "denied" | "expired" | "withdrawn";
  created_at?: Timestamp;
  resolved_at?: Timestamp;
  resolved_by?: Id;
  evidence_refs?: string[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "ApprovalRequestV1".
 */
export interface ApprovalRequestV1 {
  schema_id: "allternit.os.ApprovalRequestV1";
  schema_version: string;
  request_id: Id;
  source_store:
    | "kernel"
    | "factory"
    | "aci"
    | "permission_policy"
    | "gateway"
    | "cowork"
    | "cloud_platform"
    | "task_grant"
    | "sign_request";
  source_ref?: string;
  run_id: Id;
  principal: Id;
  assignee: Id | "owner";
  action_class: ActionClass;
  action_summary: string;
  action_payload: {};
  action_hash: Sha256Hex;
  deadline: Timestamp;
  on_expiry: "reject";
  attention_id: Id;
  check_id: Id;
  status: "open" | "approved" | "denied" | "expired" | "withdrawn";
  created_at: Timestamp;
  resolved_at?: Timestamp;
  resolved_by?: Id;
  evidence_refs?: string[];
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "SecretUseResult".
 */
export interface SecretUseResult {
  use_id: Id;
  bound: true;
  redaction_id: Id;
  receipt_id: Id;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "SecretUseV1".
 */
export interface SecretUseV1 {
  schema_id: "allternit.os.SecretUseV1";
  schema_version: string;
  use_id: Id;
  secret_ref: string;
  purpose: "http_header" | "env_for_process" | "connector_call" | "sign";
  target_host?: string | null;
  target_process?: string | null;
  action_hash: Sha256Hex;
  check_id: Id;
  approval_token?: string | null;
  new_host?: boolean;
  redaction_id?: Id;
  receipt_id?: Id;
  extensions?: ExtensionMap;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "LeaseReleaseResult".
 */
export interface LeaseReleaseResult {
  released: true;
  receipt_id: Id;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "LeaseReleaseBody".
 */
export interface LeaseReleaseBody {
  lease_id: Id;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "LeaseHeartbeatBody".
 */
export interface LeaseHeartbeatBody {
  lease_id: Id;
  state_version: StateVersion;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "LeaseAcquireBody".
 */
export interface LeaseAcquireBody {
  holder: Id;
  /**
   * @minItems 1
   */
  scope: [ResourceRef, ...ResourceRef[]];
  host: string;
  process_or_session: string;
  heartbeat_interval_ms: number;
  expires_after_ms: number;
  reclaim_policy: "REQUEUE" | "RESUME_FROM_CHECKPOINT" | "FAIL_NODE" | "NEEDS_HUMAN";
  check_id: Id;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "Labels".
 */
export interface Labels {
  [k: string]: string;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "LeaseExtensionV1".
 */
export interface LeaseExtensionV1 {
  node_id?: string;
  resources?: {
    requested_vcpu?: number;
    requested_memory_mib?: number;
    requested_gpu_vram_mib?: number;
  };
  labels?: Labels;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "CheckExtensionV1".
 */
export interface CheckExtensionV1 {
  action_class: ActionClass;
  action_hash: Sha256Hex;
  approval_token: string | null;
  autonomy_level: AutonomyLevel;
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "KernelRequestV1".
 */
export interface KernelRequestV1 {
  v: 1;
  id: string;
  call: KernelCall;
  caller: Caller;
  body: {};
}
/**
 * This interface was referenced by `AllternitOsContracts`'s JSON-Schema
 * via the `definition` "Caller".
 */
export interface Caller {
  kind: "run" | "human" | "service";
  principal: Id;
}
