//! A runtime's own web app, started on this computer for its owner
//! (Harness & Bots 2.0 D8, CONTRACT-harness-bots-2.md "Routes").
//!
//! - `POST /api/v1/runtimes/:id/live` `{sessionId?}` → `{url, resourceUri, ...}`.
//!   Looks the runtime up (id or alias) in the embedded registry; 404 when it
//!   has no `liveUi`, 409 when its CLI is not installed. Starts the app bound
//!   to 127.0.0.1 only, in the session's folder (else the home folder), waits
//!   up to 20 s for `liveUi.health` to answer (else kills it: 504), and
//!   answers `http://127.0.0.1:<port>`. One app per (user, runtime, folder):
//!   a second POST returns the running one.
//! - `GET /api/v1/runtimes/:id/live?sessionId=` → status; refreshes the
//!   heartbeat.
//! - `DELETE /api/v1/runtimes/:id/live?sessionId=` → stops it: SIGTERM to its
//!   process group, SIGKILL after 3 s.
//!
//! Owner only (`cli_catalog::caller_is_owner`: the Desktop sidecar or a
//! self-hosted install, with the Desktop token or a verified session). The
//! app never opens a browser window: `BROWSER` points at a no-op and a
//! private folder of no-op `open` / `xdg-open` / `*-browser` commands goes
//! first on its PATH (the `open` npm package and xdg-open both resolve
//! through PATH).
//!
//! An app stops when: DELETE; the session that started it is deleted
//! (`agent_session_routes::delete_session` → [`release_session`]); 30 min pass
//! with no POST or GET (the sweeper); or the gateway shuts down
//! ([`shutdown`]). On Linux the child also gets PDEATHSIG, so a crashed
//! gateway leaves no app behind.
//!
//! How each runtime's app is started (flags checked against the vendor docs):
//! - `opencode`: `opencode serve --hostname 127.0.0.1 --port <p>` — the web UI
//!   "is served from the same server that powers the TUI" and `serve` accepts
//!   `--hostname`/`--port` (opencode.ai/docs/web; same flags in our fork,
//!   cmd/gizzi-code/src/cli/network.ts). The registry's `opencode web` calls
//!   `open(url)` with no opt-out flag (cmd/gizzi-code/src/cli/commands/web.ts),
//!   so the headless `serve` is used.
//! - `openclaw`: `openclaw gateway run --port <p> --bind loopback` — the
//!   Control UI is served by the Gateway at `/` on its port
//!   (vendor/openclaw/docs/web/control-ui.md, docs/cli/gateway.md). The
//!   registry's `openclaw dashboard` only opens the UI of an already running
//!   Gateway (docs/cli/dashboard.md). If the user's own Gateway already answers
//!   on the registry port it is returned as-is (`external: true`) and never
//!   stopped by us: a second Gateway on the same profile would double-connect
//!   their channels.
//! - `aider`: `aider --browser` with `STREAMLIT_SERVER_PORT`,
//!   `STREAMLIT_SERVER_ADDRESS=127.0.0.1`, `STREAMLIT_SERVER_HEADLESS=true`
//!   (Streamlit config by env var, docs.streamlit.io "Configuration options";
//!   `server.headless`: "If false, will attempt to open a browser window").
//! - `openhands`: not started. `openhands serve` runs the GUI in Docker on a
//!   fixed port 3000 ("the port is not configurable via CLI",
//!   docs.openhands.dev …/cli/gui-server), so it cannot be bound to this
//!   computer only. 501 `live_unsupported`; clients show the look pack.
//! - Any other runtime that gains `liveUi`: its `argv` as-is on its registry
//!   port; 409 `port_in_use` when that port is taken.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::sync::{watch, Mutex};

use crate::cli_catalog::{runtime_registry, RuntimeEntry, RuntimeRegistry};
use crate::AppState;

/// How long the app has to answer its health path after it starts.
pub(crate) const HEALTH_TIMEOUT: Duration = Duration::from_secs(20);
/// SIGTERM, then this long before SIGKILL.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(3);
/// An app nobody has asked about (POST or GET) for this long is stopped.
pub(crate) const HEARTBEAT_TTL: Duration = Duration::from_secs(30 * 60);
const SWEEP_EVERY: Duration = Duration::from_secs(60);
const STDERR_TAIL: usize = 2048;
const LOOPBACK: &str = "127.0.0.1";

/// Mounted on the protected v1 router (`provider_routes` → `cli_tools` →
/// `cli_catalog::router`), so the full path is `/api/v1/runtimes/:id/live`.
/// No other route uses `/runtimes/:id…` (the public `GET /api/v1/runtimes`
/// is an exact path).
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/runtimes/:id/live",
        post(start_live).get(live_status).delete(stop_live),
    )
}

