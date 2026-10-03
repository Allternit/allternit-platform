//! Runtime side of phone calls (migration V215). Spec: the FROZEN call contract
//! in `HANDOFF-realtime-voice-2026-10-02.md` §4.1.
//!
//! The voice worker never talks to this runtime directly: cloud-api relays a
//! call's start, its events and its bot turns here, signed with the runtime's
//! device token. Four routes (all `/api/v1/voice/calls*`, public to the Clerk
//! middleware because they authenticate themselves, see [`RelayedVoiceAuth`]):
//!
//! * `POST   /api/v1/voice/calls`                  start a call, resolve its thread
//! * `POST   /api/v1/voice/calls/{callId}/events`  write `call.*` events to the thread
//! * `POST   /api/v1/voice/calls/{callId}/turn`    one bot turn, streamed as SSE
//! * `DELETE /api/v1/voice/calls/{callId}/turn`    barge-in: abort the turn
//!
//! Auth: `x-allternit-runtime-sig: v1=<hex HMAC-SHA256(device_token,
//! "<ts>.<METHOD>.<path>.<hex sha256(body)>")>`, `x-allternit-runtime-ts` (unix
//! seconds, ±300 s) and `x-allternit-owner`. Unsigned requests are never
//! accepted. allternit-api cannot read its own device token today (see
//! [`VoiceRelaySecret`]), so until that is wired the routes answer 503 to
//! signed requests and 401 to unsigned ones.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use hmac::{Hmac, Mac};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::warn;

use crate::db::DbHandle;
use crate::gateway_runner::led;
use crate::AppState;

pub const SIG_HEADER: &str = "x-allternit-runtime-sig";
pub const TS_HEADER: &str = "x-allternit-runtime-ts";
pub const OWNER_HEADER: &str = "x-allternit-owner";
/// Allowed clock skew between cloud-api and this runtime, either direction.
pub const MAX_SKEW_SECS: i64 = 300;
const MAX_BODY: usize = 1024 * 1024;
const MAX_EVENTS: usize = 200;
const MAX_TURN_CHARS: usize = 4000;

/// Kept server-side so a phone bot answers the way a person talks on the phone.
const SPOKEN_PREFACE: &str = "[Live phone call. Answer the way you would speak aloud: one to three short sentences, plain words, no markdown, lists or emoji. If you need a tool, use it, then say the result briefly.]";

// ---------------------------------------------------------------- relay secret

/// What this runtime signs/verifies relayed voice requests with: the device
/// token cloud-api issued when this runtime paired, and the owner it paired as.
///
/// **Gap:** allternit-api does not hold its own device token. Callers present
/// `allternit_runtime_…` tokens to it (`connector_routes::verify_runtime_device_token`
/// introspects them against cloud-api), but the runtime's own copy lives in
/// gizzi's environment (`ALLTERNIT_API_TOKEN`) and cloud-api's `runtime_devices`
/// table. Until a pairing step hands the token to this process, the production
/// impl ([`UnconfiguredRelaySecret`]) returns `None` and the routes answer 503.
pub trait VoiceRelaySecret: Send + Sync {
    fn device_token(&self) -> Option<String>;
    fn paired_owner(&self) -> Option<String>;
}

pub struct UnconfiguredRelaySecret;

