//! `node.core` request handlers. Every request the relay routes to the
//! daemon is answered in-process here — the daemon has no inbound ports and
//! no loopback gateway. Explicitly NOT served: screen capture, ACI/browser,
//! voice — anything needing a logged-in GUI session stays with the desktop
//! app.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::NodeConfig;
use crate::engine::EngineSlot;
use crate::launch::{self, LaunchAction};
use crate::terminal::{PtySpec, TerminalStore};
use allternit_terminal_service as terminal_service;
use futures_util::StreamExt;

/// One inbound relay request handed to the handlers.
#[derive(Debug, Clone)]
pub struct InboundRequest {
    pub method: String,
    /// Path including any query string.
    pub path: String,
    pub body: Vec<u8>,
    /// Forwarded request headers (cloud-api's allow-list; names as sent, compare
    /// case-insensitively).
    pub headers: std::collections::HashMap<String, String>,
}

impl InboundRequest {
    /// A forwarded header, any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// What a handler produced: either a full body or a chunk stream
/// (terminal SSE). Streams carry `terminal_session` so the relay loop can
/// detect pane exit and close the response.
#[derive(Debug)]
pub enum ResponseBody {
    Full(Vec<u8>),
    Stream(tokio::sync::mpsc::Receiver<Vec<u8>>, Option<String>),
}

#[derive(Debug)]
pub struct HandlerResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: ResponseBody,
}

impl HandlerResponse {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: ResponseBody::Full(value.to_string().into_bytes()),
        }
    }

    fn not_found(message: &str) -> Self {
        Self::json(
            404,
            json!({ "error": "not_found", "message": message }),
        )
    }
}

/// Relay connection statistics surfaced through node.metrics.
#[derive(Default)]
pub struct RelayStats {
    pub connects: AtomicU64,
    pub reconnects: AtomicU64,
}

pub struct DaemonState {
    pub config: NodeConfig,
    /// The PTY fallback (no pane engine on this computer). These die with
    /// the daemon.
    pub terminals: TerminalStore,
    /// The Factory pane engine, when one answers: it serves terminals first.
    pub engine: EngineSlot,
    pub started: Instant,
    pub relay: RelayStats,
}

impl DaemonState {
    pub fn new(config: NodeConfig) -> Arc<Self> {
        Arc::new(Self {
            engine: EngineSlot::new(config.pane_socket.clone()),
            config,
            terminals: TerminalStore::new(),
            started: Instant::now(),
            relay: RelayStats::default(),
        })
    }
}

/// SSE frame helpers (same wire shape as allternit-api terminal_routes).
fn sse_data_event(data: &str) -> Vec<u8> {
    let payload = json!({ "type": "data", "data": data });
    format!("data: {payload}\n\n").into_bytes()
}

fn sse_ping_event() -> Vec<u8> {
    b"event: ping\ndata: {}\n\n".to_vec()
}

fn sse_exit_event() -> Vec<u8> {
    b"data: {\"type\":\"exit\"}\n\n".to_vec()
}

/// Terminal endpoints answer with the gateway's envelope shape.
fn terminal_envelope(success: bool, message: &str, data: Option<Value>) -> Value {
    match data {
        Some(data) => json!({ "success": success, "message": message, "data": data }),
        None => json!({ "success": success, "message": message }),
    }
}

fn body_json(body: &[u8]) -> Value {
    if body.is_empty() {
        return json!({});
    }
    serde_json::from_slice(body).unwrap_or(json!({}))
}

/// Route one request. Pure dispatch — the relay client owns envelopes.
pub async fn handle(state: &Arc<DaemonState>, request: &InboundRequest) -> HandlerResponse {
    let (path, query) = request
        .path
        .split_once('?')
        .unwrap_or((request.path.as_str(), ""));
    let method = request.method.to_ascii_uppercase();

    match (method.as_str(), path) {
        ("GET", "/api/v1/node/health") => health(state),
        ("GET", "/api/v1/node/metrics") => metrics(state).await,
        ("POST", "/api/v1/node/exec") => exec(state, &request.body).await,
        ("POST", "/api/v1/node/fs") => fs_op(state, &request.body).await,
        ("GET", "/api/v1/node/processes") => processes().await,
        ("POST", "/api/v1/node/processes/kill") => kill_process(&request.body).await,
        ("POST", "/api/v1/node/launch") => launch(state, &request.body).await,
        ("GET", "/api/v1/session-start/options") => session_start_options(state).await,
        ("POST", "/api/v1/session-start") => session_start(state, request).await,
        ("POST", "/terminal/create") => terminal_create(state, &request.body).await,
        ("GET", "/terminal/list") => terminal_list(state, query).await,
        ("POST", path) if path.starts_with("/terminal/") && path.ends_with("/input") => {
            terminal_action(state, path, "input", &request.body).await
        }
        ("POST", path) if path.starts_with("/terminal/") && path.ends_with("/resize") => {
            terminal_action(state, path, "resize", &request.body).await
        }
        ("POST", path) if path.starts_with("/terminal/") && path.ends_with("/close") => {
            terminal_action(state, path, "close", &request.body).await
        }
        ("GET", path) if path.starts_with("/terminal/") && path.ends_with("/stream") => {
            terminal_stream(state, path).await
        }
        _ => HandlerResponse::not_found(
            "allternit-node serves node.core endpoints (health, exec, fs, processes, metrics, launch, terminal) only",
        ),
    }
}

fn capability_set() -> Vec<&'static str> {
    vec![
        "node.core",
        "runtime:connect",
        "runtime:execute",
        "runtime:terminal",
        "runtime:files",
    ]
}

fn health(state: &Arc<DaemonState>) -> HandlerResponse {
    HandlerResponse::json(
        200,
        json!({
            "status": "ok",
            "client": "allternit-node",
            "version": env!("CARGO_PKG_VERSION"),
            "uptimeSec": state.started.elapsed().as_secs(),
            "capabilities": capability_set(),
            "execEnabled": state.config.exec_enabled,
            "fsRoots": state.config.fs_roots,
            "terminals": state.terminals.session_count(),
        }),
    )
}

async fn metrics(state: &Arc<DaemonState>) -> HandlerResponse {
    HandlerResponse::json(
        200,
        json!({
            "uptimeSec": state.started.elapsed().as_secs(),
            "relay": {
                "connects": state.relay.connects.load(Ordering::Relaxed),
                "reconnects": state.relay.reconnects.load(Ordering::Relaxed),
            },
            "terminals": state.terminals.session_count(),
            "load": system_load(),
        }),
    )
}

