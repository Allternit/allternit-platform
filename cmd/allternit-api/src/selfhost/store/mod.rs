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
//! Several replicas (Postgres only) share one database. The store is the
//! truth for everything that crosses replicas:
//!
//! - One open work item per session: a partial unique index
//!   (`work_items_one_open`) plus [`SelfHostStore::ensure_open_work`].
//! - Soft worker affinity on claims (see [`SelfHostStore::claim_work`]).
//! - Turn ownership: `sessions.turn_replica` / `gizzi_replica`, replica
//!   liveness in `server_replicas`, and [`SelfHostStore::fail_orphaned_turns`].
//! - Idempotency-Key records in `idempotency_keys`.
//! - [`SelfHostStore::sweep_lock`]: one replica runs a sweep tick at a time.
//! - Wake hints: the Postgres store calls
//!   `pg_notify('allternit_selfhost', '<kind>:<id>[:<seq>]')` inside its own
//!   transactions ([`Wake`]); [`SelfHostStore::subscribe_wakes`] returns them
//!   as a stream. They only make waits faster: a waiter must still re-read
//!   the store, and must re-read everything on [`Wake::Resync`].
//!
//! Both implementations share one test suite: `tests/selfhost_store.rs`.

pub mod pg;
pub mod sqlite;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Largest tool result the store accepts, in bytes (100 KiB).
pub const MAX_TOOL_RESULT_BYTES: usize = 102_400;

/// Lease length handed out by a claim and a renew.
pub const LEASE_TTL_SECS: i64 = 60;

/// Work item statuses that count as "open". A session has at most one open
/// item; the database enforces it (`work_items_one_open`).
pub const OPEN_WORK_STATUSES: &[&str] = &["ready", "leased"];

/// Soft affinity window: after a worker completes or releases a session's
/// item, that worker is preferred for the session's next claim for this long.
pub const AFFINITY_SECS: i64 = 10;

/// The preference only holds while the preferred worker was seen (any claim,
/// heartbeat, renew, complete or release) within this many seconds.
pub const AFFINITY_SEEN_SECS: i64 = 60;

/// A finished Idempotency-Key record is replayed for this long.
pub const IDEMPOTENCY_TTL_HOURS: i64 = 24;

/// An Idempotency-Key record still `in_progress` after this long is taken
/// over by the next request with the same key (the first one died).
pub const IDEMPOTENCY_TAKEOVER_SECS: i64 = 60;

/// The Postgres NOTIFY channel for [`Wake`] hints.
pub const WAKE_CHANNEL: &str = "allternit_selfhost";

/// Error event code (and status reason) of a turn lost with its replica.
pub const SERVER_RESTARTED: &str = "server_restarted";

/// Message of the [`SERVER_RESTARTED`] error event.
pub const SERVER_RESTARTED_MESSAGE: &str =
    "The server replica running this turn stopped. Send the message again.";

/// Output given to tool calls left open by a lost turn.
pub const SERVER_RESTARTED_TOOL_OUTPUT: &str =
    "The turn that made this call was lost because the server replica running it stopped.";

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
    /// The replica running the session's current turn
    /// ([`SelfHostStore::claim_turn`]). None between turns.
    pub turn_replica: Option<String>,
    /// The replica whose gizzi child holds `gizzi_session_id`. Another
    /// replica rebuilds the gizzi session from the events instead.
    pub gizzi_replica: Option<String>,
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
    /// Soft affinity: until `prefer_until`, a claim from this worker is
    /// preferred (see [`SelfHostStore::claim_work`]).
    pub preferred_worker_id: Option<String>,
    pub prefer_until: Option<DateTime<Utc>>,
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
    /// Internal cutoff offset. This is NOT the public `reclaim_older_than_ms`.
    ///
    /// Leased items with `lease_expires_at <= now - lease_cutoff_offset_ms`
    /// are also claimable. `None`: ready items only. `Some(0)`: also every
    /// expired lease. A negative value also takes LIVE leases that end within
    /// `-offset` ms, i.e. whose last renew was at least
    /// `LEASE_TTL_SECS * 1000 + offset` ms ago.
    ///
    /// The public `reclaim_older_than_ms` R means "also take a live lease
    /// last renewed at least R ms ago" (floor 30000, 60000 or more adds
    /// nothing). The work routes turn it into this offset as `R - 60000`
    /// (`routes_work::reclaim_cutoff_ms`).
    pub lease_cutoff_offset_ms: Option<i64>,
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
    /// Strictly increases on every write, so it works as the version for
    /// [`SelfHostStore::put_credential_if_unchanged`].
    pub updated_at: DateTime<Utc>,
}