impl VoiceRelaySecret for UnconfiguredRelaySecret {
    fn device_token(&self) -> Option<String> {
        None
    }
    fn paired_owner(&self) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------- signature

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// Missing or malformed signature headers, bad signature, stale ts, wrong owner.
    Unauthorized(&'static str),
    /// A signature was presented but this runtime has no token to check it with.
    NotConfigured,
}

pub fn body_sha256_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// The hex signature for `(ts, method, path, body)`. Cloud-api computes the same.
pub fn sign_relay(device_token: &str, ts: i64, method: &str, path: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(device_token.as_bytes()).expect("hmac takes any key length");
    mac.update(format!("{ts}.{method}.{path}.{}", body_sha256_hex(body)).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify one relayed request; returns the owner it acts for. The compare is
/// constant-time (`Mac::verify_slice`).
pub fn verify_relay(
    secret: &dyn VoiceRelaySecret,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
    now: i64,
) -> Result<String, AuthError> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    let sig = header(SIG_HEADER).ok_or(AuthError::Unauthorized("missing signature"))?;
    let ts = header(TS_HEADER).ok_or(AuthError::Unauthorized("missing timestamp"))?;
    let owner = header(OWNER_HEADER).ok_or(AuthError::Unauthorized("missing owner"))?;
    let sig_hex = sig.strip_prefix("v1=").ok_or(AuthError::Unauthorized("unsupported signature version"))?;
    let sig_bytes = hex::decode(sig_hex).map_err(|_| AuthError::Unauthorized("malformed signature"))?;
    let ts: i64 = ts.parse().map_err(|_| AuthError::Unauthorized("malformed timestamp"))?;
    let (Some(token), Some(paired)) = (secret.device_token(), secret.paired_owner()) else {
        return Err(AuthError::NotConfigured);
    };
    if (now - ts).abs() > MAX_SKEW_SECS {
        return Err(AuthError::Unauthorized("stale timestamp"));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("hmac takes any key length");
    mac.update(format!("{ts}.{method}.{path}.{}", body_sha256_hex(body)).as_bytes());
    mac.verify_slice(&sig_bytes).map_err(|_| AuthError::Unauthorized("bad signature"))?;
    if owner != paired {
        return Err(AuthError::Unauthorized("owner does not match this runtime"));
    }
    Ok(owner.to_string())
}

fn fail(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Extractor for the four voice routes: reads the raw body, verifies the relay
/// signature over it, and yields the owner plus the verified bytes.
pub struct RelayedVoiceAuth {
    pub owner: String,
    pub body: bytes::Bytes,
}

impl RelayedVoiceAuth {
    fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, Response> {
        serde_json::from_slice(&self.body).map_err(|e| fail(StatusCode::BAD_REQUEST, &format!("invalid body: {e}")))
    }
}

#[async_trait]
impl<S: Send + Sync> FromRequest<S> for RelayedVoiceAuth {
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let deps = parts.extensions.get::<Arc<VoiceDeps>>().cloned().ok_or_else(|| fail(StatusCode::SERVICE_UNAVAILABLE, "voice relay not configured"))?;
        let body = axum::body::to_bytes(Body::new(body), MAX_BODY).await.map_err(|_| fail(StatusCode::PAYLOAD_TOO_LARGE, "body too large"))?;
        let path = parts.extensions.get::<axum::extract::OriginalUri>().map(|u| u.0.path().to_string()).unwrap_or_else(|| parts.uri.path().to_string());
        match verify_relay(deps.secret.as_ref(), &parts.headers, parts.method.as_str(), &path, &body, unix_now()) {
            Ok(owner) => Ok(Self { owner, body }),
            Err(AuthError::Unauthorized(why)) => Err(fail(StatusCode::UNAUTHORIZED, why)),
            Err(AuthError::NotConfigured) => Err(fail(StatusCode::SERVICE_UNAVAILABLE, "voice relay not configured")),
        }
    }
}

// ---------------------------------------------------------------- seams

/// Resolves the thread and session for a caller on a number. Production uses
/// `channel_phone::resolve_thread_async`; tests plug in a fake `ThreadRuntime`.
#[async_trait]
pub trait ThreadResolver: Send + Sync {
    async fn resolve(&self, db: &DbHandle, number_id: &str, caller_e164: &str) -> Result<(String, String), String>;
}

struct GizziResolver;

#[async_trait]
impl ThreadResolver for GizziResolver {
    async fn resolve(&self, db: &DbHandle, number_id: &str, caller_e164: &str) -> Result<(String, String), String> {
        let rt = crate::thread_routes::GizziRuntime { db: db.clone() };
        crate::channel_phone::resolve_thread_async(db, &rt, number_id, caller_e164).await
    }
}

/// Runs one bot turn in a call's session. `events` receives `tool` events as
/// they happen; the returned reply becomes the `text.delta`s.
#[async_trait]
pub trait VoiceTurner: Send + Sync {
    async fn run(&self, db: &DbHandle, session_id: &str, bot_id: &str, text: &str, events: mpsc::UnboundedSender<Value>) -> Result<String, String>;
    async fn abort(&self, session_id: &str);
}

/// The channel path: `send_bot_turn` goes through `gateway_runner::run_turn`
/// for vendor bots and gizzi otherwise, so tools, memory and approvals behave
/// exactly as they do for a Slack or SMS message. It reports only the final
/// reply, so no `tool` events are produced yet.
struct ChannelTurner;

#[async_trait]
impl VoiceTurner for ChannelTurner {
    async fn run(&self, db: &DbHandle, session_id: &str, bot_id: &str, text: &str, _events: mpsc::UnboundedSender<Value>) -> Result<String, String> {
        crate::agent_session_routes::send_bot_turn(db, session_id, bot_id, text).await
    }

    /// What the Stop button does: a vendor-bound session cancels on the vendor,
    /// everything else aborts in gizzi.
    async fn abort(&self, session_id: &str) {
        if crate::gateway_runner::intercept_abort(session_id).await.is_some() {
            return;
        }
        let client = crate::agent_session_routes::gizzi_client(&HeaderMap::new());
        let url = format!("{}/v1/session/{}/abort", crate::agent_session_routes::gizzi_base(), urlencoding::encode(session_id));
        if let Err(e) = client.post(url).json(&json!({})).send().await {
            warn!("voice turn abort failed: {e}");
        }
    }
}

struct InFlight {
    generation: u64,
    session_id: String,
    task: tokio::task::AbortHandle,
    out: mpsc::UnboundedSender<Value>,
}

pub struct VoiceDeps {
    pub secret: Box<dyn VoiceRelaySecret>,
    pub resolver: Box<dyn ThreadResolver>,
    pub turner: Arc<dyn VoiceTurner>,
    turns: Mutex<HashMap<String, InFlight>>,
    next_generation: AtomicU64,
}

impl VoiceDeps {
    pub fn new(secret: Box<dyn VoiceRelaySecret>, resolver: Box<dyn ThreadResolver>, turner: Arc<dyn VoiceTurner>) -> Self {
        Self { secret, resolver, turner, turns: Mutex::new(HashMap::new()), next_generation: AtomicU64::new(1) }
    }

    pub fn production() -> Self {
        Self::new(Box::new(UnconfiguredRelaySecret), Box::new(GizziResolver), Arc::new(ChannelTurner))
    }

    /// Stop the call's running turn, if any (optionally only a given generation).
    /// Tells the client its stream ended aborted, cancels the task, then aborts
    /// the turn in the session. Returns whether a turn was running.
    async fn abort_turn(&self, call_id: &str, only_generation: Option<u64>) -> bool {
        let running = {
            let mut turns = self.turns.lock().unwrap();
            match turns.get(call_id) {
                Some(t) if only_generation.map_or(true, |g| g == t.generation) => turns.remove(call_id),
                _ => None,
            }
        };
        let Some(t) = running else { return false };
        let _ = t.out.send(json!({ "type": "done", "aborted": true }));
        t.task.abort();
        let turner = self.turner.clone();
        let session = t.session_id;
        // The remote abort can be slow; the caller (barge-in) never waits on it for long.
        let _ = tokio::time::timeout(Duration::from_secs(3), turner.abort(&session)).await;
        true
    }
}

// ---------------------------------------------------------------- calls

#[derive(Debug, Clone)]
struct CallRow {
    owner_id: String,
    bot_id: String,
    thread_id: String,
    session_id: String,
    from_e164: String,
    to_e164: String,
    direction: String,
    state: String,
}

fn call_row(db: &DbHandle, call_id: &str) -> Result<Option<CallRow>, String> {
    db.connect()
        .map_err(|e| e.to_string())?
        .query_row(
            "SELECT owner_id, bot_id, thread_id, session_id, from_e164, to_e164, direction, state FROM voice_calls WHERE call_id = ?1",
            params![call_id],
            |r| Ok(CallRow { owner_id: r.get(0)?, bot_id: r.get(1)?, thread_id: r.get(2)?, session_id: r.get(3)?, from_e164: r.get(4)?, to_e164: r.get(5)?, direction: r.get(6)?, state: r.get(7)? }),
        )
        .optional()
        .map_err(|e| e.to_string())
}

/// The call, only if it belongs to `owner`. Someone else's call is a 404, not a 403.
fn owned_call(db: &DbHandle, call_id: &str, owner: &str) -> Result<CallRow, Response> {
    match call_row(db, call_id) {
        Ok(Some(c)) if c.owner_id == owner => Ok(c),
        Ok(_) => Err(fail(StatusCode::NOT_FOUND, "unknown call")),
        Err(e) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR, &e)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    call_id: String,
    bot_id: String,
    owner_id: String,
    number_id: String,
    from: String,
    to: String,
    direction: String,
    room: String,
    started_at: String,
}

async fn create_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, auth: RelayedVoiceAuth) -> Response {
    let b: CreateBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.call_id.is_empty() || b.call_id.len() > 128 || b.room.is_empty() || b.started_at.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "callId, room and startedAt are required");
    }
    if b.owner_id != auth.owner {
        return fail(StatusCode::FORBIDDEN, "ownerId does not match the signed owner");
    }
    if b.direction != "inbound" && b.direction != "outbound" {
        return fail(StatusCode::BAD_REQUEST, "direction must be inbound or outbound");
    }
    if !crate::channel_phone::is_e164(&b.from) || !crate::channel_phone::is_e164(&b.to) {
        return fail(StatusCode::BAD_REQUEST, "from and to must be E.164");
    }
    // A repeated callId (cloud-api retries while a computer wakes) returns the same ids.
    match call_row(&state.db, &b.call_id) {
        Ok(Some(c)) if c.owner_id == auth.owner => return Json(json!({ "threadId": c.thread_id, "sessionId": c.session_id })).into_response(),
        Ok(Some(_)) => return fail(StatusCode::CONFLICT, "callId belongs to another owner"),
        Ok(None) => {}
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
    let Some(number) = crate::channel_phone::number(&state.db, &b.number_id) else {
        return fail(StatusCode::NOT_FOUND, "that phone number isn't set up on this runtime");
    };
    if number.owner != auth.owner || number.bot_id != b.bot_id {
        return fail(StatusCode::FORBIDDEN, "that number does not belong to this owner and bot");
    }
    let caller = if b.direction == "inbound" { &b.from } else { &b.to };
    let (thread_id, session_id) = match deps.resolver.resolve(&state.db, &b.number_id, caller).await {
        Ok(ids) => ids,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };
    let inserted = state.db.connect().and_then(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO voice_calls (call_id, owner_id, bot_id, number_id, thread_id, session_id, from_e164, to_e164, direction, room, state, started_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'active',?11)",
            params![b.call_id, b.owner_id, b.bot_id, b.number_id, thread_id, session_id, b.from, b.to, b.direction, b.room, b.started_at],
        )
    });
    if let Err(e) = inserted {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    // Lost an insert race with a concurrent retry: answer with whatever won.
    match call_row(&state.db, &b.call_id) {
        Ok(Some(c)) if c.owner_id == auth.owner => Json(json!({ "threadId": c.thread_id, "sessionId": c.session_id })).into_response(),
        _ => fail(StatusCode::INTERNAL_SERVER_ERROR, "call was not stored"),
    }
}

