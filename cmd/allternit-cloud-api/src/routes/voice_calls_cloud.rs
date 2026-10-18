//! Cloud side of phone calls (frozen contract: HANDOFF-realtime-voice
//! -2026-10-02.md §4.1). The voice worker talks only to cloud-api; the
//! owner's runtime (cloud computer or Desktop) is woken by the relay when
//! needed and never blocks the caller.
//!
//! - `POST /api/v1/voice/calls` (worker, service token): resolve the number's
//!   owner + runtime, answer immediately from the bot-config cache, then
//!   queue `call.started` for delivery. The caller never waits on a wake.
//! - `PUT /api/v1/voice/bot-config/:botId` (user auth): the runtime refreshes
//!   the cache whenever a bot is saved. Missing row = safe defaults.
//! - `POST /api/v1/voice/calls/:callId/events` (worker): ONE `call.*` event
//!   `{type, idempotencyKey, seq, atMs, payload}` per request, stored in `seq`
//!   order, deduped by the idempotency key `call:<callId>:<type>:<n>` (a
//!   duplicate answers 409, which the worker counts as delivered).
//! - `POST /api/v1/voice/calls/:callId/turns` (worker): a finished caller turn
//!   `{callId, turnId, text, confidence}`, relayed to the runtime's SSE turn
//!   route and answered as streamed ndjson (`delta` … `done` | `error`).
//! - `POST /api/v1/voice/calls/:callId/runtime` (worker): proxy to the owner's
//!   runtime for the allowlisted tool/memory/thread paths only.
//! - `POST /api/v1/voice/calls/:callId/control` (user auth from the UI):
//!   validated, then published on the LiveKit data channel topic
//!   `allternit.call.control`.
//! - `POST /api/v1/voice/linked-calls` (Desktop device token or user auth):
//!   open a `call-lp-<uuid>` room for a bot's own phone wired to the Mac,
//!   with agent dispatch, and hand Desktop its caller token
//!   (CONTRACT-linked-phone-voice-2026-10-18 C3.1). The worker's start call
//!   for that room takes owner/runtime/bot from `voice_linked_call_rooms`.
//! - `GET /api/v1/voice/calls/live` (user auth): the owner's calls that have
//!   not ended, with their line and thread (C3.4).
//!
//! Event delivery mirrors `channel_inbound`: per-call in-order, retry with
//! backoff for 24h, wake a sleeping computer, `call.started` first because it
//! is enqueued (n=1) before the worker can post anything else.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{Stream, StreamExt};

use super::channel_inbound::{attempt_error, backoff_secs, classify, is_dead, Delivery};
use super::phone::{clean_transfer_targets, record_transfer_targets, transfer_consent_ref, transfer_targets_for_bot, PhoneError, TransferInitiator, TransferTarget};
use super::livekit_admin::{
    LiveKitAdminClient, LiveKitConfig, LiveKitError, LiveKitHttpAdmin, CONTROL_TOPIC,
    SIP_AGENT_NAME,
};
use super::runtime_relay::{relay_signed_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

/// Env var holding the voice worker's service token.
pub const VOICE_WORKER_TOKEN_ENV: &str = "ALLTERNIT_VOICE_WORKER_TOKEN";
const GIVE_UP_AFTER_HOURS: i64 = 24;
/// Events held for one call before new ones are refused (429).
const MAX_PENDING_PER_CALL: i64 = 1000;
const WORKER_INTERVAL: Duration = Duration::from_secs(5);
const LOCK_MINUTES: i32 = 3;
/// A call with no event from the worker for this long is over: the worker
/// died or never reported `call.ended`, and the thread would show it live forever.
const LOST_CALL_SILENT_HOURS: i32 = 4;
/// Runtime path that receives the relayed `call.started` (voice session owns
/// the handler in allternit-api's voice_calls.rs).
const RUNTIME_CALLS_PATH: &str = "/api/v1/voice/calls";
/// Runtime path that receives every other relayed `call.*` event.
fn runtime_events_path(call_id: &str) -> String {
    format!("/api/v1/voice/calls/{call_id}/events")
}

/// The worker's start-call timeout is 900 ms; answer inside it or not at all.
const START_CALL_BUDGET: Duration = Duration::from_millis(850);
/// Cache read inside that budget; past it the safe defaults answer instead.
const BOT_CONFIG_BUDGET: Duration = Duration::from_millis(400);
/// The first byte of a turn reply goes out within this, or the turn fails.
const TURN_FIRST_BYTE: Duration = Duration::from_secs(60);

const DEFAULT_PERSONA: &str = "A helpful AI assistant.";
const DEFAULT_VOICE_ID: &str = "allternit-default";
const DEFAULT_GREETING: &str = "How can I help you today?";

/// Runtime paths the worker may proxy to, exactly as named in the frozen
/// handoff (§4.1: tool invoke, memory read, thread write). Anything else is
/// refused; this list only grows by contract change, never by caller input.
const PROXY_ALLOWLIST: &[(&str, &str)] = &[
    ("POST", "/api/v1/tools/execute"),
    ("POST", "/api/v1/memory/query"),
    ("POST", "/api/v1/threads"),
];

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/voice/calls", post(start_call))
        // Static `live` beside the `:callId/...` routes: matchit prefers the
        // static segment, and no `/api/v1/voice/calls/:callId` route exists.
        .route("/api/v1/voice/calls/live", get(live_calls))
        .route("/api/v1/voice/linked-calls", post(linked_call))
        .route("/api/v1/voice/bot-config/:botId", put(put_bot_config))
        .route("/api/v1/voice/calls/:callId/events", post(post_events))
        .route("/api/v1/voice/calls/:callId/turns", post(post_turn))
        .route("/api/v1/voice/calls/:callId/runtime", post(runtime_proxy))
        .route("/api/v1/voice/calls/:callId/control", post(control))
}

fn voice_not_configured() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "voice_not_configured" })),
    )
        .into_response()
}

fn livekit_not_configured() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "livekit_not_configured" })),
    )
        .into_response()
}

/// Constant-time equality: SHA-256 both sides and compare every byte, so the
/// compare time never depends on where two tokens differ.
fn ct_token_eq(a: &str, b: &str) -> bool {
    use sha2::Digest as _;
    let da = sha2::Sha256::digest(a.as_bytes());
    let db = sha2::Sha256::digest(b.as_bytes());
    da.iter().zip(db.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Worker auth against `expected`. `None` (env unset) is the 503
/// not-configured path so a missing secret never crashes a route.
fn check_worker_token(headers: &HeaderMap, expected: Option<&str>) -> Result<(), ApiError> {
    let Some(expected) = expected.filter(|token| !token.is_empty()) else {
        return Err(ApiError::ServiceUnavailable("voice_not_configured".to_string()));
    };
    let presented = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match presented {
        Some(token) if ct_token_eq(token, expected) => Ok(()),
        _ => Err(ApiError::Unauthorized("invalid voice worker token".to_string())),
    }
}

fn authorize_worker(headers: &HeaderMap) -> Result<(), ApiError> {
    check_worker_token(headers, std::env::var(VOICE_WORKER_TOKEN_ENV).ok().as_deref())
}

// ---------------------------------------------------------------- directory
// phone_numbers is ao-phone-sms's table (pg 024). Until it lands, everything
// resolves through this seam; the pg impl states the assumed columns and is
// reconciled with pg 024 at merge.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberOwner {
    pub user_id: String,
    pub runtime_id: String,
    /// The bot the number answers as now. An inbound call uses it over the SIP dispatch
    /// rule's `botId`, which is fixed when the rule is made and goes stale when the number
    /// moves to another bot (`PATCH /api/v1/phone/numbers/:id`).
    pub bot_id: Option<String>,
}

#[async_trait::async_trait]
pub trait PhoneNumberDirectory: Send + Sync {
    async fn owner_for(&self, number_id: &str) -> Result<Option<NumberOwner>, ApiError>;
}

pub struct PgPhoneNumberDirectory {
    pub db: sqlx::PgPool,
}

#[async_trait::async_trait]
impl PhoneNumberDirectory for PgPhoneNumberDirectory {
    async fn owner_for(&self, number_id: &str) -> Result<Option<NumberOwner>, ApiError> {
        // Assumed pg 024 shape: phone_numbers(id, user_id, runtime_id, …).
        let row: Option<(String, String, String)> =
            sqlx::query_as("SELECT user_id, runtime_id, bot_id FROM phone_numbers WHERE id = $1")
                .bind(number_id)
                .fetch_optional(&self.db)
                .await?;
        Ok(row.map(|(user_id, runtime_id, bot_id)| NumberOwner { user_id, runtime_id, bot_id: Some(bot_id).filter(|b| !b.is_empty()) }))
    }
}

// ------------------------------------------------------------------- lines
// CONTRACT-linked-phone-voice-2026-10-18.md C1/C2: a call is on a line. A
// Telnyx number is `{kind:"number", numberId}`; a bot's own phone wired to the
// Mac running Desktop is `{kind:"linked_phone", deviceId, serial?, label,
// platform}`. Rows from before pg 094 have no line and are number calls.

/// Room prefix for a linked-phone call. Still under
/// [`super::livekit_admin::CALL_ROOM_PREFIX`]
/// (`call-`), so the voice worker accepts the room.
pub const LINKED_ROOM_PREFIX: &str = "call-lp-";
/// Desktop joins a linked-phone room as `linked-phone-<deviceId>`; the worker
/// treats that participant as the caller.
pub const LINKED_IDENTITY_PREFIX: &str = "linked-phone-";
/// Desktop's room token lives four hours (one long call, no refresh).
const LINKED_TOKEN_TTL_SECS: i64 = 4 * 3600;
/// The worker's start call is trusted for a linked room only this long after
/// Desktop opened it (the agent is dispatched when the room is created).
const LINKED_ROOM_START_MINUTES: i32 = 15;
/// Longest linked-phone call billed in one go (a lost call's end is guessed).
const LINKED_MAX_BILLED_SECS: i64 = 6 * 3600;
const LINE_FIELD_MAX: usize = 200;
const PHONE_FIELD_MAX: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallLine {
    #[serde(rename_all = "camelCase")]
    Number { number_id: String },
    #[serde(rename_all = "camelCase")]
    LinkedPhone {
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        serial: Option<String>,
        label: String,
        platform: String,
    },
}

/// A required line field: trimmed, non-empty, short, no whitespace or control
/// characters inside when `token` (ids end up in a LiveKit identity).
fn line_field(name: &str, value: &str, token: bool) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::BadRequest(format!("line.{name} is required")));
    }
    if value.chars().count() > LINE_FIELD_MAX {
        return Err(ApiError::BadRequest(format!("line.{name} is too long")));
    }
    if value.chars().any(|c| c.is_control() || (token && c.is_whitespace())) {
        return Err(ApiError::BadRequest(format!("line.{name} has invalid characters")));
    }
    Ok(value.to_string())
}

impl CallLine {
    /// The same line with every field checked and trimmed (an empty serial is
    /// no serial).
    pub fn validated(&self) -> Result<CallLine, ApiError> {
        match self {
            CallLine::Number { number_id } => Ok(CallLine::Number { number_id: line_field("numberId", number_id, true)? }),
            CallLine::LinkedPhone { device_id, serial, label, platform } => {
                let platform = platform.trim();
                if !matches!(platform, "android" | "ios") {
                    return Err(ApiError::BadRequest("line.platform must be android or ios".to_string()));
                }
                let serial = match serial.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    Some(serial) => Some(line_field("serial", serial, true)?),
                    None => None,
                };
                Ok(CallLine::LinkedPhone {
                    device_id: line_field("deviceId", device_id, true)?,
                    serial,
                    label: line_field("label", label, false)?,
                    platform: platform.to_string(),
                })
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            CallLine::Number { .. } => "number",
            CallLine::LinkedPhone { .. } => "linked_phone",
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// The line a stored call is on: its `line` column, else (rows from before
/// pg 094) the number line its `number_id` implies, else null.
fn stored_line(line: Option<Value>, number_id: Option<&str>) -> Value {
    match (line.filter(|l| !l.is_null()), number_id) {
        (Some(line), _) => line,
        (None, Some(number_id)) => json!({ "kind": "number", "numberId": number_id }),
        (None, None) => Value::Null,
    }
}

/// `threadId` from the runtime's reply to the relayed call.started.
fn thread_id_from_reply(reply: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(reply)
        .ok()?
        .get("threadId")?
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty() && t.len() <= 256)
        .map(str::to_string)
}

/// A fresh linked-phone room name, `call-lp-<uuid>`.
fn linked_room_name() -> String {
    format!("{LINKED_ROOM_PREFIX}{}", uuid::Uuid::new_v4())
}

/// Desktop's participant identity in a linked-phone room.
fn linked_identity(device_id: &str) -> String {
    format!("{LINKED_IDENTITY_PREFIX}{device_id}")
}

/// An optional phone number / caller id: trimmed, empty = none (a withheld
/// caller), at most 64 characters.
fn optional_phone(name: &str, value: Option<&str>) -> Result<Option<String>, ApiError> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if v.chars().count() > PHONE_FIELD_MAX || v.chars().any(char::is_control) => {
            Err(ApiError::BadRequest(format!("{name} is not a phone number")))
        }
        other => Ok(other.map(str::to_string)),
    }
}

// ---------------------------------------------------------------- relay seam

/// A streamed runtime reply: status plus raw body chunks.
pub type RelayStream = std::pin::Pin<Box<dyn Stream<Item = Result<bytes::Bytes, String>> + Send>>;

/// One request to the owner's runtime. Every request is signed with the
/// runtime's device-token key (`runtime_relay::relay_signed_request_to_runtime_with`),
/// because the runtime's voice routes trust only cloud-api.
#[async_trait::async_trait]
pub trait CallRelay: Send + Sync {
    /// Buffered: status and whole body (events, tool proxy, abort).
    async fn relay_with(
        &self,
        method: &str,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String>;

    /// Streamed: status as soon as the runtime answers, body as it arrives
    /// (the SSE turn).
    async fn stream(
        &self,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, RelayStream), String>;

    async fn relay(
        &self,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        self.relay_with("POST", user_id, runtime_id, path, body).await
    }
}

/// Production relay: wakes a sleeping cloud computer, waits for the answer.
pub struct ProdCallRelay<'a> {
    pub state: &'a ApiState,
}

impl ProdCallRelay<'_> {
    async fn send(
        &self,
        method: &str,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<Response, String> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        relay_signed_request_to_runtime_with(
            &self.state.db,
            &self.state.contabo_runtime_service,
            &self.state.quota_service,
            &self.state.provisioning_service,
            user_id,
            runtime_id,
            RelayRequest {
                method: method.to_string(),
                path: path.to_string(),
                headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
                body: STANDARD.encode(body),
                body_encoding: "base64".to_string(),
            },
            &["content-type"],
            HashMap::new(),
        )
        .await
        .map_err(|error| error.to_string())
    }
}

#[async_trait::async_trait]
impl CallRelay for ProdCallRelay<'_> {
    async fn relay_with(
        &self,
        method: &str,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        let response = self.send(method, user_id, runtime_id, path, body).await?;
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .map_err(|error| error.to_string())?
            .to_vec();
        Ok((status, bytes))
    }

    async fn stream(
        &self,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, RelayStream), String> {
        let response = self.send("POST", user_id, runtime_id, path, body).await?;
        let status = response.status().as_u16();
        let stream = response.into_body().into_data_stream().map(|chunk| chunk.map_err(|e| e.to_string()));
        Ok((status, Box::pin(stream)))
    }
}

/// [`ProdCallRelay`] that owns its state, for work that outlives the request
/// (aborting a turn after the worker hangs up).
struct OwnedCallRelay(Arc<ApiState>);

#[async_trait::async_trait]
impl CallRelay for OwnedCallRelay {
    async fn relay_with(
        &self,
        method: &str,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        ProdCallRelay { state: &self.0 }.relay_with(method, user_id, runtime_id, path, body).await
    }

    async fn stream(
        &self,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, RelayStream), String> {
        ProdCallRelay { state: &self.0 }.stream(user_id, runtime_id, path, body).await
    }
}

// ------------------------------------------------------------- bot config

/// What the worker gets back from `POST /api/v1/voice/calls` (its `BotConfig`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BotVoiceConfig {
    /// Spoken in the disclosure; `None` until the runtime saves one.
    name: Option<String>,
    persona: String,
    voice_id: String,
    greeting: String,
    /// Recording consent configured for this bot. Off unless the owner set it.
    recording: bool,
}

type BotRow = (Option<String>, String, String, String, String);

fn bot_from_row((name, persona, voice_id, greeting, recording): BotRow) -> BotVoiceConfig {
    BotVoiceConfig {
        name: name.filter(|n| !n.trim().is_empty()),
        persona,
        voice_id,
        greeting,
        recording: recording == "consented",
    }
}