fn system_load() -> Value {
    #[cfg(target_os = "linux")]
    {
        if let Ok(raw) = std::fs::read_to_string("/proc/loadavg") {
            let parts: Vec<&str> = raw.split_whitespace().collect();
            if parts.len() >= 3 {
                return json!({ "one": parts[0], "five": parts[1], "fifteen": parts[2] });
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
        {
            let raw = String::from_utf8_lossy(&output.stdout);
            let parts: Vec<&str> = raw
                .trim_matches(|c| c == '{' || c == '}')
                .split_whitespace()
                .collect();
            if parts.len() >= 3 {
                return json!({ "one": parts[0], "five": parts[1], "fifteen": parts[2] });
            }
        }
    }
    Value::Null
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecRequest {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
    #[serde(default)]
    shell: bool,
    #[serde(default, alias = "timeout_ms")]
    timeout_ms: Option<u64>,
}

const EXEC_TIMEOUT_DEFAULT_MS: u64 = 60_000;
const EXEC_TIMEOUT_MAX_MS: u64 = 300_000;

async fn exec(state: &Arc<DaemonState>, body: &[u8]) -> HandlerResponse {
    if !state.config.exec_enabled {
        return HandlerResponse::json(
            403,
            json!({ "error": "exec_disabled", "message": "node.exec is disabled in node-config.json" }),
        );
    }
    let request: ExecRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => {
            return HandlerResponse::json(
                400,
                json!({ "error": "bad_request", "message": format!("invalid exec request: {error}") }),
            )
        }
    };
    if request.command.trim().is_empty() {
        return HandlerResponse::json(
            400,
            json!({ "error": "bad_request", "message": "command is required" }),
        );
    }
    let timeout = Duration::from_millis(
        request.timeout_ms.unwrap_or(EXEC_TIMEOUT_DEFAULT_MS).min(EXEC_TIMEOUT_MAX_MS),
    );

    let mut command = if request.shell {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.arg("-c").arg(&request.command);
        command
    } else {
        let mut command = tokio::process::Command::new(&request.command);
        command.args(&request.args);
        command
    };
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &request.env {
        command.env(key, value);
    }
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return HandlerResponse::json(
                500,
                json!({ "error": "exec_failed", "message": error.to_string() }),
            )
        }
    };
    // Drain the pipes on dedicated tasks so a chatty child can never
    // dead-lock against the pipe buffer while we wait. On timeout the child
    // is killed and reaped explicitly — a timed-out command never outlives
    // the exec as an orphan.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        match stdout_pipe.as_mut() {
            Some(pipe) => tokio::io::AsyncReadExt::read_to_end(pipe, &mut buffer).await.map(|_| buffer),
            None => Ok(buffer),
        }
    });
    let stderr_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        match stderr_pipe.as_mut() {
            Some(pipe) => tokio::io::AsyncReadExt::read_to_end(pipe, &mut buffer).await.map(|_| buffer),
            None => Ok(buffer),
        }
    });

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout = stdout_task
                .await
                .map(|result| result.unwrap_or_default())
                .unwrap_or_default();
            let stderr = stderr_task
                .await
                .map(|result| result.unwrap_or_default())
                .unwrap_or_default();
            HandlerResponse::json(
                200,
                json!({
                    "stdout": String::from_utf8_lossy(&stdout),
                    "stderr": String::from_utf8_lossy(&stderr),
                    "exitCode": status.code(),
                }),
            )
        }
        Ok(Err(error)) => HandlerResponse::json(
            500,
            json!({ "error": "exec_failed", "message": error.to_string() }),
        ),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            HandlerResponse::json(
                504,
                json!({ "error": "exec_timeout", "message": format!("command exceeded {}ms", timeout.as_millis()) }),
            )
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum FsOp {
    List,
    Read,
    Write,
    Stat,
    Mkdir,
    Remove,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FsRequest {
    op: FsOp,
    path: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    recursive: bool,
}

const FS_READ_CAP_BYTES: usize = 1024 * 1024;
const FS_WRITE_CAP_BYTES: usize = 5 * 1024 * 1024;

async fn fs_op(state: &Arc<DaemonState>, body: &[u8]) -> HandlerResponse {
    let request: FsRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => {
            return HandlerResponse::json(
                400,
                json!({ "error": "bad_request", "message": format!("invalid fs request: {error}") }),
            )
        }
    };
    let scoped = match state.config.scope_check(std::path::Path::new(&request.path)) {
        Ok(path) => path,
        Err(error) => {
            return HandlerResponse::json(
                403,
                json!({ "error": "outside_fs_scope", "message": error.to_string() }),
            )
        }
    };

    // Size caps are enforced before the scoped-op closure so the 413s stay
    // HTTP-shaped (the closure only reports fs errors as strings).
    if request.op == FsOp::Read {
        if let Ok(metadata) = std::fs::metadata(&scoped) {
            if metadata.len() as usize > FS_READ_CAP_BYTES {
                return HandlerResponse::json(
                    413,
                    json!({ "error": "too_large", "message": format!("file exceeds {FS_READ_CAP_BYTES} byte read cap") }),
                );
            }
        }
    }
    if request.op == FsOp::Write {
        if request.content.as_deref().map_or(0, str::len) > FS_WRITE_CAP_BYTES {
            return HandlerResponse::json(
                413,
                json!({ "error": "too_large", "message": format!("write exceeds {FS_WRITE_CAP_BYTES} byte cap") }),
            );
        }
    }

    let result: Result<Value, String> = (|| -> Result<Value, String> {
        match request.op {
            FsOp::List => {
                let mut entries = Vec::new();
                let read = std::fs::read_dir(&scoped).map_err(|error| error.to_string())?;
                for entry in read.flatten() {
                    let metadata = entry.metadata().ok();
                    entries.push(json!({
                        "name": entry.file_name().to_string_lossy(),
                        "path": entry.path().to_string_lossy(),
                        "isDir": metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                        "size": metadata.as_ref().map(|m| m.len()).unwrap_or(0),
                    }));
                }
                entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Ok(json!({ "entries": entries }))
            }
            FsOp::Read => {
                let metadata = std::fs::metadata(&scoped).map_err(|error| error.to_string())?;
                let content = std::fs::read_to_string(&scoped).map_err(|error| error.to_string())?;
                Ok(json!({ "content": content, "size": metadata.len() }))
            }
            FsOp::Write => {
                let content = request.content.clone().unwrap_or_default();
                if let Some(parent) = scoped.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&scoped, content).map_err(|error| error.to_string())?;
                Ok(json!({ "written": scoped.to_string_lossy() }))
            }
            FsOp::Stat => {
                let metadata = std::fs::metadata(&scoped).map_err(|error| error.to_string())?;
                Ok(json!({
                    "path": scoped.to_string_lossy(),
                    "isDir": metadata.is_dir(),
                    "size": metadata.len(),
                    "modified": metadata.modified().ok().map(|t| {
                        chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
                    }),
                }))
            }
            FsOp::Mkdir => {
                std::fs::create_dir_all(&scoped).map_err(|error| error.to_string())?;
                Ok(json!({ "created": scoped.to_string_lossy() }))
            }
            FsOp::Remove => {
                let metadata = std::fs::metadata(&scoped).map_err(|error| error.to_string())?;
                if metadata.is_dir() {
                    if request.recursive {
                        std::fs::remove_dir_all(&scoped).map_err(|error| error.to_string())?;
                    } else {
                        std::fs::remove_dir(&scoped).map_err(|error| error.to_string())?;
                    }
                } else {
                    std::fs::remove_file(&scoped).map_err(|error| error.to_string())?;
                }
                Ok(json!({ "removed": scoped.to_string_lossy() }))
            }
        }
    })();

    match result {
        Ok(value) => HandlerResponse::json(200, value),
        Err(message) => HandlerResponse::json(
            400,
            json!({ "error": "fs_error", "message": message }),
        ),
    }
}

