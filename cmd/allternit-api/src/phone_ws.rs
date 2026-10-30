//! Phones 2.0 phone stream relay (Phase 2 platform lane).
//!
//! Lets any surface (ai.allternit.com phone layout, m.allternit.com PWA) view
//! and control a linked Android through the account's Desktop app. It mirrors
//! the computer-lane proxy (`computer_ws.rs`) minus VNC: the payload is the
//! scrcpy H.264 stream (binary) and JSON control envelopes (text), treated as
//! OPAQUE — the relay never parses scrcpy, video, or control semantics.
//!
//! # Routes
//!
//! * `GET /api/v1/phones` (Clerk/desktop-auth): the caller's registered phones
//!   from cloud PG, `{ "phones": [{ "serial", "model", "name", "online", "callBridge" }] }`.
//!   `callBridge` is `{ "enabled", "botName"? }` as the Desktop last announced it in the
//!   hello (`phones[].callBridge`, forwarded with the registered-phones upsert), or null.
//!   `online` = a phone-link session for `(account, serial)` is attached to
//!   THIS api instance's in-memory registry.
//! * `POST /api/v1/phones/:serial/ws-token`, body `{ "readOnly"?: bool }` →
//!   `{ "token", "expiresInSec" }`. Requires the phone row to exist for the
//!   caller's account AND a live phone-link session. HMAC-signed, TTL 300 s,
//!   claims `{ sub, serial, purpose: "phone_stream", read_only, exp }`.
//!   `readOnly` defaults to TRUE; a full-control token injects input into a
//!   personal phone, so `readOnly: false` goes through the same ACI
//!   confirmation grant as a full-control VNC token (`computer_ws`).
//! * `POST /api/v1/phones/:serial/viewer-token` → `{ "token", "expiresInSec": 600,
//!   "readOnly": true }`: a view-only token (purpose `phone_view`) for the
//!   Platform API live view (cloud-api `POST /v1/phones/:serial/live`). The
//!   stream route forces it read-only whatever its claim says.
//! * `/ws/phones/:serial/stream?token=…` (PUBLIC mount — the HMAC token is the
//!   credential, exactly like the computer VNC route): validates the token,
//!   then proxies frames bidirectionally between the browser and the account's
//!   attached phone-link session. When the token crosses expiry mid-stream the
//!   socket is closed with code 4401 after a JSON text frame
//!   `{"type":"end","reason":"token_expired"}`.
//! * `/ws/phone-link/session` (protected mount; the Desktop authenticates with
//!   the `x-allternit-desktop-access-token` credential like every other
//!   Desktop call — auth_middleware resolves it to the account): the Desktop's
//!   session client. First frame MUST be
//!   `{"type":"hello","phones":[{"serial","model","name","callBridge"?}]}`; the phones are
//!   upserted in cloud PG (`last_seen_at` touched) and marked online. 30 s
//!   heartbeat ping/pong.
//!
//! # Session protocol (Desktop lane contract)
//!
//! * One phone-link socket carries the frames of ONE streamed phone at a time
//!   (`one active stream per socket`). A phone-link socket may hello multiple
//!   phones (they all register and appear online; this is how a Desktop lists
//!   its whole tray), but the socket's desktop→browser output is routed to
//!   viewers of a single serial at a time — scrcpy is one device per process,
//!   so the Desktop opens one phone-link socket per phone it streams. Opening
//!   a second serial's stream on a busy socket makes that viewer WAIT until
//!   the socket frees up; it never receives another phone's frames.
//! * Relay-level JSON text frames the relay itself sends on the session
//!   socket (distinct from the opaque browser/desktop payloads):
//!   - on viewer attach:   `{"type":"viewer_attached","serial":"S"}` — the
//!     Desktop starts scrcpy for S (best effort; dropped if the pipe is full).
//!   - on viewer detach:   `{"type":"viewer_detached","serial":"S"}` — the
//!     Desktop may stop scrcpy for S.
//! * Everything else on the socket is the opaque data plane: text (control
//!   JSON) and binary (video) pass through untouched in both directions,
//!   order preserved per direction.
//! * Browser sockets that arrive while no session is attached (or while the
//!   socket streams another phone) are buffered waiters; they attach when the
//!   stream frees up, and get `{"type":"end","reason":"desktop_offline"}` when
//!   the session disconnects or the token dies (4401 for expiry).
//!
//! # Read-only enforcement
//!
//! The payload is opaque to the relay, so read-only is enforced at the pipe:
//! a viewer with a `read_only` token can receive but its browser→desktop
//! frames are dropped (the viewer cannot inject anything). Full-control
//! (`read_only: false`) tokens pass everything through.
//!
//! # Backpressure
//!
//! Bounded channels both directions (`*_CHANNEL_CAP`). Producers never block
//! on video: a full channel DROPS binary frames. Control (text) frames wait
//! up to `STALL_TIMEOUT` (5 s); a side stalled past that is treated as dead
//! and its viewer/session ends.
//!
//! # Close codes
//!
//! 4401 = token expired mid-stream (contract). Everything else that ends a
//! stream sends its `{"type":"end","reason":…}` JSON text frame first and
//! closes with 1011 (ERROR): `desktop_offline`, `invalid_hello`, stalls.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::auth::AuthUser;
use crate::computer_routes::{error_response, ApprovalQuery};
use crate::AppState;

const PHONE_STREAM_TOKEN_TTL_SECONDS: u64 = 300;
/// Viewer tokens (`POST /api/v1/phones/:serial/viewer-token`, minted for the
/// Platform API's `POST /v1/phones/:serial/live`): always view-only, 10 min.
pub const PHONE_VIEW_TOKEN_TTL_SECONDS: u64 = 600;
/// Purpose of a viewer token: accepted by the stream route, always read-only.
pub const PHONE_VIEW_PURPOSE: &str = "phone_view";
/// Contract: 4401 closes the stream when the token crosses expiry.
const PHONE_TOKEN_EXPIRED_CLOSE_CODE: u16 = 4401;
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// No inbound frame/pong for this long → the peer is dead; end the session.
const HEARTBEAT_DEAD: Duration = Duration::from_secs(90);
/// Bounded channels both directions; full → binary dropped, text stalls ≤5 s.
const DESKTOP_CHANNEL_CAP: usize = 128;
const VIEWER_CHANNEL_CAP: usize = 128;
const STALL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HELLO_PHONES: usize = 64;

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp() as u64
}

// ── Signed phone-stream WebSocket tokens ─────────────────────────────────────

/// Claims of a phone-stream ws token (POST /api/v1/phones/:serial/ws-token).
/// Signed like the desktop tokens (`bot_desktop_stream`): HS256 over
/// `b64url(header).b64url(claims)`, shared secret `desktop_ws_secret`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneStreamClaims {
    /// Clerk account the phone is registered to.
    pub sub: String,
    /// adb serial the token authorizes.
    pub serial: String,
    /// Always `"phone_stream"`.
    pub purpose: String,
    #[serde(default)]
    pub read_only: bool,
    pub exp: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PhoneTokenError {
    #[error("token has expired")]
    Expired,
    #[error("invalid token format")]
    Format,
    #[error("base64 decode error")]
    Decode,
    #[error("json error")]
    Json,
    #[error("invalid signature")]
    Signature,
    #[error("token is not scoped to this phone")]
    SerialMismatch,
    #[error("token purpose does not match this endpoint")]
    PurposeMismatch,
}

fn phone_token_with_exp(
    secret: &str,
    sub: &str,
    serial: &str,
    exp: u64,
    read_only: bool,
) -> String {
    phone_token_with_purpose(secret, sub, serial, exp, read_only, "phone_stream")
}

fn phone_token_with_purpose(
    secret: &str,
    sub: &str,
    serial: &str,
    exp: u64,
    read_only: bool,
    purpose: &str,
) -> String {
    use crate::bot_desktop_stream::{b64_encode, hmac_sign};
    let header = json!({"alg": "HS256", "typ": "PT"});
    let claims = PhoneStreamClaims {
        sub: sub.to_string(),
        serial: serial.to_string(),
        purpose: purpose.to_string(),
        read_only,
        exp,
    };
    let header_b64 = b64_encode(&serde_json::to_vec(&header).unwrap_or_default());
    let payload_b64 = b64_encode(&serde_json::to_vec(&claims).unwrap_or_default());
    let signing_input = format!("{header_b64}.{payload_b64}");
    let signature = hmac_sign(secret, &signing_input);
    format!("{signing_input}.{signature}")
}

/// Sign a short-lived phone-stream ws token (TTL seconds from now).
pub fn sign_phone_stream_token(
    secret: &str,
    sub: &str,
    serial: &str,
    expires_in_seconds: u64,
    read_only: bool,
) -> String {
    phone_token_with_exp(secret, sub, serial, now_secs() + expires_in_seconds, read_only)
}

/// Sign a view-only phone viewer token (purpose `phone_view`, TTL seconds).
pub fn sign_phone_view_token(secret: &str, sub: &str, serial: &str, expires_in_seconds: u64) -> String {
    phone_token_with_purpose(secret, sub, serial, now_secs() + expires_in_seconds, true, PHONE_VIEW_PURPOSE)
}

/// Verify a phone-stream ws token: signature + expiry, then scope it to this
/// serial and the `"phone_stream"` purpose.
pub fn verify_phone_stream_token(
    secret: &str,
    token: &str,
) -> Result<PhoneStreamClaims, PhoneTokenError> {
    use crate::bot_desktop_stream::{b64_decode, constant_time_eq, hmac_sign};
    let mut parts = token.split('.');
    let header_b64 = parts.next().ok_or(PhoneTokenError::Format)?;
    let payload_b64 = parts.next().ok_or(PhoneTokenError::Format)?;
    let signature_b64 = parts.next().ok_or(PhoneTokenError::Format)?;
    if parts.next().is_some() {
        return Err(PhoneTokenError::Format);
    }

    let signing_input = format!("{header_b64}.{payload_b64}");
    let expected = hmac_sign(secret, &signing_input);
    if !constant_time_eq(signature_b64.as_bytes(), expected.as_bytes()) {
        return Err(PhoneTokenError::Signature);
    }

    let payload_bytes = b64_decode(payload_b64).map_err(|_| PhoneTokenError::Decode)?;
    let claims: PhoneStreamClaims =
        serde_json::from_slice(&payload_bytes).map_err(|_| PhoneTokenError::Json)?;
    if claims.exp < now_secs() {
        return Err(PhoneTokenError::Expired);
    }
    Ok(claims)
}

