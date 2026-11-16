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
//!   1. app → `{"type":"hello","phoneId","name","model","pub":"<X25519 b64>","platform"?}`
//!      (`platform`: `"ios"` (default, so older iPhone apps keep working) or
//!      `"android"`; anything else is refused as `invalid_hello`).
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
//!   Android adds to its `state` frame `canTap` (Accessibility on) and
//!   `consent` (`"granted"` | `"needed"` | `"denied"`: screen-share consent).
//!   Android only, node → app:
//!   - `{"id","method":"input.events","params":{"events":[PhoneInputEvent…]}}`:
//!     taps and keys from a FULL-control viewer (a `readOnly:false` ws-token,
//!     which needs the ACI confirmation grant). Fire and forget: the node does
//!     not wait for the answer. Read-only viewers never reach this (their
//!     frames are dropped at the relay pipe).
//!   - `{"type":"screen_request","serial"}`: a viewer is waiting and no ingest
//!     is open; the app asks the person to start sharing (at most one per
//!     phone every 10 s).
//!
//! # Listing (`GET /api/v1/phones`, see [`decorate_listing`])
//!
//! App phones carry `platform` (`ios` | `android`), `route`
//! (`iphone-app-store` | `android-app`), `screen` (true: can share its
//! screen), `canTap` (Android with Accessibility on and the app connected),
//! `consent` (Android: last reported screen-share consent, `unknown` while the
//! app is not connected; iOS: null), `appOpen`, `foreground`, `browserOpen`.
//!
//! An unpaired phone (no stored key) gets `{"type":"end","reason":"not_paired"}`
//! and close 4401 on ingest/control.
//!
//! # Desktop routes (loopback only: desktop access token, not relayed)
//!
//! * `POST /api/v1/phones/app/:phoneId/rpc` `{method, params}` → 200
//!   `{"result"}` or 200 `{"error":{"code","message"}}` (the app's error), 409
//!   `{"error":"app_not_open"}`, 504 `{"error":"app_timeout"}`.
//!   `device.calendar.*`, `device.reminders.*`, `device.contacts.search` (Device
//!   Access) are refused here with `{"error":{"code":"device_access_off"}}` unless
//!   the app's last `state` frame (`deviceAccess:{calendar,reminders,contacts}`)
//!   has that type on; the app checks again and answers `permission_denied` when
//!   iOS access is denied. The call key is `(account, phone)`: another account's
//!   phone is never reachable. `input.events` is refused here with 403
//!   `viewer_only`: bots drive phones through the guard/hold path, and only a
//!   full-control viewer's own taps become `input.events`.
//!
//! # Pairing routes (the Desktop, or the signed-in owner relayed by cloud-api)
//!
//! Accepted from the local Desktop (its access token, not relayed) or from a
//! request cloud-api signed with this runtime's relay key (`relay_auth`) whose
//! `x-allternit-owner` is this runtime's paired owner AND the caller's account
//! (cloud-api `GET /api/v1/app-phones/pair/pending`,
//! `POST /api/v1/app-phones/:phoneId/pair/decision`, which check that the
//! Clerk user owns the runtime). Anything else is 403 `owner_only`.
//!
//! * `POST /api/v1/phones/app/pair/:phoneId/decision` `{approve}` → `{"ok":true}`
//!   or 404 `{"error":"no_pending_pair"}`.
//! * `GET /api/v1/phones/app/pair/pending` → `{"pairs":[…pair_request payloads]}`
//!   (a Desktop that reconnects its event stream catches up here). Each
//!   payload carries `platform`.
//!
//! Keys and frame bytes are never logged.

