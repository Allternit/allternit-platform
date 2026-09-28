//! Computer real-time plane (Phase 3): interactive PTY over WebSocket, guest
//! event stream (live + history), and the authenticated in-VM HTTP proxy.
//!
//! Guest-side helpers (PTY bridge, event collector) are plain python3 scripts
//! pushed into the guest on first use and started with `nohup` — no image
//! rebuild required (precedent: mux bootstrap in `bot_desktop_mux`). Bridge
//! tokens are random per bootstrap, kept in memory in `AppState`, and never
//! persisted; a stale bridge is detected via connection refusal and
//! re-bootstrapped once.
//!
//! Substrate support: Incus only in v1. Other providers surface an honest
//! 501 (drivers whose `guest_service_url` returns `NotSupported`).

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures::{sink::SinkExt, stream::StreamExt};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::auth::AuthUser;
use crate::computer_routes::{fetch_computer, ApprovalQuery, ComputerResponse, ComputerStatus};
use crate::AppState;
use allternit_driver_interface::{CommandSpec, DriverError, ExecutionDriver, ExecutionHandle};

const PTY_GUEST_PORT: u16 = 6010;
const PTY_BRIDGE_PATH: &str = "/tmp/allternit-pty-bridge.py";
const EVENTS_COLLECTOR_PATH: &str = "/tmp/allternit-events.py";
const EVENTS_LOG_PATH: &str = "/tmp/allternit-events.jsonl";
const WS_TOKEN_TTL_SECONDS: u64 = 300;
const PROXY_BODY_LIMIT: usize = 10 * 1024 * 1024;

/// Python PTY bridge: listens in the guest, forks a bash PTY per connection,
/// pumps both directions. The first line of every connection must be the
/// bridge token (issued per bootstrap, passed as argv). A one-line JSON
/// control prefix `{"cols":N,"rows":M}` resizes the PTY.
const PTY_BRIDGE_SCRIPT: &str = r#"#!/usr/bin/env python3
"""Allternit PTY bridge: TCP -> bash over PTY, one client at a time."""
import fcntl, json, os, pty, select, socket, struct, sys, termios

def handle_control(line, fd):
    try:
        msg = json.loads(line.decode("utf-8", "replace"))
        cols, rows = int(msg["cols"]), int(msg["rows"])
        if cols <= 0 or rows <= 0 or cols > 1000 or rows > 1000:
            return
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    except (ValueError, KeyError, TypeError, OSError):
        pass

def pump(conn, fd):
    while True:
        ready, _, _ = select.select([conn, fd], [], [])
        if conn in ready:
            data = conn.recv(65536)
            if not data:
                return
            if data.startswith(b"{"):
                end = data.find(b"\n")
                if end != -1:
                    handle_control(data[:end], fd)
                    data = data[end + 1:]
                    if not data:
                        continue
            os.write(fd, data)
        if fd in ready:
            try:
                out = os.read(fd, 65536)
            except OSError:
                return
            if not out:
                return
            conn.sendall(out)

def main():
    if len(sys.argv) < 3:
        sys.stderr.write("usage: pty-bridge.py <port> <token>\n")
        return 2
    port, token = int(sys.argv[1]), sys.argv[2]
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(1)
    while True:
        conn, _addr = srv.accept()
        conn.settimeout(30)
        line = b""
        while line is not None and not line.endswith(b"\n") and len(line) <= 256:
            try:
                chunk = conn.recv(1024)
            except OSError:
                chunk = b""
            if not chunk:
                line = None
                break
            line += chunk
        if not line or line.decode("utf-8", "replace").strip() != token:
            try:
                conn.sendall(b"invalid bridge token\n")
            except OSError:
                pass
            conn.close()
            continue
        conn.settimeout(None)
        pid, fd = pty.fork()
        if pid == 0:
            os.environ.setdefault("TERM", "xterm-256color")
            os.execvp("bash", ["bash"])
        try:
            pump(conn, fd)
        finally:
            for closer in (lambda: os.close(fd), conn.close):
                try:
                    closer()
                except OSError:
                    pass
            try:
                os.kill(pid, 9)
                os.waitpid(pid, 0)
            except OSError:
                pass
    return 0

if __name__ == "__main__":
    sys.exit(main())
"#;

/// Guest event collector: appends JSON lines (`{"ts","type","data"}`) to the
/// events log every 2s. Missing tools are tolerated (that event type is
/// skipped and logged once).
const EVENTS_COLLECTOR_SCRIPT: &str = r#"#!/usr/bin/env python3
"""Allternit guest event collector: window/clipboard/idle/processes/files."""
import json, os, subprocess, sys, time

OUT = "/tmp/allternit-events.jsonl"
INTERVAL = 2.0

def have(cmd):
    try:
        return subprocess.call(["sh", "-c", "command -v " + cmd + " >/dev/null 2>&1"]) == 0
    except OSError:
        return False

def run(argv):
    try:
        out = subprocess.check_output(argv, stderr=subprocess.DEVNULL, timeout=3)
        return out.decode("utf-8", "replace").strip()
    except Exception:
        return None

def emit(type_, data):
    try:
        with open(OUT, "a") as f:
            f.write(json.dumps({"ts": int(time.time()), "type": type_, "data": data}) + "\n")
    except OSError:
        pass

def main():
    tools = {name: have(name) for name in ("xdotool", "xclip", "xprintidle")}
    missing = [k for k, v in tools.items() if not v]
    if missing:
        sys.stderr.write("missing tools, skipping types: " + ",".join(missing) + "\n")
    last_clip = None
    known_files = set()
    tick = 0
    while True:
        tick += 1
        if tools["xdotool"]:
            wid = run(["xdotool", "getactivewindow"])
            title = run(["xdotool", "getwindowname", wid]) if wid else None
            if title:
                emit("window", {"title": title})
        if tools["xclip"]:
            clip = run(["xclip", "-o", "-selection", "clipboard"])
            if clip is not None and clip != last_clip:
                last_clip = clip
                emit("clipboard", {"text": clip[:4096]})
        if tools["xprintidle"]:
            idle = run(["xprintidle"])
            emit("idle_ms", {"idle_ms": int(idle) if idle and idle.isdigit() else 0})
        else:
            emit("idle_ms", {"idle_ms": 0})
        if tick % 15 == 0:
            psout = run(["sh", "-c", "ps -eo pcpu,comm --sort=-pcpu | head -6 | tail -5"])
            if psout:
                procs = []
                for ln in psout.splitlines():
                    parts = ln.split(None, 1)
                    if len(parts) == 2:
                        procs.append({"cpu": parts[0], "name": parts[1]})
                emit("processes", {"processes": procs})
        if tick % 3 == 2:
            home = os.path.expanduser("~")
            for sub in ("Desktop", "Downloads"):
                d = os.path.join(home, sub)
                if not os.path.isdir(d):
                    continue
                try:
                    for name in os.listdir(d):
                        p = os.path.join(d, name)
                        if p not in known_files:
                            known_files.add(p)
                            emit("files", {"path": p, "dir": sub})
                except OSError:
                    pass
        time.sleep(INTERVAL)

if __name__ == "__main__":
    main()
"#;

// ── Routers ──────────────────────────────────────────────────────────────────

/// WebSocket routes, mounted at `/ws/computers` next to the VNC ws route.
/// Interactive planes (pty/events) stay behind Clerk auth: the ws token only
/// authorizes the stream, the session authorizes the user.
pub fn computer_ws_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:id/pty", get(computer_pty_ws_handler))
        .route("/:id/events", get(computer_events_ws_handler))
}

/// PUBLIC VNC route, mounted at `/ws/computers` WITHOUT the auth middleware:
/// the HMAC computer token IS the credential. Purpose "embed" tokens are
/// minted for anonymous iframe viewers (no Clerk session exists there), and
/// purpose "vnc" tokens verify the same way — signed, computer-bound,
/// user-bound, expiring — with the claim's user enforced against any AuthUser
/// when one happens to be present (see `validate_vnc_ws_request`). Purpose
/// "embed" is always forced read-only.
pub fn computer_vnc_public_router() -> Router<Arc<AppState>> {
    Router::new().route("/:id/vnc", get(computer_vnc_ws_handler))
}

/// REST routes, merged under `/api/v1`.
pub fn computer_api_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/computers/:id/ws-token", post(issue_ws_token))
        .route("/computers/:id/events", get(computer_events_history))
        .route("/computers/:id/proxy", get(get_proxy_config))
        .route("/computers/:id/proxy/enable", post(enable_proxy))
        .route("/computers/:id/proxy/disable", post(disable_proxy))
        .route("/computers/:id/proxy/*path", any(proxy_forward))
        .layer(DefaultBodyLimit::max(PROXY_BODY_LIMIT))
}

// ── Shared helpers ───────────────────────────────────────────────────────────

fn computer_handle(computer: &ComputerResponse) -> Result<ExecutionHandle, Response> {
    let native_id = computer
        .native_id
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| crate::computer_routes::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "computer has no native id",
        ))?;
    Ok(crate::bot_desktop_routes::build_handle(
        &native_id,
        computer.os.as_deref(),
        Some(&computer.provider),
    ))
}

fn require_driver(state: &AppState) -> Result<Arc<dyn ExecutionDriver>, Response> {
    state
        .vm_driver
        .clone()
        .ok_or_else(|| crate::computer_routes::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "No VM driver is configured on this host",
        ))
}