// ─── Registry lookup ─────────────────────────────────────────────────────────

/// A registry entry's `liveUi` (`{argv, port, health, label}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveUi {
    pub argv: Vec<String>,
    pub port: u16,
    pub health: String,
    pub label: String,
}

pub(crate) fn live_ui_of(entry: &RuntimeEntry) -> Option<LiveUi> {
    let raw = entry.live_ui.as_ref()?;
    let argv: Vec<String> = raw
        .get("argv")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if argv.is_empty() {
        return None;
    }
    let port = u16::try_from(raw.get("port")?.as_u64()?).ok().filter(|p| *p > 0)?;
    let health = raw
        .get("health")
        .and_then(Value::as_str)
        .filter(|h| h.starts_with('/'))
        .unwrap_or("/")
        .to_string();
    let label = raw
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or(entry.name.as_str())
        .to_string();
    Some(LiveUi { argv, port, health, label })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiveError {
    OwnerOnly,
    UnknownRuntime,
    NoLiveUi,
    NotInstalled(String),
    Unsupported(&'static str),
    PortInUse(u16),
    NoFreePort,
    SpawnFailed(String),
    Exited(String),
    Timeout,
}

impl LiveError {
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            LiveError::OwnerOnly => StatusCode::FORBIDDEN,
            LiveError::UnknownRuntime | LiveError::NoLiveUi => StatusCode::NOT_FOUND,
            LiveError::NotInstalled(_) | LiveError::PortInUse(_) => StatusCode::CONFLICT,
            LiveError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
            LiveError::NoFreePort | LiveError::SpawnFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
            LiveError::Exited(_) => StatusCode::BAD_GATEWAY,
            LiveError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            LiveError::OwnerOnly => "owner_only",
            LiveError::UnknownRuntime => "unknown_runtime",
            LiveError::NoLiveUi => "no_live_ui",
            LiveError::NotInstalled(_) => "not_installed",
            LiveError::Unsupported(_) => "live_unsupported",
            LiveError::PortInUse(_) => "port_in_use",
            LiveError::NoFreePort => "no_free_port",
            LiveError::SpawnFailed(_) => "spawn_failed",
            LiveError::Exited(_) => "live_exited",
            LiveError::Timeout => "live_timeout",
        }
    }

    fn message(&self) -> String {
        match self {
            LiveError::OwnerOnly => "Only this computer's owner can start a runtime's web app".into(),
            LiveError::UnknownRuntime => "No runtime with that id".into(),
            LiveError::NoLiveUi => "This runtime has no web app of its own".into(),
            LiveError::NotInstalled(bin) => format!("{bin} is not installed on this computer"),
            LiveError::Unsupported(why) => (*why).to_string(),
            LiveError::PortInUse(port) => format!("Port {port} on this computer is already in use"),
            LiveError::NoFreePort => "No free local port".into(),
            LiveError::SpawnFailed(e) => format!("Could not start the web app: {e}"),
            LiveError::Exited(tail) if tail.is_empty() => "The web app stopped while starting".into(),
            LiveError::Exited(tail) => format!("The web app stopped while starting: {tail}"),
            LiveError::Timeout => format!(
                "The web app did not answer within {} s and was stopped",
                HEALTH_TIMEOUT.as_secs()
            ),
        }
    }
}

impl IntoResponse for LiveError {
    fn into_response(self) -> Response {
        (self.status(), Json(json!({ "error": self.code(), "message": self.message() }))).into_response()
    }
}

/// The runtime (id or alias) and its `liveUi`.
pub(crate) fn lookup<'a>(registry: &'a RuntimeRegistry, id: &str) -> Result<(&'a RuntimeEntry, LiveUi), LiveError> {
    let entry = registry.get(id.trim()).ok_or(LiveError::UnknownRuntime)?;
    let live = live_ui_of(entry).ok_or(LiveError::NoLiveUi)?;
    Ok((entry, live))
}

/// The CLI to run, from the same detection the catalog and provider rows use
/// (`cli_tools::resolve_binary`: PATH, then the harness `bin/`).
pub(crate) fn require_installed(bin: &str, resolved: Option<PathBuf>) -> Result<PathBuf, LiveError> {
    resolved.ok_or_else(|| LiveError::NotInstalled(bin.to_string()))
}

// ─── Launch plans ────────────────────────────────────────────────────────────

