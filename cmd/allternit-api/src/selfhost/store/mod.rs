//! Storage for the self-hosted Agents Server.
//!
//! One async trait, [`SelfHostStore`], with two implementations:
//!
//! - [`pg::PgStore`]: PostgreSQL through sqlx, schema in
//!   `migrations_selfhost_pg/` applied with `sqlx::migrate!`. Work claims use
//!   `FOR UPDATE SKIP LOCKED`.
//! - [`sqlite::SqliteStore`]: rusqlite with its own embedded schema, for dev
//!   and no-database mode, plus an in-memory mode (`:memory:`) used when no
//!   database is set and `ephemeral: true`. Claims use a conditional UPDATE
//!   inside an IMMEDIATE transaction.
//!
//! Fencing: every heartbeat, renew, complete and release names the
//! `lease_generation` it got from its claim. A mismatch is
//! [`StoreError::LeaseStale`]. A claim raises the generation and issues a new
//! session secret, so an old worker's secret and generation both stop
//! working at once.
//!
//! Both implementations share one test suite: `tests/selfhost_store.rs`.

pub mod pg;
pub mod sqlite;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Largest tool result the store accepts, in bytes (100 KiB).
pub const MAX_TOOL_RESULT_BYTES: usize = 102_400;

/// Lease length handed out by a claim and a renew.
pub const LEASE_TTL_SECS: i64 = 60;

pub type DynStore = Arc<dyn SelfHostStore>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    /// The lease generation is old: another worker owns the item now.
    #[error("lease generation is stale")]
    LeaseStale,
    #[error("tool result is {size} bytes, over the {max} byte limit")]
    ResultTooLarge { size: usize, max: usize },
    /// A conflict with a stable error code, e.g. `agent_in_use`.
    #[error("conflict: {0}")]
    Conflict(&'static str),
    /// A list cursor that names no row.
    #[error("invalid cursor")]
    InvalidCursor,
    #[error("invalid value: {0}")]
    Invalid(String),
    #[error("storage error: {0}")]
    Backend(String),
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Hex SHA-256 of a secret. API keys, environment keys and session secrets
/// are stored only as this hash.
pub fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// `prefix_` + 24 lowercase hex characters, e.g. `agent_0123456789abcdef01234567`.
pub fn new_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..24])
}

/// A random secret with a recognizable prefix: `ask_`, `aek_` or `ss_`.
pub fn new_secret(prefix: &str) -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}

// ─── Enums ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    Developer,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Developer => "developer",
            Role::Admin => "admin",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "developer" => Some(Role::Developer),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Ready,
    Leased,
    Done,
    Failed,
    Cancelled,
}

impl WorkStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkStatus::Ready => "ready",
            WorkStatus::Leased => "leased",
            WorkStatus::Done => "done",
            WorkStatus::Failed => "failed",
            WorkStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> StoreResult<WorkStatus> {
        match s {
            "ready" => Ok(WorkStatus::Ready),
            "leased" => Ok(WorkStatus::Leased),
            "done" => Ok(WorkStatus::Done),
            "failed" => Ok(WorkStatus::Failed),
            "cancelled" => Ok(WorkStatus::Cancelled),
            other => Err(StoreError::Backend(format!("unknown work status {other}"))),
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(self, WorkStatus::Done | WorkStatus::Failed | WorkStatus::Cancelled)
    }
    /// The `Lease.status` value of the public API.
    pub fn lease_status(self) -> &'static str {
        match self {
            WorkStatus::Ready => "ready",
            WorkStatus::Leased => "claimed",
            _ => "completed",
        }
    }
}

/// How a worker finished a work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkOutcome {
    #[default]
    Success,
    Failed,
    Cancelled,
}

impl WorkOutcome {
    pub fn status(self) -> WorkStatus {
        match self {
            WorkOutcome::Success => WorkStatus::Done,
            WorkOutcome::Failed => WorkStatus::Failed,
            WorkOutcome::Cancelled => WorkStatus::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    Claimed,
    Done,
}

impl ToolCallStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolCallStatus::Pending => "pending",
            ToolCallStatus::Claimed => "claimed",
            ToolCallStatus::Done => "done",
        }
    }
    pub fn parse(s: &str) -> StoreResult<ToolCallStatus> {
        match s {
            "pending" => Ok(ToolCallStatus::Pending),
            "claimed" => Ok(ToolCallStatus::Claimed),
            "done" => Ok(ToolCallStatus::Done),
            other => Err(StoreError::Backend(format!("unknown tool call status {other}"))),
        }
    }
}