fn default_bot() -> BotVoiceConfig {
    BotVoiceConfig {
        name: None,
        persona: DEFAULT_PERSONA.to_string(),
        voice_id: DEFAULT_VOICE_ID.to_string(),
        greeting: DEFAULT_GREETING.to_string(),
        recording: false,
    }
}

/// The cache read behind the immediate call answer: the stored row, or safe
/// defaults when the runtime never saved one.
async fn load_bot_config(db: &sqlx::PgPool, bot_id: &str) -> Result<BotVoiceConfig, ApiError> {
    let row: Option<BotRow> = sqlx::query_as(
        "SELECT name, persona, voice_id, greeting, recording FROM voice_bot_config WHERE bot_id = $1",
    )
    .bind(bot_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(bot_from_row).unwrap_or_else(default_bot))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutBotConfig {
    name: Option<String>,
    persona: Option<String>,
    voice_id: Option<String>,
    greeting: Option<String>,
    /// `"off"` / `"consented"`, or a bool (`true` = consented).
    recording: Option<Value>,
    /// Owner-approved warm-transfer destinations `[{e164,label}]`. Absent keeps the
    /// stored list; `[]` clears it.
    transfer_targets: Option<Vec<TransferTarget>>,
}

async fn put_bot_config(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(bot_id): Path<String>,
    Json(body): Json<PutBotConfig>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let recording = match &body.recording {
        None | Some(Value::Null) => None,
        Some(Value::Bool(on)) => Some(if *on { "consented" } else { "off" }),
        Some(Value::String(text)) if matches!(text.as_str(), "off" | "consented") => {
            Some(if text == "consented" { "consented" } else { "off" })
        }
        Some(_) => {
            return Err(ApiError::BadRequest(
                "recording must be \"off\", \"consented\" or a boolean".to_string(),
            ))
        }
    };
    let targets = match &body.transfer_targets {
        Some(raw) => Some(clean_transfer_targets(raw).map_err(|e| match e {
            PhoneError::BadRequest(message) => ApiError::BadRequest(message),
            other => ApiError::Internal(format!("transfer targets: {other:?}")),
        })?),
        None => None,
    };
    // Absent fields keep their stored value (COALESCE); first write of a
    // missing row materializes the safe defaults.
    let row: BotRow = sqlx::query_as(
        "INSERT INTO voice_bot_config (bot_id, user_id, name, persona, voice_id, greeting, recording)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (bot_id) DO UPDATE SET
             user_id = EXCLUDED.user_id,
             name = COALESCE($3, voice_bot_config.name),
             persona = COALESCE($8, voice_bot_config.persona),
             voice_id = COALESCE($9, voice_bot_config.voice_id),
             greeting = COALESCE($10, voice_bot_config.greeting),
             recording = COALESCE($11, voice_bot_config.recording),
             transfer_targets = COALESCE($12, voice_bot_config.transfer_targets),
             updated_at = now()
         RETURNING name, persona, voice_id, greeting, recording",
    )
    .bind(&bot_id)
    .bind(&user.id)
    .bind(body.name.as_deref())
    .bind(body.persona.as_deref().unwrap_or(DEFAULT_PERSONA))
    .bind(body.voice_id.as_deref().unwrap_or(DEFAULT_VOICE_ID))
    .bind(body.greeting.as_deref().unwrap_or(DEFAULT_GREETING))
    .bind(recording.unwrap_or("off"))
    .bind(body.persona.as_deref())
    .bind(body.voice_id.as_deref())
    .bind(body.greeting.as_deref())
    .bind(recording)
    .bind(targets.as_ref().map(|t| json!(t)))
    .fetch_one(&state.db)
    .await?;
    if let Some(targets) = &targets {
        record_transfer_targets(&state.db, &user.id, &bot_id, targets)
            .await
            .map_err(|e| ApiError::Internal(format!("transfer targets: {e:?}")))?;
    }
    let stored = transfer_targets_for_bot(&state.db, &bot_id).await.unwrap_or_default();
    let mut out = serde_json::to_value(bot_from_row(row)).unwrap_or(Value::Null);
    if let Some(map) = out.as_object_mut() {
        map.insert("transferTargets".into(), json!(stored));
    }
    Ok(Json(out).into_response())
}

// ------------------------------------------------------------------- calls

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartCall {
    bot_id: String,
    /// A number call's number. Absent on a linked-phone call (C3.2).
    #[serde(default)]
    number_id: Option<String>,
    /// The call's line (C1). Absent from older workers = a number call.
    #[serde(default)]
    line: Option<CallLine>,
    /// Caller id / dialled number. A linked-phone caller may be withheld.
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    direction: String,
    room: String,
    /// From the dispatch rule attributes; checked against the number's owner.
    owner_id: Option<String>,
    sip_call_id: Option<String>,
    /// Outbound only: the consent gate's reference.
    consent_ref: Option<String>,
}

async fn start_call(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<StartCall>,
) -> Result<Response, ApiError> {
    authorize_worker(&headers)?;
    let directory = PgPhoneNumberDirectory { db: state.db.clone() };
    // The worker's own timeout is 900 ms; a slow database answers 503 inside
    // it so the worker falls back instead of dropping the caller in silence.
    match tokio::time::timeout(START_CALL_BUDGET, start_call_inner(&state, &directory, body)).await {
        Ok(result) => result,
        Err(_) => Err(ApiError::ServiceUnavailable("start_call_timeout".to_string())),
    }
}

/// Who a started call belongs to and what it is on, from the number directory
/// (number line) or `voice_linked_call_rooms` (linked phone).
struct ResolvedCall {
    user_id: String,
    runtime_id: String,
    bot_id: String,
    number_id: Option<String>,
    line: CallLine,
    direction: String,
    from: String,
    to: String,
}

async fn start_call_inner(
    state: &Arc<ApiState>,
    directory: &dyn PhoneNumberDirectory,
    body: StartCall,
) -> Result<Response, ApiError> {
    if !matches!(body.direction.as_str(), "inbound" | "outbound") {
        return Err(ApiError::BadRequest("direction must be inbound or outbound".to_string()));
    }
    let line = body.line.as_ref().map(CallLine::validated).transpose()?;
    let resolved = match line {
        Some(CallLine::LinkedPhone { device_id, .. }) => {
            match resolve_linked_start(&state.db, &body, &device_id).await? {
                LinkedStart::New(resolved) => resolved,
                // A retried start for the same room answers with the call it already made.
                LinkedStart::Existing { call_id, bot_id } => return answer_call(state, call_id, &bot_id).await,
            }
        }
        number_line => {
            let number_id = match (number_line, body.number_id.as_deref().map(str::trim).filter(|n| !n.is_empty())) {
                (Some(CallLine::Number { number_id }), Some(claimed)) if claimed != number_id => {
                    return Err(ApiError::BadRequest("numberId does not match line.numberId".to_string()));
                }
                (Some(CallLine::Number { number_id }), _) => number_id,
                (_, Some(claimed)) => claimed.to_string(),
                _ => return Err(ApiError::BadRequest("numberId is required".to_string())),
            };
            let (Some(from), Some(to)) = (body.from.clone(), body.to.clone()) else {
                return Err(ApiError::BadRequest("from and to are required".to_string()));
            };
            // A Platform API project's number (or realtime session) is answered by its
            // hosted agent: registered and answered by the platform module, not a user's runtime.
            let platform = super::platform_v1::calls::worker_start(
                &state.db,
                super::platform_v1::calls::WorkerStart {
                    number_id: &number_id,
                    room: &body.room,
                    direction: &body.direction,
                    from: &from,
                    to: &to,
                    sip_call_id: body.sip_call_id.as_deref(),
                    consent_ref: body.consent_ref.as_deref(),
                },
            )
            .await?;
            if let Some(answer) = platform {
                return Ok(Json(answer).into_response());
            }
            let Some(owner) = directory.owner_for(&number_id).await? else {
                return Err(ApiError::NotFound("number not found".to_string()));
            };
            if body.owner_id.as_deref().is_some_and(|claimed| claimed != owner.user_id) {
                return Err(ApiError::Forbidden("ownerId does not match the number's owner".to_string()));
            }
            // Inbound: the number's bot now, not the dispatch rule's (stale after a move).
            let bot_id = match (body.direction.as_str(), owner.bot_id.as_deref()) {
                ("inbound", Some(current)) => current.to_string(),
                _ => body.bot_id.clone(),
            };
            ResolvedCall {
                user_id: owner.user_id,
                runtime_id: owner.runtime_id,
                bot_id,
                line: CallLine::Number { number_id: number_id.clone() },
                number_id: Some(number_id),
                direction: body.direction.clone(),
                from,
                to,
            }
        }
    };
    let call_id = uuid::Uuid::new_v4().to_string();
    let line_json = resolved.line.to_json();
    sqlx::query(
        "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164, sip_call_id, consent_ref, line, line_kind)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(&call_id)
    .bind(&resolved.user_id)
    .bind(&resolved.runtime_id)
    .bind(resolved.number_id.as_deref())
    .bind(&resolved.bot_id)
    .bind(&body.room)
    .bind(&resolved.direction)
    .bind(&resolved.from)
    .bind(&resolved.to)
    .bind(body.sip_call_id.as_deref())
    .bind(body.consent_ref.as_deref())
    .bind(&line_json)
    .bind(resolved.line.kind())
    .execute(&state.db)
    .await?;
    // The cloud's own call.started is seq 0, so in-order delivery always lands
    // it first. The worker's call.started carries the same key and is a
    // duplicate. Delivering it starts the call on the runtime (create body).
    enqueue_event(
        &state.db,
        &call_id,
        "call.started",
        1,
        0,
        Some(chrono::Utc::now().timestamp_millis()),
        &json!({
            "direction": resolved.direction,
            "from": resolved.from,
            "to": resolved.to,
            "numberId": resolved.number_id,
            "line": line_json,
        }),
    )
    .await?;
    // Deliver without blocking the answer; the caller already has its 200.
    let spawned = state.clone();
    let spawned_call = call_id.clone();
    tokio::spawn(async move {
        let relay = ProdCallRelay { state: spawned.as_ref() };
        if let Err(error) = deliver_call(&spawned, &spawned_call, &relay).await {
            tracing::warn!(call_id = %spawned_call, "voice call delivery pass failed: {error}");
        }
    });
    answer_call(state, call_id, &resolved.bot_id).await
}

/// The worker's start-call answer: the call id and the bot's cached voice
/// config (safe defaults past the cache budget).
async fn answer_call(state: &Arc<ApiState>, call_id: String, bot_id: &str) -> Result<Response, ApiError> {
    let bot = match tokio::time::timeout(BOT_CONFIG_BUDGET, load_bot_config(&state.db, bot_id)).await {
        Ok(bot) => bot?,
        Err(_) => default_bot(),
    };
    Ok(Json(json!({ "callId": call_id, "bot": bot })).into_response())
}

enum LinkedStart {
    New(ResolvedCall),
    Existing { call_id: String, bot_id: String },
}

/// A linked-phone start is trusted only for a room Desktop opened with
/// `POST /api/v1/voice/linked-calls` (C3.1) in the last few minutes; owner,
/// runtime, bot, line and direction all come from that row, never the worker.
async fn resolve_linked_start(db: &sqlx::PgPool, body: &StartCall, device_id: &str) -> Result<LinkedStart, ApiError> {
    if !body.room.starts_with(LINKED_ROOM_PREFIX) {
        return Err(ApiError::BadRequest(format!("a linked_phone call's room starts with {LINKED_ROOM_PREFIX}")));
    }
    let row: Option<(String, String, String, Value, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT owner_id, runtime_id, bot_id, line, direction, from_number, to_number
           FROM voice_linked_call_rooms
          WHERE room = $1 AND ended_at IS NULL AND created_at > now() - make_interval(mins => $2)",
    )
    .bind(&body.room)
    .bind(LINKED_ROOM_START_MINUTES)
    .fetch_optional(db)
    .await?;
    let Some((owner_id, runtime_id, bot_id, line, direction, from, to)) = row else {
        return Err(ApiError::NotFound("linked call room not found".to_string()));
    };
    if body.owner_id.as_deref().is_some_and(|claimed| claimed != owner_id) {
        return Err(ApiError::Forbidden("ownerId does not match the room's owner".to_string()));
    }
    let line: CallLine = serde_json::from_value(line)
        .map_err(|e| ApiError::Internal(format!("stored linked line: {e}")))?;
    if !matches!(&line, CallLine::LinkedPhone { device_id: stored, .. } if stored == device_id) {
        return Err(ApiError::Forbidden("line does not match the room's phone".to_string()));
    }
    let existing: Option<(String, String)> =
        sqlx::query_as("SELECT call_id, bot_id FROM voice_calls WHERE room = $1 ORDER BY started_at LIMIT 1")
            .bind(&body.room)
            .fetch_optional(db)
            .await?;
    if let Some((call_id, bot_id)) = existing {
        return Ok(LinkedStart::Existing { call_id, bot_id });
    }
    Ok(LinkedStart::New(ResolvedCall {
        user_id: owner_id,
        runtime_id,
        bot_id,
        number_id: None,
        line,
        direction,
        // The room row has what Desktop saw; a withheld caller is "".
        from: from.or_else(|| body.from.clone()).unwrap_or_default(),
        to: to.or_else(|| body.to.clone()).unwrap_or_default(),
    }))
}

// ------------------------------------------------------------------ events

/// Store one event unless its idempotency key already exists. `n` counts the
/// event's type within the call; `seq` is the worker's call-wide order.
async fn enqueue_event(
    db: &sqlx::PgPool,
    call_id: &str,
    event_type: &str,
    n: i64,
    seq: i64,
    at_ms: Option<i64>,
    payload: &Value,
) -> Result<bool, ApiError> {
    let key = format!("call:{call_id}:{event_type}:{n}");
    let inserted = sqlx::query(
        "INSERT INTO voice_call_events (call_id, event_type, n, seq, at_ms, event_key, payload)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (event_key) DO NOTHING",
    )
    .bind(call_id)
    .bind(event_type)
    .bind(n)
    .bind(seq)
    .bind(at_ms)
    .bind(&key)
    .bind(payload)
    .execute(db)
    .await?
    .rows_affected();
    Ok(inserted > 0)
}

/// The worker's `EventEnvelope`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventIn {
    #[serde(rename = "type")]
    event_type: String,
    idempotency_key: String,
    seq: i64,
    at_ms: Option<i64>,
    #[serde(default)]
    payload: Value,
}

async fn post_events(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(call_id): Path<String>,
    Json(body): Json<EventIn>,
) -> Result<Response, ApiError> {
    authorize_worker(&headers)?;
    let header_key = headers.get("idempotency-key").and_then(|v| v.to_str().ok());
    post_events_inner(&state, &call_id, header_key, body).await
}

async fn post_events_inner(
    state: &Arc<ApiState>,
    call_id: &str,
    header_key: Option<&str>,
    event: EventIn,
) -> Result<Response, ApiError> {
    let call: Option<(String,)> =
        sqlx::query_as("SELECT call_id FROM voice_calls WHERE call_id = $1")
            .bind(call_id)
            .fetch_optional(&state.db)
            .await?;
    if call.is_none() {
        return Err(ApiError::NotFound("call not found".to_string()));
    }
    if !event.event_type.starts_with("call.") || event.event_type.len() > 64 {
        return Err(ApiError::BadRequest(format!("not a call.* event: {}", event.event_type)));
    }
    if event.seq < 1 {
        return Err(ApiError::BadRequest("seq must be >= 1".to_string()));
    }
    // `call:<callId>:<type>:<n>`; n is the last segment.
    let prefix = format!("call:{call_id}:{}:", event.event_type);
    let n = event
        .idempotency_key
        .strip_prefix(&prefix)
        .and_then(|n| n.parse::<i64>().ok())
        .filter(|n| *n >= 1)
        .ok_or_else(|| {
            ApiError::BadRequest(format!("idempotencyKey must be {prefix}<n> with n >= 1"))
        })?;
    if header_key.is_some_and(|header| header != event.idempotency_key) {
        return Err(ApiError::BadRequest(
            "Idempotency-Key does not match idempotencyKey".to_string(),
        ));
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM voice_call_events WHERE call_id = $1 AND delivered_at IS NULL AND dead_at IS NULL",
    )
    .bind(call_id)
    .fetch_one(&state.db)
    .await?;
    if pending >= MAX_PENDING_PER_CALL {
        return Ok(StatusCode::TOO_MANY_REQUESTS.into_response());
    }
    let mut payload = event.payload;
    if !payload.is_object() {
        payload = json!({});
    }
    if !enqueue_event(&state.db, call_id, &event.event_type, n, event.seq, event.at_ms, &payload).await? {
        // The worker counts 409 as delivered.
        return Ok((StatusCode::CONFLICT, Json(json!({ "duplicate": true }))).into_response());
    }
    if event.event_type == "call.ended" {
        mark_call_ended(&state.db, call_id, event.at_ms).await?;
    }
    let spawned = state.clone();
    let spawned_call = call_id.to_string();
    tokio::spawn(async move {
        let relay = ProdCallRelay { state: spawned.as_ref() };
        if let Err(error) = deliver_call(&spawned, &spawned_call, &relay).await {
            tracing::warn!(call_id = %spawned_call, "voice call delivery pass failed: {error}");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "accepted": true }))).into_response())
}