/// How the port is chosen for a runtime's app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortPolicy {
    /// The CLI takes a port: the registry port when free, else any free port.
    Flexible,
    /// Like `Flexible`, but an app already answering on the registry port is
    /// the user's own instance and is returned instead of starting another.
    ReuseRunning,
    /// No known port flag: the registry port, or 409 when it is taken.
    Fixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchPlan {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub(crate) fn port_policy(runtime_id: &str) -> Result<PortPolicy, LiveError> {
    match runtime_id {
        "opencode" | "aider" => Ok(PortPolicy::Flexible),
        "openclaw" => Ok(PortPolicy::ReuseRunning),
        "openhands" => Err(LiveError::Unsupported(
            "OpenHands runs its web app in Docker on a fixed port that cannot be kept to this computer only",
        )),
        _ => Ok(PortPolicy::Fixed),
    }
}

/// The argv and extra env for `runtime_id`'s app on `port`. Every plan binds
/// 127.0.0.1 only and sets `BROWSER` to a no-op.
pub(crate) fn launch_plan(runtime_id: &str, live: &LiveUi, port: u16) -> Result<LaunchPlan, LiveError> {
    let p = port.to_string();
    let bin = live.argv[0].clone();
    let mut env = vec![("BROWSER".to_string(), no_browser_command())];
    let argv = match runtime_id {
        "opencode" => vec![bin, "serve".into(), "--hostname".into(), LOOPBACK.into(), "--port".into(), p],
        "openclaw" => vec![
            bin,
            "gateway".into(),
            "run".into(),
            "--port".into(),
            p,
            "--bind".into(),
            "loopback".into(),
        ],
        "aider" => {
            env.push(("STREAMLIT_SERVER_PORT".into(), p));
            env.push(("STREAMLIT_SERVER_ADDRESS".into(), LOOPBACK.into()));
            env.push(("STREAMLIT_SERVER_HEADLESS".into(), "true".into()));
            env.push(("STREAMLIT_BROWSER_GATHER_USAGE_STATS".into(), "false".into()));
            live.argv.clone()
        }
        other => {
            port_policy(other)?;
            live.argv.clone()
        }
    };
    Ok(LaunchPlan { argv, env })
}

fn no_browser_command() -> String {
    if cfg!(windows) {
        "none".to_string()
    } else if FsPath::new("/usr/bin/true").exists() {
        "/usr/bin/true".to_string()
    } else {
        "/bin/true".to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortChoice {
    /// Start the app on this port.
    Start(u16),
    /// The user's own app already answers here; return it, start nothing.
    Existing(u16),
}

/// Pick the port. `is_free` binds 127.0.0.1:port, `answers` asks the health
/// path, `any_free` binds port 0.
pub(crate) fn choose_port(
    policy: PortPolicy,
    preferred: u16,
    is_free: impl Fn(u16) -> bool,
    answers: impl Fn(u16) -> bool,
    any_free: impl FnOnce() -> Option<u16>,
) -> Result<PortChoice, LiveError> {
    if is_free(preferred) {
        return Ok(PortChoice::Start(preferred));
    }
    match policy {
        PortPolicy::Fixed => Err(LiveError::PortInUse(preferred)),
        PortPolicy::ReuseRunning if answers(preferred) => Ok(PortChoice::Existing(preferred)),
        PortPolicy::ReuseRunning | PortPolicy::Flexible => {
            any_free().map(PortChoice::Start).ok_or(LiveError::NoFreePort)
        }
    }
}

fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind((LOOPBACK, port)).is_ok()
}

fn any_free_port() -> Option<u16> {
    std::net::TcpListener::bind((LOOPBACK, 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

pub(crate) fn live_url(port: u16) -> String {
    format!("http://{LOOPBACK}:{port}")
}

pub(crate) fn resource_uri(runtime_id: &str, port: u16) -> String {
    format!(
        "ui://allternit/live-app?runtime={}&port={port}",
        urlencoding::encode(runtime_id)
    )
}

/// The session's workspace folder when it has one that exists, else home.
pub(crate) fn resolve_cwd(session_dir: Option<&str>, home: Option<PathBuf>) -> PathBuf {
    session_dir
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .or(home)
        .unwrap_or_else(std::env::temp_dir)
}

// ─── Running apps ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LiveKey {
    pub user: String,
    pub runtime: String,
    pub cwd: PathBuf,
}

#[derive(Debug)]
pub(crate) struct LiveApp {
    /// Process group leader; `None` for the user's own app we only point at.
    pub pid: Option<u32>,
    pub port: u16,
    pub label: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_seen: Instant,
    /// Sessions that asked for it; it stops when the last one ends.
    pub sessions: HashSet<String>,
    /// Flips to true when the child exits.
    pub exited: watch::Receiver<bool>,
}

impl LiveApp {
    pub(crate) fn alive(&self) -> bool {
        !*self.exited.borrow()
    }

    fn body(&self, key: &LiveKey, reused: bool) -> Value {
        json!({
            "running": true,
            "runtime": key.runtime,
            "url": live_url(self.port),
            "resourceUri": resource_uri(&key.runtime, self.port),
            "port": self.port,
            "label": self.label,
            "external": self.pid.is_none(),
            "reused": reused,
            "startedAt": self.started_at.to_rfc3339(),
            "stopsAfterIdleSeconds": HEARTBEAT_TTL.as_secs(),
        })
    }
}

/// The running apps, keyed by (user, runtime, folder). Pure bookkeeping; the
/// processes are started and stopped by the handlers below.
#[derive(Debug, Default)]
pub(crate) struct LiveTable {
    apps: HashMap<LiveKey, LiveApp>,
}

impl LiveTable {
    /// The running app for `key`, dropping a dead entry. Refreshes its
    /// heartbeat and records `session`.
    pub(crate) fn reuse(&mut self, key: &LiveKey, session: Option<&str>, now: Instant) -> Option<&LiveApp> {
        if self.apps.get(key).is_some_and(|app| !app.alive()) {
            self.apps.remove(key);
        }
        let app = self.apps.get_mut(key)?;
        app.last_seen = now;
        if let Some(sid) = session {
            app.sessions.insert(sid.to_string());
        }
        Some(&*app)
    }

    pub(crate) fn insert(&mut self, key: LiveKey, app: LiveApp) {
        self.apps.insert(key, app);
    }

    pub(crate) fn remove(&mut self, key: &LiveKey) -> Option<LiveApp> {
        self.apps.remove(key)
    }

    /// Drop `key` only if it is still the process `pid` (its waiter calls
    /// this after the child exits).
    pub(crate) fn remove_pid(&mut self, key: &LiveKey, pid: u32) {
        if self.apps.get(key).is_some_and(|a| a.pid == Some(pid)) {
            self.apps.remove(key);
        }
    }

    /// Apps nobody asked about for longer than `ttl`.
    pub(crate) fn expired(&self, now: Instant, ttl: Duration) -> Vec<LiveKey> {
        self.apps
            .iter()
            .filter(|(_, app)| heartbeat_expired(app.last_seen, now, ttl))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// `session` ended: forget it everywhere; the apps whose last session it
    /// was are returned to be stopped (apps started with no session are left
    /// to the heartbeat).
    pub(crate) fn release_session(&mut self, session: &str) -> Vec<LiveKey> {
        self.apps
            .iter_mut()
            .filter_map(|(key, app)| (app.sessions.remove(session) && app.sessions.is_empty()).then(|| key.clone()))
            .collect()
    }

    pub(crate) fn keys(&self) -> Vec<LiveKey> {
        self.apps.keys().cloned().collect()
    }
}

pub(crate) fn heartbeat_expired(last_seen: Instant, now: Instant, ttl: Duration) -> bool {
    now.saturating_duration_since(last_seen) > ttl
}

static TABLE: Lazy<Mutex<LiveTable>> = Lazy::new(|| Mutex::new(LiveTable::default()));
/// Serializes start and stop, so two POSTs for one key start one app.
static OP_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static SWEEPER: std::sync::OnceLock<()> = std::sync::OnceLock::new();

// ─── Handlers ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveParams {
    #[serde(default, alias = "session_id")]
    pub session_id: Option<String>,
}

fn user_key(headers: &HeaderMap) -> String {
    crate::auth::get_user(headers)
        .map(|u| u.user_id)
        .unwrap_or_else(|| "owner".to_string())
}

fn session_dir(state: &AppState, session_id: Option<&str>) -> Option<String> {
    let sid = session_id?.trim();
    if sid.is_empty() {
        return None;
    }
    state
        .db
        .get_session_metadata(sid)
        .ok()
        .flatten()
        .and_then(|bag| bag["directory"].as_str().map(str::to_owned))
}

async fn key_for(state: &AppState, headers: &HeaderMap, runtime_id: &str, params: &LiveParams) -> LiveKey {
    let dir = session_dir(state, params.session_id.as_deref());
    let cwd = resolve_cwd(dir.as_deref(), dirs::home_dir());
    let cwd = tokio::fs::canonicalize(&cwd).await.unwrap_or(cwd);
    LiveKey { user: user_key(headers), runtime: runtime_id.to_string(), cwd }
}

async fn start_live(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<LiveParams>>,
) -> Response {
    let params = body.map(|Json(b)| b).unwrap_or_default();
    match start(&state, &headers, &id, &params).await {
        Ok(body) => Json(body).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn start(state: &AppState, headers: &HeaderMap, id: &str, params: &LiveParams) -> Result<Value, LiveError> {
    if !crate::cli_catalog::caller_is_owner(state, headers).await {
        return Err(LiveError::OwnerOnly);
    }
    let (entry, live) = lookup(runtime_registry(), id)?;
    let policy = port_policy(&entry.id)?;
    let key = key_for(state, headers, &entry.id, params).await;
    let session = params.session_id.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let _op = OP_LOCK.lock().await;
    let reused = TABLE
        .lock()
        .await
        .reuse(&key, session, Instant::now())
        .map(|app| (app.body(&key, true), app.pid, app.port));
    if let Some((body, pid, port)) = reused {
        // Ours is alive (its waiter drops it on exit); the user's own app is
        // asked again, since it can stop without us knowing.
        if pid.is_some() || answers(&format!("{}{}", live_url(port), live.health)).await {
            return Ok(body);
        }
        TABLE.lock().await.remove(&key);
    }

    // 409 when the CLI is missing, from the catalog's own detection.
    let bin = entry
        .tool
        .as_deref()
        .and_then(crate::cli_catalog::find)
        .map(|tool| tool.bin.clone())
        .unwrap_or_else(|| live.argv[0].clone());
    let lookup_bin = bin.clone();
    let resolved = tokio::task::spawn_blocking(move || crate::cli_tools::resolve_binary(&lookup_bin))
        .await
        .ok()
        .flatten();
    let program = require_installed(&bin, resolved)?;

    let preferred = live.port;
    let preferred_free = port_is_free(preferred);
    let preferred_answers = !preferred_free
        && policy == PortPolicy::ReuseRunning
        && answers(&format!("{}{}", live_url(preferred), live.health)).await;
    let choice = choose_port(policy, preferred, |_| preferred_free, |_| preferred_answers, any_free_port)?;

    let port = match choice {
        PortChoice::Existing(port) => {
            let (_tx, exited) = watch::channel(false);
            let app = LiveApp {
                pid: None,
                port,
                label: live.label.clone(),
                started_at: chrono::Utc::now(),
                last_seen: Instant::now(),
                sessions: session.map(|s| HashSet::from([s.to_string()])).unwrap_or_default(),
                exited,
            };
            let body = app.body(&key, true);
            TABLE.lock().await.insert(key, app);
            return Ok(body);
        }
        PortChoice::Start(port) => port,
    };

    let mut plan = launch_plan(&entry.id, &live, port)?;
    plan.argv[0] = program.to_string_lossy().into_owned();
    let app = spawn_app(&key, &plan, &live, port, session).await?;
    let body = app.body(&key, false);
    TABLE.lock().await.insert(key, app);
    ensure_sweeper();
    Ok(body)
}

/// Start the process in its own group, keep a stderr tail, and wait for the
/// health path. On timeout or early exit the group is killed.
async fn spawn_app(
    key: &LiveKey,
    plan: &LaunchPlan,
    live: &LiveUi,
    port: u16,
    session: Option<&str>,
) -> Result<LiveApp, LiveError> {
    let mut cmd = tokio::process::Command::new(&plan.argv[0]);
    cmd.args(&plan.argv[1..])
        .current_dir(&key.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    for (name, value) in &plan.env {
        cmd.env(name, value);
    }
    if let Some(path) = path_without_browsers() {
        cmd.env("PATH", path);
    }
    crate::meeting_bot_host::own_group(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| LiveError::SpawnFailed(e.to_string()))?;
    let Some(pid) = child.id() else {
        return Err(LiveError::SpawnFailed("no process id".into()));
    };

    let tail = Arc::new(std::sync::Mutex::new(VecDeque::<u8>::with_capacity(STDERR_TAIL)));
    if let Some(mut stderr) = child.stderr.take() {
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut t) = tail.lock() {
                    t.extend(&buf[..n]);
                    while t.len() > STDERR_TAIL {
                        t.pop_front();
                    }
                }
            }
        });
    }

    let (exit_tx, exited) = watch::channel(false);
    {
        let key = key.clone();
        tokio::spawn(async move {
            let _ = child.wait().await;
            // Anything the app left in its group.
            crate::meeting_bot_host::kill_group(pid, true);
            let _ = exit_tx.send(true);
            TABLE.lock().await.remove_pid(&key, pid);
            tracing::info!(runtime = %key.runtime, pid, "live app exited");
        });
    }

    let url = format!("{}{}", live_url(port), live.health);
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    let mut exit_rx = exited.clone();
    loop {
        if *exit_rx.borrow() {
            let text = tail
                .lock()
                .map(|t| String::from_utf8_lossy(&t.iter().copied().collect::<Vec<u8>>()).into_owned())
                .unwrap_or_default();
            return Err(LiveError::Exited(crate::cli_tools::strip_ansi(text.trim())));
        }
        if answers(&url).await {
            break;
        }
        if Instant::now() >= deadline {
            stop_pid(pid, exited.clone()).await;
            return Err(LiveError::Timeout);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            _ = exit_rx.changed() => {}
        }
    }
    tracing::info!(runtime = %key.runtime, pid, port, cwd = %key.cwd.display(), "live app started");
    Ok(LiveApp {
        pid: Some(pid),
        port,
        label: live.label.clone(),
        started_at: chrono::Utc::now(),
        last_seen: Instant::now(),
        sessions: session.map(|s| HashSet::from([s.to_string()])).unwrap_or_default(),
        exited,
    })
}

/// Any HTTP answer (below 500) from the app's health path.
async fn answers(url: &str) -> bool {
    static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    });
    CLIENT
        .get(url)
        .send()
        .await
        .is_ok_and(|r| r.status().as_u16() < 500)
}

/// SIGTERM the group, SIGKILL after [`STOP_GRACE`] if it is still there.
async fn stop_pid(pid: u32, mut exited: watch::Receiver<bool>) {
    crate::meeting_bot_host::kill_group(pid, false);
    let gone = tokio::time::timeout(STOP_GRACE, async {
        while !*exited.borrow() {
            if exited.changed().await.is_err() {
                break;
            }
        }
    })
    .await
    .is_ok();
    if !gone {
        crate::meeting_bot_host::kill_group(pid, true);
    }
}

/// Stop and forget `keys`. The user's own apps (no pid) are only forgotten.
async fn stop_keys(keys: Vec<LiveKey>) -> usize {
    let mut apps = Vec::new();
    {
        let mut table = TABLE.lock().await;
        for key in keys {
            if let Some(app) = table.remove(&key) {
                apps.push((key, app));
            }
        }
    }
    let count = apps.len();
    futures::future::join_all(apps.into_iter().map(|(key, app)| async move {
        if let Some(pid) = app.pid {
            stop_pid(pid, app.exited.clone()).await;
            tracing::info!(runtime = %key.runtime, pid, "live app stopped");
        }
    }))
    .await;
    count
}

async fn live_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<LiveParams>,
) -> Response {
    if !crate::cli_catalog::caller_is_owner(&state, &headers).await {
        return LiveError::OwnerOnly.into_response();
    }
    let (entry, _) = match lookup(runtime_registry(), &id) {
        Ok(found) => found,
        Err(error) => return error.into_response(),
    };
    let key = key_for(&state, &headers, &entry.id, &params).await;
    let session = params.session_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let mut table = TABLE.lock().await;
    match table.reuse(&key, session, Instant::now()) {
        Some(app) => Json(app.body(&key, true)).into_response(),
        None => Json(json!({ "running": false, "runtime": entry.id })).into_response(),
    }
}