async fn guest_exec(
    driver: &Arc<dyn ExecutionDriver>,
    handle: &ExecutionHandle,
    command: Vec<String>,
) -> Result<allternit_driver_interface::ExecResult, DriverError> {
    driver
        .exec(
            handle,
            CommandSpec {
                command,
                env_vars: HashMap::new(),
                working_dir: None,
                stdin_data: None,
                capture_stdout: true,
                capture_stderr: true,
            },
        )
        .await
}

/// Pre-upgrade validation shared by the pty and events ws handlers: token
/// (signature, computer scope, purpose, user binding), ownership, running
/// state, and a configured driver.
async fn validate_ws_request(
    state: &Arc<AppState>,
    user: &AuthUser,
    id: &str,
    token: &str,
    purpose: &str,
) -> Result<ComputerResponse, Response> {
    let secret = crate::bot_desktop_stream::desktop_ws_secret(state).ok_or_else(|| {
        warn!("desktop ws secret not configured");
        crate::computer_routes::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop ws not configured",
        )
    })?;
    let claims = crate::bot_desktop_stream::verify_computer_token(&secret, token, id, purpose)
        .map_err(|e| {
            warn!(error = %e, computer_id = id, "invalid computer websocket token");
            crate::computer_routes::error_response(StatusCode::FORBIDDEN, "invalid token")
        })?;
    if claims.user_id != user.user_id {
        return Err(crate::computer_routes::error_response(
            StatusCode::FORBIDDEN,
            "token mismatch",
        ));
    }
    let computer = fetch_computer(state, user, id)
        .await?
        .ok_or_else(|| crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"))?;
    if computer.status != ComputerStatus::Running {
        return Err(crate::computer_routes::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "computer is not running",
        ));
    }
    require_driver(state)?;
    Ok(computer)
}

/// Decide VNC proxy access from token claims. Purpose "vnc" honors the
/// claim's `read_only` flag (full control by default, view-only when the
/// token says so); purpose "embed" is ALWAYS read-only — embed viewers can
/// never inject input. Any other purpose is rejected. Returns the effective
/// `read_only` flag.
fn resolve_vnc_access(
    claims: &crate::bot_desktop_stream::DesktopTokenClaims,
) -> Result<bool, crate::bot_desktop_stream::DesktopTokenError> {
    match claims.purpose.as_deref() {
        Some("vnc") => Ok(claims.read_only),
        Some("embed") => Ok(true),
        _ => Err(crate::bot_desktop_stream::DesktopTokenError::PurposeMismatch),
    }
}

/// Pre-upgrade validation for the VNC ws handler, mounted on the PUBLIC
/// router (see `computer_vnc_public_router`) — no Clerk session is required
/// to reach this function. Authorization is purpose-split:
///
/// - purpose "vnc" (honors the claim's read_only flag): the HMAC token is
///   the credential. It is server-signed, computer-bound (checked above),
///   user-bound, short-lived, and minting requires an authenticated,
///   owner-scoped call — but the route is mounted outside the Clerk
///   middleware (anonymous embed viewers share it), so an AuthUser extension
///   may be absent even for legitimate session clients (browsers and CLIs
///   included). When a session IS present its user id must match the token's;
///   when absent the verified claims stand alone. Requiring a session here
///   made every purpose-"vnc" token unusable (the gate could never pass on
///   the public mount) — that was the pre-fix state.
/// - purpose "embed" (always forced read-only): same token-as-credential
///   model, handed to anonymous iframe viewers who have no AuthUser at all.
///   The computer is fetched without ownership scoping; possessing a valid,
///   unexpired, computer-bound embed token is the authorization.
///
/// Both purposes still require a running computer and a configured driver.
async fn validate_vnc_ws_request(
    state: &Arc<AppState>,
    user: Option<&AuthUser>,
    id: &str,
    token: &str,
) -> Result<(ComputerResponse, bool), Response> {
    let secret = crate::bot_desktop_stream::desktop_ws_secret(state).ok_or_else(|| {
        warn!("desktop ws secret not configured");
        crate::computer_routes::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop ws not configured",
        )
    })?;
    let claims = crate::bot_desktop_stream::verify_desktop_token(&secret, token).map_err(|e| {
        warn!(error = %e, computer_id = id, "invalid computer websocket token");
        crate::computer_routes::error_response(StatusCode::FORBIDDEN, "invalid token")
    })?;
    let read_only = resolve_vnc_access(&claims).map_err(|_| {
        warn!(computer_id = id, purpose = ?claims.purpose, "token purpose does not allow vnc access");
        crate::computer_routes::error_response(StatusCode::FORBIDDEN, "invalid token")
    })?;
    if claims.computer_id.as_deref() != Some(id) {
        return Err(crate::computer_routes::error_response(
            StatusCode::FORBIDDEN,
            "token mismatch",
        ));
    }
    let computer = match claims.purpose.as_deref() {
        Some("vnc") => {
            // Session is opportunistic on this public mount: when an AuthUser
            // IS attached (e.g. a same-origin browser request that also
            // carries a Clerk session through a routed copy of this route),
            // it must match the token's user. When absent, the verified token
            // claims are the entire credential (see fn-level docs).
            if let Some(user) = user {
                if claims.user_id != user.user_id {
                    return Err(crate::computer_routes::error_response(
                        StatusCode::FORBIDDEN,
                        "token mismatch",
                    ));
                }
            }
            crate::computer_routes::fetch_computer_any_owner(state, id).await?
        }
        // "embed": token is the credential; fetch without ownership scoping.
        _ => crate::computer_routes::fetch_computer_any_owner(state, id).await?,
    }
    .ok_or_else(|| crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"))?;
    if computer.status != ComputerStatus::Running {
        return Err(crate::computer_routes::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "computer is not running",
        ));
    }
    require_driver(state)?;
    let read_only = read_only_unless_controller(state, &computer.id, &claims.user_id, read_only).await;
    Ok((computer, read_only))
}

/// Map a driver error for the proxy surface: unsupported substrate → 501,
/// anything else → upstream unreachable (502).
fn guest_error_status(e: &DriverError) -> StatusCode {
    match e {
        DriverError::NotSupported { .. } => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::BAD_GATEWAY,
    }
}

// ── Guest bootstrap ──────────────────────────────────────────────────────────

/// Ensure the PTY bridge is running in the guest; returns the bridge token.
/// `force` re-bootstraps unconditionally (used after a refused connection).
async fn ensure_pty_bridge(
    state: &Arc<AppState>,
    driver: &Arc<dyn ExecutionDriver>,
    handle: &ExecutionHandle,
    computer_id: &str,
    force: bool,
) -> Result<String, String> {
    let key = format!("{computer_id}:pty");
    if !force {
        if let Some(token) = state.computer_guest_tokens.read().await.get(&key) {
            return Ok(token.clone());
        }
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    driver
        .push_file(handle, PTY_BRIDGE_PATH, PTY_BRIDGE_SCRIPT.as_bytes().to_vec())
        .await
        .map_err(|e| format!("failed to upload pty bridge: {e}"))?;
    // The [.] bracket trick keeps pkill from matching this exec's own argv.
    let cmd = format!(
        "pkill -f 'allternit-pty-bridge[.]py' 2>/dev/null; sleep 0.3; \
         nohup python3 {PTY_BRIDGE_PATH} {PTY_GUEST_PORT} '{token}' \
         >/tmp/allternit-pty-bridge.log 2>&1 &"
    );
    let result = guest_exec(driver, handle, vec!["bash".into(), "-c".into(), cmd])
        .await
        .map_err(|e| format!("failed to start pty bridge: {e}"))?;
    if result.exit_code != 0 {
        let stderr = String::from_utf8_lossy(result.stderr.as_deref().unwrap_or(&[]));
        return Err(format!("pty bridge start failed: {}", stderr.trim()));
    }
    state
        .computer_guest_tokens
        .write()
        .await
        .insert(key, token.clone());
    info!(computer_id, "bootstrapped guest pty bridge");
    Ok(token)
}

/// Ensure the event collector is running in the guest.
async fn ensure_events_collector(
    state: &Arc<AppState>,
    driver: &Arc<dyn ExecutionDriver>,
    handle: &ExecutionHandle,
    computer_id: &str,
) -> Result<(), String> {
    let key = format!("{computer_id}:events");
    if state.computer_guest_tokens.read().await.contains_key(&key) {
        return Ok(());
    }
    driver
        .push_file(
            handle,
            EVENTS_COLLECTOR_PATH,
            EVENTS_COLLECTOR_SCRIPT.as_bytes().to_vec(),
        )
        .await
        .map_err(|e| format!("failed to upload events collector: {e}"))?;
    let cmd = format!(
        "pkill -f 'allternit-events[.]py' 2>/dev/null; sleep 0.3; \
         nohup python3 {EVENTS_COLLECTOR_PATH} >/tmp/allternit-events.log 2>&1 &"
    );
    let result = guest_exec(driver, handle, vec!["bash".into(), "-c".into(), cmd])
        .await
        .map_err(|e| format!("failed to start events collector: {e}"))?;
    if result.exit_code != 0 {
        let stderr = String::from_utf8_lossy(result.stderr.as_deref().unwrap_or(&[]));
        return Err(format!("events collector start failed: {}", stderr.trim()));
    }
    state
        .computer_guest_tokens
        .write()
        .await
        .insert(key, String::new());
    info!(computer_id, "bootstrapped guest events collector");
    Ok(())
}

// ── A. Interactive PTY over WebSocket ────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ComputerWsQuery {
    token: String,
}

