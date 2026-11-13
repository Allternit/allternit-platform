//! The agent loop of the self-hosted server (plan steps 8 and 9).
//!
//! The loop is gizzi-code, run as one supervised child process on a loopback
//! port (`gizzi-code serve --hostname 127.0.0.1 --port N`). Model calls and
//! MCP run in the child. Sandbox tools do not: the child runs with
//! `GIZZI_TOOL_HOST=remote` and hands each one to
//! `POST /internal/v1/sessions/{gizzi id}/tool_calls` (routes_internal.rs),
//! which queues it for a worker.
//!
//! [`GizziTurnRunner`] implements [`TurnRunner`]:
//!
//! - **Turn start.** Checks the model against `inference.model_map` (no
//!   outbound call for an unmapped model) and that the child is ready.
//! - **Session.** Each of our sessions has one gizzi session, made from the
//!   frozen `agent_snapshot` (model, instructions, builtin tools, MCP servers
//!   allowed by `mcp.allowed_hostnames`), in its own gizzi directory
//!   `<data_dir>/gizzi/sessions/<session id>` so MCP servers and events stay
//!   per session. Its id is stored as `gizzi_session_id`.
//! - **Events.** The turn subscribes to gizzi's event stream and turns
//!   assistant text and tool parts into `message`, `tool_call` and
//!   `tool_result` events as they finish. When the turn returns, the
//!   transcript is read back once and anything the stream missed is added,
//!   so no part is lost or repeated.
//! - **End.** Status `idle`, or `error` with an `error` event.
//! - **Interrupt.** Aborts the gizzi session.
//! - **Rehydration (step 9).** If gizzi no longer knows the session (404, for
//!   example after the child restarted), the session is rebuilt from our
//!   stored events with gizzi's `POST /session/import`, and the new id is
//!   stored.
//!
//! The child gets `ALLTERNIT_SELF_HOSTED=1`, the tool host settings, a
//! per-boot basic-auth password (so no other local process can drive it),
//! `GIZZI_PERMISSION_MODE=dontAsk` (nobody is there to answer a prompt),
//! `GIZZI_INFERENCE_CONFIG` (a 0600 JSON file written from
//! `settings.inference` with secrets resolved) and a HOME under the data
//! dir. It is restarted with backoff when it exits.
//!
//! Custom tools (agent tools with `type: custom`) are passed to gizzi in the
//! turn's metadata (`selfhost.custom_tools`). When gizzi calls one, it goes
//! through the same tool call queue as a sandbox tool; the gizzi CLI worker
//! answers it with an error, SDK workers run it with their handlers.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;

use super::routes_agents::check_model;
use super::routes_sessions::{LoopStatus, TurnRunner};
use super::settings::{resolve_secret, Settings, SettingsError};
use super::state::{INTERNAL_TOKEN_ENV, SETUP_TOKEN_ENV};
use super::store::{EventRecord, SessionRecord, MAX_TOOL_RESULT_BYTES};
use super::{ApiError, SelfHostState};

/// Basic-auth user name the server uses on its gizzi child.
const GIZZI_USER: &str = "allternit";
/// Statuses with a turn in flight.
const ACTIVE: &[&str] = &["running", "waiting_for_worker"];
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_PROBE_EVERY: Duration = Duration::from_millis(250);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A child that ran this long resets the backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
const SHORT_CALL: Duration = Duration::from_secs(30);
const STREAM_CONNECT_WAIT: Duration = Duration::from_secs(5);
const STREAM_DRAIN_WAIT: Duration = Duration::from_secs(3);
/// An interrupted turn that gizzi does not end within this is stopped.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);

// ─── Endpoint and readiness ─────────────────────────────────────────────────

/// Where the gizzi child listens, and whether it is ready.
pub struct GizziEndpoint {
    base: RwLock<String>,
    password: Option<String>,
    status: RwLock<LoopStatus>,
    /// Raised on every (re)start, so per-child setup (MCP servers) reruns.
    generation: AtomicU64,
}

impl GizziEndpoint {
    fn new(base: String, password: Option<String>, status: LoopStatus) -> Self {
        Self {
            base: RwLock::new(base),
            password,
            status: RwLock::new(status),
            generation: AtomicU64::new(1),
        }
    }

    pub fn base(&self) -> String {
        self.base.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_base(&self, base: String) {
        *self.base.write().unwrap_or_else(|e| e.into_inner()) = base;
    }

    pub fn status(&self) -> LoopStatus {
        self.status.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_status(&self, s: LoopStatus) {
        *self.status.write().unwrap_or_else(|e| e.into_inner()) = s;
    }
}

#[derive(Debug)]
enum GizziError {
    Unreachable(String),
    Http(u16, String),
    Decode(String),
}

impl std::fmt::Display for GizziError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GizziError::Unreachable(e) => write!(f, "the agent loop is not reachable ({e})"),
            GizziError::Http(s, b) => {
                write!(f, "the agent loop answered {s}: {}", b.chars().take(300).collect::<String>())
            }
            GizziError::Decode(e) => write!(f, "the agent loop sent a bad response ({e})"),
        }
    }
}

/// Why a turn failed: the `error` event's code and message.
struct Failure {
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<GizziError> for Failure {
    fn from(e: GizziError) -> Self {
        let code = match e {
            GizziError::Unreachable(_) => "agent_loop_unavailable",
            _ => "agent_loop_error",
        };
        Failure::new(code, e.to_string())
    }
}

// ─── Runner ─────────────────────────────────────────────────────────────────

struct TurnHandle {
    turn_id: u64,
    abort: tokio::task::AbortHandle,
    interrupted: Arc<AtomicBool>,
    gizzi_session_id: Arc<Mutex<Option<String>>>,
}

struct Inner {
    endpoint: Arc<GizziEndpoint>,
    http: reqwest::Client,
    /// `<data_dir>/gizzi`.
    root: PathBuf,
    turns: Mutex<HashMap<String, TurnHandle>>,
    next_turn: AtomicU64,
    /// `(child generation, session id)` pairs whose MCP servers are set up.
    mcp_done: Mutex<HashSet<(u64, String)>>,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(h) = self.supervisor.lock().unwrap_or_else(|e| e.into_inner()).take() {
            // The child is `kill_on_drop`, so stopping the task stops it.
            h.abort();
        }
    }
}

/// Runs turns on the gizzi child. See the module docs.
pub struct GizziTurnRunner {
    inner: Arc<Inner>,
}

/// How to start the child.
pub struct ChildConfig {
    pub settings: Arc<Settings>,
    /// The server's loopback URL for the internal routes, e.g.
    /// `http://127.0.0.1:41234`.
    pub tool_host_url: String,
    pub internal_token: String,
    /// None: [`crate::cloud_worker::gizzi_bin`] (`ALLTERNIT_GIZZI_BIN`, next
    /// to this binary, or `gizzi-code` on PATH).
    pub bin: Option<PathBuf>,
}