async fn stop_live(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<LiveParams>,
) -> Response {
    if !crate::cli_catalog::caller_is_owner(&state, &headers).await {
        return LiveError::OwnerOnly.into_response();
    }
    let (entry, _) = match lookup(runtime_registry(), &id) {
        Ok(found) => found,
        Err(error) => return error.into_response(),
    };
    let key = key_for(&state, &headers, &entry.id, &params).await;
    let _op = OP_LOCK.lock().await;
    let stopped = stop_keys(vec![key]).await > 0;
    Json(json!({ "stopped": stopped, "runtime": entry.id })).into_response()
}

// ─── Lifecycle hooks ─────────────────────────────────────────────────────────

/// A session was deleted (`agent_session_routes::delete_session`): stop the
/// apps it was the last session of.
pub async fn release_session(session_id: &str) {
    let keys = TABLE.lock().await.release_session(session_id);
    if !keys.is_empty() {
        let _op = OP_LOCK.lock().await;
        stop_keys(keys).await;
    }
}

/// allternit-api is shutting down: stop every app it started.
pub async fn shutdown() {
    let keys = TABLE.lock().await.keys();
    stop_keys(keys).await;
}

/// Once per process: stop apps with no heartbeat for [`HEARTBEAT_TTL`].
fn ensure_sweeper() {
    SWEEPER.get_or_init(|| {
        tokio::spawn(async {
            let mut interval = tokio::time::interval(SWEEP_EVERY);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let keys = TABLE.lock().await.expired(Instant::now(), HEARTBEAT_TTL);
                if !keys.is_empty() {
                    let _op = OP_LOCK.lock().await;
                    let n = stop_keys(keys).await;
                    tracing::info!(stopped = n, "live apps stopped after no heartbeat");
                }
            }
        });
    });
}

