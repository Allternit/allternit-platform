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
//!
//! Event delivery mirrors `channel_inbound`: per-call in-order, retry with
//! backoff for 24h, wake a sleeping computer, `call.started` first because it
//! is enqueued (n=1) before the worker can post anything else.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{post, put},
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
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT user_id, runtime_id FROM phone_numbers WHERE id = $1")
                .bind(number_id)
                .fetch_optional(&self.db)
                .await?;
        Ok(row.map(|(user_id, runtime_id)| NumberOwner { user_id, runtime_id }))
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
    number_id: String,
    from: String,
    to: String,
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

async fn start_call_inner(
    state: &Arc<ApiState>,
    directory: &dyn PhoneNumberDirectory,
    body: StartCall,
) -> Result<Response, ApiError> {
    if !matches!(body.direction.as_str(), "inbound" | "outbound") {
        return Err(ApiError::BadRequest("direction must be inbound or outbound".to_string()));
    }
    // A Platform API project's number (or realtime session) is answered by its
    // hosted agent: registered and answered by the platform module, not a user's runtime.
    let platform = super::platform_v1::calls::worker_start(
        &state.db,
        super::platform_v1::calls::WorkerStart {
            number_id: &body.number_id,
            room: &body.room,
            direction: &body.direction,
            from: &body.from,
            to: &body.to,
            sip_call_id: body.sip_call_id.as_deref(),
            consent_ref: body.consent_ref.as_deref(),
        },
    )
    .await?;
    if let Some(answer) = platform {
        return Ok(Json(answer).into_response());
    }
    let Some(owner) = directory.owner_for(&body.number_id).await? else {
        return Err(ApiError::NotFound("number not found".to_string()));
    };
    if body.owner_id.as_deref().is_some_and(|claimed| claimed != owner.user_id) {
        return Err(ApiError::Forbidden("ownerId does not match the number's owner".to_string()));
    }
    let call_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164, sip_call_id, consent_ref)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(&call_id)
    .bind(&owner.user_id)
    .bind(&owner.runtime_id)
    .bind(&body.number_id)
    .bind(&body.bot_id)
    .bind(&body.room)
    .bind(&body.direction)
    .bind(&body.from)
    .bind(&body.to)
    .bind(body.sip_call_id.as_deref())
    .bind(body.consent_ref.as_deref())
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
            "direction": body.direction,
            "from": body.from,
            "to": body.to,
            "numberId": body.number_id,
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
    let bot = match tokio::time::timeout(BOT_CONFIG_BUDGET, load_bot_config(&state.db, &body.bot_id)).await {
        Ok(bot) => bot?,
        Err(_) => default_bot(),
    };
    Ok(Json(json!({ "callId": call_id, "bot": bot })).into_response())
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
    let Some((owner_id, room, number_id, bot_id)) = sqlx::query_as::<_, (String, String, String, String)>(
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
        match transfer_consent_ref(&state.db, user_id, &number_id, &bot_id, to, initiator).await {
            Ok(consent) => consent.map(|c| c.id),
            Err(error) => return Err(ApiError::Internal(format!("transfer consent: {error:?}"))),
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
        let Some((user_id, runtime_id, bot_id, number_id, from, to, direction, room, started_at)) =
            sqlx::query_as::<_, (String, String, String, String, String, String, String, String, chrono::DateTime<chrono::Utc>)>(
                "SELECT user_id, runtime_id, bot_id, number_id, from_e164, to_e164, direction, room, started_at
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
            Ok((status, br#"{"ok":true}"#.to_vec()))
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
             number_id TEXT NOT NULL,
             bot_id TEXT NOT NULL,
             room TEXT NOT NULL,
             direction TEXT NOT NULL,
             from_e164 TEXT NOT NULL,
             to_e164 TEXT NOT NULL,
             sip_call_id TEXT,
             consent_ref TEXT,
             started_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
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
        // The number directory reads phone_numbers (pg 024) on every call start.
        sqlx::raw_sql(&include_str!("../../migrations_pg/024_phone_numbers.sql").replace("public.", "")).execute(&state.db).await.unwrap();
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
            number_id: number_id.to_string(),
            from: "+15550001111".to_string(),
            to: "+15550002222".to_string(),
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
    async fn immediate_answer_from_cache_while_runtime_asleep_then_in_order_delivery() {
        let state = voice_test_state().await;
        save_bot_config(&state, "bot-1", "consented").await;
        let directory = FakeDirectory::default();
        directory.owners.lock().unwrap().insert(
            "number-9".to_string(),
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string() },
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
            ["botId", "callId", "direction", "from", "numberId", "ownerId", "room", "startedAt", "to"]
        );
        assert_eq!(create["callId"], call_id);
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
                NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string() },
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
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string() },
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
            NumberOwner { user_id: "user-1".to_string(), runtime_id: "rt-1".to_string() },
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
}