fn loopback_client() -> reqwest::Client {
    reqwest::Client::builder()
        // Loopback only: never through an HTTP(S)_PROXY from the environment.
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

impl GizziTurnRunner {
    /// Use a gizzi server that is already running at `base_url` (tests, or an
    /// operator-run child). It is treated as ready.
    pub fn connect(base_url: &str, data_dir: &Path) -> Arc<Self> {
        let endpoint = GizziEndpoint::new(
            base_url.trim_end_matches('/').to_string(),
            None,
            LoopStatus::Ready,
        );
        Arc::new(Self {
            inner: Arc::new(Inner {
                endpoint: Arc::new(endpoint),
                http: loopback_client(),
                root: data_dir.join("gizzi"),
                turns: Mutex::new(HashMap::new()),
                next_turn: AtomicU64::new(1),
                mcp_done: Mutex::new(HashSet::new()),
                supervisor: Mutex::new(None),
            }),
        })
    }

    /// Write the inference config and start the supervised child. Errors
    /// only for a config that cannot work (a secret that does not resolve,
    /// an unwritable data dir). A missing gizzi binary is not an error: the
    /// runner reports `unavailable` on `/health/ready` and refuses turns
    /// with 503, and the rest of the API keeps working.
    pub fn spawn(cfg: ChildConfig) -> Result<Arc<Self>, String> {
        let root = cfg.settings.data_dir.join("gizzi");
        let home = root.join("home");
        let workspace = root.join("workspace");
        for d in [&root, &home, &workspace, &root.join("sessions")] {
            std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        restrict_dir(&root);
        let config_path = root.join("inference.json");
        let config = inference_config(&cfg.settings).map_err(|e| e.to_string())?;
        write_private_json(&config_path, &config)?;

        let password = super::store::new_secret("gzp");
        let endpoint = Arc::new(GizziEndpoint::new(
            String::new(),
            Some(password.clone()),
            LoopStatus::Starting,
        ));
        let http = loopback_client();
        let inner = Arc::new(Inner {
            endpoint: endpoint.clone(),
            http: http.clone(),
            root: root.clone(),
            turns: Mutex::new(HashMap::new()),
            next_turn: AtomicU64::new(1),
            mcp_done: Mutex::new(HashSet::new()),
            supervisor: Mutex::new(None),
        });

        let Some(bin) = cfg.bin.clone().or_else(crate::cloud_worker::gizzi_bin) else {
            let why = "gizzi-code was not found. Set ALLTERNIT_GIZZI_BIN or put gizzi-code on PATH.";
            tracing::error!("agent loop unavailable: {why}");
            endpoint.set_status(LoopStatus::Unavailable(why.to_string()));
            return Ok(Arc::new(Self { inner }));
        };

        let tool_timeout_ms = cfg.settings.no_worker_timeout().max(Duration::from_secs(600)).as_millis();
        let env: Vec<(String, String)> = vec![
            ("ALLTERNIT_SELF_HOSTED".into(), "1".into()),
            ("GIZZI_TOOL_HOST".into(), "remote".into()),
            ("GIZZI_TOOL_HOST_URL".into(), cfg.tool_host_url.clone()),
            ("GIZZI_TOOL_HOST_TOKEN".into(), cfg.internal_token.clone()),
            (INTERNAL_TOKEN_ENV.into(), cfg.internal_token.clone()),
            ("GIZZI_TOOL_HOST_TIMEOUT_MS".into(), tool_timeout_ms.to_string()),
            ("GIZZI_INFERENCE_CONFIG".into(), config_path.display().to_string()),
            ("GIZZI_PERMISSION_MODE".into(), "dontAsk".into()),
            ("GIZZI_PARENT_LIFELINE".into(), "stdin".into()),
            ("GIZZI_SERVER_USERNAME".into(), GIZZI_USER.into()),
            ("GIZZI_SERVER_PASSWORD".into(), password),
            ("HOME".into(), home.display().to_string()),
            ("XDG_CONFIG_HOME".into(), home.join(".config").display().to_string()),
            ("XDG_DATA_HOME".into(), home.join(".local/share").display().to_string()),
            ("XDG_CACHE_HOME".into(), home.join(".cache").display().to_string()),
            ("XDG_STATE_HOME".into(), home.join(".local/state").display().to_string()),
        ];
        tracing::info!(bin = %bin.display(), "starting the agent loop (gizzi child)");
        let task = tokio::spawn(supervise(endpoint, http, bin, env, workspace));
        *inner.supervisor.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
        Ok(Arc::new(Self { inner }))
    }

    pub fn endpoint(&self) -> Arc<GizziEndpoint> {
        self.inner.endpoint.clone()
    }
}

#[async_trait]
impl TurnRunner for GizziTurnRunner {
    fn available(&self) -> Result<(), ApiError> {
        match self.inner.endpoint.status() {
            LoopStatus::Ready | LoopStatus::Disabled => Ok(()),
            LoopStatus::Starting => Err(ApiError::unavailable(
                "agent_loop_unavailable",
                "The agent loop is starting. Try again in a few seconds.",
            )),
            LoopStatus::Unavailable(why) => Err(ApiError::unavailable(
                "agent_loop_unavailable",
                format!("The agent loop cannot run: {why}"),
            )),
        }
    }

    fn loop_status(&self) -> LoopStatus {
        self.inner.endpoint.status()
    }

    async fn start_turn(
        &self,
        st: &SelfHostState,
        session: &SessionRecord,
        user_event: &EventRecord,
    ) -> Result<(), ApiError> {
        // 11b: refused before any outbound call, also when the map changed
        // after the session was made.
        let model = snapshot_str(&session.agent_snapshot, "model");
        check_model(&st.settings, &model)?;
        self.available()?;

        let text = user_event
            .data
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let turn_id = self.inner.next_turn.fetch_add(1, Ordering::Relaxed);
        let interrupted = Arc::new(AtomicBool::new(false));
        let slot = Arc::new(Mutex::new(session.gizzi_session_id.clone()));
        let ctx = TurnCtx {
            st: st.clone(),
            inner: self.inner.clone(),
            session: session.clone(),
            user_seq: user_event.seq,
            text,
            interrupted: interrupted.clone(),
            slot: slot.clone(),
            turn_id,
        };
        // Hold the lock across the spawn so the task cannot finish (and
        // remove its entry) before the entry exists.
        let mut turns = self.inner.turns.lock().unwrap_or_else(|e| e.into_inner());
        let task = tokio::spawn(run_turn(ctx));
        turns.insert(
            session.id.clone(),
            TurnHandle {
                turn_id,
                abort: task.abort_handle(),
                interrupted,
                gizzi_session_id: slot,
            },
        );
        Ok(())
    }

    async fn interrupt(&self, st: &SelfHostState, session_id: &str) -> Result<(), ApiError> {
        let handle = self
            .inner
            .turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
        let mut gizzi = None;
        if let Some(h) = &handle {
            h.interrupted.store(true, Ordering::SeqCst);
            gizzi = h.gizzi_session_id.lock().unwrap_or_else(|e| e.into_inner()).clone();
        }
        if gizzi.is_none() {
            gizzi = st.store.get_session(session_id).await.ok().and_then(|s| s.gizzi_session_id);
        }
        if let Some(g) = gizzi {
            let path = format!("/v1/session/{}/abort", urlencoding::encode(&g));
            let dir = self.inner.session_dir(session_id);
            if let Err(e) = self.inner.call("POST", &path, Some(&dir), Some(&json!({})), Some(Duration::from_secs(5))).await {
                tracing::warn!(error = %e, session_id, "could not abort the gizzi session");
            }
        }
        if let Some(h) = handle {
            // gizzi ends the turn on abort; stop the task if it does not.
            tokio::spawn(async move {
                tokio::time::sleep(INTERRUPT_GRACE).await;
                h.abort.abort();
            });
        }
        Ok(())
    }
}

// ─── HTTP to the child ──────────────────────────────────────────────────────

impl Inner {
    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join("sessions").join(session_id)
    }

    fn request(&self, method: &str, path: &str, dir: Option<&Path>) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.endpoint.base(), path);
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut rb = self.http.request(m, url);
        if let Some(p) = &self.endpoint.password {
            rb = rb.basic_auth(GIZZI_USER, Some(p));
        }
        if let Some(d) = dir {
            let raw = d.display().to_string();
            rb = rb.header("x-gizzi-directory", urlencoding::encode(&raw).into_owned());
        }
        rb
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        dir: Option<&Path>,
        body: Option<&Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, GizziError> {
        let mut rb = self.request(method, path, dir);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        if let Some(t) = timeout {
            rb = rb.timeout(t);
        }
        let res = rb.send().await.map_err(|e| GizziError::Unreachable(e.to_string()))?;
        let status = res.status();
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(GizziError::Http(status.as_u16(), text));
        }
        let bytes = res.bytes().await.map_err(|e| GizziError::Decode(e.to_string()))?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| GizziError::Decode(e.to_string()))
    }
}