async fn computer_pty_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<ComputerWsQuery>,
) -> impl IntoResponse {
    match validate_ws_request(&state, &user, &id, &query.token, "pty").await {
        Ok(computer) => ws.on_upgrade(move |socket| handle_pty_socket(socket, state, computer)),
        Err(response) => response.into_response(),
    }
}

async fn close_with_json_reason(socket: WebSocket, payload: Value) {
    let mut socket = socket;
    let reason = payload.to_string();
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: axum::extract::ws::close_code::ERROR,
            reason: reason.into(),
        })))
        .await;
    let _ = socket.close().await;
}

async fn handle_pty_socket(socket: WebSocket, state: Arc<AppState>, computer: ComputerResponse) {
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(_) => {
            let _ = socket.close().await;
            return;
        }
    };
    let handle = match computer_handle(&computer) {
        Ok(h) => h,
        Err(_) => {
            let _ = socket.close().await;
            return;
        }
    };

    let base = match driver.guest_service_url(&handle, PTY_GUEST_PORT).await {
        Ok(url) => url,
        Err(e) => {
            // Honest unsupported-provider close before any bridging.
            if matches!(e, DriverError::NotSupported { .. }) {
                warn!(computer_id = %computer.id, provider = %computer.provider, "pty not supported on substrate");
                close_with_json_reason(
                    socket,
                    json!({"error": format!("pty is not supported on the {} substrate", computer.provider)}),
                )
                .await;
            } else {
                error!(error = %e, computer_id = %computer.id, "failed to resolve pty guest service url");
                let _ = socket.close().await;
            }
            return;
        }
    };
    let addr = match crate::bot_desktop_stream::parse_tcp_addr(&base) {
        Some(addr) => addr,
        None => {
            error!(url = %base, "could not parse pty guest service url");
            let _ = socket.close().await;
            return;
        }
    };

    // Connect to the bridge, bootstrapping it into the guest on first use.
    // A refused connection means the bridge died (guest reboot etc.) —
    // re-bootstrap once and retry.
    let mut attempts = 0;
    let (tcp, bridge_token) = loop {
        let force = attempts > 0;
        let token = match ensure_pty_bridge(&state, &driver, &handle, &computer.id, force).await {
            Ok(t) => t,
            Err(e) => {
                error!(computer_id = %computer.id, error = %e, "failed to bootstrap pty bridge");
                let _ = socket.close().await;
                return;
            }
        };
        match TcpStream::connect(&addr).await {
            Ok(stream) => break (stream, token),
            Err(e) => {
                attempts += 1;
                if attempts > 1 {
                    error!(error = %e, %addr, "failed to connect to pty bridge after re-bootstrap");
                    let _ = socket.close().await;
                    return;
                }
                debug!(error = %e, %addr, "pty bridge refused connection; re-bootstrapping");
            }
        }
    };

    // Authenticate to the bridge: first line is the token.
    let (mut tcp_read, mut tcp_write) = tokio::io::split(tcp);
    if tcp_write
        .write_all(format!("{bridge_token}\n").as_bytes())
        .await
        .is_err()
    {
        let _ = socket.close().await;
        return;
    }

    info!(computer_id = %computer.id, %addr, "Opening PTY WebSocket proxy");

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (ws_tx, mut ws_rx) = mpsc::channel::<Message>(128);
    let ws_tx2 = ws_tx.clone();
    let ws_tx3 = ws_tx.clone();

    // Channel -> WebSocket sender.
    let mut forward_to_ws = tokio::spawn(async move {
        while let Some(msg) = ws_rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // WebSocket receiver -> TCP. Binary frames are raw pty bytes; text frames
    // may carry a resize control message (`{"cols":N,"rows":M}`) which is
    // relayed to the bridge as a one-line JSON control prefix. Malformed text
    // is ignored.
    let mut ws_to_tcp = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    if tcp_write.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Text(text)) => {
                    if let Some((cols, rows)) = parse_resize_message(&text) {
                        let control = format!("{{\"cols\":{cols},\"rows\":{rows}}}\n");
                        if tcp_write.write_all(control.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(data)) => {
                    let _ = ws_tx2.send(Message::Pong(data)).await;
                }
                Ok(Message::Pong(_)) => {}
                Err(e) => {
                    debug!(error = %e, "PTY WebSocket receive error");
                    break;
                }
            }
        }
    });

    // TCP -> WebSocket channel.
    let mut tcp_to_ws = tokio::spawn(async move {
        let mut buf = vec![0u8; 16384];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if ws_tx.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!(error = %e, "PTY TCP read error");
                    break;
                }
            }
        }
    });

    // Whichever forwarder finishes first ends the proxy; abort the survivors
    // or a task still polling the TCP read half holds the split stream open,
    // leaking the server-side socket as a lingering ESTABLISHED connection.
    tokio::select! {
        _ = &mut forward_to_ws => {},
        _ = &mut ws_to_tcp => {},
        _ = &mut tcp_to_ws => {},
    }
    forward_to_ws.abort();
    ws_to_tcp.abort();
    tcp_to_ws.abort();

    drop(ws_tx3);
    info!(computer_id = %computer.id, "PTY WebSocket proxy closed");
}

/// Parse a client resize text message. Returns `None` for anything that is
/// not a well-formed `{"cols":N,"rows":M}` object.
fn parse_resize_message(text: &str) -> Option<(u16, u16)> {
    let value: Value = serde_json::from_str(text).ok()?;
    if !value.is_object() {
        return None;
    }
    let cols = value.get("cols")?.as_u64()?;
    let rows = value.get("rows")?.as_u64()?;
    if cols == 0 || rows == 0 || cols > 1000 || rows > 1000 {
        return None;
    }
    Some((cols as u16, rows as u16))
}

// ── C. VNC desktop stream (binary WS proxy) ──────────────────────────────────

async fn computer_vnc_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(id): Path<String>,
    Query(query): Query<ComputerWsQuery>,
) -> impl IntoResponse {
    match validate_vnc_ws_request(&state, user.as_ref().map(|e| &e.0), &id, &query.token).await {
        Ok((computer, read_only)) => {
            ws.on_upgrade(move |socket| handle_vnc_socket(socket, state, computer, read_only))
        }
        Err(response) => response.into_response(),
    }
}

/// Binary WS <-> TCP pump to the guest VNC endpoint, mirroring
/// `handle_bot_desktop_socket` in `bot_desktop_stream.rs`. Standalone
/// computers have no bot-style `BotDesktopControlState` arbitration (no bot
/// can hold the desktop), so there is no takeover gate here — the owner
/// always controls their own computer, and any viewer they admit is either
/// full-control (purpose "vnc", read_only=false claim) or view-only
/// (`read_only`). When `read_only` is set the client->TCP direction passes
/// through an RFB-aware filter (`crate::vnc_readonly`): the handshake and
/// display-related messages still reach the server so the viewer can connect
/// and receive frames, but input messages (KeyEvent, PointerEvent,
/// ClientCutText) are consumed without forwarding. If the client picks a
/// security mechanism the filter does not understand, the filter degrades to
/// unfiltered passthrough (flagged via `RfbReadOnlyFilter::unfilterable` and
/// logged) since read-only can no longer be enforced there. Approval/ACI
/// gating on the control surfaces is untouched — this route is
/// view-or-control streaming only.
/// Where a computer's VNC server is, and the guest password the proxy
/// answers for (None = transparent pipe; the viewer handles auth).
///
/// Remote machines paired over Fabric (`provider = fabric`) expose VNC on
/// the mesh at `<host>:5900`; the API reaches that through desktop main's
/// mesh bridge. This Mac (the computer running this API) is its own Screen
/// Sharing server on localhost. Everything else comes from the VM driver.
async fn resolve_vnc_target(state: &Arc<AppState>, computer: &ComputerResponse) -> Option<(String, Option<String>)> {
    if crate::computer_routes::is_this_device(computer) {
        return Some((format!("127.0.0.1:{}", crate::mesh_bridge::NODE_VNC_PORT), None));
    }
    if computer.provider == crate::mesh_bridge::FABRIC_PROVIDER {
        let host = computer.host.clone().filter(|h| !h.is_empty())?;
        let target = format!("{host}:{}", crate::mesh_bridge::NODE_VNC_PORT);
        return match crate::mesh_bridge::loopback_for(&target).await {
            Ok(address) => Some((address, None)),
            Err(e) => {
                warn!(computer_id = %computer.id, %target, error = %e, "couldn't reach the remote computer over the mesh");
                None
            }
        };
    }
    let driver = require_driver(state).ok()?;
    let sandbox_id = computer
        .native_id
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| computer.id.clone());
    let endpoint = match driver.get_desktop_endpoint_by_native_id(&sandbox_id).await {
        Ok(Some(ep)) => ep,
        Ok(None) => {
            warn!(computer_id = %computer.id, %sandbox_id, "no desktop endpoint found");
            return None;
        }
        Err(e) => {
            error!(error = %e, computer_id = %computer.id, %sandbox_id, "failed to resolve desktop endpoint");
            return None;
        }
    };
    if !matches!(endpoint.protocol, allternit_driver_interface::DesktopProtocol::Vnc) {
        warn!(protocol = ?endpoint.protocol, "only raw VNC over TCP is supported for WebSocket proxy");
        return None;
    }
    let Some(tcp_addr) = crate::bot_desktop_stream::parse_tcp_addr(&endpoint.url) else {
        error!(url = %endpoint.url, "could not parse VNC URL as TCP address");
        return None;
    };
    Some((tcp_addr, endpoint.token.clone().filter(|t| !t.is_empty())))
}