fn validate_phone_stream_token(
    secret: &str,
    token: &str,
    serial: &str,
) -> Result<PhoneStreamClaims, PhoneTokenError> {
    let mut claims = verify_phone_stream_token(secret, token)?;
    if claims.serial != serial {
        return Err(PhoneTokenError::SerialMismatch);
    }
    match claims.purpose.as_str() {
        "phone_stream" => {}
        // A viewer token never carries input, whatever its claim says.
        PHONE_VIEW_PURPOSE => claims.read_only = true,
        _ => return Err(PhoneTokenError::PurposeMismatch),
    }
    Ok(claims)
}

// ── Cloud directory (registered_phones in cloud PG) ─────────────────────────

/// One registered phone row as cloud-api returns it
/// (`GET /api/v1/runtime-devices/me/registered-phones`).
#[derive(Debug, Clone, Deserialize)]
pub struct CloudPhone {
    pub serial: String,
    pub model: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    /// The phone's call-bridge summary as the Desktop last announced it
    /// (contract C5); `None` when never announced or malformed.
    #[serde(default, alias = "callBridge", deserialize_with = "lenient_call_bridge")]
    pub call_bridge: Option<CallBridgeSummary>,
}

/// Longest bot name kept in a call-bridge summary.
pub const MAX_CALL_BRIDGE_BOT_NAME: usize = 80;

/// What the Desktop says about a phone's call bridge ("bot answers calls"),
/// so web and the PWA can show it read-only. JSON `{enabled, botName?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallBridgeSummary {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_name: Option<String>,
}

impl CallBridgeSummary {
    /// From any JSON value: anything malformed is `None`; the bot name is
    /// trimmed, empty dropped, and capped at [`MAX_CALL_BRIDGE_BOT_NAME`] chars.
    pub fn from_value(v: &Value) -> Option<Self> {
        let enabled = v.get("enabled")?.as_bool()?;
        let bot_name = v
            .get("botName")
            .or_else(|| v.get("bot_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(MAX_CALL_BRIDGE_BOT_NAME).collect());
        Some(Self { enabled, bot_name })
    }
}

fn lenient_call_bridge<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<CallBridgeSummary>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(CallBridgeSummary::from_value(&v))
}

#[derive(Debug, thiserror::Error)]
pub enum DirectoryError {
    #[error("this runtime is not paired to the cloud")]
    NotPaired,
    #[error("cloud phone registry request failed: {0}")]
    Transport(String),
}

/// Client for cloud-api's registered_phones routes. A seam so tests don't
/// need a network (same pattern as `phone_sync::NumberSource`).
#[async_trait::async_trait]
pub trait PhoneDirectory: Send + Sync {
    async fn list_phones(
        &self,
        cloud_url: &str,
        bearer: &str,
    ) -> Result<Vec<CloudPhone>, DirectoryError>;
    async fn upsert_phones(
        &self,
        cloud_url: &str,
        bearer: &str,
        phones: &[PhoneHelloPhone],
    ) -> Result<(), DirectoryError>;
}

pub struct HttpPhoneDirectory;

#[async_trait::async_trait]
impl PhoneDirectory for HttpPhoneDirectory {
    async fn list_phones(
        &self,
        cloud_url: &str,
        bearer: &str,
    ) -> Result<Vec<CloudPhone>, DirectoryError> {
        if bearer.is_empty() {
            return Err(DirectoryError::NotPaired);
        }
        let url = format!("{cloud_url}/api/v1/runtime-devices/me/registered-phones");
        let resp = reqwest::Client::new()
            .get(&url)
            .bearer_auth(bearer)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| DirectoryError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(DirectoryError::Transport(format!(
                "cloud returned {}",
                resp.status()
            )));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| DirectoryError::Transport(e.to_string()))?;
        let phones = body
            .get("phones")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(phones
            .iter()
            .filter_map(|p| serde_json::from_value::<CloudPhone>(p.clone()).ok())
            .collect())
    }

    async fn upsert_phones(
        &self,
        cloud_url: &str,
        bearer: &str,
        phones: &[PhoneHelloPhone],
    ) -> Result<(), DirectoryError> {
        if bearer.is_empty() {
            return Err(DirectoryError::NotPaired);
        }
        let url = format!("{cloud_url}/api/v1/runtime-devices/me/registered-phones");
        let resp = reqwest::Client::new()
            .put(&url)
            .bearer_auth(bearer)
            .json(&json!({ "phones": phones }))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| DirectoryError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(DirectoryError::Transport(format!(
                "cloud returned {}",
                resp.status()
            )));
        }
        Ok(())
    }
}

// ── Hello + registry model ───────────────────────────────────────────────────

/// A phone announced in the session hello (also the upsert payload shape for
/// the cloud directory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneHelloPhone {
    pub serial: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional `callBridge: {enabled, botName?}`; malformed is ignored.
    #[serde(default, rename = "callBridge", deserialize_with = "lenient_call_bridge", skip_serializing_if = "Option::is_none")]
    pub call_bridge: Option<CallBridgeSummary>,
}

/// Parse + validate the session's first frame. Returns the normalized phone
/// list; the frame must be exactly
/// `{"type":"hello","phones":[{"serial","model"?,...}]}` with 1..=64 phones.
pub(crate) fn parse_phone_hello(text: &str) -> Result<Vec<PhoneHelloPhone>, &'static str> {
    if text.len() > 16_384 {
        return Err("hello frame too large");
    }
    #[derive(Deserialize)]
    struct HelloFrame {
        #[serde(rename = "type")]
        kind: String,
        phones: Vec<PhoneHelloPhone>,
    }
    let frame: HelloFrame =
        serde_json::from_str(text).map_err(|_| "malformed hello frame")?;
    if frame.kind != "hello" {
        return Err("first frame must be {\"type\":\"hello\",...}");
    }
    if frame.phones.is_empty() || frame.phones.len() > MAX_HELLO_PHONES {
        return Err("hello must list between 1 and 64 phones");
    }
    let mut seen = std::collections::HashSet::new();
    let mut phones = Vec::with_capacity(frame.phones.len());
    for phone in frame.phones {
        let serial = phone.serial.trim().to_string();
        if serial.is_empty()
            || serial.len() > 128
            || serial.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err("invalid phone serial");
        }
        if !seen.insert(serial.clone()) {
            return Err("duplicate phone serial in hello");
        }
        for value in [&phone.model, &phone.name] {
            if let Some(v) = value {
                if v.chars().count() > 256 {
                    return Err("phone model/name too long");
                }
            }
        }
        phones.push(PhoneHelloPhone {
            serial,
            model: phone.model,
            name: phone.name,
            call_bridge: phone.call_bridge,
        });
    }
    Ok(phones)
}

/// A browser viewer attached to a phone pipe. `to_viewer` carries data frames
/// as `(frame, close_code_to_send_after)`; the registry closes the channel
/// (after enqueueing an end frame) to end the viewer.
#[derive(Debug, Clone)]
struct Viewer {
    id: u64,
    to_viewer: mpsc::Sender<ViewerPacket>,
}

type ViewerPacket = (Message, Option<u16>);

/// One desktop-attached phone-link socket. All registry keys
/// `(account, serial)` from its hello point at this struct.
pub(crate) struct PhoneLinkSession {
    id: u64,
    account: String,
    phones: Vec<PhoneHelloPhone>,
    /// Frames bound for the desktop socket (browser data + relay control).
    to_desktop: mpsc::Sender<Message>,
    /// Which serial currently owns the socket's desktop→browser output.
    /// One active stream per socket (see module docs).
    streaming: RwLock<Option<String>>,
    /// Attached viewers by serial.
    viewers: RwLock<HashMap<String, Vec<Viewer>>>,
    /// Video frames dropped under backpressure (logged sparsely).
    video_drops: AtomicUsize,
}

impl PhoneLinkSession {
    /// The returned receiver is the pump's desktop-bound stream; it must be
    /// held by whoever runs the session socket (the senders live on the
    /// session and in the registry's viewers).
    pub(crate) fn new(
        id: u64,
        account: String,
        phones: Vec<PhoneHelloPhone>,
    ) -> (Arc<Self>, mpsc::Receiver<Message>) {
        let (to_desktop, from_producers) = mpsc::channel(DESKTOP_CHANNEL_CAP);
        (
            Arc::new(Self {
                id,
                account,
                phones,
                to_desktop,
                streaming: RwLock::new(None),
                viewers: RwLock::new(HashMap::new()),
                video_drops: AtomicUsize::new(0),
            }),
            from_producers,
        )
    }

    /// True while at least one viewer is attached to `serial` (the app-phone
    /// ingest uses it to pause the broadcast when the last viewer leaves).
    pub(crate) async fn has_viewers(&self, serial: &str) -> bool {
        self.viewers
            .read()
            .await
            .get(serial)
            .is_some_and(|list| !list.is_empty())
    }
}

struct Waiter {
    id: u64,
    wake: mpsc::Sender<WakeSignal>,
}

#[derive(Debug)]
pub(crate) enum WakeSignal {
    SessionAvailable,
    DesktopOffline,
}

pub(crate) enum AttachOutcome {
    Attached { session: Arc<PhoneLinkSession> },
    Waiting,
}

struct RegistryInner {
    /// `(account, serial)` -> the attached session. Presence == "online".
    sessions: HashMap<(String, String), Arc<PhoneLinkSession>>,
    /// Browsers waiting for `(account, serial)` to become streamable.
    waiters: HashMap<(String, String), Vec<Waiter>>,
}

/// In-memory phone-link registry for this api instance (decided model: the
/// registry is in-memory here; cloud PG `registered_phones` rows are the only
/// persistence, and they carry identity + last_seen, never stream data).
pub struct PhoneLinkRegistry {
    directory: Arc<dyn PhoneDirectory>,
    inner: RwLock<RegistryInner>,
    next_session_id: AtomicU64,
    next_viewer_id: AtomicU64,
}