/// A free loopback port (bind, read, release).
fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

async fn probe(http: &reqwest::Client, endpoint: &GizziEndpoint) -> bool {
    let mut rb = http
        .get(format!("{}/health", endpoint.base()))
        .timeout(Duration::from_secs(2));
    if let Some(p) = &endpoint.password {
        rb = rb.basic_auth(GIZZI_USER, Some(p));
    }
    matches!(rb.send().await, Ok(r) if r.status().is_success())
}

fn pipe_logs<R>(reader: R, stream: &'static str)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                tracing::info!(target: "gizzi", stream, "{line}");
            }
        }
    });
}

/// Start the child, wait for `/health`, restart it with backoff when it
/// exits. Runs until aborted.
async fn supervise(
    endpoint: Arc<GizziEndpoint>,
    http: reqwest::Client,
    bin: PathBuf,
    env: Vec<(String, String)>,
    cwd: PathBuf,
) {
    let mut backoff = BACKOFF_START;
    loop {
        endpoint.set_status(LoopStatus::Starting);
        let port = match free_port() {
            Ok(p) => p,
            Err(e) => {
                endpoint.set_status(LoopStatus::Unavailable(format!("no free loopback port: {e}")));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        endpoint.set_base(format!("http://127.0.0.1:{port}"));
        endpoint.generation.fetch_add(1, Ordering::SeqCst);

        let mut cmd = tokio::process::Command::new(&bin);
        cmd.arg("serve")
            .arg("--hostname")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .current_dir(&cwd)
            .env_remove(SETUP_TOKEN_ENV)
            .env_remove(super::settings::CONFIG_ENV)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let started = Instant::now();
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let why = format!("cannot start {}: {e}", bin.display());
                tracing::error!("agent loop: {why}");
                endpoint.set_status(LoopStatus::Unavailable(why));
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        // Held open: when this process dies, the child's stdin hits EOF and
        // gizzi exits (GIZZI_PARENT_LIFELINE=stdin).
        let lifeline = child.stdin.take();
        if let Some(out) = child.stdout.take() {
            pipe_logs(out, "stdout");
        }
        if let Some(err) = child.stderr.take() {
            pipe_logs(err, "stderr");
        }

        let ready_by = tokio::time::Instant::now() + READY_TIMEOUT;
        let mut ready = false;
        let mut exited = None;
        while tokio::time::Instant::now() < ready_by {
            tokio::select! {
                status = child.wait() => {
                    exited = Some(status);
                    break;
                }
                _ = tokio::time::sleep(READY_PROBE_EVERY) => {
                    if probe(&http, &endpoint).await {
                        ready = true;
                        break;
                    }
                }
            }
        }
        if ready {
            tracing::info!(port, pid = ?child.id(), "agent loop ready");
            endpoint.set_status(LoopStatus::Ready);
            exited = Some(child.wait().await);
        } else if exited.is_none() {
            tracing::error!("agent loop did not become ready in {}s; restarting it", READY_TIMEOUT.as_secs());
            let _ = child.kill().await;
            exited = Some(child.wait().await);
        }
        drop(lifeline);
        endpoint.set_status(LoopStatus::Starting);
        match exited {
            Some(Ok(s)) => tracing::warn!(status = %s, "agent loop exited; restarting"),
            Some(Err(e)) => tracing::warn!(error = %e, "agent loop wait failed; restarting"),
            None => {}
        }
        if started.elapsed() >= HEALTHY_RUN {
            backoff = BACKOFF_START;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

// ─── Inference config ───────────────────────────────────────────────────────

fn resolve_opt(v: &Option<String>) -> Result<Option<String>, SettingsError> {
    v.as_deref().map(resolve_secret).transpose()
}

/// Drop null fields; None when nothing is left.
fn compact(v: Value) -> Option<Value> {
    match v {
        Value::Object(m) => {
            let m: Map<String, Value> = m.into_iter().filter(|(_, v)| !v.is_null()).collect();
            (!m.is_empty()).then_some(Value::Object(m))
        }
        other => Some(other),
    }
}

fn looks_like_ref(v: &str) -> bool {
    ["env:", "file:", "vault:"].iter().any(|p| v.starts_with(p))
}

/// The `GIZZI_INFERENCE_CONFIG` document, with secret references resolved:
/// `{provider, anthropic:{api_key, base_url?}, bedrock:{region,
/// access_key_id, secret_access_key, session_token?}, vertex:{project,
/// location, credentials_json?}, foundry:{endpoint, api_key, api_version?},
/// gateway:{format, base_url, headers}, model_map}`. Sections with no values
/// are left out.
pub fn inference_config(s: &Settings) -> Result<Value, SettingsError> {
    let inf = &s.inference;
    let c = inf.credentials.clone().unwrap_or_default();
    let mut out = Map::new();
    out.insert("provider".into(), json!(inf.provider.as_str()));
    if let Some(v) = compact(json!({ "api_key": resolve_opt(&c.api_key)? })) {
        out.insert("anthropic".into(), v);
    }
    if let Some(v) = compact(json!({
        "region": c.aws_region,
        "access_key_id": resolve_opt(&c.aws_access_key_id)?,
        "secret_access_key": resolve_opt(&c.aws_secret_access_key)?,
        "session_token": resolve_opt(&c.aws_session_token)?,
    })) {
        out.insert("bedrock".into(), v);
    }
    if let Some(v) = compact(json!({
        "project": c.gcp_project,
        "location": c.gcp_region,
        "credentials_json": resolve_opt(&c.gcp_service_account_json)?,
    })) {
        out.insert("vertex".into(), v);
    }
    if let Some(v) = compact(json!({
        "endpoint": c.foundry_endpoint,
        "api_key": resolve_opt(&c.foundry_api_key)?,
    })) {
        out.insert("foundry".into(), v);
    }
    if let Some(gw) = &inf.gateway {
        let mut headers = Map::new();
        for (k, v) in &gw.headers {
            let val = if looks_like_ref(v) { resolve_secret(v)? } else { v.clone() };
            headers.insert(k.clone(), Value::String(val));
        }
        out.insert(
            "gateway".into(),
            json!({
                "format": serde_json::to_value(gw.format).unwrap_or(Value::Null),
                "base_url": gw.base_url,
                "headers": headers,
            }),
        );
    }
    out.insert(
        "model_map".into(),
        serde_json::to_value(inf.model_map.clone().unwrap_or_default()).unwrap_or_else(|_| json!({})),
    );
    Ok(Value::Object(out))
}

fn restrict_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Write JSON readable only by this user (0600), atomically.
pub fn write_private_json(path: &Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&tmp)
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    let text = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    f.write_all(&text)
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// `(providerID, modelID)` for gizzi. A mapped name is sent as the public
/// name (the `model_map` key): gizzi loads the same `model_map` from
/// `GIZZI_INFERENCE_CONFIG`, lists exactly those keys as its models, and does
/// the key → provider id mapping itself. An unmapped `provider/model` name is
/// split. Anything else goes to the configured provider as is.
pub fn gizzi_model(s: &Settings, model: &str) -> (String, String) {
    let provider = s.inference.provider.as_str().to_string();
    if s.inference.model_map.as_ref().is_some_and(|m| m.contains_key(model)) {
        return (provider, model.to_string());
    }
    if let Some((p, m)) = model.split_once('/') {
        if !p.is_empty() && !m.is_empty() {
            return (p.to_string(), m.to_string());
        }
    }
    (provider, model.to_string())
}

// ─── Agent snapshot → gizzi ─────────────────────────────────────────────────

fn snapshot_str(snap: &Value, key: &str) -> String {
    snap.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn snapshot_tools<'a>(snap: &'a Value, kind: &str) -> Vec<&'a Value> {
    snap.get("tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|t| t.get("type").and_then(Value::as_str) == Some(kind))
                .collect()
        })
        .unwrap_or_default()
}

/// Exact host (or host:port) match, HTTPS only, on the server list and, when
/// the environment sets one, on the environment list too.
pub fn mcp_url_allowed(settings: &Settings, env_mcp: &Value, url: &str) -> Result<(), String> {
    let u = url::Url::parse(url).map_err(|e| format!("{url} is not a URL ({e})"))?;
    if u.scheme() != "https" {
        return Err(format!("{url} is not https"));
    }
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    let host_port = u.port().map(|p| format!("{host}:{p}"));
    let listed = |list: &[String]| {
        list.iter().any(|h| {
            let h = h.to_ascii_lowercase();
            h == host || host_port.as_deref() == Some(h.as_str())
        })
    };
    if !listed(&settings.mcp.allowed_hostnames) {
        return Err(format!("{host} is not in the server's mcp.allowed_hostnames"));
    }
    let env_list: Vec<String> = env_mcp
        .get("allowed_hostnames")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if !env_list.is_empty() && !listed(&env_list) {
        return Err(format!("{host} is not in the environment's mcp.allowed_hostnames"));
    }
    Ok(())
}

/// The MCP servers of the snapshot that the policy allows, and the refusals.
fn allowed_mcp_servers(settings: &Settings, env_mcp: &Value, snap: &Value) -> (Vec<Value>, Vec<String>) {
    let mut ok = Vec::new();
    let mut refused = Vec::new();
    for m in snap.get("mcp_servers").and_then(Value::as_array).into_iter().flatten() {
        let name = m.get("name").and_then(Value::as_str).unwrap_or_default();
        let url = m.get("url").and_then(Value::as_str).unwrap_or_default();
        match mcp_url_allowed(settings, env_mcp, url) {
            Ok(()) => ok.push(m.clone()),
            Err(why) => refused.push(format!("MCP server {name:?} was not connected: {why}.")),
        }
    }
    (ok, refused)
}

/// When the agent lists builtin tools, only those (and its MCP servers'
/// tools) are allowed. No list: gizzi's defaults.
fn permission_rules(snap: &Value, mcp: &[Value]) -> Option<Value> {
    let builtin = snapshot_tools(snap, "builtin");
    if builtin.is_empty() {
        return None;
    }
    let mut rules = vec![json!({ "permission": "*", "pattern": "*", "action": "deny" })];
    for t in builtin {
        if let Some(n) = t.get("name").and_then(Value::as_str) {
            rules.push(json!({ "permission": n, "pattern": "*", "action": "allow" }));
        }
    }
    for m in mcp {
        if let Some(n) = m.get("name").and_then(Value::as_str) {
            rules.push(json!({ "permission": format!("{n}_*"), "pattern": "*", "action": "allow" }));
        }
    }
    Some(Value::Array(rules))
}

fn session_title(session: &SessionRecord) -> String {
    session
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| format!("Agent session {}", session.id))
}

// ─── A turn ─────────────────────────────────────────────────────────────────

struct TurnCtx {
    st: SelfHostState,
    inner: Arc<Inner>,
    session: SessionRecord,
    user_seq: i64,
    text: String,
    interrupted: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<String>>>,
    turn_id: u64,
}

enum TurnEnd {
    Done,
    ModelError(String),
}

async fn run_turn(ctx: TurnCtx) {
    let result = drive(&ctx).await;
    let st = &ctx.st;
    let id = ctx.session.id.as_str();
    if ctx.interrupted.load(Ordering::SeqCst) {
        // The interrupt route owns the status now.
    } else {
        match result {
            Ok(TurnEnd::Done) => finish(st, id, "idle", None).await,
            Ok(TurnEnd::ModelError(message)) => {
                fail(st, id, Failure::new("model_error", message)).await;
            }
            Err(f) => fail(st, id, f).await,
        }
    }
    let mut turns = ctx.inner.turns.lock().unwrap_or_else(|e| e.into_inner());
    if turns.get(id).is_some_and(|h| h.turn_id == ctx.turn_id) {
        turns.remove(id);
    }
}

async fn finish(st: &SelfHostState, id: &str, to: &str, reason: Option<&str>) {
    match st.store.transition_session_status(id, ACTIVE, to, reason).await {
        Ok(Some(_)) => {
            let _ = st
                .append_event(id, "status", json!({ "status": to, "reason": reason }))
                .await;
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, session_id = id, "could not end the turn"),
    }
}

async fn fail(st: &SelfHostState, id: &str, f: Failure) {
    tracing::warn!(session_id = id, code = f.code, message = %f.message, "turn failed");
    let _ = st
        .append_event(id, "error", json!({ "code": f.code, "message": f.message }))
        .await;
    finish(st, id, "error", Some(f.code)).await;
}

async fn drive(ctx: &TurnCtx) -> Result<TurnEnd, Failure> {
    let dir = ctx.inner.session_dir(&ctx.session.id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| Failure::new("agent_loop_error", format!("cannot create the session folder: {e}")))?;
    let env = ctx
        .st
        .store
        .get_environment(&ctx.session.environment_id)
        .await
        .map_err(|e| Failure::new("agent_loop_error", format!("cannot read the environment: {e}")))?;
    let snap = &ctx.session.agent_snapshot;
    let (mcp, refused) = allowed_mcp_servers(&ctx.st.settings, &env.mcp, snap);
    for message in refused {
        let _ = ctx
            .st
            .append_event(&ctx.session.id, "error", json!({ "code": "host_not_allowed", "message": message }))
            .await;
    }

    let mut gsid = ensure_gizzi_session(ctx, &dir, &mcp).await?;
    let body = message_body(ctx, snap);

    let mut sub = Subscription::start(ctx, &gsid, &dir).await;
    let path = |g: &str| format!("/v1/session/{}/message", urlencoding::encode(g));
    let reply = match ctx.inner.call("POST", &path(&gsid), Some(&dir), Some(&body), None).await {
        Err(GizziError::Http(404, _)) => {
            // The child restarted between the check and the post.
            sub.stop().await;
            gsid = rehydrate(ctx, &dir, &mcp).await?;
            sub = Subscription::start(ctx, &gsid, &dir).await;
            ctx.inner.call("POST", &path(&gsid), Some(&dir), Some(&body), None).await
        }
        other => other,
    };
    sub.stop().await;
    let reply = reply.map_err(Failure::from)?;
    reconcile(ctx, &gsid, &dir, &reply, &sub.tracker).await;

    let error = reply
        .get("info")
        .and_then(|i| i.get("error"))
        .filter(|e| !e.is_null());
    if let Some(e) = error {
        let message = e
            .get("data")
            .and_then(|d| d.get("message"))
            .and_then(Value::as_str)
            .or_else(|| e.get("message").and_then(Value::as_str))
            .or_else(|| e.get("name").and_then(Value::as_str))
            .unwrap_or("The model call failed.")
            .to_string();
        return Ok(TurnEnd::ModelError(message));
    }
    Ok(TurnEnd::Done)
}

fn message_body(ctx: &TurnCtx, snap: &Value) -> Value {
    let (provider, model) = gizzi_model(&ctx.st.settings, &snapshot_str(snap, "model"));
    let custom: Vec<Value> = snapshot_tools(snap, "custom").into_iter().cloned().collect();
    let mut body = json!({
        "model": { "providerID": provider, "modelID": model },
        "agent": "build",
        "parts": [{ "type": "text", "text": ctx.text }],
        "metadata": {
            "selfhost": {
                "session_id": ctx.session.id,
                "agent_id": ctx.session.agent_id,
                "custom_tools": custom,
            }
        },
    });
    let instructions = snapshot_str(snap, "instructions");
    if !instructions.trim().is_empty() {
        // "+" appends to gizzi's own system prompt (tools, environment).
        body["system"] = json!(format!("+{}", instructions.trim()));
    }
    body
}

/// The gizzi session for this turn: the stored one if gizzi still has it,
/// else a rebuilt one (rehydration), else a new one.
async fn ensure_gizzi_session(ctx: &TurnCtx, dir: &Path, mcp: &[Value]) -> Result<String, Failure> {
    let stored = ctx
        .st
        .store
        .get_session(&ctx.session.id)
        .await
        .ok()
        .and_then(|s| s.gizzi_session_id)
        .or_else(|| ctx.session.gizzi_session_id.clone());
    let gsid = match stored {
        Some(g) => {
            let path = format!("/v1/session/{}", urlencoding::encode(&g));
            match ctx.inner.call("GET", &path, Some(dir), None, Some(SHORT_CALL)).await {
                Ok(_) => {
                    adopt(ctx, &g).await?;
                    g
                }
                Err(GizziError::Http(404, _)) => rehydrate(ctx, dir, mcp).await?,
                Err(e) => return Err(e.into()),
            }
        }
        None => create(ctx, dir, mcp).await?,
    };
    setup_mcp(ctx, dir, mcp).await;
    Ok(gsid)
}

/// Record the gizzi id: in the store, in the id map the internal route uses,
/// and in the turn's slot (for interrupt).
async fn adopt(ctx: &TurnCtx, gsid: &str) -> Result<(), Failure> {
    ctx.st.work.map_gizzi_session(gsid, &ctx.session.id);
    *ctx.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(gsid.to_string());
    if ctx.session.gizzi_session_id.as_deref() != Some(gsid) {
        ctx.st
            .store
            .set_gizzi_session_id(&ctx.session.id, gsid)
            .await
            .map_err(|e| Failure::new("agent_loop_error", format!("cannot store the gizzi session: {e}")))?;
    }
    Ok(())
}

fn id_of(v: &Value) -> Result<String, Failure> {
    v.get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Failure::new("agent_loop_error", "the agent loop returned no session id"))
}

async fn create(ctx: &TurnCtx, dir: &Path, mcp: &[Value]) -> Result<String, Failure> {
    let mut body = json!({ "title": session_title(&ctx.session) });
    if let Some(rules) = permission_rules(&ctx.session.agent_snapshot, mcp) {
        body["permission"] = rules;
    }
    let made = ctx.inner.call("POST", "/v1/session", Some(dir), Some(&body), Some(SHORT_CALL)).await?;
    let gsid = id_of(&made)?;
    adopt(ctx, &gsid).await?;
    tracing::info!(session_id = %ctx.session.id, gizzi_session_id = %gsid, "gizzi session created");
    Ok(gsid)
}

/// Step 9: rebuild the session from our events (everything before this
/// turn's user message) with gizzi's `POST /session/import`.
async fn rehydrate(ctx: &TurnCtx, dir: &Path, mcp: &[Value]) -> Result<String, Failure> {
    let mut events = Vec::new();
    let mut after = 0;
    loop {
        let page = ctx
            .st
            .store
            .list_events(&ctx.session.id, after, 500, None)
            .await
            .map_err(|e| Failure::new("agent_loop_error", format!("cannot read the session events: {e}")))?;
        let n = page.len();
        if let Some(last) = page.last() {
            after = last.seq;
        }
        events.extend(page.into_iter().filter(|e| e.seq < ctx.user_seq));
        if n < 500 || after >= ctx.user_seq {
            break;
        }
    }
    let (provider, model) = gizzi_model(&ctx.st.settings, &snapshot_str(&ctx.session.agent_snapshot, "model"));
    let messages = events_to_transcript(&events, &provider, &model, &dir.display().to_string());
    if messages.is_empty() {
        return create(ctx, dir, mcp).await;
    }
    let body = json!({ "title": session_title(&ctx.session), "messages": messages });
    let made = ctx
        .inner
        .call("POST", "/v1/session/import", Some(dir), Some(&body), Some(Duration::from_secs(120)))
        .await?;
    let gsid = id_of(&made)?;
    if let Some(rules) = permission_rules(&ctx.session.agent_snapshot, mcp) {
        let path = format!("/v1/session/{}", urlencoding::encode(&gsid));
        if let Err(e) = ctx
            .inner
            .call("PATCH", &path, Some(dir), Some(&json!({ "permission": rules })), Some(SHORT_CALL))
            .await
        {
            tracing::warn!(error = %e, "could not set the tool rules on the rebuilt session");
        }
    }
    adopt(ctx, &gsid).await?;
    tracing::info!(
        session_id = %ctx.session.id,
        gizzi_session_id = %gsid,
        messages = messages.len(),
        "gizzi session rebuilt from stored events"
    );
    Ok(gsid)
}

/// Connect the allowed MCP servers once per child (re)start and session.
async fn setup_mcp(ctx: &TurnCtx, dir: &Path, mcp: &[Value]) {
    if mcp.is_empty() {
        return;
    }
    let key = (
        ctx.inner.endpoint.generation.load(Ordering::SeqCst),
        ctx.session.id.clone(),
    );
    if ctx.inner.mcp_done.lock().unwrap_or_else(|e| e.into_inner()).contains(&key) {
        return;
    }
    for m in mcp {
        let mut body = json!({
            "name": m.get("name").cloned().unwrap_or(Value::Null),
            "type": "remote",
            "url": m.get("url").cloned().unwrap_or(Value::Null),
            "enabled": true,
            "oauth": false,
        });
        if let Some(h) = m.get("headers").filter(|h| h.is_object()) {
            body["headers"] = h.clone();
        }
        if let Err(e) = ctx.inner.call("POST", "/v1/mcp/add", Some(dir), Some(&body), Some(SHORT_CALL)).await {
            let name = m.get("name").and_then(Value::as_str).unwrap_or_default();
            let _ = ctx
                .st
                .append_event(
                    &ctx.session.id,
                    "error",
                    json!({ "code": "mcp_unavailable", "message": format!("MCP server {name:?} could not be added: {e}") }),
                )
                .await;
        }
    }
    ctx.inner.mcp_done.lock().unwrap_or_else(|e| e.into_inner()).insert(key);
}

// ─── gizzi events → our events ──────────────────────────────────────────────

/// What this turn already emitted, so the live stream and the final read
/// never store a part twice.
struct Tracker {
    gizzi_session_id: String,
    emitted: tokio::sync::Mutex<HashSet<String>>,
    assistant: Mutex<HashSet<String>>,
}

fn clip(s: &str) -> String {
    if s.len() <= MAX_TOOL_RESULT_BYTES {
        return s.to_string();
    }
    let mut end = MAX_TOOL_RESULT_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &s[..end])
}

