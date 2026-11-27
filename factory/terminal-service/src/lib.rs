//! The Allternit terminal API, served by allternit-api (and next by
//! allternit-node) over the Factory pane engine.
//!
//! Every terminal is a pane in the pane engine (`allternit-factory pane`, the
//! Factory's agent session), so a terminal tile, a Gizzi PTY and a Factory
//! agent pane are the same thing: it shows in the agent wall as workspace
//! `term-<id>`, and an agent started in it is detected like any other pane.
//!
//! - The pane engine owns the PTY: terminals survive restarts of the process
//!   that serves this API, and UI reconnects. The session id is the pane
//!   engine's terminal id, so nothing here has to be remembered or recovered.
//! - The stream replays the kept scrollback (up to 2 MiB), then live output,
//!   then `{"type":"exit","exit_code"}` when the shell exits (additive: older
//!   clients ignore unknown types). The connection then stays open with pings
//!   so an EventSource doesn't reconnect and replay again.
//! - The socket comes from `allternit-factory pane tty ensure` (which starts
//!   the engine when it is down), or `$ALLTERNIT_FACTORY_PANE_SOCKET`. The
//!   binary is `$ALLTERNIT_FACTORY_BIN`, else `allternit-factory` next to
//!   this executable, else on `PATH`.
//! - Transport: a Unix socket on macOS/Linux. On Windows the pane engine
//!   listens on a named pipe and `ensure` reports its marker file path; the
//!   pipe is `\\.\pipe\<that path>` (see [`pipe_name`]). Only the dial
//!   differs; the protocol is the same newline JSON.
//!
//! The crate has no web framework. [`TerminalService`] answers each call with
//! a [`Reply`] (HTTP status + JSON body) or, for the stream, the SSE bytes;
//! the caller's HTTP layer (axum in allternit-api) sends them as they are.

pub mod harness_modes;
mod list;
pub mod start;

pub use list::{last_rows, resolve_title, validate_env, ListQuery};
pub use start::{
    build_options, plan_start, FolderCandidate, FolderRef, GitRef, Mode, OptionsInput, Planned, StartCtx, StartError, StartGuards, StartOptions, StartPlan,
    StartRequest, StartResponse,
};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use list::{TerminalRecord, TerminalRecords};

// ─────────────────────────────────────────────────────────────────────────────
// Pane engine socket client
// ─────────────────────────────────────────────────────────────────────────────

/// One connection to the pane engine.
#[cfg(unix)]
type PaneStream = tokio::net::UnixStream;
#[cfg(windows)]
type PaneStream = tokio::net::windows::named_pipe::NamedPipeClient;

/// The named pipe the pane engine serves for `socket` (the path `pane tty
/// ensure` reports). Mirrors the pane's own mapping (interprocess
/// `GenericNamespaced` prepends `\\.\pipe\`); a value that already is a
/// local pipe path (e.g. a preset `$ALLTERNIT_FACTORY_PANE_SOCKET`) is kept.
pub fn pipe_name(socket: &std::path::Path) -> String {
    let s = socket.to_string_lossy();
    if s.starts_with(r"\\.\pipe\") {
        s.into_owned()
    } else {
        format!(r"\\.\pipe\{s}")
    }
}

#[cfg(unix)]
async fn dial(path: &std::path::Path) -> std::io::Result<PaneStream> {
    tokio::net::UnixStream::connect(path).await
}

#[cfg(windows)]
async fn dial(path: &std::path::Path) -> std::io::Result<PaneStream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    // ERROR_PIPE_BUSY: every server instance is taken; the engine creates a
    // new one per accepted connection, so wait briefly and retry.
    const ERROR_PIPE_BUSY: i32 = 231;
    let name = pipe_name(path);
    let mut tries = 0;
    loop {
        match ClientOptions::new().open(&name) {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 50 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Pane engine errors, mapped to HTTP statuses by [`PaneError::status`].
#[derive(Debug)]
pub enum PaneError {
    /// The engine can't be reached or started (503).
    Unavailable(String),
    /// No such terminal (404).
    NotFound(String),
    /// The engine refused the call (500).
    Engine(String),
}

impl PaneError {
    /// The HTTP status a route answers for this error.
    pub fn status(&self) -> u16 {
        match self {
            PaneError::Unavailable(_) => 503,
            PaneError::NotFound(_) => 404,
            PaneError::Engine(_) => 500,
        }
    }

    pub fn message(self) -> String {
        match self {
            PaneError::Unavailable(m) | PaneError::NotFound(m) | PaneError::Engine(m) => m,
        }
    }
}

/// The `allternit-factory` binary: `$ALLTERNIT_FACTORY_BIN`, else next to
/// this executable, else on `PATH`.
pub fn factory_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("ALLTERNIT_FACTORY_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(bin);
    }
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join(format!("allternit-factory{}", std::env::consts::EXE_SUFFIX)))
        })
        .filter(|p| p.is_file())
    {
        return sibling;
    }
    PathBuf::from("allternit-factory")
}

