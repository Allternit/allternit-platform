//! Share gate: the loopback listener a live link's tunnel client points at.
//!
//! Apps + MCP 2026 program, plan v2 step 15 (decisions D2, D7, D10).
//!
//! - D2: every share gets its own listener on an ephemeral **loopback** port.
//!   The OpenTunnel client forwards visitors to this gate, never to the app
//!   and never to the runtime's own API on :8013.
//! - D7: the gate forwards only to `127.0.0.1:<the one port the user shared>`.
//!   The runtime's own ports (allternit-api 8013/18013, gizzi 4096, every
//!   other port in Desktop's `PORTS` table in
//!   `surfaces/allternit-desktop/src/main/config.ts`, the env-configured
//!   service ports, and every other live gate) are refused, both when a share
//!   is created and again on every request.
//! - D10: token is the default auth mode. A visitor shows the per-share token
//!   as `Authorization: Bearer <token>`, as `X-Allternit-Share-Token`, as the
//!   `allternit_share` cookie, or once as `?allternit_share_token=<token>`
//!   (the gate then sets the cookie and redirects to the same URL without the
//!   token). No or wrong token → 401. `public` mode is only for plain app
//!   shares; MCP shares always need the token.
//!
//! Only the SHA-256 of the token is kept. The gate strips forwarding headers
//! (`Forwarded`, `X-Forwarded-*`, `X-Real-IP`), every `X-Allternit-*` header,
//! hop-by-hop headers and the share credential itself before forwarding, so
//! the shared app never sees the token and cannot be fooled about the caller.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ws::{CloseFrame as AxumCloseFrame, Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as TungCloseFrame;
use tokio_tungstenite::tungstenite::Message as TungMessage;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Cookie the gate sets after a `?allternit_share_token=` visit.
pub const SHARE_COOKIE: &str = "allternit_share";
/// One-time query parameter that carries the token on a first browser visit.
pub const SHARE_QUERY_PARAM: &str = "allternit_share_token";
/// Header alternative to `Authorization: Bearer` for clients that need their
/// own `Authorization` header to reach the shared app.
pub const SHARE_HEADER: &str = "x-allternit-share-token";

/// Ports from Desktop's `PORTS` table (`surfaces/allternit-desktop/src/main/config.ts`).
/// Keep in sync when a port is added there.
pub const BUILTIN_INTERNAL_PORTS: &[u16] = &[
    8013,  // API (allternit-api, production)
    18013, // API_DEV (allternit-api, dev default)
    4096,  // GIZZI
    3014,  // DEV_UI
    3021,  // RAILS
    3011,  // EXTENSION_BRIDGE
    3012,  // ACU_RELAY
    8760,  // ACU
    5055,  // NOTEBOOK
    8001,  // VOICE
    8080,  // WEBHOOK_RECEIVER
    8014,  // CONNECTOR_SIDECAR
    8099,  // OFFICE_ENGINE
    3015,  // LOCAL_ENGINE
    3018,  // FACTORY
    7787,  // PET_HOOK
];

/// Env vars that hold a bare port of a runtime service.
const PORT_ENVS: &[&str] = &["ALLTERNIT_API_PORT", "ALLTERNIT_WEBHOOK_RECEIVER_PORT", "GIZZI_PORT"];
/// Env vars that hold a URL or socket address of a runtime service.
const ADDR_ENVS: &[&str] = &[
    "TERMINAL_SERVER_URL",
    "ALLTERNIT_ACU_URL",
    "OFFICE_ENGINE_URL",
    "ALLTERNIT_FACTORY_URL",
    "ALLTERNIT_ADMIN_LISTEN_ADDR",
];
/// Comma-separated extra ports an operator wants refused.
pub const DENY_PORTS_ENV: &str = "ALLTERNIT_SHARE_DENY_PORTS";

/// What a share exposes. MCP shares always require the token (D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareSubject {
    App,
    Mcp,
}

