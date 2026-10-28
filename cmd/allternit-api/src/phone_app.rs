//! iPhone via the App Store app (SPEC-iphone-app-screen-2026-10-27 §2).
//!
//! The Allternit iOS app and its ReplayKit broadcast extension dial out to
//! api.allternit.com; the cloud relay (`runtime_relay` socket tickets minted
//! by `app_phones` with the phone's app phone key) tunnels three WebSockets
//! here. They arrive like every relayed socket, with the Desktop's access
//! token, so the auth middleware's AuthUser is the account. The phone is
//! `app:<phoneId>` everywhere and reuses the phone-link registry, so viewers
//! keep using `/api/v1/phones/:serial/ws-token` + `/ws/phones/:serial/stream`.
//!
//! # Sockets (relayed)
//!
//! * `WS /api/v1/phones/app/pair?phone=<id>` — plaintext JSON:
//!   1. app → `{"type":"hello","phoneId","name","model","pub":"<X25519 b64>"}`
//!   2. node → `{"type":"pending","pub":"<node X25519 b64>"}`; both sides now
//!      show the 6-digit code, and the Desktop gets the SSE event
//!      `phone_app.pair_request` (with `code`) on `/api/v1/events/stream`.
//!   3. node → `{"type":"approved","pub"}` or `{"type":"denied","reason"}`
//!      (reasons: `denied`, `timeout`, `replaced`, `bad_key`, `store_failed`).
//!      Desktop gets `phone_app.pair_resolved`.
//!   Session key + code: `phone_app_envelope`. The key is kept sealed in the
//!   runtime vault (`allternit_vault_credentials`, provider `app_phone:<id>`).
//! * `WS /api/v1/phones/app/ingest?phone=<id>` — the broadcast extension.
//!   Binary frames, one iCTRL broadcast packet each: the packet's envelope
//!   JSON as UTF-8 bytes. The opened packet goes to viewers as a binary frame. Node → app text
//!   frames (plaintext): `{"type":"resume"}` on the first viewer,
//!   `{"type":"keyframe"}` on a later viewer, `{"type":"pause"}` when the last
//!   leaves. The phone is online while this socket is open.
//! * `WS /api/v1/phones/app/control?phone=<id>` — the foreground app. Text
//!   frames only, sealed with the request construction both ways. Node → app:
//!   `{"id","method","params"}`; app → node: `{"id","result"}`,
//!   `{"id","error":{"code","message"}}` or `{"type":"state","foreground","browserOpen"}`.
//!
//! An unpaired phone (no stored key) gets `{"type":"end","reason":"not_paired"}`
//! and close 4401 on ingest/control.
//!
//! # Desktop routes (loopback only: desktop access token, not relayed)
//!
//! * `POST /api/v1/phones/app/:phoneId/rpc` `{method, params}` → 200
//!   `{"result"}` or 200 `{"error":{"code","message"}}` (the app's error), 409
//!   `{"error":"app_not_open"}`, 504 `{"error":"app_timeout"}`.
//! * `POST /api/v1/phones/app/pair/:phoneId/decision` `{approve}` → `{"ok":true}`
//!   or 404 `{"error":"no_pending_pair"}`.
//! * `GET /api/v1/phones/app/pair/pending` → `{"pairs":[…pair_request payloads]}`
//!   (a Desktop that reconnects its event stream catches up here).
//!
//! Keys and frame bytes are never logged.

use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade};
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::auth::AuthUser;
use crate::phone_app_envelope::{pairing_code, EnvelopeKey, NodeKeyPair, ReplayGuard};
use crate::phone_ws::{
    fan_out_to_viewers, FrameTransport, PhoneHelloPhone, PhoneLinkSession, WsFrameTransport,
};
use crate::AppState;