/// Session statuses of the public API.
pub const SESSION_STATUSES: &[&str] = &[
    "idle",
    "running",
    "waiting_for_worker",
    "interrupted",
    "error",
    "archived",
];

/// Event types of the public API.
pub const EVENT_TYPES: &[&str] = &["message", "tool_call", "tool_result", "status", "error"];

pub const NETWORK_MODES: &[&str] = &["none", "limited", "full"];

// ─── Records ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub instructions: String,
    pub model: String,
    /// JSON array of AgentTool.
    pub tools: Value,
    /// JSON array of McpServer.
    pub mcp_servers: Value,
    /// JSON array of skill names.
    pub skills: Value,
    pub metadata: Value,
    /// Raised by one on every update.
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct NewAgent {
    pub name: String,
    pub instructions: String,
    pub model: String,
    pub tools: Value,
    pub mcp_servers: Value,
    pub skills: Value,
    pub metadata: Value,
}

/// Absent fields stay unchanged.
#[derive(Debug, Clone, Default)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub tools: Option<Value>,
    pub mcp_servers: Option<Value>,
    pub skills: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentRecord {
    pub id: String,
    pub name: String,
    /// none | limited | full
    pub network: String,
    pub allowed_hosts: Vec<String>,
    /// GitPolicy JSON object.
    pub git: Value,
    /// McpPolicy JSON object.
    pub mcp: Value,
    pub image: Option<String>,
    /// `{apt, pip, npm}` JSON object.
    pub packages: Value,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct NewEnvironment {
    pub name: String,
    pub network: String,
    pub allowed_hosts: Vec<String>,
    pub git: Value,
    pub mcp: Value,
    pub image: Option<String>,
    pub packages: Value,
    pub metadata: Value,
}

#[derive(Debug, Clone, Default)]
pub struct EnvironmentPatch {
    pub name: Option<String>,
    pub network: Option<String>,
    pub allowed_hosts: Option<Vec<String>>,
    pub git: Option<Value>,
    pub mcp: Option<Value>,
    /// `Some(None)` clears the image.
    pub image: Option<Option<String>>,
    pub packages: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    pub agent_id: String,
    pub environment_id: String,
    pub title: Option<String>,
    pub status: String,
    pub status_reason: Option<String>,
    pub last_seq: i64,
    pub metadata: Value,
    /// The agent as it was when the session was created. Running sessions
    /// keep this version even after the agent is edited.
    pub agent_snapshot: Value,
    /// The gizzi child's own session id, set by the agent loop.
    pub gizzi_session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct NewSession {
    pub agent_id: String,
    pub environment_id: String,
    pub title: Option<String>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    pub agent_id: Option<String>,
    pub environment_id: Option<String>,
    pub status: Option<String>,
    pub include_archived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    pub id: String,
    pub session_id: String,
    /// 1, 2, 3, ... per session. Never reused.
    pub seq: i64,
    pub event_type: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkItem {
    pub id: String,
    pub environment_id: String,
    pub session_id: String,
    pub status: WorkStatus,
    pub lease_generation: i64,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub worker_id: Option<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// A successful claim. `session_secret` is shown once; the store keeps only
/// its hash.
#[derive(Debug, Clone)]
pub struct WorkClaim {
    pub item: WorkItem,
    pub session_secret: String,
}

#[derive(Debug, Clone)]
pub struct ClaimRequest {
    pub environment_id: String,
    pub worker_id: String,
    /// Also take leased items with `lease_expires_at <= now - ms`. `Some(0)`
    /// takes every expired lease. A negative value also takes live leases
    /// that end within `-ms` (the work routes use this for the public
    /// `reclaim_older_than_ms`, see `routes_work::reclaim_cutoff_ms`).
    pub reclaim_older_than_ms: Option<i64>,
    pub hostname: Option<String>,
    pub version: Option<String>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRecord {
    /// The tool_call_id.
    pub id: String,
    pub session_id: String,
    pub work_id: Option<String>,
    pub claimed_generation: Option<i64>,
    pub name: String,
    pub input: Value,
    pub status: ToolCallStatus,
    pub output: Option<String>,
    pub is_error: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewToolCall {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub input: Value,
}

/// Result of [`SelfHostStore::submit_tool_result`].
#[derive(Debug, Clone)]
pub struct ToolResultOutcome {
    pub call: ToolCallRecord,
    /// False when a result was already stored (the post was a repeat).
    pub stored: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerRecord {
    pub environment_id: String,
    pub worker_id: String,
    pub last_seen_at: DateTime<Utc>,
    pub current_work_id: Option<String>,
    pub hostname: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiKeyRecord {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub prefix: String,
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ApiKeyRecord {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at.map_or(true, |e| e > now)
    }
}

#[derive(Debug, Clone)]
pub struct NewApiKey {
    pub name: String,
    pub role: Role,
    pub prefix: String,
    pub key_hash: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentKeyRecord {
    pub id: String,
    pub environment_id: String,
    pub name: Option<String>,
    pub prefix: String,
    pub key_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl EnvironmentKeyRecord {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at.map_or(true, |e| e > now)
    }
}

#[derive(Debug, Clone)]
pub struct NewEnvironmentKey {
    pub environment_id: String,
    pub name: Option<String>,
    pub prefix: String,
    pub key_hash: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CredentialRecord {
    pub id: String,
    pub name: String,
    /// Ciphertext. The store never sees plaintext.
    pub sealed: Vec<u8>,
    /// Which encryption key sealed it (for rotation).
    pub key_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ─── Trait ──────────────────────────────────────────────────────────────────

/// List functions return up to `limit` rows after the row named by the
/// `after` cursor (an id; for events, a seq). Ask for `limit + 1` to learn
/// whether there is another page. An `after` that names no row is
/// [`StoreError::InvalidCursor`].
#[async_trait]
pub trait SelfHostStore: Send + Sync {
    /// "postgres", "sqlite" or "memory".
    fn backend(&self) -> &'static str;
    /// Cheap round trip, for /health/ready.
    async fn health(&self) -> StoreResult<()>;

    // Agents (soft-deleted; archived sessions keep pointing at them).
    async fn create_agent(&self, new: NewAgent) -> StoreResult<AgentRecord>;
    async fn get_agent(&self, id: &str) -> StoreResult<AgentRecord>;
    /// Oldest first.
    async fn list_agents(&self, limit: i64, after: Option<&str>) -> StoreResult<Vec<AgentRecord>>;
    async fn update_agent(&self, id: &str, patch: AgentPatch) -> StoreResult<AgentRecord>;
    /// `Conflict("agent_in_use")` while a non-archived session uses it.
    async fn delete_agent(&self, id: &str) -> StoreResult<()>;

    // Environments (soft-deleted).
    async fn create_environment(&self, new: NewEnvironment) -> StoreResult<EnvironmentRecord>;
    async fn get_environment(&self, id: &str) -> StoreResult<EnvironmentRecord>;
    /// Oldest first.
    async fn list_environments(
        &self,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<EnvironmentRecord>>;
    async fn update_environment(
        &self,
        id: &str,
        patch: EnvironmentPatch,
    ) -> StoreResult<EnvironmentRecord>;
    /// `Conflict("environment_in_use")` while a non-archived session uses it.
    /// Revokes the environment's keys.
    async fn delete_environment(&self, id: &str) -> StoreResult<()>;

    // Sessions.
    /// Snapshots the agent. NotFound if the agent or environment is missing.
    async fn create_session(&self, new: NewSession) -> StoreResult<SessionRecord>;
    async fn get_session(&self, id: &str) -> StoreResult<SessionRecord>;
    /// Newest first.
    async fn list_sessions(
        &self,
        filter: &SessionFilter,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<SessionRecord>>;
    /// Set the status unconditionally.
    async fn set_session_status(
        &self,
        id: &str,
        status: &str,
        reason: Option<&str>,
    ) -> StoreResult<SessionRecord>;
    /// Compare-and-set: change the status only if it is one of `from`.
    /// Returns None (and changes nothing) when the current status is not in
    /// `from`.
    async fn transition_session_status(
        &self,
        id: &str,
        from: &[&str],
        to: &str,
        reason: Option<&str>,
    ) -> StoreResult<Option<SessionRecord>>;
    async fn set_gizzi_session_id(&self, id: &str, gizzi_session_id: &str) -> StoreResult<()>;
    /// Status `archived`, `archived_at` now. A second call changes nothing.
    async fn archive_session(&self, id: &str) -> StoreResult<SessionRecord>;

    // Events.
    /// Next seq for the session, atomically. Raises `sessions.last_seq`.
    async fn append_event(
        &self,
        session_id: &str,
        event_type: &str,
        data: Value,
    ) -> StoreResult<EventRecord>;
    /// Oldest first, seq > `after_seq`.
    async fn list_events(
        &self,
        session_id: &str,
        after_seq: i64,
        limit: i64,
        types: Option<&[String]>,
    ) -> StoreResult<Vec<EventRecord>>;

    // Work queue.
    async fn enqueue_work(&self, environment_id: &str, session_id: &str) -> StoreResult<WorkItem>;
    async fn get_work(&self, id: &str) -> StoreResult<WorkItem>;
    /// The newest ready or leased item for the session, if any.
    async fn find_open_work_for_session(&self, session_id: &str) -> StoreResult<Option<WorkItem>>;
    /// Oldest claimable item: ready, or (with `reclaim_older_than_ms`) leased
    /// with `lease_expires_at <= now - reclaim_older_than_ms`. Raises the
    /// generation, starts a [`LEASE_TTL_SECS`] lease, issues a new session
    /// secret, hands the session's claimed-but-unfinished tool calls out
    /// again, and records the worker. None when there is nothing.
    async fn claim_work(&self, req: ClaimRequest) -> StoreResult<Option<WorkClaim>>;
    /// Records the worker as alive. Does not move the lease.
    async fn heartbeat_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem>;
    /// Moves `lease_expires_at` to now + [`LEASE_TTL_SECS`].
    async fn renew_work(&self, id: &str, generation: i64, now: DateTime<Utc>)
        -> StoreResult<WorkItem>;
    /// Ends the lease. Repeating it with the same generation is a no-op that
    /// returns the item.
    async fn complete_work(
        &self,
        id: &str,
        generation: i64,
        outcome: WorkOutcome,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem>;
    /// Back to ready at once. Claimed tool calls go back to pending.
    async fn release_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem>;
    /// Leased items whose lease ran out before `now - grace` go back to ready.
    /// Returns how many moved. For a periodic sweeper.
    async fn requeue_expired_work(&self, now: DateTime<Utc>, grace: Duration) -> StoreResult<u64>;
    /// Ready and leased items of the session become cancelled.
    async fn cancel_open_work_for_session(&self, session_id: &str) -> StoreResult<u64>;
    /// The leased, unexpired item whose session secret has this hash.
    async fn find_work_by_secret_hash(
        &self,
        secret_hash: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<WorkItem>>;
    /// Ready items for the environment.
    async fn queue_depth(&self, environment_id: &str) -> StoreResult<i64>;

    // Tool calls (id = tool_call_id).
    /// Idempotent. Returns the stored call and whether this call inserted it.
    /// `Conflict("tool_call_conflict")` if the id exists for another session.
    async fn insert_tool_call(&self, new: NewToolCall) -> StoreResult<(ToolCallRecord, bool)>;
    async fn get_tool_call(&self, id: &str) -> StoreResult<ToolCallRecord>;
    /// Oldest pending call of the work item's session, marked claimed under
    /// the item's current generation. LeaseStale if `generation` is not the
    /// item's current one or the item is not leased.
    async fn next_tool_call(
        &self,
        work_id: &str,
        generation: i64,
    ) -> StoreResult<Option<ToolCallRecord>>;
    /// Stores the result once. A repeat stores nothing and returns
    /// `stored: false`. `ResultTooLarge` over [`MAX_TOOL_RESULT_BYTES`].
    async fn submit_tool_result(
        &self,
        id: &str,
        output: &str,
        is_error: bool,
    ) -> StoreResult<ToolResultOutcome>;
    /// Unfinished calls of the session are finished as errors with this
    /// output. Returns how many.
    async fn cancel_open_tool_calls(&self, session_id: &str, output: &str) -> StoreResult<u64>;

    // Workers.
    async fn touch_worker(
        &self,
        environment_id: &str,
        worker_id: &str,
        current_work_id: Option<&str>,
        hostname: Option<&str>,
        version: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<()>;
    /// Seen at or after `since`, newest first.
    async fn list_workers(
        &self,
        environment_id: &str,
        since: DateTime<Utc>,
    ) -> StoreResult<Vec<WorkerRecord>>;
    async fn count_workers(&self, environment_id: &str, since: DateTime<Utc>) -> StoreResult<i64>;

    // Server API keys.
    /// Every row, revoked ones too. Zero means first boot.
    async fn count_api_keys(&self) -> StoreResult<i64>;
    async fn create_api_key(&self, new: NewApiKey) -> StoreResult<ApiKeyRecord>;
    /// Like `create_api_key`, but only while the table is empty, atomically.
    /// `Conflict("setup_already_done")` otherwise.
    async fn create_first_api_key(&self, new: NewApiKey) -> StoreResult<ApiKeyRecord>;
    async fn find_api_key_by_hash(&self, key_hash: &str) -> StoreResult<Option<ApiKeyRecord>>;
    /// Oldest first.
    async fn list_api_keys(&self, limit: i64, after: Option<&str>) -> StoreResult<Vec<ApiKeyRecord>>;
    /// `Conflict("last_admin_key")` when it is the last active admin key.
    /// Revoking a revoked key returns it unchanged.
    async fn revoke_api_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<ApiKeyRecord>;
    async fn touch_api_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<()>;

    // Environment keys.
    async fn create_environment_key(&self, new: NewEnvironmentKey)
        -> StoreResult<EnvironmentKeyRecord>;
    async fn find_environment_key_by_hash(
        &self,
        key_hash: &str,
    ) -> StoreResult<Option<EnvironmentKeyRecord>>;
    /// Oldest first.
    async fn list_environment_keys(
        &self,
        environment_id: &str,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<EnvironmentKeyRecord>>;
    async fn revoke_environment_key(
        &self,
        environment_id: &str,
        id: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<EnvironmentKeyRecord>;
    async fn touch_environment_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<()>;

    // Credentials (sealed bytes; see token_crypto / step 14).
    /// Insert or replace by name.
    async fn put_credential(
        &self,
        name: &str,
        sealed: &[u8],
        key_id: &str,
    ) -> StoreResult<CredentialRecord>;
    async fn get_credential(&self, name: &str) -> StoreResult<CredentialRecord>;
    async fn list_credentials(&self) -> StoreResult<Vec<CredentialRecord>>;
    async fn delete_credential(&self, name: &str) -> StoreResult<()>;
}

/// Lease end for a claim or renew at `now`.
pub fn lease_end(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::seconds(LEASE_TTL_SECS)
}

/// Reject event types and statuses outside the public enums before they
/// reach a CHECK constraint.
pub(crate) fn check_event_type(t: &str) -> StoreResult<()> {
    if EVENT_TYPES.contains(&t) {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!("unknown event type {t}")))
    }
}

pub(crate) fn check_session_status(s: &str) -> StoreResult<()> {
    if SESSION_STATUSES.contains(&s) {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!("unknown session status {s}")))
    }
}

pub(crate) fn check_network(n: &str) -> StoreResult<()> {
    if NETWORK_MODES.contains(&n) {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!("unknown network mode {n}")))
    }
}

pub(crate) fn check_result_size(output: &str) -> StoreResult<()> {
    if output.len() > MAX_TOOL_RESULT_BYTES {
        Err(StoreError::ResultTooLarge {
            size: output.len(),
            max: MAX_TOOL_RESULT_BYTES,
        })
    } else {
        Ok(())
    }
}

/// Agent fields frozen into a session at creation.
pub(crate) fn agent_snapshot(a: &AgentRecord) -> Value {
    serde_json::json!({
        "id": a.id,
        "name": a.name,
        "instructions": a.instructions,
        "model": a.model,
        "tools": a.tools,
        "mcp_servers": a.mcp_servers,
        "skills": a.skills,
        "metadata": a.metadata,
        "version": a.version,
    })
}

/// `[]`/`{}` defaults for JSON columns when a caller leaves a Value null.
pub(crate) fn or_array(v: Value) -> Value {
    if v.is_null() {
        Value::Array(vec![])
    } else {
        v
    }
}

pub(crate) fn or_object(v: Value) -> Value {
    if v.is_null() {
        Value::Object(Default::default())
    } else {
        v
    }
}