// ---------------------------------------------------------------- events

#[derive(Deserialize)]
struct EventsBody {
    events: Vec<EventIn>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventIn {
    r#type: String,
    n: u64,
    #[serde(default)]
    payload: Value,
    occurred_at: Option<String>,
}

fn actor_for<'a>(call: &'a CallRow, ev: &'a EventIn, owner: &'a str) -> (&'static str, String) {
    let caller = if call.direction == "inbound" { &call.from_e164 } else { &call.to_e164 };
    match ev.payload["speaker"].as_str() {
        Some("caller") => ("caller", caller.clone()),
        Some("human") => ("human", ev.payload["by"].as_str().filter(|s| !s.is_empty()).unwrap_or(owner).to_string()),
        _ => ("bot", call.bot_id.clone()),
    }
}

async fn events_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, Path(call_id): Path<String>, auth: RelayedVoiceAuth) -> Response {
    let call = match owned_call(&state.db, &call_id, &auth.owner) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let body: EventsBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body.events.len() > MAX_EVENTS {
        return fail(StatusCode::BAD_REQUEST, "too many events in one request");
    }
    if body.events.iter().any(|e| !e.r#type.starts_with("call.") || e.r#type.len() > 64) {
        return fail(StatusCode::BAD_REQUEST, "event types must be call.*");
    }
    let (mut written, mut duplicates, mut ignored) = (0, 0, 0);
    for ev in body.events {
        // Interim partials belong to the live stream, not the ledger.
        if ev.r#type == "call.transcript.delta" && ev.payload["final"].as_bool() != Some(true) {
            ignored += 1;
            continue;
        }
        let mut payload = if ev.payload.is_object() { ev.payload.clone() } else { json!({}) };
        payload["callId"] = json!(call_id);
        payload["n"] = json!(ev.n);
        if let Some(at) = &ev.occurred_at {
            payload["occurredAt"] = json!(at);
        }
        let (kind, id) = actor_for(&call, &ev, &auth.owner);
        let key = format!("call:{call_id}:{}:{}", ev.r#type, ev.n);
        if led(&state.db, &call.bot_id, &call.thread_id, Some(&call.session_id), &ev.r#type, (kind, &id), payload, Some(key)) {
            written += 1;
        } else {
            duplicates += 1;
        }
        if ev.r#type == "call.ended" {
            let at = ev.occurred_at.clone().unwrap_or_else(|| crate::agent_gateway_routes::now());
            let _ = state.db.connect().and_then(|c| c.execute("UPDATE voice_calls SET state='ended', ended_at=COALESCE(ended_at, ?1) WHERE call_id=?2", params![at, call_id]));
            // A call that ended with a turn still running has nobody to speak to.
            deps.abort_turn(&call_id, None).await;
        }
    }
    Json(json!({ "written": written, "duplicates": duplicates, "ignored": ignored })).into_response()
}