pub const APP_SERIAL_PREFIX: &str = "app:";
pub const APP_ROUTE: &str = "iphone-app-store";
pub const EVENT_PAIR_REQUEST: &str = "phone_app.pair_request";
pub const EVENT_PAIR_RESOLVED: &str = "phone_app.pair_resolved";
const VAULT_PROVIDER_PREFIX: &str = "app_phone:";
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PAIR_DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const HEARTBEAT_DEAD: Duration = Duration::from_secs(90);
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WEB_WAIT_MS: u64 = 10_000;
/// Consecutive packets that fail to open before the ingest gives up (a
/// rotated key: the phone has to pair again).
const MAX_BAD_PACKETS: u32 = 50;
const NOT_PAIRED_CLOSE: u16 = 4401;
const RPC_METHODS: &[&str] = &[
    "web.open",
    "web.snapshot",
    "web.click",
    "web.type",
    "web.scroll",
    "web.back",
    "web.wait",
    "screen.prompt",
];

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── Hub (in-memory, per api instance, like the phone-link registry) ─────────

/// One app-phone event for the Desktop's event stream.
#[derive(Clone, Debug)]
pub struct AppPhoneEvent {
    pub account: String,
    pub name: &'static str,
    pub data: Value,
}

struct PendingPair {
    id: u64,
    request: Value,
    decide: oneshot::Sender<bool>,
}

struct ControlConn {
    id: u64,
    /// Plaintext JSON requests; the control pump seals them.
    to_app: mpsc::Sender<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    state: RwLock<Value>,
}

type PhoneKey = (String, String);

struct Hub {
    pairs: Mutex<HashMap<PhoneKey, PendingPair>>,
    controls: RwLock<HashMap<PhoneKey, Arc<ControlConn>>>,
    /// One replay window per phone key, shared by its ingest and control
    /// sockets (both seal requests under the same key).
    replay: Mutex<HashMap<PhoneKey, Arc<std::sync::Mutex<ReplayGuard>>>>,
    events: broadcast::Sender<AppPhoneEvent>,
    next_id: AtomicU64,
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        pairs: Mutex::new(HashMap::new()),
        controls: RwLock::new(HashMap::new()),
        replay: Mutex::new(HashMap::new()),
        events: broadcast::channel(64).0,
        next_id: AtomicU64::new(1),
    })
}

/// The Desktop event stream (`events_feed`) subscribes here.
pub fn subscribe() -> broadcast::Receiver<AppPhoneEvent> {
    hub().events.subscribe()
}

fn emit(account: &str, name: &'static str, data: Value) {
    let _ = hub().events.send(AppPhoneEvent {
        account: account.to_string(),
        name,
        data,
    });
}

async fn replay_guard(key: &PhoneKey) -> Arc<std::sync::Mutex<ReplayGuard>> {
    hub().replay
        .lock()
        .await
        .entry(key.clone())
        .or_default()
        .clone()
}

/// True while the iOS app holds its control socket for this phone.
pub async fn control_open(account: &str, phone_id: &str) -> bool {
    let Some(phone_id) = canonical_phone_id(phone_id) else { return false };
    hub().controls.read().await.contains_key(&(account.to_string(), phone_id))
}

/// Add the app-phone fields to a `GET /api/v1/phones` row.
pub async fn decorate_listing(account: &str, phone_id: &str, phone: &mut Value) {
    let control = hub()
        .controls
        .read()
        .await
        .get(&(account.to_string(), phone_id.to_string()))
        .cloned();
    let state = match &control {
        Some(conn) => conn.state.read().await.clone(),
        None => json!({}),
    };
    if let Some(obj) = phone.as_object_mut() {
        obj.insert("route".into(), json!(APP_ROUTE));
        obj.insert("screen".into(), json!(true));
        obj.insert("appOpen".into(), json!(control.is_some()));
        obj.insert("foreground".into(), state.get("foreground").cloned().unwrap_or(json!(false)));
        obj.insert("browserOpen".into(), state.get("browserOpen").cloned().unwrap_or(json!(false)));
    }
}

// ── Stored session key (runtime vault) ──────────────────────────────────────

struct StoredPhone {
    key: EnvelopeKey,
    name: Option<String>,
    model: Option<String>,
}