async fn processes() -> HandlerResponse {
    let output = match tokio::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,pcpu=,pmem=,comm="])
        .output()
        .await
    {
        Ok(output) if output.status.success() => output,
        _ => {
            return HandlerResponse::json(
                500,
                json!({ "error": "ps_failed", "message": "unable to list processes" }),
            )
        }
    };
    let mut list = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(cpu), Some(mem), Some(command)) = (
            parts.next().and_then(|v| v.parse::<u32>().ok()),
            parts.next().and_then(|v| v.parse::<u32>().ok()),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        list.push(json!({
            "pid": pid,
            "ppid": ppid,
            "cpuPercent": cpu,
            "memPercent": mem,
            "command": command,
        }));
    }
    HandlerResponse::json(200, json!({ "processes": list }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KillRequest {
    pid: u32,
    #[serde(default = "default_signal")]
    signal: String,
}

fn default_signal() -> String {
    "TERM".to_string()
}

async fn kill_process(body: &[u8]) -> HandlerResponse {
    let request: KillRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => {
            return HandlerResponse::json(
                400,
                json!({ "error": "bad_request", "message": format!("invalid kill request: {error}") }),
            )
        }
    };
    if !matches!(request.signal.as_str(), "TERM" | "KILL" | "HUP" | "INT") {
        return HandlerResponse::json(
            400,
            json!({ "error": "bad_request", "message": "signal must be one of TERM, KILL, HUP, INT" }),
        );
    }
    #[cfg(unix)]
    let command = format!("kill -{} {}", request.signal, request.pid);
    #[cfg(not(unix))]
    let command = format!("taskkill /PID {} /F", request.pid);
    match tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&command)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            HandlerResponse::json(200, json!({ "killed": request.pid, "signal": request.signal }))
        }
        Ok(output) => HandlerResponse::json(
            500,
            json!({
                "error": "kill_failed",
                "message": String::from_utf8_lossy(&output.stderr).trim().to_string(),
            }),
        ),
        Err(error) => HandlerResponse::json(
            500,
            json!({ "error": "kill_failed", "message": error.to_string() }),
        ),
    }
}

async fn launch(state: &Arc<DaemonState>, body: &[u8]) -> HandlerResponse {
    #[derive(Deserialize)]
    struct LaunchRequest {
        #[serde(default)]
        action: Option<LaunchAction>,
    }    let action = serde_json::from_slice::<LaunchRequest>(body)
        .ok()
        .and_then(|request| request.action)
        .unwrap_or(LaunchAction::Start);    let result = launch::launch_desktop(action, state.config.desktop_launch.as_deref()).await;
    HandlerResponse::json(
        if result.started { 200 } else { 500 },
        json!(result),
    )
}

/// The shell to spawn: the one asked for when this computer has it, else
/// `$SHELL`, else bash or sh. The web UI asks for `/bin/zsh`, which a Linux
/// cloud computer usually doesn't have (ENOENT).
fn resolve_shell(requested: &str) -> String {
    let requested = requested.trim();
    let exists = |path: &str| std::path::Path::new(path).exists();
    if !requested.is_empty() && (!requested.starts_with('/') || exists(requested)) {
        return requested.to_string();
    }
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty() && exists(shell))
        .or_else(|| ["/bin/bash", "/bin/sh"].into_iter().find(|shell| exists(shell)).map(str::to_string))
        .unwrap_or_else(|| "/bin/sh".to_string())
}

/// What an engine terminal call answered, as a relay response. A 503 means
/// the engine stopped answering: the next request probes for it again.
async fn engine_reply(state: &Arc<DaemonState>, mut reply: terminal_service::Reply) -> HandlerResponse {
    if reply.status == 503 {
        state.engine.forget().await;
        reply.body.message = "the Factory pane engine on this computer is not answering".to_string();
    }
    HandlerResponse::json(
        reply.status,
        serde_json::to_value(&reply.body).unwrap_or_else(|_| json!({})),
    )
}

/// A request body as `T`; an empty body is `{}` (every field defaulted).
fn parse_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, serde_json::Error> {
    if body.iter().all(u8::is_ascii_whitespace) {
        serde_json::from_slice(b"{}")
    } else {
        serde_json::from_slice(body)
    }
}

/// The PTY create for a create request: `command` + `args` when given, else
/// the shell.
fn pty_spec(request: &terminal_service::CreateTerminalRequest) -> PtySpec {
    let program = request
        .command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let argv = match program {
        Some(program) => std::iter::once(program.to_string())
            .chain(request.args.iter().cloned())
            .collect(),
        None => vec![resolve_shell(&request.shell)],
    };
    PtySpec {
        argv,
        cwd: request.cwd.clone(),
        cols: request.cols,
        rows: request.rows,
        env: request.env.clone(),
        title: request.title.clone(),
    }
}