// ------------------------------------------------------------------- turns

/// The worker's `TurnRequest`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnIn {
    #[allow(dead_code)]
    call_id: Option<String>,
    turn_id: String,
    text: String,
    #[allow(dead_code)]
    confidence: Option<f64>,
}

fn runtime_turn_path(call_id: &str) -> String {
    format!("/api/v1/voice/calls/{call_id}/turn")
}

async fn post_turn(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(call_id): Path<String>,
    Json(body): Json<TurnIn>,
) -> Result<Response, ApiError> {
    authorize_worker(&headers)?;
    let relay: Arc<dyn CallRelay> = Arc::new(OwnedCallRelay(state.clone()));
    turn_inner(&state, relay, &call_id, body).await
}

/// One NDJSON line for the worker.
fn ndjson_line(value: Value) -> bytes::Bytes {
    let mut line = value.to_string();
    line.push('\n');
    bytes::Bytes::from(line)
}

/// Translate the runtime's SSE turn (`text.delta` / `done` / `error` data
/// frames, keep-alive comments) into the worker's ndjson lines. The stream
/// ends at `done` or `error`, or with an error line if the runtime hangs up
/// first. `finished` flips when that happens, so a Drop without it means the
/// worker went away mid-turn.
fn sse_to_ndjson(
    upstream: RelayStream,
    finished: Arc<std::sync::atomic::AtomicBool>,
) -> impl Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    use std::sync::atomic::Ordering;
    futures_util::stream::unfold(
        (upstream, Vec::<u8>::new(), false),
        move |(mut upstream, mut buf, mut done)| {
            let finished = finished.clone();
            async move {
                loop {
                    if done {
                        return None;
                    }
                    // A frame ends at a blank line; its data lines are the payload.
                    if let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
                        let frame: Vec<u8> = buf.drain(..end + 2).collect();
                        let frame = String::from_utf8_lossy(&frame).into_owned();
                        let data: String = frame
                            .lines()
                            .filter_map(|line| line.strip_prefix("data:"))
                            .map(|line| line.strip_prefix(' ').unwrap_or(line))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let Ok(event) = serde_json::from_str::<Value>(&data) else { continue };
                        match event["type"].as_str() {
                            Some("text.delta") => {
                                let text = event["text"].as_str().unwrap_or_default();
                                if text.is_empty() {
                                    continue;
                                }
                                let line = ndjson_line(json!({ "type": "delta", "text": text }));
                                return Some((Ok(line), (upstream, buf, done)));
                            }
                            Some("done") => {
                                finished.store(true, Ordering::SeqCst);
                                done = true;
                                let line = ndjson_line(json!({ "type": "done" }));
                                return Some((Ok(line), (upstream, buf, done)));
                            }
                            Some("error") => {
                                finished.store(true, Ordering::SeqCst);
                                done = true;
                                let message = event["message"].as_str().unwrap_or("turn failed");
                                let line = ndjson_line(json!({ "type": "error", "message": message }));
                                return Some((Ok(line), (upstream, buf, done)));
                            }
                            // Tool progress and anything newer: not spoken.
                            _ => continue,
                        }
                    }
                    match upstream.next().await {
                        // CRLF framing is normalized to LF so frames split on a blank line.
                        Some(Ok(chunk)) => buf.extend(chunk.iter().filter(|b| **b != b'\r')),
                        Some(Err(_)) | None => {
                            finished.store(true, Ordering::SeqCst);
                            done = true;
                            let line = ndjson_line(json!({ "type": "error", "message": "runtime ended the turn early" }));
                            return Some((Ok(line), (upstream, buf, done)));
                        }
                    }
                }
            }
        },
    )
}

/// Aborts the runtime's turn when the worker drops the connection before the
/// turn finished (barge-in).
struct TurnAbortGuard {
    relay: Arc<dyn CallRelay>,
    user_id: String,
    runtime_id: String,
    call_id: String,
    finished: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for TurnAbortGuard {
    fn drop(&mut self) {
        if self.finished.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let (relay, user_id, runtime_id, call_id) =
            (self.relay.clone(), self.user_id.clone(), self.runtime_id.clone(), self.call_id.clone());
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let path = runtime_turn_path(&call_id);
                if let Err(error) = relay.relay_with("DELETE", &user_id, &runtime_id, &path, &[]).await {
                    tracing::warn!(%call_id, "aborting the runtime turn failed: {error}");
                }
            });
        }
    }
}

async fn turn_inner(
    state: &Arc<ApiState>,
    relay: Arc<dyn CallRelay>,
    call_id: &str,
    body: TurnIn,
) -> Result<Response, ApiError> {
    let Some((user_id, runtime_id)) = sqlx::query_as::<_, (String, String)>(
        "SELECT user_id, runtime_id FROM voice_calls WHERE call_id = $1",
    )
    .bind(call_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Err(ApiError::NotFound("call not found".to_string()));
    };
    if body.text.trim().is_empty() {
        return Err(ApiError::BadRequest("text is required".to_string()));
    }
    if super::platform_v1::calls::is_platform_owner(&user_id) {
        // A hosted agent's call: the turn runs in its hosted-runtime session.
        let Ok(opened) = tokio::time::timeout(TURN_FIRST_BYTE, super::platform_v1::calls::worker_turn(state, call_id, &body.text)).await else {
            return Ok((StatusCode::GATEWAY_TIMEOUT, Json(json!({ "error": "runtime did not answer the turn in time" }))).into_response());
        };
        let Some(upstream) = opened? else {
            return Err(ApiError::NotFound("call not found".to_string()));
        };
        // No barge-in abort on the hosted runtime: the turn finishes there and the next one queues behind it.
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let mut response = Response::new(axum::body::Body::from_stream(sse_to_ndjson(upstream, finished)));
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/x-ndjson"),
        );
        return Ok(response);
    }
    let runtime_body = serde_json::to_vec(&json!({ "text": body.text, "segmentId": body.turn_id }))
        .unwrap_or_default();
    let path = runtime_turn_path(call_id);
    let Ok(opened) = tokio::time::timeout(
        TURN_FIRST_BYTE,
        relay.stream(&user_id, &runtime_id, &path, &runtime_body),
    )
    .await
    else {
        return Ok((
            StatusCode::GATEWAY_TIMEOUT,
            Json(json!({ "error": "runtime did not answer the turn in time" })),
        )
            .into_response());
    };
    let (status, upstream) =
        opened.map_err(|error| ApiError::Internal(format!("turn relay failed: {error}")))?;
    if !(200..300).contains(&status) {
        // Pass the runtime's refusal through (409 ended call, 503 asleep, …) so
        // the worker's retry rules see the real status.
        let mut upstream = upstream;
        let mut bytes = Vec::new();
        while let Some(Ok(chunk)) = upstream.next().await {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > 64 * 1024 {
                break;
            }
        }
        return Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), bytes).into_response());
    }
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let guard = TurnAbortGuard {
        relay,
        user_id,
        runtime_id,
        call_id: call_id.to_string(),
        finished: finished.clone(),
    };
    // The guard lives exactly as long as the response stream.
    let stream = sse_to_ndjson(upstream, finished).map(move |item| {
        let _keep = &guard;
        item
    });
    let mut response = Response::new(axum::body::Body::from_stream(stream));
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/x-ndjson"),
    );
    Ok(response)
}

// ------------------------------------------------------------------ proxy

#[derive(Deserialize)]
struct RuntimeProxyRequest {
    method: Option<String>,
    path: String,
    body: Option<Value>,
}

async fn runtime_proxy(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(call_id): Path<String>,
    Json(request): Json<RuntimeProxyRequest>,
) -> Result<Response, ApiError> {
    authorize_worker(&headers)?;
    let relay = ProdCallRelay { state: state.as_ref() };
    runtime_proxy_inner(&state, &relay, &call_id, request).await
}

async fn runtime_proxy_inner(
    state: &Arc<ApiState>,
    relay: &dyn CallRelay,
    call_id: &str,
    request: RuntimeProxyRequest,
) -> Result<Response, ApiError> {
    let Some((user_id, runtime_id)) = sqlx::query_as::<_, (String, String)>(
        "SELECT user_id, runtime_id FROM voice_calls WHERE call_id = $1",
    )
    .bind(call_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Err(ApiError::NotFound("call not found".to_string()));
    };
    let method = request.method.unwrap_or_else(|| "POST".to_string()).to_ascii_uppercase();
    let path = request.path.split('?').next().unwrap_or("").to_string();
    if !PROXY_ALLOWLIST.iter().any(|(m, p)| *m == method && *p == path) {
        return Err(ApiError::Forbidden(format!("{method} {path} is not allowlisted")));
    }
    // The relay itself bounds the wait (RELAY_TIMEOUT); the worker's own
    // client timeout sits on top of that.
    let body = serde_json::to_vec(&request.body.unwrap_or(Value::Null)).unwrap_or_default();
    let (status, bytes) = relay
        .relay(&user_id, &runtime_id, &path, &body)
        .await
        .map_err(|error| ApiError::Internal(format!("runtime proxy failed: {error}")))?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), bytes).into_response())
}

// ------------------------------------------------------------ linked phone

/// `POST /api/v1/voice/linked-calls` body (C3.1).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkedCallRequest {
    bot_id: String,
    line: CallLine,
    direction: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    /// User-token callers only: the runtime that runs the bot. A device token
    /// is its own runtime and ignores this.
    #[serde(default)]
    runtime_id: Option<String>,
}

/// A checked [`LinkedCallRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkedCall {
    bot_id: String,
    line: CallLine,
    device_id: String,
    direction: String,
    from: Option<String>,
    to: Option<String>,
}

impl LinkedCallRequest {
    fn validated(&self) -> Result<LinkedCall, ApiError> {
        let bot_id = self.bot_id.trim();
        if bot_id.is_empty() {
            return Err(ApiError::BadRequest("botId is required".to_string()));
        }
        if bot_id.chars().count() > LINE_FIELD_MAX {
            return Err(ApiError::BadRequest("botId is too long".to_string()));
        }
        let line = self.line.validated()?;
        let CallLine::LinkedPhone { device_id, .. } = &line else {
            return Err(ApiError::BadRequest("line.kind must be linked_phone".to_string()));
        };
        let direction = self.direction.trim();
        if !matches!(direction, "inbound" | "outbound") {
            return Err(ApiError::BadRequest("direction must be inbound or outbound".to_string()));
        }
        Ok(LinkedCall {
            bot_id: bot_id.to_string(),
            device_id: device_id.clone(),
            line: line.clone(),
            direction: direction.to_string(),
            from: optional_phone("from", self.from.as_deref())?,
            to: optional_phone("to", self.to.as_deref())?,
        })
    }
}

/// Who opened a linked-phone call: the Desktop runtime by its device token
/// (owner + runtime), or a signed-in user (owner only).
struct LinkedCaller {
    owner_id: String,
    runtime_id: Option<String>,
}

async fn linked_caller(db: &sqlx::PgPool, headers: &HeaderMap) -> Result<LinkedCaller, ApiError> {
    if let Some(token) = super::runtime_pairing::device_token_from_headers(headers) {
        let device = super::runtime_pairing::runtime_device_for_token(db, token, None).await?;
        return Ok(LinkedCaller { owner_id: device.user_id, runtime_id: Some(device.id) });
    }
    let user = crate::auth::resolve_user_scoped(db, headers, "compute").await?;
    Ok(LinkedCaller { owner_id: user.id, runtime_id: None })
}

async fn linked_call(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<LinkedCallRequest>,
) -> Result<Response, ApiError> {
    let caller = linked_caller(&state.db, &headers).await?;
    let Some(config) = LiveKitConfig::from_env() else {
        return Ok(livekit_not_configured());
    };
    let livekit = LiveKitHttpAdmin::new(config);
    let answer = linked_call_inner(&state.db, &caller, &body, &livekit).await?;
    Ok((StatusCode::CREATED, Json(answer)).into_response())
}

/// The runtime a user-token caller's call goes to: the named one if it is
/// theirs, else their most recently seen runtime.
async fn runtime_for_owner(db: &sqlx::PgPool, owner_id: &str, named: Option<&str>) -> Result<String, ApiError> {
    let found: Option<String> = match named.map(str::trim).filter(|r| !r.is_empty()) {
        Some(runtime_id) => sqlx::query_scalar(
            "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
        )
        .bind(runtime_id)
        .bind(owner_id)
        .fetch_optional(db)
        .await?,
        None => sqlx::query_scalar(
            "SELECT id FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL
              ORDER BY last_seen_at DESC NULLS LAST, created_at DESC LIMIT 1",
        )
        .bind(owner_id)
        .fetch_optional(db)
        .await?,
    };
    found.ok_or_else(|| ApiError::NotFound("runtime not found".to_string()))
}

