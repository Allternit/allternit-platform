//! Brain runners: one sealed box per person for their own Claude, ChatGPT,
//! Grok or Kimi plan (PLAN-brain-runner-2026-11-14, CONTRACT-connections-2026-11
//! section 9.6).
//!
//! A box is a small Incus container on the runner host (`ALLTERNIT_RUNNER_HOST`,
//! default `host_mail`) that runs the vendor CLIs unmodified. The person signs
//! in themselves; the runner program inside dials out to
//! `/api/v1/brain-runner/socket` (routes::brain_runners) with a per-box token,
//! and every request goes over that socket.
//!
//! Rules this module enforces:
//! - Never `provisioned_hosts` placement and never in compute-status. The RAM
//!   ledger is its own: rows in state `creating` or `awake` each hold
//!   [`RunnerConfig::memory_mb`] (768) of `ALLTERNIT_RUNNER_RAM_BUDGET_MB`
//!   (default 4096, so 5 awake). A wake waits up to 10 s for room, then
//!   `runner_busy`.
//! - Kill switch: `ALLTERNIT_BRAIN_RUNNERS` must be `1`, or `ensure` refuses.
//!   Cleanup (`logout`, `destroy`, the sweeps) always runs.
//! - Idle 5 minutes: stopped by the reconcile task ([`BrainRunnerService::sweep_idle`]).
//!   Unused 90 days: destroyed by the lifecycle task ([`BrainRunnerService::sweep_unused`]).
//! - A failed create alerts the team (`ops_alert`) and answers `could_not_start`.
//! - One turn in flight per account and a pace limit (20 turns / 5 minutes).
//!   The token-type and person-header checks (D7) live in the model router.
//!
//! Errors carry their contract code as the message prefix (`runner_busy: …`);
//! [`error_code`] and [`error_response`] turn them into the section 9 bodies.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::sync::{mpsc, Notify};

use crate::error::ApiError;
use crate::services::provisioning::{
    runner_incus_name_for, BackendStatus, IncusHttpBackend, ProvisionBackend, ProvisionError, RunnerSpec,
};

pub const DISCLOSURE_VERSION: &str = "2026-11-14.1";

// ---------------------------------------------------------------------------
// Contract types (section 9.6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Claude,
    Chatgpt,
    Grok,
    Kimi,
}

impl Vendor {
    pub const ALL: [Vendor; 4] = [Vendor::Claude, Vendor::Chatgpt, Vendor::Grok, Vendor::Kimi];

    /// "claude" | "chatgpt" | "grok" | "kimi"
    pub fn parse(row_id: &str) -> Option<Vendor> {
        match row_id {
            "claude" => Some(Vendor::Claude),
            "chatgpt" => Some(Vendor::Chatgpt),
            "grok" => Some(Vendor::Grok),
            "kimi" => Some(Vendor::Kimi),
            _ => None,
        }
    }

    pub fn row_id(self) -> &'static str {
        match self {
            Vendor::Claude => "claude",
            Vendor::Chatgpt => "chatgpt",
            Vendor::Grok => "grok",
            Vendor::Kimi => "kimi",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Vendor::Claude => "Claude",
            Vendor::Chatgpt => "ChatGPT",
            Vendor::Grok => "Grok",
            Vendor::Kimi => "Kimi",
        }
    }
}

/// The box for one account: awake, connected over the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerHandle {
    pub runner_id: String,
    pub user_id: String,
}

/// What the connections route turns into section 4's `login` object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoginView {
    pub kind: &'static str, // device_code | link_paste
    pub code: Option<String>,
    pub url: Option<String>,
    pub url_with_code: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub poll: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LoginStatus {
    Pending,
    Done,
    Expired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Disclosure {
    pub version: &'static str,
    pub title: String,
    pub body: Vec<String>,
    pub accept_label: &'static str,
    pub decline_label: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
} // roles: system | user | assistant

#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    Delta(String),
    Done { input_tokens: Option<u32>, output_tokens: Option<u32> },
    Error { code: &'static str, message: String }, // login_expired | vendor_limited | runner_busy | could_not_start
}

/// A turn's event stream, boxed so the route holds one type.
pub type TurnStream = std::pin::Pin<Box<dyn futures::Stream<Item = TurnEvent> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum VendorState {
    Off,
    Connecting,
    On { found: Option<String> },
    Expired,
    Limited,
}

// ---------------------------------------------------------------------------
// Error codes (sections 9.2, 9.3)
// ---------------------------------------------------------------------------

pub const RUNNER_BUSY: &str = "runner_busy";
pub const COULD_NOT_START: &str = "could_not_start";
pub const PLAN_TURN_IN_FLIGHT: &str = "plan_turn_in_flight";
pub const PLAN_RATE: &str = "plan_rate";
pub const NOT_FOUND: &str = "not_found";

const CODES: [&str; 5] = [RUNNER_BUSY, COULD_NOT_START, PLAN_TURN_IN_FLIGHT, PLAN_RATE, NOT_FOUND];

fn runner_busy() -> ApiError {
    ApiError::ServiceUnavailable(format!("{RUNNER_BUSY}: Busy. Try again"))
}

fn could_not_start() -> ApiError {
    ApiError::ServiceUnavailable(format!("{COULD_NOT_START}: Could not start. Try again"))
}

fn not_offered() -> ApiError {
    ApiError::NotFound(format!("{NOT_FOUND}: this plan cannot be connected here"))
}

/// The section 9 code an error from this module carries, if any.
pub fn error_code(error: &ApiError) -> Option<&'static str> {
    let message = match error {
        ApiError::ServiceUnavailable(m) | ApiError::NotFound(m) | ApiError::Conflict(m) | ApiError::TooManyRequests(m) => m,
        _ => return None,
    };
    let code = message.split(':').next()?;
    CODES.iter().copied().find(|c| *c == code)
}

/// The section 9 response (`{"error":{"code","message"}}` with its status)
/// for an error from this module; `None` for any other error.
pub fn error_response(error: &ApiError) -> Option<Response> {
    let code = error_code(error)?;
    let (status, message) = match code {
        RUNNER_BUSY => (StatusCode::SERVICE_UNAVAILABLE, "Busy. Try again"),
        COULD_NOT_START => (StatusCode::BAD_GATEWAY, "Could not start. Try again"),
        PLAN_TURN_IN_FLIGHT => (StatusCode::CONFLICT, "Wait for the reply before sending another message"),
        PLAN_RATE => (StatusCode::TOO_MANY_REQUESTS, "Too many messages. Wait a few minutes"),
        _ => (StatusCode::NOT_FOUND, "Not found"),
    };
    Some((status, Json(json!({ "error": { "code": code, "message": message } }))).into_response())
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// `ALLTERNIT_BRAIN_RUNNERS=1`. Off: `ensure` refuses; cleanup still runs.
    pub enabled: bool,
    /// `ALLTERNIT_RUNNER_VENDORS`, default all four.
    pub vendors: Vec<Vendor>,
    /// `ALLTERNIT_RUNNER_HOST`, default `host_mail`.
    pub host_id: String,
    /// `ALLTERNIT_RUNNER_INCUS_ENDPOINT`; unset reads the host's
    /// `provisioned_hosts.incus_endpoint` (a lookup, never placement).
    pub incus_endpoint: Option<String>,
    /// `ALLTERNIT_RUNNER_IMAGE`, default `allternit-brain-runner`.
    pub image: String,
    /// `ALLTERNIT_RUNNER_POOL`, default `runner`.
    pub storage_pool: String,
    /// `ALLTERNIT_RUNNER_PROFILES`, default `brain-runner`.
    pub profiles: Vec<String>,
    /// `ALLTERNIT_RUNNER_INIT`, default `/usr/local/bin/allternit-runner-init` (the image wrapper: fixes the home owner, drops root, runs the runner under tini).
    pub init_cmd: String,
    /// `ALLTERNIT_RUNNER_SOCKET_URL`, what the box dials.
    pub socket_url: String,
    /// `ALLTERNIT_RUNNER_RAM_BUDGET_MB`, default 4096.
    pub ram_budget_mb: u32,
    pub memory_mb: u32,
    pub cpu_cores: u32,
    pub max_processes: u32,
    pub home_size_mb: u32,
    pub idle_stop: chrono::Duration,
    pub destroy_after: chrono::Duration,
    /// How long a wake waits for room in the budget before `runner_busy`.
    pub budget_wait: Duration,
    /// How long a started box has to dial in and say hello.
    pub socket_wait: Duration,
    /// Login, submit, status and logout replies.
    pub request_timeout: Duration,
    /// A turn is closed when no frame arrives for this long.
    pub turn_idle_timeout: Duration,
    /// `ALLTERNIT_PLAN_PACE_TURNS` turns per `pace_window` per account.
    pub pace_turns: usize,
    pub pace_window: Duration,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            vendors: Vendor::ALL.to_vec(),
            host_id: "host_mail".to_string(),
            incus_endpoint: None,
            image: "allternit-brain-runner".to_string(),
            storage_pool: "runner".to_string(),
            profiles: vec!["brain-runner".to_string()],
            init_cmd: "/usr/local/bin/allternit-runner-init".to_string(),
            socket_url: "wss://api.allternit.com/api/v1/brain-runner/socket".to_string(),
            ram_budget_mb: 4096,
            memory_mb: 768,
            cpu_cores: 1,
            max_processes: 256,
            home_size_mb: 256,
            idle_stop: chrono::Duration::minutes(5),
            destroy_after: chrono::Duration::days(90),
            budget_wait: Duration::from_secs(10),
            socket_wait: Duration::from_secs(45),
            request_timeout: Duration::from_secs(30),
            turn_idle_timeout: Duration::from_secs(60),
            pace_turns: 20,
            pace_window: Duration::from_secs(300),
        }
    }
}