impl PhoneLinkRegistry {
    pub fn new(directory: Arc<dyn PhoneDirectory>) -> Arc<Self> {
        Arc::new(Self {
            directory,
            inner: RwLock::new(RegistryInner {
                sessions: HashMap::new(),
                waiters: HashMap::new(),
            }),
            next_session_id: AtomicU64::new(1),
            next_viewer_id: AtomicU64::new(1),
        })
    }

    /// The production registry (HTTP cloud directory).
    pub fn new_http() -> Arc<Self> {
        Self::new(Arc::new(HttpPhoneDirectory))
    }

    pub(crate) fn next_session_id(&self) -> u64 {
        self.next_session_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn next_viewer_id(&self) -> u64 {
        self.next_viewer_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// True while a phone-link session for `(account, serial)` is attached.
    pub async fn is_online(&self, account: &str, serial: &str) -> bool {
        self.inner
            .read()
            .await
            .sessions
            .contains_key(&(account.to_string(), serial.to_string()))
    }

    /// Attach a viewer to `(account, serial)`, or buffer it as a waiter —
    /// ONE atomic registry decision, so no wake can be missed:
    /// - no session, or the socket streams another phone → the waiter is
    ///   queued (a later `register_session`/`detach`/`unregister_session`
    ///   wakes it and REMOVES the entry);
    /// - stream free → the viewer attaches and owns the socket's output.
    ///
    /// Lock order (kept consistent everywhere): `inner` → `streaming` →
    /// `viewers`; no path takes `inner` while holding a session lock.
    pub(crate) async fn attach_or_wait(
        &self,
        account: &str,
        serial: &str,
        to_viewer: mpsc::Sender<ViewerPacket>,
        viewer_id: u64,
        wake: mpsc::Sender<WakeSignal>,
    ) -> AttachOutcome {
        let mut inner = self.inner.write().await;
        let key = (account.to_string(), serial.to_string());
        let Some(session) = inner.sessions.get(&key).cloned() else {
            inner.waiters.entry(key).or_default().push(Waiter {
                id: viewer_id,
                wake,
            });
            return AttachOutcome::Waiting;
        };
        {
            let mut streaming = session.streaming.write().await;
            let free = streaming
                .as_deref()
                .map(|current| current == serial)
                .unwrap_or(true);
            if !free {
                drop(streaming);
                inner.waiters.entry(key).or_default().push(Waiter {
                    id: viewer_id,
                    wake,
                });
                return AttachOutcome::Waiting;
            }
            *streaming = Some(serial.to_string());
        }
        session
            .viewers
            .write()
            .await
            .entry(serial.to_string())
            .or_default()
            .push(Viewer {
                id: viewer_id,
                to_viewer: to_viewer.clone(),
            });
        // Best-effort viewer-presence notification on the session socket.
        let _ = session.to_desktop.try_send(Message::Text(
            json!({"type": "viewer_attached", "serial": serial}).to_string(),
        ));
        AttachOutcome::Attached { session }
    }

    pub(crate) async fn unregister_waiter(&self, account: &str, serial: &str, waiter_id: u64) {
        let mut inner = self.inner.write().await;
        let key = (account.to_string(), serial.to_string());
        if let Some(list) = inner.waiters.get_mut(&key) {
            list.retain(|w| w.id != waiter_id);
            if list.is_empty() {
                inner.waiters.remove(&key);
            }
        }
    }

    /// Register every hello phone as online, and wake that serial's waiters.
    pub(crate) async fn register_session(&self, session: Arc<PhoneLinkSession>) {
        let mut inner = self.inner.write().await;
        for phone in &session.phones {
            let key = (session.account.clone(), phone.serial.clone());
            inner.sessions.insert(key.clone(), session.clone());
            if let Some(waiters) = inner.waiters.remove(&key) {
                for waiter in waiters {
                    let _ = waiter.wake.try_send(WakeSignal::SessionAvailable);
                }
            }
        }
    }

    /// Session socket ended: mark its phones offline, end attached viewers
    /// and buffered waiters with `desktop_offline`.
    pub(crate) async fn unregister_session(&self, session: &Arc<PhoneLinkSession>) {
        {
            let mut inner = self.inner.write().await;
            for phone in &session.phones {
                let key = (session.account.clone(), phone.serial.clone());
                // A newer session may already own the key (reconnect race).
                let ours = inner
                    .sessions
                    .get(&key)
                    .is_some_and(|current| current.id == session.id);
                if !ours {
                    continue;
                }
                inner.sessions.remove(&key);
                if let Some(waiters) = inner.waiters.remove(&key) {
                    for waiter in waiters {
                        let _ = waiter.wake.try_send(WakeSignal::DesktopOffline);
                    }
                }
            }
        }
        let end: ViewerPacket = (
            Message::Text(end_frame("desktop_offline")),
            Some(axum::extract::ws::close_code::ERROR),
        );
        {
            let mut streaming = session.streaming.write().await;
            *streaming = None;
            let mut viewers = session.viewers.write().await;
            for list in viewers.values_mut() {
                for viewer in list.drain(..) {
                    let _ = viewer.to_viewer.try_send(end.clone());
                }
            }
        }
        debug!(account = %session.account, "phone-link session marked offline");
    }

    /// Viewer detached: drop it; when the streaming serial's last viewer
    /// leaves, free the socket and promote the longest-waiting waiter of any
    /// phone this session serves.
    pub(crate) async fn detach(
        &self,
        session: &Arc<PhoneLinkSession>,
        account: &str,
        serial: &str,
        viewer_id: u64,
    ) {
        let mut freed = false;
        {
            let mut streaming = session.streaming.write().await;
            let mut viewers = session.viewers.write().await;
            let empty = match viewers.get_mut(serial) {
                Some(list) => {
                    list.retain(|v| v.id != viewer_id);
                    list.is_empty()
                }
                None => true,
            };
            if empty {
                viewers.remove(serial);
                if streaming.as_deref() == Some(serial) {
                    *streaming = None;
                    freed = true;
                }
            }
        }
        let _ = session.to_desktop.try_send(Message::Text(
            json!({"type": "viewer_detached", "serial": serial}).to_string(),
        ));
        if !freed {
            return;
        }
        // The stream is free: wake the longest-waiting waiter of the first
        // phone that has one. If a racing attach grabbed the stream first,
        // the promoted waiter re-buffers on its retry — no wake is lost.
        let mut inner = self.inner.write().await;
        for phone in &session.phones {
            let key = (account.to_string(), phone.serial.clone());
            if let Some(list) = inner.waiters.get_mut(&key) {
                if !list.is_empty() {
                    let waiter = list.remove(0);
                    if list.is_empty() {
                        inner.waiters.remove(&key);
                    }
                    let _ = waiter.wake.try_send(WakeSignal::SessionAvailable);
                    break;
                }
            }
        }
    }

    // ── Cloud directory helpers (best-effort; see DirectoryError) ────────────

    /// The account's registered phones; `None` = the cloud is unreachable.
    /// An unpaired runtime answers with an empty list (there can be no rows).
    pub async fn cloud_list_phones(&self) -> Option<Vec<CloudPhone>> {
        let bearer = crate::phone_sync::runtime_bearer().unwrap_or_default();
        let url = crate::phone_sync::cloud_base();
        match self.directory.list_phones(&url, &bearer).await {
            Ok(rows) => Some(rows),
            Err(DirectoryError::NotPaired) => Some(Vec::new()),
            Err(DirectoryError::Transport(e)) => {
                warn!(error = %e, "registered phones list failed");
                None
            }
        }
    }

    /// Upsert phones (hello + disconnect-time last_seen refresh). Never
    /// blocks the relay: failures are logged and dropped.
    pub async fn cloud_upsert_phones(&self, phones: &[PhoneHelloPhone]) {
        let bearer = crate::phone_sync::runtime_bearer().unwrap_or_default();
        if bearer.is_empty() {
            debug!("runtime not paired; skipping registered phones upsert");
            return;
        }
        let url = crate::phone_sync::cloud_base();
        if let Err(e) = self.directory.upsert_phones(&url, &bearer, phones).await {
            warn!(error = %e, "registered phones upsert failed");
        }
    }
}

// ── Relay mechanics ──────────────────────────────────────────────────────────

/// Result of one bounded-channel relay send.
#[derive(Debug, PartialEq, Eq)]
enum RelayOutcome {
    Sent,
    /// Dropped without blocking (video under backpressure).
    Dropped,
    /// The other side is gone or stalled past the timeout.
    Dead,
}

/// Bounded-channel send with the relay's backpressure rule: never block on
/// droppable (video binary) frames; wait up to `stall` on control frames.
async fn relay_send<T>(
    sender: &mpsc::Sender<T>,
    packet: T,
    droppable: bool,
    stall: Duration,
) -> RelayOutcome {
    match sender.try_send(packet) {
        Ok(()) => RelayOutcome::Sent,
        Err(TrySendError::Full(packet)) => {
            if droppable {
                return RelayOutcome::Dropped;
            }
            match tokio::time::timeout(stall, sender.send(packet)).await {
                Ok(Ok(())) => RelayOutcome::Sent,
                _ => RelayOutcome::Dead,
            }
        }
        Err(TrySendError::Closed(_)) => RelayOutcome::Dead,
    }
}

fn end_frame(reason: &str) -> String {
    json!({ "type": "end", "reason": reason }).to_string()
}

fn is_video(msg: &Message) -> bool {
    matches!(msg, Message::Binary(_))
}

/// Route one desktop-originated frame to the viewers of the serial that
/// currently owns the socket's output.
pub(crate) async fn fan_out_to_viewers(session: &Arc<PhoneLinkSession>, msg: Message) {
    let serial = session.streaming.read().await.clone();
    let Some(serial) = serial else {
        return;
    };
    let targets: Vec<Viewer> = session
        .viewers
        .read()
        .await
        .get(&serial)
        .cloned()
        .unwrap_or_default();
    if targets.is_empty() {
        return;
    }
    let mut dead: Vec<u64> = Vec::new();
    for viewer in &targets {
        match relay_send(
            &viewer.to_viewer,
            (msg.clone(), None),
            is_video(&msg),
            STALL_TIMEOUT,
        )
        .await
        {
            RelayOutcome::Sent => {}
            RelayOutcome::Dropped => {
                let n = session
                    .video_drops
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                if n == 1 || n % 100 == 0 {
                    debug!(serial = %serial, dropped = n, "dropping video frames: viewer backpressure");
                }
            }
            RelayOutcome::Dead => dead.push(viewer.id),
        }
    }
    if !dead.is_empty() {
        let mut viewers = session.viewers.write().await;
        if let Some(list) = viewers.get_mut(&serial) {
            list.retain(|v| !dead.contains(&v.id));
        }
    }
}

/// Byte/frame transport seam: production wraps `axum::extract::ws::WebSocket`;
/// tests substitute a `tokio::io::DuplexStream` codec (see `tests`). Errors
/// are folded into `None`/`false` — the relay treats any transport error as
/// end-of-stream.
#[async_trait::async_trait]
pub(crate) trait FrameTransport: Send {
    async fn recv(&mut self) -> Option<Message>;
    async fn send(&mut self, msg: Message) -> bool;
}

pub(crate) struct WsFrameTransport {
    pub(crate) socket: WebSocket,
}

#[async_trait::async_trait]
impl FrameTransport for WsFrameTransport {
    async fn recv(&mut self) -> Option<Message> {
        match self.socket.recv().await {
            Some(Ok(msg)) => Some(msg),
            Some(Err(e)) => {
                debug!(error = %e, "phone stream ws receive error");
                None
            }
            None => None,
        }
    }

    async fn send(&mut self, msg: Message) -> bool {
        if let Err(e) = self.socket.send(msg).await {
            debug!(error = %e, "phone stream ws send error");
            return false;
        }
        true
    }
}

/// Send the `{"type":"end","reason":…}` JSON frame, then close — with 4401
/// for token expiry (the contract's code) and 1011 for every other end.
async fn send_end<T: FrameTransport>(transport: &mut T, reason: &str, code: Option<u16>) {
    if !transport.send(Message::Text(end_frame(reason))).await {
        return;
    }
    if let Some(code) = code {
        let _ = transport
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.to_string().into(),
            })))
            .await;
    }
}