async fn linked_call_inner(
    db: &sqlx::PgPool,
    caller: &LinkedCaller,
    body: &LinkedCallRequest,
    livekit: &dyn LiveKitAdminClient,
) -> Result<Value, ApiError> {
    let call = body.validated()?;
    let owner_id = caller.owner_id.as_str();
    // Linked-phone calls are Cloud Voice minutes: a plan with Cloud Voice only.
    let plan = crate::services::voice_usage::plan_for_user(db, owner_id).await?;
    if !crate::services::voice_usage::plan_allows_overage(&plan) {
        return Err(ApiError::PaymentRequired("plan_has_no_voice".to_string()));
    }
    // Bots live on the runtime; the cloud's record of who owns a bot is the
    // voice config the runtime saves (PUT /api/v1/voice/bot-config/:botId).
    // Someone else's bot reads as missing; a bot with no saved config yet runs
    // on the owner's runtime with safe defaults, as a number call does.
    let config_owner: Option<String> = sqlx::query_scalar("SELECT user_id FROM voice_bot_config WHERE bot_id = $1")
        .bind(&call.bot_id)
        .fetch_optional(db)
        .await?;
    if config_owner.is_some_and(|owner| owner != owner_id) {
        return Err(ApiError::NotFound("bot not found".to_string()));
    }
    let runtime_id = match &caller.runtime_id {
        Some(runtime_id) => runtime_id.clone(),
        None => runtime_for_owner(db, owner_id, body.runtime_id.as_deref()).await?,
    };
    let room = linked_room_name();
    let identity = linked_identity(&call.device_id);
    let line_json = call.line.to_json();
    let livekit_err = |error: LiveKitError| match error {
        LiveKitError::NotConfigured => ApiError::ServiceUnavailable("livekit_not_configured".into()),
        LiveKitError::PublicUrlMissing => ApiError::ServiceUnavailable("livekit_public_url_not_configured".into()),
        other => ApiError::Internal(format!("livekit: {other}")),
    };
    // Mint first: a missing public URL fails before anything is stored or created.
    let access = livekit
        .participant_access_ttl(&room, &identity, true, LINKED_TOKEN_TTL_SECS)
        .map_err(livekit_err)?;
    // The row exists before the room does, so the agent's start call (sent as
    // soon as the room opens) always finds it.
    sqlx::query(
        "INSERT INTO voice_linked_call_rooms (room, owner_id, runtime_id, bot_id, line, direction, from_number, to_number)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&room)
    .bind(owner_id)
    .bind(&runtime_id)
    .bind(&call.bot_id)
    .bind(&line_json)
    .bind(&call.direction)
    .bind(call.from.as_deref())
    .bind(call.to.as_deref())
    .execute(db)
    .await?;
    let metadata = json!({
        "source": "linked_phone",
        "botId": call.bot_id,
        "ownerId": owner_id,
        "line": line_json,
        "direction": call.direction,
        "from": call.from,
        "to": call.to,
    })
    .to_string();
    if let Err(error) = livekit.create_room_with_agent(&room, SIP_AGENT_NAME, &metadata).await {
        let _ = sqlx::query("DELETE FROM voice_linked_call_rooms WHERE room = $1").bind(&room).execute(db).await;
        return Err(livekit_err(error));
    }
    Ok(json!({ "room": room, "livekitUrl": access.url, "token": access.token, "identity": identity }))
}

/// `GET /api/v1/voice/calls/live` (C3.4): the signed-in owner's calls that have
/// not ended, newest first. `threadId` is null until the runtime has answered
/// the relayed call.started.
async fn live_calls(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    Ok(Json(live_calls_for(&state.db, &user.id).await?).into_response())
}

type LiveRow = (String, String, Option<String>, Option<Value>, Option<String>, chrono::DateTime<chrono::Utc>, String, String, String);

async fn live_calls_for(db: &sqlx::PgPool, user_id: &str) -> Result<Value, ApiError> {
    // ended_at is stamped with call.ended (pg 094). The NOT EXISTS covers
    // calls that ended before 094 ran; the day bound covers a call whose
    // events were already cleaned up (the lost-call sweep ends silent calls
    // after 4 hours, so a live call is never that old).
    let rows: Vec<LiveRow> = sqlx::query_as(
        "SELECT c.call_id, c.bot_id, c.thread_id, c.line, c.number_id, c.started_at, c.direction, c.from_e164, c.to_e164
           FROM voice_calls c
          WHERE c.user_id = $1 AND c.ended_at IS NULL AND c.started_at > now() - interval '1 day'
            AND NOT EXISTS (SELECT 1 FROM voice_call_events e WHERE e.call_id = c.call_id AND e.event_type = 'call.ended')
          ORDER BY c.started_at DESC
          LIMIT 50",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let blank = |v: String| Some(v).filter(|v| !v.is_empty());
    Ok(Value::Array(
        rows.into_iter()
            .map(|(call_id, bot_id, thread_id, line, number_id, started_at, direction, from, to)| {
                json!({
                    "callId": call_id,
                    "botId": bot_id,
                    "threadId": thread_id,
                    "line": stored_line(line, number_id.as_deref()),
                    "startedAt": started_at.to_rfc3339(),
                    "direction": direction,
                    "from": blank(from),
                    "to": blank(to),
                })
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------- controls

async fn control(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(call_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let Some(config) = LiveKitConfig::from_env() else {
        return Ok(livekit_not_configured());
    };
    let livekit = LiveKitHttpAdmin::new(config);
    control_inner(&state, &user.id, &call_id, body, &livekit).await
}

async fn control_inner(
    state: &Arc<ApiState>,
    user_id: &str,
    call_id: &str,
    body: Value,
    livekit: &dyn LiveKitAdminClient,
) -> Result<Response, ApiError> {
    // number_id is NULL on a linked-phone call (pg 094): it has no number to
    // issue a transfer consent against, and the worker refuses transfer there.
    let Some((owner_id, room, number_id, bot_id)) = sqlx::query_as::<_, (String, String, Option<String>, String)>(
        "SELECT user_id, room, number_id, bot_id FROM voice_calls WHERE call_id = $1",
    )
    .bind(call_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Err(ApiError::NotFound("call not found".to_string()));
    };
    if owner_id != user_id {
        // Same 404 as a missing call: a caller must not learn that a call
        // they don't own exists.
        return Err(ApiError::NotFound("call not found".to_string()));
    }
    validate_control(&body)?;
    let action = body["action"].as_str().unwrap_or_default().to_string();
    let livekit_err = |error: LiveKitError| match error {
        LiveKitError::NotConfigured => ApiError::ServiceUnavailable("livekit_not_configured".into()),
        LiveKitError::PublicUrlMissing => {
            ApiError::ServiceUnavailable("livekit_public_url_not_configured".into())
        }
        other => ApiError::Internal(format!("livekit: {other}")),
    };
    // listen / takeover hand the requesting owner a room token. Listen is
    // receive-only and touches nothing in the call; takeover also tells the
    // worker (it emits call.takeover) and returns a publish-capable token.
    let access = match action.as_str() {
        "listen" => Some(
            livekit
                .participant_access(&room, &format!("listener-{user_id}-{}", uuid::Uuid::new_v4().simple()), false)
                .map_err(livekit_err)?,
        ),
        "takeover" => Some(
            livekit
                .participant_access(&room, &format!("human-{user_id}"), true)
                .map_err(livekit_err)?,
        ),
        _ => None,
    };
    if action == "listen" {
        let access = access.expect("listen mints a token");
        return Ok((
            StatusCode::OK,
            Json(json!({ "token": access.token, "url": access.url, "room": room })),
        )
            .into_response());
    }
    // A phone screen's mute button mutes the bot's voice, so no target = bot.
    let target = match body.get("target").filter(|t| !t.is_null()) {
        Some(target) => target.clone(),
        None if matches!(action.as_str(), "mute" | "unmute") => json!("bot"),
        None => Value::Null,
    };
    // A warm transfer dials a third party, so the worker refuses it without a consentRef.
    // The authenticated owner pressing transfer gets `owner_directed`; a bot-initiated
    // request (`initiator: "bot"`) only gets one for an owner-approved target. No ref =
    // the worker fails the transfer cleanly (call.transferred ok:false).
    let consent_ref = if action == "transfer" && body.get("mode").and_then(Value::as_str) == Some("warm") {
        let initiator = if body.get("initiator").and_then(Value::as_str) == Some("bot") {
            TransferInitiator::Bot
        } else {
            TransferInitiator::Owner
        };
        let to = body.get("to").and_then(Value::as_str).unwrap_or_default();
        match number_id.as_deref() {
            Some(number_id) => match transfer_consent_ref(&state.db, user_id, number_id, &bot_id, to, initiator).await {
                Ok(consent) => consent.map(|c| c.id),
                Err(error) => return Err(ApiError::Internal(format!("transfer consent: {error:?}"))),
            },
            None => None,
        }
    } else {
        None
    };
    let payload = json!({
        "callId": call_id,
        "action": body["action"],
        "consentRef": consent_ref,
        "by": user_id,
        "target": target,
        "digits": body.get("digits").cloned().unwrap_or(Value::Null),
        "to": body.get("to").cloned().unwrap_or(Value::Null),
        "mode": body.get("mode").cloned().unwrap_or(Value::Null),
    });
    livekit
        .send_data(&room, CONTROL_TOPIC, payload.to_string().as_bytes())
        .await
        .map_err(|error| match error {
            LiveKitError::NotConfigured => ApiError::ServiceUnavailable("livekit_not_configured".into()),
            other => ApiError::Internal(format!("publishing control: {other}")),
        })?;
    if let Some(access) = access {
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "ok": true, "token": access.token, "url": access.url, "room": room })),
        )
            .into_response());
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response())
}

fn validate_control(body: &Value) -> Result<(), ApiError> {
    let action = body
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("action is required".to_string()))?;
    if !matches!(
        action,
        "hangup"
            | "mute"
            | "unmute"
            | "hold"
            | "resume"
            | "dtmf"
            | "transfer"
            | "takeover"
            | "listen"
            | "release"
    ) {
        return Err(ApiError::BadRequest(format!("unknown action: {action}")));
    }
    if let Some(target) = body.get("target").filter(|t| !t.is_null()) {
        if !matches!(target.as_str(), Some("bot" | "caller")) {
            return Err(ApiError::BadRequest("target must be bot or caller".to_string()));
        }
    }
    if action == "dtmf" && body.get("digits").and_then(Value::as_str).is_none() {
        return Err(ApiError::BadRequest("dtmf needs digits".to_string()));
    }
    if action == "transfer" && body.get("to").and_then(Value::as_str).is_none() {
        return Err(ApiError::BadRequest("transfer needs to".to_string()));
    }
    Ok(())
}

// ----------------------------------------------------------------- delivery

/// Start the background delivery loop and the 7-day cleanup.
pub fn start_voice_calls_worker(state: Arc<ApiState>) {
    tokio::spawn(async move {
        let mut ticks: u64 = 0;
        loop {
            if let Err(error) = deliver_due_calls(&state).await {
                tracing::warn!("voice calls worker: {error}");
            }
            ticks += 1;
            if ticks % 60 == 0 {
                match end_lost_calls(&state.db).await {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(calls = n, "voice calls worker: ended calls the worker stopped reporting"),
                    Err(error) => tracing::warn!("voice calls worker: lost-call sweep: {error}"),
                }
            }
            if ticks % 720 == 0 {
                let _ = sqlx::query(
                    "DELETE FROM voice_call_events WHERE (delivered_at IS NOT NULL AND delivered_at < now() - interval '7 days')
                        OR (dead_at IS NOT NULL AND dead_at < now() - interval '7 days')",
                )
                .execute(&state.db)
                .await;
            }
            tokio::time::sleep(WORKER_INTERVAL).await;
        }
    });
}

/// Queue `call.ended` (`reason: "lost"`) for each call that has no
/// `call.ended` and no event for [`LOST_CALL_SILENT_HOURS`]; the runtime then
/// ends it like any other call. The key is the worker's own first `call.ended`,
/// so a late real one is a duplicate, not a second end.
async fn end_lost_calls(db: &sqlx::PgPool) -> Result<usize, ApiError> {
    let lost: Vec<(String, i64)> = sqlx::query_as(
        "SELECT e.call_id, max(e.seq) FROM voice_call_events e
          GROUP BY e.call_id
         HAVING max(e.received_at) < now() - make_interval(hours => $1)
            AND bool_and(e.event_type <> 'call.ended')
          LIMIT 50",
    )
    .bind(LOST_CALL_SILENT_HOURS)
    .fetch_all(db)
    .await?;
    let mut ended = 0;
    for (call_id, seq) in lost {
        let payload = json!({ "reason": "lost", "detail": format!("no word from the call worker for {LOST_CALL_SILENT_HOURS} hours") });
        if enqueue_event(db, &call_id, "call.ended", 1, seq + 1, Some(chrono::Utc::now().timestamp_millis()), &payload).await? {
            ended += 1;
        }
        // Lost: billed up to the last word from the worker, not the 4 silent hours.
        let last_ms: Option<i64> = sqlx::query_scalar(
            "SELECT (extract(epoch FROM max(received_at)) * 1000)::bigint FROM voice_call_events
              WHERE call_id = $1 AND NOT (event_type = 'call.ended' AND payload->>'reason' = 'lost')",
        )
        .bind(&call_id)
        .fetch_one(db)
        .await?;
        mark_call_ended(db, &call_id, last_ms).await?;
    }
    Ok(ended)
}

/// The call is over (`call.ended` queued): stamp `ended_at` once, close its
/// linked-phone room, and meter a linked-phone call's seconds as Cloud Voice
/// (C3.5). Metering always records, like the Cloud Voice ticket path;
/// `ALLTERNIT_VOICE_BILLING_MODE` decides downstream whether those seconds are
/// ever priced or charged (`voice_billing::compute_period`), so `off` bills
/// nothing. Number calls are metered elsewhere and untouched here.
async fn mark_call_ended(db: &sqlx::PgPool, call_id: &str, at_ms: Option<i64>) -> Result<(), ApiError> {
    let now = chrono::Utc::now();
    let ended_at = at_ms
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .filter(|at| *at <= now)
        .unwrap_or(now);
    let ended: Option<(String, Option<String>, String, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "UPDATE voice_calls SET ended_at = GREATEST($2, started_at)
          WHERE call_id = $1 AND ended_at IS NULL
          RETURNING user_id, line_kind, room, started_at, ended_at",
    )
    .bind(call_id)
    .bind(ended_at)
    .fetch_optional(db)
    .await?;
    let Some((user_id, line_kind, room, started_at, ended_at)) = ended else {
        return Ok(());
    };
    if line_kind.as_deref() != Some("linked_phone") {
        return Ok(());
    }
    sqlx::query("UPDATE voice_linked_call_rooms SET ended_at = now() WHERE room = $1 AND ended_at IS NULL")
        .bind(&room)
        .execute(db)
        .await?;
    let seconds = linked_billed_seconds(started_at, ended_at);
    crate::services::voice_usage::record_at(
        db,
        &user_id,
        crate::services::voice_usage::ENGINE_CLOUD,
        seconds,
        &format!("linked-call:{call_id}"),
        ended_at,
    )
    .await
}

/// Whole seconds between start and end, never negative, capped at
/// [`LINKED_MAX_BILLED_SECS`].
fn linked_billed_seconds(started_at: chrono::DateTime<chrono::Utc>, ended_at: chrono::DateTime<chrono::Utc>) -> i32 {
    (ended_at - started_at).num_seconds().clamp(0, LINKED_MAX_BILLED_SECS) as i32
}

async fn deliver_due_calls(state: &Arc<ApiState>) -> Result<(), ApiError> {
    let calls: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT call_id FROM voice_call_events
          WHERE delivered_at IS NULL AND dead_at IS NULL AND next_attempt_at <= now()
            AND (locked_until IS NULL OR locked_until < now())
          LIMIT 50",
    )
    .fetch_all(&state.db)
    .await?;
    for (call_id,) in calls {
        let spawned = state.clone();
        tokio::spawn(async move {
            let relay = ProdCallRelay { state: spawned.as_ref() };
            if let Err(error) = deliver_call(&spawned, &call_id, &relay).await {
                tracing::warn!(%call_id, "voice call delivery pass failed: {error}");
            }
        });
    }
    Ok(())
}

/// Deliver one call's due events in n order; stop at the first that must
/// wait, so the runtime sees them exactly as the worker numbered them.
async fn deliver_call(
    state: &ApiState,
    call_id: &str,
    relay: &dyn CallRelay,
) -> Result<(), ApiError> {
    loop {
        let claimed: Option<(i64, String, i64, Option<i64>, Value, chrono::DateTime<chrono::Utc>, i32)> =
            sqlx::query_as(
                "UPDATE voice_call_events SET locked_until = now() + make_interval(mins => $2), attempts = attempts + 1
                  WHERE id = (
                    SELECT id FROM voice_call_events
                     WHERE call_id = $1 AND delivered_at IS NULL AND dead_at IS NULL
                     ORDER BY seq, id LIMIT 1 FOR UPDATE SKIP LOCKED)
                    AND next_attempt_at <= now() AND (locked_until IS NULL OR locked_until < now())
                  RETURNING id, event_type, n::bigint, at_ms, payload, received_at, attempts",
            )
            .bind(call_id)
            .bind(LOCK_MINUTES)
            .fetch_optional(&state.db)
            .await?;
        let Some((id, event_type, n, at_ms, payload, received_at, attempts)) = claimed
        else {
            return Ok(());
        };
        // number_id is NULL and line set on a linked-phone call (pg 094);
        // rows from before 094 have no line and are number calls.
        let Some((user_id, runtime_id, bot_id, number_id, from, to, direction, room, started_at, line)) =
            sqlx::query_as::<_, (String, String, String, Option<String>, String, String, String, String, chrono::DateTime<chrono::Utc>, Option<Value>)>(
                "SELECT user_id, runtime_id, bot_id, number_id, from_e164, to_e164, direction, room, started_at, line
                   FROM voice_calls WHERE call_id = $1",
            )
            .bind(call_id)
            .fetch_optional(&state.db)
            .await?
        else {
            sqlx::query(
                "UPDATE voice_call_events SET dead_at = now(), last_error = 'call gone' WHERE id = $1",
            )
            .bind(id)
            .execute(&state.db)
            .await?;
            continue;
        };
        if super::platform_v1::calls::is_platform_owner(&user_id) {
            // A hosted agent's call: applied in the cloud (transcript, end, webhooks), never relayed.
            let applied = super::platform_v1::calls::apply_worker_event(&state.db, call_id, &event_type, &payload).await;
            let (delivered, error) = match &applied {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e.to_string())),
            };
            sqlx::query(
                "UPDATE voice_call_events SET delivered_at = CASE WHEN $2 THEN now() END, locked_until = NULL, last_error = $3,
                        next_attempt_at = now() + make_interval(secs => $4),
                        dead_at = CASE WHEN NOT $2 AND $5 THEN now() END
                  WHERE id = $1",
            )
            .bind(id)
            .bind(delivered)
            .bind(error)
            .bind(backoff_secs(attempts) as f64)
            .bind(attempts >= 10)
            .execute(&state.db)
            .await?;
            if delivered || attempts >= 10 {
                continue;
            }
            return Ok(());
        }
        // What the runtime's voice routes accept (allternit-api voice_calls.rs):
        // call.started becomes the create body, anything else a one-event batch.
        let (path, body) = if event_type == "call.started" {
            (
                RUNTIME_CALLS_PATH.to_string(),
                json!({
                    "callId": call_id,
                    "botId": bot_id,
                    "ownerId": user_id,
                    "numberId": number_id,
                    "line": stored_line(line, number_id.as_deref()),
                    "from": from,
                    "to": to,
                    "direction": direction,
                    "room": room,
                    "startedAt": started_at.to_rfc3339(),
                }),
            )
        } else {
            let occurred_at = at_ms
                .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                .unwrap_or(received_at);
            (
                runtime_events_path(call_id),
                json!({ "events": [{
                    "type": event_type,
                    "n": n,
                    "payload": payload,
                    "occurredAt": occurred_at.to_rfc3339(),
                }] }),
            )
        };
        let body = serde_json::to_vec(&body).unwrap_or_default();
        let outcome = relay.relay(&user_id, &runtime_id, &path, &body).await;
        let (status, error) = match &outcome {
            Ok((status, _)) => (Some(*status), None),
            Err(message) => (None, Some(message.clone())),
        };
        if status.map(classify) == Some(Delivery::Done) {
            // The runtime answers the create body with the call's thread; keep it
            // for GET /api/v1/voice/calls/live (web and PWA find the call by it).
            if event_type == "call.started" {
                if let Some(thread_id) = outcome.as_ref().ok().and_then(|(_, reply)| thread_id_from_reply(reply)) {
                    sqlx::query("UPDATE voice_calls SET thread_id = $2 WHERE call_id = $1")
                        .bind(call_id)
                        .bind(&thread_id)
                        .execute(&state.db)
                        .await?;
                }
            }
            sqlx::query(
                "UPDATE voice_call_events SET delivered_at = now(), locked_until = NULL, last_status = $2 WHERE id = $1",
            )
            .bind(id)
            .bind(status.map(i32::from))
            .execute(&state.db)
            .await?;
            continue;
        }
        let error = attempt_error(status, error);
        let give_up = is_dead(
            status,
            attempts,
            chrono::Utc::now() - received_at > chrono::Duration::hours(GIVE_UP_AFTER_HOURS),
        );
        if give_up {
            tracing::warn!(%call_id, event_id = id, %event_type, ?status, attempts, error = ?error, "voice call event dead: not delivered to the runtime");
        }
        sqlx::query(
            "UPDATE voice_call_events
                SET locked_until = NULL, last_status = $2, last_error = $3,
                    next_attempt_at = now() + make_interval(secs => $4),
                    dead_at = CASE WHEN $5 THEN now() ELSE NULL END
              WHERE id = $1",
        )
        .bind(id)
        .bind(status.map(i32::from))
        .bind(error)
        .bind(backoff_secs(attempts) as f64)
        .bind(give_up)
        .execute(&state.db)
        .await?;
        if give_up {
            continue;
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{self, MockGateway};

    // ---- fakes

    #[derive(Default)]
    struct FakeDirectory {
        owners: std::sync::Mutex<HashMap<String, NumberOwner>>,
    }

    #[async_trait::async_trait]
    impl PhoneNumberDirectory for FakeDirectory {
        async fn owner_for(&self, number_id: &str) -> Result<Option<NumberOwner>, ApiError> {
            Ok(self.owners.lock().unwrap().get(number_id).cloned())
        }
    }

    #[derive(Default)]
    struct FakeRelay {
        /// (path, body-json) per call, in order.
        calls: std::sync::Mutex<Vec<(String, Value)>>,
        /// Status returned per attempt; defaults to 200.
        statuses: std::sync::Mutex<VecDeque<u16>>,
        /// HTTP method per call, parallel to `calls`.
        methods: std::sync::Mutex<Vec<String>>,
        /// SSE chunks the streamed turn yields, then the stream stays open
        /// (like a runtime still working) unless `sse_ends` is set.
        sse: std::sync::Mutex<Vec<String>>,
        sse_ends: std::sync::atomic::AtomicBool,
        /// Reply body for buffered relays; defaults to `{"ok":true}`.
        reply: std::sync::Mutex<Option<String>>,
    }

    use std::collections::VecDeque;

    impl FakeRelay {
        fn asleep_then_awake() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
                statuses: std::sync::Mutex::new(VecDeque::from(vec![503u16])),
                ..Self::default()
            }
        }
        fn recorded(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl CallRelay for FakeRelay {
        async fn relay_with(
            &self,
            method: &str,
            _user_id: &str,
            _runtime_id: &str,
            path: &str,
            body: &[u8],
        ) -> Result<(u16, Vec<u8>), String> {
            let status = self
                .statuses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(200);
            self.methods.lock().unwrap().push(method.to_string());
            self.calls.lock().unwrap().push((
                path.to_string(),
                if body.is_empty() { Value::Null } else { serde_json::from_slice(body).unwrap() },
            ));
            let reply = self.reply.lock().unwrap().clone().unwrap_or_else(|| r#"{"ok":true}"#.to_string());
            Ok((status, reply.into_bytes()))
        }

        async fn stream(
            &self,
            _user_id: &str,
            _runtime_id: &str,
            path: &str,
            body: &[u8],
        ) -> Result<(u16, RelayStream), String> {
            self.methods.lock().unwrap().push("POST".to_string());
            self.calls
                .lock()
                .unwrap()
                .push((path.to_string(), serde_json::from_slice(body).unwrap()));
            let chunks: Vec<Result<bytes::Bytes, String>> = self
                .sse
                .lock()
                .unwrap()
                .iter()
                .map(|chunk| Ok(bytes::Bytes::from(chunk.clone())))
                .collect();
            let head = futures_util::stream::iter(chunks);
            if self.sse_ends.load(std::sync::atomic::Ordering::SeqCst) {
                Ok((200, Box::pin(head)))
            } else {
                Ok((200, Box::pin(head.chain(futures_util::stream::pending()))))
            }
        }
    }

    #[derive(Default)]
    struct FakeLiveKit {
        sent: std::sync::Mutex<Vec<(String, String, Value)>>,
        /// (room, agent, metadata) per create_room_with_agent.
        rooms: std::sync::Mutex<Vec<(String, String, Value)>>,
        /// (room, identity, can_publish, ttl) per minted token.
        tokens: std::sync::Mutex<Vec<(String, String, bool, i64)>>,
        fail_rooms: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl LiveKitAdminClient for FakeLiveKit {
        async fn ensure_inbound_trunk(&self, _n: &str, _e: &str) -> Result<String, LiveKitError> {
            unimplemented!("not needed for these tests")
        }
        async fn delete_inbound_trunk(&self, _t: &str) -> Result<(), LiveKitError> {
            unimplemented!()
        }
        async fn ensure_dispatch_rule(
            &self,
            _t: &str,
            _n: &str,
            _b: &str,
            _o: &str,
            _to: &str,
        ) -> Result<String, LiveKitError> {
            unimplemented!()
        }
        async fn delete_dispatch_rule(&self, _r: &str) -> Result<(), LiveKitError> {
            unimplemented!()
        }
        async fn create_sip_participant(
            &self,
            _r: crate::routes::livekit_admin::CreateSipParticipantRequest,
        ) -> Result<Value, LiveKitError> {
            unimplemented!()
        }
        async fn create_room_with_agent(&self, room: &str, agent: &str, metadata: &str) -> Result<(), LiveKitError> {
            if self.fail_rooms.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(LiveKitError::Http("boom".into()));
            }
            self.rooms.lock().unwrap().push((room.to_string(), agent.to_string(), serde_json::from_str(metadata).unwrap()));
            Ok(())
        }
        fn participant_access_ttl(
            &self,
            room: &str,
            identity: &str,
            can_publish: bool,
            ttl_secs: i64,
        ) -> Result<crate::routes::livekit_admin::ParticipantAccess, LiveKitError> {
            self.tokens.lock().unwrap().push((room.to_string(), identity.to_string(), can_publish, ttl_secs));
            self.participant_access(room, identity, can_publish)
        }
        async fn send_data(&self, room: &str, topic: &str, payload: &[u8]) -> Result<(), LiveKitError> {
            self.sent.lock().unwrap().push((
                room.to_string(),
                topic.to_string(),
                serde_json::from_slice(payload).unwrap(),
            ));
            Ok(())
        }
        fn participant_access(
            &self,
            room: &str,
            identity: &str,
            can_publish: bool,
        ) -> Result<crate::routes::livekit_admin::ParticipantAccess, LiveKitError> {
            Ok(crate::routes::livekit_admin::ParticipantAccess {
                token: format!("tok:{room}:{identity}:{can_publish}"),
                url: Self::PUBLIC.to_string(),
            })
        }
    }

    impl FakeLiveKit {
        const PUBLIC: &'static str = "wss://livekit.test";
    }

    // ---- scaffolding: schema-per-test pool with the 023 tables, on top of
    // the same test_support ApiState the other namespace tests use.

    const VOICE_TABLES: &[&str] = &[
        r#"CREATE TABLE voice_bot_config (
             bot_id TEXT PRIMARY KEY,
             user_id TEXT NOT NULL,
             name TEXT,
             persona TEXT NOT NULL DEFAULT 'A helpful AI assistant.',
             voice_id TEXT NOT NULL DEFAULT 'allternit-default',
             greeting TEXT NOT NULL DEFAULT 'How can I help you today?',
             recording TEXT NOT NULL DEFAULT 'off' CHECK (recording IN ('off','consented')),
             transfer_targets JSONB NOT NULL DEFAULT '[]'::jsonb,
             updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
           )"#,
        r#"CREATE TABLE voice_calls (
             call_id TEXT PRIMARY KEY,
             user_id TEXT NOT NULL,
             runtime_id TEXT NOT NULL,
             number_id TEXT,
             bot_id TEXT NOT NULL,
             room TEXT NOT NULL,
             direction TEXT NOT NULL,
             from_e164 TEXT NOT NULL,
             to_e164 TEXT NOT NULL,
             sip_call_id TEXT,
             consent_ref TEXT,
             started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
             line JSONB,
             line_kind TEXT,
             ended_at TIMESTAMPTZ,
             thread_id TEXT
           )"#,
        r#"CREATE TABLE voice_linked_call_rooms (
             room TEXT PRIMARY KEY,
             owner_id TEXT NOT NULL,
             runtime_id TEXT NOT NULL,
             bot_id TEXT NOT NULL,
             line JSONB NOT NULL,
             direction TEXT NOT NULL CHECK (direction IN ('inbound','outbound')),
             from_number TEXT,
             to_number TEXT,
             created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
             ended_at TIMESTAMPTZ
           )"#,
        r#"CREATE TABLE voice_call_events (
             id BIGSERIAL PRIMARY KEY,
             call_id TEXT NOT NULL REFERENCES voice_calls(call_id) ON DELETE CASCADE,
             event_type TEXT NOT NULL,
             n INTEGER NOT NULL,
             seq BIGINT NOT NULL DEFAULT 0,
             at_ms BIGINT,
             event_key TEXT NOT NULL UNIQUE,
             payload JSONB NOT NULL,
             received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
             attempts INTEGER NOT NULL DEFAULT 0,
             next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
             locked_until TIMESTAMPTZ,
             delivered_at TIMESTAMPTZ,
             dead_at TIMESTAMPTZ,
             last_status INTEGER,
             last_error TEXT
           )"#,
    ];

    async fn voice_test_state() -> Arc<ApiState> {
        let state = test_support::test_state(Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![],
        )))
        .await;
        for statement in VOICE_TABLES {
            sqlx::query(statement).execute(&state.db).await.unwrap();
        }
        // A call start first asks whether a Platform API project owns the
        // number (`platform_v1::calls::worker_start`: phone_numbers,
        // platform_calls, platform_agents).
        test_support::platform_schema(&state.db).await;
        state
    }

    async fn save_bot_config(state: &ApiState, bot_id: &str, recording: &str) {
        sqlx::query(
            "INSERT INTO voice_bot_config (bot_id, user_id, name, persona, voice_id, greeting, recording)
             VALUES ($1, 'user-1', 'Test Bot', 'Test persona', 'voice-x', 'Hi there', $2)",
        )
        .bind(bot_id)
        .bind(recording)
        .execute(&state.db)
        .await
        .unwrap();
    }

    fn start_body(number_id: &str, bot_id: &str) -> StartCall {
        StartCall {
            bot_id: bot_id.to_string(),
            number_id: Some(number_id.to_string()),
            line: None,
            from: Some("+15550001111".to_string()),
            to: Some("+15550002222".to_string()),
            direction: "inbound".to_string(),
            room: "call-room-1".to_string(),
            owner_id: None,
            sip_call_id: None,
            consent_ref: None,
        }
    }

    fn worker_event(call_id: &str, event_type: &str, n: i64, seq: i64, payload: Value) -> EventIn {
        EventIn {
            event_type: event_type.to_string(),
            idempotency_key: format!("call:{call_id}:{event_type}:{n}"),
            seq,
            at_ms: Some(1_700_000_000_000 + seq),
            payload,
        }
    }

    // ---- tests

    #[test]
    fn worker_token_gate() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer wrong".parse().unwrap());
        // Unset env: 503-class error, never a crash.
        let err = check_worker_token(&headers, None).unwrap_err();
        assert!(matches!(err, ApiError::ServiceUnavailable(_)));
        // Wrong token: 401.
        let err = check_worker_token(&headers, Some("sekrit")).unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)));
        // Right token, and a constant-time compare that accepts it.
        headers.insert("authorization", "Bearer sekrit".parse().unwrap());
        assert!(check_worker_token(&headers, Some("sekrit")).is_ok());
        // Same length, different token: still rejected.
        headers.insert("authorization", "Bearer sekr!t".parse().unwrap());
        assert!(check_worker_token(&headers, Some("sekrit")).is_err());
    }

    #[tokio::test]
    async fn an_inbound_call_answers_as_the_numbers_current_bot_not_the_dispatch_rules() {
        let state = voice_test_state().await;
        save_bot_config(&state, "bot-1", "off").await;
        sqlx::query("INSERT INTO voice_bot_config (bot_id, user_id, name, persona, voice_id, greeting, recording) VALUES ('bot-2', 'user-1', 'A://', 'Main persona', 'voice-y', 'Hello from main', 'off')")
            .execute(&state.db)
            .await
            .unwrap();
        let directory = FakeDirectory::default();
        // The number was moved to bot-2; the SIP dispatch rule still says bot-1.
        directory.owners.lock().unwrap().insert(
            "number-9".to_string(),
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string(), bot_id: Some("bot-2".to_string()) },
        );
        let response = start_call_inner(&state, &directory, start_body("number-9", "bot-1")).await.unwrap();
        let answer: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert_eq!(answer["bot"]["greeting"], "Hello from main");
        let stored: String = sqlx::query_scalar("SELECT bot_id FROM voice_calls WHERE call_id = $1").bind(answer["callId"].as_str().unwrap()).fetch_one(&state.db).await.unwrap();
        assert_eq!(stored, "bot-2");

        // An outbound call keeps the bot that placed it.
        let mut out = start_body("number-9", "bot-1");
        out.direction = "outbound".to_string();
        out.room = "call-room-2".to_string();
        out.consent_ref = Some("cc_1".to_string());
        let response = start_call_inner(&state, &directory, out).await.unwrap();
        let answer: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert_eq!(answer["bot"]["greeting"], "Hi there");
    }

    #[tokio::test]
    async fn immediate_answer_from_cache_while_runtime_asleep_then_in_order_delivery() {
        let state = voice_test_state().await;
        save_bot_config(&state, "bot-1", "consented").await;
        let directory = FakeDirectory::default();
        directory.owners.lock().unwrap().insert(
            "number-9".to_string(),
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string(), bot_id: None },
        );
        let relay = FakeRelay::asleep_then_awake();

        // The answer comes from the cache even though the runtime is asleep.
        let response = start_call_inner(&state, &directory, start_body("number-9", "bot-1"))
            .await
            .unwrap();
        let answer: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap(),
        )
        .unwrap();
        let call_id = answer["callId"].as_str().unwrap().to_string();
        assert!(!call_id.is_empty());
        assert_eq!(answer["bot"]["persona"], "Test persona");
        assert_eq!(answer["bot"]["voiceId"], "voice-x");
        assert_eq!(answer["bot"]["greeting"], "Hi there");
        assert_eq!(answer["bot"]["recording"], true);
        assert_eq!(answer["bot"]["name"], "Test Bot");

        // The worker's own call.started (same key as the cloud's) is a
        // duplicate: 409, which the worker counts as delivered.
        let dup = post_events_inner(
            &state,
            &call_id,
            Some(&format!("call:{call_id}:call.started:1")),
            worker_event(&call_id, "call.started", 1, 1, json!({"direction": "inbound"})),
        )
        .await
        .unwrap();
        assert_eq!(dup.status(), StatusCode::CONFLICT);

        // Worker posts two more events, one per request, out of arrival order
        // (seq 3 lands before seq 2); delivery follows seq.
        for event in [
            worker_event(&call_id, "call.dtmf", 1, 3, json!({ "digits": "5", "from": "caller" })),
            worker_event(
                &call_id,
                "call.transcript.delta",
                1,
                2,
                json!({ "speaker": "caller", "text": "hi", "final": true, "segmentId": "s1" }),
            ),
        ] {
            let key = event.idempotency_key.clone();
            let posted = post_events_inner(&state, &call_id, Some(&key), event).await.unwrap();
            assert_eq!(posted.status(), StatusCode::ACCEPTED);
        }

        // start_call/post_events also launch a real delivery pass in the
        // background (no runtime in tests, so it fails and backs off). Let
        // those settle, then make every event due again so only the fake
        // relay below decides the outcome.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let make_due = || async {
            sqlx::query("UPDATE voice_call_events SET next_attempt_at = now(), locked_until = NULL WHERE call_id = $1")
                .bind(&call_id)
                .execute(&state.db)
                .await
                .unwrap();
        };
        make_due().await;

        // First delivery pass: runtime asleep (503) — nothing delivered.
        deliver_call(&state, &call_id, &relay).await.unwrap();
        assert_eq!(relay.recorded().len(), 1, "one attempt (call.started), refused with 503");
        let pending: (i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE delivered_at IS NULL),
                    count(*) FILTER (WHERE delivered_at IS NOT NULL)
               FROM voice_call_events WHERE call_id = $1",
        )
        .bind(&call_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(pending, (3, 0));

        // Runtime wakes. The failed pass scheduled a backoff, so bring the
        // retry time forward instead of sleeping; the next pass delivers
        // everything, call.started first.
        make_due().await;
        deliver_call(&state, &call_id, &relay).await.unwrap();
        let recorded: Vec<(String, Value)> = relay.recorded().into_iter().skip(1).collect();
        let paths: Vec<&str> = recorded.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "/api/v1/voice/calls",
                format!("/api/v1/voice/calls/{call_id}/events").as_str(),
                format!("/api/v1/voice/calls/{call_id}/events").as_str(),
            ]
        );
        // call.started becomes the runtime's create body, exactly its fields.
        let create = recorded[0].1.as_object().unwrap();
        let mut keys: Vec<&str> = create.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["botId", "callId", "direction", "from", "line", "numberId", "ownerId", "room", "startedAt", "to"]
        );
        assert_eq!(create["callId"], call_id);
        assert_eq!(create["line"], json!({ "kind": "number", "numberId": "number-9" }));
        assert_eq!(create["botId"], "bot-1");
        assert_eq!(create["ownerId"], "user-1");
        assert_eq!(create["numberId"], "number-9");
        assert_eq!(create["direction"], "inbound");
        assert_eq!(create["room"], "call-room-1");
        assert!(chrono::DateTime::parse_from_rfc3339(create["startedAt"].as_str().unwrap()).is_ok());
        // Everything else is a one-event `{events:[{type,n,payload,occurredAt}]}`.
        let transcript = &recorded[1].1["events"][0];
        assert_eq!(recorded[1].1["events"].as_array().unwrap().len(), 1);
        assert_eq!(transcript["type"], "call.transcript.delta");
        assert_eq!(transcript["n"], 1);
        assert_eq!(transcript["payload"]["speaker"], "caller");
        assert!(chrono::DateTime::parse_from_rfc3339(transcript["occurredAt"].as_str().unwrap()).is_ok());
        let dtmf = &recorded[2].1["events"][0];
        assert_eq!(dtmf["type"], "call.dtmf");
        assert_eq!(dtmf["payload"]["digits"], "5");

        let delivered: (i64,) =
            sqlx::query_as("SELECT count(*) FROM voice_call_events WHERE call_id = $1 AND delivered_at IS NOT NULL")
                .bind(&call_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(delivered.0, 3);
    }

    /// A runtime 404 ("that number / call isn't known here yet") is not "delivered": it retries
    /// while the number may still sync, then the event goes dead with the reason; an auth refusal
    /// dies at once. Events behind a dead one still go through in order.
    #[tokio::test]
    async fn runtime_404_retries_then_dies_and_auth_refusals_die_at_once() {
        for (refusal, passes) in [(404u16, crate::routes::channel_inbound::NOT_FOUND_MAX_ATTEMPTS), (401u16, 1)] {
            let state = voice_test_state().await;
            save_bot_config(&state, "bot-1", "consented").await;
            let directory = FakeDirectory::default();
            directory.owners.lock().unwrap().insert(
                "number-9".to_string(),
                NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string(), bot_id: None },
            );
            let response = start_call_inner(&state, &directory, start_body("number-9", "bot-1")).await.unwrap();
            let answer: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
            let call_id = answer["callId"].as_str().unwrap().to_string();
            let dtmf = worker_event(&call_id, "call.dtmf", 1, 2, json!({ "digits": "5", "from": "caller" }));
            let key = dtmf.idempotency_key.clone();
            post_events_inner(&state, &call_id, Some(&key), dtmf).await.unwrap();
            // Let the background delivery passes (no runtime in tests) settle, then start counting from zero.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let reset = || async {
                sqlx::query("UPDATE voice_call_events SET next_attempt_at = now(), locked_until = NULL, attempts = 0 WHERE call_id = $1 AND delivered_at IS NULL AND dead_at IS NULL")
                    .bind(&call_id)
                    .execute(&state.db)
                    .await
                    .unwrap();
            };
            let relay = FakeRelay::default();
            relay.statuses.lock().unwrap().extend(std::iter::repeat(refusal).take(passes as usize));
            reset().await;
            let started_state = || async {
                sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>, Option<i32>, Option<String>)>(
                    "SELECT dead_at, last_status, last_error FROM voice_call_events WHERE call_id = $1 AND event_type = 'call.started'",
                )
                .bind(&call_id)
                .fetch_one(&state.db)
                .await
                .unwrap()
            };
            for pass in 1..=passes {
                assert!(started_state().await.0.is_none(), "{refusal}: still retrying before pass {pass}");
                deliver_call(&state, &call_id, &relay).await.unwrap();
                if pass < passes {
                    sqlx::query("UPDATE voice_call_events SET next_attempt_at = now(), locked_until = NULL WHERE call_id = $1 AND delivered_at IS NULL AND dead_at IS NULL")
                        .bind(&call_id)
                        .execute(&state.db)
                        .await
                        .unwrap();
                }
            }
            let (dead_at, last_status, last_error) = started_state().await;
            assert!(dead_at.is_some(), "{refusal}: dead after {passes} attempt(s)");
            assert_eq!(last_status, Some(i32::from(refusal)));
            assert_eq!(last_error.as_deref(), Some(format!("runtime answered {refusal}").as_str()));
            let delivered_started: i64 = sqlx::query_scalar("SELECT count(*) FROM voice_call_events WHERE call_id = $1 AND event_type = 'call.started' AND delivered_at IS NOT NULL")
                .bind(&call_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
            assert_eq!(delivered_started, 0, "{refusal}: never reported delivered");
            // The next event was not held hostage by the dead one.
            let behind: i64 = sqlx::query_scalar("SELECT count(*) FROM voice_call_events WHERE call_id = $1 AND event_type = 'call.dtmf' AND delivered_at IS NOT NULL")
                .bind(&call_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
            assert_eq!(behind, 1, "{refusal}: later events still delivered");
        }
    }

    #[tokio::test]
    async fn missing_bot_config_answers_with_safe_defaults() {
        let state = voice_test_state().await;
        let bot = load_bot_config(&state.db, "never-saved").await.unwrap();
        assert_eq!(bot.persona, DEFAULT_PERSONA);
        assert_eq!(bot.voice_id, DEFAULT_VOICE_ID);
        assert!(!bot.recording);
        assert_eq!(bot.name, None);
    }

    #[tokio::test]
    async fn duplicate_event_keys_are_dropped() {
        let state = voice_test_state().await;
        let directory = FakeDirectory::default();
        directory.owners.lock().unwrap().insert(
            "number-9".to_string(),
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string(), bot_id: None },
        );
        let response =
            start_call_inner(&state, &directory, start_body("number-9", "bot-x")).await.unwrap();
        let call_id: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap(),
        )
        .unwrap();
        let call_id = call_id["callId"].as_str().unwrap().to_string();

        let event = || worker_event(&call_id, "call.transcript.delta", 1, 2, json!({ "text": "once", "final": true }));
        let key = format!("call:{call_id}:call.transcript.delta:1");
        let first = post_events_inner(&state, &call_id, Some(&key), event()).await.unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        // Same key redelivered (a worker retry) must not double: 409.
        let second = post_events_inner(&state, &call_id, Some(&key), event()).await.unwrap();
        assert_eq!(second.status(), StatusCode::CONFLICT);
        let rows: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM voice_call_events WHERE call_id = $1 AND event_type = 'call.transcript.delta'",
        )
        .bind(&call_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(rows.0, 1);
    }

    #[tokio::test]
    async fn control_is_published_to_the_call_control_topic() {
        let state = voice_test_state().await;
        sqlx::query(
            "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164)
             VALUES ('call-1', 'user-1', 'rt-1', 'number-9', 'bot-1', 'room-7', 'inbound', '+1', '+2')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let livekit = FakeLiveKit::default();
        let response = control_inner(
            &state,
            "user-1",
            "call-1",
            json!({ "action": "dtmf", "digits": "42" }),
            &livekit,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let sent = livekit.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "room-7");
        assert_eq!(sent[0].1, "allternit.call.control");
        assert_eq!(sent[0].2["callId"], "call-1");
        assert_eq!(sent[0].2["action"], "dtmf");
        assert_eq!(sent[0].2["digits"], "42");
    }

    async fn seed_call(state: &ApiState) {
        sqlx::query(
            "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164)
             VALUES ('call-1', 'user-1', 'rt-1', 'number-9', 'bot-1', 'room-7', 'inbound', '+1', '+2')",
        )
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn body_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn listen_returns_receive_only_token_and_publishes_nothing() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let livekit = FakeLiveKit::default();
        let response = control_inner(&state, "user-1", "call-1", json!({ "action": "listen" }), &livekit)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["room"], "room-7");
        assert_eq!(body["url"], FakeLiveKit::PUBLIC);
        assert!(body["token"].as_str().unwrap().ends_with(":false"), "receive-only");
        assert!(livekit.sent.lock().unwrap().is_empty());
        // another user cannot listen in
        assert!(control_inner(&state, "user-2", "call-1", json!({ "action": "listen" }), &livekit)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn takeover_returns_publish_token_and_forwards_release() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let livekit = FakeLiveKit::default();
        let response = control_inner(&state, "user-1", "call-1", json!({ "action": "takeover" }), &livekit)
            .await
            .unwrap();
        let body = body_json(response).await;
        assert!(body["token"].as_str().unwrap().ends_with(":true"));
        assert_eq!(body["url"], FakeLiveKit::PUBLIC);
        assert_eq!(body["room"], "room-7");
        let response = control_inner(&state, "user-1", "call-1", json!({ "action": "release" }), &livekit)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let sent = livekit.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|s| s.1 == "allternit.call.control"));
        assert_eq!(sent[0].2["action"], "takeover");
        assert_eq!(sent[1].2["action"], "release");
    }

    #[tokio::test]
    async fn control_rejects_bad_input_and_non_owner() {
        let state = voice_test_state().await;
        sqlx::query(
            "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164)
             VALUES ('call-1', 'user-1', 'rt-1', 'number-9', 'bot-1', 'room-7', 'inbound', '+1', '+2')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let livekit = FakeLiveKit::default();
        // Unknown action.
        let err = control_inner(&state, "user-1", "call-1", json!({ "action": "explode" }), &livekit)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
        // dtmf without digits.
        let err = control_inner(&state, "user-1", "call-1", json!({ "action": "dtmf" }), &livekit)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
        // transfer without a number.
        let err = control_inner(&state, "user-1", "call-1", json!({ "action": "transfer" }), &livekit)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
        // A different user sees 404, not 403 — the call's existence stays hidden.
        let err = control_inner(&state, "user-2", "call-1", json!({ "action": "hangup" }), &livekit)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
        // Nothing was published for any of the rejected controls.
        assert!(livekit.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn runtime_proxy_only_allows_the_handoff_paths() {
        let state = voice_test_state().await;
        sqlx::query(
            "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164)
             VALUES ('call-1', 'user-1', 'rt-1', 'number-9', 'bot-1', 'room-7', 'inbound', '+1', '+2')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let relay = FakeRelay::default();
        let ok = runtime_proxy_inner(
            &state,
            &relay,
            "call-1",
            RuntimeProxyRequest {
                method: None,
                path: "/api/v1/tools/execute".to_string(),
                body: Some(json!({ "name": "calendar.get" })),
            },
        )
        .await
        .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let recorded = relay.recorded();
        assert_eq!(recorded[0].0, "/api/v1/tools/execute");
        assert_eq!(recorded[0].1["name"], "calendar.get");

        let err = runtime_proxy_inner(
            &state,
            &relay,
            "call-1",
            RuntimeProxyRequest {
                method: Some("DELETE".to_string()),
                path: "/api/v1/tools/execute".to_string(),
                body: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
        let err = runtime_proxy_inner(
            &state,
            &relay,
            "call-1",
            RuntimeProxyRequest {
                method: None,
                path: "/api/v1/secrets".to_string(),
                body: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
        let err = runtime_proxy_inner(
            &state,
            &relay,
            "call-nope",
            RuntimeProxyRequest { method: None, path: "/api/v1/tools/execute".to_string(), body: None },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn events_for_unknown_call_are_not_found() {
        let state = voice_test_state().await;
        let err = post_events_inner(
            &state,
            "nope",
            None,
            worker_event("nope", "call.dtmf", 1, 1, json!({})),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn bot_config_upsert_keeps_fields_and_validates_recording() {
        let state = voice_test_state().await;
        sqlx::query(
            "INSERT INTO voice_bot_config (bot_id, user_id, persona, voice_id, greeting, recording)
             VALUES ('bot-1', 'user-1', 'P', 'v', 'G', 'consented')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        // Partial save: only the greeting changes.
        let row: (String, String, String, String) = sqlx::query_as(
            "INSERT INTO voice_bot_config (bot_id, user_id, persona, voice_id, greeting, recording)
             VALUES ('bot-1', 'user-1', 'ignored', 'ignored', 'New greeting', 'off')
             ON CONFLICT (bot_id) DO UPDATE SET
                 user_id = EXCLUDED.user_id,
                 persona = COALESCE(NULLIF(EXCLUDED.persona,'ignored'), voice_bot_config.persona),
                 voice_id = COALESCE(NULLIF(EXCLUDED.voice_id,'ignored'), voice_bot_config.voice_id),
                 greeting = COALESCE(NULLIF(EXCLUDED.greeting,'ignored'), voice_bot_config.greeting),
                 recording = COALESCE(NULLIF(EXCLUDED.recording,'off'), voice_bot_config.recording),
                 updated_at = now()
             RETURNING persona, voice_id, greeting, recording",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
        // This mirrors the production upsert's COALESCE behavior at the SQL
        // level; the handler enforces the recording enum before this point.
        assert_eq!(row.0, "P");
        assert_eq!(row.1, "v");
        assert_eq!(row.2, "New greeting");
        assert_eq!(row.3, "consented");
    }

    #[tokio::test]
    async fn start_call_checks_the_claimed_owner_and_stores_worker_fields() {
        let state = voice_test_state().await;
        let directory = FakeDirectory::default();
        directory.owners.lock().unwrap().insert(
            "number-9".to_string(),
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string(), bot_id: None },
        );
        let mut body = start_body("number-9", "bot-1");
        body.owner_id = Some("someone-else".to_string());
        let err = start_call_inner(&state, &directory, body).await.unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));

        let mut body = start_body("number-9", "bot-1");
        body.owner_id = Some("user-1".to_string());
        body.sip_call_id = Some("sip-abc".to_string());
        body.consent_ref = Some("consent-7".to_string());
        let response = start_call_inner(&state, &directory, body).await.unwrap();
        let answer: Value =
            serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap();
        // No saved config: safe defaults, bool recording, name absent.
        assert_eq!(answer["bot"]["recording"], false);
        assert_eq!(answer["bot"]["name"], Value::Null);
        let row: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT sip_call_id, consent_ref FROM voice_calls WHERE call_id = $1",
        )
        .bind(answer["callId"].as_str().unwrap())
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(row, (Some("sip-abc".to_string()), Some("consent-7".to_string())));
    }

    #[tokio::test]
    async fn event_requests_are_validated() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let bad = |event: EventIn, header: Option<&'static str>| {
            let state = state.clone();
            async move { post_events_inner(&state, "call-1", header, event).await.unwrap_err() }
        };
        // Not a call.* type, bad seq, key that does not match the type/call,
        // and an Idempotency-Key header that disagrees with the body.
        let mut e = worker_event("call-1", "call.dtmf", 1, 1, json!({}));
        e.event_type = "other.thing".into();
        assert!(matches!(bad(e, None).await, ApiError::BadRequest(_)));
        assert!(matches!(
            bad(worker_event("call-1", "call.dtmf", 1, 0, json!({})), None).await,
            ApiError::BadRequest(_)
        ));
        let mut e = worker_event("call-1", "call.dtmf", 1, 1, json!({}));
        e.idempotency_key = "call:call-2:call.dtmf:1".into();
        assert!(matches!(bad(e, None).await, ApiError::BadRequest(_)));
        assert!(matches!(
            bad(worker_event("call-1", "call.dtmf", 1, 1, json!({})), Some("call:call-1:call.dtmf:9")).await,
            ApiError::BadRequest(_)
        ));
    }

    fn turn_in(text: &str) -> TurnIn {
        TurnIn {
            call_id: Some("call-1".to_string()),
            turn_id: "turn:call-1:1".to_string(),
            text: text.to_string(),
            confidence: Some(0.9),
        }
    }

    async fn ndjson_lines(response: Response) -> Vec<Value> {
        assert_eq!(response.headers()["content-type"], "application/x-ndjson");
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn turn_relays_sse_as_ndjson() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let relay = Arc::new(FakeRelay::default());
        relay.sse_ends.store(true, std::sync::atomic::Ordering::SeqCst);
        // Frames split across chunks, a keep-alive comment, tool progress the
        // caller never hears, CRLF framing, then done.
        *relay.sse.lock().unwrap() = vec![
            ": keep-alive\n\n".into(),
            "data: {\"type\":\"tool.started\",\"name\":\"calendar\"}\n\ndata: {\"type\":\"text.de".into(),
            "lta\",\"text\":\"Sure. \"}\n\n".into(),
            "data: {\"type\":\"text.delta\",\"text\":\"Done.\"}\r\n\r\n".into(),
            "data: {\"type\":\"done\"}\n\n".into(),
            "data: {\"type\":\"text.delta\",\"text\":\"after done\"}\n\n".into(),
        ];
        let response = turn_inner(&state, relay.clone(), "call-1", turn_in("what time is it")).await.unwrap();
        assert_eq!(
            ndjson_lines(response).await,
            vec![
                json!({"type": "delta", "text": "Sure. "}),
                json!({"type": "delta", "text": "Done."}),
                json!({"type": "done"}),
            ]
        );
        // One runtime call, to the runtime's SSE turn route, with its body.
        let calls = relay.recorded();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "/api/v1/voice/calls/call-1/turn");
        assert_eq!(calls[0].1["text"], "what time is it");
        // Finished normally: no abort is sent when the stream is dropped.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(relay.recorded().len(), 1);
    }

    #[tokio::test]
    async fn turn_error_and_early_hangup_end_with_an_error_line() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let relay = Arc::new(FakeRelay::default());
        relay.sse_ends.store(true, std::sync::atomic::Ordering::SeqCst);
        *relay.sse.lock().unwrap() = vec![
            "data: {\"type\":\"text.delta\",\"text\":\"Hm\"}\n\ndata: {\"type\":\"error\",\"message\":\"model down\"}\n\n".into(),
        ];
        let lines = ndjson_lines(turn_inner(&state, relay.clone(), "call-1", turn_in("hi")).await.unwrap()).await;
        assert_eq!(lines, vec![json!({"type":"delta","text":"Hm"}), json!({"type":"error","message":"model down"})]);

        // The runtime closing the stream before done/error is an error too.
        *relay.sse.lock().unwrap() = vec!["data: {\"type\":\"text.delta\",\"text\":\"Hm\"}\n\n".into()];
        let lines = ndjson_lines(turn_inner(&state, relay.clone(), "call-1", turn_in("hi")).await.unwrap()).await;
        assert_eq!(lines[0], json!({"type":"delta","text":"Hm"}));
        assert_eq!(lines[1]["type"], "error");
    }

    #[tokio::test]
    async fn dropping_the_turn_stream_aborts_the_runtime_turn() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let relay = Arc::new(FakeRelay::default());
        // The runtime keeps working (stream stays open) after the first delta.
        *relay.sse.lock().unwrap() = vec!["data: {\"type\":\"text.delta\",\"text\":\"One. \"}\n\n".into()];
        let response = turn_inner(&state, relay.clone(), "call-1", turn_in("hi")).await.unwrap();
        let mut body = response.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        let first: Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(first, json!({"type": "delta", "text": "One. "}));
        // The worker hangs up (barge-in): the runtime's turn is deleted.
        drop(body);
        for _ in 0..50 {
            if relay.recorded().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let calls = relay.recorded();
        assert_eq!(calls.len(), 2, "the abort must reach the runtime");
        assert_eq!(calls[1].0, "/api/v1/voice/calls/call-1/turn");
        assert_eq!(relay.methods.lock().unwrap().clone(), vec!["POST", "DELETE"]);
    }

    #[tokio::test]
    async fn turn_for_unknown_call_or_empty_text_is_refused() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let relay = Arc::new(FakeRelay::default());
        let err = turn_inner(&state, relay.clone(), "nope", turn_in("hi")).await.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
        let err = turn_inner(&state, relay.clone(), "call-1", turn_in("   ")).await.unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
        assert!(relay.recorded().is_empty());
    }

    #[tokio::test]
    async fn control_carries_the_actor_and_mute_defaults_to_the_bot() {
        let state = voice_test_state().await;
        seed_call(&state).await;
        let livekit = FakeLiveKit::default();
        for body in [json!({"action": "mute"}), json!({"action": "unmute", "target": "caller"}), json!({"action": "takeover"})] {
            control_inner(&state, "user-1", "call-1", body, &livekit).await.unwrap();
        }
        let sent = livekit.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|s| s.2["by"] == "user-1"));
        assert_eq!(sent[0].2["target"], "bot");
        assert_eq!(sent[1].2["target"], "caller");
        assert_eq!(sent[2].2["action"], "takeover");
        assert_eq!(sent[2].2["target"], Value::Null);
        let err = control_inner(&state, "user-1", "call-1", json!({"action": "mute", "target": "nobody"}), &livekit)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[tokio::test]
    async fn warm_transfer_carries_a_consent_ref_only_when_the_cloud_can_issue_one() {
        let state = voice_test_state().await;
        for sql in [include_str!("../../migrations_pg/024_phone_numbers.sql")] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.unwrap();
        }
        sqlx::query("INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier) VALUES ('number-9', 'user-1', 'rt-1', 'bot-1', '+14155550100', 'telnyx')")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164)
             VALUES ('call-1', 'user-1', 'rt-1', 'number-9', 'bot-1', 'room-7', 'inbound', '+1', '+2')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let (desk, other) = ("+15550008888", "+15550009999");
        let livekit = FakeLiveKit::default();
        let ref_of = |i: usize| livekit.sent.lock().unwrap()[i].2.clone();

        // Owner press: consentRef with owner_directed basis.
        control_inner(&state, "user-1", "call-1", json!({ "action": "transfer", "to": other, "mode": "warm" }), &livekit).await.unwrap();
        let sent = ref_of(0);
        let id = sent["consentRef"].as_str().expect("owner transfer carries a consentRef").to_string();
        let basis: String = sqlx::query_scalar("SELECT basis FROM call_consents WHERE id = $1").bind(&id).fetch_one(&state.db).await.unwrap();
        assert_eq!(basis, "owner_directed");

        // Cold transfers need no ref.
        control_inner(&state, "user-1", "call-1", json!({ "action": "transfer", "to": other, "mode": "cold" }), &livekit).await.unwrap();
        assert!(ref_of(1)["consentRef"].is_null());

        // Bot-initiated: off-list = no ref; after the owner approves the target via the config route's helpers = ref.
        control_inner(&state, "user-1", "call-1", json!({ "action": "transfer", "to": desk, "mode": "warm", "initiator": "bot" }), &livekit).await.unwrap();
        assert!(ref_of(2)["consentRef"].is_null(), "bot cannot dial an unapproved target");
        let targets = clean_transfer_targets(&[TransferTarget { e164: desk.into(), label: "Desk".into() }]).unwrap();
        sqlx::query("INSERT INTO voice_bot_config (bot_id, user_id, transfer_targets) VALUES ('bot-1', 'user-1', $1)").bind(json!(targets)).execute(&state.db).await.unwrap();
        record_transfer_targets(&state.db, "user-1", "bot-1", &targets).await.unwrap();
        control_inner(&state, "user-1", "call-1", json!({ "action": "transfer", "to": desk, "mode": "warm", "initiator": "bot" }), &livekit).await.unwrap();
        let id = ref_of(3)["consentRef"].as_str().expect("approved target gets a ref").to_string();
        let basis: String = sqlx::query_scalar("SELECT basis FROM call_consents WHERE id = $1").bind(&id).fetch_one(&state.db).await.unwrap();
        assert_eq!(basis, "owner_transfer_target");
    }


    // ---- linked phone (CONTRACT-linked-phone-voice-2026-10-18 C1–C3)

    fn moto() -> CallLine {
        CallLine::LinkedPhone {
            device_id: "ZY22ABCD".to_string(),
            serial: Some("ZY22ABCD".to_string()),
            label: "Moto".to_string(),
            platform: "android".to_string(),
        }
    }

    #[test]
    fn lines_parse_validate_and_serialize_in_the_contract_shape() {
        let number: CallLine = serde_json::from_value(json!({ "kind": "number", "numberId": "num-1" })).unwrap();
        assert_eq!(number.validated().unwrap(), CallLine::Number { number_id: "num-1".into() });
        assert_eq!(number.to_json(), json!({ "kind": "number", "numberId": "num-1" }));

        let linked: CallLine = serde_json::from_value(json!({
            "kind": "linked_phone", "deviceId": " ZY22ABCD ", "serial": "", "label": " Moto ", "platform": "android"
        }))
        .unwrap();
        let checked = linked.validated().unwrap();
        // Trimmed, and an empty serial is no serial (left out of the JSON).
        assert_eq!(checked.to_json(), json!({ "kind": "linked_phone", "deviceId": "ZY22ABCD", "label": "Moto", "platform": "android" }));
        assert_eq!(checked.kind(), "linked_phone");
        assert_eq!(moto().to_json()["serial"], "ZY22ABCD");

        let bad = |v: Value| serde_json::from_value::<CallLine>(v).map_err(|_| ()).and_then(|l| l.validated().map_err(|_| ()));
        assert!(bad(json!({ "kind": "linked_phone", "deviceId": "x", "label": "Moto", "platform": "windows" })).is_err());
        assert!(bad(json!({ "kind": "linked_phone", "deviceId": "  ", "label": "Moto", "platform": "ios" })).is_err());
        assert!(bad(json!({ "kind": "linked_phone", "deviceId": "a b", "label": "Moto", "platform": "ios" })).is_err());
        assert!(bad(json!({ "kind": "linked_phone", "deviceId": "x", "label": "", "platform": "ios" })).is_err());
        assert!(bad(json!({ "kind": "linked_phone", "deviceId": "x".repeat(201), "label": "Moto", "platform": "ios" })).is_err());
        assert!(bad(json!({ "kind": "number", "numberId": "" })).is_err());
        assert!(bad(json!({ "kind": "landline", "numberId": "n" })).is_err());
        // An iPhone udid with dashes is fine; a label may have spaces.
        assert!(bad(json!({ "kind": "linked_phone", "deviceId": "00008110-001A2B3C", "label": "Eoj's iPhone", "platform": "ios" })).is_ok());
    }

    #[test]
    fn linked_rooms_and_identities_follow_c2() {
        let room = linked_room_name();
        assert!(room.starts_with("call-lp-"));
        assert!(room.starts_with(crate::routes::livekit_admin::CALL_ROOM_PREFIX), "the worker only takes call- rooms");
        assert!(uuid::Uuid::parse_str(room.trim_start_matches(LINKED_ROOM_PREFIX)).is_ok());
        assert_ne!(room, linked_room_name());
        assert_eq!(linked_identity("ZY22ABCD"), "linked-phone-ZY22ABCD");
    }

    #[test]
    fn linked_call_requests_are_checked() {
        let parse = |v: Value| serde_json::from_value::<LinkedCallRequest>(v).unwrap();
        let ok = parse(json!({
            "botId": " bot-1 ", "direction": "inbound", "from": " +15550001111 ", "to": "",
            "line": { "kind": "linked_phone", "deviceId": "ZY22ABCD", "label": "Moto", "platform": "android" }
        }))
        .validated()
        .unwrap();
        assert_eq!((ok.bot_id.as_str(), ok.device_id.as_str(), ok.direction.as_str()), ("bot-1", "ZY22ABCD", "inbound"));
        assert_eq!((ok.from.as_deref(), ok.to.as_deref()), (Some("+15550001111"), None));
        // from/to may be null or missing (withheld caller).
        let withheld = parse(json!({ "botId": "b", "direction": "outbound", "from": null,
            "line": { "kind": "linked_phone", "deviceId": "d", "label": "Moto", "platform": "ios" } }));
        assert_eq!(withheld.validated().unwrap().from, None);

        let line = json!({ "kind": "linked_phone", "deviceId": "d", "label": "Moto", "platform": "ios" });
        for bad in [
            json!({ "botId": "", "direction": "inbound", "line": line }),
            json!({ "botId": "b", "direction": "sideways", "line": line }),
            json!({ "botId": "b", "direction": "inbound", "line": { "kind": "number", "numberId": "n" } }),
            json!({ "botId": "b", "direction": "inbound", "line": line, "from": "x".repeat(65) }),
        ] {
            assert!(matches!(parse(bad).validated(), Err(ApiError::BadRequest(_))));
        }
    }

    #[test]
    fn stored_lines_thread_replies_and_billed_seconds() {
        assert_eq!(stored_line(None, Some("num-1")), json!({ "kind": "number", "numberId": "num-1" }));
        assert_eq!(stored_line(Some(moto().to_json()), None), moto().to_json());
        assert_eq!(stored_line(Some(Value::Null), None), Value::Null);
        assert_eq!(thread_id_from_reply(br#"{"threadId":"thr-1","sessionId":"s"}"#).as_deref(), Some("thr-1"));
        assert_eq!(thread_id_from_reply(br#"{"threadId":""}"#), None);
        assert_eq!(thread_id_from_reply(b"not json"), None);
        let t0 = chrono::Utc::now();
        assert_eq!(linked_billed_seconds(t0, t0 + chrono::Duration::milliseconds(61_900)), 61);
        assert_eq!(linked_billed_seconds(t0, t0 - chrono::Duration::seconds(5)), 0);
        assert_eq!(linked_billed_seconds(t0, t0 + chrono::Duration::hours(30)), LINKED_MAX_BILLED_SECS as i32);
    }

    async fn linked_test_state(plan: Option<&str>) -> Arc<ApiState> {
        let state = voice_test_state().await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/029_voice_usage.sql").replace("public.", ""))
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE billing_subscriptions (user_id TEXT, plan_id TEXT, plan_tier TEXT, \
             status TEXT, updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        )
        .execute(&state.db)
        .await
        .unwrap();
        if let Some(plan) = plan {
            sqlx::query("INSERT INTO billing_subscriptions (user_id, plan_id, plan_tier, status) VALUES ('user-1', $1, 'pro', 'active')")
                .bind(plan)
                .execute(&state.db)
                .await
                .unwrap();
        }
        for (id, user, seen) in [("rt-old", "user-1", "1 hour"), ("rt-new", "user-1", "1 minute"), ("rt-x", "user-2", "1 second")] {
            sqlx::query(&format!("INSERT INTO runtime_devices (id, user_id, name, last_seen_at) VALUES ($1, $2, 'Mac', now() - interval '{seen}')"))
                .bind(id)
                .bind(user)
                .execute(&state.db)
                .await
                .unwrap();
        }
        state
    }

    fn linked_request(bot: &str) -> LinkedCallRequest {
        serde_json::from_value(json!({
            "botId": bot, "direction": "inbound", "from": "+15550001111", "to": null,
            "line": moto().to_json(),
        }))
        .unwrap()
    }

    fn desktop(runtime: &str) -> LinkedCaller {
        LinkedCaller { owner_id: "user-1".into(), runtime_id: Some(runtime.into()) }
    }

    #[tokio::test]
    async fn linked_call_opens_a_room_with_agent_dispatch_and_a_caller_token() {
        let state = linked_test_state(Some("plus")).await;
        let livekit = FakeLiveKit::default();
        let answer = linked_call_inner(&state.db, &desktop("rt-desk"), &linked_request("bot-1"), &livekit).await.unwrap();
        let room = answer["room"].as_str().unwrap().to_string();
        assert!(room.starts_with("call-lp-"));
        assert_eq!(answer["identity"], "linked-phone-ZY22ABCD");
        assert_eq!(answer["livekitUrl"], FakeLiveKit::PUBLIC);
        assert_eq!(answer["token"], format!("tok:{room}:linked-phone-ZY22ABCD:true"));
        // Publish + subscribe, four hours.
        assert_eq!(livekit.tokens.lock().unwrap()[0], (room.clone(), "linked-phone-ZY22ABCD".to_string(), true, 4 * 3600));
        let rooms = livekit.rooms.lock().unwrap().clone();
        assert_eq!(rooms.len(), 1);
        assert_eq!((rooms[0].0.as_str(), rooms[0].1.as_str()), (room.as_str(), "allternit-voice"));
        assert_eq!(
            rooms[0].2,
            json!({ "source": "linked_phone", "botId": "bot-1", "ownerId": "user-1", "line": moto().to_json(),
                    "direction": "inbound", "from": "+15550001111", "to": null })
        );
        let row: (String, String, String, Value, String) =
            sqlx::query_as("SELECT owner_id, runtime_id, bot_id, line, direction FROM voice_linked_call_rooms WHERE room = $1")
                .bind(&room)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(row, ("user-1".into(), "rt-desk".into(), "bot-1".into(), moto().to_json(), "inbound".into()));

        // A signed-in user (no device token) goes to their most recently seen runtime, or the named one if theirs.
        let user = LinkedCaller { owner_id: "user-1".into(), runtime_id: None };
        let answer = linked_call_inner(&state.db, &user, &linked_request("bot-1"), &livekit).await.unwrap();
        let rt: String = sqlx::query_scalar("SELECT runtime_id FROM voice_linked_call_rooms WHERE room = $1")
            .bind(answer["room"].as_str().unwrap()).fetch_one(&state.db).await.unwrap();
        assert_eq!(rt, "rt-new");
        let mut named = linked_request("bot-1");
        named.runtime_id = Some("rt-old".into());
        let answer = linked_call_inner(&state.db, &user, &named, &livekit).await.unwrap();
        let rt: String = sqlx::query_scalar("SELECT runtime_id FROM voice_linked_call_rooms WHERE room = $1")
            .bind(answer["room"].as_str().unwrap()).fetch_one(&state.db).await.unwrap();
        assert_eq!(rt, "rt-old");
        named.runtime_id = Some("rt-x".into());
        assert!(matches!(linked_call_inner(&state.db, &user, &named, &livekit).await, Err(ApiError::NotFound(_))));
    }

    #[tokio::test]
    async fn linked_call_needs_a_voice_plan_the_owners_bot_and_a_working_livekit() {
        let free = linked_test_state(None).await;
        let livekit = FakeLiveKit::default();
        let err = linked_call_inner(&free.db, &desktop("rt-desk"), &linked_request("bot-1"), &livekit).await.unwrap_err();
        assert!(matches!(err, ApiError::PaymentRequired(_)));
        assert!(livekit.rooms.lock().unwrap().is_empty());

        let state = linked_test_state(Some("super")).await;
        sqlx::query("INSERT INTO voice_bot_config (bot_id, user_id) VALUES ('bot-theirs', 'user-2'), ('bot-mine', 'user-1')")
            .execute(&state.db)
            .await
            .unwrap();
        let err = linked_call_inner(&state.db, &desktop("rt-desk"), &linked_request("bot-theirs"), &livekit).await.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
        assert!(linked_call_inner(&state.db, &desktop("rt-desk"), &linked_request("bot-mine"), &livekit).await.is_ok());

        // LiveKit refuses the room: nothing is left behind for the worker to trust.
        livekit.fail_rooms.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(linked_call_inner(&state.db, &desktop("rt-desk"), &linked_request("bot-mine"), &livekit).await.is_err());
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM voice_linked_call_rooms").fetch_one(&state.db).await.unwrap();
        assert_eq!(rows, 1);
    }

    fn linked_start(room: &str) -> StartCall {
        StartCall {
            bot_id: "bot-from-worker".to_string(),
            number_id: None,
            line: Some(moto()),
            from: None,
            to: None,
            direction: "inbound".to_string(),
            room: room.to_string(),
            owner_id: Some("user-1".to_string()),
            sip_call_id: None,
            consent_ref: None,
        }
    }

    #[tokio::test]
    async fn a_linked_phone_start_is_trusted_only_for_a_room_desktop_opened() {
        let state = linked_test_state(Some("plus")).await;
        save_bot_config(&state, "bot-1", "off").await;
        let livekit = FakeLiveKit::default();
        let opened = linked_call_inner(&state.db, &desktop("rt-desk"), &linked_request("bot-1"), &livekit).await.unwrap();
        let room = opened["room"].as_str().unwrap().to_string();
        let directory = FakeDirectory::default();

        // Unknown room, wrong owner, wrong phone, non-call-lp room: refused, nothing stored.
        let err = start_call_inner(&state, &directory, linked_start("call-lp-nope")).await.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
        let mut other_owner = linked_start(&room);
        other_owner.owner_id = Some("user-2".into());
        assert!(matches!(start_call_inner(&state, &directory, other_owner).await, Err(ApiError::Forbidden(_))));
        let mut other_phone = linked_start(&room);
        other_phone.line = Some(CallLine::LinkedPhone { device_id: "OTHER".into(), serial: None, label: "Moto".into(), platform: "android".into() });
        assert!(matches!(start_call_inner(&state, &directory, other_phone).await, Err(ApiError::Forbidden(_))));
        assert!(matches!(start_call_inner(&state, &directory, linked_start("call-room-1")).await, Err(ApiError::BadRequest(_))));

        // The room's call: owner, runtime and bot come from the room row, not the worker.
        let response = start_call_inner(&state, &directory, linked_start(&room)).await.unwrap();
        let answer = body_json(response).await;
        assert_eq!(answer["bot"]["greeting"], "Hi there");
        let call_id = answer["callId"].as_str().unwrap().to_string();
        let row: (String, String, Option<String>, String, Option<Value>, Option<String>, String, String) = sqlx::query_as(
            "SELECT user_id, runtime_id, number_id, bot_id, line, line_kind, from_e164, to_e164 FROM voice_calls WHERE call_id = $1",
        )
        .bind(&call_id)
        .fetch_one(&state.db)
        .await
        .unwrap();
        assert_eq!(
            row,
            ("user-1".into(), "rt-desk".into(), None, "bot-1".into(), Some(moto().to_json()), Some("linked_phone".into()), "+15550001111".into(), "".into())
        );
        let started: Value = sqlx::query_scalar("SELECT payload FROM voice_call_events WHERE call_id = $1 AND event_type = 'call.started'")
            .bind(&call_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(started["line"], moto().to_json());
        assert!(started["numberId"].is_null());

        // A retried start answers with the same call.
        let again = body_json(start_call_inner(&state, &directory, linked_start(&room)).await.unwrap()).await;
        assert_eq!(again["callId"], call_id.as_str());
        let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM voice_calls").fetch_one(&state.db).await.unwrap();
        assert_eq!(calls, 1);

        // Delivery: the runtime's create body has the line and a null numberId; its threadId is kept.
        let relay = FakeRelay::default();
        *relay.reply.lock().unwrap() = Some(r#"{"threadId":"thr-lp","sessionId":"s-1"}"#.to_string());
        // Let the background delivery pass (no runtime in tests) settle, then make the event due.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        sqlx::query("UPDATE voice_call_events SET next_attempt_at = now(), locked_until = NULL").execute(&state.db).await.unwrap();
        deliver_call(&state, &call_id, &relay).await.unwrap();
        let sent = relay.recorded();
        let create = sent.iter().find(|(path, _)| path == "/api/v1/voice/calls").expect("create body relayed");
        assert_eq!(create.1["line"], moto().to_json());
        assert!(create.1["numberId"].is_null());
        assert_eq!(create.1["botId"], "bot-1");
        let thread: Option<String> = sqlx::query_scalar("SELECT thread_id FROM voice_calls WHERE call_id = $1")
            .bind(&call_id)
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(thread.as_deref(), Some("thr-lp"));
        let live = live_calls_for(&state.db, "user-1").await.unwrap();
        assert_eq!(live[0]["threadId"], "thr-lp");
    }

    #[tokio::test]
    async fn a_number_start_still_works_and_now_records_its_line() {
        let state = voice_test_state().await;
        let directory = FakeDirectory::default();
        directory.owners.lock().unwrap().insert(
            "number-9".to_string(),
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string(), bot_id: None },
        );
        // numberId only inside the line (new worker), or only at the top (old worker).
        let mut body = start_body("ignored", "bot-1");
        body.number_id = None;
        body.line = Some(CallLine::Number { number_id: "number-9".into() });
        let answer = body_json(start_call_inner(&state, &directory, body).await.unwrap()).await;
        let (line, kind): (Option<Value>, Option<String>) = sqlx::query_as("SELECT line, line_kind FROM voice_calls WHERE call_id = $1")
            .bind(answer["callId"].as_str().unwrap())
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!((line, kind.as_deref()), (Some(json!({ "kind": "number", "numberId": "number-9" })), Some("number")));
        let started: Value = sqlx::query_scalar("SELECT payload FROM voice_call_events WHERE call_id = $1")
            .bind(answer["callId"].as_str().unwrap())
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!((started["numberId"].as_str(), started["line"]["kind"].as_str()), (Some("number-9"), Some("number")));

        let mut mismatch = start_body("number-9", "bot-1");
        mismatch.line = Some(CallLine::Number { number_id: "number-8".into() });
        assert!(matches!(start_call_inner(&state, &directory, mismatch).await, Err(ApiError::BadRequest(_))));
        let mut none = start_body("x", "bot-1");
        none.number_id = None;
        assert!(matches!(start_call_inner(&state, &directory, none).await, Err(ApiError::BadRequest(_))));
        let mut no_from = start_body("number-9", "bot-1");
        no_from.from = None;
        assert!(matches!(start_call_inner(&state, &directory, no_from).await, Err(ApiError::BadRequest(_))));
    }

    async fn seed_linked_call(state: &ApiState, call_id: &str, room: &str, started_ago_secs: i64) {
        sqlx::query(
            "INSERT INTO voice_linked_call_rooms (room, owner_id, runtime_id, bot_id, line, direction) VALUES ($1, 'user-1', 'rt-1', 'bot-1', $2, 'inbound')",
        )
        .bind(room)
        .bind(moto().to_json())
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164, line, line_kind, started_at)
             VALUES ($1, 'user-1', 'rt-1', NULL, 'bot-1', $2, 'inbound', '', '', $3, 'linked_phone', now() - make_interval(secs => $4))",
        )
        .bind(call_id)
        .bind(room)
        .bind(moto().to_json())
        .bind(started_ago_secs as f64)
        .execute(&state.db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn controls_on_a_linked_line_never_touch_a_number() {
        let state = voice_test_state().await;
        seed_linked_call(&state, "call-lp", "call-lp-room", 5).await;
        let livekit = FakeLiveKit::default();
        // Warm transfer: no number to issue consent against, so no ref; the worker answers unsupported.
        let response = control_inner(&state, "user-1", "call-lp", json!({ "action": "transfer", "to": "+15550009999", "mode": "warm" }), &livekit).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let response = control_inner(&state, "user-1", "call-lp", json!({ "action": "hangup" }), &livekit).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let sent = livekit.sent.lock().unwrap().clone();
        assert!(sent[0].2["consentRef"].is_null());
        assert_eq!((sent[1].0.as_str(), sent[1].2["action"].as_str()), ("call-lp-room", Some("hangup")));
        let listen = body_json(control_inner(&state, "user-1", "call-lp", json!({ "action": "listen" }), &livekit).await.unwrap()).await;
        assert_eq!(listen["room"], "call-lp-room");
    }

    #[tokio::test]
    async fn live_calls_list_the_owners_open_calls_and_ending_bills_linked_seconds() {
        let state = linked_test_state(Some("plus")).await;
        seed_linked_call(&state, "call-lp", "call-lp-room", 90).await;
        seed_call(&state).await; // number call 'call-1' for user-1
        sqlx::query("UPDATE voice_calls SET thread_id = 'thr-9' WHERE call_id = 'call-lp'").execute(&state.db).await.unwrap();
        sqlx::query(
            "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164)
             VALUES ('call-theirs', 'user-2', 'rt-x', 'number-2', 'bot-2', 'room-x', 'inbound', '+1', '+2')",
        )
        .execute(&state.db)
        .await
        .unwrap();

        let live = live_calls_for(&state.db, "user-1").await.unwrap();
        let live = live.as_array().unwrap();
        assert_eq!(live.len(), 2);
        let lp = live.iter().find(|c| c["callId"] == "call-lp").unwrap();
        assert_eq!(lp["line"], moto().to_json());
        assert_eq!((lp["botId"].as_str(), lp["threadId"].as_str(), lp["direction"].as_str()), (Some("bot-1"), Some("thr-9"), Some("inbound")));
        assert!(lp["from"].is_null() && lp["to"].is_null(), "a withheld caller reads null");
        let num = live.iter().find(|c| c["callId"] == "call-1").unwrap();
        assert_eq!(num["line"], json!({ "kind": "number", "numberId": "number-9" }));
        assert!(num["threadId"].is_null());
        assert_eq!(num["from"], "+1");

        // The worker reports the linked call ended: off the live list, room closed, seconds metered as cloud once.
        let ended = worker_event("call-lp", "call.ended", 1, 9, json!({ "reason": "hangup" }));
        let mut ended_now = ended;
        ended_now.at_ms = Some(chrono::Utc::now().timestamp_millis());
        post_events_inner(&state, "call-lp", None, ended_now).await.unwrap();
        // A duplicate never bills twice.
        let mut dup = worker_event("call-lp", "call.ended", 1, 9, json!({}));
        dup.at_ms = Some(chrono::Utc::now().timestamp_millis());
        assert_eq!(post_events_inner(&state, "call-lp", None, dup).await.unwrap().status(), StatusCode::CONFLICT);
        mark_call_ended(&state.db, "call-lp", None).await.unwrap();

        let live = live_calls_for(&state.db, "user-1").await.unwrap();
        assert_eq!(live.as_array().unwrap().iter().map(|c| c["callId"].as_str().unwrap()).collect::<Vec<_>>(), ["call-1"]);
        let room_ended: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT ended_at FROM voice_linked_call_rooms WHERE room = 'call-lp-room'").fetch_one(&state.db).await.unwrap();
        assert!(room_ended.is_some());
        let usage: Vec<(String, String, i32, String)> =
            sqlx::query_as("SELECT user_id, engine, seconds, ref FROM voice_usage").fetch_all(&state.db).await.unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!((usage[0].0.as_str(), usage[0].1.as_str(), usage[0].3.as_str()), ("user-1", "cloud", "linked-call:call-lp"));
        assert!((89..=92).contains(&usage[0].2), "about 90 s, got {}", usage[0].2);

        // A number call's end stamps ended_at but meters nothing here.
        let mut num_end = worker_event("call-1", "call.ended", 1, 3, json!({}));
        num_end.at_ms = None;
        post_events_inner(&state, "call-1", None, num_end).await.unwrap();
        assert!(live_calls_for(&state.db, "user-1").await.unwrap().as_array().unwrap().is_empty());
        let usage: i64 = sqlx::query_scalar("SELECT count(*) FROM voice_usage").fetch_one(&state.db).await.unwrap();
        assert_eq!(usage, 1);
    }

    #[tokio::test]
    async fn a_lost_linked_call_is_billed_to_its_last_event() {
        let state = linked_test_state(Some("plus")).await;
        seed_linked_call(&state, "call-lost", "call-lp-lost", 6 * 3600).await;
        sqlx::query(
            "INSERT INTO voice_call_events (call_id, event_type, n, seq, event_key, payload, received_at, delivered_at)
             VALUES ('call-lost', 'call.started', 1, 0, 'call:call-lost:call.started:1', '{}', now() - interval '6 hours', now()),
                    ('call-lost', 'call.turn', 1, 1, 'call:call-lost:call.turn:1', '{}', now() - interval '5 hours', now())",
        )
        .execute(&state.db)
        .await
        .unwrap();
        assert_eq!(end_lost_calls(&state.db).await.unwrap(), 1);
        let seconds: i32 = sqlx::query_scalar("SELECT seconds FROM voice_usage WHERE ref = 'linked-call:call-lost'").fetch_one(&state.db).await.unwrap();
        assert!((3595..=3605).contains(&seconds), "billed to the last event (~1 h), got {seconds}");
    }

    /// The full router builds (a route overlap panics and crash-loops prod) and
    /// the new routes are mounted next to `/api/v1/voice/calls/:callId/...`.
    #[tokio::test]
    async fn full_router_mounts_linked_calls_and_live_without_overlap() {
        use tower::ServiceExt;
        let state = voice_test_state().await;
        let router = crate::create_router(state);
        let call = |method: &str, path: &str| {
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"botId":"b","direction":"inbound","line":{"kind":"linked_phone","deviceId":"d","label":"Moto","platform":"android"}}"#,
                ))
                .unwrap()
        };
        let r = router.clone().oneshot(call("GET", "/api/v1/voice/calls/live")).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = router.clone().oneshot(call("POST", "/api/v1/voice/linked-calls")).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        // The param routes beside `live` still route to their handlers.
        let r = router.clone().oneshot(call("POST", "/api/v1/voice/calls/live/control")).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = router.clone().oneshot(call("GET", "/api/v1/voice/calls/some-call/recording")).await.unwrap();
        assert_ne!(r.status(), StatusCode::NOT_FOUND);
    }
}