fn env_text(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl RunnerConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        cfg.enabled = env_text("ALLTERNIT_BRAIN_RUNNERS").as_deref() == Some("1");
        if let Some(list) = env_text("ALLTERNIT_RUNNER_VENDORS") {
            cfg.vendors = list.split(',').filter_map(|v| Vendor::parse(v.trim())).collect();
        }
        if let Some(v) = env_text("ALLTERNIT_RUNNER_HOST") {
            cfg.host_id = v;
        }
        cfg.incus_endpoint = env_text("ALLTERNIT_RUNNER_INCUS_ENDPOINT");
        if let Some(v) = env_text("ALLTERNIT_RUNNER_IMAGE") {
            cfg.image = v;
        }
        if let Some(v) = env_text("ALLTERNIT_RUNNER_POOL") {
            cfg.storage_pool = v;
        }
        if let Some(v) = env_text("ALLTERNIT_RUNNER_PROFILES") {
            cfg.profiles = v.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        }
        if let Some(v) = env_text("ALLTERNIT_RUNNER_INIT") {
            cfg.init_cmd = v;
        }
        if let Some(v) = env_text("ALLTERNIT_RUNNER_SOCKET_URL") {
            cfg.socket_url = v;
        }
        if let Some(v) = env_text("ALLTERNIT_RUNNER_RAM_BUDGET_MB").and_then(|v| v.parse().ok()) {
            cfg.ram_budget_mb = v;
        }
        if let Some(v) = env_text("ALLTERNIT_PLAN_PACE_TURNS").and_then(|v| v.parse().ok()) {
            cfg.pace_turns = v;
        }
        cfg
    }

    /// Boxes that may be awake at once.
    pub fn capacity(&self) -> i64 {
        i64::from(self.ram_budget_mb / self.memory_mb.max(1))
    }
}

// ---------------------------------------------------------------------------
// Backend seam
// ---------------------------------------------------------------------------

/// What the service needs from the host. Production is Incus
/// ([`IncusRunnerBackend`]); tests use a fake.
#[async_trait]
pub trait RunnerBackend: Send + Sync {
    /// Make the box and its home volume, not started.
    async fn create(&self, spec: &RunnerSpec) -> Result<(), ProvisionError>;
    async fn start(&self, name: &str) -> Result<(), ProvisionError>;
    async fn stop(&self, name: &str) -> Result<(), ProvisionError>;
    async fn status(&self, name: &str) -> Result<BackendStatus, ProvisionError>;
    /// Delete the box and its home volume. Missing pieces are success.
    async fn destroy(&self, name: &str, storage_pool: &str, home_volume: &str) -> Result<(), ProvisionError>;
}

/// Incus on the runner host, built on first use so cloud-api boots without
/// the runner host configured.
pub struct IncusRunnerBackend {
    db: PgPool,
    host_id: String,
    endpoint: Option<String>,
    inner: tokio::sync::OnceCell<Arc<IncusHttpBackend>>,
}

impl IncusRunnerBackend {
    pub fn new(db: PgPool, host_id: String, endpoint: Option<String>) -> Self {
        Self { db, host_id, endpoint, inner: tokio::sync::OnceCell::new() }
    }

    async fn incus(&self) -> Result<Arc<IncusHttpBackend>, ProvisionError> {
        self.inner
            .get_or_try_init(|| async {
                let endpoint = match &self.endpoint {
                    Some(endpoint) => endpoint.clone(),
                    None => sqlx::query_scalar::<_, String>("SELECT incus_endpoint FROM provisioned_hosts WHERE id = $1")
                        .bind(&self.host_id)
                        .fetch_optional(&self.db)
                        .await
                        .map_err(|e| ProvisionError::Request(e.to_string()))?
                        .ok_or_else(|| ProvisionError::Request(format!("runner host {} is not registered", self.host_id)))?,
                };
                IncusHttpBackend::new(&endpoint).map(Arc::new)
            })
            .await
            .cloned()
    }
}

#[async_trait]
impl RunnerBackend for IncusRunnerBackend {
    async fn create(&self, spec: &RunnerSpec) -> Result<(), ProvisionError> {
        self.incus().await?.create_runner(spec).await
    }
    async fn start(&self, name: &str) -> Result<(), ProvisionError> {
        ProvisionBackend::start(&*self.incus().await?, name).await
    }
    async fn stop(&self, name: &str) -> Result<(), ProvisionError> {
        ProvisionBackend::stop(&*self.incus().await?, name).await
    }
    async fn status(&self, name: &str) -> Result<BackendStatus, ProvisionError> {
        ProvisionBackend::status(&*self.incus().await?, name).await
    }
    async fn destroy(&self, name: &str, storage_pool: &str, home_volume: &str) -> Result<(), ProvisionError> {
        self.incus().await?.delete_runner(name, storage_pool, home_volume).await
    }
}

// ---------------------------------------------------------------------------
// Socket map (one live socket per runner; routes::brain_runners pumps it)
// ---------------------------------------------------------------------------

type Pending = Arc<StdMutex<HashMap<String, mpsc::UnboundedSender<Value>>>>;

struct SocketEntry {
    conn_id: u64,
    tx: mpsc::UnboundedSender<String>,
    ready: bool,
    pending: Pending,
}

/// In-memory socket per runner, like `runtime_relay`'s hub. Dropping an
/// entry drops its outgoing sender (the pump then closes the socket) and its
/// pending reply channels (waiting requests then fail).
#[derive(Default)]
pub struct RunnerSockets {
    map: StdMutex<HashMap<String, SocketEntry>>,
    next_conn: AtomicU64,
    changed: Notify,
}

/// One attached socket: frames for the runner arrive on `outgoing`.
pub struct RunnerConn {
    pub runner_id: String,
    pub conn_id: u64,
    pub outgoing: mpsc::UnboundedReceiver<String>,
}

impl RunnerSockets {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SocketEntry>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A second connection for the same runner replaces the first.
    fn attach(&self, runner_id: &str) -> RunnerConn {
        let (tx, outgoing) = mpsc::unbounded_channel();
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = SocketEntry { conn_id, tx, ready: false, pending: Arc::default() };
        self.lock().insert(runner_id.to_string(), entry);
        self.changed.notify_waiters();
        RunnerConn { runner_id: runner_id.to_string(), conn_id, outgoing }
    }

    fn detach(&self, runner_id: &str, conn_id: u64) {
        let mut map = self.lock();
        if map.get(runner_id).is_some_and(|e| e.conn_id == conn_id) {
            map.remove(runner_id);
        }
        drop(map);
        self.changed.notify_waiters();
    }

    fn close(&self, runner_id: &str) {
        self.lock().remove(runner_id);
        self.changed.notify_waiters();
    }