/// How visitors authenticate at the gate (D10: token is the default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareAuthMode {
    #[default]
    Token,
    Public,
}

/// Why a share target or policy was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareRefusal {
    /// Port 0 is not a real target.
    InvalidPort,
    /// The port belongs to the runtime itself (or another gate).
    InternalPort(u16),
    /// `public` was asked for an MCP share.
    PublicMcp,
}

impl ShareRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            ShareRefusal::InvalidPort => "invalid_port",
            ShareRefusal::InternalPort(_) => "port_refused",
            ShareRefusal::PublicMcp => "public_mcp_refused",
        }
    }

    pub fn message(&self) -> String {
        match self {
            ShareRefusal::InvalidPort => "Pick a port between 1 and 65535.".to_string(),
            ShareRefusal::InternalPort(port) => {
                format!("Port {port} belongs to the Allternit runtime and cannot be shared.")
            }
            ShareRefusal::PublicMcp => "MCP servers always need the share token; public is for apps only.".to_string(),
        }
    }
}

fn parse_port_value(raw: &str) -> Option<u16> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(port) = raw.parse::<u16>() {
        return Some(port);
    }
    if let Ok(addr) = raw.parse::<SocketAddr>() {
        return Some(addr.port());
    }
    reqwest::Url::parse(raw).ok().and_then(|url| url.port_or_known_default())
}

/// Every port the gate refuses to forward to, read fresh (env can change).
pub fn internal_ports() -> BTreeSet<u16> {
    let mut ports: BTreeSet<u16> = BUILTIN_INTERNAL_PORTS.iter().copied().collect();
    for name in PORT_ENVS.iter().chain(ADDR_ENVS.iter()) {
        if let Some(port) = std::env::var(name).ok().as_deref().and_then(parse_port_value) {
            ports.insert(port);
        }
    }
    if let Ok(extra) = std::env::var(DENY_PORTS_ENV) {
        ports.extend(extra.split(',').filter_map(parse_port_value));
    }
    ports
}

/// Check one target port against the internal list plus `also_denied`
/// (the ports of the other live gates).
pub fn check_target_port(port: u16, also_denied: &BTreeSet<u16>) -> Result<(), ShareRefusal> {
    if port == 0 {
        return Err(ShareRefusal::InvalidPort);
    }
    if also_denied.contains(&port) || internal_ports().contains(&port) {
        return Err(ShareRefusal::InternalPort(port));
    }
    Ok(())
}

/// A new random share token (`ast_` + 64 hex chars). Shown once, never stored.
pub fn mint_token() -> String {
    let bytes: [u8; 32] = rand::random();
    format!("ast_{}", hex::encode(bytes))
}