impl Tracker {
    fn new(gizzi_session_id: &str) -> Self {
        Self {
            gizzi_session_id: gizzi_session_id.to_string(),
            emitted: tokio::sync::Mutex::new(HashSet::new()),
            assistant: Mutex::new(HashSet::new()),
        }
    }

    async fn emit(&self, st: &SelfHostState, session_id: &str, key: String, kind: &str, data: Value) {
        let mut done = self.emitted.lock().await;
        if done.contains(&key) {
            return;
        }
        match st.append_event(session_id, kind, data).await {
            Ok(_) => {
                done.insert(key);
            }
            Err(e) => tracing::warn!(error = %e, session_id, "could not store a turn event"),
        }
    }

    /// One gizzi bus event (`{type, properties}`).
    async fn on_bus_event(&self, st: &SelfHostState, session_id: &str, ev: &Value) {
        let props = ev.get("properties").cloned().unwrap_or(Value::Null);
        match ev.get("type").and_then(Value::as_str) {
            Some("message.updated") => {
                let info = &props["info"];
                if info["sessionID"].as_str() == Some(self.gizzi_session_id.as_str())
                    && info["role"].as_str() == Some("assistant")
                {
                    if let Some(id) = info["id"].as_str() {
                        self.assistant
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(id.to_string());
                    }
                }
            }
            Some("message.part.updated") => {
                let part = &props["part"];
                if part["sessionID"].as_str() == Some(self.gizzi_session_id.as_str()) {
                    self.on_part(st, session_id, part, false).await;
                }
            }
            _ => {}
        }
    }

