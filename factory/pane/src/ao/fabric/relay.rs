//! Relay client: line-faithful Rust port of `cmd/agent-daemon/src/index.ts`
//! (446-line production node client). authenticate-first WSS handshake,
//! `request` → chunked-response HTTP tunnel to the loopback shim, `socket_*`
//! WS tunnel with per-socket tasks, path allow-lists, identity-header
//! injection, 1 s→30 s reconnect backoff, 1012 teardown on relay drop.
//!
//! One deliberate, documented deviation: agent-daemon's `socket_open`
//! allow-list omits `/v1/` (its tunnels target `/ws`, `/terminal`, …), but
//! the ao shim's session-events socket lives at `/v1/remote-control/…`, so
//! the socket path guard reuses the request allow-list (which includes
//! `/v1/`, index.ts:287-296). Everything else maps 1:1.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Message};
use tokio_tungstenite::{connect_async, tungstenite};

use super::cloud::CloudClient;
use super::identity::NodeIdentity;
use super::wire::{CloudMessage, NodeMessage};

const INITIAL_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// Server-side cap is 90 s (`RELAY_TIMEOUT`, runtime_relay.rs:77); the node
/// gives the local forward a small buffer past that.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(95);

fn allowed_path(path: &str) -> bool {
    if !path.starts_with('/') || path.contains("..") || path.contains("://") {
        return false;
    }
    // index.ts:287-296 (request list; socket_open uses the same list in this
    // port — see module doc).
    const ALLOWED_PREFIXES: &[&str] = &[
        "/api/", "/viz", "/sandbox", "/vm-session", "/rails", "/stream",
        "/terminal", "/mcp", "/platform", "/metrics", "/alabs", "/cowork",
        "/webhooks", "/status", "/health",
        "/ws", "/panes",
        "/v1/",
    ];
    ALLOWED_PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

/// Response headers forwarded back through the tunnel (index.ts:316-319).
const FORWARDED_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "cache-control",
    "content-disposition",
    "etag",
    "last-modified",
    "x-request-id",
];

type LocalSocketSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

struct RelayState {
    identity: Arc<Mutex<NodeIdentity>>,
    http: reqwest::Client,
    local_gateway: String,
    /// Live tunneled sockets, keyed by the cloud's socket_id.
    local_sockets: Mutex<HashMap<String, LocalSocketSink>>,
}

impl RelayState {
    async fn snapshot(&self) -> NodeIdentity {
        self.identity.lock().await.clone()
    }

    fn inject_identity_headers(identity: &NodeIdentity, headers: &mut reqwest::header::HeaderMap) {
        use reqwest::header::{HeaderName, HeaderValue};
        let mut set = |name: &str, value: &str| {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(name, value);
            }
        };
        if let (Ok(token), Ok(user_id)) = (identity.device_token(), identity.user_id()) {
            set("x-allternit-desktop-access-token", token);
            set("x-allternit-user-id", user_id);
        }
        if let Ok(email) = identity.user_email() {
            set("x-allternit-user-email", email);
        }
        if let Some(org) = identity.organization_id.as_deref() {
            set("x-allternit-tenant-id", org);
        }
    }
}

/// Runs the relay loop until `shutdown` fires. `reconnect` is bumped by the
/// lifecycle loop after a credential rotation to force a reconnect on the
/// new token (agent-daemon `relay?.close()` post-rotate, index.ts:236, and
/// the single-relay guard at index.ts:240 — this task IS the single relay).
pub(crate) async fn run(
    identity: Arc<Mutex<NodeIdentity>>,
    cloud: CloudClient,
    mut reconnect: tokio::sync::watch::Receiver<u64>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    label: Arc<tokio::sync::RwLock<String>>,
    local_gateway: String,
) {
    let state = Arc::new(RelayState {
        identity,
        http: match reqwest::Client::builder().build() {
            Ok(client) => client,
            Err(err) => {
                eprintln!("[ao-fabric] relay: cannot build http client: {err}");
                return;
            }
        },
        local_gateway,
        local_sockets: Mutex::new(HashMap::new()),
    });

    let mut retry = INITIAL_RETRY;
    loop {
        if *shutdown.borrow() {
            break;
        }
        *label.write().await = "connecting".to_string();
        let outcome = connect_and_run(
            Arc::clone(&state),
            &cloud,
            &mut reconnect,
            &mut shutdown,
            Arc::clone(&label),
        )
        .await;
        // index.ts:271-278 — close tunneled sockets with 1012 on relay drop.
        close_all_local_sockets(&state, 1012, "Relay disconnected".to_string()).await;
        *label.write().await = "reconnecting".to_string();
        match outcome {
            RunOutcome::Shutdown => break,
            // Credential rotated: reconnect immediately with the new token.
            RunOutcome::ReconnectNow => continue,
            RunOutcome::Disconnected => {}
        }
        eprintln!("[ao-fabric] relay: reconnecting in {}s", retry.as_secs());
        tokio::select! {
            _ = tokio::time::sleep(retry) => {}
            _ = shutdown.changed() => {
                let _ = shutdown.borrow_and_update();
                break;
            }
        }
        retry = (retry * 2).min(MAX_RETRY);
    }
    close_all_local_sockets(&state, 1012, "Relay disconnected".to_string()).await;
}

