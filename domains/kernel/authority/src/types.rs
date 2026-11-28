//! os/v1 wire types (CONTRACT.md sections 2 to 5, 11). Mirrors
//! `spec/Contracts/os/v1/generated/rust/src/generated.rs` field for field; the build train checks
//! they agree. Frozen ABI types (`PolicyCheckV1`, `PolicyDecisionV1`, `LeaseV1`, `ReceiptChainV1`)
//! belong to `allternit-kernel-abi` and stay opaque JSON objects here, so this core needs no std.
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

pub type Id = String;
pub type Timestamp = String;
/// `sha256:<64 lowercase hex>` (CONTRACT.md 11.3).
pub type ContentHash = String;
/// Decimal string, never a float.
pub type DecimalString = String;

/// Open JSON object. Without the `serde` feature this is the raw JSON text.
#[cfg(feature = "serde")]
pub type JsonObject = serde_json::Map<String, serde_json::Value>;
#[cfg(not(feature = "serde"))]
pub type JsonObject = String;

/// ABI `PolicyCheckV1`; `extensions["x-os-v1"]` holds a [`CheckExtensionV1`].
pub type PolicyCheck = JsonObject;
/// ABI `PolicyDecisionV1` (ALLOW | ALLOW_WITH_LIMITS | ASK | DENY).
pub type PolicyDecision = JsonObject;
/// ABI `LeaseV1`.
pub type Lease = JsonObject;
/// ABI `ReceiptChainV1`.
pub type ReceiptChain = JsonObject;

/// Key under which os/v1 fields ride in ABI `extensions` (CONTRACT.md 11.1).
pub const OS_EXTENSION_KEY: &str = "x-os-v1";

/// Keeps "absent" and "null" apart for optional + nullable fields.
#[cfg(feature = "serde")]
mod nullable {
    use serde::{Deserialize, Deserializer};
    pub fn de<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

// ---------------------------------------------------------------- enums

/// CONTRACT.md 4.1. A class outside this table would be `ask`, never allow; the enum is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
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

/// The five human-only classes (Eoj, 11-27).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum HumanOnlyActionClass {
    Money,
    ClientMessage,
    Deploy,
    SecretNewHost,
    Irreversible,
}

impl From<HumanOnlyActionClass> for ActionClass {
    fn from(c: HumanOnlyActionClass) -> Self {
        match c {
            HumanOnlyActionClass::Money => ActionClass::Money,
            HumanOnlyActionClass::ClientMessage => ActionClass::ClientMessage,
            HumanOnlyActionClass::Deploy => ActionClass::Deploy,
            HumanOnlyActionClass::SecretNewHost => ActionClass::SecretNewHost,
            HumanOnlyActionClass::Irreversible => ActionClass::Irreversible,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum AutonomyLevel {
    AskFirst,
    AskRisky,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
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

impl ErrorCode {
    /// False for the three codes that carry no receipt (CONTRACT.md 2.0).
    pub const fn has_receipt(self) -> bool {
        !matches!(
            self,
            ErrorCode::BadRequest | ErrorCode::FrameTooLarge | ErrorCode::UnsupportedVersion
        )
    }
}

/// Transport-set caller kind on the envelope (CONTRACT.md 2.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum CallerKind {
    Run,
    Human,
    Service,
}

/// What a principal is. Only `Human` may resolve approvals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum PrincipalKind {
    Human,
    Agent,
    Bot,
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ApprovalStatus {
    Open,
    Approved,
    Denied,
    Expired,
    Withdrawn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum SecretPurpose {
    HttpHeader,
    EnvForProcess,
    ConnectorCall,
    Sign,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum BudgetScope {
    Run,
    Day,
    Month,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum BudgetStatus {
    Reserved,
    Committed,
    Released,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum DeployStatus {
    PendingApproval,
    Admitted,
    Denied,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ReclaimPolicy {
    Requeue,
    ResumeFromCheckpoint,
    FailNode,
    NeedsHuman,
}

/// Kernel receipt kind, carried in the receipt payload (CONTRACT.md 11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ReceiptKind {
    Check,
    Approval,
    SecretUse,
    Budget,
    Deploy,
    Lease,
}

// ---------------------------------------------------------------- principal, caller, receipt

/// An actor: what it is and its id.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Principal {
    pub kind: PrincipalKind,
    pub id: Id,
}

/// Envelope `caller`, set by the transport.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Caller {
    pub kind: CallerKind,
    pub principal: Id,
}

/// One row of `kernel_receipts` (CONTRACT.md 5). `kind` mirrors `payload.kind`.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct KernelReceipt {
    pub receipt_id: Id,
    pub run_id: Id,
    pub idx: u64,
    pub domain: String,
    pub kind: ReceiptKind,
    pub schema_id: String,
    pub schema_version: String,
    /// Canonical JSON (RFC 8785). Never holds a secret value.
    pub payload: JsonObject,
    /// `sha256:` + 64 zeros for index 0.
    pub prev_hash: ContentHash,
    pub hash: ContentHash,
    /// Base64url Ed25519 over `hash`.
    pub signature: String,
    pub key_id: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub idempotency_key: Option<String>,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------- check + lease

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct CheckExtensionV1 {
    pub action_class: ActionClass,
    pub action_hash: ContentHash,
    pub approval_token: Option<String>,
    pub autonomy_level: AutonomyLevel,
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LeaseResources {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub requested_vcpu: Option<u64>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub requested_memory_mib: Option<u64>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub requested_gpu_vram_mib: Option<u64>,
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LeaseExtensionV1 {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub node_id: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub resources: Option<LeaseResources>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub labels: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
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

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LeaseHeartbeatBody {
    pub lease_id: Id,
    /// ABI `StateVersion` (non-negative integer).
    pub state_version: u64,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LeaseReleaseBody {
    pub lease_id: Id,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct LeaseReleaseResult {
    pub released: bool,
    pub receipt_id: Id,
}

/// `lease.acquire`, `lease.heartbeat` or `lease.release`.
#[derive(Clone, Debug)]
pub enum LeaseRequest {
    Acquire(LeaseAcquireBody),
    Heartbeat(LeaseHeartbeatBody),
    Release(LeaseReleaseBody),
}

/// Acquire and heartbeat return the ABI `LeaseV1`; release returns [`LeaseReleaseResult`].
#[derive(Clone, Debug)]
pub enum LeaseResponse {
    Lease(Lease),
    Released(LeaseReleaseResult),
}

// ---------------------------------------------------------------- secret.use

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct SecretUseV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub use_id: Id,
    /// A handle. A value-shaped string is `bad_request`.
    pub secret_ref: String,
    pub purpose: SecretPurpose,
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            deserialize_with = "nullable::de",
            skip_serializing_if = "Option::is_none"
        )
    )]
    pub target_host: Option<Option<String>>,
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            deserialize_with = "nullable::de",
            skip_serializing_if = "Option::is_none"
        )
    )]
    pub target_process: Option<Option<String>>,
    pub action_hash: ContentHash,
    pub check_id: Id,
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            deserialize_with = "nullable::de",
            skip_serializing_if = "Option::is_none"
        )
    )]
    pub approval_token: Option<Option<String>>,
    /// Server-set.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub new_host: Option<bool>,
    /// Server-set.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub redaction_id: Option<Id>,
    /// Server-set.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub receipt_id: Option<Id>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub extensions: Option<JsonObject>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct SecretUseResult {
    pub use_id: Id,
    pub bound: bool,
    pub redaction_id: Id,
    pub receipt_id: Id,
}