/// Who and what an Idempotency-Key record is for. The key is unique per
/// (principal, method, path, key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyScope {
    /// The authenticated caller, e.g. the API key id.
    pub principal: String,
    pub method: String,
    pub path: String,
    /// The `Idempotency-Key` header value.
    pub key: String,
}

/// A stored 2xx response, replayed for a repeat of the same request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResponse {
    pub status: u16,
    /// Header (name, value) pairs to replay.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Result of [`SelfHostStore::idempotency_begin`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyBegin {
    /// This request owns the key now (new, expired, or taken over from a
    /// request in progress for more than [`IDEMPOTENCY_TAKEOVER_SECS`]). Run
    /// it, then call `idempotency_finish` (2xx) or `idempotency_abandon`.
    New,
    /// Same key, same body, finished: send this response again.
    Replay(StoredResponse),
    /// Same key, same body, still running (409 `idempotency_in_progress`).
    InProgress,
    /// Same key, different body (422 `idempotency_key_reuse`).
    Mismatch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplicaRecord {
    pub replica_id: String,
    pub started_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    pub version: Option<String>,
}

/// A wake hint. The Postgres store sends one as a NOTIFY on
/// [`WAKE_CHANNEL`] inside the transaction that made the change, so it is
/// delivered on commit. Payload: `<kind>:<id>[:<seq>]`.
///
/// Hints only make waits faster. Notifications can be lost (a dropped
/// listener connection, a full queue), so a waiter must re-read the store
/// on a hint and on a timer, and re-read everything on [`Wake::Resync`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Wake {
    /// `work:<environment_id>`: claimable work appeared in the environment
    /// (an item queued, released, or put back after its lease ran out).
    Work { environment_id: String },
    /// `session:<session_id>`: the session's work item or pending tool
    /// calls changed (item claimed, completed or cancelled; call queued).
    Session { session_id: String },
    /// `call:<tool_call_id>`: the tool call got its result or was closed.
    Call { tool_call_id: String },
    /// `event:<session_id>:<seq>`: an event was appended.
    Event { session_id: String, seq: i64 },
    /// `status:<session_id>`: the session's status changed.
    Status { session_id: String },
    /// Not sent by the store: the listener lost its connection (and
    /// reconnects by itself). Hints may have been missed; re-read the store.
    Resync,
}

impl Wake {
    /// The NOTIFY payload. Empty for [`Wake::Resync`].
    pub fn payload(&self) -> String {
        match self {
            Wake::Work { environment_id } => format!("work:{environment_id}"),
            Wake::Session { session_id } => format!("session:{session_id}"),
            Wake::Call { tool_call_id } => format!("call:{tool_call_id}"),
            Wake::Event { session_id, seq } => format!("event:{session_id}:{seq}"),
            Wake::Status { session_id } => format!("status:{session_id}"),
            Wake::Resync => String::new(),
        }
    }

    /// Parse a payload. None for anything unknown.
    pub fn parse(payload: &str) -> Option<Wake> {
        let (kind, rest) = payload.split_once(':')?;
        if rest.is_empty() {
            return None;
        }
        let id = rest.to_string();
        Some(match kind {
            "work" => Wake::Work { environment_id: id },
            "session" => Wake::Session { session_id: id },
            "call" => Wake::Call { tool_call_id: id },
            "status" => Wake::Status { session_id: id },
            "event" => {
                let (sid, seq) = rest.rsplit_once(':')?;
                if sid.is_empty() {
                    return None;
                }
                Wake::Event {
                    session_id: sid.to_string(),
                    seq: seq.parse().ok()?,
                }
            }
            _ => return None,
        })
    }
}

/// Endless stream of wake hints from [`SelfHostStore::subscribe_wakes`].
pub type WakeStream = BoxStream<'static, Wake>;

/// Held while this replica runs a sweep tick ([`SelfHostStore::sweep_lock`]).
/// Call [`SweepGuard::release`] when the tick is done; dropping it also
/// releases the lock, a little later.
pub struct SweepGuard {
    release: Option<Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>>,
}

impl SweepGuard {
    /// A guard that holds nothing (single-node stores).
    pub fn local() -> Self {
        Self { release: None }
    }

