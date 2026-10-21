//! Terminal API routes: Code Mode terminals backed by the Allternit Factory
//! pane engine.
//!
//! Same HTTP contract as before (create/input/close/resize/stream), but every
//! terminal is a pane in the pane engine (`allternit-factory pane`, the
//! Factory's agent session), so a terminal tile, a Gizzi PTY and a Factory
//! agent pane are the same thing: it shows in the agent wall as workspace
//! `term-<id>`, and an agent started in it is detected like any other pane.
//!
//! - The pane engine owns the PTY: terminals survive allternit-api restarts
//!   and UI reconnects. The session id is the pane engine's terminal id, so
//!   nothing here has to be remembered or recovered.
//! - The stream replays the kept scrollback (up to 2 MiB), then live output,
//!   then `{"type":"exit","exit_code"}` when the shell exits (additive: older
//!   clients ignore unknown types). The connection then stays open with pings
//!   so an EventSource doesn't reconnect and replay again.
//! - The socket comes from `allternit-factory pane tty ensure` (which starts
//!   the engine when it is down), or `$ALLTERNIT_FACTORY_PANE_SOCKET`. The
//!   binary is `$ALLTERNIT_FACTORY_BIN`, else `allternit-factory` next to
//!   this executable, else on `PATH`.

use axum::{
    body::Body,
    extract::{FromRef, Json, Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::AppState;

// ─────────────────────────────────────────────────────────────────────────────
// Pane engine socket client
// ─────────────────────────────────────────────────────────────────────────────

/// Pane engine errors, mapped to HTTP statuses at the route.
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
    fn status(&self) -> StatusCode {
        match self {
            PaneError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            PaneError::NotFound(_) => StatusCode::NOT_FOUND,
            PaneError::Engine(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn message(self) -> String {
        match self {
            PaneError::Unavailable(m) | PaneError::NotFound(m) | PaneError::Engine(m) => m,
        }
    }
}

fn factory_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("ALLTERNIT_FACTORY_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(bin);
    }
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("allternit-factory")))
        .filter(|p| p.is_file())
    {
        return sibling;
    }
    PathBuf::from("allternit-factory")
}

/// Where terminals live: the pane engine's socket, resolved once and again
/// whenever a connect fails (the engine restarted or was never started).
#[derive(Clone)]
pub struct TerminalSessionStore {
    socket: Arc<Mutex<Option<PathBuf>>>,
    /// The `allternit-factory` binary (`None`: resolved by [`factory_bin`]).
    bin: Option<PathBuf>,
}

impl Default for TerminalSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalSessionStore {
    pub fn new() -> Self {
        let preset = std::env::var_os("ALLTERNIT_FACTORY_PANE_SOCKET")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self {
            socket: Arc::new(Mutex::new(preset)),
            bin: None,
        }
    }

    /// A store pinned to one socket and binary (tests).
    pub fn with_socket(path: PathBuf, bin: PathBuf) -> Self {
        Self {
            socket: Arc::new(Mutex::new(Some(path))),
            bin: Some(bin),
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

    async fn connect(&self) -> Result<UnixStream, PaneError> {
        let mut socket = self.socket.lock().await;
        if let Some(path) = socket.as_ref() {
            if let Ok(stream) = UnixStream::connect(path).await {
                return Ok(stream);
            }
        }
        let path = self.ensure().await?;
        let stream = UnixStream::connect(&path).await.map_err(|e| {
            PaneError::Unavailable(format!("the Factory pane engine is not reachable at {}: {e}", path.display()))
        })?;
        *socket = Some(path);
        Ok(stream)
    }

    /// Opens a connection and sends one request (the pane engine serves one
    /// request per connection). Returns the reader, positioned at the
    /// response line.
    async fn open(&self, method: &str, params: Value) -> Result<BufReader<UnixStream>, PaneError> {
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

    async fn read_response(reader: &mut BufReader<UnixStream>) -> Result<Value, PaneError> {
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
    "/bin/zsh".into()
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

fn sse_data_event(data: &str) -> String {
    let payload = serde_json::json!({ "type": "data", "data": data });
    format!("data: {}\n\n", payload)
}

fn sse_exit_event(exit_code: &Value) -> String {
    let payload = serde_json::json!({ "type": "exit", "exit_code": exit_code });
    format!("data: {}\n\n", payload)
}

fn err_response(status: StatusCode, message: String) -> (StatusCode, Json<TerminalMessageResponse>) {
    (
        status,
        Json(TerminalMessageResponse {
            success: false,
            message,
            data: None,
        }),
    )
}

fn pane_err(err: PaneError) -> (StatusCode, Json<TerminalMessageResponse>) {
    let status = err.status();
    err_response(status, err.message())
}

fn ok(message: &str, data: Option<Value>) -> (StatusCode, Json<TerminalMessageResponse>) {
    (
        StatusCode::OK,
        Json(TerminalMessageResponse {
            success: true,
            message: message.into(),
            data,
        }),
    )
}

/// Terminal ids double as session ids; reject anything else before it
/// reaches the engine.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn not_found(session_id: &str) -> (StatusCode, Json<TerminalMessageResponse>) {
    err_response(
        StatusCode::NOT_FOUND,
        format!("Terminal session '{}' not found", session_id),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Routes
// ─────────────────────────────────────────────────────────────────────────────

impl FromRef<Arc<AppState>> for TerminalSessionStore {
    fn from_ref(state: &Arc<AppState>) -> Self {
        state.terminal_sessions.clone()
    }
}

pub fn terminal_router() -> Router<Arc<AppState>> {
    routes()
}

/// The routes over any state that holds the store (tests use the store
/// alone).
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    TerminalSessionStore: FromRef<S>,
{
    Router::new()
        .route("/create", post(create_terminal))
        .route("/:session_id/input", post(terminal_input))
        .route("/:session_id/close", post(terminal_close))
        .route("/:session_id/resize", post(terminal_resize))
        .route("/:session_id/stream", get(terminal_stream))
}

async fn create_terminal(
    State(store): State<TerminalSessionStore>,
    Json(request): Json<CreateTerminalRequest>,
) -> impl IntoResponse {
    let session_id = Uuid::new_v4().to_string();
    let cwd = request.cwd.clone().or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    });
    let created = store
        .request(
            "factory.terminal.create",
            json!({
                "terminal_id": session_id,
                "cwd": cwd,
                "cols": request.cols,
                "rows": request.rows,
                "command": [request.shell],
                // Browser shells need a real terminal type whatever the
                // engine inherited (an engine started from a service can
                // have TERM=dumb).
                "env": { "TERM": "xterm-256color", "COLORTERM": "truecolor" },
            }),
        )
        .await;
    if let Err(err) = created {
        return pane_err(err);
    }
    debug!(%session_id, "Terminal session created (factory pane)");
    ok("Terminal session created", Some(json!({ "session_id": session_id })))
}

async fn terminal_input(
    State(store): State<TerminalSessionStore>,
    Path(session_id): Path<String>,
    Json(request): Json<TerminalInputRequest>,
) -> impl IntoResponse {
    if !valid_session_id(&session_id) {
        return not_found(&session_id);
    }
    // Raw bytes straight through: a real PTY treats '\r' as Enter.
    match store
        .request(
            "factory.terminal.write",
            json!({ "terminal_id": session_id, "data": request.content }),
        )
        .await
    {
        Ok(_) => ok("Input forwarded", None),
        Err(PaneError::NotFound(_)) => not_found(&session_id),
        Err(err @ PaneError::Unavailable(_)) => pane_err(err),
        Err(err) => {
            // The shell exited or its queue is full: the input is dropped,
            // as typing into a closed terminal is.
            warn!(err = %err.message(), %session_id, "Failed to send terminal input");
            ok("Input forwarded", None)
        }
    }
}

async fn terminal_resize(
    State(store): State<TerminalSessionStore>,
    Path(session_id): Path<String>,
    Json(request): Json<TerminalResizeRequest>,
) -> impl IntoResponse {
    if !valid_session_id(&session_id) {
        return not_found(&session_id);
    }
    match store
        .request(
            "factory.terminal.resize",
            json!({ "terminal_id": session_id, "cols": request.cols, "rows": request.rows }),
        )
        .await
    {
        Ok(_) => ok("Terminal resized", None),
        Err(PaneError::NotFound(_)) => not_found(&session_id),
        Err(err @ PaneError::Unavailable(_)) => pane_err(err),
        Err(err) => {
            warn!(err = %err.message(), %session_id, "Failed to resize terminal");
            ok("Terminal resized", None)
        }
    }
}

async fn terminal_close(
    State(store): State<TerminalSessionStore>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    if valid_session_id(&session_id) {
        match store
            .request("factory.terminal.close", json!({ "terminal_id": session_id }))
            .await
        {
            Ok(_) | Err(PaneError::NotFound(_)) => {}
            Err(err) => warn!(err = %err.message(), %session_id, "Failed to close terminal"),
        }
    }
    ok("Terminal session closed", None)
}

async fn terminal_stream(
    State(store): State<TerminalSessionStore>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    if !valid_session_id(&session_id) {
        return not_found(&session_id).into_response();
    }
    let opened = store
        .open(
            "factory.terminal.output",
            json!({ "terminal_id": session_id, "replay": true, "follow": true }),
        )
        .await;
    let mut reader = match opened {
        Ok(r) => r,
        Err(err) => return pane_err(err).into_response(),
    };
    if let Err(err) = TerminalSessionStore::read_response(&mut reader).await {
        return match err {
            PaneError::NotFound(_) => not_found(&session_id).into_response(),
            err => pane_err(err).into_response(),
        };
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
                                        yield Ok::<_, std::convert::Infallible>(Bytes::from(sse_data_event(chunk)));
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

    let body = Body::from_stream(stream);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    //! The routes against a fake pane engine that speaks the real wire
    //! protocol (one request per connection, `factory.terminal.*`) and echoes
    //! input back as output. The real engine is covered by the pane crate's
    //! tests and the smoke boot.
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use tokio::net::UnixListener;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    #[derive(Default)]
    struct FakeTerm {
        output: String,
        size: (u64, u64),
        exit: Option<i64>,
    }

    #[derive(Default)]
    struct Fake {
        terms: std::sync::Mutex<HashMap<String, FakeTerm>>,
        changed: Notify,
    }

    async fn serve_conn(fake: Arc<Fake>, stream: UnixStream) {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read);
        let mut line = String::new();
        if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let req: Value = serde_json::from_str(line.trim()).unwrap();
        let id = req["id"].clone();
        let p = &req["params"];
        let tid = p["terminal_id"].as_str().unwrap_or_default().to_string();
        let missing = json!({ "id": id, "error": { "code": "terminal_not_found", "message": format!("terminal {tid} not found") } });
        let reply = |v: Value| format!("{v}\n");
        let resp = {
            let mut terms = fake.terms.lock().unwrap();
            match req["method"].as_str().unwrap() {
                "factory.terminal.create" => {
                    assert_eq!(p["env"]["TERM"], "xterm-256color");
                    terms.insert(tid.clone(), FakeTerm {
                        output: "$ ".into(),
                        size: (p["cols"].as_u64().unwrap(), p["rows"].as_u64().unwrap()),
                        exit: None,
                    });
                    Some(json!({ "id": id, "result": { "terminal": { "terminal_id": tid, "running": true } } }))
                }
                "factory.terminal.write" => match terms.get_mut(&tid) {
                    Some(t) => {
                        let data = p["data"].as_str().unwrap();
                        if data == "exit\r" {
                            t.exit = Some(0);
                        } else {
                            t.output.push_str(&data.replace('\r', "\r\n"));
                        }
                        Some(json!({ "id": id, "result": {} }))
                    }
                    None => Some(missing.clone()),
                },
                "factory.terminal.resize" => match terms.get_mut(&tid) {
                    Some(t) => {
                        t.size = (p["cols"].as_u64().unwrap(), p["rows"].as_u64().unwrap());
                        Some(json!({ "id": id, "result": {} }))
                    }
                    None => Some(missing.clone()),
                },
                "factory.terminal.close" => match terms.remove(&tid) {
                    Some(_) => Some(json!({ "id": id, "result": {} })),
                    None => Some(missing.clone()),
                },
                "factory.terminal.output" => {
                    if terms.contains_key(&tid) {
                        None
                    } else {
                        Some(missing.clone())
                    }
                }
                other => panic!("unexpected method {other}"),
            }
        };
        fake.changed.notify_waiters();
        if let Some(resp) = resp {
            let _ = write.write_all(reply(resp).as_bytes()).await;
            return;
        }
        // factory.terminal.output: replay, then follow until exit/close.
        let _ = write.write_all(reply(json!({ "id": id, "result": { "terminal_id": tid } })).as_bytes()).await;
        let mut sent = 0;
        loop {
            let notified = fake.changed.notified();
            let (chunk, exit, gone) = {
                let terms = fake.terms.lock().unwrap();
                match terms.get(&tid) {
                    Some(t) => (t.output[sent..].to_string(), t.exit, false),
                    None => (String::new(), None, true),
                }
            };
            if gone {
                return;
            }
            if !chunk.is_empty() {
                sent += chunk.len();
                if write.write_all(reply(json!({ "type": "output", "data": chunk })).as_bytes()).await.is_err() {
                    return;
                }
            }
            if let Some(code) = exit {
                let _ = write.write_all(reply(json!({ "type": "exit", "exit_code": code })).as_bytes()).await;
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(200), notified).await;
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        fake: Arc<Fake>,
        app: Router,
    }

    fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("pane.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let fake = Arc::new(Fake::default());
        let f = fake.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_conn(f.clone(), stream));
            }
        });
        let store = TerminalSessionStore::with_socket(sock, dir.path().join("no-allternit-factory"));
        Harness { _dir: dir, fake, app: routes().with_state(store) }
    }

    async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::post(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn create(app: &Router) -> String {
        let (status, body) = post(app, "/create", json!({ "cols": 120, "rows": 40 })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["success"], true);
        body["data"]["session_id"].as_str().unwrap().to_string()
    }

    /// Reads the SSE stream until `until` is in the decoded `data` text (or
    /// an exit event when `until` is `None`); returns the text and exit code.
    async fn read_stream(app: &Router, id: &str, until: Option<&str>) -> (String, Option<Value>) {
        let resp = app
            .clone()
            .oneshot(Request::get(format!("/{id}/stream")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        let mut body = resp.into_body();
        let mut raw = String::new();
        let mut text = String::new();
        let mut exit = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let frame = tokio::time::timeout_at(deadline, body.frame()).await.expect("stream timed out");
            let Some(Ok(frame)) = frame else { break };
            if let Ok(data) = frame.into_data() {
                raw.push_str(&String::from_utf8_lossy(&data));
            }
            while let Some(end) = raw.find("\n\n") {
                let event: String = raw.drain(..end + 2).collect();
                if let Some(payload) = event.strip_prefix("data: ") {
                    let v: Value = serde_json::from_str(payload.trim()).unwrap();
                    match v["type"].as_str() {
                        Some("data") => text.push_str(v["data"].as_str().unwrap()),
                        Some("exit") => exit = Some(v["exit_code"].clone()),
                        _ => {}
                    }
                }
            }
            match until {
                Some(needle) if text.contains(needle) => return (text, exit),
                None if exit.is_some() => return (text, exit),
                _ => {}
            }
        }
        (text, exit)
    }

    #[tokio::test]
    async fn create_write_read_resize_close() {
        let h = harness();
        let id = create(&h.app).await;
        assert_eq!(h.fake.terms.lock().unwrap()[&id].size, (120, 40));

        let (status, _) = post(&h.app, &format!("/{id}/input"), json!({ "content": "echo hi\r" })).await;
        assert_eq!(status, StatusCode::OK);
        let (text, _) = read_stream(&h.app, &id, Some("echo hi\r\n")).await;
        assert!(text.starts_with("$ echo hi"), "{text:?}");

        let (status, body) = post(&h.app, &format!("/{id}/resize"), json!({ "cols": 100, "rows": 30 })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(h.fake.terms.lock().unwrap()[&id].size, (100, 30));

        let (status, body) = post(&h.app, &format!("/{id}/close"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert!(!h.fake.terms.lock().unwrap().contains_key(&id));

        // Gone after close.
        let (status, _) = post(&h.app, &format!("/{id}/input"), json!({ "content": "x" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // Closing again is still a success (idempotent, as before).
        let (status, _) = post(&h.app, &format!("/{id}/close"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn reconnect_replays_scrollback_and_exit_is_reported() {
        let h = harness();
        let id = create(&h.app).await;
        post(&h.app, &format!("/{id}/input"), json!({ "content": "one\r" })).await;
        let (first, _) = read_stream(&h.app, &id, Some("one\r\n")).await;
        // A second connection (a UI reconnect) gets the same scrollback.
        let (again, _) = read_stream(&h.app, &id, Some("one\r\n")).await;
        assert_eq!(first, again);

        post(&h.app, &format!("/{id}/input"), json!({ "content": "exit\r" })).await;
        let (_, exit) = read_stream(&h.app, &id, None).await;
        assert_eq!(exit, Some(json!(0)));
    }

    #[tokio::test]
    async fn two_terminals_are_independent() {
        let h = harness();
        let a = create(&h.app).await;
        let b = create(&h.app).await;
        assert_ne!(a, b);
        post(&h.app, &format!("/{a}/input"), json!({ "content": "alpha\r" })).await;
        post(&h.app, &format!("/{b}/input"), json!({ "content": "beta\r" })).await;
        let (ta, _) = read_stream(&h.app, &a, Some("alpha\r\n")).await;
        let (tb, _) = read_stream(&h.app, &b, Some("beta\r\n")).await;
        assert!(!ta.contains("beta"), "{ta:?}");
        assert!(!tb.contains("alpha"), "{tb:?}");
        post(&h.app, &format!("/{a}/close"), json!({})).await;
        // Closing one leaves the other running.
        let (status, _) = post(&h.app, &format!("/{b}/resize"), json!({ "cols": 90, "rows": 20 })).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_terminal_is_404() {
        let h = harness();
        let resp = h
            .app
            .clone()
            .oneshot(Request::get("/nope/stream").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let (status, _) = post(&h.app, "/nope/resize", json!({ "cols": 80, "rows": 24 })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = post(&h.app, "/bad%2Fid/input", json!({ "content": "x" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn engine_down_is_503_not_a_silent_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = TerminalSessionStore::with_socket(
            dir.path().join("absent.sock"),
            dir.path().join("no-allternit-factory"),
        );
        let app: Router = routes().with_state(store);
        let (status, body) = post(&app, "/create", json!({})).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["success"], false);
        assert!(body["message"].as_str().unwrap().contains("pane engine"), "{body}");
    }
}