// ---------------------------------------------------------------- approvals

/// `ApprovalRequestV1` with server fields set, and the `approval.request` body with them absent.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ApprovalRequestV1 {
    pub schema_id: String,
    pub schema_version: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub request_id: Option<Id>,
    pub source_store: SourceStore,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub source_ref: Option<String>,
    pub run_id: Id,
    pub principal: Id,
    /// An Id or the literal "owner".
    pub assignee: String,
    pub action_class: ActionClass,
    pub action_summary: String,
    pub action_payload: JsonObject,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub action_hash: Option<ContentHash>,
    pub deadline: Timestamp,
    /// Always "reject": never auto-approve.
    pub on_expiry: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub attention_id: Option<Id>,
    pub check_id: Id,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub status: Option<ApprovalStatus>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub created_at: Option<Timestamp>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub resolved_at: Option<Timestamp>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub resolved_by: Option<Id>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub evidence_refs: Option<Vec<String>>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub extensions: Option<JsonObject>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ApprovalRedeemBody {
    pub token: String,
    pub action_hash: ContentHash,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ApprovalRedeemResult {
    pub redeemed: bool,
    pub request_id: Id,
    pub spend_receipt_id: Id,
}

/// Ed25519, kernel-signed, bound to the action payload, single use, short TTL.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct ApprovalTokenV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub token_id: Id,
    pub request_id: Id,
    pub action_class: ActionClass,
    pub action_hash: ContentHash,
    pub principal: Id,
    pub approved_by: Id,
    pub issued_at: Timestamp,
    pub expires_at: Timestamp,
    /// Always true.
    pub single_use: bool,
    pub key_id: String,
    pub signature: String,
    pub token: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub redemption_receipt_id: Option<Id>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub extensions: Option<JsonObject>,
}

// ---------------------------------------------------------------- audit, budget, deploy

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct AuditAppendBody {
    pub run_id: Id,
    /// ABI `ReceiptChainV1.domain` (`allternit.receipt.v1`).
    pub domain: String,
    pub schema_id: String,
    pub schema_version: String,
    /// Open object; `kind` ([`ReceiptKind`]) is required (CONTRACT.md 11.2).
    pub payload: JsonObject,
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct BudgetUsage {
    pub event_type: String,
    pub quantity: f64,
    pub unit: String,
    pub measured_at: Timestamp,
}

/// `BudgetReservationV1` and the `budget.reserve` body (server fields absent).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct BudgetReservationV1 {
    pub schema_id: String,
    pub schema_version: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub reservation_id: Option<Id>,
    pub run_id: Id,
    pub principal: Id,
    pub scope: BudgetScope,
    pub amount_reserved: DecimalString,
    pub currency: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub status: Option<BudgetStatus>,
    pub expires_at: Timestamp,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub amount_committed: Option<DecimalString>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub placement_id: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub resource_id: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub usage: Option<BudgetUsage>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub receipt_id: Option<Id>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub extensions: Option<JsonObject>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct BudgetCommitBody {
    pub reservation_id: Id,
    pub actual_amount: DecimalString,
    pub quantity: f64,
    pub unit: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct BudgetReleaseBody {
    pub reservation_id: Id,
}

/// `DeployAdmissionV1` and the `deploy.admit` body (server fields absent).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct DeployAdmissionV1 {
    pub schema_id: String,
    pub schema_version: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub admission_id: Option<Id>,
    pub principal: Id,
    pub target: String,
    pub artifact_digest: ContentHash,
    pub command_preview: String,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub action_hash: Option<ContentHash>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub status: Option<DeployStatus>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub approval_token_id: Option<Id>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub receipt_id: Option<Id>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub extensions: Option<JsonObject>,
}