    /// A guard that runs `release` when released.
    pub fn new(release: impl FnOnce() -> BoxFuture<'static, ()> + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }

    pub async fn release(mut self) {
        if let Some(f) = self.release.take() {
            f().await;
        }
    }
}

impl std::fmt::Debug for SweepGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SweepGuard")
            .field("held", &self.release.is_some())
            .finish()
    }
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
    /// Insert a ready item. `Conflict("work_already_open")` when the session
    /// already has an open item; use [`SelfHostStore::ensure_open_work`] to
    /// find-or-insert.
    async fn enqueue_work(&self, environment_id: &str, session_id: &str) -> StoreResult<WorkItem>;
    /// The session's open item, inserted now (ready) if it has none. Returns
    /// the item and whether this call inserted it. Safe under any number of
    /// concurrent callers on any number of replicas: the unique index makes
    /// the insert a no-op for all but one. A new item inherits a live
    /// affinity preference from the session's previous item.
    async fn ensure_open_work(
        &self,
        environment_id: &str,
        session_id: &str,
    ) -> StoreResult<(WorkItem, bool)>;
    async fn get_work(&self, id: &str) -> StoreResult<WorkItem>;
    /// The session's ready or leased item, if any (at most one exists).
    async fn find_open_work_for_session(&self, session_id: &str) -> StoreResult<Option<WorkItem>>;
    /// Oldest claimable item: ready, or (with `lease_cutoff_offset_ms`)
    /// leased with `lease_expires_at <= now - lease_cutoff_offset_ms`. Raises
    /// the generation, starts a [`LEASE_TTL_SECS`] lease, issues a new
    /// session secret, hands the session's claimed-but-unfinished tool calls
    /// out again, clears the affinity preference, and records the worker.
    /// None when there is nothing.
    ///
    /// Soft affinity, the whole rule: an item with `prefer_until > now` and a
    /// `preferred_worker_id` other than the claimer is skipped, unless the
    /// preferred worker was last seen more than [`AFFINITY_SEEN_SECS`] ago
    /// (or never). Among claimable items, the claimer's own preferred items
    /// come first, then oldest first. Nothing else: no waiting, no scoring.
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
    /// returns the item. The worker becomes the session's preferred worker
    /// for [`AFFINITY_SECS`] (inherited by the next item).
    async fn complete_work(
        &self,
        id: &str,
        generation: i64,
        outcome: WorkOutcome,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem>;
    /// Back to ready at once. Claimed tool calls go back to pending. The
    /// releasing worker is preferred for the item for [`AFFINITY_SECS`].
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
    /// Does the session have a tool call without a result (pending or
    /// claimed)? Read from the store, so it sees calls made on any replica.
    async fn has_open_tool_calls(&self, session_id: &str) -> StoreResult<bool>;

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
    /// Compare-and-set: replace the sealed value only if the row's
    /// `updated_at` is still `expected_updated_at` (as read). False when
    /// another write came first (re-read and retry). NotFound when no
    /// credential has the name (it was deleted).
    async fn put_credential_if_unchanged(
        &self,
        name: &str,
        expected_updated_at: DateTime<Utc>,
        sealed: &[u8],
        key_id: &str,
    ) -> StoreResult<bool>;

    // Idempotency-Key records (2xx responses only).
    /// Claim the key for this request, or say what to do instead. A record
    /// older than [`IDEMPOTENCY_TTL_HOURS`] counts as absent. A record still
    /// in progress after [`IDEMPOTENCY_TAKEOVER_SECS`] is taken over (`New`).
    /// A different `request_hash` is `Mismatch`, whatever the state.
    async fn idempotency_begin(
        &self,
        scope: &IdempotencyScope,
        request_hash: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<IdempotencyBegin>;
    /// Store the (2xx) response of the request that got `New`.
    async fn idempotency_finish(
        &self,
        scope: &IdempotencyScope,
        request_hash: &str,
        response: &StoredResponse,
        now: DateTime<Utc>,
    ) -> StoreResult<()>;
    /// Drop the in-progress record (the request ended without a 2xx), so a
    /// retry runs again.
    async fn idempotency_abandon(&self, scope: &IdempotencyScope, request_hash: &str)
        -> StoreResult<()>;
    /// Delete records created before `older_than`. Returns how many.
    async fn idempotency_purge(&self, older_than: DateTime<Utc>) -> StoreResult<u64>;

    // Server replicas and turn ownership.
    /// Insert or refresh this replica's row. `started_at` is the process
    /// start (a restarted replica with the same id resets it).
    async fn replica_heartbeat(
        &self,
        replica_id: &str,
        started_at: DateTime<Utc>,
        version: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<()>;
    /// Replicas with a heartbeat at or after `since`, oldest start first.
    async fn list_live_replicas(&self, since: DateTime<Utc>) -> StoreResult<Vec<ReplicaRecord>>;
    /// Every replica row, live or not, oldest start first.
    async fn list_replicas(&self) -> StoreResult<Vec<ReplicaRecord>>;
    /// Delete the replica's row (clean shutdown, or after its turns were
    /// failed). Missing is fine.
    async fn remove_replica(&self, replica_id: &str) -> StoreResult<()>;
    /// Compare-and-set `turn_replica` to `replica_id` when it is unset or
    /// already this replica. False when another replica owns the turn.
    /// NotFound for a missing session.
    async fn claim_turn(&self, session_id: &str, replica_id: &str) -> StoreResult<bool>;
    /// Clear `turn_replica` if it is `replica_id`. False when it was not.
    async fn release_turn(&self, session_id: &str, replica_id: &str) -> StoreResult<bool>;
    /// Record whose gizzi child holds the session's `gizzi_session_id`.
    async fn set_gizzi_replica(&self, session_id: &str, replica_id: Option<&str>) -> StoreResult<()>;
    /// End turns nobody runs any more. A session in `running` or
    /// `waiting_for_worker` is failed when its `turn_replica` is one of
    /// `dead_replicas`, or (with `ownerless_before`) when it has no
    /// `turn_replica` and was last updated before that time (rows left by a
    /// 1.0 server). Each failed session gets, once: an `error` event
    /// `{code: "server_restarted", message}`, status `error` with reason
    /// `server_restarted` plus a `status` event, `turn_replica` cleared, and
    /// its open tool calls closed as errors. The check and the change are
    /// one atomic step, so concurrent callers fail a session exactly once.
    /// Returns the failed sessions.
    async fn fail_orphaned_turns(
        &self,
        dead_replicas: &[String],
        ownerless_before: Option<DateTime<Utc>>,
    ) -> StoreResult<Vec<SessionRecord>>;

    // Coordination between replicas.
    /// Take the sweep lock for one tick. None when another replica holds it
    /// (skip this tick). Postgres: `pg_try_advisory_xact_lock` in a
    /// transaction the guard keeps open, keyed by the schema name.
    /// SQLite: always granted.
    async fn sweep_lock(&self) -> StoreResult<Option<SweepGuard>>;
    /// Wake hints from every replica's writes (Postgres LISTEN on
    /// [`WAKE_CHANNEL`]). None when the store has no cross-process
    /// notifications (SQLite: one process, wake locally).
    async fn subscribe_wakes(&self) -> StoreResult<Option<WakeStream>>;

    // Observability (step 13). Defaults keep other implementations building;
    // both real stores override them.
    /// Store the W3C `traceparent` of the span that owns this row, so a
    /// later span (another request, another worker) can join the same
    /// trace. NotFound when the row is missing; Invalid for a malformed
    /// value.
    async fn set_traceparent(
        &self,
        target: TraceTarget,
        id: &str,
        traceparent: &str,
    ) -> StoreResult<()> {
        let _ = (target, id, traceparent);
        Ok(())
    }
    /// The stored `traceparent`, if any. NotFound when the row is missing.
    async fn get_traceparent(&self, target: TraceTarget, id: &str) -> StoreResult<Option<String>> {
        let _ = (target, id);
        Ok(None)
    }
    /// `(status, count)` for every session status in use, archived included.
    /// For the metrics listener.
    async fn count_sessions_by_status(&self) -> StoreResult<Vec<(String, i64)>> {
        let mut counts = std::collections::BTreeMap::<String, i64>::new();
        let filter = SessionFilter {
            include_archived: true,
            ..Default::default()
        };
        let mut after: Option<String> = None;
        loop {
            let page = self.list_sessions(&filter, 500, after.as_deref()).await?;
            for s in &page {
                *counts.entry(s.status.clone()).or_default() += 1;
            }
            match page.last() {
                Some(last) if page.len() == 500 => after = Some(last.id.clone()),
                _ => break,
            }
        }
        Ok(counts.into_iter().collect())
    }
}

/// A row that carries a `traceparent` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceTarget {
    Session,
    Work,
    ToolCall,
}

impl TraceTarget {
    /// The table, a constant (safe to format into SQL).
    pub fn table(self) -> &'static str {
        match self {
            TraceTarget::Session => "sessions",
            TraceTarget::Work => "work_items",
            TraceTarget::ToolCall => "tool_calls",
        }
    }
}

/// A `traceparent` must parse as W3C version 00 before it is stored.
pub(crate) fn check_traceparent(tp: &str) -> StoreResult<()> {
    if crate::netops::hops::parse(tp).is_some() {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!("not a W3C traceparent: {tp}")))
    }
}

/// Lease end for a claim or renew at `now`.
pub fn lease_end(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::seconds(LEASE_TTL_SECS)
}

/// End of the affinity window started at `now`.
pub fn affinity_end(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::seconds(AFFINITY_SECS)
}

/// Data of the error event a lost turn gets.
pub(crate) fn server_restarted_event() -> Value {
    serde_json::json!({ "code": SERVER_RESTARTED, "message": SERVER_RESTARTED_MESSAGE })
}

/// Data of the status event a lost turn gets.
pub(crate) fn server_restarted_status_event() -> Value {
    serde_json::json!({ "status": "error", "reason": SERVER_RESTARTED })
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