/// The phone-link session pump: runs on the desktop-attached socket from
/// hello to disconnect. `account` is resolved from the desktop credential by
/// the route handler (auth middleware).
pub(crate) async fn run_phone_link_session<T: FrameTransport>(
    mut transport: T,
    registry: Arc<PhoneLinkRegistry>,
    account: String,
) {
    // 1. The first frame must be a valid hello (bounded wait).
    let phones = match tokio::time::timeout(HELLO_TIMEOUT, transport.recv()).await {
        Ok(Some(Message::Text(text))) => match parse_phone_hello(&text) {
            Ok(phones) => phones,
            Err(reason) => {
                warn!(account = %account, reason, "rejecting phone-link session: invalid hello");
                send_end(
                    &mut transport,
                    "invalid_hello",
                    Some(axum::extract::ws::close_code::ERROR),
                )
                .await;
                return;
            }
        },
        Ok(Some(Message::Close(_))) | Ok(None) => return,
        Ok(Some(_)) => {
            send_end(
                &mut transport,
                "invalid_hello",
                Some(axum::extract::ws::close_code::ERROR),
            )
            .await;
            return;
        }
        Err(_) => {
            warn!(account = %account, "phone-link session timed out waiting for hello");
            return;
        }
    };

    // 2. Register (online) + upsert identity rows in cloud PG (best-effort).
    let (session, from_producers) = PhoneLinkSession::new(
        registry.next_session_id(),
        account.clone(),
        phones.clone(),
    );
    registry.register_session(session.clone()).await;
    info!(account = %account, phones = phones.len(), "phone-link session attached");
    registry.cloud_upsert_phones(&phones).await;

    // 3. Pump until either direction ends.
    run_session_pump(&mut transport, &session, from_producers).await;

    // 4. Mark offline, end viewers/waiters, refresh last_seen (best-effort).
    registry.unregister_session(&session).await;
    registry.cloud_upsert_phones(&phones).await;
    info!(account = %account, "phone-link session detached");
}

/// One desktop socket's select loop: heartbeat + desktop-bound channel →
/// socket; socket → viewers.
async fn run_session_pump<T: FrameTransport>(
    transport: &mut T,
    session: &Arc<PhoneLinkSession>,
    mut from_producers: mpsc::Receiver<Message>,
) {
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await; // the first tick is immediate; skip it
    let mut last_inbound = tokio::time::Instant::now();

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if !transport.send(Message::Ping(Vec::new())).await {
                    break;
                }
                if last_inbound.elapsed() > HEARTBEAT_DEAD {
                    warn!(account = %session.account, "phone-link peer silent past heartbeat deadline");
                    break;
                }
            }
            packet = from_producers.recv() => {
                match packet {
                    Some(msg) => {
                        if !transport.send(msg).await {
                            break;
                        }
                    }
                    None => break,
                }
            }
            msg = transport.recv() => {
                match msg {
                    Some(m @ (Message::Text(_) | Message::Binary(_))) => {
                        last_inbound = tokio::time::Instant::now();
                        fan_out_to_viewers(session, m).await;
                    }
                    Some(Message::Ping(ping)) => {
                        if !transport.send(Message::Pong(ping)).await {
                            break;
                        }
                    }
                    Some(Message::Pong(_)) => {
                        last_inbound = tokio::time::Instant::now();
                    }
                    Some(Message::Close(_)) => break,
                    None => break,
                }
            }
        }
    }
}

/// The browser-side viewer pump: attach (or wait), then relay both ways
/// until the socket, the session, or the token's lifetime ends.
pub(crate) async fn run_phone_stream_viewer<T: FrameTransport>(
    mut transport: T,
    registry: Arc<PhoneLinkRegistry>,
    account: String,
    serial: String,
    read_only: bool,
    ttl: Duration,
) {
    let deadline = tokio::time::Instant::now() + ttl;
    let viewer_id = registry.next_viewer_id();
    let (packet_tx, mut packet_rx) = mpsc::channel::<ViewerPacket>(VIEWER_CHANNEL_CAP);

    // 1. Attach, or buffer as a waiter until the stream is available. The
    //    attach-or-buffer decision is one atomic registry pass, so no wake
    //    can be missed; every path that wakes a waiter removes its entry.
    let session = loop {
        let (wake_tx, mut wake_rx) = mpsc::channel::<WakeSignal>(4);
        match registry
            .attach_or_wait(
                &account,
                &serial,
                packet_tx.clone(),
                viewer_id,
                wake_tx,
            )
            .await
        {
            AttachOutcome::Attached { session } => break session,
            AttachOutcome::Waiting => {
                let mut timer = Box::pin(tokio::time::sleep_until(deadline));
                loop {
                    tokio::select! {
                        _ = &mut timer => {
                            registry.unregister_waiter(&account, &serial, viewer_id).await;
                            send_end(
                                &mut transport,
                                "token_expired",
                                Some(PHONE_TOKEN_EXPIRED_CLOSE_CODE),
                            )
                            .await;
                            return;
                        }
                        signal = wake_rx.recv() => match signal {
                            // Our waiter entry was removed by the waker;
                            // loop back and re-attempt the attach.
                            Some(WakeSignal::SessionAvailable) => break,
                            Some(WakeSignal::DesktopOffline) | None => {
                                registry.unregister_waiter(&account, &serial, viewer_id).await;
                                send_end(
                                    &mut transport,
                                    "desktop_offline",
                                    Some(axum::extract::ws::close_code::ERROR),
                                )
                                .await;
                                return;
                            }
                        },
                        msg = transport.recv() => match msg {
                            None | Some(Message::Close(_)) => {
                                registry.unregister_waiter(&account, &serial, viewer_id).await;
                                return;
                            }
                            Some(Message::Ping(ping)) => {
                                let _ = transport.send(Message::Pong(ping)).await;
                            }
                            Some(_) => {} // data before attach: nowhere to go
                        }
                    }
                }
            }
        }
    };

    info!(serial = %serial, account = %account, read_only, "phone stream viewer attached");

    // 2. Attached: relay until the socket, the session, or the token ends.
    let mut timer = Box::pin(tokio::time::sleep_until(deadline));
    loop {
        tokio::select! {
            _ = &mut timer => {
                send_end(
                    &mut transport,
                    "token_expired",
                    Some(PHONE_TOKEN_EXPIRED_CLOSE_CODE),
                )
                .await;
                break;
            }
            packet = packet_rx.recv() => match packet {
                Some((msg, close)) => {
                    if !transport.send(msg).await {
                        break;
                    }
                    if let Some(code) = close {
                        let _ = transport
                            .send(Message::Close(Some(CloseFrame {
                                code,
                                reason: "stream ended".into(),
                            })))
                            .await;
                        break;
                    }
                }
                None => break,
            },
            msg = transport.recv() => match msg {
                None | Some(Message::Close(_)) => break,
                Some(Message::Ping(ping)) => {
                    let _ = transport.send(Message::Pong(ping)).await;
                }
                Some(Message::Pong(_)) => {}
                Some(data) => {
                    // Read-only enforcement at the pipe: the payload is
                    // opaque, so a read-only viewer cannot inject anything.
                    if read_only {
                        continue;
                    }
                    let droppable = is_video(&data);
                    match relay_send(&session.to_desktop, data, droppable, STALL_TIMEOUT).await {
                        RelayOutcome::Sent | RelayOutcome::Dropped => {}
                        RelayOutcome::Dead => {
                            debug!(serial = %serial, "desktop side not consuming; closing viewer");
                            break;
                        }
                    }
                }
            }
        }
    }

    registry.detach(&session, &account, &serial, viewer_id).await;
}

// ── REST routes (merged under /api/v1, Clerk/desktop-auth'd) ────────────────

pub fn phone_api_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/phones", get(list_phones))
        .route("/phones/:serial/ws-token", post(issue_phone_ws_token))
        .route("/phones/:serial/viewer-token", post(issue_phone_viewer_token))
}

#[derive(Debug, Deserialize)]
pub struct IssuePhoneWsTokenRequest {
    /// Defaults to TRUE — a full-control token injects input into a personal
    /// phone, so `false` requires the ACI confirmation grant (same pattern
    /// as the full-control VNC token in `computer_ws`).
    #[serde(default, rename = "readOnly", alias = "read_only")]
    read_only: Option<bool>,
}