fn hash_token(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hasher.finalize().into()
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The policy one gate enforces. Shared between the gate and the share routes
/// (pause, rotate, stop).
#[derive(Debug)]
pub struct GatePolicy {
    pub share_id: String,
    pub target_port: u16,
    pub subject: ShareSubject,
    pub auth_mode: ShareAuthMode,
    token_hash: RwLock<Option<[u8; 32]>>,
    paused: AtomicBool,
    cancel: CancellationToken,
}

impl GatePolicy {
    /// Build a policy. Refuses internal ports and public MCP shares. Returns
    /// the policy plus the plaintext token (None for public shares).
    pub fn new(
        share_id: impl Into<String>,
        target_port: u16,
        subject: ShareSubject,
        auth_mode: ShareAuthMode,
        also_denied: &BTreeSet<u16>,
    ) -> Result<(Arc<Self>, Option<String>), ShareRefusal> {
        check_target_port(target_port, also_denied)?;
        if subject == ShareSubject::Mcp && auth_mode == ShareAuthMode::Public {
            return Err(ShareRefusal::PublicMcp);
        }
        let token = match auth_mode {
            ShareAuthMode::Token => Some(mint_token()),
            ShareAuthMode::Public => None,
        };
        let policy = Arc::new(Self {
            share_id: share_id.into(),
            target_port,
            subject,
            auth_mode,
            token_hash: RwLock::new(token.as_deref().map(hash_token)),
            paused: AtomicBool::new(false),
            cancel: CancellationToken::new(),
        });
        Ok((policy, token))
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Stop the gate: the listener closes, open requests and sockets end, and
    /// any kept-alive connection gets 410 from now on.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Replace the token; the old one stops working at once. Returns the new
    /// plaintext token, or None for a public share.
    pub fn rotate_token(&self) -> Option<String> {
        if self.auth_mode == ShareAuthMode::Public {
            return None;
        }
        let token = mint_token();
        *self.token_hash.write().unwrap_or_else(|e| e.into_inner()) = Some(hash_token(&token));
        Some(token)
    }

    /// True when `candidate` is this share's token.
    pub fn token_matches(&self, candidate: &str) -> bool {
        let guard = self.token_hash.read().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some(expected) => constant_time_eq(expected, &hash_token(candidate.trim())),
            None => false,
        }
    }
}

/// A running gate listener. Dropping it stops the gate.
#[derive(Debug)]
pub struct RunningGate {
    pub policy: Arc<GatePolicy>,
    pub addr: SocketAddr,
}

impl RunningGate {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

impl Drop for RunningGate {
    fn drop(&mut self) {
        self.policy.stop();
    }
}

#[derive(Clone)]
struct GateState {
    policy: Arc<GatePolicy>,
    client: reqwest::Client,
}

/// Bind `127.0.0.1:0` and serve the gate for `policy` until it is stopped.
pub async fn spawn_gate(policy: Arc<GatePolicy>) -> std::io::Result<RunningGate> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let client = reqwest::Client::builder()
        // Never follow redirects: the visitor's browser does that.
        .redirect(reqwest::redirect::Policy::none())
        // A system HTTP proxy must not see loopback traffic.
        .no_proxy()
        // Pass bodies through untouched.
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(std::io::Error::other)?;
    let app = gate_router(GateState { policy: policy.clone(), client });
    let shutdown = policy.cancel.clone().cancelled_owned();
    let share_id = policy.share_id.clone();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(shutdown)
            .await
        {
            warn!(share_id = %share_id, error = %e, "share gate stopped with an error");
        }
    });
    info!(share_id = %policy.share_id, gate = %addr, target_port = policy.target_port, "share gate listening");
    Ok(RunningGate { policy, addr })
}

fn gate_router(state: GateState) -> Router {
    Router::new().fallback(gate_handler).with_state(state)
}

fn gate_error(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response = (status, Json(json!({ "error": code, "message": message }))).into_response();
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"allternit-share\""),
        );
    }
    response
}

/// Where the visitor's credential came from, so it can be stripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    Authorization,
    Header,
    Cookie,
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| v.trim())
}

fn presented_token(headers: &HeaderMap) -> Option<(String, CredentialSource)> {
    if let Some(value) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let value = value.trim();
        if value.len() > 7 && value[..7].eq_ignore_ascii_case("bearer ") {
            return Some((value[7..].trim().to_string(), CredentialSource::Authorization));
        }
    }
    if let Some(value) = headers.get(SHARE_HEADER).and_then(|v| v.to_str().ok()) {
        return Some((value.trim().to_string(), CredentialSource::Header));
    }
    cookie_value(headers, SHARE_COOKIE).map(|v| (v.to_string(), CredentialSource::Cookie))
}