use axum::extract::ws::{CloseFrame, Message, WebSocketUpgrade};
use axum::extract::{Extension, OriginalUri, Path, Query, State};
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
/// Listing route of an Android phone on the app-phone lane (Phone Link inside
/// the Allternit Android app: MediaProjection screen + Accessibility taps).
pub const ANDROID_APP_ROUTE: &str = "android-app";
pub const EVENT_PAIR_REQUEST: &str = "phone_app.pair_request";
pub const EVENT_PAIR_RESOLVED: &str = "phone_app.pair_resolved";
const VAULT_PROVIDER_PREFIX: &str = "app_phone:";
/// Vault rows need an agent or a session id (V37 CHECK). An app phone
/// belongs to the account, so its key is filed under this fixed session id.
const VAULT_SESSION: &str = "app-phone";
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
/// At most one `screen_request` per phone in this window.
const SCREEN_REQUEST_INTERVAL: Duration = Duration::from_secs(10);
/// Viewer input batch limits (`input.events`).
const MAX_INPUT_EVENTS: usize = 64;
const MAX_INPUT_TEXT: usize = 2000;
const MAX_INPUT_FRAME: usize = 64 * 1024;
const INPUT_EVENT_TYPES: &[&str] = &["touch", "key", "text", "scroll", "back", "home", "app_switch", "power"];
/// Control methods only a full-control viewer's own input may produce; never
/// accepted on `POST /phones/app/:id/rpc` (the route bots use).
const VIEWER_ONLY_METHODS: &[&str] = &["input.events"];
const RPC_METHODS: &[&str] = &[
    "web.open",
    "web.snapshot",
    "web.click",
    "web.type",
    "web.scroll",
    "web.back",
    "web.wait",
    "screen.prompt",
    // Device Access: calendar, reminders, contacts. Answered by the app only
    // while it is open and only for types the person switched on.
    "device.calendar.list",
    "device.calendar.create",
    "device.reminders.list",
    "device.reminders.create",
    "device.contacts.search",
];

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Which app holds the phone's sockets. Absent in a hello or a stored row
/// (every iPhone paired before Android existed) means iOS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppPlatform {
    #[default]
    Ios,
    Android,
}