/// Where terminals live: the pane engine's socket, resolved once and again
/// whenever a connect fails (the engine restarted or was never started).
/// Cheap to clone: clones share the socket and the records.
#[derive(Clone)]
pub struct TerminalService {
    socket: Arc<Mutex<Option<PathBuf>>>,
    /// The `allternit-factory` binary (`None`: resolved by [`factory_bin`]).
    bin: Option<PathBuf>,
    /// What this service created (title, command, cwd, time), which the
    /// engine's terminal info lacks. Read by [`TerminalService::list`].
    pub(crate) records: TerminalRecords,
}

impl Default for TerminalService {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalService {
    pub fn new() -> Self {
        let preset = std::env::var_os("ALLTERNIT_FACTORY_PANE_SOCKET")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self {
            socket: Arc::new(Mutex::new(preset)),
            bin: None,
            records: Default::default(),
        }
    }

    /// A service pinned to one socket and binary (tests).
    pub fn with_socket(path: PathBuf, bin: PathBuf) -> Self {
        Self {
            socket: Arc::new(Mutex::new(Some(path))),
            bin: Some(bin),
            records: Default::default(),
        }
    }

    /// Runs `allternit-factory pane tty ensure`: starts the pane engine when
    /// it is down and reports its socket.
    async fn ensure(&self) -> Result<PathBuf, PaneError> {
        let bin = self.bin.clone().unwrap_or_else(factory_bin);
        let out = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(&bin)
                .args(["pane", "tty", "ensure"])
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| PaneError::Unavailable("the Factory pane engine did not start within 30s".into()))?
        .map_err(|e| {
            PaneError::Unavailable(format!(
                "the Factory pane engine could not be started ({}: {e})",
                bin.display()
            ))
        })?;
        if !out.status.success() {
            return Err(PaneError::Unavailable(format!(
                "the Factory pane engine could not be started: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| {
            PaneError::Unavailable(format!("unexpected output from {} pane tty ensure: {e}", bin.display()))
        })?;
        v["socket"]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(|| PaneError::Unavailable("pane tty ensure reported no socket".into()))
    }

    async fn connect(&self) -> Result<PaneStream, PaneError> {
        let mut socket = self.socket.lock().await;
        if let Some(path) = socket.as_ref() {
            if let Ok(stream) = dial(path).await {
                return Ok(stream);
            }
        }
        let path = self.ensure().await?;
        let stream = dial(&path).await.map_err(|e| {
            PaneError::Unavailable(format!("the Factory pane engine is not reachable at {}: {e}", path.display()))
        })?;
        *socket = Some(path);
        Ok(stream)
    }

    /// Opens a connection and sends one request (the pane engine serves one
    /// request per connection). Returns the reader, positioned at the
    /// response line.
    async fn open(&self, method: &str, params: Value) -> Result<BufReader<PaneStream>, PaneError> {
        let mut stream = self.connect().await?;
        let frame = json!({ "id": Uuid::new_v4().to_string(), "method": method, "params": params });
        let mut line = frame.to_string();
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|e| PaneError::Unavailable(format!("pane engine write: {e}")))?;
        Ok(BufReader::new(stream))
    }

