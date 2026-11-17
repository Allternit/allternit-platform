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
//!   Requests the extension answers (`device.screen.read`) go node → app as
//!   sealed text frames `{"type":"device.screen.read","id","filter"?}`; the
//!   answer is a kind 6 packet (`{"type":"device.screen.read.result","id",
//!   "result":<envelope>}`) that goes to the waiting call, not to viewers.
//! * `WS /api/v1/phones/app/control?phone=<id>` — the foreground app. Text
//!   frames only, sealed with the request construction both ways. Node → app:
//!   `{"id","method","params"}`; app → node: `{"id","result"}`,
//!   `{"id","error":{"code","message"}}` or `{"type":"state","foreground","browserOpen",
//!   "deviceAccess":{calendar,reminders,contacts,shortcuts,home}}`. Phone Use
//!   replies also carry `"type":"<method>.result"`.
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
//!   - `hid.*` through the rpc route below (Phone Use `bt-android`): the
//!     Android app sends them to the iPhone as Bluetooth keyboard/mouse reports.
//!
//! # Listing (`GET /api/v1/phones`, see [`decorate_listing`])
//!
//! App phones carry `platform` (`ios` | `android`), `route`
//! (`iphone-app-store` | `android-app`), `screen` (true: can share its
//! screen), `canTap` (Android with Accessibility on and the app connected),
//! `consent` (Android: last reported screen-share consent, `unknown` while the
//! app is not connected; iOS: null), `appOpen`, `foreground`, `browserOpen`.
//! Phone Use ([`mark_phone_use`]): a second iPhone or iPad in keyboard mode
//! carries `keyboard_for` (the iPhone it types for, `""` when not certain);
//! other app iPhones carry `routes` (`official`, plus `bt-apple` / `bt-android`
//! when an iPhone/iPad or Android phone in keyboard mode is listed for it).
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
//!   iOS access is denied. Phone Use (contract §9.1): `device.shortcuts.run` and
//!   `device.home.*` need the `shortcuts` / `home` switch the same way;
//!   `device.describe` and `device.screen.read` are reads with no switch. Their
//!   result is the phone's envelope `{ok,route,verified,detail,needs_you?}`. They
//!   wait `timeout_s` (1-120, default 30) plus 5 s, and their waiting call is kept
//!   per phone, not per socket: running a shortcut brings Shortcuts to the front,
//!   which closes the control socket, and the app sends the answer on the next
//!   one. `device.screen.read` goes to the broadcast extension's ingest socket;
//!   with no broadcast the result is `needs_you: BROADCAST_STOPPED`. Other calls
//!   still fail with `app_not_open` when their socket closes. At most 64 calls
//!   wait per phone (429 `busy`). The call key is `(account, phone)`: another account's
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
/// Phone Use calls wait `timeout_s` (1-120, default 30, contract §3.1/§9)
/// plus this margin: the phone answers at its own timeout first, so the bot
/// gets the phone's plain sentence rather than the runtime's `app_timeout`.
const PHONE_USE_DEFAULT_TIMEOUT_S: u64 = 30;
const PHONE_USE_MAX_TIMEOUT_S: u64 = 120;
const PHONE_USE_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);
/// Calls waiting for one phone at once (a stuck bot cannot grow the map).
const MAX_WAITING_PER_PHONE: usize = 64;
/// LiveScreenPacket kind 6: the broadcast extension's answer to a request on
/// the ingest socket (`device.screen.read`).
const PACKET_REPLY: u8 = 6;
const PACKET_HEADER: usize = 5;
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
    // Phone Use, `official` route (contract §9.1). describe and screen.read
    // are reads with no switch; shortcuts and home need their switch on.
    // screen.read is answered by the broadcast extension on the ingest socket.
    "device.describe",
    "device.shortcuts.run",
    "device.home.list",
    "device.home.set",
    "device.home.scene",
    "device.screen.read",
    // Phone Use `bt-android` (contract §9.2): the Android app as the iPhone's
    // Bluetooth keyboard and mouse. Desktop's guard runs before any of these.
    "hid.pair.status",
    "hid.pointer.move",
    "hid.pointer.click",
    "hid.pointer.reset",
    "hid.key",
    "hid.type",
    "hid.scroll",
];
/// Phone Use methods. Their waiting calls outlive the socket the request went
/// out on: running a shortcut brings Shortcuts to the front, which closes the
/// control socket, and the app sends the answer on its next one.
const PHONE_USE_METHODS: &[&str] = &[
    "device.describe",
    "device.shortcuts.run",
    "device.home.list",
    "device.home.set",
    "device.home.scene",
    "device.screen.read",
];
const SCREEN_READ: &str = "device.screen.read";
/// Device Access kinds the app reports in its `state` frame (`deviceAccess`).
const DEVICE_ACCESS_KINDS: &[&str] = &["calendar", "reminders", "contacts", "shortcuts", "home"];
/// `device.*` kinds that are reads with no switch.
const OPEN_DEVICE_KINDS: &[&str] = &["describe", "screen"];

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
    state: RwLock<Value>,
}

/// The broadcast extension's ingest socket, for requests it answers
/// (`device.screen.read`). Plaintext JSON; the ingest pump seals them.
struct IngestConn {
    id: u64,
    to_ext: mpsc::Sender<String>,
}