    /// `final_read`: the part comes from the transcript read after the turn,
    /// so it is known to be an assistant part and is finished.
    async fn on_part(&self, st: &SelfHostState, session_id: &str, part: &Value, final_read: bool) {
        match part["type"].as_str() {
            Some("text") => {
                let msg = part["messageID"].as_str().unwrap_or_default();
                let is_assistant = final_read
                    || self
                        .assistant
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .contains(msg);
                let finished = final_read || part["time"]["end"].is_number();
                if !is_assistant
                    || !finished
                    || part["synthetic"].as_bool() == Some(true)
                    || part["ignored"].as_bool() == Some(true)
                {
                    return;
                }
                let text = part["text"].as_str().unwrap_or_default();
                let Some(pid) = part["id"].as_str() else { return };
                if text.trim().is_empty() {
                    return;
                }
                self.emit(
                    st,
                    session_id,
                    format!("text:{pid}"),
                    "message",
                    json!({ "role": "assistant", "content": text }),
                )
                .await;
            }
            Some("tool") => {
                let Some(call_id) = part["callID"].as_str() else { return };
                let name = part["tool"].as_str().unwrap_or_default();
                let state = &part["state"];
                let status = state["status"].as_str().unwrap_or_default();
                if !matches!(status, "running" | "completed" | "error") {
                    return;
                }
                let input = if state["input"].is_object() { state["input"].clone() } else { json!({}) };
                self.emit(
                    st,
                    session_id,
                    format!("call:{call_id}"),
                    "tool_call",
                    json!({ "tool_call_id": call_id, "name": name, "input": input }),
                )
                .await;
                let result = match status {
                    "completed" => Some((state["output"].as_str().unwrap_or_default(), false)),
                    "error" => Some((state["error"].as_str().unwrap_or_default(), true)),
                    _ => None,
                };
                if let Some((output, is_error)) = result {
                    self.emit(
                        st,
                        session_id,
                        format!("result:{call_id}"),
                        "tool_result",
                        json!({ "tool_call_id": call_id, "output": clip(output), "is_error": is_error }),
                    )
                    .await;
                }
            }
            _ => {}
        }
    }
}