async fn store_phone(
    state: &AppState,
    user: &AuthUser,
    phone_id: &str,
    key: &EnvelopeKey,
    name: &Option<String>,
    model: &Option<String>,
) -> bool {
    let vault = crate::allternit_vault::AllternitVault::new(state.db.clone());
    let user = user.clone();
    let provider = format!("{VAULT_PROVIDER_PREFIX}{phone_id}");
    let value = json!({ "key": key.to_hex(), "name": name, "model": model }).to_string();
    match tokio::task::spawn_blocking(move || vault.put(&user, &provider, None, None, &value, None)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            warn!(error = %e, "app phone key not stored");
            false
        }
        Err(_) => false,
    }
}

async fn load_phone(state: &AppState, account: &str, phone_id: &str) -> Option<StoredPhone> {
    let vault = crate::allternit_vault::AllternitVault::new(state.db.clone());
    let account = account.to_string();
    let provider = format!("{VAULT_PROVIDER_PREFIX}{phone_id}");
    let raw = tokio::task::spawn_blocking(move || vault.get(&account, &provider, None, None))
        .await
        .ok()?
        .ok()??;
    let value: Value = serde_json::from_str(&raw).ok()?;
    Some(StoredPhone {
        key: EnvelopeKey::from_hex(value["key"].as_str()?)?,
        name: value["name"].as_str().map(str::to_string),
        model: value["model"].as_str().map(str::to_string),
    })
}

// ── Router ──────────────────────────────────────────────────────────────────

/// Merged under `/api/v1` on the protected router (next to
/// `phone_ws::phone_api_router`).
pub fn phone_app_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/phones/app/pair", get(pair_ws))
        .route("/phones/app/ingest", get(ingest_ws))
        .route("/phones/app/control", get(control_ws))
        .route("/phones/app/pair/pending", get(list_pending_pairs))
        .route("/phones/app/pair/:phone_id/decision", post(pair_decision))
        .route("/phones/app/:phone_id/rpc", post(rpc))
}

#[derive(Debug, Deserialize)]
pub struct PhoneQuery {
    phone: String,
}

/// Canonical lowercase hyphenated phone id (what `app_phones` stores).
fn canonical_phone_id(raw: &str) -> Option<String> {
    uuid::Uuid::parse_str(raw.trim())
        .ok()
        .map(|id| id.hyphenated().to_string())
}

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

/// The Desktop's own calls: its access token, and not forwarded by the relay.
fn is_local_desktop(state: &AppState, headers: &HeaderMap) -> bool {
    headers.get(crate::cowork_devices_routes::RELAYED_HEADER).is_none()
        && crate::auth::verify_desktop_access_token(headers, &state.config)
}

fn clean_label(value: Option<&str>) -> Option<String> {
    value
        .map(|v| v.trim().chars().filter(|c| !c.is_control()).take(120).collect::<String>())
        .filter(|v| !v.is_empty())
}

async fn send_end<T: FrameTransport>(transport: &mut T, reason: &str, code: u16) {
    if transport
        .send(Message::Text(json!({ "type": "end", "reason": reason }).to_string()))
        .await
    {
        let _ = transport
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.to_string().into(),
            })))
            .await;
    }
}

// ── Pair ────────────────────────────────────────────────────────────────────

async fn pair_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<PhoneQuery>,
) -> Response {
    let Some(phone_id) = canonical_phone_id(&query.phone) else {
        return err(StatusCode::BAD_REQUEST, "bad_phone_id", "phone must be a UUID");
    };
    ws.on_upgrade(move |socket| run_pair(WsFrameTransport { socket }, state, user, phone_id))
}

#[derive(Deserialize)]
struct PairHello {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, rename = "phoneId")]
    phone_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(rename = "pub")]
    public: String,
}

fn parse_pair_hello(text: &str, phone_id: &str) -> Option<(PairHello, [u8; 32])> {
    if text.len() > 4096 {
        return None;
    }
    let hello: PairHello = serde_json::from_str(text).ok()?;
    if hello.kind != "hello" {
        return None;
    }
    if let Some(id) = &hello.phone_id {
        if canonical_phone_id(id).as_deref() != Some(phone_id) {
            return None;
        }
    }
    let public: [u8; 32] = STANDARD.decode(hello.public.trim()).ok()?.try_into().ok()?;
    Some((hello, public))
}