/// `POST /terminal/create` — the same body as allternit-api (`shell`, `cwd`,
/// `cols`, `rows`, `command`, `args`, `env`, `title`) on both paths. A body
/// that doesn't parse creates the default shell, as it always has here.
async fn terminal_create(state: &Arc<DaemonState>, body: &[u8]) -> HandlerResponse {
    let request = match parse_body::<terminal_service::CreateTerminalRequest>(body) {
        Ok(request) => request,
        Err(_) => serde_json::from_slice(b"{}").expect("an empty create request parses"),
    };
    if let Some(engine) = state.engine.get().await {
        return match engine.create(request, None).await {
            Ok(session_id) => HandlerResponse::json(
                200,
                terminal_envelope(true, "Terminal session created", Some(json!({ "session_id": session_id }))),
            ),
            Err(reply) => engine_reply(state, reply).await,
        };
    }
    if let Err(message) = terminal_service::validate_env(&request.env) {
        return HandlerResponse::json(400, terminal_envelope(false, &message, None));
    }
    match state.terminals.create_with(pty_spec(&request)) {
        Ok(created) => HandlerResponse::json(
            200,
            terminal_envelope(true, "Terminal session created", Some(json!({ "session_id": created.session_id }))),
        ),
        Err(error) => HandlerResponse::json(
            500,
            terminal_envelope(false, &format!("unable to create terminal: {error}"), None),
        ),
    }
}

// ───────────────────────────────────────────────────────────── Start anywhere

/// The Gizzi engine on this computer (the engine kind is forwarded to it).
const GIZZI_ENGINE: &str = "http://127.0.0.1:4096";
/// Where `folder.git` starts are cloned (`$HOME/workspaces/<repo>`).
const START_WORKSPACES: &str = "workspaces";
/// How long a CLI gets to draw its prompt before a prompt it takes no flag for is typed in.
const START_TYPE_PROMPT_AFTER: Duration = Duration::from_millis(2500);

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn start_guards() -> &'static terminal_service::StartGuards {
    static G: std::sync::OnceLock<terminal_service::StartGuards> = std::sync::OnceLock::new();
    G.get_or_init(terminal_service::StartGuards::new)
}

fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

/// What a start on this cloud computer checks against.
struct NodeStartFacts {
    options: terminal_service::StartOptions,
    installed: std::collections::HashSet<String>,
}

