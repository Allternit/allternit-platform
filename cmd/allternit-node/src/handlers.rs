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
use crate::launch::{self, LaunchAction};
use crate::terminal::TerminalStore;

/// One inbound relay request handed to the handlers.
#[derive(Debug, Clone)]
pub struct InboundRequest {
    pub method: String,
    /// Path including any query string.
    pub path: String,
    pub body: Vec<u8>,
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
    pub terminals: TerminalStore,
    pub started: Instant,
    pub relay: RelayStats,
}

impl DaemonState {
    pub fn new(config: NodeConfig) -> Arc<Self> {
        Arc::new(Self {
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
    let (path, _query) = request
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
        ("POST", "/terminal/create") => terminal_create(state, &request.body),
        ("POST", path) if path.starts_with("/terminal/") && path.ends_with("/input") => {
            terminal_action(state, path, "input", &request.body)
        }
        ("POST", path) if path.starts_with("/terminal/") && path.ends_with("/resize") => {
            terminal_action(state, path, "resize", &request.body)
        }
        ("POST", path) if path.starts_with("/terminal/") && path.ends_with("/close") => {
            terminal_action(state, path, "close", &request.body)
        }
        ("GET", path) if path.starts_with("/terminal/") && path.ends_with("/stream") => {
            terminal_stream(state, path)
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

#[derive(Debug, Deserialize)]
struct CreateTerminalRequest {
    #[serde(default = "default_shell")]
    shell: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default = "default_cols")]
    cols: u16,
    #[serde(default = "default_rows")]
    rows: u16,
}

fn default_shell() -> String {
    "/bin/zsh".to_string()
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

fn default_cols() -> u16 {
    80
}
fn default_rows() -> u16 {
    24
}

fn terminal_create(state: &Arc<DaemonState>, body: &[u8]) -> HandlerResponse {
    let request: CreateTerminalRequest = serde_json::from_slice(body).unwrap_or(CreateTerminalRequest {
        shell: default_shell(),
        cwd: None,
        cols: default_cols(),
        rows: default_rows(),
    });
    let shell = resolve_shell(&request.shell);
    match state.terminals.create(&shell, request.cwd.as_deref(), request.cols, request.rows) {
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

fn terminal_session_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/terminal/")?;
    for suffix in ["/input", "/resize", "/close", "/stream"] {
        if let Some(id) = rest.strip_suffix(suffix) {
            return Some(id);
        }
    }
    None
}

fn terminal_action(state: &Arc<DaemonState>, path: &str, action: &str, body: &[u8]) -> HandlerResponse {
    let Some(session_id) = terminal_session_id(path) else {
        return HandlerResponse::not_found("malformed terminal path");
    };
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

/// GET /terminal/:id/stream → SSE. Replays the in-memory scrollback, then
/// forwards live PTY output until the pane exits (or the subscriber lags out
/// and resyncs). The relay carries this as response_start + response_chunk*.
fn terminal_stream(state: &Arc<DaemonState>, path: &str) -> HandlerResponse {
    let Some(session_id) = terminal_session_id(path) else {
        return HandlerResponse::not_found("malformed terminal path");
    };
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
            ..NodeConfig::default()
        })
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
            },
        )
        .await;
        // The default desktop command cannot start in CI — the endpoint must
        // still answer deterministically (not panic, not 404).
        assert!(response.status == 200 || response.status == 500);
    }
}