async fn run_pair<T: FrameTransport>(mut transport: T, state: Arc<AppState>, user: AuthUser, phone_id: String) {
    let account = user.user_id.clone();
    let parsed = match tokio::time::timeout(HELLO_TIMEOUT, transport.recv()).await {
        Ok(Some(Message::Text(text))) => parse_pair_hello(&text, &phone_id),
        _ => None,
    };
    let Some((hello, app_pub)) = parsed else {
        send_end(&mut transport, "invalid_hello", axum::extract::ws::close_code::ERROR).await;
        return;
    };
    let name = clean_label(hello.name.as_deref());
    let model = clean_label(hello.model.as_deref());
    let node = NodeKeyPair::generate();
    let node_pub = STANDARD.encode(node.public);
    let code = pairing_code(&app_pub, &node.public);
    if !transport
        .send(Message::Text(json!({ "type": "pending", "pub": node_pub }).to_string()))
        .await
    {
        return;
    }

    let key = (account.clone(), phone_id.clone());
    let pair_id = hub().next_id.fetch_add(1, Ordering::Relaxed);
    let expires_at = chrono::Utc::now()
        + chrono::Duration::seconds(PAIR_DECISION_TIMEOUT.as_secs() as i64);
    let request = json!({
        // The Desktop feed reader keys on `type` in the data (it ignores
        // the SSE `event:` line).
        "type": EVENT_PAIR_REQUEST,
        "phoneId": phone_id,
        "serial": format!("{APP_SERIAL_PREFIX}{phone_id}"),
        "name": name,
        "model": model,
        "code": code,
        "expiresAt": expires_at.to_rfc3339(),
    });
    let (decide, decision) = oneshot::channel();
    // A newer attempt for the same phone replaces the old one (its sender is
    // dropped, so the old socket answers `replaced`).
    hub().pairs.lock().await.insert(
        key.clone(),
        PendingPair { id: pair_id, request: request.clone(), decide },
    );
    emit(&account, EVENT_PAIR_REQUEST, request);
    info!(phone = %phone_id, "app phone pairing requested");

    let outcome: Result<bool, &'static str> = {
        let mut decision = decision;
        let timer = tokio::time::sleep(PAIR_DECISION_TIMEOUT);
        tokio::pin!(timer);
        loop {
            tokio::select! {
                _ = &mut timer => break Err("timeout"),
                answer = &mut decision => break answer.map_err(|_| "replaced"),
                msg = transport.recv() => match msg {
                    None | Some(Message::Close(_)) => break Err("abandoned"),
                    Some(Message::Ping(p)) => { let _ = transport.send(Message::Pong(p)).await; }
                    Some(_) => {}
                },
            }
        }
    };
    {
        let mut pairs = hub().pairs.lock().await;
        if pairs.get(&key).is_some_and(|p| p.id == pair_id) {
            pairs.remove(&key);
        }
    }

    let reason = match outcome {
        Ok(true) => match node.derive_session_key(&app_pub, &phone_id) {
            Some(session_key) => {
                if store_phone(&state, &user, &phone_id, &session_key, &name, &model).await {
                    let phone = PhoneHelloPhone {
                        serial: format!("{APP_SERIAL_PREFIX}{phone_id}"),
                        model: model.clone(),
                        name: name.clone(),
                        call_bridge: None,
                    };
                    state.phone_link.cloud_upsert_phones(&[phone]).await;
                    let _ = transport
                        .send(Message::Text(json!({ "type": "approved", "pub": node_pub }).to_string()))
                        .await;
                    emit(&account, EVENT_PAIR_RESOLVED, json!({ "type": EVENT_PAIR_RESOLVED, "phoneId": phone_id, "approved": true }));
                    info!(phone = %phone_id, "app phone paired");
                    let _ = transport.send(Message::Close(None)).await;
                    return;
                }
                "store_failed"
            }
            None => "bad_key",
        },
        Ok(false) => "denied",
        Err(reason) => reason,
    };
    emit(&account, EVENT_PAIR_RESOLVED, json!({ "type": EVENT_PAIR_RESOLVED, "phoneId": phone_id, "approved": false, "reason": reason }));
    if reason != "abandoned" {
        let _ = transport
            .send(Message::Text(json!({ "type": "denied", "reason": reason }).to_string()))
            .await;
        let _ = transport.send(Message::Close(None)).await;
    }
}