    fn mark_ready(&self, runner_id: &str, conn_id: u64) {
        if let Some(entry) = self.lock().get_mut(runner_id).filter(|e| e.conn_id == conn_id) {
            entry.ready = true;
        }
        self.changed.notify_waiters();
    }

    fn is_ready(&self, runner_id: &str) -> bool {
        self.lock().get(runner_id).is_some_and(|e| e.ready)
    }

    /// Send `frame` (which carries `id`) and return where its replies land.
    fn request(&self, runner_id: &str, id: &str, frame: &Value) -> Option<mpsc::UnboundedReceiver<Value>> {
        let map = self.lock();
        let entry = map.get(runner_id).filter(|e| e.ready)?;
        let (tx, rx) = mpsc::unbounded_channel();
        entry.pending.lock().unwrap_or_else(|p| p.into_inner()).insert(id.to_string(), tx);
        entry.tx.send(frame.to_string()).ok()?;
        Some(rx)
    }

    fn forget(&self, runner_id: &str, id: &str) {
        if let Some(entry) = self.lock().get(runner_id) {
            entry.pending.lock().unwrap_or_else(|p| p.into_inner()).remove(id);
        }
    }

    fn deliver(&self, runner_id: &str, conn_id: u64, frame: Value) {
        let Some(id) = frame.get("id").and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        let map = self.lock();
        let Some(entry) = map.get(runner_id).filter(|e| e.conn_id == conn_id) else {
            return;
        };
        let mut pending = entry.pending.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = pending.get(&id) {
            if tx.send(frame).is_err() {
                pending.remove(&id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Turn limits
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TurnLimits {
    in_flight: HashSet<String>,
    recent: HashMap<String, VecDeque<Instant>>,
}

/// Frees the account's one in-flight turn when the stream ends or is dropped.
struct TurnGuard {
    limits: Arc<StdMutex<TurnLimits>>,
    user_id: String,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.limits.lock().unwrap_or_else(|p| p.into_inner()).in_flight.remove(&self.user_id);
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct RunnerRow {
    id: String,
    instance_name: String,
    home_volume: String,
    storage_pool: String,
    state: String,
    vendors: Value,
    vendor_plans: Value,
}

const ROW_COLUMNS: &str = "id, instance_name, home_volume, storage_pool, state, vendors, vendor_plans";

pub struct BrainRunnerService {
    db: PgPool,
    backend: Arc<dyn RunnerBackend>,
    sockets: Arc<RunnerSockets>,
    cfg: RunnerConfig,
    /// Serializes the budget check-and-claim.
    ledger: tokio::sync::Mutex<()>,
    /// Signalled when a box stops, fails or is destroyed (room in the budget).
    freed: Notify,
    /// One wake at a time per account.
    user_locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    turns: Arc<StdMutex<TurnLimits>>,
}

static SHARED: OnceLock<Arc<BrainRunnerService>> = OnceLock::new();

/// The process-wide service (one socket map per process), built from the
/// environment on first use.
pub fn shared(db: &PgPool) -> Arc<BrainRunnerService> {
    SHARED
        .get_or_init(|| {
            let cfg = RunnerConfig::from_env();
            let backend = Arc::new(IncusRunnerBackend::new(db.clone(), cfg.host_id.clone(), cfg.incus_endpoint.clone()));
            Arc::new(BrainRunnerService::new(db.clone(), backend, cfg))
        })
        .clone()
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn new_token() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The runner's word for a vendor (hello `vendors` values) as stored.
fn stored_vendor_word(value: &Value) -> Option<&'static str> {
    let word = value.as_str().or_else(|| value.get("state").and_then(Value::as_str))?;
    Some(match word {
        "signed_in" => "signed_in",
        "signed_out" => "signed_out",
        "expired" | "login_expired" => "expired",
        "limited" | "vendor_limited" => "limited",
        "connecting" | "pending" => "connecting",
        _ => return None,
    })
}

fn parse_login_status(value: Option<&str>) -> Option<LoginStatus> {
    Some(match value? {
        "pending" => LoginStatus::Pending,
        "done" => LoginStatus::Done,
        "expired" => LoginStatus::Expired,
        "failed" => LoginStatus::Failed,
        _ => return None,
    })
}

impl BrainRunnerService {
    pub fn new(db: PgPool, backend: Arc<dyn RunnerBackend>, cfg: RunnerConfig) -> Self {
        Self {
            db,
            backend,
            sockets: Arc::new(RunnerSockets::default()),
            cfg,
            ledger: tokio::sync::Mutex::new(()),
            freed: Notify::new(),
            user_locks: StdMutex::new(HashMap::new()),
            turns: Arc::new(StdMutex::new(TurnLimits::default())),
        }
    }

    pub fn config(&self) -> &RunnerConfig {
        &self.cfg
    }

    /// Whether `vendor` can be connected through a runner on this server
    /// (kill switch on and the vendor in `ALLTERNIT_RUNNER_VENDORS`).
    pub fn offers(&self, vendor: Vendor) -> bool {
        self.cfg.enabled && self.cfg.vendors.contains(&vendor)
    }

    fn check_offered(&self, vendor: Vendor) -> Result<(), ApiError> {
        if self.offers(vendor) {
            Ok(())
        } else {
            Err(not_offered())
        }
    }

    fn user_lock(&self, user_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.user_locks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(user_id.to_string())
            .or_default()
            .clone()
    }

    async fn row(&self, user_id: &str) -> Result<Option<RunnerRow>, ApiError> {
        Ok(sqlx::query_as::<_, RunnerRow>(&format!("SELECT {ROW_COLUMNS} FROM brain_runners WHERE user_id = $1"))
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?)
    }

    async fn touch(&self, runner_id: &str) -> Result<(), ApiError> {
        sqlx::query("UPDATE brain_runners SET last_used_at = now() WHERE id = $1")
            .bind(runner_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn set_state(&self, runner_id: &str, state: &str, error: Option<&str>) -> Result<(), ApiError> {
        sqlx::query("UPDATE brain_runners SET state = $2, last_error = COALESCE($3, last_error), updated_at = now() WHERE id = $1")
            .bind(runner_id)
            .bind(state)
            .bind(error)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn set_vendor(&self, runner_id: &str, vendor: Vendor, word: &str) -> Result<(), ApiError> {
        set_vendor_word(&self.db, runner_id, vendor, word).await
    }

    fn spec(&self, row: &RunnerRow, token: &str) -> RunnerSpec {
        RunnerSpec {
            name: row.instance_name.clone(),
            image: self.cfg.image.clone(),
            storage_pool: row.storage_pool.clone(),
            profiles: self.cfg.profiles.clone(),
            home_volume: row.home_volume.clone(),
            home_size_mb: self.cfg.home_size_mb,
            memory_mb: self.cfg.memory_mb,
            cpu_cores: self.cfg.cpu_cores,
            max_processes: self.cfg.max_processes,
            init_cmd: self.cfg.init_cmd.clone(),
            environment: vec![
                ("ALLTERNIT_RUNNER_URL".to_string(), self.cfg.socket_url.clone()),
                ("ALLTERNIT_RUNNER_TOKEN".to_string(), token.to_string()),
                ("HOME".to_string(), crate::services::provisioning::RUNNER_HOME.to_string()),
                // The box enables only these; without it the runner defaults to claude alone.
                (
                    "ALLTERNIT_RUNNER_VENDORS".to_string(),
                    self.cfg.vendors.iter().map(|v| v.row_id()).collect::<Vec<_>>().join(","),
                ),
            ],
        }
    }

    // -- Lifecycle ---------------------------------------------------------

    /// Create or wake the person's box and wait until its socket is up.
    /// Err `runner_busy` after the 10 s budget queue; `could_not_start` on failure.
    pub async fn ensure(&self, user_id: &str) -> Result<RunnerHandle, ApiError> {
        if !self.cfg.enabled {
            return Err(not_offered());
        }
        match self.wake(user_id, true).await? {
            Some(handle) => Ok(handle),
            None => Err(could_not_start()),
        }
    }

    /// Wake the box. `allow_create` false (cleanup paths) answers `None`
    /// when the account has no box, and ignores the kill switch.
    async fn wake(&self, user_id: &str, allow_create: bool) -> Result<Option<RunnerHandle>, ApiError> {
        let lock = self.user_lock(user_id);
        let _one_wake = lock.lock().await;
        let row = self.row(user_id).await?;
        match &row {
            Some(r) if r.state == "awake" && self.sockets.is_ready(&r.id) => {
                self.touch(&r.id).await?;
                return Ok(Some(RunnerHandle { runner_id: r.id.clone(), user_id: user_id.to_string() }));
            }
            None if !allow_create => return Ok(None),
            _ => {}
        }
        let counted = row.as_ref().is_some_and(|r| r.state == "awake" || r.state == "creating");
        let (row, token) = if counted {
            (row.expect("counted implies a row"), None)
        } else {
            self.claim_slot(user_id, row).await?
        };
        match self.boot(&row, token).await {
            Ok(()) => {
                self.set_state(&row.id, "awake", None).await?;
                self.touch(&row.id).await?;
                Ok(Some(RunnerHandle { runner_id: row.id, user_id: user_id.to_string() }))
            }
            Err(detail) => {
                tracing::warn!(runner = %row.id, %detail, "brain runner could not start");
                // Never leave a box holding RAM outside the ledger.
                let _ = self.backend.stop(&row.instance_name).await;
                self.sockets.close(&row.id);
                self.set_state(&row.id, "failed", Some(&detail)).await?;
                self.freed.notify_waiters();
                crate::services::ops_alert::send_once(
                    &format!("brain-runner-start:{}", self.cfg.host_id),
                    format!("Brain runner could not start on {}", self.cfg.host_id),
                    format!("Runner {} ({}) failed to start: {detail}", row.id, row.instance_name),
                );
                Err(could_not_start())
            }
        }
    }

    /// Wait for room in the RAM budget, then mark the box `creating` (new
    /// row, with a fresh token, when the account has none).
    async fn claim_slot(&self, user_id: &str, row: Option<RunnerRow>) -> Result<(RunnerRow, Option<String>), ApiError> {
        let deadline = Instant::now() + self.cfg.budget_wait;
        loop {
            let freed = self.freed.notified();
            {
                let _ledger = self.ledger.lock().await;
                let used: i64 = sqlx::query_scalar("SELECT count(*) FROM brain_runners WHERE state IN ('creating', 'awake')")
                    .fetch_one(&self.db)
                    .await?;
                if used < self.cfg.capacity() {
                    return match row {
                        Some(row) => {
                            self.set_state(&row.id, "creating", None).await?;
                            Ok((RunnerRow { state: "creating".to_string(), ..row }, None))
                        }
                        None => {
                            let token = new_token();
                            let id = format!("br_{}", uuid::Uuid::new_v4().simple());
                            let name = runner_incus_name_for(user_id);
                            let created = sqlx::query_as::<_, RunnerRow>(&format!(
                                "INSERT INTO brain_runners (id, user_id, instance_name, home_volume, host_id, storage_pool, state, token_hash) \
                                 VALUES ($1, $2, $3, $4, $5, $6, 'creating', $7) RETURNING {ROW_COLUMNS}"
                            ))
                            .bind(&id)
                            .bind(user_id)
                            .bind(&name)
                            .bind(format!("{name}-home"))
                            .bind(&self.cfg.host_id)
                            .bind(&self.cfg.storage_pool)
                            .bind(sha256_hex(&token))
                            .fetch_one(&self.db)
                            .await?;
                            Ok((created, Some(token)))
                        }
                    };
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(runner_busy());
            }
            let _ = tokio::time::timeout(left.min(Duration::from_millis(500)), freed).await;
        }
    }

    /// Bring the box up and wait for its hello. `token` is set for a box
    /// that does not exist yet.
    async fn boot(&self, row: &RunnerRow, token: Option<String>) -> Result<(), String> {
        let token = match token {
            Some(token) => Some(token),
            None => match self.backend.status(&row.instance_name).await.map_err(|e| e.to_string())? {
                BackendStatus::Running | BackendStatus::Busy => None,
                BackendStatus::Stopped => {
                    self.backend.start(&row.instance_name).await.map_err(|e| e.to_string())?;
                    None
                }
                // The container is gone (the home volume may still hold the
                // sign-ins and is reused). A new token, since only its hash
                // was kept.
                BackendStatus::NotFound => {
                    let token = new_token();
                    sqlx::query("UPDATE brain_runners SET token_hash = $2, updated_at = now() WHERE id = $1")
                        .bind(&row.id)
                        .bind(sha256_hex(&token))
                        .execute(&self.db)
                        .await
                        .map_err(|e| e.to_string())?;
                    Some(token)
                }
            },
        };
        if let Some(token) = token {
            self.backend.create(&self.spec(row, &token)).await.map_err(|e| e.to_string())?;
            self.backend.start(&row.instance_name).await.map_err(|e| e.to_string())?;
        }
        let deadline = Instant::now() + self.cfg.socket_wait;
        loop {
            let changed = self.sockets.changed.notified();
            if self.sockets.is_ready(&row.id) {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the runner did not connect".to_string());
            }
            let _ = tokio::time::timeout(left.min(Duration::from_millis(250)), changed).await;
        }
    }

    /// Stop and delete the box and all CLI homes (account erase, 90-day idle sweep). Keeps consent rows.
    pub async fn destroy(&self, user_id: &str) -> Result<(), ApiError> {
        let lock = self.user_lock(user_id);
        let _one = lock.lock().await;
        let Some(row) = self.row(user_id).await? else {
            return Ok(());
        };
        self.sockets.close(&row.id);
        self.backend
            .destroy(&row.instance_name, &row.storage_pool, &row.home_volume)
            .await
            .map_err(|e| ApiError::ServiceUnavailable(format!("brain runner destroy: {e}")))?;
        sqlx::query("DELETE FROM brain_runners WHERE id = $1").bind(&row.id).execute(&self.db).await?;
        self.freed.notify_waiters();
        Ok(())
    }

    /// Stop boxes idle for 5 minutes (none with a turn in flight), and fail
    /// boxes stuck starting for 10. Run by the reconcile task.
    pub async fn sweep_idle(&self, now: DateTime<Utc>) -> Result<(), ApiError> {
        let idle: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, user_id, instance_name FROM brain_runners \
             WHERE (state = 'awake' AND last_used_at < $1) OR (state = 'creating' AND updated_at < $2)",
        )
        .bind(now - self.cfg.idle_stop)
        .bind(now - chrono::Duration::minutes(10))
        .fetch_all(&self.db)
        .await?;
        for (id, user_id, name) in idle {
            if self.turns.lock().unwrap_or_else(|p| p.into_inner()).in_flight.contains(&user_id) {
                continue;
            }
            let lock = self.user_lock(&user_id);
            let Ok(_one) = lock.try_lock() else {
                continue; // a wake or turn is starting
            };
            match self.backend.stop(&name).await {
                Ok(()) | Err(ProvisionError::NotFound(_)) => {}
                Err(error) => {
                    tracing::warn!(runner = %id, %error, "brain runner idle stop failed");
                    continue;
                }
            }
            self.sockets.close(&id);
            sqlx::query("UPDATE brain_runners SET state = 'stopped', updated_at = now() WHERE id = $1 AND state IN ('awake', 'creating')")
                .bind(&id)
                .execute(&self.db)
                .await?;
            self.freed.notify_waiters();
        }
        Ok(())
    }

    /// Destroy boxes unused for 90 days. Run by the lifecycle task.
    pub async fn sweep_unused(&self, now: DateTime<Utc>) -> Result<(), ApiError> {
        let users: Vec<String> = sqlx::query_scalar("SELECT user_id FROM brain_runners WHERE last_used_at < $1")
            .bind(now - self.cfg.destroy_after)
            .fetch_all(&self.db)
            .await?;
        for user_id in users {
            if let Err(error) = self.destroy(&user_id).await {
                tracing::warn!(%error, "brain runner 90-day destroy failed");
            }
        }
        Ok(())
    }

    // -- Socket (routes::brain_runners) ------------------------------------

    /// The runner id for a socket token, or `None`.
    pub async fn authenticate_runner(&self, token: &str) -> Result<Option<String>, ApiError> {
        Ok(sqlx::query_scalar("SELECT id FROM brain_runners WHERE token_hash = $1")
            .bind(sha256_hex(token))
            .fetch_optional(&self.db)
            .await?)
    }

    /// Register a runner's socket (replacing an older one).
    pub fn attach_runner(&self, runner_id: &str) -> RunnerConn {
        self.sockets.attach(runner_id)
    }

    pub fn detach_runner(&self, runner_id: &str, conn_id: u64) {
        self.sockets.detach(runner_id, conn_id);
    }

    /// One text frame from a runner. `hello` records the vendors it reports
    /// and marks the socket up; replies go to the request with their `id`;
    /// anything else is ignored.
    pub async fn runner_frame(&self, runner_id: &str, conn_id: u64, text: &str) {
        let Ok(frame) = serde_json::from_str::<Value>(text) else {
            return;
        };
        match frame.get("type").and_then(Value::as_str) {
            Some("hello") => {
                let mut vendors = serde_json::Map::new();
                let mut plans = serde_json::Map::new();
                if let Some(reported) = frame.get("vendors").and_then(Value::as_object) {
                    for (key, value) in reported {
                        let (Some(vendor), Some(word)) = (Vendor::parse(key), stored_vendor_word(value)) else {
                            continue;
                        };
                        vendors.insert(vendor.row_id().to_string(), word.into());
                        if let Some(plan) = value.get("plan").and_then(Value::as_str) {
                            plans.insert(vendor.row_id().to_string(), plan.into());
                        }
                    }
                }
                let saved = sqlx::query("UPDATE brain_runners SET vendors = $2, vendor_plans = $3, updated_at = now() WHERE id = $1")
                    .bind(runner_id)
                    .bind(Value::Object(vendors))
                    .bind(Value::Object(plans))
                    .execute(&self.db)
                    .await;
                if let Err(error) = saved {
                    tracing::warn!(runner = %runner_id, %error, "brain runner hello not saved");
                }
                self.sockets.mark_ready(runner_id, conn_id);
            }
            Some("pong") | None => {}
            Some(_) => self.sockets.deliver(runner_id, conn_id, frame),
        }
    }

    /// Send one request and wait for its single reply. Error frames for
    /// `login_expired` / `vendor_limited` update the vendor's state.
    async fn call(&self, handle: &RunnerHandle, vendor: Vendor, mut frame: Value) -> Result<Value, ApiError> {
        let id = format!("r_{}", uuid::Uuid::new_v4().simple());
        frame["id"] = id.clone().into();
        frame["vendor"] = vendor.row_id().into();
        let mut replies = self.sockets.request(&handle.runner_id, &id, &frame).ok_or_else(could_not_start)?;
        let reply = tokio::time::timeout(self.cfg.request_timeout, replies.recv()).await;
        self.sockets.forget(&handle.runner_id, &id);
        let reply = match reply {
            Ok(Some(reply)) => reply,
            Ok(None) | Err(_) => return Err(could_not_start()),
        };
        if reply.get("type").and_then(Value::as_str) == Some("error") {
            match reply.get("code").and_then(Value::as_str) {
                Some("login_expired") => self.set_vendor(&handle.runner_id, vendor, "expired").await?,
                Some("vendor_limited") => self.set_vendor(&handle.runner_id, vendor, "limited").await?,
                _ => {}
            }
            return Err(could_not_start());
        }
        Ok(reply)
    }

    // -- Disclosure --------------------------------------------------------

    /// Current text, filled with the vendor name (section 9.8).
    pub async fn disclosure_for(&self, vendor: Vendor) -> Disclosure {
        let name = vendor.display_name();
        Disclosure {
            version: DISCLOSURE_VERSION,
            title: format!("Before you connect your {name} plan"),
            body: vec![
                "Your plan runs on an Allternit server, in a private space only your account uses.".to_string(),
                format!(
                    "{name}'s terms may not allow using a plan this way. {name} could limit or close your account. \
                     Allternit does not control that and cannot undo it."
                ),
                "You sign in yourself. Your sign-in stays in that private space and Allternit does not copy it anywhere else. \
                 Disconnect signs you out and deletes it."
                    .to_string(),
                "Your plan only answers messages you send yourself, in chat. Bots, routines, schedules and agents never use it."
                    .to_string(),
            ],
            accept_label: "I understand, connect",
            decline_label: "Use a key instead",
        }
    }

    /// The newest disclosure version this account accepted for `vendor`.
    pub async fn accepted_version(&self, user_id: &str, vendor: Vendor) -> Result<Option<String>, ApiError> {
        Ok(sqlx::query_scalar(
            "SELECT disclosure_version FROM plan_runner_consents WHERE user_id = $1 AND vendor = $2 \
             ORDER BY accepted_at DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(vendor.row_id())
        .fetch_optional(&self.db)
        .await?)
    }

    /// `version` must equal [`DISCLOSURE_VERSION`] or this is a 400. Accepting
    /// again keeps the first time.
    pub async fn accept_disclosure(&self, user_id: &str, vendor: Vendor, version: &str) -> Result<DateTime<Utc>, ApiError> {
        if version != DISCLOSURE_VERSION {
            return Err(ApiError::BadRequest(format!("disclosure version {version} is not the current one")));
        }
        sqlx::query(
            "INSERT INTO plan_runner_consents (user_id, vendor, disclosure_version) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(vendor.row_id())
        .bind(version)
        .execute(&self.db)
        .await?;
        Ok(sqlx::query_scalar(
            "SELECT accepted_at FROM plan_runner_consents WHERE user_id = $1 AND vendor = $2 AND disclosure_version = $3",
        )
        .bind(user_id)
        .bind(vendor.row_id())
        .bind(version)
        .fetch_one(&self.db)
        .await?)
    }

    // -- Sign-in -----------------------------------------------------------

    pub async fn start_login(&self, user_id: &str, vendor: Vendor) -> Result<LoginView, ApiError> {
        self.check_offered(vendor)?;
        let handle = self.ensure(user_id).await?;
        let reply = self.call(&handle, vendor, json!({ "type": "login_start" })).await?;
        let kind = match reply.get("kind").and_then(Value::as_str) {
            Some("device_code") => "device_code",
            Some("link_paste") => "link_paste",
            _ => return Err(could_not_start()),
        };
        let text = |key: &str| reply.get(key).and_then(Value::as_str).map(str::to_string);
        let view = LoginView {
            kind,
            code: text("code"),
            url: text("url"),
            url_with_code: text("url_with_code"),
            expires_at: text("expires_at")
                .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
                .map(|t| t.with_timezone(&Utc)),
            poll: reply.get("poll").and_then(Value::as_bool).unwrap_or(true),
        };
        self.set_vendor(&handle.runner_id, vendor, "connecting").await?;
        Ok(view)
    }

    pub async fn submit_code(&self, user_id: &str, vendor: Vendor, value: &str) -> Result<LoginStatus, ApiError> {
        self.check_offered(vendor)?;
        let handle = self.ensure(user_id).await?;
        let reply = self.call(&handle, vendor, json!({ "type": "login_submit", "value": value })).await?;
        self.record_login(&handle, vendor, &reply).await
    }

    pub async fn login_status(&self, user_id: &str, vendor: Vendor) -> Result<LoginStatus, ApiError> {
        self.check_offered(vendor)?;
        let handle = self.ensure(user_id).await?;
        let reply = self.call(&handle, vendor, json!({ "type": "login_status" })).await?;
        self.record_login(&handle, vendor, &reply).await
    }

    async fn record_login(&self, handle: &RunnerHandle, vendor: Vendor, reply: &Value) -> Result<LoginStatus, ApiError> {
        let status = parse_login_status(reply.get("status").and_then(Value::as_str)).ok_or_else(could_not_start)?;
        let word = match status {
            LoginStatus::Done => "signed_in",
            LoginStatus::Pending => "connecting",
            LoginStatus::Expired | LoginStatus::Failed => "signed_out",
        };
        self.set_vendor(&handle.runner_id, vendor, word).await?;
        Ok(status)
    }

    /// Row state for `build_rows`: signed in, expired, limited, or off. Does not wake the box.
    pub async fn vendor_state(&self, user_id: &str) -> Result<Vec<(Vendor, VendorState)>, ApiError> {
        let row = self.row(user_id).await?;
        Ok(Vendor::ALL
            .iter()
            .map(|vendor| {
                let word = row.as_ref().and_then(|r| r.vendors.get(vendor.row_id())).and_then(Value::as_str);
                let state = match word {
                    Some("signed_in") => VendorState::On {
                        found: row
                            .as_ref()
                            .and_then(|r| r.vendor_plans.get(vendor.row_id()))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    },
                    Some("connecting") => VendorState::Connecting,
                    Some("expired") => VendorState::Expired,
                    Some("limited") => VendorState::Limited,
                    _ => VendorState::Off,
                };
                (*vendor, state)
            })
            .collect())
    }

    /// CLI logout, delete that vendor's CLI home; destroys the box when it was the last vendor.
    pub async fn logout(&self, user_id: &str, vendor: Vendor) -> Result<(), ApiError> {
        let Some(handle) = self.wake(user_id, false).await? else {
            return Ok(());
        };
        let logged_out = self.call(&handle, vendor, json!({ "type": "logout" })).await;
        sqlx::query("UPDATE brain_runners SET vendors = vendors - $2::text, vendor_plans = vendor_plans - $2::text, updated_at = now() WHERE id = $1")
            .bind(&handle.runner_id)
            .bind(vendor.row_id())
            .execute(&self.db)
            .await?;
        let still_connected = self
            .vendor_state(user_id)
            .await?
            .iter()
            .any(|(_, state)| *state != VendorState::Off);
        if !still_connected {
            // Destroying the box deletes every CLI home, so the sign-in is
            // gone even when the logout itself failed.
            return self.destroy(user_id).await;
        }
        // Other vendors stay: only the runner can delete this vendor's home,
        // so a failed logout is an error the client retries.
        logged_out.map(|_| ())
    }

    // -- Turns -------------------------------------------------------------

    fn begin_turn(&self, user_id: &str) -> Result<TurnGuard, ApiError> {
        let mut limits = self.turns.lock().unwrap_or_else(|p| p.into_inner());
        if limits.in_flight.contains(user_id) {
            return Err(ApiError::Conflict(format!("{PLAN_TURN_IN_FLIGHT}: a reply is still coming")));
        }
        let now = Instant::now();
        let window = self.cfg.pace_window;
        let recent = limits.recent.entry(user_id.to_string()).or_default();
        while recent.front().is_some_and(|at| now.duration_since(*at) >= window) {
            recent.pop_front();
        }
        if recent.len() >= self.cfg.pace_turns {
            return Err(ApiError::TooManyRequests(format!("{PLAN_RATE}: too many plan turns")));
        }
        recent.push_back(now);
        limits.in_flight.insert(user_id.to_string());
        Ok(TurnGuard { limits: self.turns.clone(), user_id: user_id.to_string() })
    }

    /// One streamed turn. History is replayed from `messages`. Enforces one turn in flight (`plan_turn_in_flight`)
    /// and the pace limit (`plan_rate`); the D7 token and header checks stay in the model_router branch.
    pub async fn turn(&self, user_id: &str, vendor: Vendor, model: &str, messages: Vec<ChatMessage>) -> Result<TurnStream, ApiError> {
        self.check_offered(vendor)?;
        if let Some(bad) = messages.iter().find(|m| !matches!(m.role.as_str(), "system" | "user" | "assistant")) {
            return Err(ApiError::BadRequest(format!("a plan turn cannot carry a {} message", bad.role)));
        }
        let guard = self.begin_turn(user_id)?;
        let handle = self.ensure(user_id).await?;
        let id = format!("t_{}", uuid::Uuid::new_v4().simple());
        let frame = json!({
            "type": "turn",
            "id": id,
            "vendor": vendor.row_id(),
            "model": model,
            "messages": messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
        });
        let replies = self.sockets.request(&handle.runner_id, &id, &frame).ok_or_else(could_not_start)?;

        struct Turn {
            replies: mpsc::UnboundedReceiver<Value>,
            sockets: Arc<RunnerSockets>,
            db: PgPool,
            runner_id: String,
            vendor: Vendor,
            idle: Duration,
            finished: bool,
            _guard: TurnGuard,
        }
        let turn = Turn {
            replies,
            sockets: self.sockets.clone(),
            db: self.db.clone(),
            runner_id: handle.runner_id,
            vendor,
            idle: self.cfg.turn_idle_timeout,
            finished: false,
            _guard: guard,
        };
        let stream = futures::stream::unfold(turn, |mut turn| async move {
            if turn.finished {
                let _ = sqlx::query("UPDATE brain_runners SET last_used_at = now() WHERE id = $1")
                    .bind(&turn.runner_id)
                    .execute(&turn.db)
                    .await;
                return None;
            }
            let event = match tokio::time::timeout(turn.idle, turn.replies.recv()).await {
                Err(_) => {
                    // Silent for 60 s: close the socket and the turn.
                    turn.sockets.close(&turn.runner_id);
                    TurnEvent::Error { code: COULD_NOT_START, message: "Could not start. Try again".to_string() }
                }
                Ok(None) => TurnEvent::Error { code: COULD_NOT_START, message: "Could not start. Try again".to_string() },
                Ok(Some(frame)) => match frame.get("type").and_then(Value::as_str) {
                    Some("turn_delta") => TurnEvent::Delta(frame.get("text").and_then(Value::as_str).unwrap_or("").to_string()),
                    Some("turn_done") => {
                        let tokens = |k: &str| frame.get(k).and_then(Value::as_u64).map(|v| v.min(u64::from(u32::MAX)) as u32);
                        TurnEvent::Done { input_tokens: tokens("input_tokens"), output_tokens: tokens("output_tokens") }
                    }
                    Some("error") => {
                        let message = frame.get("message").and_then(Value::as_str).unwrap_or("").to_string();
                        match frame.get("code").and_then(Value::as_str) {
                            Some("login_expired") => {
                                let _ = set_vendor_word(&turn.db, &turn.runner_id, turn.vendor, "expired").await;
                                TurnEvent::Error { code: "login_expired", message }
                            }
                            Some("vendor_limited") => {
                                let _ = set_vendor_word(&turn.db, &turn.runner_id, turn.vendor, "limited").await;
                                TurnEvent::Error { code: "vendor_limited", message }
                            }
                            _ => TurnEvent::Error { code: COULD_NOT_START, message: "Could not start. Try again".to_string() },
                        }
                    }
                    // Unknown frame types are ignored; keep waiting.
                    _ => TurnEvent::Delta(String::new()),
                },
            };
            if !matches!(event, TurnEvent::Delta(_)) {
                turn.finished = true;
            }
            Some((event, turn))
        });
        // Drop the empty deltas that stand in for ignored frames.
        use futures::StreamExt;
        let stream = stream.filter(|event| std::future::ready(!matches!(event, TurnEvent::Delta(text) if text.is_empty())));
        Ok(Box::pin(stream))
    }
}

async fn set_vendor_word(db: &PgPool, runner_id: &str, vendor: Vendor, word: &str) -> Result<(), ApiError> {
    sqlx::query("UPDATE brain_runners SET vendors = jsonb_set(vendors, ARRAY[$2::text], to_jsonb($3::text)), updated_at = now() WHERE id = $1")
        .bind(runner_id)
        .bind(vendor.row_id())
        .bind(word)
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    /// The Incus edge: boxes and home volumes in memory. `start` hands the
    /// box's token to the fake runner, which dials the service like the real
    /// program dials the socket route.
    #[derive(Default)]
    struct FakeBackend {
        boxes: StdMutex<HashMap<String, (bool, String)>>, // name -> (running, token)
        volumes: StdMutex<HashSet<String>>,
        creates: AtomicU64,
        fail_create: std::sync::atomic::AtomicBool,
        started: StdMutex<Option<mpsc::UnboundedSender<String>>>,
    }

    #[async_trait]
    impl RunnerBackend for FakeBackend {
        async fn create(&self, spec: &RunnerSpec) -> Result<(), ProvisionError> {
            if self.fail_create.load(Ordering::SeqCst) {
                return Err(ProvisionError::Api { status: 500, message: "pool full".to_string() });
            }
            self.creates.fetch_add(1, Ordering::SeqCst);
            let token = spec.environment.iter().find(|(k, _)| k == "ALLTERNIT_RUNNER_TOKEN").unwrap().1.clone();
            let vendors = spec.environment.iter().find(|(k, _)| k == "ALLTERNIT_RUNNER_VENDORS").map(|(_, v)| v.clone());
            assert!(vendors.is_some_and(|v| !v.is_empty()), "the box must be told which vendors to enable");
            self.volumes.lock().unwrap().insert(spec.home_volume.clone());
            self.boxes.lock().unwrap().insert(spec.name.clone(), (false, token));
            Ok(())
        }
        async fn start(&self, name: &str) -> Result<(), ProvisionError> {
            let token = {
                let mut boxes = self.boxes.lock().unwrap();
                let entry = boxes.get_mut(name).ok_or_else(|| ProvisionError::NotFound(name.to_string()))?;
                entry.0 = true;
                entry.1.clone()
            };
            if let Some(tx) = self.started.lock().unwrap().as_ref() {
                let _ = tx.send(token);
            }
            Ok(())
        }
        async fn stop(&self, name: &str) -> Result<(), ProvisionError> {
            match self.boxes.lock().unwrap().get_mut(name) {
                Some(entry) => {
                    entry.0 = false;
                    Ok(())
                }
                None => Err(ProvisionError::NotFound(name.to_string())),
            }
        }
        async fn status(&self, name: &str) -> Result<BackendStatus, ProvisionError> {
            Ok(match self.boxes.lock().unwrap().get(name) {
                Some((true, _)) => BackendStatus::Running,
                Some((false, _)) => BackendStatus::Stopped,
                None => BackendStatus::NotFound,
            })
        }
        async fn destroy(&self, name: &str, _pool: &str, home_volume: &str) -> Result<(), ProvisionError> {
            self.boxes.lock().unwrap().remove(name);
            self.volumes.lock().unwrap().remove(home_volume);
            Ok(())
        }
    }

    impl FakeBackend {
        fn running(&self) -> usize {
            self.boxes.lock().unwrap().values().filter(|(running, _)| *running).count()
        }
    }

    fn test_config() -> RunnerConfig {
        RunnerConfig {
            enabled: true,
            budget_wait: Duration::from_millis(300),
            socket_wait: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
            turn_idle_timeout: Duration::from_secs(5),
            ..RunnerConfig::default()
        }
    }

    pub(crate) async fn runner_db() -> PgPool {
        let db = crate::routes::test_support::test_pool().await;
        sqlx::raw_sql(include_str!("../../migrations_pg/140_brain_runners.sql")).execute(&db).await.expect("140 applies");
        db
    }

    /// A fake runner program per started box: says hello, then answers
    /// login, logout and turn frames the way the real one does.
    fn spawn_fake_runners(service: Arc<BrainRunnerService>, backend: &FakeBackend) {
        let (tx, mut started) = mpsc::unbounded_channel::<String>();
        *backend.started.lock().unwrap() = Some(tx);
        tokio::spawn(async move {
            while let Some(token) = started.recv().await {
                let service = service.clone();
                tokio::spawn(async move {
                    let runner_id = service.authenticate_runner(&token).await.unwrap().expect("box token is known");
                    let mut conn = service.attach_runner(&runner_id);
                    let hello = json!({ "type": "hello", "runner": "0.1.0", "vendors": { "chatgpt": "signed_in", "claude": "signed_out" } });
                    service.runner_frame(&runner_id, conn.conn_id, &hello.to_string()).await;
                    while let Some(text) = conn.outgoing.recv().await {
                        let frame: Value = serde_json::from_str(&text).unwrap();
                        let id = frame["id"].clone();
                        let replies = match frame["type"].as_str().unwrap() {
                            "login_start" => vec![json!({ "type": "login_view", "id": id, "kind": "device_code", "code": "KQ7M-24TX",
                                "url": "https://auth.openai.com/codex/device", "url_with_code": null, "expires_at": "2026-11-14T18:30:00Z" })],
                            "login_status" => vec![json!({ "type": "login_status_result", "id": id, "status": "done" })],
                            "logout" => vec![json!({ "type": "logout_done", "id": id, "home_deleted": true })],
                            "turn" if frame["vendor"] == "claude" => {
                                vec![json!({ "type": "error", "id": id, "code": "login_expired", "message": "Sign-in expired" })]
                            }
                            "turn" => vec![
                                json!({ "type": "turn_delta", "id": id, "text": "Hi" }),
                                json!({ "type": "turn_delta", "id": id, "text": " there" }),
                                json!({ "type": "turn_done", "id": id, "input_tokens": 12, "output_tokens": 3 }),
                            ],
                            _ => vec![],
                        };
                        for reply in replies {
                            service.runner_frame(&runner_id, conn.conn_id, &reply.to_string()).await;
                        }
                    }
                });
            }
        });
    }

    fn service_with(db: PgPool, cfg: RunnerConfig) -> (Arc<BrainRunnerService>, Arc<FakeBackend>) {
        let backend = Arc::new(FakeBackend::default());
        let service = Arc::new(BrainRunnerService::new(db, backend.clone(), cfg));
        spawn_fake_runners(service.clone(), &backend);
        (service, backend)
    }

    async fn state_of(db: &PgPool, user: &str) -> Option<String> {
        sqlx::query_scalar("SELECT state FROM brain_runners WHERE user_id = $1").bind(user).fetch_optional(db).await.unwrap()
    }

    #[tokio::test]
    async fn brain_runner_create_wake_idle_stop_destroy() {
        let db = runner_db().await;
        let (service, backend) = service_with(db.clone(), test_config());

        let first = service.ensure("user_a").await.unwrap();
        assert_eq!(state_of(&db, "user_a").await.as_deref(), Some("awake"));
        assert_eq!(backend.running(), 1);
        // Only the token's hash is stored, and it is the one in the box.
        let (name, hash): (String, String) =
            sqlx::query_as("SELECT instance_name, token_hash FROM brain_runners WHERE user_id = 'user_a'").fetch_one(&db).await.unwrap();
        let token = backend.boxes.lock().unwrap()[&name].1.clone();
        assert_eq!(hash, sha256_hex(&token));
        assert_ne!(hash, token);
        // The hello's vendors became row state, without waking anything.
        let states = service.vendor_state("user_a").await.unwrap();
        assert!(states.contains(&(Vendor::Chatgpt, VendorState::On { found: None })));
        assert!(states.contains(&(Vendor::Claude, VendorState::Off)));

        // Not idle yet: the sweep leaves it.
        service.sweep_idle(Utc::now()).await.unwrap();
        assert_eq!(state_of(&db, "user_a").await.as_deref(), Some("awake"));
        // Idle past 5 minutes: stopped, and its RAM share is free.
        service.sweep_idle(Utc::now() + chrono::Duration::minutes(6)).await.unwrap();
        assert_eq!(state_of(&db, "user_a").await.as_deref(), Some("stopped"));
        assert_eq!(backend.running(), 0);

        // Wake starts the same box; nothing is created twice.
        let again = service.ensure("user_a").await.unwrap();
        assert_eq!(again.runner_id, first.runner_id);
        assert_eq!(backend.creates.load(Ordering::SeqCst), 1);
        assert_eq!(backend.running(), 1);

        service.accept_disclosure("user_a", Vendor::Chatgpt, DISCLOSURE_VERSION).await.unwrap();
        service.destroy("user_a").await.unwrap();
        assert_eq!(state_of(&db, "user_a").await, None);
        assert!(backend.boxes.lock().unwrap().is_empty());
        assert!(backend.volumes.lock().unwrap().is_empty(), "the home volume goes with the box");
        assert_eq!(
            service.accepted_version("user_a", Vendor::Chatgpt).await.unwrap().as_deref(),
            Some(DISCLOSURE_VERSION),
            "consent is kept until the account is erased"
        );

        // A box unused for 90 days is destroyed by the lifecycle sweep.
        service.ensure("user_old").await.unwrap();
        service.sweep_unused(Utc::now() + chrono::Duration::days(89)).await.unwrap();
        assert!(state_of(&db, "user_old").await.is_some());
        service.sweep_unused(Utc::now() + chrono::Duration::days(91)).await.unwrap();
        assert_eq!(state_of(&db, "user_old").await, None);
    }

    #[tokio::test]
    async fn brain_runner_sixth_concurrent_wake_gets_runner_busy() {
        let db = runner_db().await;
        let (service, backend) = service_with(db.clone(), test_config());
        assert_eq!(service.config().capacity(), 5, "4096 MB at 768 MB a box");

        let wakes = (1..=6).map(|i| {
            let service = service.clone();
            async move { service.ensure(&format!("user_{i}")).await }
        });
        let results = futures::future::join_all(wakes).await;
        let busy: Vec<&ApiError> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 5);
        assert_eq!(busy.len(), 1);
        assert_eq!(error_code(busy[0]), Some(RUNNER_BUSY));
        let response = error_response(busy[0]).unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(backend.running(), 5);

        // Room frees up when the others go idle; the waiting person gets in.
        let waiting = format!("user_{}", results.iter().position(|r| r.is_err()).unwrap() + 1);
        service.sweep_idle(Utc::now() + chrono::Duration::minutes(6)).await.unwrap();
        service.ensure(&waiting).await.unwrap();
        assert_eq!(backend.running(), 1);
    }

    #[tokio::test]
    async fn brain_runner_kill_switch_off_refuses_ensure() {
        let db = runner_db().await;
        let cfg = RunnerConfig { enabled: false, ..test_config() };
        let (service, backend) = service_with(db.clone(), cfg);

        let refused = service.ensure("user_a").await.unwrap_err();
        assert_eq!(error_code(&refused), Some(NOT_FOUND));
        assert_eq!(error_response(&refused).unwrap().status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(&service.start_login("user_a", Vendor::Claude).await.unwrap_err()), Some(NOT_FOUND));
        assert_eq!(backend.creates.load(Ordering::SeqCst), 0);
        assert_eq!(state_of(&db, "user_a").await, None);

        // A vendor off ALLTERNIT_RUNNER_VENDORS is refused the same way.
        let cfg = RunnerConfig { vendors: vec![Vendor::Chatgpt], ..test_config() };
        let (service, _) = service_with(db, cfg);
        assert_eq!(error_code(&service.start_login("user_b", Vendor::Kimi).await.unwrap_err()), Some(NOT_FOUND));
    }

    #[tokio::test]
    async fn brain_runner_failed_create_is_could_not_start_and_frees_the_budget() {
        let db = runner_db().await;
        let (service, backend) = service_with(db.clone(), test_config());
        backend.fail_create.store(true, Ordering::SeqCst);

        let error = service.ensure("user_a").await.unwrap_err();
        assert_eq!(error_code(&error), Some(COULD_NOT_START));
        assert_eq!(error_response(&error).unwrap().status(), StatusCode::BAD_GATEWAY);
        assert_eq!(state_of(&db, "user_a").await.as_deref(), Some("failed"));
        let used: i64 = sqlx::query_scalar("SELECT count(*) FROM brain_runners WHERE state IN ('creating', 'awake')")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(used, 0);

        // The next try makes the box (a new token, since the old one was never used).
        backend.fail_create.store(false, Ordering::SeqCst);
        service.ensure("user_a").await.unwrap();
        assert_eq!(state_of(&db, "user_a").await.as_deref(), Some("awake"));
    }

    #[tokio::test]
    async fn brain_runner_login_turn_and_logout_over_the_socket() {
        let db = runner_db().await;
        let cfg = RunnerConfig { pace_turns: 2, ..test_config() };
        let (service, backend) = service_with(db.clone(), cfg);

        let view = service.start_login("user_a", Vendor::Chatgpt).await.unwrap();
        assert_eq!(view.kind, "device_code");
        assert_eq!(view.code.as_deref(), Some("KQ7M-24TX"));
        assert_eq!(view.expires_at, Some("2026-11-14T18:30:00Z".parse().unwrap()));
        assert!(service.vendor_state("user_a").await.unwrap().contains(&(Vendor::Chatgpt, VendorState::Connecting)));
        assert_eq!(service.login_status("user_a", Vendor::Chatgpt).await.unwrap(), LoginStatus::Done);
        assert!(service.vendor_state("user_a").await.unwrap().contains(&(Vendor::Chatgpt, VendorState::On { found: None })));

        let messages = vec![ChatMessage { role: "user".to_string(), content: "Hello".to_string() }];
        let mut stream = service.turn("user_a", Vendor::Chatgpt, "gpt-5", messages.clone()).await.unwrap();
        // One turn in flight per account.
        let second = service.turn("user_a", Vendor::Chatgpt, "gpt-5", messages.clone()).await.err().unwrap();
        assert_eq!(error_code(&second), Some(PLAN_TURN_IN_FLIGHT));
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        assert_eq!(
            events,
            vec![
                TurnEvent::Delta("Hi".to_string()),
                TurnEvent::Delta(" there".to_string()),
                TurnEvent::Done { input_tokens: Some(12), output_tokens: Some(3) },
            ]
        );
        drop(stream);

        // A vendor whose sign-in lapsed: the runner's error becomes row state.
        let expired: Vec<TurnEvent> = service.turn("user_a", Vendor::Claude, "sonnet", messages.clone()).await.unwrap().collect().await;
        assert_eq!(expired, vec![TurnEvent::Error { code: "login_expired", message: "Sign-in expired".to_string() }]);
        assert!(service.vendor_state("user_a").await.unwrap().contains(&(Vendor::Claude, VendorState::Expired)));
        // Pace limit (2 turns per window in this config).
        let paced = service.turn("user_a", Vendor::Chatgpt, "gpt-5", messages.clone()).await.err().unwrap();
        assert_eq!(error_code(&paced), Some(PLAN_RATE));
        // Tool messages never reach a plan.
        let tool = vec![ChatMessage { role: "tool".to_string(), content: "{}".to_string() }];
        assert!(matches!(service.turn("user_b", Vendor::Chatgpt, "gpt-5", tool).await, Err(ApiError::BadRequest(_))));

        // Logout of one vendor keeps the box while another is still connected...
        service.logout("user_a", Vendor::Chatgpt).await.unwrap();
        assert_eq!(state_of(&db, "user_a").await.as_deref(), Some("awake"));
        assert!(service.vendor_state("user_a").await.unwrap().contains(&(Vendor::Chatgpt, VendorState::Off)));
        // ...and the last one destroys it.
        service.logout("user_a", Vendor::Claude).await.unwrap();
        assert_eq!(state_of(&db, "user_a").await, None);
        assert!(backend.boxes.lock().unwrap().is_empty());
        // Logout with no box is a no-op, even with the kill switch off.
        service.logout("user_a", Vendor::Claude).await.unwrap();
    }

    #[tokio::test]
    async fn brain_runner_disclosure_text_and_acceptance() {
        let db = runner_db().await;
        let (service, _) = service_with(db, test_config());
        let text = service.disclosure_for(Vendor::Chatgpt).await;
        assert_eq!(text.title, "Before you connect your ChatGPT plan");
        assert_eq!(text.body.len(), 4);
        assert!(text.body[1].starts_with("ChatGPT's terms may not allow"));
        assert_eq!((text.accept_label, text.decline_label), ("I understand, connect", "Use a key instead"));

        assert_eq!(service.accepted_version("user_a", Vendor::Grok).await.unwrap(), None);
        assert!(matches!(service.accept_disclosure("user_a", Vendor::Grok, "2026-01-01.1").await, Err(ApiError::BadRequest(_))));
        let first = service.accept_disclosure("user_a", Vendor::Grok, DISCLOSURE_VERSION).await.unwrap();
        let again = service.accept_disclosure("user_a", Vendor::Grok, DISCLOSURE_VERSION).await.unwrap();
        assert_eq!(first, again, "accepting twice keeps the first time");
        assert_eq!(service.accepted_version("user_a", Vendor::Grok).await.unwrap().as_deref(), Some(DISCLOSURE_VERSION));
        assert_eq!(service.accepted_version("user_a", Vendor::Kimi).await.unwrap(), None);
    }

    /// Exposed to account_erase's test: a service on `db` with a fake box.
    pub(crate) async fn service_with_box(db: PgPool, user_id: &str) -> (Arc<BrainRunnerService>, Arc<FakeBackendHandle>) {
        let (service, backend) = service_with(db, test_config());
        service.ensure(user_id).await.unwrap();
        (service, Arc::new(FakeBackendHandle(backend)))
    }

    pub(crate) struct FakeBackendHandle(Arc<FakeBackend>);

    impl FakeBackendHandle {
        pub(crate) fn boxes(&self) -> usize {
            self.0.boxes.lock().unwrap().len()
        }
    }
}

#[cfg(test)]
pub(crate) use tests::{runner_db as test_runner_db, service_with_box as test_service_with_box};