enum RunOutcome {
    Shutdown,
    /// Rotation bumped the epoch — reconnect immediately with the new token.
    ReconnectNow,
    Disconnected,
}

async fn connect_and_run(
    state: Arc<RelayState>,
    cloud: &CloudClient,
    reconnect: &mut tokio::sync::watch::Receiver<u64>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    label: Arc<tokio::sync::RwLock<String>>,
) -> RunOutcome {
    let identity = state.snapshot().await;
    let (Ok(runtime_id), Ok(token)) = (identity.runtime_id(), identity.device_token()) else {
        eprintln!("[ao-fabric] relay: identity is not paired; cannot connect");
        return RunOutcome::Shutdown;
    };
    let url = relay_ws_url(&cloud.base_url, runtime_id);

    let (ws_stream, _response) = match connect_async(&url).await {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("[ao-fabric] relay: connect failed: {err}");
            return RunOutcome::Disconnected;
        }
    };
    let (mut sink, mut stream) = ws_stream.split();

    // authenticate-first handshake (runtime_relay.rs:618-634).
    let auth = NodeMessage::Authenticate {
        runtime_id: runtime_id.to_string(),
        device_token: token.to_string(),
    };
    if let Ok(frame) = serde_json::to_string(&auth) {
        if sink.send(Message::Text(frame)).await.is_err() {
            return RunOutcome::Disconnected;
        }
    }

    // Outbound frames: relay handlers and socket tasks send through here;
    // the writer task owns the relay sink.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<NodeMessage>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let Ok(text) = serde_json::to_string(&frame) else { continue };
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    let mut outcome = RunOutcome::Disconnected;

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                let _ = shutdown.borrow_and_update();
                outcome = RunOutcome::Shutdown;
                break;
            }
            changed = reconnect.changed() => {
                if changed.is_ok() {
                    eprintln!("[ao-fabric] relay: credential rotated — reconnecting on the new token");
                    outcome = RunOutcome::ReconnectNow;
                }
                break;
            }
            frame = stream.next() => {
                let Some(frame) = frame else { break };
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) => {
                        eprintln!("[ao-fabric] relay: {err}");
                        break;
                    }
                };
                match frame {
                    Message::Text(text) => {
                        let Ok(message) = serde_json::from_str::<CloudMessage>(&text) else { continue };
                        if matches!(message, CloudMessage::Authenticated { .. }) {
                            *label.write().await = "connected".to_string();
                        }
                        handle_cloud_message(Arc::clone(&state), &out_tx, message).await;
                    }
                    Message::Close(_) => break,
                    // WS-level ping/pong: the server's liveness is the JSON
                    // `ping` frame (answered with a JSON `pong` above).
                    _ => {}
                }
            }
        }
    }

    // Aborting the writer drops the sink, which closes the TCP stream and
    // unblocks any still-running socket tasks on their next send.
    writer.abort();
    outcome
}

async fn handle_cloud_message(
    state: Arc<RelayState>,
    out_tx: &mpsc::UnboundedSender<NodeMessage>,
    message: CloudMessage,
) {
    match message {
        CloudMessage::Authenticated { .. } => {
            println!("[ao-fabric] Secure runtime relay connected.");
        }
        CloudMessage::Ping => {
            let _ = out_tx.send(NodeMessage::Pong);
        }
        CloudMessage::Request {
            request_id,
            method,
            path,
            headers,
            body,
            body_encoding,
        } => {
            // index.ts:282-357 — handleRelayRequest.
            let state = Arc::clone(&state);
            let out_tx = out_tx.clone();
            tokio::spawn(async move {
                handle_relay_request(
                    state, out_tx, request_id, method, path, headers, body, body_encoding,
                )
                .await;
            });
        }
        CloudMessage::SocketOpen {
            socket_id,
            path,
            headers,
        } => {
            let state = Arc::clone(&state);
            let out_tx = out_tx.clone();
            tokio::spawn(async move {
                handle_relay_socket_open(state, out_tx, socket_id, path, headers).await;
            });
        }
        CloudMessage::SocketData {
            socket_id,
            body,
            body_encoding,
        } => {
            // index.ts:409-415 — handleRelaySocketData.
            let payload = if body_encoding == "base64" {
                STANDARD.decode(&body).unwrap_or_default()
            } else {
                body.into_bytes()
            };
            let mut sockets = state.local_sockets.lock().await;
            if let Some(socket) = sockets.get_mut(&socket_id) {
                let _ = socket.send(Message::Binary(payload)).await;
            }
        }
        CloudMessage::SocketClose {
            socket_id,
            code,
            reason,
        } => {
            // index.ts:417-423 — handleRelaySocketClose (code clamp 1000..4999).
            let code = if (1000..=4999).contains(&code) { code } else { 1000 };
            let mut sockets = state.local_sockets.lock().await;
            if let Some(mut socket) = sockets.remove(&socket_id) {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: reason.into(),
                    })))
                    .await;
            }
        }
        CloudMessage::Unknown => {}
    }
}