#[derive(Deserialize)]
struct PairDecision {
    approve: bool,
}

async fn pair_decision(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(phone_id): Path<String>,
    Json(body): Json<PairDecision>,
) -> Response {
    if !is_local_desktop(&state, &headers) {
        return err(StatusCode::FORBIDDEN, "desktop_only", "Approve pairing on the Desktop");
    }
    let Some(phone_id) = canonical_phone_id(&phone_id) else {
        return err(StatusCode::BAD_REQUEST, "bad_phone_id", "phoneId must be a UUID");
    };
    let pending = hub().pairs.lock().await.remove(&(user.user_id.clone(), phone_id));
    match pending {
        Some(pair) => {
            let _ = pair.decide.send(body.approve);
            Json(json!({ "ok": true })).into_response()
        }
        None => err(StatusCode::NOT_FOUND, "no_pending_pair", "No pairing request is waiting for this phone"),
    }
}

async fn list_pending_pairs(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    if !is_local_desktop(&state, &headers) {
        return err(StatusCode::FORBIDDEN, "desktop_only", "Desktop only");
    }
    let pairs: Vec<Value> = hub()
        .pairs
        .lock()
        .await
        .iter()
        .filter(|((account, _), _)| account == &user.user_id)
        .map(|(_, pair)| pair.request.clone())
        .collect();
    Json(json!({ "pairs": pairs })).into_response()
}

// ── Ingest (broadcast extension) ────────────────────────────────────────────

async fn ingest_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<PhoneQuery>,
) -> Response {
    let Some(phone_id) = canonical_phone_id(&query.phone) else {
        return err(StatusCode::BAD_REQUEST, "bad_phone_id", "phone must be a UUID");
    };
    ws.on_upgrade(move |socket| {
        run_ingest(WsFrameTransport { socket }, state, user.user_id, phone_id)
    })
}

/// What the ingest tells the extension about a viewer change, given whether
/// the broadcast is paused and whether viewers remain.
#[derive(Debug, PartialEq, Eq)]
enum IngestHint {
    Resume,
    Keyframe,
    Pause,
}

fn ingest_hint(relay_event: &str, paused: bool, has_viewers: bool) -> Option<IngestHint> {
    match relay_event {
        "viewer_attached" if paused => Some(IngestHint::Resume),
        "viewer_attached" => Some(IngestHint::Keyframe),
        "viewer_detached" if !paused && !has_viewers => Some(IngestHint::Pause),
        _ => None,
    }
}