    async fn read_response(reader: &mut BufReader<PaneStream>) -> Result<Value, PaneError> {
        let mut buf = String::new();
        let n = reader
            .read_line(&mut buf)
            .await
            .map_err(|e| PaneError::Unavailable(format!("pane engine read: {e}")))?;
        if n == 0 {
            return Err(PaneError::Unavailable("the pane engine closed the connection".into()));
        }
        let resp: Value = serde_json::from_str(buf.trim()).map_err(|e| PaneError::Engine(e.to_string()))?;
        if let Some(err) = resp.get("error") {
            let code = err["code"].as_str().unwrap_or("error");
            let msg = err["message"].as_str().unwrap_or("unknown").to_string();
            return Err(match code {
                "terminal_not_found" => PaneError::NotFound(msg),
                _ => PaneError::Engine(format!("{code}: {msg}")),
            });
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    /// One request, one response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, PaneError> {
        let mut reader = self.open(method, params).await?;
        Self::read_response(&mut reader).await
    }
}

fn default_shell() -> String {
    if cfg!(windows) {
        // Empty: the pane engine's default shell (PowerShell on Windows).
        String::new()
    } else {
        "/bin/zsh".into()
    }
}

/// The argv for a new terminal. Empty runs the pane engine's default shell.
/// On Windows a Unix shell path (the web UI defaults to `/bin/zsh`) can't
/// run, so it also means the default shell (PowerShell). Elsewhere an empty
/// shell, or an absolute path that isn't on this computer (zsh on a Linux
/// cloud computer), runs `$SHELL` or bash when there is one, instead of a
/// spawn that fails with ENOENT.
fn shell_command(shell: &str) -> Vec<String> {
    let shell = shell.trim();
    if cfg!(windows) && (shell.is_empty() || shell.starts_with('/')) {
        return Vec::new();
    }
    let exists = |path: &str| std::path::Path::new(path).exists();
    if !shell.is_empty() && (!shell.starts_with('/') || exists(shell)) {
        return vec![shell.to_string()];
    }
    std::env::var("SHELL")
        .ok()
        .filter(|path| path.starts_with('/') && exists(path))
        .or_else(|| exists("/bin/bash").then(|| "/bin/bash".to_string()))
        .map(|path| vec![path])
        .unwrap_or_default()
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

// ─────────────────────────────────────────────────────────────────────────────
// Request/response types (unchanged contract)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateTerminalRequest {
    #[serde(default = "default_shell")]
    pub shell: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    /// Run this program instead of the shell (with `args`).
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Added to the terminal's environment, over `TERM`/`COLORTERM`.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// The terminal's name in `GET /list` and the agent wall.
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TerminalMessageResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct TerminalInputRequest {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct TerminalResizeRequest {
    pub cols: u16,
    pub rows: u16,
}

/// `POST /:id/close` body (optional; old clients send none).
#[derive(Debug, Default, Deserialize)]
pub struct TerminalCloseRequest {
    /// Ask the terminal's program to quit (SIGTERM, up to 10 s) before the
    /// pane closes. Only for a terminal created with a `command`; a shell
    /// closes as before. Never SIGKILL.
    #[serde(default)]
    pub graceful: bool,
}

/// What a terminal call answers: the HTTP status and the JSON body
/// (`{success, message, data?}`) every terminal route sends.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: TerminalMessageResponse,
}

impl Reply {
    fn ok(message: &str, data: Option<Value>) -> Self {
        Reply {
            status: 200,
            body: TerminalMessageResponse {
                success: true,
                message: message.into(),
                data,
            },
        }
    }

    fn err(status: u16, message: String) -> Self {
        Reply {
            status,
            body: TerminalMessageResponse {
                success: false,
                message,
                data: None,
            },
        }
    }

    fn not_found(session_id: &str) -> Self {
        Reply::err(404, format!("Terminal session '{}' not found", session_id))
    }
}

impl From<PaneError> for Reply {
    fn from(err: PaneError) -> Self {
        let status = err.status();
        Reply::err(status, err.message())
    }
}

/// The `GET /:id/stream` body: `text/event-stream` bytes.
pub type TerminalStream = Pin<Box<dyn futures::Stream<Item = Result<Bytes, Infallible>> + Send>>;

fn sse_data_event(data: &str) -> String {
    let payload = serde_json::json!({ "type": "data", "data": data });
    format!("data: {}\n\n", payload)
}

fn sse_exit_event(exit_code: &Value) -> String {
    let payload = serde_json::json!({ "type": "exit", "exit_code": exit_code });
    format!("data: {}\n\n", payload)
}

/// Terminal ids double as session ids; reject anything else before it
/// reaches the engine.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The user's login shell, for running an agent command with the PATH their own
/// terminal has (`$SHELL`, else zsh).
pub fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|p| p.starts_with('/') && std::path::Path::new(p).exists())
        .unwrap_or_else(|| "/bin/zsh".to_string())
}

/// A create request that runs `argv` in `cwd` through the user's login shell
/// (`$SHELL -lc 'exec "$0" "$@"' argv…`: no quoting, the agent replaces the
/// shell, so the terminal's pid is the agent's).
pub fn agent_request(argv: &[String], cwd: &str, title: &str, cols: u16, rows: u16) -> CreateTerminalRequest {
    let mut args = vec!["-lc".to_string(), r#"exec "$0" "$@""#.to_string()];
    args.extend(argv.iter().cloned());
    CreateTerminalRequest {
        shell: String::new(),
        cwd: Some(cwd.to_string()),
        cols,
        rows,
        command: Some(login_shell()),
        args,
        env: HashMap::new(),
        title: Some(title.to_string()),
    }
}

const GRACEFUL_WAIT: Duration = Duration::from_secs(10);

// ─────────────────────────────────────────────────────────────────────────────
// The calls (one per route)
// ─────────────────────────────────────────────────────────────────────────────

impl TerminalService {
    /// `POST /create`: create a terminal and return its id. `shown`: the
    /// command `GET /list` reports when the launched argv wraps it (adopt runs
    /// the agent through the user's login shell).
    pub async fn create(&self, request: CreateTerminalRequest, shown: Option<Vec<String>>) -> Result<String, Reply> {
        if let Err(message) = validate_env(&request.env) {
            return Err(Reply::err(400, message));
        }
        let session_id = Uuid::new_v4().to_string();
        let cwd = request.cwd.clone().or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        });
        let program = request
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        let custom_command = program.is_some();
        let argv: Vec<String> = match program {
            Some(program) => std::iter::once(program.to_string())
                .chain(request.args.iter().cloned())
                .collect(),
            None => shell_command(&request.shell),
        };
        let title = request
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string);
        // Browser shells need a real terminal type whatever the engine inherited
        // (an engine started from a service can have TERM=dumb); the caller's env
        // goes over it.
        let mut env: HashMap<String, String> = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]);
        env.extend(request.env.clone());
        self.request(
            "factory.terminal.create",
            json!({
                "terminal_id": session_id,
                "label": title,
                "cwd": cwd,
                "cols": request.cols,
                "rows": request.rows,
                "command": argv,
                "env": env,
            }),
        )
        .await
        .map_err(Reply::from)?;
        self.records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                session_id.clone(),
                TerminalRecord {
                    title,
                    command: shown.unwrap_or(argv),
                    cwd,
                    created_at: chrono::Utc::now().to_rfc3339(),
                    custom_command,
                },
            );
        Ok(session_id)
    }

    /// `POST /:id/input`: raw bytes straight through (a real PTY treats '\r'
    /// as Enter).
    pub async fn input(&self, session_id: &str, content: &str) -> Reply {
        if !valid_session_id(session_id) {
            return Reply::not_found(session_id);
        }
        match self
            .request(
                "factory.terminal.write",
                json!({ "terminal_id": session_id, "data": content }),
            )
            .await
        {
            Ok(_) => Reply::ok("Input forwarded", None),
            Err(PaneError::NotFound(_)) => Reply::not_found(session_id),
            Err(err @ PaneError::Unavailable(_)) => err.into(),
            Err(err) => {
                // The shell exited or its queue is full: the input is dropped,
                // as typing into a closed terminal is.
                warn!(err = %err.message(), %session_id, "Failed to send terminal input");
                Reply::ok("Input forwarded", None)
            }
        }
    }

    /// `POST /:id/resize`.
    pub async fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Reply {
        if !valid_session_id(session_id) {
            return Reply::not_found(session_id);
        }
        match self
            .request(
                "factory.terminal.resize",
                json!({ "terminal_id": session_id, "cols": cols, "rows": rows }),
            )
            .await
        {
            Ok(_) => Reply::ok("Terminal resized", None),
            Err(PaneError::NotFound(_)) => Reply::not_found(session_id),
            Err(err @ PaneError::Unavailable(_)) => err.into(),
            Err(err) => {
                warn!(err = %err.message(), %session_id, "Failed to resize terminal");
                Reply::ok("Terminal resized", None)
            }
        }
    }

    /// The engine's view of a terminal: `(running, pid)`.
    async fn terminal_proc(&self, session_id: &str) -> Option<(bool, Option<u32>)> {
        let got = self
            .request("factory.terminal.get", json!({ "terminal_id": session_id }))
            .await
            .ok()?;
        let t = &got["terminal"];
        Some((
            t["running"].as_bool().unwrap_or(false),
            t["pid"].as_u64().and_then(|p| u32::try_from(p).ok()),
        ))
    }

    /// SIGTERM the program of a terminal this service started with a
    /// `command`, and wait up to 10 s for the engine to report it exited. The
    /// pid is the engine's own child, read again right before the signal.
    async fn stop_program(&self, session_id: &str) {
        let custom = self
            .records
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)
            .is_some_and(|r| r.custom_command);
        if !custom {
            return;
        }
        let Some((true, Some(pid))) = self.terminal_proc(session_id).await else { return };
        #[cfg(unix)]
        {
            // SAFETY: plain SIGTERM to the terminal's own running program.
            let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            if r != 0 {
                return;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            return;
        }
        let deadline = tokio::time::Instant::now() + GRACEFUL_WAIT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
            match self.terminal_proc(session_id).await {
                Some((true, Some(p))) if p == pid => continue,
                _ => return,
            }
        }
        warn!(%session_id, pid, "terminal program did not exit within 10 s of SIGTERM; closing the pane");
    }

    /// `POST /:id/close`: always a success (idempotent). `graceful`: SIGTERM
    /// a `command` terminal's program first (see [`TerminalCloseRequest`]).
    pub async fn close(&self, session_id: &str, graceful: bool) -> Reply {
        if valid_session_id(session_id) {
            if graceful {
                self.stop_program(session_id).await;
            }
            match self
                .request("factory.terminal.close", json!({ "terminal_id": session_id }))
                .await
            {
                Ok(_) | Err(PaneError::NotFound(_)) => {}
                Err(err) => warn!(err = %err.message(), %session_id, "Failed to close terminal"),
            }
        }
        Reply::ok("Terminal session closed", None)
    }

    /// `GET /:id/stream`: the SSE body (`data: {"type":"data"|"exit",…}` and
    /// `event: ping` every 15 s), or the error reply when the terminal or the
    /// engine isn't there.
    pub async fn stream(&self, session_id: &str) -> Result<TerminalStream, Reply> {
        if !valid_session_id(session_id) {
            return Err(Reply::not_found(session_id));
        }
        let mut reader = self
            .open(
                "factory.terminal.output",
                json!({ "terminal_id": session_id, "replay": true, "follow": true }),
            )
            .await
            .map_err(Reply::from)?;
        if let Err(err) = Self::read_response(&mut reader).await {
            return Err(match err {
                PaneError::NotFound(_) => Reply::not_found(session_id),
                err => err.into(),
            });
        }

        let stream = async_stream::stream! {
            let mut live = true;
            // Outside the loop: `read_line` keeps a partial line here when the
            // ping timer wins the select, so nothing is lost.
            let mut line = String::new();
            loop {
                tokio::select! {
                    read = reader.read_line(&mut line), if live => {
                        match read {
                            Ok(0) | Err(_) => break, // the engine went away
                            Ok(_) => {
                                let parsed = serde_json::from_str::<Value>(line.trim());
                                line.clear();
                                let frame: Value = match parsed {
                                    Ok(f) => f,
                                    Err(_) => continue,
                                };
                                match frame["type"].as_str() {
                                    Some("output") => {
                                        let chunk = frame["data"].as_str().unwrap_or("");
                                        if !chunk.is_empty() {
                                            yield Ok::<_, Infallible>(Bytes::from(sse_data_event(chunk)));
                                        }
                                    }
                                    Some("exit") => {
                                        yield Ok(Bytes::from(sse_exit_event(&frame["exit_code"])));
                                        // Stay open (pings only): ending the
                                        // stream would make the client reconnect
                                        // and replay the scrollback again.
                                        live = false;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(15)) => {
                        yield Ok(Bytes::from("event: ping\ndata: {}\n\n"));
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    #[test]
    fn pipe_name_prefixes_the_marker_path_like_the_pane_engine() {
        assert_eq!(
            pipe_name(std::path::Path::new(r"C:\Users\me\AppData\herdr\ao\herdr.sock")),
            r"\\.\pipe\C:\Users\me\AppData\herdr\ao\herdr.sock"
        );
        assert_eq!(pipe_name(std::path::Path::new(r"\\.\pipe\x")), r"\\.\pipe\x");
    }

    #[test]
    fn shell_command_runs_the_default_shell_when_empty() {
        if cfg!(windows) {
            assert!(shell_command("").is_empty());
            assert!(shell_command("  ").is_empty());
            assert!(shell_command("/bin/zsh").is_empty());
            assert_eq!(shell_command("pwsh.exe"), vec!["pwsh.exe".to_string()]);
        } else {
            assert_eq!(shell_command("/bin/sh"), vec!["/bin/sh".to_string()]);
            // Missing or empty: $SHELL or bash when this computer has one, else the engine's default.
            assert_eq!(shell_command("/no/such/shell"), shell_command(""));
            assert!(shell_command("").iter().all(|path| std::path::Path::new(path).exists()));
        }
    }
}