/// Folders (engine projects, terminal cwds, cloned workspaces, home), installed CLIs and modes.
/// A cloud computer has no Desktop safety switch: `remoteStart` is always true.
async fn node_start_facts(state: &Arc<DaemonState>) -> NodeStartFacts {
    use terminal_service::start::{build_options, FolderCandidate, OptionsInput, StartDevice, ENGINE_RUNTIME};
    let home = home_dir();
    let mut candidates: Vec<FolderCandidate> = Vec::new();
    let engine = async {
        let res = reqwest::Client::new()
            .get(format!("{GIZZI_ENGINE}/v1/session-start/options"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .ok()?;
        res.status().is_success().then_some(())?;
        res.json::<Value>().await.ok()
    }
    .await;
    let engine_up = engine.is_some();
    for f in engine.as_ref().and_then(|e| e["folders"].as_array()).into_iter().flatten() {
        if let Some(path) = f["path"].as_str() {
            candidates.push(FolderCandidate { path: path.into(), last_used: None });
        }
    }
    let listed = terminal_list(state, "").await;
    if let ResponseBody::Full(bytes) = &listed.body {
        let v: Value = serde_json::from_slice(bytes).unwrap_or_default();
        for t in v["terminals"].as_array().into_iter().flatten() {
            if let Some(cwd) = t["cwd"].as_str().filter(|c| !c.is_empty()) {
                candidates.push(FolderCandidate { path: cwd.into(), last_used: None });
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(home.join(START_WORKSPACES)) {
        for e in entries.flatten() {
            candidates.push(FolderCandidate { path: e.path(), last_used: None });
        }
    }
    candidates.push(FolderCandidate { path: home.clone(), last_used: None });
    // The CLIs cloud images ship; their mode flags and start argv come from
    // `terminal_service::harness_modes`, the table allternit-api reads too.
    let mut installed: std::collections::HashSet<String> =
        ["claude", "codex"].into_iter().filter(|p| on_path(p)).map(str::to_string).collect();
    let mut runtimes: Vec<String> = installed.iter().cloned().collect();
    runtimes.sort();
    if engine_up {
        installed.insert(ENGINE_RUNTIME.to_string());
    }
    let device = StartDevice {
        runtime_id: crate::identity::RuntimeIdentity::load(&state.config.identity_path)
            .map(|i| i.runtime_id)
            .unwrap_or_else(|_| "cloud".to_string()),
        name: std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|h| h.trim().to_string())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "Cloud computer".to_string()),
        os: "linux".to_string(),
        remote_start: true,
    };
    let input = OptionsInput { device, candidates, runtimes, home };
    let options = tokio::task::spawn_blocking(move || build_options(input))
        .await
        .expect("options builder does not panic");
    NodeStartFacts { options, installed }
}

/// `GET /api/v1/session-start/options`.
async fn session_start_options(state: &Arc<DaemonState>) -> HandlerResponse {
    let facts = node_start_facts(state).await;
    HandlerResponse::json(200, serde_json::to_value(&facts.options).unwrap_or_default())
}

fn start_error(e: terminal_service::StartError) -> HandlerResponse {
    HandlerResponse::json(e.status, e.body())
}

/// The workspace folder for a git start: `$HOME/workspaces/<repo>` (with a short suffix when
/// that name already holds another remote).
fn workspace_for(home: &std::path::Path, remote: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let clean = terminal_service::start::strip_credentials(remote);
    let repo: String = clean
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect();
    let repo = if repo.is_empty() || repo.starts_with('.') { "repo".to_string() } else { repo };
    let base = home.join(START_WORKSPACES).join(&repo);
    let origin = std::process::Command::new("git")
        .args(["-C", &base.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| terminal_service::start::strip_credentials(String::from_utf8_lossy(&o.stdout).trim()));
    if !base.exists() || origin.as_deref() == Some(clean.as_str()) {
        return base;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    clean.hash(&mut h);
    home.join(START_WORKSPACES).join(format!("{repo}-{:08x}", h.finish() as u32))
}

fn git(args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `folder.git` on a cloud computer: clone the branch (depth 1, the Code cloud-environment
/// clone, `agent_operations_routes::resolve_gc_workspace`), or bring an existing clone to it.
/// Any failure is `clone_failed`; a dirty checkout is never overwritten (checkout refuses).
pub fn clone_for_start(
    home: &std::path::Path,
    git_ref: &terminal_service::GitRef,
) -> Result<std::path::PathBuf, terminal_service::StartError> {
    let failed = terminal_service::StartError::CLONE_FAILED;
    if git_ref.remote.trim().is_empty() || git_ref.branch.trim().is_empty() || git_ref.branch.starts_with('-') || git_ref.remote.starts_with('-') {
        return Err(failed);
    }
    let dir = workspace_for(home, &git_ref.remote);
    let d = dir.to_string_lossy().into_owned();
    if dir.join(".git").is_dir() {
        let ok = git(&["-C", &d, "fetch", "--depth", "1", "origin", &git_ref.branch])
            && git(&["-C", &d, "checkout", "-B", &git_ref.branch, "FETCH_HEAD"]);
        return if ok { Ok(dir) } else { Err(failed) };
    }
    if dir.exists() {
        return Err(failed);
    }
    std::fs::create_dir_all(dir.parent().unwrap_or(home)).map_err(|_| failed)?;
    if git(&["clone", "--depth", "1", "--branch", &git_ref.branch, "--", &git_ref.remote, &d]) {
        Ok(dir)
    } else {
        let _ = std::fs::remove_dir_all(&dir);
        Err(failed)
    }
}

/// A terminal for a start: the pane engine when it answers, else a PTY.
async fn create_start_terminal(
    state: &Arc<DaemonState>,
    request: terminal_service::CreateTerminalRequest,
    argv: Vec<String>,
) -> Result<String, HandlerResponse> {
    if let Some(engine) = state.engine.get().await {
        return match engine.create(request, Some(argv)).await {
            Ok(id) => Ok(id),
            Err(reply) => Err(engine_reply(state, reply).await),
        };
    }
    state
        .terminals
        .create_with(pty_spec(&request))
        .map(|c| c.session_id)
        .map_err(|e| HandlerResponse::json(500, json!({ "error": "terminal_failed", "message": format!("unable to create terminal: {e}"), "action": "Try again." })))
}

/// `POST /api/v1/session-start`: every request here came through the relay. Runtime
/// `allternit` goes to the Gizzi engine; claude and codex open a terminal. `folder.git` clones.
async fn session_start(state: &Arc<DaemonState>, request: &InboundRequest) -> HandlerResponse {
    use terminal_service::start::{Mode, Planned, StartCtx, StartPlan, StartRequest, StartResponse, ENGINE_RUNTIME};
    let req: StartRequest = match serde_json::from_slice(&request.body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResponse::json(400, json!({ "error": "bad_request", "message": format!("Expected the session-start body: {e}"), "action": "Send {folder, runtime, origin}." }))
        }
    };
    let idem = request.header("idempotency-key").map(str::to_string);
    let facts = node_start_facts(state).await;
    let device_id = facts.options.device.runtime_id.clone();
    let mode = req
        .mode
        .unwrap_or_else(|| facts.options.modes.get(&req.runtime).map(|m| m.default).unwrap_or(Mode::Default));
    // The plan may clone (blocking git): run it off the async threads.
    let (planned, req) = {
        let device_id = device_id.clone();
        let idem = idem.clone();
        tokio::task::spawn_blocking(move || {
            let home = home_dir();
            let resolver: &dyn Fn(&terminal_service::GitRef) -> Result<std::path::PathBuf, terminal_service::StartError> =
                &|g| clone_for_start(&home, g);
            let signed_out = std::collections::HashSet::new();
            let ctx = StartCtx {
                remote_start_enabled: true,
                relayed: true,
                caller_mode: None,
                installed: &facts.installed,
                signed_out: &signed_out,
                modes: &facts.options.modes,
                folders: &facts.options.folders,
                git_resolver: Some(resolver),
            };
            (start_guards().plan(&device_id, idem.as_deref(), &req, &ctx), req)
        })
        .await
        .expect("start planning does not panic")
    };
    let plan = match planned {
        Err(e) => return start_error(e),
        Ok(Planned::Replay(body)) => return HandlerResponse::json(201, body),
        Ok(Planned::Fresh(plan)) => plan,
    };
    let body = match plan {
        StartPlan::Engine { directory, mode, prompt, origin } => {
            let mut forward = json!({
                "folder": { "path": directory.to_string_lossy() },
                "runtime": ENGINE_RUNTIME,
                "mode": mode.as_str(),
                "confirmBypass": req.confirm_bypass,
                "origin": origin,
            });
            if let Some(p) = prompt {
                forward["prompt"] = json!(p);
            }
            let res = match reqwest::Client::new()
                .post(format!("{GIZZI_ENGINE}/v1/session-start"))
                .timeout(Duration::from_secs(30))
                .json(&forward)
                .send()
                .await
            {
                Ok(r) => r,
                Err(_) => return start_error(terminal_service::StartError::RUNTIME_MISSING),
            };
            let status = res.status().as_u16();
            let reply: Value = res.json().await.unwrap_or_default();
            if !(200..300).contains(&status) {
                return HandlerResponse::json(status, reply);
            }
            reply
        }
        StartPlan::Terminal { argv, cwd, title, agent, send_after_start } => {
            let cwd = cwd.to_string_lossy().into_owned();
            let mut create = terminal_service::agent_request(&argv, &cwd, &title, 120, 32);
            create.env.insert("ALLTERNIT_SESSION_START_DEPTH".into(), req.origin.depth.unwrap_or(0).to_string());
            create.env.insert("ALLTERNIT_SESSION_START_MODE".into(), mode.as_str().to_string());
            let terminal_id = match create_start_terminal(state, create, argv).await {
                Ok(id) => id,
                Err(failed) => return failed,
            };
            if let Some(text) = send_after_start {
                let state = state.clone();
                let id = terminal_id.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(START_TYPE_PROMPT_AFTER).await;
                    for chunk in [text.as_str(), "\r"] {
                        let body = json!({ "content": chunk }).to_string();
                        let _ = terminal_action(&state, &format!("/terminal/{id}/input"), "input", body.as_bytes()).await;
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                });
            }
            serde_json::to_value(StartResponse::Terminal { terminal_id, cwd, agent }).unwrap_or_default()
        }
    };
    if let Some(key) = idem.as_deref() {
        start_guards().remember(&device_id, key, &body);
    }
    HandlerResponse::json(201, body)
}

/// `?preview=N` from a query string.
fn preview_param(query: &str) -> Option<u32> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "preview")
        .and_then(|(_, value)| value.parse().ok())
}

/// The `GET /terminal/list` body: `terminals` plus which backend served them
/// (`"pane"` or `"missing"`, so a client can tell a computer whose image has
/// no pane engine yet). PTY sessions the node still holds come after the
/// engine's terminals.
pub fn list_body(mut terminals: Vec<Value>, ptys: Vec<Value>, engine: &str) -> Value {
    terminals.extend(ptys);
    json!({ "terminals": terminals, "engine": engine })
}

/// `GET /terminal/list?preview=N`.
async fn terminal_list(state: &Arc<DaemonState>, query: &str) -> HandlerResponse {
    let ptys: Vec<Value> = state
        .terminals
        .list()
        .iter()
        .map(crate::terminal::list_entry)
        .collect();
    if let Some(engine) = state.engine.get().await {
        match engine.list(preview_param(query)).await {
            Ok(listed) => {
                let terminals = listed["terminals"].as_array().cloned().unwrap_or_default();
                return HandlerResponse::json(200, list_body(terminals, ptys, "pane"));
            }
            Err(reply) if reply.status == 503 => state.engine.forget().await,
            Err(reply) => return engine_reply(state, reply).await,
        }
    }
    HandlerResponse::json(200, list_body(Vec::new(), ptys, "missing"))
}

fn terminal_session_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/terminal/")?;
    for suffix in ["/input", "/resize", "/close", "/stream"] {
        if let Some(id) = rest.strip_suffix(suffix) {
            return Some(id);
        }
    }
    None
}

/// `POST /terminal/:id/{input,resize,close}`. A PTY id this node holds goes
/// to the PTY; any other id goes to the pane engine when there is one (which
/// answers 404 for an id it doesn't know), else to the PTY path as before.
async fn terminal_action(state: &Arc<DaemonState>, path: &str, action: &str, body: &[u8]) -> HandlerResponse {
    let Some(session_id) = terminal_session_id(path) else {
        return HandlerResponse::not_found("malformed terminal path");
    };
    if !state.terminals.contains(session_id) {
        if let Some(engine) = state.engine.get().await {
            let reply = match action {
                "input" => match serde_json::from_slice::<terminal_service::TerminalInputRequest>(body) {
                    Ok(request) => engine.input(session_id, &request.content).await,
                    Err(error) => {
                        return HandlerResponse::json(
                            400,
                            terminal_envelope(false, &format!("invalid input request: {error}"), None),
                        )
                    }
                },
                "resize" => match serde_json::from_slice::<terminal_service::TerminalResizeRequest>(body) {
                    Ok(request) => engine.resize(session_id, request.cols, request.rows).await,
                    Err(error) => {
                        return HandlerResponse::json(
                            400,
                            terminal_envelope(false, &format!("invalid resize request: {error}"), None),
                        )
                    }
                },
                // Old clients send no body; `{graceful: true}` asks the program to quit first.
                _ => {
                    let request = parse_body::<terminal_service::TerminalCloseRequest>(body).unwrap_or_default();
                    engine.close(session_id, request.graceful).await
                }
            };
            return engine_reply(state, reply).await;
        }
    }
    pty_action(state, session_id, action, body)
}

/// Today's PTY behaviour for input/resize/close.
fn pty_action(state: &Arc<DaemonState>, session_id: &str, action: &str, body: &[u8]) -> HandlerResponse {
    let result: Result<Value, String> = match action {
        "input" => {
            #[derive(Deserialize)]
            struct InputRequest {
                content: String,
            }
            match serde_json::from_slice::<InputRequest>(body) {
                Ok(request) => state
                    .terminals
                    .input(session_id, &request.content)
                    .map(|_| terminal_envelope(true, "Input forwarded", None))
                    .map_err(|error| error.to_string()),
                Err(error) => Err(format!("invalid input request: {error}")),
            }
        }
        "resize" => {
            #[derive(Deserialize)]
            struct ResizeRequest {
                cols: u16,
                rows: u16,
            }
            match serde_json::from_slice::<ResizeRequest>(body) {
                Ok(request) => state
                    .terminals
                    .resize(session_id, request.cols, request.rows)
                    .map(|_| terminal_envelope(true, "Terminal resized", None))
                    .map_err(|error| error.to_string()),
                Err(error) => Err(format!("invalid resize request: {error}")),
            }
        }
        _ => state
            .terminals
            .close(session_id)
            .map(|_| terminal_envelope(true, "Terminal session closed", None))
            .map_err(|error| error.to_string()),
    };
    match result {
        Ok(value) => HandlerResponse::json(200, value),
        Err(message) if message.contains("not found") => {
            HandlerResponse::json(404, terminal_envelope(false, &message, None))
        }
        Err(message) => HandlerResponse::json(500, terminal_envelope(false, &message, None)),
    }
}

/// True when an SSE chunk holds the terminal's `{"type":"exit",…}` event.
pub fn is_exit_event(chunk: &[u8]) -> bool {
    String::from_utf8_lossy(chunk)
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .any(|data| serde_json::from_str::<Value>(data).is_ok_and(|event| event["type"] == "exit"))
}

/// GET /terminal/:id/stream → SSE, routed like [`terminal_action`]. Engine
/// terminals replay the engine's scrollback (also after a node restart), then
/// live output, then the exit event (with `exit_code`); the response then
/// ends, the same frames and ending the PTY stream has always sent over the
/// relay (response_start + response_chunk* + response_end).
async fn terminal_stream(state: &Arc<DaemonState>, path: &str) -> HandlerResponse {
    let Some(session_id) = terminal_session_id(path) else {
        return HandlerResponse::not_found("malformed terminal path");
    };
    if !state.terminals.contains(session_id) {
        if let Some(engine) = state.engine.get().await {
            let mut stream = match engine.stream(session_id).await {
                Ok(stream) => stream,
                Err(reply) => return engine_reply(state, reply).await,
            };
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            tokio::spawn(async move {
                while let Some(Ok(chunk)) = stream.next().await {
                    let exit = is_exit_event(&chunk);
                    if tx.send(chunk.to_vec()).await.is_err() || exit {
                        return; // subscriber gone, or the program exited
                    }
                }
            });
            return HandlerResponse {
                status: 200,
                content_type: "text/event-stream",
                body: ResponseBody::Stream(rx, Some(session_id.to_string())),
            };
        }
    }
    pty_stream(state, session_id)
}

/// The PTY stream: replays the in-memory scrollback, then forwards live PTY
/// output until the pane exits (or the subscriber lags out and resyncs).
fn pty_stream(state: &Arc<DaemonState>, session_id: &str) -> HandlerResponse {
    if !state.terminals.alive(session_id) && state.terminals.scrollback(session_id).is_none() {
        return HandlerResponse::json(
            404,
            terminal_envelope(
                false,
                &format!("Terminal session '{session_id}' not found"),
                None,
            ),
        );
    }
    let Some(mut live) = state.terminals.subscribe(session_id) else {
        return HandlerResponse::json(
            404,
            terminal_envelope(
                false,
                &format!("Terminal session '{session_id}' not found"),
                None,
            ),
        );
    };
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let state = state.clone();
    let session_owned = session_id.to_string();
    tokio::spawn(async move {
        let session_id = session_owned;
        if let Some(scrollback) = state.terminals.scrollback(&session_id) {
            if !scrollback.is_empty() && tx.send(sse_data_event(&scrollback)).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                chunk = live.recv() => match chunk {
                    Ok(chunk) => {
                        if tx.send(sse_data_event(&chunk)).await.is_err() {
                            return; // subscriber gone
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Resync from scrollback after a lag.
                        if let Some(scrollback) = state.terminals.scrollback(&session_id) {
                            if tx.send(sse_data_event(&scrollback)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = tokio::time::sleep(Duration::from_secs(15)) => {
                    if tx.send(sse_ping_event()).await.is_err() {
                        return;
                    }
                }
            }
            if !state.terminals.alive(&session_id) {
                // Drain any final output already queued, then exit.
                while let Ok(chunk) = live.try_recv() {
                    if tx.send(sse_data_event(&chunk)).await.is_err() {
                        return;
                    }
                }
                break;
            }
        }
        let _ = tx.send(sse_exit_event()).await;
    });
    HandlerResponse {
        status: 200,
        content_type: "text/event-stream",
        body: ResponseBody::Stream(rx, Some(session_id.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> Arc<DaemonState> {
        let temp = tempfile::tempdir().unwrap();
        DaemonState::new(NodeConfig {
            fs_roots: vec![temp.path().to_path_buf()],
            // No pane engine: the tests never reach a real one on the host.
            pane_socket: Some(std::path::PathBuf::from("/nonexistent/allternit-node-test/herdr.sock")),
            ..NodeConfig::default()
        })
    }

    async fn call(state: &Arc<DaemonState>, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
        let response = handle(
            state,
            &InboundRequest {
                method: method.into(),
                path: path.into(),
                body: body.to_vec(),
                headers: Default::default(),
            },
        )
        .await;
        let ResponseBody::Full(bytes) = response.body else {
            panic!("{method} {path} must be buffered");
        };
        (response.status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn without_an_engine_list_says_engine_missing() {
        let state = test_state();
        let (status, body) = call(&state, "GET", "/terminal/list?preview=16", b"").await;
        assert_eq!(status, 200);
        assert_eq!(body, json!({ "terminals": [], "engine": "missing" }));
    }

    #[tokio::test]
    async fn without_an_engine_unknown_ids_are_404_and_close_still_succeeds() {
        let state = test_state();
        let (status, body) = call(&state, "POST", "/terminal/nope/input", br#"{"content":"ls\r"}"#).await;
        assert_eq!(status, 404);
        assert_eq!(body["success"], false);
        let (status, _) = call(&state, "GET", "/terminal/nope/stream", b"").await;
        assert_eq!(status, 404);
        let (status, body) = call(&state, "POST", "/terminal/nope/close", br#"{"graceful":true}"#).await;
        assert_eq!(status, 200);
        assert_eq!(body["success"], true);
    }

    #[tokio::test]
    async fn without_an_engine_a_bad_env_key_is_refused_before_spawning() {
        let state = test_state();
        let (status, body) = call(
            &state,
            "POST",
            "/terminal/create",
            br#"{"command":"claude","env":{"A=B":"1"}}"#,
        )
        .await;
        assert_eq!(status, 400);
        assert_eq!(body["success"], false);
        assert_eq!(state.terminals.session_count(), 0);
    }

    #[test]
    fn a_pty_list_item_has_the_engine_fields_with_no_pane() {
        let item = crate::terminal::list_entry(&crate::terminal::PtyListing {
            session_id: "s1".into(),
            title: None,
            cwd: Some("/home/me/proj/".into()),
            command: vec!["/bin/bash".into()],
            running: false,
            exit_code: Some(0),
            pid: Some(42),
            created_at: "2026-11-22T00:00:00+00:00".into(),
        });
        assert_eq!(
            item,
            json!({
                "session_id": "s1",
                "pane_id": null,
                "title": "proj",
                "cwd": "/home/me/proj/",
                "command": ["/bin/bash"],
                "running": false,
                "exit_code": 0,
                "pid": 42,
                "created_at": "2026-11-22T00:00:00+00:00",
            })
        );
        let untitled = crate::terminal::list_entry(&crate::terminal::PtyListing {
            session_id: "s2".into(),
            title: Some("  ".into()),
            cwd: None,
            command: vec![],
            running: true,
            exit_code: None,
            pid: None,
            created_at: String::new(),
        });
        assert_eq!(untitled["title"], "Terminal");
    }

    #[test]
    fn the_list_puts_engine_terminals_first_and_names_the_backend() {
        let body = list_body(vec![json!({ "session_id": "e1" })], vec![json!({ "session_id": "p1" })], "pane");
        assert_eq!(body["engine"], "pane");
        assert_eq!(body["terminals"][0]["session_id"], "e1");
        assert_eq!(body["terminals"][1]["session_id"], "p1");
    }

    #[test]
    fn the_engine_stream_ends_on_the_exit_event_only() {
        assert!(is_exit_event(b"data: {\"exit_code\":0,\"type\":\"exit\"}\n\n"));
        assert!(is_exit_event(b"data: {\"type\":\"exit\",\"exit_code\":null}\n\n"));
        assert!(!is_exit_event(b"data: {\"type\":\"data\",\"data\":\"type exit\"}\n\n"));
        assert!(!is_exit_event(b"event: ping\ndata: {}\n\n"));
    }

    #[tokio::test]
    async fn health_advertises_node_core_only() {
        let state = test_state();
        let response = handle(
            &state,
            &InboundRequest {
                method: "GET".into(),
                path: "/api/v1/node/health".into(),
                body: vec![],
                headers: Default::default(),
            },
        )
        .await;
        assert_eq!(response.status, 200);
        let ResponseBody::Full(body) = response.body else {
            panic!("health must be buffered");
        };
        let value: Value = serde_json::from_slice(&body).unwrap();
        let caps = value["capabilities"].as_array().unwrap();
        let caps: Vec<&str> = caps.iter().map(|c| c.as_str().unwrap()).collect();
        assert!(caps.contains(&"node.core"));
        assert!(caps.contains(&"runtime:execute"));
        assert!(!caps.contains(&"runtime:remote_control"));
        assert!(!caps.contains(&"providers:use"));
        assert_eq!(value["client"], "allternit-node");
    }

    #[tokio::test]
    async fn unknown_paths_answer_404_without_side_effects() {
        let state = test_state();
        for (method, path) in [
            ("GET", "/v1/remote-control/desktop/frame"),
            ("GET", "/api/aci/stream"),
            ("POST", "/api/v1/providers/video/generate"),
            ("GET", "/"),
        ] {
            let response = handle(
                &state,
                &InboundRequest {
                    method: method.into(),
                    path: path.into(),
                    body: vec![],
                    headers: Default::default(),
                },
            )
            .await;
            assert_eq!(response.status, 404, "{method} {path} must not be served");
        }
    }

    #[tokio::test]
    async fn exec_runs_commands_and_captures_output() {
        let state = test_state();
        let response = handle(
            &state,
            &InboundRequest {
                method: "POST".into(),
                path: "/api/v1/node/exec".into(),
                body: br#"{"command":"echo","args":["hello-node"]}"#.to_vec(),
                headers: Default::default(),
            },
        )
        .await;
        assert_eq!(response.status, 200);
        let ResponseBody::Full(body) = response.body else {
            panic!("exec must be buffered");
        };
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["stdout"], "hello-node\n");
        assert_eq!(value["exitCode"], 0);
    }

    #[tokio::test]
    async fn exec_times_out_and_reports_it() {
        let state = test_state();
        let response = handle(
            &state,
            &InboundRequest {
                method: "POST".into(),
                path: "/api/v1/node/exec".into(),
                body: br#"{"command":"sleep","args":["5"],"timeoutMs":200}"#.to_vec(),
                headers: Default::default(),
            },
        )
        .await;
        assert_eq!(response.status, 504);
    }

    #[tokio::test]
    async fn fs_scopes_writes_and_reads_to_roots() {
        let state = test_state();
        let root = state.config.fs_roots[0].clone();
        let target = root.join("sub").join("note.txt");
        let write = handle(
            &state,
            &InboundRequest {
                method: "POST".into(),
                path: "/api/v1/node/fs".into(),
                body: serde_json::to_vec(&json!({
                    "op": "write",
                    "path": target,
                    "content": "scoped",
                }))
                .unwrap(),
                headers: Default::default(),
            },
        )
        .await;
        assert_eq!(write.status, 200);

        let read = handle(
            &state,
            &InboundRequest {
                method: "POST".into(),
                path: "/api/v1/node/fs".into(),
                body: serde_json::to_vec(&json!({ "op": "read", "path": target })).unwrap(),
                headers: Default::default(),
            },
        )
        .await;
        let ResponseBody::Full(body) = read.body else {
            panic!("read must be buffered");
        };
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["content"], "scoped");

        // Outside the root → 403.
        let escape = handle(
            &state,
            &InboundRequest {
                method: "POST".into(),
                path: "/api/v1/node/fs".into(),
                body: serde_json::to_vec(&json!({ "op": "read", "path": "/etc/passwd" })).unwrap(),
                headers: Default::default(),
            },
        )
        .await;
        assert_eq!(escape.status, 403);
    }

    #[tokio::test]
    async fn processes_and_metrics_and_launch_endpoints() {
        let state = test_state();
        let response = handle(
            &state,
            &InboundRequest {
                method: "GET".into(),
                path: "/api/v1/node/processes".into(),
                body: vec![],
                headers: Default::default(),
            },
        )
        .await;
        assert_eq!(response.status, 200);

        let response = handle(
            &state,
            &InboundRequest {
                method: "POST".into(),
                path: "/api/v1/node/launch".into(),
                body: br#"{"action":"start"}"#.to_vec(),
                headers: Default::default(),
            },
        )
        .await;
        // The default desktop command cannot start in CI — the endpoint must
        // still answer deterministically (not panic, not 404).
        assert!(response.status == 200 || response.status == 500);
    }

    fn git_ok(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn session_start_git_folder_clones_into_workspaces_and_reuses_it() {
        let root = std::env::temp_dir().join(format!("allternit-node-start-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src").join("demo");
        let home = root.join("home");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        git_ok(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("README"), "hi").unwrap();
        git_ok(&src, &["add", "."]);
        git_ok(&src, &["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-q", "-m", "one"]);
        let remote = format!("file://{}", src.display());
        let git_ref = terminal_service::GitRef { remote: remote.clone(), branch: "main".into(), head: None };

        let dir = clone_for_start(&home, &git_ref).expect("clone");
        assert_eq!(dir, home.join("workspaces").join("demo"));
        assert!(dir.join("README").is_file());
        // A second start of the same remote reuses the clone.
        assert_eq!(clone_for_start(&home, &git_ref).expect("reuse"), dir);

        let missing = terminal_service::GitRef { remote: format!("file://{}/nope", root.display()), branch: "main".into(), head: None };
        assert_eq!(clone_for_start(&home, &missing).unwrap_err().error, "clone_failed");
        let flag = terminal_service::GitRef { remote, branch: "--upload-pack=x".into(), head: None };
        assert_eq!(clone_for_start(&home, &flag).unwrap_err().error, "clone_failed");
        let _ = std::fs::remove_dir_all(&root);
    }
}