async fn run_ingest<T: FrameTransport>(mut transport: T, state: Arc<AppState>, account: String, phone_id: String) {
    let Some(stored) = load_phone(&state, &account, &phone_id).await else {
        send_end(&mut transport, "not_paired", NOT_PAIRED_CLOSE).await;
        return;
    };
    let serial = format!("{APP_SERIAL_PREFIX}{phone_id}");
    let phone = PhoneHelloPhone {
        serial: serial.clone(),
        model: stored.model.clone(),
        name: stored.name.clone(),
        call_bridge: None,
    };
    let registry = state.phone_link.clone();
    let (session, mut from_registry) =
        PhoneLinkSession::new(registry.next_session_id(), account.clone(), vec![phone.clone()]);
    registry.register_session(session.clone()).await;
    registry.cloud_upsert_phones(std::slice::from_ref(&phone)).await;
    info!(phone = %phone_id, "app phone broadcast attached");

    let replay = replay_guard(&(account.clone(), phone_id.clone())).await;
    // The extension starts paused; a viewer that was already waiting attaches
    // as soon as the session registers and its `viewer_attached` resumes it.
    let mut paused = true;
    let mut bad_packets: u32 = 0;
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut last_inbound = tokio::time::Instant::now();

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if !transport.send(Message::Ping(Vec::new())).await || last_inbound.elapsed() > HEARTBEAT_DEAD {
                    break;
                }
            }
            relay = from_registry.recv() => {
                let Some(msg) = relay else { break };
                // Registry presence frames become pause/resume/keyframe; viewer
                // input is dropped (no taps on native apps this round).
                let Message::Text(text) = msg else { continue };
                let Ok(event) = serde_json::from_str::<Value>(&text) else { continue };
                let has_viewers = session.has_viewers(&serial).await;
                let hint = ingest_hint(event["type"].as_str().unwrap_or_default(), paused, has_viewers);
                let frame = match hint {
                    Some(IngestHint::Resume) => { paused = false; "resume" }
                    Some(IngestHint::Keyframe) => "keyframe",
                    Some(IngestHint::Pause) => { paused = true; "pause" }
                    None => continue,
                };
                if !transport.send(Message::Text(json!({ "type": frame }).to_string())).await {
                    break;
                }
            }
            msg = transport.recv() => match msg {
                Some(Message::Binary(frame)) => {
                    last_inbound = tokio::time::Instant::now();
                    let opened = {
                        let mut guard = replay.lock().unwrap_or_else(|p| p.into_inner());
                        stored.key.open_bytes(&frame, now_secs(), &mut guard)
                    };
                    match opened {
                        Ok(packet) => {
                            bad_packets = 0;
                            fan_out_to_viewers(&session, Message::Binary(packet)).await;
                        }
                        Err(e) => {
                            bad_packets += 1;
                            if bad_packets == 1 {
                                debug!(phone = %phone_id, code = e.code(), "app phone packet refused");
                            }
                            if bad_packets >= MAX_BAD_PACKETS {
                                warn!(phone = %phone_id, code = e.code(), "app phone packets keep failing; closing ingest");
                                send_end(&mut transport, "bad_envelope", NOT_PAIRED_CLOSE).await;
                                break;
                            }
                        }
                    }
                }
                Some(Message::Text(_)) | Some(Message::Pong(_)) => {
                    last_inbound = tokio::time::Instant::now();
                }
                Some(Message::Ping(p)) => {
                    last_inbound = tokio::time::Instant::now();
                    if !transport.send(Message::Pong(p)).await { break; }
                }
                Some(Message::Close(_)) | None => break,
            }
        }
    }

    registry.unregister_session(&session).await;
    registry.cloud_upsert_phones(std::slice::from_ref(&phone)).await;
    info!(phone = %phone_id, "app phone broadcast detached");
}

// ── Control (foreground app) ────────────────────────────────────────────────

async fn control_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<PhoneQuery>,
) -> Response {
    let Some(phone_id) = canonical_phone_id(&query.phone) else {
        return err(StatusCode::BAD_REQUEST, "bad_phone_id", "phone must be a UUID");
    };
    ws.on_upgrade(move |socket| {
        run_control(WsFrameTransport { socket }, state, user.user_id, phone_id)
    })
}