/// One rpc call waiting for its answer. Kept per phone, not per socket, so a
/// reply that arrives on the phone's next socket still reaches it.
struct Waiter {
    /// The socket the request went out on.
    conn: u64,
    /// Phone Use calls survive that socket closing; others fail at once.
    survives: bool,
    /// Dropped after this even if nobody collects it.
    deadline: tokio::time::Instant,
    reply: oneshot::Sender<Value>,
}

type PhoneKey = (String, String);

struct Hub {
    pairs: Mutex<HashMap<PhoneKey, PendingPair>>,
    controls: RwLock<HashMap<PhoneKey, Arc<ControlConn>>>,
    ingests: RwLock<HashMap<PhoneKey, Arc<IngestConn>>>,
    /// Waiting rpc calls per phone, by request id.
    waiting: Mutex<HashMap<PhoneKey, HashMap<String, Waiter>>>,
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
        ingests: RwLock::new(HashMap::new()),
        waiting: Mutex::new(HashMap::new()),
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

/// Register a waiting call for `key`. Expired waiters of this phone are
/// dropped first; `None` when the phone already has too many waiting.
async fn wait_for(key: &PhoneKey, id: &str, conn: u64, survives: bool, timeout: Duration) -> Option<oneshot::Receiver<Value>> {
    let now = tokio::time::Instant::now();
    let mut waiting = hub().waiting.lock().await;
    let calls = waiting.entry(key.clone()).or_default();
    calls.retain(|_, w| w.deadline > now && !w.reply.is_closed());
    if calls.len() >= MAX_WAITING_PER_PHONE {
        return None;
    }
    let (reply, rx) = oneshot::channel();
    calls.insert(id.to_string(), Waiter { conn, survives, deadline: now + timeout, reply });
    Some(rx)
}

/// Hand a reply (`{"id",...}`) to the call waiting for it on this phone,
/// whichever socket it came in on. False when nobody waits for that id.
async fn deliver(key: &PhoneKey, reply: Value) -> bool {
    let Some(id) = reply["id"].as_str().map(str::to_string) else { return false };
    let waiter = {
        let mut waiting = hub().waiting.lock().await;
        let Some(calls) = waiting.get_mut(key) else { return false };
        let waiter = calls.remove(&id);
        if calls.is_empty() {
            waiting.remove(key);
        }
        waiter
    };
    match waiter {
        Some(w) => w.reply.send(reply).is_ok(),
        None => false,
    }
}

async fn forget(key: &PhoneKey, id: &str) {
    let mut waiting = hub().waiting.lock().await;
    if let Some(calls) = waiting.get_mut(key) {
        calls.remove(id);
        if calls.is_empty() {
            waiting.remove(key);
        }
    }
}

/// A socket closed: its calls that cannot survive that fail now (dropping the
/// sender answers them `app_not_open`); Phone Use calls keep waiting for the
/// phone's next socket until their deadline.
async fn drop_socket_waiters(key: &PhoneKey, conn: u64) {
    let mut waiting = hub().waiting.lock().await;
    if let Some(calls) = waiting.get_mut(key) {
        calls.retain(|_, w| w.survives || w.conn != conn);
        if calls.is_empty() {
            waiting.remove(key);
        }
    }
}

/// How long the runtime waits for the phone on this call.
fn rpc_timeout(method: &str, params: &Value) -> Duration {
    if !PHONE_USE_METHODS.contains(&method) {
        return RPC_TIMEOUT;
    }
    let secs = params["timeout_s"]
        .as_u64()
        .filter(|s| (1..=PHONE_USE_MAX_TIMEOUT_S).contains(s))
        .unwrap_or(PHONE_USE_DEFAULT_TIMEOUT_S);
    Duration::from_secs(secs) + PHONE_USE_TIMEOUT_MARGIN
}

/// The contract envelope for a phone whose broadcast is not running (§5).
fn broadcast_stopped() -> Value {
    let message = "Start the screen broadcast in the Allternit app on your iPhone.";
    json!({
        "ok": false,
        "route": "official",
        "verified": false,
        "detail": { "error": "broadcast_stopped", "message": message },
        "needs_you": { "code": "BROADCAST_STOPPED", "message": message },
    })
}

/// A kind 6 LiveScreenPacket (`u8 kind | u32 BE length | JSON`) as its JSON,
/// or `None` for every other packet (video, sound, status go to viewers).
fn reply_packet(packet: &[u8]) -> Option<Value> {
    if packet.len() < PACKET_HEADER || packet[0] != PACKET_REPLY {
        return None;
    }
    let len = u32::from_be_bytes([packet[1], packet[2], packet[3], packet[4]]) as usize;
    let body = packet.get(PACKET_HEADER..PACKET_HEADER.checked_add(len)?)?;
    let value: Value = serde_json::from_slice(body).ok()?;
    value["id"].is_string().then_some(value)
}

/// The control socket's last `state` frame, as the hub keeps it.
fn control_state(value: &Value) -> Value {
    let access: serde_json::Map<String, Value> = DEVICE_ACCESS_KINDS
        .iter()
        .map(|kind| (kind.to_string(), json!(value["deviceAccess"][*kind].as_bool().unwrap_or(false))))
        .collect();
    json!({
        "foreground": value["foreground"].as_bool().unwrap_or(false),
        "browserOpen": value["browserOpen"].as_bool().unwrap_or(false),
        // Android: Accessibility on, screen-share consent.
        "canTap": value["canTap"].as_bool().unwrap_or(false),
        "consent": value["consent"].as_str().filter(|c| matches!(*c, "granted" | "needed" | "denied")),
        "deviceAccess": access,
        // Phone Use: this phone as a Bluetooth keyboard and mouse for an iPhone
        // (iOS keyboard mode `bt-apple`, Android `bt-android`).
        "keyboard": keyboard_frame(&value["keyboard"]),
    })
}

/// Longest name or serial kept from a `keyboard` state (a Bluetooth name is at most 248 bytes).
const KEYBOARD_TEXT_MAX: usize = 256;

/// The `keyboard` part of a `state` frame as the hub keeps it: `{on, connected,
/// paired, keyboard_for?, host?}`, or null when the app sent none (closed or older).
/// iOS sends `keyboard_for` (the serial of the iPhone the person picked); Android
/// sends `host` (the Bluetooth name of the iPhone it is paired with).
fn keyboard_frame(k: &Value) -> Value {
    if !k.is_object() {
        return Value::Null;
    }
    let on = k["on"].as_bool() == Some(true);
    let connected = on && k["connected"].as_bool() == Some(true);
    let text = |key: &str| {
        k[key].as_str().map(str::trim).filter(|s| on && !s.is_empty() && s.len() <= KEYBOARD_TEXT_MAX).map(|s| json!(s)).unwrap_or(Value::Null)
    };
    json!({
        "on": on,
        "connected": connected,
        // iOS never knows "paired" apart from "connected" (a host subscribes only after bonding).
        "paired": connected || (on && k["paired"].as_bool() == Some(true)),
        "keyboard_for": text("keyboard_for"),
        "host": text("host"),
    })
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
        // Device Access switches from the app's last `state` frame (Phone Use
        // health on the Desktop); null while the app is not connected.
        "deviceAccess": control_state.and_then(|s| s.get("deviceAccess").cloned()).unwrap_or(Value::Null),
        // Keyboard mode from the app's state frame (Phone Use `bt-apple` on iOS,
        // `bt-android` on Android): this phone is a Bluetooth keyboard and mouse
        // for an iPhone. Read by [`mark_phone_use`] and dropped there.
        "keyboard": control_state.map(|s| keyboard_frame(&s["keyboard"])).unwrap_or(Value::Null),
    })
}