/// Split `?allternit_share_token=` out of a path+query. Returns the token and
/// the path+query without it.
fn take_query_token(path_and_query: &str) -> Option<(String, String)> {
    let url = reqwest::Url::parse(&format!("http://gate.local{path_and_query}")).ok()?;
    let mut token = None;
    let mut rest = url::form_urlencoded::Serializer::new(String::new());
    let mut kept = 0usize;
    for (key, value) in url.query_pairs() {
        if key == SHARE_QUERY_PARAM {
            token = Some(value.into_owned());
        } else {
            rest.append_pair(&key, &value);
            kept += 1;
        }
    }
    let token = token?;
    let query = rest.finish();
    let location = if kept == 0 { url.path().to_string() } else { format!("{}?{}", url.path(), query) };
    Some((token, location))
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// Headers that must never reach the shared app.
fn is_stripped_request_header(name: &HeaderName) -> bool {
    let n = name.as_str();
    is_hop_by_hop(name)
        || n == "host"
        || n == "forwarded"
        || n == "x-real-ip"
        || n.starts_with("x-forwarded-")
        || n.starts_with("x-allternit-")
}

/// The request headers forwarded to the app: forwarding/identity headers and
/// the share credential removed; other cookies and auth kept.
fn upstream_headers(headers: &HeaderMap, credential: Option<CredentialSource>) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers.iter() {
        if is_stripped_request_header(name) {
            continue;
        }
        if name == header::AUTHORIZATION && credential == Some(CredentialSource::Authorization) {
            continue;
        }
        if name == header::COOKIE {
            let kept: Vec<&str> = value
                .to_str()
                .unwrap_or("")
                .split(';')
                .map(str::trim)
                .filter(|pair| !pair.is_empty())
                .filter(|pair| pair.split_once('=').map(|(k, _)| k.trim() != SHARE_COOKIE).unwrap_or(true))
                .collect();
            if !kept.is_empty() {
                if let Ok(v) = HeaderValue::from_str(&kept.join("; ")) {
                    out.append(header::COOKIE, v);
                }
            }
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

fn is_websocket_request(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

async fn gate_handler(State(state): State<GateState>, request: Request) -> Response {
    let policy = &state.policy;
    if policy.is_stopped() {
        return gate_error(StatusCode::GONE, "share_stopped", "This share has stopped.");
    }
    // D7, checked again per request: env may have changed since create.
    if let Err(refusal) = check_target_port(policy.target_port, &BTreeSet::new()) {
        return gate_error(StatusCode::FORBIDDEN, refusal.code(), &refusal.message());
    }
    if policy.is_paused() {
        return gate_error(StatusCode::SERVICE_UNAVAILABLE, "share_paused", "This share is paused.");
    }

    let (mut parts, body) = request.into_parts();
    let path_and_query = parts.uri.path_and_query().map(|pq| pq.as_str().to_string()).unwrap_or_else(|| "/".to_string());

    let mut credential = None;
    if policy.auth_mode == ShareAuthMode::Token {
        // First browser visit: token in the query → cookie + clean redirect.
        if let Some((token, location)) = take_query_token(&path_and_query) {
            if !policy.token_matches(&token) {
                return gate_error(StatusCode::UNAUTHORIZED, "share_token_invalid", "The share token is not valid.");
            }
            let cookie = format!("{SHARE_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax");
            let mut response = StatusCode::SEE_OTHER.into_response();
            if let (Ok(loc), Ok(set)) = (HeaderValue::from_str(&location), HeaderValue::from_str(&cookie)) {
                response.headers_mut().insert(header::LOCATION, loc);
                response.headers_mut().insert(header::SET_COOKIE, set);
                response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            }
            return response;
        }
        match presented_token(&parts.headers) {
            None => {
                return gate_error(StatusCode::UNAUTHORIZED, "share_token_required", "This share needs its token.");
            }
            Some((token, source)) => {
                if !policy.token_matches(&token) {
                    return gate_error(StatusCode::UNAUTHORIZED, "share_token_invalid", "The share token is not valid.");
                }
                credential = Some(source);
            }
        }
    }

    let forwarded = upstream_headers(&parts.headers, credential);

    if is_websocket_request(&parts.headers) {
        let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
            Ok(ws) => ws,
            Err(rejection) => return rejection.into_response(),
        };
        return proxy_websocket(ws, policy.clone(), &path_and_query, &forwarded).await;
    }

    let url = format!("http://127.0.0.1:{}{}", policy.target_port, path_and_query);
    let has_body = parts.headers.contains_key(header::CONTENT_LENGTH) || parts.headers.contains_key(header::TRANSFER_ENCODING);
    let mut upstream = state.client.request(parts.method.clone(), &url).headers(forwarded);
    if has_body {
        upstream = upstream.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    }
    let response = match upstream.send().await {
        Ok(response) => response,
        Err(e) => {
            warn!(share_id = %policy.share_id, error = %e, "share target unreachable");
            return gate_error(StatusCode::BAD_GATEWAY, "share_target_unreachable", "The shared app is not answering.");
        }
    };

    let mut builder = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if !is_hop_by_hop(name) {
            builder = builder.header(name, value);
        }
    }
    let cancel = policy.cancel.clone();
    // End long-lived streams (SSE) as soon as the share stops.
    let stream = response.bytes_stream().take_until(cancel.cancelled_owned());
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| gate_error(StatusCode::BAD_GATEWAY, "share_bad_response", "The shared app sent a bad response."))
}

/// WebSocket passthrough to `ws://127.0.0.1:<port>`. The upstream handshake
/// runs first so its chosen subprotocol (e.g. `vite-hmr`) is echoed back.
async fn proxy_websocket(
    ws: WebSocketUpgrade,
    policy: Arc<GatePolicy>,
    path_and_query: &str,
    forwarded: &HeaderMap,
) -> Response {
    use tokio_tungstenite::tungstenite::http::{HeaderName as TName, HeaderValue as TValue};

    let url = format!("ws://127.0.0.1:{}{}", policy.target_port, path_and_query);
    let mut upstream_request = match url.as_str().into_client_request() {
        Ok(request) => request,
        Err(_) => return gate_error(StatusCode::BAD_REQUEST, "share_bad_request", "Bad WebSocket URL."),
    };
    for (name, value) in forwarded.iter() {
        // tungstenite writes its own handshake headers.
        if name.as_str().starts_with("sec-websocket-") && name.as_str() != "sec-websocket-protocol" {
            continue;
        }
        if let (Ok(n), Ok(v)) = (TName::from_bytes(name.as_str().as_bytes()), TValue::from_bytes(value.as_bytes())) {
            upstream_request.headers_mut().append(n, v);
        }
    }
    let (upstream, handshake) = match tokio_tungstenite::connect_async(upstream_request).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!(share_id = %policy.share_id, error = %e, "share websocket target unreachable");
            return gate_error(StatusCode::BAD_GATEWAY, "share_target_unreachable", "The shared app refused the WebSocket.");
        }
    };
    let protocol = handshake
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let ws = match protocol {
        Some(p) => ws.protocols(vec![p]),
        None => ws,
    };
    let cancel = policy.cancel.clone();
    ws.on_upgrade(move |socket| pump_websocket(socket, upstream, cancel))
}