/// GET /api/v1/phones — the caller's registered phones; `online` reflects
/// this api instance's attached phone-link sessions.
async fn list_phones(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let rows = match state.phone_link.cloud_list_phones().await {
        Some(rows) => rows,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "phone registry is unavailable",
            )
        }
    };
    let mut phones = Vec::with_capacity(rows.len());
    for row in rows {
        let online = state.phone_link.is_online(&user.user_id, &row.serial).await;
        let mut phone = json!({
            "serial": row.serial,
            "model": row.model.unwrap_or_default(),
            "name": row.name.unwrap_or_default(),
            "online": online,
            "callBridge": row.call_bridge,
        });
        // iPhone via the App Store app (`phone_app`): screen only, online
        // while its broadcast extension holds the ingest socket.
        if let Some(phone_id) = row.serial.strip_prefix(crate::phone_app::APP_SERIAL_PREFIX) {
            crate::phone_app::decorate_listing(&user.user_id, phone_id, &mut phone).await;
        }
        phones.push(phone);
    }
    Json(json!({ "phones": phones })).into_response()
}

/// POST /api/v1/phones/:serial/ws-token — mint the short-lived HMAC token the
/// browser presents at `/ws/phones/:serial/stream`. Requires the phone row
/// for this account AND an attached Desktop session.
async fn issue_phone_ws_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(serial): Path<String>,
    Query(approval): Query<ApprovalQuery>,
    Json(body): Json<IssuePhoneWsTokenRequest>,
) -> impl IntoResponse {
    let secret = match crate::bot_desktop_stream::desktop_ws_secret(&state) {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop ws not configured",
            )
        }
    };
    let rows = match state.phone_link.cloud_list_phones().await {
        Some(rows) => rows,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "phone registry is unavailable",
            )
        }
    };
    if !rows.iter().any(|row| row.serial == serial) {
        return error_response(StatusCode::NOT_FOUND, "phone not found");
    }
    if !state.phone_link.is_online(&user.user_id, &serial).await {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop_offline",
        );
    }
    let read_only = body.read_only.unwrap_or(true);
    if !read_only {
        let descriptor = json!({
            "serial": serial,
            "purpose": "phone_stream",
            "read_only": false,
        });
        if let Err(denial) = crate::aci_safety::enforce_confirmation(
            &state.approval_store,
            &user.user_id,
            "phones.ws_token",
            crate::aci_safety::ConfirmationClass::Risky,
            &descriptor,
            approval.approval_id.as_deref(),
        ) {
            return (denial.status, Json(denial.body)).into_response();
        }
    }
    let token = sign_phone_stream_token(
        &secret,
        &user.user_id,
        &serial,
        PHONE_STREAM_TOKEN_TTL_SECONDS,
        read_only,
    );
    Json(json!({
        "token": token,
        "expiresInSec": PHONE_STREAM_TOKEN_TTL_SECONDS,
    }))
    .into_response()
}

/// POST /api/v1/phones/:serial/viewer-token — a view-only viewer token
/// (purpose `phone_view`, 10 min) for `/ws/phones/:serial/stream`. cloud-api
/// asks for it on behalf of a Platform API key with the `phones` scope
/// (`POST /v1/phones/:serial/live`, relayed and signed as the owner) and
/// tunnels its viewer page's socket here; no ACI grant is needed because the
/// token can never carry input.
async fn issue_phone_viewer_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(serial): Path<String>,
) -> impl IntoResponse {
    let Some(secret) = crate::bot_desktop_stream::desktop_ws_secret(&state) else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "desktop ws not configured");
    };
    let Some(rows) = state.phone_link.cloud_list_phones().await else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "phone registry is unavailable");
    };
    if !rows.iter().any(|row| row.serial == serial) {
        return error_response(StatusCode::NOT_FOUND, "phone not found");
    }
    if !state.phone_link.is_online(&user.user_id, &serial).await {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "desktop_offline");
    }
    let token = sign_phone_view_token(&secret, &user.user_id, &serial, PHONE_VIEW_TOKEN_TTL_SECONDS);
    Json(json!({ "token": token, "expiresInSec": PHONE_VIEW_TOKEN_TTL_SECONDS, "readOnly": true })).into_response()
}

// ── WebSocket routes ─────────────────────────────────────────────────────────

/// `/ws/phone-link/*` — the Desktop-attached session socket. Mounted on the
/// protected router: the desktop credential resolves to the AuthUser whose
/// id is the account the phones register under.
pub fn phone_link_router() -> Router<Arc<AppState>> {
    Router::new().route("/session", get(phone_link_session_handler))
}

async fn phone_link_session_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        handle_phone_link_session(socket, state.phone_link.clone(), user.user_id)
    })
}

async fn handle_phone_link_session(
    socket: WebSocket,
    registry: Arc<PhoneLinkRegistry>,
    account: String,
) {
    run_phone_link_session(WsFrameTransport { socket }, registry, account).await;
}

/// PUBLIC `/ws/phones/*` mount (like the computer VNC route): the HMAC token
/// IS the credential; an AuthUser, when the same-origin browser carries one
/// through, must match the token's `sub`.
pub fn phone_stream_public_router() -> Router<Arc<AppState>> {
    Router::new().route("/:serial/stream", get(phone_stream_ws_handler))
}

#[derive(Debug, Deserialize)]
pub struct PhoneStreamWsQuery {
    token: String,
}

async fn phone_stream_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(serial): Path<String>,
    Query(query): Query<PhoneStreamWsQuery>,
) -> impl IntoResponse {
    let secret = match crate::bot_desktop_stream::desktop_ws_secret(&state) {
        Some(s) => s,
        None => {
            warn!("desktop ws secret not configured");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "desktop ws not configured",
            );
        }
    };
    let claims = match validate_phone_stream_token(&secret, &query.token, &serial) {
        Ok(claims) => claims,
        Err(e) => {
            warn!(error = %e, serial = %serial, "invalid phone stream token");
            return error_response(StatusCode::FORBIDDEN, "invalid token");
        }
    };
    if let Some(Extension(user)) = user {
        if user.user_id != claims.sub {
            return error_response(StatusCode::FORBIDDEN, "token mismatch");
        }
    }
    let ttl_seconds = claims.exp.saturating_sub(now_secs()).max(1);
    ws.on_upgrade(move |socket| {
        handle_phone_stream_socket(socket, state.phone_link.clone(), claims, ttl_seconds)
    })
}