fn route_row(route: &str, healthy: bool, needs_you: Option<&str>) -> Value {
    let mut row = json!({ "route": route, "healthy": healthy });
    if let Some(code) = needs_you {
        row["needs_you"] = json!(code);
    }
    row
}

/// Phone Use fields on the whole `GET /api/v1/phones` list, after
/// [`decorate_listing`] ran on each row (contract §1, §3.1, §9.3):
/// - each phone in keyboard mode is matched to the iPhone it types for: on iOS
///   the person's pick (`keyboard_for`, a serial) when that iPhone is listed; on
///   Android the iPhone whose name is the connected host's Bluetooth name; else
///   the only other app iPhone there is. A Bluetooth peripheral is never told
///   its host's name, so with several iPhones and no pick it stays unmatched;
/// - a second iPhone or iPad in keyboard mode gets `keyboard_for`: that
///   iPhone's name, or `""` when unmatched (an Android phone stays a phone and
///   gets no `keyboard_for`);
/// - every other app iPhone gets `routes` with what the node itself can see:
///   `official` (the app open), `bt-apple` and `bt-android` (a keyboard matched
///   to it, or an unmatched one that could be). Desktop adds the Mac routes
///   (mirror, bt-mac, usb-agent) itself (`phones.phoneRoutes`); web and the PWA
///   show what the node lists.
pub fn mark_phone_use(phones: &mut [Value]) {
    let is_app = |p: &Value, platform: &str| {
        p["serial"].as_str().is_some_and(|s| s.starts_with(APP_SERIAL_PREFIX)) && p["platform"].as_str() == Some(platform)
    };
    let is_keyboard = |p: &Value| p["keyboard"]["on"].as_bool() == Some(true);
    let targets: Vec<usize> = (0..phones.len()).filter(|&i| is_app(&phones[i], "ios") && !is_keyboard(&phones[i])).collect();
    let only = (targets.len() == 1).then(|| targets[0]);
    // (route, keyboard row, matched iPhone row)
    let mut keyboards: Vec<(&str, usize, Option<usize>)> = Vec::new();
    for i in 0..phones.len() {
        if !is_keyboard(&phones[i]) {
            continue;
        }
        let k = &phones[i]["keyboard"];
        if is_app(&phones[i], "ios") {
            let picked = k["keyboard_for"].as_str().and_then(|serial| targets.iter().copied().find(|&t| phones[t]["serial"].as_str() == Some(serial)));
            keyboards.push(("bt-apple", i, picked.or(only)));
        } else if is_app(&phones[i], "android") {
            let named = k["host"].as_str().and_then(|host| {
                let mut same = targets.iter().copied().filter(|&t| same_device_name(phones[t]["name"].as_str().unwrap_or_default(), host));
                let first = same.next();
                if same.next().is_some() { None } else { first }
            });
            keyboards.push(("bt-android", i, named.or(only)));
        }
    }
    for &(route, k, target) in &keyboards {
        if route == "bt-apple" {
            let name = target.map(|t| phones[t]["name"].as_str().unwrap_or_default().to_string()).unwrap_or_default();
            phones[k]["keyboard_for"] = json!(name);
        }
    }
    for &t in &targets {
        let mut routes = vec![if phones[t]["appOpen"].as_bool() == Some(true) {
            route_row("official", true, None)
        } else {
            route_row("official", false, Some("APP_NOT_FOREGROUND"))
        }];
        let live = phones[t]["online"].as_bool() == Some(true);
        for route in ["bt-apple", "bt-android"] {
            let mine: Vec<usize> = keyboards.iter().filter(|(r, _, to)| *r == route && *to == Some(t)).map(|&(_, k, _)| k).collect();
            let unmatched = keyboards.iter().any(|(r, _, to)| *r == route && to.is_none());
            if mine.is_empty() {
                if unmatched {
                    // A keyboard is on but which iPhone it types for is not known: not set up.
                    routes.push(route_row(route, false, None));
                }
                continue;
            }
            let connected = mine.iter().filter(|&&k| phones[k]["keyboard"]["connected"].as_bool() == Some(true)).count();
            routes.push(if connected > 1 {
                // Two keyboards connected to one iPhone: the sender is not certain.
                route_row(route, false, None)
            } else if connected == 0 {
                route_row(route, false, Some("BT_NOT_PAIRED"))
            } else if !live {
                route_row(route, false, Some("BROADCAST_STOPPED"))
            } else {
                route_row(route, true, None)
            });
        }
        phones[t]["routes"] = Value::Array(routes);
    }
    for p in phones.iter_mut() {
        if let Some(obj) = p.as_object_mut() {
            obj.remove("keyboard");
        }
    }
}