async fn run_control<T: FrameTransport>(mut transport: T, state: Arc<AppState>, account: String, phone_id: String) {
    let Some(stored) = load_phone(&state, &account, &phone_id).await else {
        send_end(&mut transport, "not_paired", NOT_PAIRED_CLOSE).await;
        return;
    };
    let key = (account.clone(), phone_id.clone());
    let (to_app, mut outbound) = mpsc::channel::<String>(32);
    let conn = Arc::new(ControlConn {
        id: hub().next_id.fetch_add(1, Ordering::Relaxed),
        to_app,
        pending: Mutex::new(HashMap::new()),
        state: RwLock::new(json!({ "foreground": true, "browserOpen": false })),
    });
    hub().controls.write().await.insert(key.clone(), conn.clone());
    let replay = replay_guard(&key).await;
    info!(phone = %phone_id, "app phone control attached");

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut last_inbound = tokio::time::Instant::now();

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if !transport.send(Message::Ping(Vec::new())).await || last_inbound.elapsed() > HEARTBEAT_DEAD {
                    break;
                }
            }
            request = outbound.recv() => {
                let Some(plain) = request else { break };
                let sealed = {
                    let mut guard = replay.lock().unwrap_or_else(|p| p.into_inner());
                    stored.key.seal(&plain, now_secs(), &mut guard)
                };
                if !transport.send(Message::Text(sealed)).await {
                    break;
                }
            }
            msg = transport.recv() => match msg {
                Some(Message::Text(text)) => {
                    last_inbound = tokio::time::Instant::now();
                    let opened = {
                        let mut guard = replay.lock().unwrap_or_else(|p| p.into_inner());
                        stored.key.open_text(&text, now_secs(), &mut guard)
                    };
                    let plain = match opened {
                        Ok(plain) => plain,
                        Err(e) => {
                            debug!(phone = %phone_id, code = e.code(), "app phone control frame refused");
                            continue;
                        }
                    };
                    let Ok(value) = serde_json::from_str::<Value>(&plain) else { continue };
                    if value["type"].as_str() == Some("state") {
                        *conn.state.write().await = json!({
                            "foreground": value["foreground"].as_bool().unwrap_or(false),
                            "browserOpen": value["browserOpen"].as_bool().unwrap_or(false),
                        });
                    } else if let Some(id) = value["id"].as_str() {
                        if let Some(waiter) = conn.pending.lock().await.remove(id) {
                            let _ = waiter.send(value);
                        }
                    }
                }
                Some(Message::Binary(_)) | Some(Message::Pong(_)) => {
                    last_inbound = tokio::time::Instant::now();
                }
                Some(Message::Ping(p)) => {
                    last_inbound = tokio::time::Instant::now();
                    if !transport.send(Message::Pong(p)).await { break; }
                }
                Some(Message::Close(_)) | None => break,
            }
        }
    }

    {
        let mut controls = hub().controls.write().await;
        if controls.get(&key).is_some_and(|c| c.id == conn.id) {
            controls.remove(&key);
        }
    }
    // Dropping the waiters answers their rpc calls with app_not_open.
    conn.pending.lock().await.clear();
    info!(phone = %phone_id, "app phone control detached");
}

#[derive(Deserialize)]
struct RpcRequest {
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

fn validate_rpc(method: &str, params: &Value) -> Result<(), &'static str> {
    if !RPC_METHODS.contains(&method) {
        return Err("unknown method");
    }
    if method == "web.wait" && params["ms"].as_u64().map_or(true, |ms| ms > MAX_WEB_WAIT_MS) {
        return Err("web.wait needs ms between 0 and 10000");
    }
    Ok(())
}