// ---------------------------------------------------------------- turn

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnBody {
    text: String,
    #[allow(dead_code)]
    segment_id: Option<String>,
}

/// Sentence-sized pieces (whitespace kept) so a TTS can start on the first one.
fn speakable_chunks(reply: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = reply.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?' | '\n') && chars.peek().map_or(true, |n| n.is_whitespace()) {
            while let Some(n) = chars.peek().copied().filter(|n| n.is_whitespace()) {
                cur.push(n);
                chars.next();
            }
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Aborts the turn when the SSE response is dropped (the relay hung up) while
/// that turn is still the call's current one.
struct StreamGuard {
    deps: Arc<VoiceDeps>,
    call_id: String,
    generation: u64,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        let (deps, call_id, generation) = (self.deps.clone(), self.call_id.clone(), self.generation);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                deps.abort_turn(&call_id, Some(generation)).await;
            });
        }
    }
}

async fn turn_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, Path(call_id): Path<String>, auth: RelayedVoiceAuth) -> Response {
    let call = match owned_call(&state.db, &call_id, &auth.owner) {
        Ok(c) => c,
        Err(r) => return r,
    };
    if call.state != "active" {
        return fail(StatusCode::CONFLICT, "the call has ended");
    }
    let body: TurnBody = match auth.json() {
        Ok(b) => b,
        Err(r) => return r,
    };
    let text = body.text.trim().to_string();
    if text.is_empty() || text.chars().count() > MAX_TURN_CHARS {
        return fail(StatusCode::BAD_REQUEST, "text must be 1 to 4000 characters");
    }
    // One in-flight turn per call: a new one aborts the old one first.
    deps.abort_turn(&call_id, None).await;

    let (tx, rx) = mpsc::unbounded_channel::<Value>();
    let generation = deps.next_generation.fetch_add(1, Ordering::Relaxed);
    let task = {
        let (deps, db, tx, call, call_id) = (deps.clone(), state.db.clone(), tx.clone(), call.clone(), call_id.clone());
        let prompt = format!("{SPOKEN_PREFACE}\n\n{text}");
        tokio::spawn(async move {
            let result = deps.turner.run(&db, &call.session_id, &call.bot_id, &prompt, tx.clone()).await;
            match result {
                Ok(reply) => {
                    for chunk in speakable_chunks(&reply) {
                        let _ = tx.send(json!({ "type": "text.delta", "text": chunk }));
                    }
                    let _ = tx.send(json!({ "type": "done" }));
                }
                Err(message) => {
                    let _ = tx.send(json!({ "type": "error", "message": message }));
                }
            }
            let mut turns = deps.turns.lock().unwrap();
            if turns.get(&call_id).map_or(false, |t| t.generation == generation) {
                turns.remove(&call_id);
            }
        })
    };
    deps.turns.lock().unwrap().insert(call_id.clone(), InFlight { generation, session_id: call.session_id.clone(), task: task.abort_handle(), out: tx });

    let guard = StreamGuard { deps, call_id, generation };
    let stream = futures::stream::unfold((rx, guard, false), |(mut rx, guard, finished)| async move {
        if finished {
            return None;
        }
        let v = rx.recv().await?;
        let last = matches!(v["type"].as_str(), Some("done") | Some("error"));
        Some((Ok::<_, Infallible>(Event::default().data(v.to_string())), (rx, guard, last)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

async fn abort_h(State(state): State<Arc<AppState>>, Extension(deps): Extension<Arc<VoiceDeps>>, Path(call_id): Path<String>, auth: RelayedVoiceAuth) -> Response {
    if let Err(r) = owned_call(&state.db, &call_id, &auth.owner) {
        return r;
    }
    deps.abort_turn(&call_id, None).await;
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------- router

pub fn voice_calls_router() -> Router<Arc<AppState>> {
    voice_calls_router_with(Arc::new(VoiceDeps::production()))
}

pub fn voice_calls_router_with(deps: Arc<VoiceDeps>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/voice/calls", post(create_h))
        .route("/api/v1/voice/calls/:call_id/events", post(events_h))
        .route("/api/v1/voice/calls/:call_id/turn", post(turn_h).delete(abort_h))
        .layer(Extension(deps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TOKEN: &str = "allternit_runtime_test_token";
    const OWNER: &str = "user-a";

    struct Secret(Option<(&'static str, &'static str)>);
    impl VoiceRelaySecret for Secret {
        fn device_token(&self) -> Option<String> {
            self.0.map(|s| s.0.to_string())
        }
        fn paired_owner(&self) -> Option<String> {
            self.0.map(|s| s.1.to_string())
        }
    }

    struct FakeResolver;
    #[async_trait]
    impl ThreadResolver for FakeResolver {
        async fn resolve(&self, _db: &DbHandle, number_id: &str, caller: &str) -> Result<(String, String), String> {
            Ok((format!("thr-{number_id}-{caller}"), format!("sess-{number_id}-{caller}")))
        }
    }

    #[derive(Default)]
    struct FakeTurner {
        reply: Mutex<Option<Result<String, String>>>,
        /// Block until aborted (task cancelled).
        hang: Mutex<bool>,
        prompts: Mutex<Vec<(String, String)>>,
        aborted: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl VoiceTurner for FakeTurner {
        async fn run(&self, _db: &DbHandle, session_id: &str, _bot: &str, text: &str, events: mpsc::UnboundedSender<Value>) -> Result<String, String> {
            self.prompts.lock().unwrap().push((session_id.into(), text.into()));
            let _ = events.send(json!({ "type": "tool", "name": "calendar", "status": "started" }));
            if *self.hang.lock().unwrap() {
                futures::future::pending::<()>().await;
            }
            let _ = events.send(json!({ "type": "tool", "name": "calendar", "status": "done" }));
            self.reply.lock().unwrap().clone().unwrap_or_else(|| Ok("Sure. I can do that. Anything else?".into()))
        }
        async fn abort(&self, session_id: &str) {
            self.aborted.lock().unwrap().push(session_id.into());
        }
    }

    fn now_ts() -> i64 {
        unix_now()
    }

    fn headers(sig: &str, ts: i64, owner: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(SIG_HEADER, sig.parse().unwrap());
        h.insert(TS_HEADER, ts.to_string().parse().unwrap());
        h.insert(OWNER_HEADER, owner.parse().unwrap());
        h
    }

    fn signed(token: &str, ts: i64, owner: &str, method: &str, path: &str, body: &[u8]) -> HeaderMap {
        headers(&format!("v1={}", sign_relay(token, ts, method, path, body)), ts, owner)
    }

    // ---------------------------------------------------------- verifier

    #[test]
    fn verifier_accepts_a_good_signature_and_returns_the_owner() {
        let (secret, now, body) = (Secret(Some((TOKEN, OWNER))), now_ts(), br#"{"a":1}"#);
        let h = signed(TOKEN, now, OWNER, "POST", "/api/v1/voice/calls", body);
        assert_eq!(verify_relay(&secret, &h, "POST", "/api/v1/voice/calls", body, now), Ok(OWNER.to_string()));
    }

    #[test]
    fn verifier_rejects_bad_stale_future_wrong_owner_and_tampered() {
        let (secret, now, body) = (Secret(Some((TOKEN, OWNER))), now_ts(), br#"{"a":1}"#);
        let path = "/api/v1/voice/calls";
        let bad = |r: Result<String, AuthError>| matches!(r, Err(AuthError::Unauthorized(_)));
        // wrong key
        assert!(bad(verify_relay(&secret, &signed("other", now, OWNER, "POST", path, body), "POST", path, body, now)));
        // stale and future timestamps (validly signed for their own ts)
        assert!(bad(verify_relay(&secret, &signed(TOKEN, now - 301, OWNER, "POST", path, body), "POST", path, body, now)));
        assert!(bad(verify_relay(&secret, &signed(TOKEN, now + 301, OWNER, "POST", path, body), "POST", path, body, now)));
        assert!(verify_relay(&secret, &signed(TOKEN, now - 299, OWNER, "POST", path, body), "POST", path, body, now).is_ok());
        // wrong owner header, validly signed
        assert!(bad(verify_relay(&secret, &signed(TOKEN, now, "user-b", "POST", path, body), "POST", path, body, now)));
        // body, method and path tamper
        let h = signed(TOKEN, now, OWNER, "POST", path, body);
        assert!(bad(verify_relay(&secret, &h, "POST", path, br#"{"a":2}"#, now)));
        assert!(bad(verify_relay(&secret, &h, "DELETE", path, body, now)));
        assert!(bad(verify_relay(&secret, &h, "POST", "/api/v1/voice/calls/x/turn", body, now)));
        // missing headers, wrong version, non-hex, bad ts
        assert!(bad(verify_relay(&secret, &HeaderMap::new(), "POST", path, body, now)));
        let sig = sign_relay(TOKEN, now, "POST", path, body);
        assert!(bad(verify_relay(&secret, &headers(&format!("v2={sig}"), now, OWNER), "POST", path, body, now)));
        assert!(bad(verify_relay(&secret, &headers("v1=zz", now, OWNER), "POST", path, body, now)));
        let mut h = headers(&format!("v1={sig}"), now, OWNER);
        h.insert(TS_HEADER, "soon".parse().unwrap());
        assert!(bad(verify_relay(&secret, &h, "POST", path, body, now)));
    }

    #[test]
    fn verifier_without_a_device_token_never_accepts() {
        let (now, body) = (now_ts(), b"{}");
        let h = signed(TOKEN, now, OWNER, "POST", "/p", body);
        assert_eq!(verify_relay(&Secret(None), &h, "POST", "/p", body, now), Err(AuthError::NotConfigured));
        // Unsigned stays a 401-class error even when unconfigured.
        assert!(matches!(verify_relay(&Secret(None), &HeaderMap::new(), "POST", "/p", body, now), Err(AuthError::Unauthorized(_))));
    }

    // ---------------------------------------------------------- routes

    struct H {
        app: Router,
        state: Arc<AppState>,
        turner: Arc<FakeTurner>,
    }

    async fn setup(tag: &str, secret: Option<(&'static str, &'static str)>) -> H {
        let dir = std::env::temp_dir().join(format!("allternit-voice-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        state.db.connect().unwrap().execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1','user-a','b','m','p',1,'{}')", []).unwrap();
        crate::channel_phone::upsert_number(&state.db, "num-1", OWNER, "bot-1", "+14155550100", None).unwrap();
        let turner = Arc::new(FakeTurner::default());
        let deps = Arc::new(VoiceDeps::new(Box::new(Secret(secret)), Box::new(FakeResolver), turner.clone()));
        let app = voice_calls_router_with(deps).with_state(state.clone());
        H { app, state, turner }
    }

    async fn send(app: &Router, method: &str, path: &str, body: Value, sign: bool) -> (StatusCode, String) {
        let bytes = if body.is_null() { Vec::new() } else { serde_json::to_vec(&body).unwrap() };
        let mut req = Request::builder().method(method).uri(path).header("content-type", "application/json");
        if sign {
            for (k, v) in signed(TOKEN, now_ts(), OWNER, method, path, &bytes).iter() {
                req = req.header(k, v);
            }
        }
        let resp = app.clone().oneshot(req.body(Body::from(bytes)).unwrap()).await.unwrap();
        let status = resp.status();
        (status, String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).to_string())
    }

    fn call_body(id: &str) -> Value {
        json!({ "callId": id, "botId": "bot-1", "ownerId": OWNER, "numberId": "num-1", "from": "+14155550123", "to": "+14155550100", "direction": "inbound", "room": "call-1", "startedAt": "2026-10-03T10:00:00Z" })
    }

    const CALLS: &str = "/api/v1/voice/calls";

    #[tokio::test]
    async fn unsigned_is_401_and_unconfigured_signed_is_503() {
        let h = setup("auth", Some((TOKEN, OWNER))).await;
        assert_eq!(send(&h.app, "POST", CALLS, call_body("c1"), false).await.0, StatusCode::UNAUTHORIZED);
        let u = setup("unconf", None).await;
        assert_eq!(send(&u.app, "POST", CALLS, call_body("c1"), false).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(send(&u.app, "POST", CALLS, call_body("c1"), true).await.0, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn create_is_idempotent_and_checks_ownership() {
        let h = setup("create", Some((TOKEN, OWNER))).await;
        let (s1, b1) = send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        assert_eq!(s1, StatusCode::OK, "{b1}");
        let v1: Value = serde_json::from_str(&b1).unwrap();
        assert_eq!(v1["threadId"], "thr-num-1-+14155550123");
        assert_eq!(v1["sessionId"], "sess-num-1-+14155550123");
        let (s2, b2) = send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        assert_eq!((s2, serde_json::from_str::<Value>(&b2).unwrap()), (StatusCode::OK, v1));
        let n: i64 = h.state.db.connect().unwrap().query_row("SELECT COUNT(*) FROM voice_calls", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // Outbound resolves the thread by the number dialed.
        let mut out = call_body("c2");
        (out["direction"], out["from"], out["to"]) = (json!("outbound"), json!("+14155550100"), json!("+14155550199"));
        let v: Value = serde_json::from_str(&send(&h.app, "POST", CALLS, out, true).await.1).unwrap();
        assert_eq!(v["threadId"], "thr-num-1-+14155550199");
        // Validation.
        let mut bad = call_body("c3");
        bad["ownerId"] = json!("user-b");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::FORBIDDEN);
        let mut bad = call_body("c3");
        bad["numberId"] = json!("nope");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::NOT_FOUND);
        let mut bad = call_body("c3");
        bad["botId"] = json!("bot-2");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::FORBIDDEN);
        let mut bad = call_body("c3");
        bad["from"] = json!("4155550123");
        assert_eq!(send(&h.app, "POST", CALLS, bad, true).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(send(&h.app, "POST", CALLS, json!({ "nope": 1 }), true).await.0, StatusCode::BAD_REQUEST);
    }

    fn ledger(h: &H, call: &str) -> Vec<(String, String, String, Value)> {
        let conn = h.state.db.connect().unwrap();
        let mut stmt = conn.prepare("SELECT event_type, actor_type, actor_id, payload FROM bot_events WHERE thread_id = ?1 AND idempotency_key LIKE ?2 ORDER BY rowid").unwrap();
        stmt.query_map(params!["thr-num-1-+14155550123", format!("call:{call}:%")], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap()))).unwrap().map(Result::unwrap).collect()
    }

    #[tokio::test]
    async fn events_write_in_order_idempotently_and_skip_interim() {
        let h = setup("events", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        let path = format!("{CALLS}/c1/events");
        let batch = json!({ "events": [
            { "type": "call.started", "n": 0, "payload": { "direction": "inbound", "from": "+14155550123", "to": "+14155550100", "numberId": "num-1" }, "occurredAt": "2026-10-03T10:00:00Z" },
            { "type": "call.transcript.delta", "n": 1, "payload": { "speaker": "caller", "text": "hel", "final": false, "segmentId": "s1" } },
            { "type": "call.transcript.delta", "n": 2, "payload": { "speaker": "caller", "text": "hello", "final": true, "segmentId": "s1" } },
            { "type": "call.transcript.delta", "n": 3, "payload": { "speaker": "bot", "text": "Hi there", "final": true, "segmentId": "s2" } },
            { "type": "call.transcript.delta", "n": 4, "payload": { "speaker": "human", "by": "user-a", "text": "taking over", "final": true, "segmentId": "s3" } },
        ] });
        let (st, body) = send(&h.app, "POST", &path, batch.clone(), true).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({ "written": 4, "duplicates": 0, "ignored": 1 }));
        let rows = ledger(&h, "c1");
        let kinds: Vec<_> = rows.iter().map(|r| (r.0.as_str(), r.1.as_str(), r.2.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                ("call.started", "bot", "bot-1"),
                ("call.transcript.delta", "caller", "+14155550123"),
                ("call.transcript.delta", "bot", "bot-1"),
                ("call.transcript.delta", "human", "user-a"),
            ]
        );
        assert!(rows.iter().all(|r| r.3["callId"] == "c1"));
        assert_eq!(rows[1].3["text"], "hello");
        // Replay: nothing new.
        let (_, body) = send(&h.app, "POST", &path, batch, true).await;
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({ "written": 0, "duplicates": 4, "ignored": 1 }));
        assert_eq!(ledger(&h, "c1").len(), 4);
        // call.ended closes the call.
        let end = json!({ "events": [{ "type": "call.ended", "n": 5, "payload": { "durationSec": 12, "reason": "hangup" }, "occurredAt": "2026-10-03T10:00:12Z" }] });
        assert_eq!(send(&h.app, "POST", &path, end, true).await.0, StatusCode::OK);
        let (state, ended): (String, Option<String>) = h.state.db.connect().unwrap().query_row("SELECT state, ended_at FROM voice_calls WHERE call_id='c1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((state.as_str(), ended.as_deref()), ("ended", Some("2026-10-03T10:00:12Z")));
        // Unknown call, bad type.
        assert_eq!(send(&h.app, "POST", &format!("{CALLS}/nope/events"), batch_of("call.started"), true).await.0, StatusCode::NOT_FOUND);
        assert_eq!(send(&h.app, "POST", &path, batch_of("agent.tool.started"), true).await.0, StatusCode::BAD_REQUEST);
    }

    fn batch_of(kind: &str) -> Value {
        json!({ "events": [{ "type": kind, "n": 9, "payload": {} }] })
    }

    fn sse_events(body: &str) -> Vec<Value> {
        body.lines().filter_map(|l| l.strip_prefix("data:")).map(|d| serde_json::from_str(d.trim()).unwrap()).collect()
    }

    #[tokio::test]
    async fn turn_streams_the_reply_with_a_spoken_preface() {
        let h = setup("turn", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        let (st, body) = send(&h.app, "POST", &format!("{CALLS}/c1/turn"), json!({ "text": "  what's on today? ", "segmentId": "s9" }), true).await;
        assert_eq!(st, StatusCode::OK);
        let ev = sse_events(&body);
        let kinds: Vec<_> = ev.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["tool", "tool", "text.delta", "text.delta", "text.delta", "done"]);
        assert_eq!((ev[0]["name"].as_str(), ev[0]["status"].as_str(), ev[1]["status"].as_str()), (Some("calendar"), Some("started"), Some("done")));
        let spoken: String = ev.iter().filter(|e| e["type"] == "text.delta").map(|e| e["text"].as_str().unwrap()).collect();
        assert_eq!(spoken, "Sure. I can do that. Anything else?");
        let prompts = h.turner.prompts.lock().unwrap();
        assert_eq!(prompts[0].0, "sess-num-1-+14155550123");
        assert!(prompts[0].1.starts_with("[Live phone call.") && prompts[0].1.ends_with("what's on today?"));
    }

    #[tokio::test]
    async fn turn_errors_stream_and_bad_requests_are_rejected() {
        let h = setup("turn-err", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        *h.turner.reply.lock().unwrap() = Some(Err("gizzi refused".into()));
        let (_, body) = send(&h.app, "POST", &format!("{CALLS}/c1/turn"), json!({ "text": "hi" }), true).await;
        assert_eq!(sse_events(&body).last().unwrap(), &json!({ "type": "error", "message": "gizzi refused" }));
        let turn = format!("{CALLS}/c1/turn");
        assert_eq!(send(&h.app, "POST", &turn, json!({ "text": "   " }), true).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(send(&h.app, "POST", &format!("{CALLS}/nope/turn"), json!({ "text": "hi" }), true).await.0, StatusCode::NOT_FOUND);
        assert_eq!(send(&h.app, "POST", &turn, json!({ "text": "hi" }), false).await.0, StatusCode::UNAUTHORIZED);
        send(&h.app, "POST", &format!("{CALLS}/c1/events"), json!({ "events": [{ "type": "call.ended", "n": 1, "payload": {} }] }), true).await;
        assert_eq!(send(&h.app, "POST", &turn, json!({ "text": "hi" }), true).await.0, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_new_turn_aborts_the_running_one_and_delete_aborts_too() {
        let h = setup("abort", Some((TOKEN, OWNER))).await;
        send(&h.app, "POST", CALLS, call_body("c1"), true).await;
        let turn = format!("{CALLS}/c1/turn");
        // Nothing running: still 204.
        assert_eq!(send(&h.app, "DELETE", &turn, Value::Null, true).await.0, StatusCode::NO_CONTENT);
        assert!(h.turner.aborted.lock().unwrap().is_empty());
        // Unsigned DELETE is refused; an unknown call is a 404.
        assert_eq!(send(&h.app, "DELETE", &turn, Value::Null, false).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(send(&h.app, "DELETE", &format!("{CALLS}/nope/turn"), Value::Null, true).await.0, StatusCode::NOT_FOUND);

        // Turn 1 hangs; turn 2 aborts it, then completes normally.
        *h.turner.hang.lock().unwrap() = true;
        let mut req = Request::builder().method("POST").uri(&turn);
        let body1 = serde_json::to_vec(&json!({ "text": "first" })).unwrap();
        for (k, v) in signed(TOKEN, now_ts(), OWNER, "POST", &turn, &body1).iter() {
            req = req.header(k, v);
        }
        let first = h.app.clone().oneshot(req.body(Body::from(body1)).unwrap()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        *h.turner.hang.lock().unwrap() = false;
        let (_, second) = send(&h.app, "POST", &turn, json!({ "text": "second" }), true).await;
        assert_eq!(sse_events(&second).last().unwrap(), &json!({ "type": "done" }));
        let first_body = String::from_utf8_lossy(&first.into_body().collect().await.unwrap().to_bytes()).to_string();
        assert_eq!(sse_events(&first_body).last().unwrap(), &json!({ "type": "done", "aborted": true }));
        assert_eq!(h.turner.aborted.lock().unwrap().as_slice(), ["sess-num-1-+14155550123"]);

        // DELETE aborts a hanging turn and its stream ends aborted.
        *h.turner.hang.lock().unwrap() = true;
        let mut req = Request::builder().method("POST").uri(&turn);
        let body3 = serde_json::to_vec(&json!({ "text": "third" })).unwrap();
        for (k, v) in signed(TOKEN, now_ts(), OWNER, "POST", &turn, &body3).iter() {
            req = req.header(k, v);
        }
        let third = h.app.clone().oneshot(req.body(Body::from(body3)).unwrap()).await.unwrap();
        assert_eq!(send(&h.app, "DELETE", &turn, Value::Null, true).await.0, StatusCode::NO_CONTENT);
        let third_body = String::from_utf8_lossy(&third.into_body().collect().await.unwrap().to_bytes()).to_string();
        assert_eq!(sse_events(&third_body).last().unwrap(), &json!({ "type": "done", "aborted": true }));
        assert_eq!(h.turner.aborted.lock().unwrap().len(), 2);
    }

    #[test]
    fn speakable_chunks_split_on_sentences_and_lose_nothing() {
        let t = "One. Two! Three?\nFour without end";
        let c = speakable_chunks(t);
        assert!(c.len() >= 4);
        assert_eq!(c.concat(), t);
        assert_eq!(speakable_chunks("3.5 percent"), vec!["3.5 percent"]);
        assert!(speakable_chunks("").is_empty());
    }
}