/// True when this user (on any device) holds the computer's control lease.
async fn holds_control(state: &Arc<AppState>, computer_id: &str, user_id: &str) -> bool {
    let db = state.db.clone();
    let (id, uid) = (computer_id.to_string(), user_id.to_string());
    tokio::task::spawn_blocking(move || {
        db.connect()
            .ok()
            .and_then(|conn| crate::computer_control_lease::current(&conn, &id, chrono::Utc::now().timestamp()).ok().flatten())
            .is_some_and(|lease| lease.holder.kind == crate::computer_control_lease::HolderKind::User && lease.holder.id == uid)
    })
    .await
    .unwrap_or(false)
}

/// One controller per computer: a viewer who doesn't hold control (someone
/// else does) watches read-only.
async fn read_only_unless_controller(state: &Arc<AppState>, computer_id: &str, user_id: &str, read_only: bool) -> bool {
    if read_only {
        return true;
    }
    let db = state.db.clone();
    let (id, uid) = (computer_id.to_string(), user_id.to_string());
    let lease = tokio::task::spawn_blocking(move || {
        db.connect()
            .ok()
            .and_then(|conn| crate::computer_control_lease::current(&conn, &id, chrono::Utc::now().timestamp()).ok().flatten())
            .map(|lease| !(lease.holder.kind == crate::computer_control_lease::HolderKind::User && lease.holder.id == uid))
    })
    .await
    .ok()
    .flatten();
    lease.unwrap_or(false)
}

async fn handle_vnc_socket(
    socket: WebSocket,
    state: Arc<AppState>,
    computer: ComputerResponse,
    read_only: bool,
) {
    let Some((tcp_addr, vnc_password)) = resolve_vnc_target(&state, &computer).await else {
        let _ = socket.close().await;
        return;
    };

    info!(computer_id = %computer.id, %tcp_addr, read_only, "Opening VNC WebSocket proxy");

    let tcp = match TcpStream::connect(&tcp_addr).await {
        Ok(stream) => stream,
        Err(e) => {
            error!(error = %e, %tcp_addr, "failed to connect to VNC endpoint");
            let _ = socket.close().await;
            return;
        }
    };

    // Shared-guest-password interception: guests run x11vnc with a password the
    // driver knows (endpoint.token); the browser viewer must never see it, so
    // during the RFB handshake the proxy answers the DES challenge itself and
    // presents only None-auth to the viewer. Tart desktops leave token as None
    // and get a fully transparent pipe.
    //
    // The interceptor and the read-only filter live in ONE mutex: the filter's
    // client->server state machine must observe every byte the server
    // receives, including proxy-injected ones (the upstream type-2 choice and
    // the DES response are written from the tcp->ws task), or its handshake
    // states desynchronize and it starts eating real client messages.
    struct VncCodec {
        auth: Option<crate::vnc_auth::VncAuthInterceptor>,
        ro_filter: Option<crate::vnc_readonly::RfbReadOnlyFilter>,
    }
    let codec = std::sync::Arc::new(std::sync::Mutex::new(VncCodec {
        auth: vnc_password
            .as_deref()
            .map(crate::vnc_auth::VncAuthInterceptor::new),
        ro_filter: read_only.then(crate::vnc_readonly::RfbReadOnlyFilter::new),
    }));

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (mut tcp_read, tcp_write) = tokio::io::split(tcp);
    // The write half is shared: during the handshake the tcp->ws forwarder
    // produces proxy-injected bytes (security choice, DES response) that must
    // reach the VNC server immediately, not wait for the next client message.
    let tcp_write = std::sync::Arc::new(tokio::sync::Mutex::new(tcp_write));

    // Channel for messages that need to go to the browser.
    let (ws_tx, mut ws_rx) = mpsc::channel::<Message>(128);
    let ws_tx2 = ws_tx.clone();
    let computer_id_for_filter = computer.id.clone();

    // Forward channel -> WebSocket sender.
    let mut forward_to_ws = tokio::spawn(async move {
        while let Some(msg) = ws_rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Forward WebSocket receiver -> TCP. In read-only mode binary frames pass
    // through an RFB-aware filter: the handshake and display-related messages
    // (SetPixelFormat, SetEncodings, FramebufferUpdateRequest, ...) are
    // forwarded so the viewer can actually connect, while input messages
    // (KeyEvent, PointerEvent, ClientCutText) are consumed without forwarding.
    // Control frames (ping/pong/close) are still honored so the connection
    // stays healthy.
    let mut unfilterable_warned = false;
    let codec_ws_to_tcp = codec.clone();
    let tcp_write_ws_to_tcp = tcp_write.clone();
    let mut ws_to_tcp = tokio::spawn(async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    let write_result = {
                        let mut codec = codec_ws_to_tcp.lock().unwrap();
                        let after_auth = match codec.auth.as_mut() {
                            Some(a) => match a.client_bytes(&data) {
                                Ok(p) => p.to_server,
                                Err(e) => {
                                    warn!(error = %e, "VNC auth interception failed (client->server)");
                                    break;
                                }
                            },
                            None => data.to_vec(),
                        };
                        match codec.ro_filter.as_mut() {
                            Some(f) => {
                                let filtered = f.feed(&after_auth);
                                if f.unfilterable() && !unfilterable_warned {
                                    unfilterable_warned = true;
                                    warn!(
                                        computer_id = %computer_id_for_filter,
                                        "read-only degraded to unfiltered passthrough: unknown VNC security/version bytes; input can no longer be blocked"
                                    );
                                }
                                filtered
                            }
                            None => after_auth,
                        }
                    };
                    if !write_result.is_empty()
                        && tcp_write_ws_to_tcp
                            .lock()
                            .await
                            .write_all(&write_result)
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(Message::Ping(data)) => {
                    let _ = ws_tx2.send(Message::Pong(data)).await;
                }
                Ok(Message::Pong(_)) => {}
                Err(e) => {
                    debug!(error = %e, "VNC WebSocket receive error");
                    break;
                }
                _ => {}
            }
        }
    });

    // Forward TCP -> WebSocket channel.
    let mut tcp_to_ws = tokio::spawn(async move {
        let mut buf = vec![0u8; 16384];
        loop {
            match tcp_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let (to_server, to_client) = {
                        let mut codec = codec.lock().unwrap();
                        let pipe = match codec.auth.as_mut() {
                            Some(a) => match a.server_bytes(&buf[..n]) {
                                Ok(p) => p,
                                Err(e) => {
                                    warn!(error = %e, "VNC auth interception failed (server->client)");
                                    break;
                                }
                            },
                            None => crate::vnc_auth::VncPipe {
                                to_client: buf[..n].to_vec(),
                                to_server: Vec::new(),
                            },
                        };
                        // The read-only filter tracks every byte the server
                        // receives, injected or client-originated, so its
                        // handshake state stays aligned with the server.
                        let to_server = match codec.ro_filter.as_mut() {
                            Some(f) => {
                                // The filter reads ARD's key length from the
                                // server's side of the handshake.
                                f.observe_server(&buf[..n]);
                                f.feed(&pipe.to_server)
                            }
                            None => pipe.to_server,
                        };
                        (to_server, pipe.to_client)
                    };
                    if !to_server.is_empty()
                        && tcp_write
                            .lock()
                            .await
                            .write_all(&to_server)
                            .await
                            .is_err()
                    {
                        break;
                    }
                    if !to_client.is_empty()
                        && ws_tx.send(Message::Binary(to_client)).await.is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    debug!(error = %e, "VNC TCP read error");
                    break;
                }
            }
        }
    });

    // Whichever forwarder finishes first ends the proxy; abort the survivors
    // or a task still polling the TCP read half holds the split stream open,
    // leaking the server-side socket as a lingering ESTABLISHED connection.
    tokio::select! {
        _ = &mut forward_to_ws => {},
        _ = &mut ws_to_tcp => {},
        _ = &mut tcp_to_ws => {},
    }
    forward_to_ws.abort();
    ws_to_tcp.abort();
    tcp_to_ws.abort();

    info!(computer_id = %computer.id, "VNC WebSocket proxy closed");
}

// ── D. Guest event stream ────────────────────────────────────────────────────

async fn computer_events_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<ComputerWsQuery>,
) -> impl IntoResponse {
    match validate_ws_request(&state, &user, &id, &query.token, "events").await {
        Ok(computer) => ws.on_upgrade(move |socket| handle_events_socket(socket, state, computer)),
        Err(response) => response.into_response(),
    }
}