async fn pump_websocket(
    client: WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    cancel: CancellationToken,
) {
    let (mut client_tx, mut client_rx) = client.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    let to_upstream = async {
        while let Some(Ok(message)) = client_rx.next().await {
            if upstream_tx.send(axum_to_tung(message)).await.is_err() {
                break;
            }
        }
        let _ = upstream_tx.close().await;
    };
    let to_client = async {
        while let Some(Ok(message)) = upstream_rx.next().await {
            let Some(message) = tung_to_axum(message) else { continue };
            if client_tx.send(message).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    };
    tokio::select! {
        _ = to_upstream => {}
        _ = to_client => {}
        _ = cancel.cancelled() => {}
    }
}

fn axum_to_tung(message: AxumMessage) -> TungMessage {
    match message {
        AxumMessage::Text(text) => TungMessage::Text(text),
        AxumMessage::Binary(data) => TungMessage::Binary(data),
        AxumMessage::Ping(data) => TungMessage::Ping(data),
        AxumMessage::Pong(data) => TungMessage::Pong(data),
        AxumMessage::Close(frame) => TungMessage::Close(frame.map(|f| TungCloseFrame {
            code: CloseCode::from(f.code),
            reason: f.reason,
        })),
    }
}

fn tung_to_axum(message: TungMessage) -> Option<AxumMessage> {
    Some(match message {
        TungMessage::Text(text) => AxumMessage::Text(text),
        TungMessage::Binary(data) => AxumMessage::Binary(data),
        TungMessage::Ping(data) => AxumMessage::Ping(data),
        TungMessage::Pong(data) => AxumMessage::Pong(data),
        TungMessage::Close(frame) => AxumMessage::Close(frame.map(|f| AxumCloseFrame {
            code: u16::from(f.code),
            reason: f.reason,
        })),
        TungMessage::Frame(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;

    /// A stand-in for the user's app: echoes what the gate forwarded.
    async fn spawn_app() -> u16 {
        async fn echo(headers: HeaderMap) -> Json<serde_json::Value> {
            let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            Json(json!({
                "ok": true,
                "authorization": get("authorization"),
                "x_forwarded_for": get("x-forwarded-for"),
                "x_allternit_user_id": get("x-allternit-user-id"),
                "share_header": get(SHARE_HEADER),
                "cookie": get("cookie"),
            }))
        }
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route("/", get(echo)).route("/page", get(echo));
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        port
    }

    fn http() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap()
    }

    #[test]
    fn share_gate_refuses_runtime_ports() {
        let none = BTreeSet::new();
        assert_eq!(check_target_port(8013, &none), Err(ShareRefusal::InternalPort(8013)));
        assert_eq!(check_target_port(18013, &none), Err(ShareRefusal::InternalPort(18013)));
        assert_eq!(check_target_port(4096, &none), Err(ShareRefusal::InternalPort(4096)));
        assert_eq!(check_target_port(0, &none), Err(ShareRefusal::InvalidPort));
        // Another live gate's port is refused too (no gate chaining).
        let gates: BTreeSet<u16> = [51234].into_iter().collect();
        assert_eq!(check_target_port(51234, &gates), Err(ShareRefusal::InternalPort(51234)));
        // An ordinary dev server port is fine.
        assert_eq!(check_target_port(5173, &none), Ok(()));
        // And a policy for an internal port is never built.
        let refused = GatePolicy::new("shr_x", 8013, ShareSubject::App, ShareAuthMode::Token, &none);
        assert_eq!(refused.unwrap_err(), ShareRefusal::InternalPort(8013));
        let refused = GatePolicy::new("shr_x", 4096, ShareSubject::Mcp, ShareAuthMode::Token, &none);
        assert_eq!(refused.unwrap_err(), ShareRefusal::InternalPort(4096));
    }

    #[test]
    fn share_gate_refuses_public_mcp() {
        let refused = GatePolicy::new("shr_x", 5173, ShareSubject::Mcp, ShareAuthMode::Public, &BTreeSet::new());
        assert_eq!(refused.unwrap_err(), ShareRefusal::PublicMcp);
    }

    #[tokio::test]
    async fn share_gate_missing_or_wrong_token_is_401() {
        let app_port = spawn_app().await;
        let (policy, token) =
            GatePolicy::new("shr_t", app_port, ShareSubject::App, ShareAuthMode::Token, &BTreeSet::new()).unwrap();
        let token = token.unwrap();
        let gate = spawn_gate(policy).await.unwrap();
        let base = format!("http://{}", gate.addr);

        let none = http().get(&base).send().await.unwrap();
        assert_eq!(none.status(), StatusCode::UNAUTHORIZED);
        let body: serde_json::Value = none.json().await.unwrap();
        assert_eq!(body["error"], "share_token_required");

        let wrong = http().get(&base).bearer_auth("ast_wrong").send().await.unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let ok = http().get(&base).bearer_auth(&token).send().await.unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body: serde_json::Value = ok.json().await.unwrap();
        assert_eq!(body["ok"], true);
        // The share token never reaches the app.
        assert!(body["authorization"].is_null());
    }

    #[tokio::test]
    async fn share_gate_strips_forwarding_and_identity_headers() {
        let app_port = spawn_app().await;
        let (policy, token) =
            GatePolicy::new("shr_h", app_port, ShareSubject::App, ShareAuthMode::Token, &BTreeSet::new()).unwrap();
        let token = token.unwrap();
        let gate = spawn_gate(policy).await.unwrap();
        let response = http()
            .get(format!("http://{}/", gate.addr))
            .header(SHARE_HEADER, &token)
            .header("authorization", "Bearer app-own-token")
            .header("x-forwarded-for", "203.0.113.9")
            .header("x-allternit-user-id", "spoofed")
            .header("cookie", format!("{SHARE_COOKIE}=x; theme=dark"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["x_forwarded_for"].is_null());
        assert!(body["x_allternit_user_id"].is_null());
        assert!(body["share_header"].is_null());
        // The app's own credentials pass through; the share cookie does not.
        assert_eq!(body["authorization"], "Bearer app-own-token");
        assert_eq!(body["cookie"], "theme=dark");
    }

    #[tokio::test]
    async fn share_gate_query_token_sets_cookie_and_redirects() {
        let app_port = spawn_app().await;
        let (policy, token) =
            GatePolicy::new("shr_q", app_port, ShareSubject::App, ShareAuthMode::Token, &BTreeSet::new()).unwrap();
        let token = token.unwrap();
        let gate = spawn_gate(policy).await.unwrap();
        let response = http()
            .get(format!("http://{}/page?a=1&{SHARE_QUERY_PARAM}={token}", gate.addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/page?a=1");
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(cookie.starts_with(&format!("{SHARE_COOKIE}={token};")));
        assert!(cookie.contains("HttpOnly"));

        let follow = http()
            .get(format!("http://{}/page?a=1", gate.addr))
            .header("cookie", format!("{SHARE_COOKIE}={token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(follow.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn share_gate_pause_rotate_and_stop() {
        let app_port = spawn_app().await;
        let (policy, token) =
            GatePolicy::new("shr_p", app_port, ShareSubject::Mcp, ShareAuthMode::Token, &BTreeSet::new()).unwrap();
        let old = token.unwrap();
        let gate = spawn_gate(policy.clone()).await.unwrap();
        let base = format!("http://{}/", gate.addr);

        policy.set_paused(true);
        let paused = http().get(&base).bearer_auth(&old).send().await.unwrap();
        assert_eq!(paused.status(), StatusCode::SERVICE_UNAVAILABLE);
        policy.set_paused(false);

        let new = policy.rotate_token().unwrap();
        assert_eq!(http().get(&base).bearer_auth(&old).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(http().get(&base).bearer_auth(&new).send().await.unwrap().status(), StatusCode::OK);

        // A client holding a kept-alive connection gets 410 after stop.
        let client = http();
        assert_eq!(client.get(&base).bearer_auth(&new).send().await.unwrap().status(), StatusCode::OK);
        drop(gate);
        assert!(policy.is_stopped());
        if let Ok(response) = client.get(&base).bearer_auth(&new).send().await {
            assert_eq!(response.status(), StatusCode::GONE);
        }
    }

    #[tokio::test]
    async fn share_gate_public_app_needs_no_token() {
        let app_port = spawn_app().await;
        let (policy, token) =
            GatePolicy::new("shr_pub", app_port, ShareSubject::App, ShareAuthMode::Public, &BTreeSet::new()).unwrap();
        assert!(token.is_none());
        let gate = spawn_gate(policy).await.unwrap();
        let response = http().get(format!("http://{}/", gate.addr)).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