async fn rpc(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(phone_id): Path<String>,
    Json(body): Json<RpcRequest>,
) -> Response {
    if !is_local_desktop(&state, &headers) {
        return err(StatusCode::FORBIDDEN, "desktop_only", "Desktop only");
    }
    let Some(phone_id) = canonical_phone_id(&phone_id) else {
        return err(StatusCode::BAD_REQUEST, "bad_phone_id", "phoneId must be a UUID");
    };
    let params = body.params.unwrap_or_else(|| json!({}));
    if let Err(message) = validate_rpc(&body.method, &params) {
        return err(StatusCode::BAD_REQUEST, "bad_request", message);
    }
    let conn = hub()
        .controls
        .read()
        .await
        .get(&(user.user_id.clone(), phone_id.clone()))
        .cloned();
    let not_open = || err(StatusCode::CONFLICT, "app_not_open", "Open the Allternit app on the iPhone");
    let Some(conn) = conn else { return not_open() };

    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    conn.pending.lock().await.insert(id.clone(), tx);
    let request = json!({ "id": id, "method": body.method, "params": params }).to_string();
    if tokio::time::timeout(Duration::from_secs(5), conn.to_app.send(request))
        .await
        .map_or(true, |sent| sent.is_err())
    {
        conn.pending.lock().await.remove(&id);
        return not_open();
    }
    match tokio::time::timeout(RPC_TIMEOUT, rx).await {
        Ok(Ok(answer)) => match answer.get("error").filter(|e| !e.is_null()) {
            Some(error) => Json(json!({ "error": error })).into_response(),
            None => Json(json!({ "result": answer.get("result").cloned().unwrap_or(Value::Null) })).into_response(),
        },
        Ok(Err(_)) => not_open(),
        Err(_) => {
            conn.pending.lock().await.remove(&id);
            err(StatusCode::GATEWAY_TIMEOUT, "app_timeout", "The iPhone app did not answer in time")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_changes_map_to_extension_hints() {
        // First viewer resumes; a second only asks for a keyframe.
        assert_eq!(ingest_hint("viewer_attached", true, true), Some(IngestHint::Resume));
        assert_eq!(ingest_hint("viewer_attached", false, true), Some(IngestHint::Keyframe));
        // Pause only once the last viewer is gone.
        assert_eq!(ingest_hint("viewer_detached", false, true), None);
        assert_eq!(ingest_hint("viewer_detached", false, false), Some(IngestHint::Pause));
        assert_eq!(ingest_hint("viewer_detached", true, false), None);
        assert_eq!(ingest_hint("end", false, false), None);
    }

    #[test]
    fn pair_hello_must_name_this_phone_and_carry_a_32_byte_key() {
        let id = "8c0f6b4e-1d2a-4f3b-9c5d-6e7f8a9b0c1d";
        let pubkey = STANDARD.encode([9u8; 32]);
        let ok = json!({ "type": "hello", "phoneId": id.to_uppercase(), "name": "Eoj's iPhone", "pub": pubkey }).to_string();
        assert!(parse_pair_hello(&ok, id).is_some());
        let other = json!({ "type": "hello", "phoneId": uuid::Uuid::new_v4().to_string(), "pub": pubkey }).to_string();
        assert!(parse_pair_hello(&other, id).is_none());
        let short = json!({ "type": "hello", "pub": STANDARD.encode([9u8; 31]) }).to_string();
        assert!(parse_pair_hello(&short, id).is_none());
        assert!(parse_pair_hello(&json!({ "type": "hi", "pub": pubkey }).to_string(), id).is_none());
    }

    #[test]
    fn rpc_methods_are_the_contract_list_with_a_wait_cap() {
        assert!(validate_rpc("web.snapshot", &json!({})).is_ok());
        assert!(validate_rpc("web.wait", &json!({ "ms": 10_000 })).is_ok());
        assert!(validate_rpc("web.wait", &json!({ "ms": 10_001 })).is_err());
        assert!(validate_rpc("web.wait", &json!({})).is_err());
        assert!(validate_rpc("web.eval", &json!({ "js": "1" })).is_err());
    }

    /// `/phones/app:<id>/ws-token` (the viewer route) must still reach
    /// `phone_ws`, not 404 on the new `/phones/app/...` static prefix.
    #[tokio::test]
    async fn app_serial_ws_token_still_routes_next_to_app_routes() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let dir = std::env::temp_dir().join(format!("allternit-app-phone-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let app = crate::phone_ws::phone_api_router().merge(phone_app_router()).with_state(state);
        for uri in [
            "/phones/app:8c0f6b4e-1d2a-4f3b-9c5d-6e7f8a9b0c1d/ws-token",
            "/phones/app%3A8c0f6b4e-1d2a-4f3b-9c5d-6e7f8a9b0c1d/ws-token",
        ] {
            let resp = app
                .clone()
                .oneshot(Request::post(uri).header("content-type", "application/json").body(Body::from("{}")).unwrap())
                .await
                .unwrap();
            // No AuthUser extension here, so the handler is reached and fails
            // its extractor (500), which is not the router's 404/405.
            assert_ne!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_ne!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "{uri}");
        }
    }
}