async fn handle_events_socket(socket: WebSocket, state: Arc<AppState>, computer: ComputerResponse) {
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(_) => {
            let _ = socket.close().await;
            return;
        }
    };
    let handle = match computer_handle(&computer) {
        Ok(h) => h,
        Err(_) => {
            let _ = socket.close().await;
            return;
        }
    };

    if let Err(e) = ensure_events_collector(&state, &driver, &handle, &computer.id).await {
        error!(computer_id = %computer.id, error = %e, "failed to bootstrap events collector");
        let _ = socket.close().await;
        return;
    }

    let (mut ws_sender, _ws_receiver) = socket.split();

    // Prime with the last 50 events, then poll for appended bytes.
    let initial = guest_exec(
        &driver,
        &handle,
        vec!["tail".into(), "-n".into(), "50".into(), EVENTS_LOG_PATH.into()],
    )
    .await;
    let mut offset: usize = match guest_exec(
        &driver,
        &handle,
        vec![
            "sh".into(),
            "-c".into(),
            format!("stat -c %s {EVENTS_LOG_PATH} 2>/dev/null || echo 0"),
        ],
    )
    .await
    {
        Ok(result) => String::from_utf8_lossy(result.stdout.as_deref().unwrap_or(&[]))
            .trim()
            .parse()
            .unwrap_or(0),
        Err(_) => 0,
    };

    match initial {
        Ok(result) => {
            let text = String::from_utf8_lossy(result.stdout.as_deref().unwrap_or(&[])).to_string();
            if !send_event_lines(&state, &computer.id, &text, &mut ws_sender).await {
                return;
            }
        }
        Err(e) => {
            debug!(error = %e, computer_id = %computer.id, "initial events tail failed (collector may still be starting)");
        }
    }

    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    info!(computer_id = %computer.id, "guest event stream opened");

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if ws_sender.send(Message::Text(json!({"type": "ping"}).to_string().into())).await.is_err() {
                    break;
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                let result = guest_exec(
                    &driver,
                    &handle,
                    vec![
                        "sh".into(),
                        "-c".into(),
                        format!("tail -c +{} {EVENTS_LOG_PATH} 2>/dev/null || true", offset + 1),
                    ],
                )
                .await;
                match result {
                    Ok(result) => {
                        let bytes = result.stdout.as_deref().unwrap_or(&[]);
                        offset += bytes.len();
                        let text = String::from_utf8_lossy(bytes).to_string();
                        if !send_event_lines(&state, &computer.id, &text, &mut ws_sender).await {
                            break;
                        }
                    }
                    Err(e) => {
                        debug!(error = %e, computer_id = %computer.id, "incremental events tail failed");
                    }
                }
            }
        }
    }

    info!(computer_id = %computer.id, "guest event stream closed");
}

/// Forward well-formed JSON event lines to the WebSocket. Returns false when
/// the socket is gone. Any delivered event refreshes `last_activity_at` so the
/// idle auto-stop sees live streams as activity.
async fn send_event_lines(
    state: &Arc<AppState>,
    computer_id: &str,
    text: &str,
    ws_sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> bool {
    let mut sent_any = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || serde_json::from_str::<Value>(line).is_err() {
            continue;
        }
        if ws_sender.send(Message::Text(line.into())).await.is_err() {
            return false;
        }
        sent_any = true;
    }
    if sent_any {
        crate::computer_routes::touch_computer_activity(&state.db, computer_id);
    }
    true
}

#[derive(Debug, Deserialize)]
pub struct EventsHistoryQuery {
    limit: Option<usize>,
}

/// GET /api/v1/computers/:id/events?limit=N — parsed event history (read-only;
/// auth + audit only, no ACI gate).
async fn computer_events_history(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<EventsHistoryQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(response) => return response,
    };
    let handle = match computer_handle(&computer) {
        Ok(h) => h,
        Err(response) => return response,
    };
    match guest_exec(
        &driver,
        &handle,
        vec!["tail".into(), "-n".into(), limit.to_string(), EVENTS_LOG_PATH.into()],
    )
    .await
    {
        Ok(result) => {
            let stdout = String::from_utf8_lossy(result.stdout.as_deref().unwrap_or(&[]));
            let events: Vec<Value> = stdout
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
                .collect();
            Json(json!({
                "computer_id": computer.id,
                "events": events,
                "limit": limit,
            }))
            .into_response()
        }
        Err(e @ DriverError::NotSupported { .. }) => (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({"error": format!("events are not supported on the {} substrate", computer.provider), "detail": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("failed to read guest events: {e}")})),
        )
            .into_response(),
    }
}

// ── ws-token issue ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct IssueWsTokenRequest {
    purpose: String,
    /// VNC only: mint the token view-only. Defaults to TRUE — a full-control
    /// VNC token (read_only=false) can inject keyboard/mouse into the guest,
    /// so it requires an ACI confirmation grant like the other control
    /// surfaces. Ignored for pty/events (always full-control interactive).
    #[serde(default)]
    read_only: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct IssueWsTokenResponse {
    token: String,
    expires_in: u64,
}

/// POST /api/v1/computers/:id/ws-token — mint a short-lived computer-scoped
/// ws token for the pty, events, or vnc stream.
///
/// Purpose "vnc" defaults to read-only. Minting a FULL-CONTROL vnc token
/// (`read_only: false`) is an approval-gated action: the pty/events purposes
/// have no mint gate (an interactive shell is reachable through the pty
/// anyway), but a read_only=false VNC token hands over the graphical desktop,
/// so it goes through the same `enforce_confirmation` grant machinery as the
/// control routes — pass `?approval_id=` from the confirmation handoff.
async fn issue_ws_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(approval): Query<ApprovalQuery>,
    Json(body): Json<IssueWsTokenRequest>,
) -> impl IntoResponse {
    if body.purpose != "pty" && body.purpose != "events" && body.purpose != "vnc" {
        return crate::computer_routes::error_response(
            StatusCode::BAD_REQUEST,
            "purpose must be \"pty\", \"events\", or \"vnc\"",
        );
    }
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    let read_only = match body.purpose.as_str() {
        "vnc" => body.read_only.unwrap_or(true),
        // pty/events are inherently interactive; read_only is not applicable.
        _ => false,
    };
    // Holding the control lease is itself the explicit "I'm driving" act
    // (Take control / an accepted hand-off), so a controller's full-control
    // token needs no second confirmation. Anyone else still goes through it.
    if body.purpose == "vnc" && !read_only && !holds_control(&state, &computer.id, &user.user_id).await {
        let descriptor = json!({
            "computer_id": computer.id,
            "purpose": "vnc",
            "read_only": false,
        });
        if let Err(denial) = crate::aci_safety::enforce_confirmation(
            &state.approval_store,
            &user.user_id,
            "computers.ws_token",
            crate::aci_safety::ConfirmationClass::Risky,
            &descriptor,
            approval.approval_id.as_deref(),
        ) {
            return (denial.status, Json(denial.body)).into_response();
        }
    }
    let secret = match crate::bot_desktop_stream::desktop_ws_secret(&state) {
        Some(s) => s,
        None => {
            return crate::computer_routes::error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop ws not configured",
            )
        }
    };
    let sandbox_id = computer
        .native_id
        .clone()
        .unwrap_or_else(|| computer.id.clone());
    let token = crate::bot_desktop_stream::sign_computer_token(
        &secret,
        &computer.id,
        &sandbox_id,
        &user.user_id,
        WS_TOKEN_TTL_SECONDS,
        &body.purpose,
        read_only,
    );
    crate::computer_audit::log_computer_access(
        &state.db,
        &computer.id,
        &user.user_id,
        crate::computer_audit::KIND_WS_TOKEN,
        &format!("purpose={} read_only={read_only}", body.purpose),
    );
    Json(IssueWsTokenResponse {
        token,
        expires_in: WS_TOKEN_TTL_SECONDS,
    })
    .into_response()
}

// ── C. Authenticated in-VM HTTP proxy ────────────────────────────────────────

/// True when `path` is covered by the allowlist. `"*"` allows everything;
/// otherwise a path is allowed when it starts with one of the prefixes
/// (a prefix of "/" allows everything).
pub(crate) fn proxy_path_allowed(paths: &[String], path: &str) -> bool {
    let path = path.trim_start_matches('/');
    paths.iter().any(|prefix| {
        if prefix == "*" {
            return true;
        }
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return false;
        }
        let normalized = prefix.trim_start_matches('/');
        normalized.is_empty() || path.starts_with(normalized)
    })
}

/// Headers that must never be forwarded to (or back from) the guest upstream:
/// `host`, `connection`, and the rest of the hop-by-hop set.
pub(crate) fn is_hop_by_hop(name: &str) -> bool {
    const HOP_BY_HOP: &[&str] = &[
        "host",
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "trailers",
        "transfer-encoding",
        "upgrade",
        "content-length",
    ];
    HOP_BY_HOP.contains(&name.to_ascii_lowercase().as_str())
}

/// Validate the proxy enable payload. Returns the normalized paths JSON
/// (default `["*"]`).
pub(crate) fn validate_proxy_config(
    port: i64,
    paths: Option<Vec<String>>,
) -> Result<Vec<String>, (StatusCode, &'static str)> {
    if !(1..=65535).contains(&port) {
        return Err((StatusCode::BAD_REQUEST, "port must be between 1 and 65535"));
    }
    let paths = paths.unwrap_or_else(|| vec!["*".to_string()]);
    if paths.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "paths must not be empty"));
    }
    for path in &paths {
        if path != "*" && !path.starts_with('/') {
            return Err((StatusCode::BAD_REQUEST, "paths must be \"*\" or start with /"));
        }
    }
    Ok(paths)
}

#[derive(Debug, Deserialize)]
pub struct EnableProxyRequest {
    port: i64,
    paths: Option<Vec<String>>,
}