/// index.ts:282-357 — validate, forward to the loopback shim, stream the
/// response back as response_start / response_chunk(base64) / response_end.
async fn handle_relay_request(
    state: Arc<RelayState>,
    out_tx: mpsc::UnboundedSender<NodeMessage>,
    request_id: String,
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
    body_encoding: String,
) {
    let method = method.to_uppercase();
    if request_id.is_empty() || !allowed_path(&path) {
        return;
    }
    let identity = state.snapshot().await;
    let Ok(token) = identity.device_token() else { return };

    let send = |frame: NodeMessage| {
        let _ = out_tx.send(frame);
    };

    let url = format!("{}{path}", state.local_gateway);
    let Ok(method) = reqwest::Method::from_bytes(method.as_bytes()) else {
        return;
    };
    let mut request = state.http.request(method.clone(), &url);
    let mut header_map = reqwest::header::HeaderMap::new();
    for (name, value) in &headers {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            header_map.insert(name, value);
        }
    }
    // Prefer the caller's own Clerk JWT when the envelope carries one;
    // otherwise authenticate as the runtime itself (index.ts:298-308).
    if !header_map.contains_key(reqwest::header::AUTHORIZATION) {
        header_map.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    RelayState::inject_identity_headers(&identity, &mut header_map);
    request = request.headers(header_map);

    if method != reqwest::Method::GET && method != reqwest::Method::HEAD && !body.is_empty() {
        let bytes = if body_encoding == "base64" {
            STANDARD.decode(&body).unwrap_or_default()
        } else {
            body.into_bytes()
        };
        request = request.body(bytes);
    }

    let response = match tokio::time::timeout(FORWARD_TIMEOUT, request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => {
            send(NodeMessage::ResponseStart {
                request_id: request_id.clone(),
                status: 502,
                headers: [("content-type".to_string(), "application/json".to_string())]
                    .into_iter()
                    .collect(),
            });
            let payload =
                serde_json::json!({ "error": "runtime_proxy_error", "message": err.to_string() });
            send(NodeMessage::ResponseChunk {
                request_id: request_id.clone(),
                body: STANDARD.encode(payload.to_string()),
                body_encoding: "base64".to_string(),
            });
            send(NodeMessage::ResponseEnd { request_id: request_id.clone() });
            return;
        }
        Err(_) => return,
    };

    let mut response_headers = HashMap::new();
    for name in FORWARDED_RESPONSE_HEADERS {
        if let Some(value) = response.headers().get(*name) {
            if let Ok(value) = value.to_str() {
                response_headers.insert((*name).to_string(), value.to_string());
            }
        }
    }
    let status = response.status().as_u16();
    send(NodeMessage::ResponseStart {
        request_id: request_id.clone(),
        status,
        headers: response_headers,
    });
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        if !chunk.is_empty() {
            send(NodeMessage::ResponseChunk {
                request_id: request_id.clone(),
                body: STANDARD.encode(&chunk),
                body_encoding: "base64".to_string(),
            });
        }
    }
    send(NodeMessage::ResponseEnd { request_id });
}