/// The live event stream of one turn.
struct Subscription {
    tracker: Arc<Tracker>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Subscription {
    /// Subscribes and waits (up to 5 s) for gizzi's `server.connected`, so
    /// the turn's first events are not missed. Without a stream the final
    /// read still records the turn.
    async fn start(ctx: &TurnCtx, gsid: &str, dir: &Path) -> Self {
        let tracker = Arc::new(Tracker::new(gsid));
        let (stop_tx, stop_rx) = oneshot::channel();
        let (conn_tx, conn_rx) = oneshot::channel();
        let task = tokio::spawn(stream_events(
            ctx.st.clone(),
            ctx.inner.clone(),
            ctx.session.id.clone(),
            dir.to_path_buf(),
            tracker.clone(),
            stop_rx,
            conn_tx,
        ));
        if tokio::time::timeout(STREAM_CONNECT_WAIT, conn_rx).await.is_err() {
            tracing::warn!(session_id = %ctx.session.id, "gizzi event stream did not connect; using the final read only");
        }
        Self {
            tracker,
            stop: Some(stop_tx),
            task: Some(task),
        }
    }

    async fn stop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.task.take() {
            if tokio::time::timeout(STREAM_DRAIN_WAIT, t).await.is_err() {
                tracing::debug!("gizzi event stream did not stop in time");
            }
        }
    }
}