struct ProxyConfig {
    port: u16,
    paths: Vec<String>,
}

fn load_proxy_config(
    db: &crate::db::DbHandle,
    computer_id: &str,
) -> Result<Option<ProxyConfig>, rusqlite::Error> {
    let conn = db.connect()?;
    let row = conn
        .query_row(
            "SELECT proxy_port, proxy_paths FROM computer_cloud_desktop WHERE computer_id = ?1",
            rusqlite::params![computer_id],
            |row| -> rusqlite::Result<(Option<i64>, Option<String>)> {
                Ok((row.get(0)?, row.get(1)?))
            },
        )
        .optional()?;
    match row {
        Some((Some(port), paths_json)) => {
            let paths: Vec<String> = paths_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| vec!["*".to_string()]);
            Ok(Some(ProxyConfig {
                port: port as u16,
                paths,
            }))
        }
        _ => Ok(None),
    }
}

fn store_proxy_config(
    db: &crate::db::DbHandle,
    computer_id: &str,
    port: Option<i64>,
    paths: Option<&str>,
) -> Result<(), rusqlite::Error> {
    let conn = db.connect()?;
    conn.execute(
        "INSERT INTO computer_cloud_desktop (computer_id, sandbox_id, control_state, proxy_port, proxy_paths)
         VALUES (?1, ?1, 'bot_controls', ?2, ?3)
         ON CONFLICT(computer_id) DO UPDATE SET proxy_port = excluded.proxy_port, proxy_paths = excluded.proxy_paths",
        rusqlite::params![computer_id, port, paths],
    )?;
    Ok(())
}

/// POST /api/v1/computers/:id/proxy/enable — ACI approval-gated.
async fn enable_proxy(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(approval): Query<ApprovalQuery>,
    Json(body): Json<EnableProxyRequest>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    let paths = match validate_proxy_config(body.port, body.paths) {
        Ok(p) => p,
        Err((status, message)) => return crate::computer_routes::error_response(status, message),
    };
    // ACI gate: reversible state-changing, in the existing taxonomy style.
    let action = crate::computer_control::ComputerControlAction::ProxyEnable {
        port: body.port as u16,
    };
    let descriptor = crate::computer_control::control_action_descriptor(&action);
    let class = crate::computer_control::classify_control_action(&action);
    if let Err(denial) = crate::aci_safety::enforce_confirmation(
        &state.approval_store,
        &user.user_id,
        "computer.proxy",
        class,
        &descriptor,
        approval.approval_id.as_deref(),
    ) {
        return (denial.status, Json(denial.body)).into_response();
    }
    let paths_json = json!(paths).to_string();
    match store_proxy_config(&state.db, &computer.id, Some(body.port), Some(&paths_json)) {
        Ok(()) => {
            crate::computer_audit::log_computer_access(
                &state.db,
                &computer.id,
                &user.user_id,
                crate::computer_audit::KIND_PROXY_ENABLE,
                &format!("port={} paths={}", body.port, paths_json),
            );
            Json(json!({"enabled": true, "port": body.port, "paths": paths})).into_response()
        }
        Err(e) => {
            warn!(error = %e, computer_id = %computer.id, "failed to store proxy config");
            crate::computer_routes::error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
}

/// POST /api/v1/computers/:id/proxy/disable — ACI approval-gated.
async fn disable_proxy(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(approval): Query<ApprovalQuery>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    let action = crate::computer_control::ComputerControlAction::ProxyDisable;
    let descriptor = crate::computer_control::control_action_descriptor(&action);
    let class = crate::computer_control::classify_control_action(&action);
    if let Err(denial) = crate::aci_safety::enforce_confirmation(
        &state.approval_store,
        &user.user_id,
        "computer.proxy",
        class,
        &descriptor,
        approval.approval_id.as_deref(),
    ) {
        return (denial.status, Json(denial.body)).into_response();
    }
    match store_proxy_config(&state.db, &computer.id, None, None) {
        Ok(()) => {
            crate::computer_audit::log_computer_access(
                &state.db,
                &computer.id,
                &user.user_id,
                crate::computer_audit::KIND_PROXY_DISABLE,
                "proxy disabled",
            );
            Json(json!({"enabled": false})).into_response()
        }
        Err(e) => {
            warn!(error = %e, computer_id = %computer.id, "failed to clear proxy config");
            crate::computer_routes::error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
}

/// GET /api/v1/computers/:id/proxy — current proxy config (no gate).
async fn get_proxy_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    match load_proxy_config(&state.db, &computer.id) {
        Ok(Some(config)) => Json(json!({
            "enabled": true,
            "port": config.port,
            "paths": config.paths,
        }))
        .into_response(),
        Ok(None) => Json(json!({"enabled": false, "port": Value::Null, "paths": Value::Null})).into_response(),
        Err(e) => {
            warn!(error = %e, computer_id = %computer.id, "failed to load proxy config");
            crate::computer_routes::error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
}

const PROXY_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// An SSE subscription (`Accept: text/event-stream`) is long-lived by design
/// and exempt from the per-request timeout; its body streams through.
pub(crate) fn wants_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false)
}

fn proxy_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        // No client-wide total timeout: it would cut event streams. Plain
        // requests get PROXY_REQUEST_TIMEOUT per request (proxy_forward).
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("proxy reqwest client builds")
    })
}

/// ANY /api/v1/computers/:id/proxy/*path — forward to the guest port the
/// owner opted into. Auth'd; every request is audit-logged.
/// The forwarded Host must look loopback-shaped to the guest service: A6.1
/// guards (e.g. the subscription gateway) reject any other Host, and reqwest
/// would otherwise derive `mail.news...:port` from the upstream URL.
pub(crate) fn upstream_guest_host(guest_port: u16) -> String {
    format!("127.0.0.1:{}", guest_port)
}