/// index.ts:359-407 — dial the loopback shim's WS endpoint and relay frames
/// both ways until either side closes.
async fn handle_relay_socket_open(
    state: Arc<RelayState>,
    out_tx: mpsc::UnboundedSender<NodeMessage>,
    socket_id: String,
    path: String,
    _headers: HashMap<String, String>,
) {
    if socket_id.is_empty() || !allowed_path(&path) {
        return;
    }
    let identity = state.snapshot().await;
    let Ok(token) = identity.device_token() else { return };

    // Close any previous local socket under this id (index.ts:369).
    {
        let mut sockets = state.local_sockets.lock().await;
        if let Some(mut previous) = sockets.remove(&socket_id) {
            let _ = previous.send(Message::Close(None)).await;
        }
    }

    let url = format!("{}{path}", state.local_gateway.replacen("http", "ws", 1));
    let mut request = match url.as_str().into_client_request() {
        Ok(request) => request,
        Err(err) => {
            eprintln!("[ao-fabric] local runtime socket: bad url {url}: {err}");
            return;
        }
    };
    let headers = request.headers_mut();
    use tungstenite::http::HeaderValue;
    let _ = headers.append(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).expect("bearer header"),
    );
    let _ = headers.append(
        "X-Allternit-Desktop-Access-Token",
        HeaderValue::from_str(token).expect("token header"),
    );
    if let (Ok(user_id), Ok(email)) = (identity.user_id(), identity.user_email()) {
        let _ = headers.append(
            "X-Allternit-User-Id",
            HeaderValue::from_str(user_id).expect("user id header"),
        );
        let _ = headers.append(
            "X-Allternit-User-Email",
            HeaderValue::from_str(email).expect("email header"),
        );
    }
    if let Some(org) = identity.organization_id.as_deref() {
        if let Ok(value) = HeaderValue::from_str(org) {
            let _ = headers.append("X-Allternit-Tenant-Id", value);
        }
    }

    let Ok((local, _)) = connect_async(request).await else {
        eprintln!("[ao-fabric] local runtime socket: dial failed for {path}");
        let _ = out_tx.send(NodeMessage::SocketClose {
            socket_id,
            code: 1011,
            reason: "Local runtime socket failed".to_string(),
        });
        return;
    };
    let (local_sink, mut local_stream) = local.split();
    state
        .local_sockets
        .lock()
        .await
        .insert(socket_id.clone(), local_sink);

    // socket_ready once the local end is open (index.ts:380-384).
    let _ = out_tx.send(NodeMessage::SocketReady {
        socket_id: socket_id.clone(),
    });

    // Local → relay.
    while let Some(frame) = local_stream.next().await {
        let Ok(frame) = frame else { break };
        let message = match frame {
            Message::Text(text) => NodeMessage::SocketData {
                socket_id: socket_id.clone(),
                body: text,
                body_encoding: "utf8".to_string(),
            },
            Message::Binary(data) => NodeMessage::SocketData {
                socket_id: socket_id.clone(),
                body: STANDARD.encode(&data),
                body_encoding: "base64".to_string(),
            },
            Message::Close(frame) => NodeMessage::SocketClose {
                socket_id: socket_id.clone(),
                code: frame.as_ref().map(|frame| u16::from(frame.code)).unwrap_or(1000),
                reason: frame
                    .as_ref()
                    .map(|frame| frame.reason.to_string())
                    .unwrap_or_default(),
            },
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
        };
        let is_close = matches!(message, NodeMessage::SocketClose { .. });
        let _ = out_tx.send(message);
        if is_close {
            break;
        }
    }
    state.local_sockets.lock().await.remove(&socket_id);
}

async fn close_all_local_sockets(state: &Arc<RelayState>, code: u16, reason: String) {
    let mut sockets = state.local_sockets.lock().await;
    for (_socket_id, mut socket) in sockets.drain() {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: reason.clone().into(),
            })))
            .await;
    }
}

fn relay_ws_url(base_url: &str, runtime_id: &str) -> String {
    let url = format!("{base_url}/api/v1/runtime-relay/connect/{runtime_id}");
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_allow_list_matches_agent_daemon() {
        assert!(allowed_path("/v1/remote-control/sessions"));
        assert!(allowed_path("/api/v1/sessions"));
        assert!(allowed_path("/status"));
        assert!(allowed_path("/ws/panes"));
        assert!(!allowed_path("/etc/passwd"));
        assert!(!allowed_path("/api/../secret"));
        assert!(!allowed_path("https://evil.example/"));
        assert!(!allowed_path("relative/path"));
        assert!(!allowed_path("/"));
    }

    #[test]
    fn relay_url_switches_to_wss() {
        assert_eq!(
            relay_ws_url("https://api.allternit.com", "rt_1"),
            "wss://api.allternit.com/api/v1/runtime-relay/connect/rt_1"
        );
        assert_eq!(
            relay_ws_url("http://127.0.0.1:8080", "rt_1"),
            "ws://127.0.0.1:8080/api/v1/runtime-relay/connect/rt_1"
        );
    }

    #[test]
    fn backoff_doubles_to_thirty_seconds() {
        let mut retry = INITIAL_RETRY;
        retry = (retry * 2).min(MAX_RETRY);
        assert_eq!(retry, Duration::from_secs(2));
        for _ in 0..10 {
            retry = (retry * 2).min(MAX_RETRY);
        }
        assert_eq!(retry, MAX_RETRY);
    }
}