// ─── No browser windows ──────────────────────────────────────────────────────

#[cfg(unix)]
const BROWSER_COMMANDS: &[&str] = &[
    "open",
    "xdg-open",
    "sensible-browser",
    "x-www-browser",
    "www-browser",
    "gnome-open",
    "kde-open",
];

/// PATH with a private folder of no-op browser openers first (created once,
/// mode 0700, under `~/.allternit/live-app/no-browser`). `None` on Windows or
/// when the folder cannot be made.
fn path_without_browsers() -> Option<std::ffi::OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        static DIR: Lazy<Option<PathBuf>> = Lazy::new(|| {
            let dir = dirs::home_dir()?.join(".allternit").join("live-app").join("no-browser");
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
            for name in BROWSER_COMMANDS {
                let path = dir.join(name);
                std::fs::write(&path, "#!/bin/sh\nexit 0\n").ok()?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).ok()?;
            }
            Some(dir)
        });
        let dir = DIR.clone()?;
        let rest = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&rest))).ok()
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> &'static RuntimeRegistry {
        runtime_registry()
    }

    fn app(pid: Option<u32>, last_seen: Instant, sessions: &[&str]) -> (LiveApp, watch::Sender<bool>) {
        let (tx, exited) = watch::channel(false);
        (
            LiveApp {
                pid,
                port: 4096,
                label: "OpenCode web app".into(),
                started_at: chrono::Utc::now(),
                last_seen,
                sessions: sessions.iter().map(|s| s.to_string()).collect(),
                exited,
            },
            tx,
        )
    }

    fn key(runtime: &str, cwd: &str) -> LiveKey {
        LiveKey { user: "user_1".into(), runtime: runtime.into(), cwd: PathBuf::from(cwd) }
    }

    #[test]
    fn live_lookup_finds_live_ui_runtimes_and_404s_the_rest() {
        let (entry, live) = lookup(registry(), "opencode").expect("opencode has liveUi");
        assert_eq!(entry.id, "opencode");
        assert_eq!(live.port, 4096);
        assert_eq!(live.health, "/");
        assert_eq!(live.argv[0], "opencode");
        assert!(lookup(registry(), "OpenClaw").is_ok(), "ids are case-insensitive");
        assert_eq!(lookup(registry(), "claude").unwrap_err(), LiveError::NoLiveUi);
        assert_eq!(lookup(registry(), "no-such-runtime").unwrap_err(), LiveError::UnknownRuntime);
        assert_eq!(LiveError::NoLiveUi.status(), StatusCode::NOT_FOUND);
        assert_eq!(LiveError::UnknownRuntime.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn live_missing_cli_is_409_not_installed() {
        let err = require_installed("opencode", None).unwrap_err();
        assert_eq!(err, LiveError::NotInstalled("opencode".into()));
        assert_eq!(err.status(), StatusCode::CONFLICT);
        assert_eq!(
            require_installed("opencode", Some(PathBuf::from("/opt/bin/opencode"))).unwrap(),
            PathBuf::from("/opt/bin/opencode")
        );
        assert_eq!(LiveError::Timeout.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(LiveError::OwnerOnly.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn live_plans_bind_loopback_and_never_open_a_browser() {
        let (_, opencode) = lookup(registry(), "opencode").unwrap();
        let plan = launch_plan("opencode", &opencode, 5001).unwrap();
        assert_eq!(plan.argv, ["opencode", "serve", "--hostname", "127.0.0.1", "--port", "5001"]);
        assert!(plan.env.iter().any(|(k, _)| k == "BROWSER"));

        let (_, openclaw) = lookup(registry(), "openclaw").unwrap();
        let plan = launch_plan("openclaw", &openclaw, 18789).unwrap();
        assert_eq!(plan.argv, ["openclaw", "gateway", "run", "--port", "18789", "--bind", "loopback"]);

        let (_, aider) = lookup(registry(), "aider").unwrap();
        let plan = launch_plan("aider", &aider, 8600).unwrap();
        assert_eq!(plan.argv, ["aider", "--browser"]);
        let env: HashMap<_, _> = plan.env.into_iter().collect();
        assert_eq!(env["STREAMLIT_SERVER_PORT"], "8600");
        assert_eq!(env["STREAMLIT_SERVER_ADDRESS"], "127.0.0.1");
        assert_eq!(env["STREAMLIT_SERVER_HEADLESS"], "true");
        assert!(env.contains_key("BROWSER"));

        let (_, openhands) = lookup(registry(), "openhands").unwrap();
        let err = launch_plan("openhands", &openhands, 8000).unwrap_err();
        assert_eq!(err.status(), StatusCode::NOT_IMPLEMENTED);
        assert!(port_policy("openhands").is_err());
    }

    #[test]
    fn live_port_selection_prefers_the_registry_port() {
        let free = |p: u16| p != 4096;
        let never = |_: u16| false;
        assert_eq!(
            choose_port(PortPolicy::Flexible, 4097, free, never, || Some(1)).unwrap(),
            PortChoice::Start(4097)
        );
        assert_eq!(
            choose_port(PortPolicy::Flexible, 4096, free, never, || Some(51000)).unwrap(),
            PortChoice::Start(51000)
        );
        assert_eq!(
            choose_port(PortPolicy::Fixed, 4096, free, never, || Some(51000)).unwrap_err(),
            LiveError::PortInUse(4096)
        );
        assert_eq!(
            choose_port(PortPolicy::Flexible, 4096, free, never, || None).unwrap_err(),
            LiveError::NoFreePort
        );
        // The user's own OpenClaw Gateway already answers: point at it.
        assert_eq!(
            choose_port(PortPolicy::ReuseRunning, 4096, free, |_| true, || Some(51000)).unwrap(),
            PortChoice::Existing(4096)
        );
        // Something else holds the port: start ours elsewhere.
        assert_eq!(
            choose_port(PortPolicy::ReuseRunning, 4096, free, never, || Some(51000)).unwrap(),
            PortChoice::Start(51000)
        );
    }

    #[test]
    fn live_second_start_reuses_the_running_instance() {
        let mut table = LiveTable::default();
        let now = Instant::now();
        let (a, _tx) = app(Some(42), now - Duration::from_secs(60), &["ses_a"]);
        table.insert(key("opencode", "/w"), a);
        let reused = table.reuse(&key("opencode", "/w"), Some("ses_b"), now).expect("running");
        assert_eq!(reused.pid, Some(42));
        assert_eq!(reused.last_seen, now, "reuse refreshes the heartbeat");
        assert_eq!(reused.sessions.len(), 2);
        // Another folder or runtime is a different instance.
        assert!(table.reuse(&key("opencode", "/other"), None, now).is_none());
        assert!(table.reuse(&key("aider", "/w"), None, now).is_none());
    }

    #[test]
    fn live_dead_instance_is_not_reused() {
        let mut table = LiveTable::default();
        let (a, tx) = app(Some(42), Instant::now(), &[]);
        table.insert(key("opencode", "/w"), a);
        tx.send(true).unwrap();
        assert!(table.reuse(&key("opencode", "/w"), None, Instant::now()).is_none());
        assert!(table.keys().is_empty());
    }

    #[test]
    fn live_waiter_removes_only_its_own_pid() {
        let mut table = LiveTable::default();
        let (a, _tx) = app(Some(7), Instant::now(), &[]);
        table.insert(key("opencode", "/w"), a);
        table.remove_pid(&key("opencode", "/w"), 8);
        assert_eq!(table.keys().len(), 1);
        table.remove_pid(&key("opencode", "/w"), 7);
        assert!(table.keys().is_empty());
    }

    #[test]
    fn live_heartbeat_expires_after_30_minutes() {
        let now = Instant::now() + Duration::from_secs(3600);
        assert!(!heartbeat_expired(now - Duration::from_secs(29 * 60), now, HEARTBEAT_TTL));
        assert!(heartbeat_expired(now - Duration::from_secs(31 * 60), now, HEARTBEAT_TTL));
        let mut table = LiveTable::default();
        let (old, _t1) = app(Some(1), now - Duration::from_secs(31 * 60), &[]);
        let (fresh, _t2) = app(Some(2), now - Duration::from_secs(60), &[]);
        table.insert(key("opencode", "/old"), old);
        table.insert(key("opencode", "/fresh"), fresh);
        assert_eq!(table.expired(now, HEARTBEAT_TTL), vec![key("opencode", "/old")]);
    }

    #[test]
    fn live_session_end_stops_apps_it_was_the_last_session_of() {
        let mut table = LiveTable::default();
        let now = Instant::now();
        let (shared, _t1) = app(Some(1), now, &["ses_a", "ses_b"]);
        let (solo, _t2) = app(Some(2), now, &["ses_a"]);
        let (none, _t3) = app(Some(3), now, &[]);
        table.insert(key("opencode", "/shared"), shared);
        table.insert(key("opencode", "/solo"), solo);
        table.insert(key("opencode", "/none"), none);
        assert_eq!(table.release_session("ses_a"), vec![key("opencode", "/solo")]);
        assert_eq!(table.release_session("ses_b"), vec![key("opencode", "/shared")]);
        assert!(table.release_session("ses_c").is_empty());
    }

    #[test]
    fn live_url_and_resource_uri_shapes() {
        assert_eq!(live_url(4096), "http://127.0.0.1:4096");
        assert_eq!(resource_uri("opencode", 4096), "ui://allternit/live-app?runtime=opencode&port=4096");
    }

    #[test]
    fn live_cwd_prefers_the_session_workspace_then_home() {
        let temp = std::env::temp_dir();
        let home = PathBuf::from("/home/owner");
        assert_eq!(resolve_cwd(temp.to_str(), Some(home.clone())), temp);
        assert_eq!(resolve_cwd(Some("/no/such/folder/xyz"), Some(home.clone())), home);
        assert_eq!(resolve_cwd(None, Some(home.clone())), home);
        assert_eq!(resolve_cwd(Some("  "), Some(home.clone())), home);
    }
}