/// Same phone name, ignoring case, spaces at the ends and curly apostrophes
/// ("Sam’s iPhone" in Bluetooth, "Sam's iPhone" in the app).
fn same_device_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().replace(['\u{2019}', '\u{2018}'], "'").to_lowercase();
    !a.trim().is_empty() && norm(a) == norm(b)
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
    // Requests the extension answers itself (device.screen.read), sealed
    // below: the extension never takes a request from a plain frame.
    let (to_ext, mut requests) = mpsc::channel::<String>(8);
    let ingest = Arc::new(IngestConn { id: hub().next_id.fetch_add(1, Ordering::Relaxed), to_ext });
    hub().ingests.write().await.insert(phone_key.clone(), ingest.clone());
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
            request = requests.recv() => {
                let Some(plain) = request else { break };
                let sealed = {
                    let mut guard = replay.lock().unwrap_or_else(|p| p.into_inner());
                    stored.key.seal(&plain, now_secs(), &mut guard)
                };
                if !transport.send(Message::Text(sealed)).await {
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
                            // A reply (kind 6) goes to its waiting call, never to viewers.
                            if let Some(reply) = reply_packet(&packet) {
                                if !deliver(&phone_key, reply).await {
                                    debug!(phone = %phone_id, "broadcast reply with no waiting call");
                                }
                                continue;
                            }
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

    {
        let mut ingests = hub().ingests.write().await;
        if ingests.get(&phone_key).is_some_and(|c| c.id == ingest.id) {
            ingests.remove(&phone_key);
        }
    }
    drop_socket_waiters(&phone_key, ingest.id).await;
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
                        *conn.state.write().await = control_state(&value);
                    } else if value["id"].is_string() {
                        // Any socket of this phone: a reply may come in on the
                        // socket after the one its request went out on.
                        if !deliver(&key, value).await {
                            debug!(phone = %phone_id, "app phone reply with no waiting call");
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
    // Calls that cannot outlive this socket answer app_not_open now.
    drop_socket_waiters(&key, conn.id).await;
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
    if PHONE_USE_METHODS.contains(&method)
        && params.get("timeout_s").is_some_and(|t| !t.as_u64().is_some_and(|s| (1..=PHONE_USE_MAX_TIMEOUT_S).contains(&s)))
    {
        return Err("timeout_s must be a whole number of seconds, 1-120");
    }
    let text = |field: &str, max: usize| params[field].as_str().is_some_and(|v| !v.trim().is_empty() && v.len() <= max);
    match method {
        "device.shortcuts.run" if !text("name", 200) => Err("device.shortcuts.run needs a name (up to 200 characters)"),
        "device.home.set" if !text("device_id", 200) || !params["state"].is_object() => Err("device.home.set needs device_id and a state object"),
        "device.home.scene" if !text("scene_id", 200) => Err("device.home.scene needs a scene_id"),
        "device.home.list" if !(params["kind"].is_null() || params["kind"].is_string()) => Err("kind must be a string"),
        SCREEN_READ if !(params["filter"].is_null() || params["filter"].is_object()) => Err("filter must be an object"),
        _ => Ok(()),
    }
}

/// Device Access gate on the server path: the app reports which types the
/// person switched on (and iOS allows) in its `state` frame; a device.* call
/// for a type that is not explicitly on never goes to the phone.
fn device_gate(state: &Value, method: &str) -> Result<(), &'static str> {
    let Some(rest) = method.strip_prefix("device.") else { return Ok(()) };
    let kind = rest.split('.').next().unwrap_or("");
    // What the phone can do, and the screen: reads with no switch.
    if OPEN_DEVICE_KINDS.contains(&kind) {
        return Ok(());
    }
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
    let key = (user.user_id.clone(), phone_id);
    let timeout = rpc_timeout(&body.method, &params);
    if body.method == SCREEN_READ {
        return screen_read(&key, &params, timeout).await;
    }
    let conn = hub().controls.read().await.get(&key).cloned();
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
    let survives = PHONE_USE_METHODS.contains(&body.method.as_str());
    let Some(rx) = wait_for(&key, &id, conn.id, survives, timeout).await else {
        return err(StatusCode::TOO_MANY_REQUESTS, "busy", "Too many calls are waiting for this iPhone");
    };
    let request = json!({ "id": id, "method": body.method, "params": params }).to_string();
    if tokio::time::timeout(Duration::from_secs(5), conn.to_app.send(request))
        .await
        .map_or(true, |sent| sent.is_err())
    {
        forget(&key, &id).await;
        return not_open();
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(answer)) => answer_response(answer),
        Ok(Err(_)) => not_open(),
        Err(_) => {
            forget(&key, &id).await;
            err(StatusCode::GATEWAY_TIMEOUT, "app_timeout", "The iPhone app did not answer in time")
        }
    }
}

