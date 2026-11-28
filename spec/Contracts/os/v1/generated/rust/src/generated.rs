// HAND-WRITTEN to match spec/Contracts/os/v1/schemas (os contracts 1.0.0, DRAFT).
// cargo-typify could not run in the coding session (train guard blocks cargo).
// The build train regenerates this file with scripts/regenerate.sh. Until then, edit it
// only together with the schemas.
use allternit_kernel_abi::{ExtensionMap, StateVersion, TaskIrv1};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type Id = String;
pub type Timestamp = String;
/// 64 lowercase hex characters (no `sha256:` prefix).
pub type Sha256Hex = String;
pub type DecimalString = String;
pub type JsonObject = Map<String, Value>;

/// Keeps "absent" and "null" apart for optional + nullable fields.
mod nullable {
    use serde::{Deserialize, Deserializer};
    pub fn de<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

// ---------------------------------------------------------------- authority enums

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    Read,
    UiLayout,
    RunControl,
    FileWrite,
    Network,
    AppInstall,
    InboxPrepare,
    Money,
    ClientMessage,
    Deploy,
    SecretNewHost,
    Irreversible,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HumanOnlyActionClass {
    Money,
    ClientMessage,
    Deploy,
    SecretNewHost,
    Irreversible,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    AskFirst,
    AskRisky,
    Full,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KernelCall {
    #[serde(rename = "check")]
    Check,
    #[serde(rename = "lease.acquire")]
    LeaseAcquire,
    #[serde(rename = "lease.heartbeat")]
    LeaseHeartbeat,
    #[serde(rename = "lease.release")]
    LeaseRelease,
    #[serde(rename = "secret.use")]
    SecretUse,
    #[serde(rename = "approval.request")]
    ApprovalRequest,
    #[serde(rename = "approval.resolve")]
    ApprovalResolve,
    #[serde(rename = "approval.redeem")]
    ApprovalRedeem,
    #[serde(rename = "audit.append")]
    AuditAppend,
    #[serde(rename = "budget.reserve")]
    BudgetReserve,
    #[serde(rename = "budget.commit")]
    BudgetCommit,
    #[serde(rename = "budget.release")]
    BudgetRelease,
    #[serde(rename = "deploy.admit")]
    DeployAdmit,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    UnsupportedVersion,
    FrameTooLarge,
    Unauthenticated,
    ForbiddenCaller,
    Denied,
    ApprovalRequired,
    TokenInvalid,
    TokenExpired,
    TokenReplayed,
    HashMismatch,
    BudgetExceeded,
    NotFound,
    Conflict,
    ChainBroken,
    KernelUnavailable,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CallerKind {
    Run,
    Human,
    Service,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SourceStore {
    Kernel,
    Factory,
    Aci,
    PermissionPolicy,
    Gateway,
    Cowork,
    CloudPlatform,
    TaskGrant,
    SignRequest,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Open,
    Approved,
    Denied,
    Expired,
    Withdrawn,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SecretPurpose {
    HttpHeader,
    EnvForProcess,
    ConnectorCall,
    Sign,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    Run,
    Day,
    Month,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    Reserved,
    Committed,
    Released,
    Expired,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    PendingApproval,
    Admitted,
    Denied,
    Expired,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResolveDecision {
    Approve,
    Deny,
}

/// ABI `LeaseV1.reclaim_policy` (referenced, values copied for typing only).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReclaimPolicy {
    Requeue,
    ResumeFromCheckpoint,
    FailNode,
    NeedsHuman,
}

// ---------------------------------------------------------------- envelope

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct Caller {
    pub kind: CallerKind,
    pub principal: Id,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct KernelRequestV1 {
    pub v: u8,
    pub id: String,
    pub call: KernelCall,
    pub caller: Caller,
    pub body: JsonObject,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct KernelError {
    pub code: ErrorCode,
    pub message: String,
    pub receipt_id: Option<Id>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum KernelResponseV1 {
    Ok(KernelResponseOk),
    Err(KernelResponseErr),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct KernelResponseOk {
    pub v: u8,
    pub id: String,
    pub ok: bool,
    pub body: JsonObject,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct KernelResponseErr {
    pub v: u8,
    pub id: String,
    pub ok: bool,
    pub error: KernelError,
}

// ---------------------------------------------------------------- check + lease

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct CheckExtensionV1 {
    pub action_class: ActionClass,
    pub action_hash: Sha256Hex,
    pub approval_token: Option<String>,
    pub autonomy_level: AutonomyLevel,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct LeaseResources {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_vcpu: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_memory_mib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_gpu_vram_mib: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct LeaseExtensionV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<LeaseResources>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LeaseAcquireBody {
    pub holder: Id,
    pub scope: Vec<String>,
    pub host: String,
    pub process_or_session: String,
    pub heartbeat_interval_ms: u64,
    pub expires_after_ms: u64,
    pub reclaim_policy: ReclaimPolicy,
    pub check_id: Id,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LeaseHeartbeatBody {
    pub lease_id: Id,
    pub state_version: StateVersion,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LeaseReleaseBody {
    pub lease_id: Id,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LeaseReleaseResult {
    pub released: bool,
    pub receipt_id: Id,
}

// ---------------------------------------------------------------- secret.use

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SecretUseV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub use_id: Id,
    pub secret_ref: String,
    pub purpose: SecretPurpose,
    #[serde(default, deserialize_with = "nullable::de", skip_serializing_if = "Option::is_none")]
    pub target_host: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable::de", skip_serializing_if = "Option::is_none")]
    pub target_process: Option<Option<String>>,
    pub action_hash: Sha256Hex,
    pub check_id: Id,
    #[serde(default, deserialize_with = "nullable::de", skip_serializing_if = "Option::is_none")]
    pub approval_token: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_host: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SecretUseResult {
    pub use_id: Id,
    pub bound: bool,
    pub redaction_id: Id,
    pub receipt_id: Id,
}

// ---------------------------------------------------------------- approvals

/// Used for both `ApprovalRequestV1` (all server fields set) and `ApprovalRequestBody`
/// (server fields absent); the schema decides which fields are required.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequestV1 {
    pub schema_id: String,
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Id>,
    pub source_store: SourceStore,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub run_id: Id,
    pub principal: Id,
    /// An Id or the literal "owner".
    pub assignee: String,
    pub action_class: ActionClass,
    pub action_summary: String,
    pub action_payload: JsonObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_hash: Option<Sha256Hex>,
    pub deadline: Timestamp,
    /// Always "reject".
    pub on_expiry: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_id: Option<Id>,
    pub check_id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ApprovalStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_refs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}
pub type ApprovalRequestBody = ApprovalRequestV1;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResolveBody {
    pub request_id: Id,
    pub decision: ResolveDecision,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRedeemBody {
    pub token: String,
    pub action_hash: Sha256Hex,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRedeemResult {
    pub redeemed: bool,
    pub request_id: Id,
    pub spend_receipt_id: Id,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ApprovalTokenV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub token_id: Id,
    pub request_id: Id,
    pub action_class: ActionClass,
    pub action_hash: Sha256Hex,
    pub principal: Id,
    pub approved_by: Id,
    pub issued_at: Timestamp,
    pub expires_at: Timestamp,
    /// Always true.
    pub single_use: bool,
    pub key_id: String,
    pub signature: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redemption_receipt_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}

// ---------------------------------------------------------------- audit, budget, deploy

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AuditAppendBody {
    pub run_id: Id,
    /// ABI ReceiptChainV1.domain ("allternit.receipt.v1").
    pub domain: String,
    pub schema_id: String,
    pub schema_version: String,
    pub payload: JsonObject,
    pub idempotency_key: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct BudgetUsage {
    pub event_type: String,
    pub quantity: f64,
    pub unit: String,
    pub measured_at: Timestamp,
}

/// Used for both `BudgetReservationV1` and `BudgetReserveBody`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct BudgetReservationV1 {
    pub schema_id: String,
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<Id>,
    pub run_id: Id,
    pub principal: Id,
    pub scope: BudgetScope,
    pub amount_reserved: DecimalString,
    pub currency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<BudgetStatus>,
    pub expires_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_committed: Option<DecimalString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<BudgetUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}
pub type BudgetReserveBody = BudgetReservationV1;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct BudgetCommitBody {
    pub reservation_id: Id,
    pub actual_amount: DecimalString,
    pub quantity: f64,
    pub unit: String,
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct BudgetReleaseBody {
    pub reservation_id: Id,
}

/// Used for both `DeployAdmissionV1` and `DeployAdmitBody`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DeployAdmissionV1 {
    pub schema_id: String,
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_id: Option<Id>,
    pub principal: Id,
    pub target: String,
    pub artifact_digest: Sha256Hex,
    pub command_preview: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_hash: Option<Sha256Hex>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<DeployStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_token_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}
pub type DeployAdmitBody = DeployAdmissionV1;

// ---------------------------------------------------------------- inbox

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum InboxItemKind {
    Approval,
    Informational,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct InboxItemV1 {
    pub id: Id,
    pub kind: InboxItemKind,
    pub action_class: ActionClass,
    pub action_summary: String,
    pub action_payload: JsonObject,
    pub assignee: String,
    pub deadline: Timestamp,
    pub urgency: Urgency,
    pub run_id: Id,
    pub source_store: SourceStore,
    pub status: ApprovalStatus,
    pub attention_id: Id,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum InboxScope {
    NeedsYou,
    Snoozed,
    All,
}

/// The model's inbox tool. There is NO approve, deny or resolve action.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum InboxToolInputV1 {
    List {
        scope: InboxScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u64>,
    },
    Summarise {
        ids: Vec<Id>,
    },
    Snooze {
        id: Id,
        until: Timestamp,
    },
    Prepare {
        id: Id,
        draft: JsonObject,
    },
}

pub type InboxListResult = Vec<InboxItemV1>;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct InboxSnoozeResult {
    pub ok: bool,
}

// ---------------------------------------------------------------- shell

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Desktop,
    Web,
    Phone,
    Pwa,
    Ios,
    Android,
    Tui,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Builtin,
    App,
    Protected,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Arrangement {
    Single,
    Split,
    Grid,
    Free,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ShellAction {
    Open,
    Close,
    Focus,
    Arrange,
    StartRun,
    StopRun,
    InstallApp,
    ForkApp,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ViewEntry {
    /// Always "app-host".
    #[serde(rename = "type")]
    pub type_: String,
    pub url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ViewDescriptorV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub view_id: String,
    pub kind: ViewKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<ViewEntry>,
    pub surfaces: Vec<Surface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub singleton: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_schema: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellViewInstance {
    pub instance_id: String,
    pub view_id: String,
    pub title: String,
    pub focused: bool,
    pub region: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellRun {
    pub run_id: Id,
    pub status: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct InstalledApp {
    pub app_id: String,
    pub app_version: String,
    pub view_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct InboxCounts {
    pub needs_you: u64,
    pub snoozed: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellStateV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub state_version: u64,
    pub surface: Surface,
    pub views: Vec<ShellViewInstance>,
    pub focused_instance: Option<String>,
    pub arrangement: Arrangement,
    pub runs: Vec<ShellRun>,
    pub installed_apps: Vec<InstalledApp>,
    pub inbox_counts: InboxCounts,
    pub protected_open: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ArrangementSlot {
    pub instance_id: String,
    pub region: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellArrangement {
    pub layout: Arrangement,
    pub slots: Vec<ArrangementSlot>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellTaskPrompt {
    pub prompt: String,
    pub profile_id: String,
}

/// `start_run` task: an ABI TaskIRV1 or a prompt + profile.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ShellTask {
    TaskIr(Box<TaskIrv1>),
    Prompt(ShellTaskPrompt),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AppSourceKind {
    Catalog,
    Url,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct AppSource {
    pub app_id: String,
    pub source: AppSourceKind,
    #[serde(rename = "ref")]
    pub ref_: String,
    pub digest: Sha256Hex,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ForkOf {
    pub app_id: String,
    pub app_version: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellCommandV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub command_id: Id,
    pub action: ShellAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_state_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<ShellArrangement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<ShellTask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_source: Option<AppSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_of: Option<ForkOf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionMap>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ShellToolResultV1 {
    Ok(ShellToolOk),
    Err(ShellToolErr),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellToolOk {
    pub ok: bool,
    pub command_id: Id,
    pub state_version: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct ShellToolErr {
    pub ok: bool,
    pub error: ErrorCode,
    pub receipt_id: Id,
}