async fn proxy_forward(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, path)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    // WebSocket upgrades are not proxied (the ws plane has its own routes).
    if headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
    {
        return crate::computer_routes::error_response(
            StatusCode::NOT_IMPLEMENTED,
            "websocket upgrade is not supported through the computer proxy",
        );
    }

    let computer = match fetch_computer(&state, &user, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return crate::computer_routes::error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    if computer.status != ComputerStatus::Running {
        return crate::computer_routes::error_response(StatusCode::SERVICE_UNAVAILABLE, "computer is not running");
    }
    let config = match load_proxy_config(&state.db, &computer.id) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return crate::computer_routes::error_response(
                StatusCode::NOT_FOUND,
                "proxy is not enabled for this computer",
            )
        }
        Err(e) => {
            warn!(error = %e, computer_id = %computer.id, "failed to load proxy config");
            return crate::computer_routes::error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    };
    if !proxy_path_allowed(&config.paths, &path) {
        return crate::computer_routes::error_response(StatusCode::FORBIDDEN, "path is not allowed by the proxy allowlist");
    }
    if body.len() > PROXY_BODY_LIMIT {
        return crate::computer_routes::error_response(StatusCode::PAYLOAD_TOO_LARGE, "request body exceeds 10 MB");
    }
    let driver = match require_driver(&state) {
        Ok(d) => d,
        Err(response) => return response,
    };
    let handle = match computer_handle(&computer) {
        Ok(h) => h,
        Err(response) => return response,
    };
    let base = match driver.guest_service_url(&handle, config.port).await {
        Ok(url) => url,
        Err(e) => {
            let status = guest_error_status(&e);
            return crate::computer_routes::error_response(
                status,
                format!("proxy upstream is unreachable: {e}"),
            );
        }
    };

    let mut url = format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'));
    if let Some(query) = uri.query() {
        url.push('?');
        url.push_str(query);
    }

    let mut upstream = proxy_client()
        .request(method.clone(), &url)
        .header(header::HOST, upstream_guest_host(config.port));
    if !wants_event_stream(&headers) {
        upstream = upstream.timeout(PROXY_REQUEST_TIMEOUT);
    }
    for (name, value) in headers.iter() {
        if is_hop_by_hop(name.as_str()) {
            continue;
        }
        upstream = upstream.header(name.as_str(), value.as_bytes());
    }
    let upstream = upstream.body(body.to_vec()).send().await;

    let response = match upstream {
        Ok(resp) => {
            let status = resp.status();
            let mut builder = Response::builder().status(status);
            let response_headers = builder
                .headers_mut()
                .expect("response builder has headers");
            for (name, value) in resp.headers().iter() {
                if !is_hop_by_hop(name.as_str()) {
                    response_headers.append(name, value.clone());
                }
            }
            // Stream the body through (SSE must not wait for the upstream to
            // end — it never does). A mid-body upstream error ends the stream.
            match builder.body(axum::body::Body::from_stream(resp.bytes_stream())) {
                Ok(response) => response,
                Err(e) => {
                    warn!(error = %e, "failed to build proxy response");
                    return crate::computer_routes::error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to build proxy response",
                    );
                }
            }
        }
        Err(e) => {
            warn!(error = %e, %url, "proxy upstream request failed");
            return crate::computer_routes::error_response(
                StatusCode::BAD_GATEWAY,
                format!("proxy upstream is unreachable: {e}"),
            );
        }
    };

    crate::computer_audit::log_computer_access(
        &state.db,
        &computer.id,
        &user.user_id,
        crate::computer_audit::KIND_PROXY,
        &format!("{} /{} → {}", method, path, response.status()),
    );
    response
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const TEST_SECRET: &str = "test-ws-secret-for-unit-tests";

    #[test]
    fn event_stream_requests_are_exempt_from_the_proxy_timeout() {
        let mut headers = HeaderMap::new();
        assert!(!wants_event_stream(&headers));
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        assert!(!wants_event_stream(&headers));
        headers.insert(header::ACCEPT, HeaderValue::from_static("Text/Event-Stream"));
        assert!(wants_event_stream(&headers));
    }

    #[test]
    fn vnc_access_resolution() {
        use crate::bot_desktop_stream::{sign_computer_token, verify_desktop_token, DesktopTokenError};

        // Purpose "vnc" honors the claim's read_only flag.
        let token = sign_computer_token(TEST_SECRET, "computer-1", "s", "user-1", 60, "vnc", false);
        let claims = verify_desktop_token(TEST_SECRET, &token).unwrap();
        assert_eq!(resolve_vnc_access(&claims), Ok(false));
        let token = sign_computer_token(TEST_SECRET, "computer-1", "s", "user-1", 60, "vnc", true);
        let claims = verify_desktop_token(TEST_SECRET, &token).unwrap();
        assert_eq!(resolve_vnc_access(&claims), Ok(true));

        // Purpose "embed" is ALWAYS read-only, even if the claim says false.
        let token = sign_computer_token(TEST_SECRET, "computer-1", "s", "user-1", 60, "embed", false);
        let claims = verify_desktop_token(TEST_SECRET, &token).unwrap();
        assert_eq!(resolve_vnc_access(&claims), Ok(true));

        // Other purposes are rejected on the VNC surface.
        for purpose in ["pty", "events"] {
            let token = sign_computer_token(TEST_SECRET, "computer-1", "s", "user-1", 60, purpose, false);
            let claims = verify_desktop_token(TEST_SECRET, &token).unwrap();
            assert!(matches!(
                resolve_vnc_access(&claims),
                Err(DesktopTokenError::PurposeMismatch)
            ));
        }

        // Bot-shaped tokens (no purpose) are rejected too.
        let bot_token = crate::bot_desktop_stream::sign_desktop_token(TEST_SECRET, "bot-1", "s", "user-1", 60);
        let claims = verify_desktop_token(TEST_SECRET, &bot_token).unwrap();
        assert!(matches!(
            resolve_vnc_access(&claims),
            Err(DesktopTokenError::PurposeMismatch)
        ));
    }

    /// Insert a running owner-scoped computer for handler-level tests.
    async fn state_with_computer(
        vm_driver: Option<Arc<dyn ExecutionDriver>>,
    ) -> Arc<AppState> {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state_with_driver(temp.path(), vm_driver).await;
        // Keep the tempdir alive for the duration of the test.
        std::mem::forget(temp);
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', 'user-1', 'Builder Box', 'ubuntu-24.04', 'sandbox-1', 'credits')",
                [],
            )
            .unwrap();
        state
    }

    /// Presence-only driver stub: `validate_vnc_ws_request` checks that a
    /// driver is configured but never calls it on the success path.
    #[derive(Debug)]
    struct StubDriver;

    #[async_trait::async_trait]
    impl ExecutionDriver for StubDriver {
        fn capabilities(&self) -> allternit_driver_interface::DriverCapabilities {
            use allternit_driver_interface::{
                DriverCapabilities, DriverFeatures, DriverType, EnvSpecType, IsolationLevel,
                ResourceSpec,
            };
            DriverCapabilities {
                resize: false,
                clone: false,
                driver_type: DriverType::Container,
                isolation: IsolationLevel::Standard,
                max_resources: ResourceSpec {
                    cpu_millis: 2000,
                    memory_mib: 4096,
                    disk_mib: Some(20480),
                    network_egress_kib: None,
                    gpu_count: None,
                },
                supported_env_specs: vec![EnvSpecType::Oci],
                features: DriverFeatures {
                    snapshot: false,
                    live_restore: false,
                    gpu: false,
                    prewarm: false,
                },
            }
        }

        async fn spawn(
            &self,
            _spec: allternit_driver_interface::SpawnSpec,
        ) -> std::result::Result<ExecutionHandle, allternit_driver_interface::DriverError> {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "spawn".to_string(),
            })
        }

        async fn pause_vm(
            &self,
            _handle: &ExecutionHandle,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn resume_vm(
            &self,
            _handle: &ExecutionHandle,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn exec(
            &self,
            _handle: &ExecutionHandle,
            _cmd: CommandSpec,
        ) -> std::result::Result<allternit_driver_interface::ExecResult, allternit_driver_interface::DriverError>
        {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "exec".to_string(),
            })
        }

        async fn stream_logs(
            &self,
            _handle: &ExecutionHandle,
        ) -> std::result::Result<Vec<allternit_driver_interface::LogEntry>, allternit_driver_interface::DriverError>
        {
            Ok(Vec::new())
        }

        async fn get_artifacts(
            &self,
            _handle: &ExecutionHandle,
        ) -> std::result::Result<Vec<allternit_driver_interface::Artifact>, allternit_driver_interface::DriverError>
        {
            Ok(Vec::new())
        }

        async fn destroy(
            &self,
            _handle: &ExecutionHandle,
        ) -> std::result::Result<(), allternit_driver_interface::DriverError> {
            Ok(())
        }

        async fn get_consumption(
            &self,
            _handle: &ExecutionHandle,
        ) -> std::result::Result<allternit_driver_interface::ResourceConsumption, allternit_driver_interface::DriverError>
        {
            Err(allternit_driver_interface::DriverError::NotSupported {
                feature: "consumption".to_string(),
            })
        }

        async fn get_receipt(
            &self,
            _handle: &ExecutionHandle,
        ) -> std::result::Result<Option<allternit_driver_interface::Receipt>, allternit_driver_interface::DriverError>
        {
            Ok(None)
        }

        async fn health_check(&self) -> std::result::Result<allternit_driver_interface::DriverHealth, allternit_driver_interface::DriverError> {
            Ok(allternit_driver_interface::DriverHealth {
                healthy: true,
                ..Default::default()
            })
        }
    }

    fn handler_test_secret() {
        // validate/issue paths read the secret from env on every call.
        std::env::set_var("ALLTERNIT_DESKTOP_WS_SECRET", TEST_SECRET);
    }

    fn auth_user(user_id: &str) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            organization_id: Some("org-1".into()),
            tenant_id: None,
            email: None,
            name: None,
            avatar_url: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    #[tokio::test]
    async fn vnc_embed_token_needs_no_auth_user() {
        // The public VNC route exists for anonymous iframe viewers: an embed
        // token (HMAC, computer-bound, expiring, forced read-only) is the
        // entire credential — None user must validate fine.
        handler_test_secret();
        let state = state_with_computer(Some(Arc::new(StubDriver))).await;
        let token = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-1", "sandbox-1", "user-1", 60, "embed", true,
        );
        let (computer, read_only) =
            validate_vnc_ws_request(&state, None, "computer-1", &token)
                .await
                .expect("embed token validates without an AuthUser");
        assert_eq!(computer.id, "computer-1");
        assert!(read_only, "embed is always read-only");
    }

    #[tokio::test]
    async fn vnc_full_control_token_validates_without_session() {
        // Purpose "vnc" tokens are minted owner-scoped behind auth, but the
        // vnc route is mounted on the PUBLIC router (embed viewers share it),
        // so no AuthUser is ever attached. The verified token — signed,
        // computer-bound, user-bound, expiring — is the credential. Before
        // this fix the session requirement made every vnc token 403.
        handler_test_secret();
        let state = state_with_computer(Some(Arc::new(StubDriver))).await;
        let token = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-1", "sandbox-1", "user-1", 60, "vnc", false,
        );
        let (computer, read_only) =
            validate_vnc_ws_request(&state, None, "computer-1", &token)
                .await
                .expect("vnc token validates without a session on the public mount");
        assert_eq!(computer.id, "computer-1");
        assert!(!read_only, "read_only=false claim must be honored for vnc tokens");
    }

    #[tokio::test]
    async fn vnc_token_rejects_mismatched_user() {
        handler_test_secret();
        let state = state_with_computer(None).await;
        let token = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-1", "sandbox-1", "user-1", 60, "vnc", true,
        );
        let err = validate_vnc_ws_request(&state, Some(&auth_user("user-2")), "computer-1", &token)
            .await
            .expect_err("vnc token minted for user-1 must not authorize user-2");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn vnc_embed_token_is_computer_bound() {
        handler_test_secret();
        let state = state_with_computer(None).await;
        let token = crate::bot_desktop_stream::sign_computer_token(
            TEST_SECRET, "computer-a", "sandbox-a", "user-1", 60, "embed", true,
        );
        let err = validate_vnc_ws_request(&state, None, "computer-1", &token)
            .await
            .expect_err("embed token for another computer must be rejected");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn vnc_public_route_is_not_behind_auth_middleware() {
        // Router-mount level: a request with NO Authorization header and no
        // Clerk session must get past any auth gate. In-process oneshot has no
        // connection upgrade state, so axum's WebSocketUpgrade extractor
        // rejects with 426 ConnectionNotUpgradable — that is the proof: the
        // request reached the route's own extractor instead of the auth
        // middleware's 401 MissingToken.
        handler_test_secret();
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        std::mem::forget(temp);
        let request = axum::http::Request::builder()
            .uri("/computer-1/vnc?token=not-a-real-token")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("sec-websocket-version", "13")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(
            computer_vnc_public_router().with_state(state),
            request,
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UPGRADE_REQUIRED,
            "request must reach the route's own ws extractor (426), not the auth middleware (401)"
        );
    }

    #[tokio::test]
    async fn ws_token_mints_vnc_read_only_by_default() {
        handler_test_secret();
        let state = state_with_computer(None).await;
        let response = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: None }),
            Json(IssueWsTokenRequest { purpose: "vnc".into(), read_only: None }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims = crate::bot_desktop_stream::verify_desktop_token(
            TEST_SECRET,
            value["token"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(claims.purpose.as_deref(), Some("vnc"));
        assert!(claims.read_only, "vnc ws tokens must default to read-only");
    }

    #[tokio::test]
    async fn ws_token_full_control_vnc_requires_approval() {
        handler_test_secret();
        let state = state_with_computer(None).await;
        let response = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: None }),
            Json(IssueWsTokenRequest { purpose: "vnc".into(), read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"], "confirmation_required");
    }

    #[tokio::test]
    async fn ws_token_full_control_vnc_for_the_controller_needs_no_approval() {
        handler_test_secret();
        let state = state_with_computer(None).await;
        let me = crate::computer_control_lease::Holder {
            kind: crate::computer_control_lease::HolderKind::User,
            id: "user-1".into(),
            label: None,
            device_id: Some("mac-1".into()),
        };
        let conn = state.db.connect().unwrap();
        crate::computer_control_lease::take(&conn, "computer-1", &me, chrono::Utc::now().timestamp()).unwrap();
        let response = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: None }),
            Json(IssueWsTokenRequest { purpose: "vnc".into(), read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims = crate::bot_desktop_stream::verify_desktop_token(TEST_SECRET, value["token"].as_str().unwrap()).unwrap();
        assert!(!claims.read_only, "the controller gets a full-control token");
    }

    #[tokio::test]
    async fn ws_token_full_control_vnc_redeems_approval_grant() {
        handler_test_secret();
        let state = state_with_computer(None).await;
        // Mirror the REST control-surface pattern: the denial payload carries
        // a fresh approval_id; the human handoff approves it
        // (`GRANTS.approve`), and presenting the approved grant back satisfies
        // the gate. A Pending (unapproved) grant must NOT redeem.
        let denied = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: None }),
            Json(IssueWsTokenRequest { purpose: "vnc".into(), read_only: Some(false) }),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(denied.into_body(), 1 << 20)
            .await
            .unwrap();
        let denial: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let approval_id = denial["approval_id"].as_str().unwrap().to_string();

        // Pending grant: not yet approved → still denied.
        let pending = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: Some(approval_id.clone()) }),
            Json(IssueWsTokenRequest { purpose: "vnc".into(), read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(pending.status(), StatusCode::FORBIDDEN);

        // Human approval through the handoff, then the grant redeems once.
        assert!(crate::aci_approvals::GRANTS.approve(&approval_id));
        let allowed = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: Some(approval_id.clone()) }),
            Json(IssueWsTokenRequest { purpose: "vnc".into(), read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(allowed.status(), StatusCode::OK);

        // Grants are single-use: the second attempt is denied.
        let reused = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: Some(approval_id) }),
            Json(IssueWsTokenRequest { purpose: "vnc".into(), read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(reused.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ws_token_rejects_unknown_purpose() {
        handler_test_secret();
        let state = state_with_computer(None).await;
        let response = issue_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("computer-1".to_string()),
            Query(crate::computer_routes::ApprovalQuery { approval_id: None }),
            Json(IssueWsTokenRequest { purpose: "bogus".into(), read_only: None }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn resize_message_parsing() {
        assert_eq!(parse_resize_message(r#"{"cols":120,"rows":40}"#), Some((120, 40)));
        assert_eq!(parse_resize_message(r#"{"rows":40,"cols":120}"#), Some((120, 40)));
        assert!(parse_resize_message(r#"{"cols":0,"rows":40}"#).is_none());
        assert!(parse_resize_message(r#"{"cols":2000,"rows":40}"#).is_none());
        assert!(parse_resize_message("not json").is_none());
        assert!(parse_resize_message(r#"{"cols":"120","rows":40}"#).is_none());
        assert!(parse_resize_message(r#"[1,2]"#).is_none());
        assert!(parse_resize_message("ls -la").is_none());
    }

    #[test]
    fn pty_bridge_script_compiles() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("python3 not available; skipping pty bridge compile check");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("allternit-pty-bridge.py");
        std::fs::write(&path, PTY_BRIDGE_SCRIPT).unwrap();
        let status = std::process::Command::new("python3")
            .arg("-m")
            .arg("py_compile")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "pty bridge script must compile");
    }

    #[test]
    fn events_collector_script_compiles() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("python3 not available; skipping events collector compile check");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("allternit-events.py");
        std::fs::write(&path, EVENTS_COLLECTOR_SCRIPT).unwrap();
        let status = std::process::Command::new("python3")
            .arg("-m")
            .arg("py_compile")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "events collector script must compile");
    }

    #[test]
    fn proxy_path_allowlist_matching() {
        // "*" allows everything.
        assert!(proxy_path_allowed(&["*".to_string()], "anything/at/all"));
        // Prefix matching.
        let paths = vec!["/api".to_string(), "/static".to_string()];
        assert!(proxy_path_allowed(&paths, "api"));
        assert!(proxy_path_allowed(&paths, "api/v1/x"));
        assert!(proxy_path_allowed(&paths, "/api/v1/x"));
        assert!(proxy_path_allowed(&paths, "static/app.js"));
        assert!(!proxy_path_allowed(&paths, "etc/passwd"));
        // Prefix matching is deliberately naive string-prefix (per spec):
        // "apiary" starts with the "/api" prefix.
        assert!(proxy_path_allowed(&paths, "apiary"));
        // Root prefix allows everything.
        assert!(proxy_path_allowed(&["/".to_string()], "deep/nested/path"));
        // Empty and malformed entries match nothing.
        assert!(!proxy_path_allowed(&["".to_string()], "api"));
        assert!(!proxy_path_allowed(&[], "api"));
    }

    #[test]
    fn proxy_header_filtering() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("example.com"));
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.insert("content-length", HeaderValue::from_static("42"));
        headers.insert("authorization", HeaderValue::from_static("Bearer x"));
        headers.insert("x-custom", HeaderValue::from_static("yes"));
        let forwarded: Vec<&str> = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| !is_hop_by_hop(name))
            .collect();
        assert_eq!(forwarded, vec!["authorization", "x-custom"]);
        // Case-insensitive deny list.
        assert!(is_hop_by_hop("HOST"));
        assert!(is_hop_by_hop("Connection"));
        assert!(!is_hop_by_hop("content-type"));
    }

    #[test]
    fn proxy_forward_uses_loopback_shaped_guest_host() {
        // The subscription gateway's A6.1 host guard only accepts loopback
        // Hosts with no port or the arrived-on port; the forwarded Host must
        // be 127.0.0.1:<guest_port>, never the Incus host the URL derives.
        assert_eq!(upstream_guest_host(7788), "127.0.0.1:7788");
        assert!(is_hop_by_hop("host"));
    }

    #[test]
    fn proxy_enable_validation() {
        // Valid cases.
        assert_eq!(validate_proxy_config(8080, None).unwrap(), vec!["*"]);        assert_eq!(
            validate_proxy_config(1, Some(vec!["/api".to_string()])).unwrap(),
            vec!["/api"]
        );
        assert_eq!(
            validate_proxy_config(65535, Some(vec!["*".to_string()])).unwrap(),
            vec!["*"]
        );
        // Port range.
        assert!(validate_proxy_config(0, None).is_err());
        assert!(validate_proxy_config(-1, None).is_err());
        assert!(validate_proxy_config(65536, None).is_err());
        assert!(validate_proxy_config(70000, None).is_err());
        // Paths.
        assert!(validate_proxy_config(8080, Some(vec![])).is_err());
        assert!(validate_proxy_config(8080, Some(vec!["no-slash".to_string()])).is_err());
    }

    #[test]
    fn guest_error_status_mapping() {
        assert_eq!(
            guest_error_status(&DriverError::NotSupported {
                feature: "guest service url".to_string()
            }),
            StatusCode::NOT_IMPLEMENTED
        );
        assert_eq!(
            guest_error_status(&DriverError::InternalError {
                message: "boom".to_string()
            }),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn proxy_config_store_and_load_round_trip() {
        let db = crate::db::DbHandle::new_memory().unwrap();
        // computer_cloud_desktop.computer_id references computers(id).
        db.connect()
            .unwrap()
            .execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', 'u', 'Test', 'credits')",
                [],
            )
            .unwrap();
        store_proxy_config(&db, "computer-1", Some(8080), Some(r#"["/api"]"#)).unwrap();
        let config = load_proxy_config(&db, "computer-1").unwrap().unwrap();
        assert_eq!(config.port, 8080);
        assert_eq!(config.paths, vec!["/api"]);
        store_proxy_config(&db, "computer-1", None, None).unwrap();
        assert!(load_proxy_config(&db, "computer-1").unwrap().is_none());
        assert!(load_proxy_config(&db, "missing").unwrap().is_none());
    }
}