fn answer_response(answer: Value) -> Response {
    match answer.get("error").filter(|e| !e.is_null()) {
        Some(error) => Json(json!({ "error": error })).into_response(),
        None => Json(json!({ "result": answer.get("result").cloned().unwrap_or(Value::Null) })).into_response(),
    }
}

/// `device.screen.read` goes to the broadcast extension (it holds the frames),
/// sealed like every request; its kind 6 packet carries the answer. No
/// broadcast, or it ends before answering: `BROADCAST_STOPPED`.
async fn screen_read(key: &PhoneKey, params: &Value, timeout: Duration) -> Response {
    let stopped = || Json(json!({ "result": broadcast_stopped() })).into_response();
    let Some(ingest) = hub().ingests.read().await.get(key).cloned() else { return stopped() };
    let id = uuid::Uuid::new_v4().to_string();
    let Some(rx) = wait_for(key, &id, ingest.id, false, timeout).await else {
        return err(StatusCode::TOO_MANY_REQUESTS, "busy", "Too many calls are waiting for this iPhone");
    };
    let mut request = json!({ "type": SCREEN_READ, "id": id });
    if params["filter"].is_object() {
        request["filter"] = params["filter"].clone();
    }
    if tokio::time::timeout(Duration::from_secs(5), ingest.to_ext.send(request.to_string()))
        .await
        .map_or(true, |sent| sent.is_err())
    {
        forget(key, &id).await;
        return stopped();
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(answer)) => answer_response(answer),
        Ok(Err(_)) => stopped(),
        Err(_) => {
            forget(key, &id).await;
            err(StatusCode::GATEWAY_TIMEOUT, "app_timeout", "The iPhone broadcast did not answer in time")
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
    fn listing_marks_the_keyboard_device_and_lists_routes_for_the_iphone() {
        let row = |serial: &str, name: &str, online: bool, state: Option<Value>| {
            let mut p = json!({ "serial": serial, "name": name, "online": online });
            let fields = listing_fields(AppPlatform::Ios, state.as_ref());
            for (k, v) in fields.as_object().unwrap() {
                p[k] = v.clone();
            }
            p
        };
        let mut phones = vec![
            row("app:main", "Sam's iPhone", true, Some(json!({ "foreground": true }))),
            row("app:pad", "Sam's iPad", false, Some(json!({ "keyboard": { "on": true, "connected": true } }))),
            json!({ "serial": "adb-1", "name": "Pixel", "online": true }),
        ];
        mark_phone_use(&mut phones);
        assert_eq!(phones[1]["keyboard_for"], "Sam's iPhone");
        assert!(phones[1].get("routes").is_none(), "a keyboard is not another phone");
        assert!(phones[0].get("keyboard_for").is_none());
        assert_eq!(phones[0]["routes"], json!([{ "route": "official", "healthy": true }, { "route": "bt-apple", "healthy": true }]));
        assert!(phones.iter().all(|p| p.get("keyboard").is_none()), "the raw state stays inside the node");
        assert!(phones[2].get("routes").is_none());

        // Keyboard on but no iPhone connected to it, and the app closed.
        let mut waiting = vec![
            row("app:main", "Sam's iPhone", true, None),
            row("app:pad", "Sam's iPad", false, Some(json!({ "keyboard": { "on": true, "connected": false } }))),
        ];
        mark_phone_use(&mut waiting);
        assert_eq!(
            waiting[0]["routes"],
            json!([{ "route": "official", "healthy": false, "needs_you": "APP_NOT_FOREGROUND" }, { "route": "bt-apple", "healthy": false, "needs_you": "BT_NOT_PAIRED" }])
        );

        // Two iPhones besides the keyboard: the keyboard's iPhone is not known.
        let mut two = vec![
            row("app:a", "A", true, Some(json!({}))),
            row("app:b", "B", true, Some(json!({}))),
            row("app:pad", "Pad", false, Some(json!({ "keyboard": { "on": true, "connected": true } }))),
        ];
        mark_phone_use(&mut two);
        assert_eq!(two[2]["keyboard_for"], "");
        assert_eq!(two[0]["routes"][1], json!({ "route": "bt-apple", "healthy": false }));

        // No keyboard device: only the official route.
        let mut alone = vec![row("app:main", "Sam's iPhone", true, Some(json!({})))];
        mark_phone_use(&mut alone);
        assert_eq!(alone[0]["routes"], json!([{ "route": "official", "healthy": true }]));
    }

    /// Regression: the hub kept only known state fields, so `keyboard` never
    /// reached the listing and no keyboard device or Bluetooth route showed.
    #[test]
    fn the_state_frame_keeps_keyboard_mode() {
        let ios = control_state(&json!({ "type": "state", "foreground": true,
            "keyboard": { "on": true, "connected": true, "keyboard_for": " app:main ", "extra": 1 } }));
        assert_eq!(ios["keyboard"], json!({ "on": true, "connected": true, "paired": true, "keyboard_for": "app:main", "host": null }));
        let android = control_state(&json!({ "type": "state", "keyboard": { "on": true, "paired": true, "connected": false, "host": "Sam’s iPhone" } }));
        assert_eq!(android["keyboard"], json!({ "on": true, "connected": false, "paired": true, "keyboard_for": null, "host": "Sam’s iPhone" }));
        // Off: nothing about a host is kept.
        let off = control_state(&json!({ "type": "state", "keyboard": { "on": false, "connected": true, "host": "x" } }));
        assert_eq!(off["keyboard"], json!({ "on": false, "connected": false, "paired": false, "keyboard_for": null, "host": null }));
        assert!(control_state(&json!({ "type": "state" }))["keyboard"].is_null());
        assert_eq!(listing_fields(AppPlatform::Android, Some(&android))["keyboard"]["host"], "Sam’s iPhone");
    }

    #[test]
    fn the_keyboard_follows_the_iphone_the_person_picked() {
        let row = |serial: &str, name: &str, platform: AppPlatform, online: bool, frame: Value| {
            let mut p = json!({ "serial": serial, "name": name, "online": online });
            let state = control_state(&frame);
            for (k, v) in listing_fields(platform, Some(&state)).as_object().unwrap() {
                p[k] = v.clone();
            }
            p
        };
        let ios = AppPlatform::Ios;
        let mut phones = vec![
            row("app:a", "Work iPhone", ios, true, json!({})),
            row("app:b", "Sam's iPhone", ios, true, json!({})),
            row("app:pad", "Sam's iPad", ios, false, json!({ "keyboard": { "on": true, "connected": true, "keyboard_for": "app:b" } })),
        ];
        mark_phone_use(&mut phones);
        assert_eq!(phones[2]["keyboard_for"], "Sam's iPhone");
        assert_eq!(phones[1]["routes"][1], json!({ "route": "bt-apple", "healthy": true }));
        assert_eq!(phones[0]["routes"], json!([{ "route": "official", "healthy": true }]), "the other iPhone has no keyboard");

        // A pick for an iPhone no longer listed, with two iPhones: not certain.
        let mut stale = vec![
            row("app:a", "A", ios, true, json!({})),
            row("app:b", "B", ios, true, json!({})),
            row("app:pad", "Pad", ios, false, json!({ "keyboard": { "on": true, "connected": true, "keyboard_for": "app:gone" } })),
        ];
        mark_phone_use(&mut stale);
        assert_eq!(stale[2]["keyboard_for"], "");
        assert_eq!(stale[0]["routes"][1], json!({ "route": "bt-apple", "healthy": false }));
    }

    #[test]
    fn an_android_keyboard_lists_bt_android_on_its_iphone() {
        let row = |serial: &str, name: &str, platform: AppPlatform, online: bool, frame: Option<Value>| {
            let mut p = json!({ "serial": serial, "name": name, "online": online });
            let state = frame.map(|f| control_state(&f));
            for (k, v) in listing_fields(platform, state.as_ref()).as_object().unwrap() {
                p[k] = v.clone();
            }
            p
        };
        let (ios, android) = (AppPlatform::Ios, AppPlatform::Android);
        let pixel = |connected: bool, host: &str| json!({ "keyboard": { "on": true, "paired": true, "connected": connected, "host": host } });

        // Two iPhones: the host's Bluetooth name picks one (curly apostrophe in Bluetooth).
        let mut phones = vec![
            row("app:a", "Work iPhone", ios, true, Some(json!({}))),
            row("app:b", "Sam's iPhone", ios, true, Some(json!({}))),
            row("app:px", "Pixel", android, true, Some(pixel(true, "Sam’s iPhone"))),
        ];
        mark_phone_use(&mut phones);
        assert_eq!(phones[1]["routes"], json!([{ "route": "official", "healthy": true }, { "route": "bt-android", "healthy": true }]));
        assert_eq!(phones[0]["routes"], json!([{ "route": "official", "healthy": true }]));
        assert!(phones[2].get("keyboard_for").is_none(), "an Android phone stays a phone");
        assert!(phones[2].get("routes").is_none());
        assert!(phones.iter().all(|p| p.get("keyboard").is_none()));

        // One iPhone whose app name differs from its Bluetooth name; broadcast off.
        let mut one = vec![row("app:b", "iPhone", ios, false, Some(json!({}))), row("app:px", "Pixel", android, true, Some(pixel(true, "Sam’s iPhone")))];
        mark_phone_use(&mut one);
        assert_eq!(one[0]["routes"][1], json!({ "route": "bt-android", "healthy": false, "needs_you": "BROADCAST_STOPPED" }));

        // On but no iPhone connected yet.
        let mut waiting = vec![row("app:b", "iPhone", ios, true, Some(json!({}))), row("app:px", "Pixel", android, true, Some(pixel(false, "")))];
        mark_phone_use(&mut waiting);
        assert_eq!(waiting[0]["routes"][1], json!({ "route": "bt-android", "healthy": false, "needs_you": "BT_NOT_PAIRED" }));

        // Two iPhones and a host name that matches neither: not set up on both.
        let mut unknown = vec![
            row("app:a", "A", ios, true, Some(json!({}))),
            row("app:b", "B", ios, true, Some(json!({}))),
            row("app:px", "Pixel", android, true, Some(pixel(true, "C"))),
        ];
        mark_phone_use(&mut unknown);
        assert_eq!(unknown[0]["routes"][1], json!({ "route": "bt-android", "healthy": false }));
        assert_eq!(unknown[1]["routes"][1], json!({ "route": "bt-android", "healthy": false }));

        // The Android app closed: no keyboard state, no route.
        let mut closed = vec![row("app:b", "iPhone", ios, true, Some(json!({}))), row("app:px", "Pixel", android, false, None)];
        mark_phone_use(&mut closed);
        assert_eq!(closed[0]["routes"], json!([{ "route": "official", "healthy": true }]));
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

    // ── Phone Use (official route) ───────────────────────────────────────────

    async fn paired_phone(state: &AppState, account: &str, secret: &EnvelopeKey) -> String {
        state
            .db
            .connect()
            .unwrap()
            .execute("INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?1 || '@test.local')", [account])
            .unwrap();
        let phone = uuid::Uuid::new_v4().to_string();
        assert!(store_phone(state, &user(account), &phone, secret, &None, &None, AppPlatform::Ios).await);
        phone
    }

    /// The control socket id for this phone once it differs from `old`.
    async fn control_id(key: &PhoneKey, old: Option<u64>) -> u64 {
        for _ in 0..200 {
            if let Some(conn) = hub().controls.read().await.get(key) {
                if Some(conn.id) != old {
                    return conn.id;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("control socket never attached");
    }

    /// Regression: waiting calls lived on the control socket, so a shortcut
    /// that brought Shortcuts to the front (closing the socket) lost its
    /// answer, which the app sends on its next socket.
    #[tokio::test]
    async fn a_phone_use_reply_on_the_next_control_socket_reaches_the_waiting_call() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let secret = EnvelopeKey::from_hex(&"ef".repeat(32)).unwrap();
        let phone = paired_phone(&state, "user-late", &secret).await;
        let key = ("user-late".to_string(), phone.clone());

        let ((first_in, _first_out), first) = chan_pair();
        tokio::spawn(run_control(first, state.clone(), "user-late".to_string(), phone.clone()));
        let first_id = control_id(&key, None).await;
        let shortcut = wait_for(&key, "run-1", first_id, true, Duration::from_secs(10)).await.unwrap();
        let web = wait_for(&key, "web-1", first_id, false, Duration::from_secs(10)).await.unwrap();

        // Shortcuts comes to the front: the socket closes. A web call fails now.
        drop(first_in);
        assert!(tokio::time::timeout(Duration::from_secs(2), web).await.unwrap().is_err());

        // The app is back on a new socket and sends the late answer there.
        let ((second_in, _second_out), second) = chan_pair();
        tokio::spawn(run_control(second, state.clone(), "user-late".to_string(), phone.clone()));
        control_id(&key, Some(first_id)).await;
        let reply = json!({
            "id": "run-1",
            "type": "device.shortcuts.run.result",
            "result": { "ok": true, "route": "official", "verified": true, "detail": { "output": "done", "returned_via": "x_callback" } },
        })
        .to_string();
        let mut app_replay = ReplayGuard::default();
        second_in.send(Message::Text(secret.seal(&reply, now_secs(), &mut app_replay))).await.unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(2), shortcut).await.unwrap().unwrap();
        assert_eq!(answer_response(answer).status(), StatusCode::OK);
        drop(second_in);
    }

    #[tokio::test]
    async fn waiting_calls_are_bounded_per_phone_and_expire() {
        let key = ("user-bound".to_string(), uuid::Uuid::new_v4().to_string());
        let mut held = Vec::new();
        for i in 0..MAX_WAITING_PER_PHONE {
            held.push(wait_for(&key, &format!("c{i}"), 1, true, Duration::from_secs(60)).await.unwrap());
        }
        assert!(wait_for(&key, "one-too-many", 1, true, Duration::from_secs(60)).await.is_none());
        // Calls whose caller went away are dropped on the next insert.
        drop(held);
        let expired = wait_for(&key, "short", 1, true, Duration::ZERO).await.unwrap();
        let _later = wait_for(&key, "later", 1, true, Duration::from_secs(60)).await.unwrap();
        assert!(expired.await.is_err(), "an expired call is dropped, not kept forever");
        assert!(!deliver(&key, json!({ "id": "short" })).await);
        assert!(deliver(&key, json!({ "id": "later" })).await);
    }

    #[test]
    fn phone_use_calls_wait_their_timeout_s_plus_a_margin() {
        let secs = |method: &str, params: Value| rpc_timeout(method, &params).as_secs();
        assert_eq!(secs("device.shortcuts.run", json!({ "name": "x" })), 35);
        assert_eq!(secs("device.shortcuts.run", json!({ "name": "x", "timeout_s": 90 })), 95);
        assert_eq!(secs("device.shortcuts.run", json!({ "name": "x", "timeout_s": 120 })), 125);
        assert_eq!(secs("device.screen.read", json!({})), 35);
        assert_eq!(rpc_timeout("web.snapshot", &json!({ "timeout_s": 90 })), RPC_TIMEOUT);
        for bad in [json!(0), json!(121), json!("5"), json!(1.5), json!(-1)] {
            assert!(validate_rpc("device.shortcuts.run", &json!({ "name": "x", "timeout_s": bad })).is_err(), "{bad}");
        }
        assert!(validate_rpc("device.shortcuts.run", &json!({})).is_err());
        assert!(validate_rpc("device.home.set", &json!({ "device_id": "a", "state": { "on": true } })).is_ok());
        assert!(validate_rpc("device.home.set", &json!({ "id": "a", "state": { "on": true } })).is_err());
        assert!(validate_rpc("device.home.scene", &json!({ "scene_id": "s" })).is_ok());
        assert!(validate_rpc("device.home.list", &json!({ "kind": "lock" })).is_ok());
        assert!(validate_rpc("device.describe", &json!({})).is_ok());
        assert!(validate_rpc("device.screen.read", &json!({ "filter": { "label": "Send" } })).is_ok());
        assert!(validate_rpc("device.screen.read", &json!({ "filter": "Send" })).is_err());
        assert!(validate_rpc("device.actions.run", &json!({})).is_err(), "not answered by the iPhone app yet");
    }

    /// Regression: the state frame kept only calendar, reminders and contacts,
    /// so every shortcut and Home call was refused as device_access_off.
    #[test]
    fn the_state_frame_keeps_the_phone_use_switches() {
        let frame = json!({ "type": "state", "foreground": true,
            "deviceAccess": { "calendar": false, "contacts": true, "shortcuts": true, "home": false, "health": true } });
        let state = control_state(&frame);
        assert!(device_gate(&state, "device.shortcuts.run").is_ok());
        assert!(device_gate(&state, "device.contacts.search").is_ok());
        assert_eq!(device_gate(&state, "device.home.set"), Err("device_access_off"));
        assert_eq!(device_gate(&state, "device.home.list"), Err("device_access_off"));
        assert_eq!(device_gate(&state, "device.calendar.list"), Err("device_access_off"));
        assert!(state["deviceAccess"].get("health").is_none(), "only known switches are kept");
        // What the phone can do, and the screen, need no switch.
        assert!(device_gate(&json!({}), "device.describe").is_ok());
        assert!(device_gate(&json!({}), "device.screen.read").is_ok());
        // The Desktop reads the switches from the listing (Phone Use health).
        assert_eq!(listing_fields(AppPlatform::Ios, Some(&state))["deviceAccess"]["shortcuts"], true);
        assert!(listing_fields(AppPlatform::Ios, None)["deviceAccess"].is_null());
    }

    fn reply_packet_bytes(body: Value) -> Vec<u8> {
        let mut body = body.to_string();
        // The test seals text, so keep the length bytes ASCII.
        while (body.len() as u32).to_be_bytes().iter().any(|b| *b >= 0x80) {
            body.push(' ');
        }
        let mut packet = vec![PACKET_REPLY];
        packet.extend((body.len() as u32).to_be_bytes());
        packet.extend(body.as_bytes());
        packet
    }

    #[tokio::test]
    async fn screen_read_goes_sealed_to_the_broadcast_and_its_reply_packet_answers_it() {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let secret = EnvelopeKey::from_hex(&"a1".repeat(32)).unwrap();
        let phone = paired_phone(&state, "user-read", &secret).await;
        let key = ("user-read".to_string(), phone.clone());

        // No broadcast: the phone has to start it.
        let resp = screen_read(&key, &json!({}), Duration::from_secs(1)).await;
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["result"]["needs_you"]["code"], "BROADCAST_STOPPED");

        let ((ingest_in, mut ingest_out), ingest) = chan_pair();
        tokio::spawn(run_ingest(ingest, state.clone(), "user-read".to_string(), phone.clone()));
        for _ in 0..200 {
            if hub().ingests.read().await.contains_key(&key) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let call_key = key.clone();
        let call = tokio::spawn(async move { screen_read(&call_key, &json!({ "filter": { "label": "Send" } }), Duration::from_secs(5)).await });

        // The request reaches the extension sealed (it ignores plain requests).
        let request = loop {
            let Message::Text(text) = tokio::time::timeout(Duration::from_secs(2), ingest_out.recv()).await.unwrap().unwrap() else { continue };
            if let Ok(plain) = secret.open_text(&text, now_secs(), &mut ReplayGuard::default()) {
                break serde_json::from_str::<Value>(&plain).unwrap();
            }
        };
        assert_eq!(request["type"], SCREEN_READ);
        assert_eq!(request["filter"]["label"], "Send");
        let id = request["id"].as_str().unwrap().to_string();

        // Its kind 6 packet answers the call.
        let packet = reply_packet_bytes(json!({ "type": "device.screen.read.result", "id": id,
            "result": { "ok": true, "route": "official", "verified": true, "detail": { "text": "Send" } } }));
        assert!(reply_packet(&packet).is_some());
        let sealed = secret.seal(std::str::from_utf8(&packet).unwrap(), now_secs(), &mut ReplayGuard::default());
        ingest_in.send(Message::Binary(sealed.into_bytes())).await.unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(2), call).await.unwrap().unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["result"]["detail"]["text"], "Send");

        // Video and status packets are not replies.
        assert!(reply_packet(&[2, 0, 0, 0, 1, 0]).is_none());
        assert!(reply_packet(&[PACKET_REPLY, 0, 0, 0, 9, b'{']).is_none());
        drop(ingest_in);
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