async fn handle_phone_stream_socket(
    socket: WebSocket,
    registry: Arc<PhoneLinkRegistry>,
    claims: PhoneStreamClaims,
    ttl_seconds: u64,
) {
    info!(
        serial = %claims.serial,
        sub = %claims.sub,
        read_only = claims.read_only,
        "Opening phone stream WebSocket proxy"
    );
    run_phone_stream_viewer(
        WsFrameTransport { socket },
        registry,
        claims.sub,
        claims.serial,
        claims.read_only,
        Duration::from_secs(ttl_seconds),
    )
    .await;
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::response::Response;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    const TEST_SECRET: &str = "test-phone-ws-secret-for-unit-tests";

    fn handler_test_secret() {
        // Validate/issue paths read the secret from env on every call.
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

    // ── Test frame transport over a tokio duplex (the "mocked desktop") ─────

    /// Length-prefixed `Message` codec over a duplex stream: kind u8, len u32
    /// BE, payload (close carries u16 BE code + reason; 0xffff = no frame).
    async fn write_frame(stream: &mut DuplexStream, msg: &Message) -> bool {
        let mut buf = Vec::new();
        let mut push = |kind: u8, payload: &[u8]| {
            buf.push(kind);
            buf.extend((payload.len() as u32).to_be_bytes());
            buf.extend_from_slice(payload);
        };
        match msg {
            Message::Text(text) => push(1, text.as_bytes()),
            Message::Binary(bytes) => push(2, bytes),
            Message::Ping(payload) => push(9, payload),
            Message::Pong(payload) => push(10, payload),
            Message::Close(frame) => match frame {
                Some(frame) => {
                    let mut payload = frame.code.to_be_bytes().to_vec();
                    payload.extend_from_slice(frame.reason.as_bytes());
                    push(8, &payload);
                }
                None => push(8, &0xffff_u16.to_be_bytes()),
            },
        }
        stream.write_all(&buf).await.is_ok()
    }

    async fn read_frame(stream: &mut DuplexStream) -> Option<Message> {
        let mut kind = [0u8; 1];
        stream.read_exact(&mut kind).await.ok()?;
        let mut len = [0u8; 4];
        stream.read_exact(&mut len).await.ok()?;
        let len = u32::from_be_bytes(len) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await.ok()?;
        match kind[0] {
            1 => Some(Message::Text(String::from_utf8_lossy(&payload).into_owned())),
            2 => Some(Message::Binary(payload)),
            9 => Some(Message::Ping(payload)),
            10 => Some(Message::Pong(payload)),
            8 => {
                if len < 2 {
                    return Some(Message::Close(None));
                }
                let code = u16::from_be_bytes([payload[0], payload[1]]);
                if code == 0xffff {
                    Some(Message::Close(None))
                } else {
                    Some(Message::Close(Some(CloseFrame {
                        code,
                        reason: String::from_utf8_lossy(&payload[2..]).into_owned().into(),
                    })))
                }
            }
            _ => None,
        }
    }

    struct DuplexFrameTransport {
        stream: DuplexStream,
    }

    #[async_trait::async_trait]
    impl FrameTransport for DuplexFrameTransport {
        async fn recv(&mut self) -> Option<Message> {
            read_frame(&mut self.stream).await
        }

        async fn send(&mut self, msg: Message) -> bool {
            write_frame(&mut self.stream, &msg).await
        }
    }

    fn duplex_pair(size: usize) -> (DuplexFrameTransport, DuplexFrameTransport) {
        let (a, b) = tokio::io::duplex(size);
        (
            DuplexFrameTransport { stream: a },
            DuplexFrameTransport { stream: b },
        )
    }

    async fn recv_timeout(t: &mut DuplexFrameTransport) -> Message {
        tokio::time::timeout(Duration::from_secs(5), t.recv())
            .await
            .expect("frame within 5s")
            .expect("stream open")
    }

    async fn expect_silence(t: &mut DuplexFrameTransport, window: Duration) {
        assert!(
            tokio::time::timeout(window, t.recv()).await.is_err(),
            "expected no frame within {:?}",
            window
        );
    }

    fn hello_frame(serials: &[&str]) -> Message {
        Message::Text(format!(
            r#"{{"type":"hello","phones":[{}]}}"#,
            serials
                .iter()
                .map(|s| format!(r#"{{"serial":"{s}","model":"Pixel 9","name":"test phone"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        ))
    }

    fn end_reason(msg: &Message) -> String {
        let text = match msg {
            Message::Text(text) => text,
            other => panic!("expected end text frame, got {other:?}"),
        };
        serde_json::from_str::<Value>(text)
            .expect("end frame is json")
            .get("reason")
            .and_then(Value::as_str)
            .expect("end frame has a reason")
            .to_string()
    }

    #[derive(Default)]
    struct StubDirectory {
        rows: std::sync::Mutex<Vec<CloudPhone>>,
        upserts: std::sync::Mutex<Vec<Vec<PhoneHelloPhone>>>,
    }

    #[async_trait::async_trait]
    impl PhoneDirectory for StubDirectory {
        async fn list_phones(
            &self,
            _cloud_url: &str,
            _bearer: &str,
        ) -> Result<Vec<CloudPhone>, DirectoryError> {
            Ok(self.rows.lock().unwrap().clone())
        }

        async fn upsert_phones(
            &self,
            _cloud_url: &str,
            _bearer: &str,
            phones: &[PhoneHelloPhone],
        ) -> Result<(), DirectoryError> {
            self.upserts.lock().unwrap().push(phones.to_vec());
            Ok(())
        }
    }

    fn stub_directory(rows: Vec<CloudPhone>) -> Arc<StubDirectory> {
        Arc::new(StubDirectory {
            rows: std::sync::Mutex::new(rows),
            ..Default::default()
        })
    }

    // ── Token tests ─────────────────────────────────────────────────────────

    #[test]
    fn phone_token_sign_verify_round_trip() {
        let token = sign_phone_stream_token(TEST_SECRET, "user-1", "serial-1", 60, true);
        let claims = verify_phone_stream_token(TEST_SECRET, &token).unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.serial, "serial-1");
        assert_eq!(claims.purpose, "phone_stream");
        assert!(claims.read_only);
        assert!(claims.exp >= now_secs() + 59);

        let claims = validate_phone_stream_token(TEST_SECRET, &token, "serial-1").unwrap();
        assert_eq!(claims.sub, "user-1");
    }

    #[test]
    fn viewer_tokens_are_always_view_only() {
        let token = sign_phone_view_token(TEST_SECRET, "user-1", "serial-1", PHONE_VIEW_TOKEN_TTL_SECONDS);
        let claims = validate_phone_stream_token(TEST_SECRET, &token, "serial-1").unwrap();
        assert_eq!((claims.purpose.as_str(), claims.read_only), (PHONE_VIEW_PURPOSE, true));
        assert!(claims.exp >= now_secs() + PHONE_VIEW_TOKEN_TTL_SECONDS - 1);
        // A forged-looking viewer token claiming input is still read-only at the pipe.
        let writable = phone_token_with_purpose(TEST_SECRET, "user-1", "serial-1", now_secs() + 60, false, PHONE_VIEW_PURPOSE);
        assert!(validate_phone_stream_token(TEST_SECRET, &writable, "serial-1").unwrap().read_only);
        assert!(matches!(validate_phone_stream_token(TEST_SECRET, &token, "serial-2"), Err(PhoneTokenError::SerialMismatch)));
    }

    #[test]
    fn phone_token_rejects_tampering_and_wrong_scope() {
        let token = sign_phone_stream_token(TEST_SECRET, "user-1", "serial-1", 60, true);
        assert!(matches!(
            verify_phone_stream_token("other-secret", &token),
            Err(PhoneTokenError::Signature)
        ));
        assert!(matches!(
            validate_phone_stream_token(TEST_SECRET, &token, "serial-2"),
            Err(PhoneTokenError::SerialMismatch)
        ));
        let wrong_purpose = phone_token_with_exp(TEST_SECRET, "user-1", "serial-1", now_secs() + 60, true);
        // Craft a token whose purpose claim is different.
        let mut claims = verify_phone_stream_token(TEST_SECRET, &wrong_purpose).unwrap();
        claims.purpose = "vnc".to_string();
        let payload = crate::bot_desktop_stream::b64_encode(&serde_json::to_vec(&claims).unwrap());
        let header = crate::bot_desktop_stream::b64_encode(br#"{"alg":"HS256","typ":"PT"}"#);
        let input = format!("{header}.{payload}");
        let sig = crate::bot_desktop_stream::hmac_sign(TEST_SECRET, &input);
        let token = format!("{input}.{sig}");
        assert!(matches!(
            validate_phone_stream_token(TEST_SECRET, &token, "serial-1"),
            Err(PhoneTokenError::PurposeMismatch)
        ));
    }

    #[test]
    fn phone_token_expiry_and_read_only_passthrough() {
        let expired = phone_token_with_exp(TEST_SECRET, "user-1", "serial-1", now_secs().saturating_sub(10), true);
        assert!(matches!(
            verify_phone_stream_token(TEST_SECRET, &expired),
            Err(PhoneTokenError::Expired)
        ));
        let full = sign_phone_stream_token(TEST_SECRET, "user-1", "serial-1", 60, false);
        assert!(!verify_phone_stream_token(TEST_SECRET, &full).unwrap().read_only);
        let read_only = sign_phone_stream_token(TEST_SECRET, "user-1", "serial-1", 60, true);
        assert!(verify_phone_stream_token(TEST_SECRET, &read_only).unwrap().read_only);
    }

    // ── Hello parsing ───────────────────────────────────────────────────────

    #[test]
    fn hello_parsing() {
        let phones = parse_phone_hello(
            r#"{"type":"hello","phones":[{"serial":"  R5CRA1A2LBA  ","model":"Pixel 9","name":"Joe's Pixel"}]}"#,
        )
        .unwrap();
        assert_eq!(phones.len(), 1);
        assert_eq!(phones[0].serial, "R5CRA1A2LBA", "serial is trimmed");
        assert_eq!(phones[0].model.as_deref(), Some("Pixel 9"));

        assert!(parse_phone_hello(r#"{"type":"goodbye","phones":[{"serial":"a"}]}"#).is_err());
        assert!(parse_phone_hello(r#"{"phones":[{"serial":"a"}]}"#).is_err(), "type is required");
        assert!(parse_phone_hello(r#"{"type":"hello","phones":[]}"#).is_err());
        assert!(parse_phone_hello(r#"{"type":"hello","phones":[{"serial":"has space"}]}"#).is_err());
        assert!(parse_phone_hello(r#"{"type":"hello","phones":[{"serial":""}]}"#).is_err());
        assert!(
            parse_phone_hello(&format!(
                r#"{{"type":"hello","phones":[{{"serial":"{}"}}]}}"#,
                "x".repeat(129)
            ))
            .is_err()
        );
        assert!(
            parse_phone_hello(
                r#"{"type":"hello","phones":[{"serial":"a"},{"serial":"a"}]}"#
            )
            .is_err(),
            "duplicate serials are rejected"
        );
        assert!(parse_phone_hello("not json").is_err());
    }

    #[tokio::test]
    async fn call_bridge_rides_the_hello_to_the_cloud_and_back_to_the_list() {
        let long = "B".repeat(200);
        let hello = format!(
            r#"{{"type":"hello","phones":[
                {{"serial":"s1","callBridge":{{"enabled":true,"botName":"  Front desk  "}}}},
                {{"serial":"s2","callBridge":{{"enabled":"yes"}}}},
                {{"serial":"s3","callBridge":{{"enabled":false,"botName":"{long}"}}}},
                {{"serial":"s4","callBridge":null}},
                {{"serial":"s5"}}
            ]}}"#
        );
        let phones = parse_phone_hello(&hello).expect("malformed callBridge never fails the hello");
        let bridges: Vec<_> = phones.iter().map(|p| p.call_bridge.clone()).collect();
        assert_eq!(bridges[0], Some(CallBridgeSummary { enabled: true, bot_name: Some("Front desk".into()) }));
        assert_eq!(bridges[1], None, "malformed is ignored");
        assert_eq!(bridges[2].as_ref().unwrap().bot_name.as_ref().unwrap().chars().count(), MAX_CALL_BRIDGE_BOT_NAME);
        assert_eq!((bridges[3].clone(), bridges[4].clone()), (None, None));

        // The upsert payload (what reaches /registered-phones) carries it camelCase, absent when None.
        let payload = serde_json::to_value(&phones).unwrap();
        assert_eq!(payload[0]["callBridge"], json!({ "enabled": true, "botName": "Front desk" }));
        assert!(payload[4].get("callBridge").is_none());

        // The cloud's rows (either casing) come back on GET /phones; null when absent.
        handler_test_secret();
        let rows: Vec<CloudPhone> = serde_json::from_value(json!([
            { "serial": "s1", "call_bridge": { "enabled": true, "botName": "Front desk" } },
            { "serial": "s2", "callBridge": { "enabled": false } },
            { "serial": "s3", "callBridge": "garbage" },
            { "serial": "s4" }
        ]))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let directory = stub_directory(rows);
        let state = crate::test_helpers::app_state_with_phone_directory(temp.path(), directory.clone()).await;
        std::mem::forget(temp);
        state.phone_link.directory.upsert_phones("", "", &phones).await.unwrap();
        assert_eq!(directory.upserts.lock().unwrap()[0][0].call_bridge, bridges[0]);
        let response = list_phones(State(state), Extension(auth_user("user-1"))).await.into_response();
        let value: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
        let listed: Vec<Value> = value["phones"].as_array().unwrap().iter().map(|p| p["callBridge"].clone()).collect();
        assert_eq!(listed, vec![json!({ "enabled": true, "botName": "Front desk" }), json!({ "enabled": false }), Value::Null, Value::Null]);
    }

    // ── Registry behavior ───────────────────────────────────────────────────

    fn registry_with_stub() -> Arc<PhoneLinkRegistry> {
        PhoneLinkRegistry::new(Arc::new(StubDirectory::default()))
    }

    async fn test_session(
        registry: &PhoneLinkRegistry,
        account: &str,
        serials: &[&str],
    ) -> Arc<PhoneLinkSession> {
        let phones: Vec<PhoneHelloPhone> = serials
            .iter()
            .map(|serial| PhoneHelloPhone {
                serial: serial.to_string(),
                model: Some("Pixel 9".into()),
                name: None,
                call_bridge: None,
            })
            .collect();
        let (session, _rx) = PhoneLinkSession::new(registry.next_session_id(), account.to_string(), phones);
        registry.register_session(session.clone()).await;
        session
    }

    #[tokio::test]
    async fn registry_marks_online_and_offline() {
        let registry = registry_with_stub();
        assert!(!registry.is_online("user-1", "serial-1").await);
        let session = test_session(&registry, "user-1", &["serial-1"]).await;
        assert!(registry.is_online("user-1", "serial-1").await);
        registry.unregister_session(&session).await;
        assert!(!registry.is_online("user-1", "serial-1").await);
    }

    #[tokio::test]
    async fn registry_replacement_session_survives_old_unregister() {
        let registry = registry_with_stub();
        let first = test_session(&registry, "user-1", &["serial-1"]).await;
        let second = test_session(&registry, "user-1", &["serial-1"]).await;
        // The stale (reconnected) session must not take the new one down.
        registry.unregister_session(&first).await;
        assert!(registry.is_online("user-1", "serial-1").await);
        registry.unregister_session(&second).await;
        assert!(!registry.is_online("user-1", "serial-1").await);
    }

    #[tokio::test]
    async fn registry_busy_stream_waits_and_disconnect_ends_waiter() {
        let registry = registry_with_stub();
        let session = test_session(&registry, "user-1", &["serial-1", "serial-2"]).await;

        let (viewer_tx, _viewer_rx) = mpsc::channel(VIEWER_CHANNEL_CAP);
        let outcome = registry
            .attach_or_wait("user-1", "serial-1", viewer_tx.clone(), 100, mpsc::channel(4).0)
            .await;
        assert!(matches!(outcome, AttachOutcome::Attached { .. }), "free stream attaches");

        let (wake_tx, mut wake_rx) = mpsc::channel(4);
        let outcome = registry
            .attach_or_wait("user-1", "serial-2", viewer_tx, 101, wake_tx)
            .await;
        assert!(matches!(outcome, AttachOutcome::Waiting), "one active stream per socket");

        registry.unregister_session(&session).await;
        assert!(
            matches!(wake_rx.recv().await, Some(WakeSignal::DesktopOffline)),
            "the buffered waiter is told the desktop went offline"
        );
    }

    #[tokio::test]
    async fn registry_detach_frees_stream_and_promotes_waiter() {
        let registry = registry_with_stub();
        let _session = test_session(&registry, "user-1", &["serial-1", "serial-2"]).await;

        let (viewer_tx, _rx) = mpsc::channel(VIEWER_CHANNEL_CAP);
        let first = registry
            .attach_or_wait("user-1", "serial-1", viewer_tx.clone(), 100, mpsc::channel(4).0)
            .await;
        let AttachOutcome::Attached { session: attached } = first else {
            panic!("first viewer attaches");
        };

        let (wake_tx, mut wake_rx) = mpsc::channel(4);
        let waiting = registry
            .attach_or_wait("user-1", "serial-2", viewer_tx, 101, wake_tx)
            .await;
        assert!(matches!(waiting, AttachOutcome::Waiting));

        registry.detach(&attached, "user-1", "serial-1", 100).await;
        assert!(
            matches!(wake_rx.recv().await, Some(WakeSignal::SessionAvailable)),
            "freeing the stream promotes the buffered waiter"
        );
        let promoted = registry
            .attach_or_wait("user-1", "serial-2", mpsc::channel(4).0, 101, mpsc::channel(4).0)
            .await;
        assert!(matches!(promoted, AttachOutcome::Attached { .. }));
    }

    #[tokio::test]
    async fn relay_send_backpressure_drops_video_never_blocks_forever() {
        let (tx, mut rx) = mpsc::channel::<Message>(1);
        assert_eq!(
            relay_send(&tx, Message::Text("a".into()), false, Duration::from_millis(50)).await,
            RelayOutcome::Sent
        );
        // Full channel: binary is dropped without blocking.
        assert_eq!(
            relay_send(&tx, Message::Binary(vec![1, 2, 3]), true, Duration::from_millis(50)).await,
            RelayOutcome::Dropped
        );
        // Full channel: control waits, then gives up as Dead.
        assert_eq!(
            relay_send(&tx, Message::Text("b".into()), false, Duration::from_millis(50)).await,
            RelayOutcome::Dead
        );
        assert_eq!(rx.recv().await, Some(Message::Text("a".into())), "the video frame was dropped, not queued");
        drop(rx);
        assert_eq!(
            relay_send(&tx, Message::Text("c".into()), false, Duration::from_millis(50)).await,
            RelayOutcome::Dead
        );
    }

    // ── Duplex integration: the relay against a mocked desktop socket ───────

    #[tokio::test]
    async fn relay_passes_frames_both_directions_in_order() {
        let registry = registry_with_stub();
        let (desktop_transport, mut desktop) = duplex_pair(64 * 1024);
        let session_task =
            tokio::spawn(run_phone_link_session(desktop_transport, registry.clone(), "user-1".into()));
        assert!(desktop.send(hello_frame(&["serial-1"])).await);

        let (viewer_transport, mut viewer) = duplex_pair(64 * 1024);
        let viewer_task = tokio::spawn(run_phone_stream_viewer(
            viewer_transport,
            registry.clone(),
            "user-1".into(),
            "serial-1".into(),
            false,
            Duration::from_secs(60),
        ));

        // The desktop learns the viewer arrived, then receives its frames in order.
        let attached = recv_timeout(&mut desktop).await;
        let attached: Value = serde_json::from_str(match &attached {
            Message::Text(t) => t,
            other => panic!("expected viewer_attached text, got {other:?}"),
        })
        .unwrap();
        assert_eq!(attached["type"], "viewer_attached");
        assert_eq!(attached["serial"], "serial-1");

        // desktop -> viewer: binary video + JSON metadata, order preserved.
        for frame in ["frame-1", "frame-2", "frame-3"] {
            assert!(desktop.send(Message::Binary(frame.as_bytes().to_vec())).await);
        }
        assert!(desktop.send(Message::Text(r#"{"type":"device","width":1080,"height":2400}"#.into())).await);
        for frame in ["frame-1", "frame-2", "frame-3"] {
            assert_eq!(recv_timeout(&mut viewer).await, Message::Binary(frame.as_bytes().to_vec()));
        }
        let metadata = recv_timeout(&mut viewer).await;
        assert_eq!(
            metadata,
            Message::Text(r#"{"type":"device","width":1080,"height":2400}"#.into())
        );

        // viewer -> desktop: control JSON flows untouched.
        assert!(viewer.send(Message::Text(r#"{"type":"tap","x":10,"y":20}"#.into())).await);
        assert_eq!(
            recv_timeout(&mut desktop).await,
            Message::Text(r#"{"type":"tap","x":10,"y":20}"#.into())
        );

        // Teardown: desktop goes away, the viewer gets desktop_offline.
        drop(desktop);
        let end = recv_timeout(&mut viewer).await;
        assert_eq!(end_reason(&end), "desktop_offline");
        match recv_timeout(&mut viewer).await {
            Message::Close(frame) => assert_eq!(frame.unwrap().code, axum::extract::ws::close_code::ERROR),
            other => panic!("expected close after end, got {other:?}"),
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), viewer_task).await.expect("viewer ends").unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), session_task).await.expect("session ends").unwrap();
        assert!(!registry.is_online("user-1", "serial-1").await);
    }

    #[tokio::test]
    async fn read_only_viewer_receives_but_cannot_inject() {
        let registry = registry_with_stub();
        let (desktop_transport, mut desktop) = duplex_pair(64 * 1024);
        tokio::spawn(run_phone_link_session(
            desktop_transport,
            registry.clone(),
            "user-1".into(),
        ));
        assert!(desktop.send(hello_frame(&["serial-1"])).await);

        let (viewer_transport, mut viewer) = duplex_pair(64 * 1024);
        let viewer_task = tokio::spawn(run_phone_stream_viewer(
            viewer_transport,
            registry.clone(),
            "user-1".into(),
            "serial-1".into(),
            true, // read-only token
            Duration::from_secs(60),
        ));
        let attached = recv_timeout(&mut desktop).await;
        assert!(matches!(&attached, Message::Text(t) if t.contains("viewer_attached")));

        // Desktop -> viewer flows.
        assert!(desktop.send(Message::Binary(b"video".to_vec())).await);
        assert_eq!(recv_timeout(&mut viewer).await, Message::Binary(b"video".to_vec()));

        // Viewer -> desktop is dropped at the pipe (opaque payload, enforced
        // at the relay): the desktop sees nothing.
        assert!(viewer.send(Message::Text(r#"{"type":"tap","x":1,"y":1}"#.into())).await);
        expect_silence(&mut desktop, Duration::from_millis(300)).await;

        drop(viewer);
        let _ = tokio::time::timeout(Duration::from_secs(5), viewer_task).await;
    }

    #[tokio::test]
    async fn waiting_viewer_attaches_when_session_registers() {
        let registry = registry_with_stub();
        let (viewer_transport, mut viewer) = duplex_pair(64 * 1024);
        let viewer_task = tokio::spawn(run_phone_stream_viewer(
            viewer_transport,
            registry.clone(),
            "user-1".into(),
            "serial-1".into(),
            false,
            Duration::from_secs(60),
        ));

        // The desktop arrives a moment later; the buffered waiter attaches.
        let (desktop_transport, mut desktop) = duplex_pair(64 * 1024);
        let session_task = tokio::spawn(run_phone_link_session(
            desktop_transport,
            registry.clone(),
            "user-1".into(),
        ));
        assert!(desktop.send(hello_frame(&["serial-1"])).await);
        let attached = recv_timeout(&mut desktop).await;
        assert!(matches!(&attached, Message::Text(t) if t.contains("viewer_attached")));

        assert!(desktop.send(Message::Binary(b"late-video".to_vec())).await);
        assert_eq!(recv_timeout(&mut viewer).await, Message::Binary(b"late-video".to_vec()));

        drop(desktop);
        let end = recv_timeout(&mut viewer).await;
        assert_eq!(end_reason(&end), "desktop_offline");
        let _ = tokio::time::timeout(Duration::from_secs(5), viewer_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), session_task).await;
    }

    #[tokio::test]
    async fn second_serial_waits_while_socket_streams_the_first() {
        let registry = registry_with_stub();
        let (desktop_transport, mut desktop) = duplex_pair(64 * 1024);
        let session_task = tokio::spawn(run_phone_link_session(
            desktop_transport,
            registry.clone(),
            "user-1".into(),
        ));
        assert!(desktop.send(hello_frame(&["serial-1", "serial-2"])).await);

        let (viewer1_transport, mut viewer1) = duplex_pair(64 * 1024);
        let viewer1_task = tokio::spawn(run_phone_stream_viewer(
            viewer1_transport,
            registry.clone(),
            "user-1".into(),
            "serial-1".into(),
            false,
            Duration::from_secs(60),
        ));
        let attached = recv_timeout(&mut desktop).await;
        assert!(matches!(&attached, Message::Text(t) if t.contains("serial-1")));

        let (viewer2_transport, mut viewer2) = duplex_pair(64 * 1024);
        let viewer2_task = tokio::spawn(run_phone_stream_viewer(
            viewer2_transport,
            registry.clone(),
            "user-1".into(),
            "serial-2".into(),
            false,
            Duration::from_secs(60),
        ));
        // serial-2 must not steal or observe serial-1's stream.
        expect_silence(&mut desktop, Duration::from_millis(300)).await;
        assert!(desktop.send(Message::Binary(b"phone-1-video".to_vec())).await);
        assert_eq!(recv_timeout(&mut viewer1).await, Message::Binary(b"phone-1-video".to_vec()));

        // serial-1's viewer leaves: the socket frees and the waiter attaches.
        drop(viewer1);
        let _ = tokio::time::timeout(Duration::from_secs(5), viewer1_task).await;
        let detached = recv_timeout(&mut desktop).await;
        assert!(matches!(&detached, Message::Text(t) if t.contains("viewer_detached") && t.contains("serial-1")));
        let attached2 = recv_timeout(&mut desktop).await;
        assert!(matches!(&attached2, Message::Text(t) if t.contains("viewer_attached") && t.contains("serial-2")));

        assert!(desktop.send(Message::Binary(b"phone-2-video".to_vec())).await);
        assert_eq!(recv_timeout(&mut viewer2).await, Message::Binary(b"phone-2-video".to_vec()));

        drop(desktop);
        let end = recv_timeout(&mut viewer2).await;
        assert_eq!(end_reason(&end), "desktop_offline");
        let _ = tokio::time::timeout(Duration::from_secs(5), viewer2_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), session_task).await;
    }

    #[tokio::test]
    async fn invalid_hello_ends_the_session_without_registering() {
        let registry = registry_with_stub();
        let (desktop_transport, mut desktop) = duplex_pair(64 * 1024);
        let task = tokio::spawn(run_phone_link_session(
            desktop_transport,
            registry.clone(),
            "user-1".into(),
        ));
        assert!(desktop.send(Message::Text(r#"{"type":"nope"}"#.into())).await);
        let end = recv_timeout(&mut desktop).await;
        assert_eq!(end_reason(&end), "invalid_hello");
        match recv_timeout(&mut desktop).await {
            Message::Close(frame) => assert_eq!(frame.unwrap().code, axum::extract::ws::close_code::ERROR),
            other => panic!("expected close after invalid hello, got {other:?}"),
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await.expect("task ends").unwrap();
        assert!(!registry.is_online("user-1", "serial-1").await);
    }

    #[tokio::test]
    async fn expired_token_closes_4401_with_end_frame() {
        let registry = registry_with_stub();
        let (viewer_transport, mut viewer) = duplex_pair(64 * 1024);
        let task = tokio::spawn(run_phone_stream_viewer(
            viewer_transport,
            registry.clone(),
            "user-1".into(),
            "serial-1".into(),
            true,
            Duration::from_millis(0), // already expired
        ));
        let end = recv_timeout(&mut viewer).await;
        assert_eq!(end_reason(&end), "token_expired");
        match recv_timeout(&mut viewer).await {
            Message::Close(frame) => assert_eq!(frame.unwrap().code, 4401),
            other => panic!("expected 4401 close, got {other:?}"),
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await.expect("task ends").unwrap();
    }

    // ── REST route handlers ─────────────────────────────────────────────────

    #[tokio::test]
    async fn list_phones_merges_cloud_rows_with_online_flags() {
        handler_test_secret();
        let temp = tempfile::tempdir().unwrap();
        let directory = stub_directory(vec![
            CloudPhone { serial: "serial-1".into(), model: Some("Pixel 9".into()), name: Some("Pixel".into()), last_seen_at: None, call_bridge: None },
            CloudPhone { serial: "serial-2".into(), model: None, name: None, last_seen_at: None, call_bridge: None },
        ]);
        let state = crate::test_helpers::app_state_with_phone_directory(temp.path(), directory).await;
        std::mem::forget(temp);
        let _session = test_session(&state.phone_link, "user-1", &["serial-2"]).await;

        let response = list_phones(State(state), Extension(auth_user("user-1"))).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let phones = value["phones"].as_array().unwrap();
        assert_eq!(phones.len(), 2);
        assert_eq!(
            (phones[0]["serial"].as_str(), phones[0]["model"].as_str(), phones[0]["name"].as_str()),
            (Some("serial-1"), Some("Pixel 9"), Some("Pixel"))
        );
        assert!(!phones[0]["online"].as_bool().unwrap(), "no session for serial-1");
        assert!(phones[1]["online"].as_bool().unwrap(), "session attached for serial-2");
        assert_eq!(phones[1]["model"].as_str(), Some(""), "missing model serializes as the empty string");
    }

    async fn mint_token(state: Arc<AppState>, user: &str, serial: &str, read_only: Option<bool>) -> Response {
        issue_phone_ws_token(
            State(state),
            Extension(auth_user(user)),
            Path(serial.to_string()),
            Query(ApprovalQuery { approval_id: None }),
            Json(IssuePhoneWsTokenRequest { read_only }),
        )
        .await
        .into_response()
    }

    #[tokio::test]
    async fn ws_token_mints_read_only_by_default_and_requires_live_session() {
        handler_test_secret();
        let temp = tempfile::tempdir().unwrap();
        let directory = stub_directory(vec![CloudPhone {
            serial: "serial-1".into(),
            model: None,
            name: None,
            last_seen_at: None,
            call_bridge: None,
        }]);
        let state = crate::test_helpers::app_state_with_phone_directory(temp.path(), directory).await;
        std::mem::forget(temp);

        // Desktop offline → minting refused.
        let response = mint_token(state.clone(), "user-1", "serial-1", None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        // Unknown phone → 404 even with a session attached.
        let _session = test_session(&state.phone_link, "user-1", &["serial-1"]).await;
        let response = mint_token(state.clone(), "user-1", "serial-unknown", None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Happy path: read-only by default, TTL 300, right claims.
        let response = mint_token(state.clone(), "user-1", "serial-1", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["expiresInSec"], 300);
        let claims = verify_phone_stream_token(TEST_SECRET, value["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.serial, "serial-1");
        assert_eq!(claims.purpose, "phone_stream");
        assert!(claims.read_only, "phone ws tokens default to read-only");

        // A different account cannot mint for these rows.
        let response = mint_token(state.clone(), "user-2", "serial-1", None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "no session attached for user-2");
    }

    #[tokio::test]
    async fn ws_token_full_control_requires_approval_grant() {
        handler_test_secret();
        let temp = tempfile::tempdir().unwrap();
        let directory = stub_directory(vec![CloudPhone {
            serial: "serial-1".into(),
            model: None,
            name: None,
            last_seen_at: None,
            call_bridge: None,
        }]);
        let state = crate::test_helpers::app_state_with_phone_directory(temp.path(), directory).await;
        std::mem::forget(temp);
        let _session = test_session(&state.phone_link, "user-1", &["serial-1"]).await;

        // Full control without an approval → confirmation_required.
        let denied = mint_token(state.clone(), "user-1", "serial-1", Some(false)).await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(denied.into_body(), 1 << 20).await.unwrap();
        let denial: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(denial["error"], "confirmation_required");
        let approval_id = denial["approval_id"].as_str().unwrap().to_string();

        // Presenting the unapproved grant still fails…
        let pending = issue_phone_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("serial-1".to_string()),
            Query(ApprovalQuery { approval_id: Some(approval_id.clone()) }),
            Json(IssuePhoneWsTokenRequest { read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(pending.status(), StatusCode::FORBIDDEN);

        // …the human approves…
        assert!(crate::aci_approvals::GRANTS.approve(&approval_id));
        let allowed = issue_phone_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("serial-1".to_string()),
            Query(ApprovalQuery { approval_id: Some(approval_id.clone()) }),
            Json(IssuePhoneWsTokenRequest { read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(allowed.status(), StatusCode::OK);
        let body = axum::body::to_bytes(allowed.into_body(), 1 << 20).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let claims = verify_phone_stream_token(TEST_SECRET, value["token"].as_str().unwrap()).unwrap();
        assert!(!claims.read_only, "the approved grant mints a full-control token");

        // Grants are single-use.
        let reused = issue_phone_ws_token(
            State(state.clone()),
            Extension(auth_user("user-1")),
            Path("serial-1".to_string()),
            Query(ApprovalQuery { approval_id: Some(approval_id) }),
            Json(IssuePhoneWsTokenRequest { read_only: Some(false) }),
        )
        .await
        .into_response();
        assert_eq!(reused.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn stream_route_is_public_and_token_gated() {
        // Router-mount level: no Authorization header must still reach the
        // route's own ws extractor (426), not an auth middleware 401 — the
        // token query param is the credential.
        handler_test_secret();
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        std::mem::forget(temp);
        let request = axum::http::Request::builder()
            .uri("/serial-1/stream?token=not-a-real-token")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("sec-websocket-version", "13")
            .body(axum::body::Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(
            phone_stream_public_router().with_state(state),
            request,
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UPGRADE_REQUIRED,
            "request must reach the route's own ws extractor (426), not an auth middleware (401)"
        );
    }
}