async fn stream_events(
    st: SelfHostState,
    inner: Arc<Inner>,
    session_id: String,
    dir: PathBuf,
    tracker: Arc<Tracker>,
    mut stop: oneshot::Receiver<()>,
    connected: oneshot::Sender<()>,
) {
    let res = inner
        .request("GET", "/v1/event", Some(&dir))
        .header("accept", "text/event-stream")
        .send()
        .await;
    let res = match res {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::warn!(status = r.status().as_u16(), "gizzi event stream refused");
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "gizzi event stream failed");
            return;
        }
    };
    let mut connected = Some(connected);
    let mut stream = Box::pin(res.bytes_stream());
    let mut buf: Vec<u8> = Vec::new();
    let mut data = String::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = &mut stop => break,
            c = stream.next() => c,
        };
        let Some(Ok(bytes)) = chunk else { break };
        buf.extend_from_slice(&bytes);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(|c| c == '\n' || c == '\r');
            if line.is_empty() {
                if !data.is_empty() {
                    if let Ok(ev) = serde_json::from_str::<Value>(&data) {
                        if ev["type"].as_str() == Some("server.connected") {
                            if let Some(tx) = connected.take() {
                                let _ = tx.send(());
                            }
                        } else {
                            tracker.on_bus_event(&st, &session_id, &ev).await;
                        }
                    }
                    data.clear();
                }
            } else if let Some(d) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(d.strip_prefix(' ').unwrap_or(d));
            }
        }
    }
}

/// After the turn: read the turn's assistant messages back and store any
/// part the live stream missed.
async fn reconcile(ctx: &TurnCtx, gsid: &str, dir: &Path, reply: &Value, tracker: &Tracker) {
    let parent = reply["info"]["parentID"].as_str().map(str::to_string);
    let mut messages: Vec<Value> = Vec::new();
    if let Some(parent) = &parent {
        let path = format!("/v1/session/{}/messages", urlencoding::encode(gsid));
        match ctx.inner.call("GET", &path, Some(dir), None, Some(SHORT_CALL)).await {
            Ok(Value::Array(all)) => {
                messages = all
                    .into_iter()
                    .filter(|m| {
                        m["info"]["role"].as_str() == Some("assistant")
                            && m["info"]["parentID"].as_str() == Some(parent.as_str())
                    })
                    .collect();
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "could not read the turn back from gizzi"),
        }
    }
    if messages.is_empty() {
        messages.push(reply.clone());
    }
    for m in &messages {
        for part in m["parts"].as_array().into_iter().flatten() {
            tracker.on_part(&ctx.st, &ctx.session.id, part, true).await;
        }
    }
}

// ─── Rehydration ────────────────────────────────────────────────────────────

/// Our stored events as a gizzi transcript (`MessageV2.WithParts[]`) for
/// `POST /session/import`. User messages start a turn. Assistant text and
/// tool calls go into assistant messages; a new step (text or a call after a
/// tool result) starts a new assistant message, as gizzi does. A call with no
/// recorded result is imported as an error ("no result was recorded").
/// Status and error events are left out. Ids are placeholders; gizzi gives
/// every message and part a new id on import.
pub fn events_to_transcript(events: &[EventRecord], provider: &str, model: &str, dir: &str) -> Vec<Value> {
    const SID: &str = "ses_import";
    let mut out: Vec<Value> = Vec::new();
    let mut last_user: Option<String> = None;
    // Index into `out` of the open assistant message.
    let mut assistant: Option<usize> = None;
    let mut after_result = false;
    // call id → (message index, part index)
    let mut calls: HashMap<String, (usize, usize)> = HashMap::new();
    let mut n = 0u64;
    let mut next_id = |prefix: &str| {
        n += 1;
        format!("{prefix}_import_{n:08}")
    };

    for e in events {
        let ms = e.created_at.timestamp_millis();
        match e.event_type.as_str() {
            "message" => {
                let role = e.data["role"].as_str().unwrap_or_default();
                let text = e.data["content"].as_str().unwrap_or_default();
                if role == "user" {
                    let id = next_id("msg");
                    let pid = next_id("prt");
                    out.push(json!({
                        "info": {
                            "id": id, "sessionID": SID, "role": "user",
                            "time": { "created": ms },
                            "agent": "build",
                            "model": { "providerID": provider, "modelID": model },
                        },
                        "parts": [{ "id": pid, "sessionID": SID, "messageID": id, "type": "text", "text": text }],
                    }));
                    last_user = Some(id);
                    assistant = None;
                    after_result = false;
                } else if role == "assistant" {
                    let Some(parent) = last_user.clone() else { continue };
                    let idx = open_assistant(&mut out, &mut assistant, &mut after_result, &parent, provider, model, dir, ms, &mut next_id);
                    let mid = out[idx]["info"]["id"].as_str().unwrap_or_default().to_string();
                    let pid = next_id("prt");
                    push_part(&mut out[idx], json!({ "id": pid, "sessionID": SID, "messageID": mid, "type": "text", "text": text }));
                }
            }
            "tool_call" => {
                let Some(parent) = last_user.clone() else { continue };
                let Some(call_id) = e.data["tool_call_id"].as_str() else { continue };
                let idx = open_assistant(&mut out, &mut assistant, &mut after_result, &parent, provider, model, dir, ms, &mut next_id);
                let mid = out[idx]["info"]["id"].as_str().unwrap_or_default().to_string();
                let pid = next_id("prt");
                let input = if e.data["input"].is_object() { e.data["input"].clone() } else { json!({}) };
                let part = json!({
                    "id": pid, "sessionID": SID, "messageID": mid, "type": "tool",
                    "callID": call_id,
                    "tool": e.data["name"].as_str().unwrap_or_default(),
                    "state": {
                        "status": "error", "input": input,
                        "error": "No result was recorded for this call.",
                        "time": { "start": ms, "end": ms },
                    },
                });
                let pi = push_part(&mut out[idx], part);
                calls.insert(call_id.to_string(), (idx, pi));
            }
            "tool_result" => {
                let Some(call_id) = e.data["tool_call_id"].as_str() else { continue };
                let Some(&(mi, pi)) = calls.get(call_id) else { continue };
                let output = e.data["output"].as_str().unwrap_or_default();
                let is_error = e.data["is_error"].as_bool().unwrap_or(false);
                let part = &mut out[mi]["parts"][pi];
                let input = part["state"]["input"].clone();
                let start = part["state"]["time"]["start"].clone();
                let name = part["tool"].clone();
                part["state"] = if is_error {
                    json!({ "status": "error", "input": input, "error": output, "time": { "start": start, "end": ms } })
                } else {
                    json!({
                        "status": "completed", "input": input, "output": output,
                        "title": name, "metadata": {},
                        "time": { "start": start, "end": ms },
                    })
                };
                after_result = true;
            }
            _ => {}
        }
    }
    out
}