impl AppPlatform {
    /// `None` for a value that is neither (refused in the hello).
    fn parse(raw: Option<&str>) -> Option<Self> {
        match raw.map(|v| v.trim().to_ascii_lowercase()) {
            None => Some(Self::Ios),
            Some(v) if v.is_empty() || v == "ios" => Some(Self::Ios),
            Some(v) if v == "android" => Some(Self::Android),
            Some(_) => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }

    pub fn route(self) -> &'static str {
        match self {
            Self::Ios => APP_ROUTE,
            Self::Android => ANDROID_APP_ROUTE,
        }
    }
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
    platform: AppPlatform,
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
    /// Platform of each phone seen since start (pair, ingest, control), so
    /// the listing does not read the vault for phones it already knows.
    platforms: RwLock<HashMap<PhoneKey, AppPlatform>>,
    /// Last `screen_request` per phone (throttle).
    screen_requests: Mutex<HashMap<PhoneKey, tokio::time::Instant>>,
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        pairs: Mutex::new(HashMap::new()),
        controls: RwLock::new(HashMap::new()),
        replay: Mutex::new(HashMap::new()),
        events: broadcast::channel(64).0,
        next_id: AtomicU64::new(1),
        platforms: RwLock::new(HashMap::new()),
        screen_requests: Mutex::new(HashMap::new()),
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

async fn remember_platform(key: &PhoneKey, platform: AppPlatform) {
    hub().platforms.write().await.insert(key.clone(), platform);
}

/// The phone's platform: the open control socket, else what this instance
/// saw, else the stored pairing (vault), else iOS.
async fn phone_platform(state: &AppState, account: &str, phone_id: &str) -> AppPlatform {
    let key = (account.to_string(), phone_id.to_string());
    if let Some(conn) = hub().controls.read().await.get(&key) {
        return conn.platform;
    }
    if let Some(platform) = hub().platforms.read().await.get(&key).copied() {
        return platform;
    }
    match load_phone(state, account, phone_id).await {
        Some(stored) => {
            remember_platform(&key, stored.platform).await;
            stored.platform
        }
        None => AppPlatform::Ios,
    }
}

/// The listing fields for one app phone, from its platform and the control
/// socket's last `state` frame (`None` = the app is not connected).
fn listing_fields(platform: AppPlatform, control_state: Option<&Value>) -> Value {
    let state = control_state.cloned().unwrap_or_else(|| json!({}));
    let android = platform == AppPlatform::Android;
    let consent = if !android {
        Value::Null
    } else if control_state.is_none() {
        json!("unknown")
    } else {
        json!(state["consent"].as_str().filter(|c| matches!(*c, "granted" | "needed" | "denied")).unwrap_or("needed"))
    };
    json!({
        "platform": platform.as_str(),
        "route": platform.route(),
        "screen": true,
        "canTap": android && control_state.is_some() && state["canTap"].as_bool() == Some(true),
        "consent": consent,
        "appOpen": control_state.is_some(),
        "foreground": state.get("foreground").cloned().unwrap_or(json!(false)),
        "browserOpen": state.get("browserOpen").cloned().unwrap_or(json!(false)),
    })
}

/// Add the app-phone fields to a `GET /api/v1/phones` row.
pub async fn decorate_listing(state: &AppState, account: &str, phone_id: &str, phone: &mut Value) {
    let platform = phone_platform(state, account, phone_id).await;
    let control = hub()
        .controls
        .read()
        .await
        .get(&(account.to_string(), phone_id.to_string()))
        .cloned();
    let control_state = match &control {
        Some(conn) => Some(conn.state.read().await.clone()),
        None => None,
    };
    let fields = listing_fields(platform, control_state.as_ref());
    if let (Some(obj), Some(fields)) = (phone.as_object_mut(), fields.as_object()) {
        for (k, v) in fields {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// True when a viewer may wait for this phone's screen although no ingest is
/// open: an Android app is connected and can be asked to share
/// ([`request_screen`]). iPhones cannot start a broadcast from the network.
pub async fn can_request_screen(account: &str, phone_id: &str) -> bool {
    let Some(phone_id) = canonical_phone_id(phone_id) else { return false };
    hub()
        .controls
        .read()
        .await
        .get(&(account.to_string(), phone_id))
        .is_some_and(|conn| conn.platform == AppPlatform::Android)
}

/// A viewer is waiting for this phone and no ingest is open: ask the Android
/// app to start sharing (`{"type":"screen_request"}` on its control socket).
/// Throttled per phone; a no-op for iPhones and closed apps. True when sent.
pub async fn request_screen(account: &str, phone_id: &str) -> bool {
    let Some(phone_id) = canonical_phone_id(phone_id) else { return false };
    let key = (account.to_string(), phone_id.clone());
    let Some(conn) = hub().controls.read().await.get(&key).cloned() else { return false };
    if conn.platform != AppPlatform::Android {
        return false;
    }
    {
        let mut last = hub().screen_requests.lock().await;
        let now = tokio::time::Instant::now();
        if last.get(&key).is_some_and(|at| now.duration_since(*at) < SCREEN_REQUEST_INTERVAL) {
            return false;
        }
        last.insert(key, now);
    }
    let frame = json!({ "type": "screen_request", "serial": format!("{APP_SERIAL_PREFIX}{phone_id}") }).to_string();
    let sent = conn.to_app.try_send(frame).is_ok();
    if sent {
        info!(phone = %phone_id, "app phone screen requested");
    }
    sent
}

/// The `{"type":"input","events":[…]}` frame a full-control viewer sent, as
/// the `input.events` request for the phone, or `None` when it is not a
/// well-formed input batch (dropped).
fn input_request(frame: &Value) -> Option<String> {
    if frame["type"].as_str() != Some("input") {
        return None;
    }
    let events = frame["events"].as_array()?;
    if events.is_empty() || events.len() > MAX_INPUT_EVENTS {
        return None;
    }
    for event in events {
        let kind = event["type"].as_str()?;
        if !INPUT_EVENT_TYPES.contains(&kind) {
            return None;
        }
        if kind == "text" && !event["text"].as_str().is_some_and(|t| t.len() <= MAX_INPUT_TEXT) {
            return None;
        }
    }
    Some(
        json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "input.events",
            "params": { "events": events },
        })
        .to_string(),
    )
}

/// Hand a full-control viewer's input to the Android app's control socket.
/// Never blocks the relay: a closed app or a full pipe drops the batch.
async fn forward_viewer_input(key: &PhoneKey, request: String) -> bool {
    let Some(conn) = hub().controls.read().await.get(key).cloned() else { return false };
    conn.platform == AppPlatform::Android && conn.to_app.try_send(request).is_ok()
}

// ── Stored session key (runtime vault) ──────────────────────────────────────

struct StoredPhone {
    key: EnvelopeKey,
    name: Option<String>,
    model: Option<String>,
    platform: AppPlatform,
}

async fn store_phone(
    state: &AppState,
    user: &AuthUser,
    phone_id: &str,
    key: &EnvelopeKey,
    name: &Option<String>,
    model: &Option<String>,
    platform: AppPlatform,
) -> bool {
    let vault = crate::allternit_vault::AllternitVault::new(state.db.clone());
    let user = user.clone();
    let provider = format!("{VAULT_PROVIDER_PREFIX}{phone_id}");
    let value = json!({ "key": key.to_hex(), "name": name, "model": model, "platform": platform.as_str() }).to_string();
    match tokio::task::spawn_blocking(move || vault.put(&user, &provider, None, Some(VAULT_SESSION), &value, None)).await {
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
    let raw = tokio::task::spawn_blocking(move || vault.get(&account, &provider, None, Some(VAULT_SESSION)))
        .await
        .ok()?
        .ok()??;
    let value: Value = serde_json::from_str(&raw).ok()?;
    Some(StoredPhone {
        key: EnvelopeKey::from_hex(value["key"].as_str()?)?,
        name: value["name"].as_str().map(str::to_string),
        model: value["model"].as_str().map(str::to_string),
        // Rows stored before Android have no platform: iOS.
        platform: AppPlatform::parse(value["platform"].as_str()).unwrap_or_default(),
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
    #[serde(default)]
    platform: Option<String>,
}

fn parse_pair_hello(text: &str, phone_id: &str) -> Option<(PairHello, AppPlatform, [u8; 32])> {
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
    let platform = AppPlatform::parse(hello.platform.as_deref())?;
    let public: [u8; 32] = STANDARD.decode(hello.public.trim()).ok()?.try_into().ok()?;
    Some((hello, platform, public))
}

async fn run_pair<T: FrameTransport>(mut transport: T, state: Arc<AppState>, user: AuthUser, phone_id: String) {
    let account = user.user_id.clone();
    let parsed = match tokio::time::timeout(HELLO_TIMEOUT, transport.recv()).await {
        Ok(Some(Message::Text(text))) => parse_pair_hello(&text, &phone_id),
        _ => None,
    };
    let Some((hello, platform, app_pub)) = parsed else {
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
        "platform": platform.as_str(),
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
                if store_phone(&state, &user, &phone_id, &session_key, &name, &model, platform).await {
                    remember_platform(&key, platform).await;
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

type RelaySecretExt = Option<Extension<Arc<dyn crate::relay_auth::RelaySecret>>>;

/// Who may see and answer pairing requests: the local Desktop, or the
/// signed-in owner relayed by cloud-api. The relayed request must carry
/// cloud-api's signature over this exact method, path and body (`relay_auth`,
/// keyed with this runtime's device token), and the signed owner must be both
/// the runtime's paired owner (checked by `verify_relay`) and the caller's
/// account, so one account can never decide another's phones.
fn pairing_caller_allowed(
    state: &AppState,
    secret: &RelaySecretExt,
    user: &AuthUser,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
) -> bool {
    if is_local_desktop(state, headers) {
        return true;
    }
    let secret = match secret {
        Some(Extension(secret)) => secret.clone(),
        None => crate::relay_auth::process_secret(),
    };
    match crate::relay_auth::verify_relay(secret.as_ref(), headers, method, path, body, crate::relay_auth::unix_now()) {
        Ok(owner) => owner == user.user_id,
        Err(_) => false,
    }
}

fn owner_only() -> Response {
    err(
        StatusCode::FORBIDDEN,
        "owner_only",
        "Approve pairing on the Desktop or while signed in as this computer's owner",
    )
}

async fn pair_decision(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    secret: RelaySecretExt,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(phone_id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if !pairing_caller_allowed(&state, &secret, &user, &headers, "POST", uri.path(), &body) {
        return owner_only();
    }
    let Ok(body) = serde_json::from_slice::<PairDecision>(&body) else {
        return err(StatusCode::BAD_REQUEST, "bad_request", "body must be {\"approve\": bool}");
    };
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
    secret: RelaySecretExt,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if !pairing_caller_allowed(&state, &secret, &user, &headers, "GET", uri.path(), b"") {
        return owner_only();
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

    let phone_key = (account.clone(), phone_id.clone());
    remember_platform(&phone_key, stored.platform).await;
    let replay = replay_guard(&phone_key).await;
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
                // Registry presence frames become pause/resume/keyframe. Viewer
                // input reaches here only from full-control viewers (read-only
                // frames are dropped at the relay pipe); on Android it becomes
                // `input.events` on the control socket, on iOS it is dropped.
                let Message::Text(text) = msg else { continue };
                if text.len() > MAX_INPUT_FRAME { continue; }
                let Ok(event) = serde_json::from_str::<Value>(&text) else { continue };
                if stored.platform == AppPlatform::Android {
                    if let Some(request) = input_request(&event) {
                        if !forward_viewer_input(&phone_key, request).await {
                            debug!(phone = %phone_id, "viewer input dropped: app not connected");
                        }
                        continue;
                    }
                }
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
        platform: stored.platform,
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
                            // Android: Accessibility on, screen-share consent.
                            "canTap": value["canTap"].as_bool().unwrap_or(false),
                            "consent": value["consent"].as_str().filter(|c| matches!(*c, "granted" | "needed" | "denied")),
                            "deviceAccess": {
                                "calendar": value["deviceAccess"]["calendar"].as_bool().unwrap_or(false),
                                "reminders": value["deviceAccess"]["reminders"].as_bool().unwrap_or(false),
                                "contacts": value["deviceAccess"]["contacts"].as_bool().unwrap_or(false),
                            },
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
    if method.starts_with("device.") && !params.is_object() {
        return Err("params must be an object");
    }
    if method == "device.contacts.search"
        && !params["query"].as_str().is_some_and(|q| !q.trim().is_empty() && q.len() <= 400)
    {
        return Err("device.contacts.search needs a query");
    }
    Ok(())
}

/// Device Access gate on the server path: the app reports which types the
/// person switched on (and iOS allows) in its `state` frame; a device.* call
/// for a type that is not explicitly on never goes to the phone.
fn device_gate(state: &Value, method: &str) -> Result<(), &'static str> {
    let Some(rest) = method.strip_prefix("device.") else { return Ok(()) };
    let kind = rest.split('.').next().unwrap_or("");
    if state["deviceAccess"][kind].as_bool() == Some(true) {
        Ok(())
    } else {
        Err("device_access_off")
    }
}

async fn rpc(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(phone_id): Path<String>,
    Json(body): Json<RpcRequest>,
) -> Response {
    // Refused for every caller: bot input goes through the phone guard/hold
    // path, and `input.events` only comes from a full-control viewer.
    if VIEWER_ONLY_METHODS.contains(&body.method.as_str()) {
        return err(
            StatusCode::FORBIDDEN,
            "viewer_only",
            "Phone input from bots goes through the phone guard; input.events is for the owner's live view",
        );
    }
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
    if let Err(code) = device_gate(&*conn.state.read().await, &body.method) {
        return Json(json!({ "error": {
            "code": code,
            "message": "The person has not turned this on in the Allternit app (Settings > Device Access)."
        } }))
        .into_response();
    }

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

    // Regression: the vault CHECK needs an agent or a session id, so pairing
    // used to fail at "app phone key not stored" and the iPhone showed
    // "The Desktop declined this iPhone".
    #[tokio::test]
    async fn a_paired_phone_key_is_stored_and_read_back() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        state
            .db
            .connect()
            .unwrap()
            .execute("INSERT INTO users (id, email) VALUES ('user-p', 'user-p@test.local')", [])
            .unwrap();
        let user = AuthUser {
            user_id: "user-p".to_string(),
            email: Some("user-p@test.local".to_string()),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        };
        let phone = uuid::Uuid::new_v4().to_string();
        let key = EnvelopeKey::from_hex(&"ab".repeat(32)).unwrap();
        let name = Some("iPhone".to_string());
        let model = Some("iPhone15,4".to_string());
        assert!(store_phone(&state, &user, &phone, &key, &name, &model, AppPlatform::Ios).await);
        let stored = load_phone(&state, "user-p", &phone).await.expect("stored phone");
        assert_eq!(stored.key.to_hex(), key.to_hex());
        assert_eq!(stored.model.as_deref(), Some("iPhone15,4"));
        assert_eq!(stored.platform, AppPlatform::Ios);
        let android = uuid::Uuid::new_v4().to_string();
        assert!(store_phone(&state, &user, &android, &key, &None, &None, AppPlatform::Android).await);
        assert_eq!(load_phone(&state, "user-p", &android).await.unwrap().platform, AppPlatform::Android);
    }

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
    fn device_calls_need_the_type_switched_on() {
        let on = json!({ "deviceAccess": { "calendar": true, "reminders": false, "contacts": false } });
        assert!(device_gate(&on, "device.calendar.list").is_ok());
        assert!(device_gate(&on, "device.calendar.create").is_ok());
        assert_eq!(device_gate(&on, "device.reminders.list"), Err("device_access_off"));
        assert_eq!(device_gate(&on, "device.contacts.search"), Err("device_access_off"));
        // An app that never reported Device Access (or an old one) is off, not on.
        assert_eq!(device_gate(&json!({ "foreground": true }), "device.calendar.list"), Err("device_access_off"));
        assert!(device_gate(&json!({}), "web.snapshot").is_ok());
        assert!(validate_rpc("device.contacts.search", &json!({ "query": "ann" })).is_ok());
        assert!(validate_rpc("device.contacts.search", &json!({ "query": " " })).is_err());
        assert!(validate_rpc("device.calendar.list", &json!([])).is_err());
        assert!(validate_rpc("device.health.read", &json!({})).is_err());
    }

    #[tokio::test]
    async fn a_control_socket_is_only_reachable_by_its_own_account() {
        let phone = uuid::Uuid::new_v4().to_string();
        let (to_app, _rx) = mpsc::channel::<String>(1);
        let conn = Arc::new(ControlConn {
            id: u64::MAX,
            platform: AppPlatform::Ios,
            to_app,
            pending: Mutex::new(HashMap::new()),
            state: RwLock::new(json!({})),
        });
        hub().controls.write().await.insert(("owner".to_string(), phone.clone()), conn);
        assert!(control_open("owner", &phone).await);
        assert!(!control_open("someone-else", &phone).await);
        hub().controls.write().await.remove(&("owner".to_string(), phone));
    }

    // ── Android lane ─────────────────────────────────────────────────────────

    struct ChanTransport {
        rx: mpsc::Receiver<Message>,
        tx: mpsc::Sender<Message>,
    }

    #[async_trait::async_trait]
    impl FrameTransport for ChanTransport {
        async fn recv(&mut self) -> Option<Message> {
            self.rx.recv().await
        }
        async fn send(&mut self, msg: Message) -> bool {
            self.tx.send(msg).await.is_ok()
        }
    }

    /// (our end: send into the pump, read what it sends) + the pump's transport.
    fn chan_pair() -> ((mpsc::Sender<Message>, mpsc::Receiver<Message>), ChanTransport) {
        let (in_tx, in_rx) = mpsc::channel(64);
        let (out_tx, out_rx) = mpsc::channel(64);
        ((in_tx, out_rx), ChanTransport { rx: in_rx, tx: out_tx })
    }

    fn fake_control(platform: AppPlatform) -> (Arc<ControlConn>, mpsc::Receiver<String>) {
        let (to_app, rx) = mpsc::channel::<String>(8);
        let conn = Arc::new(ControlConn {
            id: u64::MAX - 1,
            platform,
            to_app,
            pending: Mutex::new(HashMap::new()),
            state: RwLock::new(json!({})),
        });
        (conn, rx)
    }

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    #[test]
    fn pair_hello_platform_defaults_to_ios_and_refuses_unknown() {
        let id = "8c0f6b4e-1d2a-4f3b-9c5d-6e7f8a9b0c1d";
        let pubkey = STANDARD.encode([9u8; 32]);
        let hello = |platform: Option<&str>| {
            let mut v = json!({ "type": "hello", "phoneId": id, "pub": pubkey });
            if let Some(p) = platform {
                v["platform"] = json!(p);
            }
            parse_pair_hello(&v.to_string(), id).map(|(_, platform, _)| platform)
        };
        // Old iPhone apps send no platform.
        assert_eq!(hello(None), Some(AppPlatform::Ios));
        assert_eq!(hello(Some("ios")), Some(AppPlatform::Ios));
        assert_eq!(hello(Some("Android")), Some(AppPlatform::Android));
        assert_eq!(hello(Some("windows")), None);
    }

    #[test]
    fn listing_fields_per_platform() {
        let ios = listing_fields(AppPlatform::Ios, Some(&json!({ "foreground": true, "canTap": true })));
        assert_eq!(ios["route"], APP_ROUTE);
        assert_eq!(ios["platform"], "ios");
        assert_eq!(ios["canTap"], false, "iPhones never take taps");
        assert!(ios["consent"].is_null());
        assert_eq!(ios["screen"], true);

        let live = listing_fields(AppPlatform::Android, Some(&json!({ "canTap": true, "consent": "granted" })));
        assert_eq!(live["route"], ANDROID_APP_ROUTE);
        assert_eq!(live["canTap"], true);
        assert_eq!(live["consent"], "granted");
        assert_eq!(live["appOpen"], true);

        let no_access = listing_fields(AppPlatform::Android, Some(&json!({ "consent": "bogus" })));
        assert_eq!(no_access["canTap"], false);
        assert_eq!(no_access["consent"], "needed");

        let closed = listing_fields(AppPlatform::Android, None);
        assert_eq!(closed["canTap"], false);
        assert_eq!(closed["consent"], "unknown");
        assert_eq!(closed["appOpen"], false);
    }

    #[test]
    fn viewer_input_becomes_an_input_events_request() {
        let frame = json!({ "type": "input", "events": [
            { "type": "touch", "action": "down", "pointerId": 0, "x": 10, "y": 20 },
            { "type": "text", "text": "hi" },
            { "type": "back" },
        ]});
        let request: Value = serde_json::from_str(&input_request(&frame).unwrap()).unwrap();
        assert_eq!(request["method"], "input.events");
        assert_eq!(request["params"]["events"].as_array().unwrap().len(), 3);
        assert!(request["id"].as_str().is_some());

        assert!(input_request(&json!({ "type": "displayPower", "on": true })).is_none());
        assert!(input_request(&json!({ "type": "input", "events": [] })).is_none());
        assert!(input_request(&json!({ "type": "input", "events": [{ "type": "shell", "cmd": "rm" }] })).is_none());
        assert!(input_request(&json!({ "type": "input", "events": [{ "type": "text", "text": "x".repeat(MAX_INPUT_TEXT + 1) }] })).is_none());
        let many: Vec<Value> = (0..=MAX_INPUT_EVENTS).map(|_| json!({ "type": "home" })).collect();
        assert!(input_request(&json!({ "type": "input", "events": many })).is_none());
    }

    #[test]
    fn input_events_is_never_a_bot_rpc_method() {
        assert!(VIEWER_ONLY_METHODS.contains(&"input.events"));
        assert!(!RPC_METHODS.contains(&"input.events"));
        assert!(validate_rpc("input.events", &json!({ "events": [] })).is_err());
    }

    /// The bot route refuses `input.events` with 403 `viewer_only`, whoever calls.
    #[tokio::test]
    async fn rpc_route_refuses_input_events() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let app = phone_app_router().layer(Extension(user("user-rpc"))).with_state(state);
        let resp = app
            .oneshot(
                Request::post(format!("/phones/app/{}/rpc", uuid::Uuid::new_v4()))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "method": "input.events", "params": { "events": [{ "type": "home" }] } }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"], "viewer_only");
    }

    /// Full-control viewer taps reach the Android app as `input.events`;
    /// a read-only viewer's frames never do.
    #[tokio::test]
    async fn full_viewer_input_reaches_the_android_control_socket() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        state
            .db
            .connect()
            .unwrap()
            .execute("INSERT INTO users (id, email) VALUES ('user-and', 'user-and@test.local')", [])
            .unwrap();
        let phone = uuid::Uuid::new_v4().to_string();
        let serial = format!("{APP_SERIAL_PREFIX}{phone}");
        let key = EnvelopeKey::from_hex(&"cd".repeat(32)).unwrap();
        assert!(store_phone(&state, &user("user-and"), &phone, &key, &None, &None, AppPlatform::Android).await);
        let (conn, mut to_app) = fake_control(AppPlatform::Android);
        let phone_key = ("user-and".to_string(), phone.clone());
        hub().controls.write().await.insert(phone_key.clone(), conn);

        let ((_ingest_in, mut ingest_out), ingest) = chan_pair();
        tokio::spawn(run_ingest(ingest, state.clone(), "user-and".to_string(), phone.clone()));
        let registry = state.phone_link.clone();
        for _ in 0..100 {
            if registry.is_online("user-and", &serial).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(registry.is_online("user-and", &serial).await);

        // Read-only viewer: attaches (the ingest resumes), its input is dropped.
        let ((ro_in, _ro_out), ro) = chan_pair();
        tokio::spawn(crate::phone_ws::run_phone_stream_viewer(
            ro, registry.clone(), "user-and".to_string(), serial.clone(), true, Duration::from_secs(30),
        ));
        let resumed = tokio::time::timeout(Duration::from_secs(2), ingest_out.recv()).await.unwrap().unwrap();
        assert!(matches!(&resumed, Message::Text(t) if t.contains("resume")));
        let tap = json!({ "type": "input", "events": [{ "type": "touch", "action": "down", "pointerId": 0, "x": 1, "y": 2 }] }).to_string();
        ro_in.send(Message::Text(tap.clone())).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(300), to_app.recv()).await.is_err(), "read-only input leaked");

        // Full-control viewer: the same frame becomes input.events.
        let ((full_in, _full_out), full) = chan_pair();
        tokio::spawn(crate::phone_ws::run_phone_stream_viewer(
            full, registry.clone(), "user-and".to_string(), serial.clone(), false, Duration::from_secs(30),
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        full_in.send(Message::Text(tap)).await.unwrap();
        let sent = tokio::time::timeout(Duration::from_secs(2), to_app.recv()).await.unwrap().unwrap();
        let sent: Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(sent["method"], "input.events");
        assert_eq!(sent["params"]["events"][0]["type"], "touch");
        hub().controls.write().await.remove(&phone_key);
    }

    /// A viewer that arrives while the Android phone is not sharing asks it to
    /// (once per throttle window); iPhones are never asked.
    #[tokio::test]
    async fn waiting_viewer_sends_screen_request_to_android_only() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let registry = state.phone_link.clone();
        let android = uuid::Uuid::new_v4().to_string();
        let (conn, mut to_app) = fake_control(AppPlatform::Android);
        hub().controls.write().await.insert(("user-sr".to_string(), android.clone()), conn);
        assert!(can_request_screen("user-sr", &android).await);

        let ((_v_in, _v_out), viewer) = chan_pair();
        tokio::spawn(crate::phone_ws::run_phone_stream_viewer(
            viewer, registry.clone(), "user-sr".to_string(), format!("{APP_SERIAL_PREFIX}{android}"), false, Duration::from_secs(30),
        ));
        let frame = tokio::time::timeout(Duration::from_secs(2), to_app.recv()).await.unwrap().unwrap();
        let frame: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame["type"], "screen_request");
        assert_eq!(frame["serial"], format!("{APP_SERIAL_PREFIX}{android}"));
        // Throttled: a second waiting viewer does not ask again right away.
        assert!(!request_screen("user-sr", &android).await);

        let iphone = uuid::Uuid::new_v4().to_string();
        let (conn, mut ios_rx) = fake_control(AppPlatform::Ios);
        hub().controls.write().await.insert(("user-sr".to_string(), iphone.clone()), conn);
        assert!(!can_request_screen("user-sr", &iphone).await);
        assert!(!request_screen("user-sr", &iphone).await);
        assert!(ios_rx.try_recv().is_err());
        hub().controls.write().await.remove(&("user-sr".to_string(), android));
        hub().controls.write().await.remove(&("user-sr".to_string(), iphone));
    }

    /// Pairing routes: the local Desktop, or a cloud-api-signed request whose
    /// owner is this runtime's owner AND the caller. Nothing else.
    #[tokio::test]
    async fn pairing_routes_accept_only_the_signed_owner_off_the_desktop() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        const TOKEN: &str = "allternit_runtime_pair_test_token";
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let secret: Arc<dyn crate::relay_auth::RelaySecret> =
            Arc::new(crate::relay_auth::StaticRelaySecret { token: TOKEN.into(), owner: "user-own".into() });
        let app = |caller: &str| {
            phone_app_router()
                .layer(Extension(secret.clone()))
                .layer(Extension(user(caller)))
                .with_state(state.clone())
        };
        let get = |signed_as: Option<(&str, &str)>| {
            let mut req = Request::get("/phones/app/pair/pending").header(crate::cowork_devices_routes::RELAYED_HEADER, "1");
            if let Some((token, owner)) = signed_as {
                for (k, v) in crate::relay_auth::signed_headers(token, owner, "GET", "/phones/app/pair/pending", b"") {
                    req = req.header(k, v);
                }
            }
            req.body(Body::empty()).unwrap()
        };
        let status = |resp: Response| resp.status();
        assert_eq!(status(app("user-own").oneshot(get(Some((TOKEN, "user-own")))).await.unwrap()), StatusCode::OK);
        // Unsigned relayed request (the generic proxy): refused.
        assert_eq!(status(app("user-own").oneshot(get(None)).await.unwrap()), StatusCode::FORBIDDEN);
        // Signed with another key: refused.
        assert_eq!(status(app("user-own").oneshot(get(Some(("wrong-token", "user-own")))).await.unwrap()), StatusCode::FORBIDDEN);
        // Signed for the owner but the caller's account is someone else: refused.
        assert_eq!(status(app("user-other").oneshot(get(Some((TOKEN, "user-own")))).await.unwrap()), StatusCode::FORBIDDEN);

        // Decision from the signed owner resolves the waiting pair.
        let phone = uuid::Uuid::new_v4().to_string();
        let (decide, decision) = oneshot::channel();
        hub().pairs.lock().await.insert(
            ("user-own".to_string(), phone.clone()),
            PendingPair { id: u64::MAX, request: json!({}), decide },
        );
        let path = format!("/phones/app/pair/{phone}/decision");
        let body = br#"{"approve":true}"#;
        let mut req = Request::post(&path).header("content-type", "application/json").header(crate::cowork_devices_routes::RELAYED_HEADER, "1");
        for (k, v) in crate::relay_auth::signed_headers(TOKEN, "user-own", "POST", &path, body) {
            req = req.header(k, v);
        }
        let resp = app("user-own").oneshot(req.body(Body::from(body.to_vec())).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(decision.await, Ok(true));
        // A signature over a different body does not carry over.
        let mut req = Request::post(&path).header("content-type", "application/json");
        for (k, v) in crate::relay_auth::signed_headers(TOKEN, "user-own", "POST", &path, br#"{"approve":false}"#) {
            req = req.header(k, v);
        }
        let resp = app("user-own").oneshot(req.body(Body::from(body.to_vec())).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
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