fn push_part(message: &mut Value, part: Value) -> usize {
    let parts = message["parts"].as_array_mut().expect("parts is an array");
    parts.push(part);
    parts.len() - 1
}

#[allow(clippy::too_many_arguments)]
fn open_assistant(
    out: &mut Vec<Value>,
    assistant: &mut Option<usize>,
    after_result: &mut bool,
    parent: &str,
    provider: &str,
    model: &str,
    dir: &str,
    ms: i64,
    next_id: &mut impl FnMut(&str) -> String,
) -> usize {
    if let Some(i) = *assistant {
        if !*after_result {
            return i;
        }
    }
    let id = next_id("msg");
    out.push(json!({
        "info": {
            "id": id, "sessionID": "ses_import", "role": "assistant",
            "time": { "created": ms, "completed": ms },
            "parentID": parent,
            "modelID": model, "providerID": provider,
            "mode": "build", "agent": "build",
            "path": { "cwd": dir, "root": dir },
            "cost": 0,
            "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } },
            "finish": "stop",
        },
        "parts": [],
    }));
    *after_result = false;
    *assistant = Some(out.len() - 1);
    out.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn ev(seq: i64, kind: &str, data: Value) -> EventRecord {
        EventRecord {
            id: format!("evt_{seq}"),
            session_id: "sess_1".into(),
            seq,
            event_type: kind.into(),
            data,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn transcript_keeps_turns_tools_and_steps() {
        let events = vec![
            ev(1, "message", json!({"role": "user", "content": "list files"})),
            ev(2, "status", json!({"status": "running"})),
            ev(3, "tool_call", json!({"tool_call_id": "c1", "name": "bash", "input": {"command": "ls"}})),
            ev(4, "tool_result", json!({"tool_call_id": "c1", "output": "a\nb", "is_error": false})),
            ev(5, "message", json!({"role": "assistant", "content": "Two files."})),
            ev(6, "message", json!({"role": "user", "content": "thanks"})),
            ev(7, "tool_call", json!({"tool_call_id": "c2", "name": "read", "input": {}})),
        ];
        let t = events_to_transcript(&events, "anthropic", "claude-x", "/d");
        let roles: Vec<&str> = t.iter().map(|m| m["info"]["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "assistant", "user", "assistant"]);
        assert_eq!(t[1]["parts"][0]["state"]["status"], "completed");
        assert_eq!(t[1]["parts"][0]["state"]["output"], "a\nb");
        assert_eq!(t[2]["parts"][0]["text"], "Two files.");
        assert_eq!(t[2]["info"]["parentID"], t[0]["info"]["id"]);
        // A call with no result is imported as an error, not left pending.
        assert_eq!(t[4]["parts"][0]["state"]["status"], "error");
        assert_eq!(t[4]["info"]["parentID"], t[3]["info"]["id"]);
    }

    #[test]
    fn mcp_hosts_match_exactly_over_https() {
        let s = Settings::from_str(
            "inference: {provider: anthropic}\nmcp: {allowed_hostnames: ['mcp.example.com', 'tools.example.com:8443']}\n",
        )
        .unwrap();
        let none = json!({});
        assert!(mcp_url_allowed(&s, &none, "https://mcp.example.com/sse").is_ok());
        assert!(mcp_url_allowed(&s, &none, "https://tools.example.com:8443/mcp").is_ok());
        assert!(mcp_url_allowed(&s, &none, "https://evil.mcp.example.com/").is_err());
        assert!(mcp_url_allowed(&s, &none, "http://mcp.example.com/").is_err());
        let env = json!({"allowed_hostnames": ["tools.example.com:8443"]});
        assert!(mcp_url_allowed(&s, &env, "https://mcp.example.com/").is_err());
    }

    #[test]
    fn model_map_drives_the_gizzi_model() {
        let s = Settings::from_str(
            "inference:\n  provider: bedrock\n  model_map: {claude-sonnet: anthropic.claude-sonnet-v1}\n",
        )
        .unwrap();
        assert_eq!(
            gizzi_model(&s, "claude-sonnet"),
            ("bedrock".to_string(), "claude-sonnet".to_string())
        );
    }

    #[test]
    fn inference_config_resolves_secrets_and_drops_empty_sections() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        std::fs::write(&key, "sk-test\n").unwrap();
        let s = Settings::from_str(&format!(
            "inference:\n  provider: anthropic\n  credentials:\n    api_key: file:{}\n",
            key.display()
        ))
        .unwrap();
        let c = inference_config(&s).unwrap();
        assert_eq!(c["provider"], "anthropic");
        assert_eq!(c["anthropic"]["api_key"], "sk-test");
        assert!(c.get("bedrock").is_none());
        assert_eq!(c["model_map"], json!({}));
        let path = dir.path().join("inference.json");
        write_private_json(&path, &c).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
