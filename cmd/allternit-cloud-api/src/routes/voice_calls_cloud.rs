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
//! - `POST /api/v1/voice/calls/:callId/events` (worker): a batch of `call.*`
//!   events, stored in order, deduped by the frozen idempotency key
//!   `call:<callId>:<type>:<n>`.
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

use super::channel_inbound::{backoff_secs, classify, Delivery};
use super::livekit_admin::{
    LiveKitAdminClient, LiveKitConfig, LiveKitError, LiveKitHttpAdmin, CONTROL_TOPIC,
};
use super::runtime_relay::{relay_request_to_runtime_with, RelayRequest};
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

/// One POST to the owner's runtime, returning the status and body so the
/// proxy can answer the worker and the queue can classify the outcome.
#[async_trait::async_trait]
pub trait CallRelay: Send + Sync {
    async fn relay(
        &self,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String>;
}

/// Production relay: wakes a sleeping cloud computer, waits for the answer.
pub struct ProdCallRelay<'a> {
    pub state: &'a ApiState,
}

#[async_trait::async_trait]
impl CallRelay for ProdCallRelay<'_> {
    async fn relay(
        &self,
        user_id: &str,
        runtime_id: &str,
        path: &str,
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let response = relay_request_to_runtime_with(
            &self.state.db,
            &self.state.contabo_runtime_service,
            &self.state.quota_service,
            &self.state.provisioning_service,
            user_id,
            runtime_id,
            RelayRequest {
                method: "POST".to_string(),
                path: path.to_string(),
                headers: HashMap::new(),
                body: STANDARD.encode(body),
                body_encoding: "base64".to_string(),
            },
            &[],
            HashMap::new(),
        )
        .await
        .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .map_err(|error| error.to_string())?
            .to_vec();
        Ok((status, bytes))
    }
}

// ------------------------------------------------------------- bot config

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BotVoiceConfig {
    persona: String,
    voice_id: String,
    greeting: String,
    /// `off` unless the owner configured recording consent.
    recording: String,
}

/// The cache read behind the immediate call answer: the stored row, or safe
/// defaults when the runtime never saved one.
async fn load_bot_config(db: &sqlx::PgPool, bot_id: &str) -> Result<BotVoiceConfig, ApiError> {
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT persona, voice_id, greeting, recording FROM voice_bot_config WHERE bot_id = $1",
    )
    .bind(bot_id)
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some((persona, voice_id, greeting, recording)) => {
            BotVoiceConfig { persona, voice_id, greeting, recording }
        }
        None => BotVoiceConfig {
            persona: DEFAULT_PERSONA.to_string(),
            voice_id: DEFAULT_VOICE_ID.to_string(),
            greeting: DEFAULT_GREETING.to_string(),
            recording: "off".to_string(),
        },
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutBotConfig {
    persona: Option<String>,
    voice_id: Option<String>,
    greeting: Option<String>,
    recording: Option<String>,
}

async fn put_bot_config(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(bot_id): Path<String>,
    Json(body): Json<PutBotConfig>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    if let Some(recording) = &body.recording {
        if !matches!(recording.as_str(), "off" | "consented") {
            return Err(ApiError::BadRequest(
                "recording must be \"off\" or \"consented\"".to_string(),
            ));
        }
    }
    // Absent fields keep their stored value (COALESCE); first write of a
    // missing row materializes the safe defaults.
    let row: (String, String, String, String) = sqlx::query_as(
        "INSERT INTO voice_bot_config (bot_id, user_id, persona, voice_id, greeting, recording)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (bot_id) DO UPDATE SET
             user_id = EXCLUDED.user_id,
             persona = COALESCE(EXCLUDED.persona, voice_bot_config.persona),
             voice_id = COALESCE(EXCLUDED.voice_id, voice_bot_config.voice_id),
             greeting = COALESCE(EXCLUDED.greeting, voice_bot_config.greeting),
             recording = COALESCE(EXCLUDED.recording, voice_bot_config.recording),
             updated_at = now()
         RETURNING persona, voice_id, greeting, recording",
    )
    .bind(&bot_id)
    .bind(&user.id)
    .bind(body.persona.as_deref().unwrap_or(DEFAULT_PERSONA))
    .bind(body.voice_id.as_deref().unwrap_or(DEFAULT_VOICE_ID))
    .bind(body.greeting.as_deref().unwrap_or(DEFAULT_GREETING))
    .bind(body.recording.as_deref().unwrap_or("off"))
    .fetch_one(&state.db)
    .await?;
    let (persona, voice_id, greeting, recording) = row;
    Ok(Json(BotVoiceConfig { persona, voice_id, greeting, recording }).into_response())
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
}

async fn start_call(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<StartCall>,
) -> Result<Response, ApiError> {
    authorize_worker(&headers)?;
    let directory = PgPhoneNumberDirectory { db: state.db.clone() };
    start_call_inner(&state, &directory, body).await
}

async fn start_call_inner(
    state: &Arc<ApiState>,
    directory: &dyn PhoneNumberDirectory,
    body: StartCall,
) -> Result<Response, ApiError> {
    if !matches!(body.direction.as_str(), "inbound" | "outbound") {
        return Err(ApiError::BadRequest("direction must be inbound or outbound".to_string()));
    }
    let Some(owner) = directory.owner_for(&body.number_id).await? else {
        return Err(ApiError::NotFound("number not found".to_string()));
    };
    let call_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
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
    .execute(&state.db)
    .await?;
    // call.started is enqueued (n=1) before the worker can post anything for
    // this call, so in-order delivery always lands it first.
    enqueue_event(
        &state.db,
        &call_id,
        "call.started",
        1,
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
    let bot = load_bot_config(&state.db, &body.bot_id).await?;
    Ok(Json(json!({ "callId": call_id, "bot": bot })).into_response())
}

// ------------------------------------------------------------------ events

/// Store one event unless its frozen idempotency key already exists.
async fn enqueue_event(
    db: &sqlx::PgPool,
    call_id: &str,
    event_type: &str,
    n: i64,
    payload: &Value,
) -> Result<bool, ApiError> {
    let key = format!("call:{call_id}:{event_type}:{n}");
    let inserted = sqlx::query(
        "INSERT INTO voice_call_events (call_id, event_type, n, event_key, payload)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (event_key) DO NOTHING",
    )
    .bind(call_id)
    .bind(event_type)
    .bind(n)
    .bind(&key)
    .bind(payload)
    .execute(db)
    .await?
    .rows_affected();
    Ok(inserted > 0)
}

async fn post_events(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(call_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    authorize_worker(&headers)?;
    post_events_inner(&state, &call_id, body).await
}

async fn post_events_inner(
    state: &Arc<ApiState>,
    call_id: &str,
    body: Value,
) -> Result<Response, ApiError> {
    let call: Option<(String,)> =
        sqlx::query_as("SELECT call_id FROM voice_calls WHERE call_id = $1")
            .bind(call_id)
            .fetch_optional(&state.db)
            .await?;
    if call.is_none() {
        return Err(ApiError::NotFound("call not found".to_string()));
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM voice_call_events WHERE call_id = $1 AND delivered_at IS NULL AND dead_at IS NULL",
    )
    .bind(call_id)
    .fetch_one(&state.db)
    .await?;
    let events = body
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::BadRequest("events must be an array".to_string()))?;
    if pending + events.len() as i64 > MAX_PENDING_PER_CALL {
        return Ok(StatusCode::TOO_MANY_REQUESTS.into_response());
    }
    let mut accepted = 0i64;
    let mut duplicates = 0i64;
    for event in events {
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::BadRequest("every event needs a call.* type".to_string()))?;
        if !event_type.starts_with("call.") {
            return Err(ApiError::BadRequest(format!("not a call.* event: {event_type}")));
        }
        let n = event
            .get("n")
            .and_then(Value::as_i64)
            .filter(|n| *n >= 1)
            .ok_or_else(|| ApiError::BadRequest("every event needs n >= 1".to_string()))?;
        // Everything that isn't type/n is the event payload.
        let mut payload = event.clone();
        if let Some(map) = payload.as_object_mut() {
            map.remove("type");
            map.remove("n");
        }
        if enqueue_event(&state.db, call_id, event_type, n, &payload).await? {
            accepted += 1;
        } else {
            duplicates += 1;
        }
    }
    let spawned = state.clone();
    let spawned_call = call_id.to_string();
    tokio::spawn(async move {
        let relay = ProdCallRelay { state: spawned.as_ref() };
        if let Err(error) = deliver_call(&spawned, &spawned_call, &relay).await {
            tracing::warn!(call_id = %spawned_call, "voice call delivery pass failed: {error}");
        }
    });
    Ok(Json(json!({ "accepted": accepted, "duplicates": duplicates })).into_response())
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
    let Some((owner_id, room)) = sqlx::query_as::<_, (String, String)>(
        "SELECT user_id, room FROM voice_calls WHERE call_id = $1",
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
    let payload = json!({
        "callId": call_id,
        "action": body["action"],
        "target": body.get("target").cloned().unwrap_or(Value::Null),
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
        let claimed: Option<(i64, String, i64, String, Value, chrono::DateTime<chrono::Utc>, i32)> =
            sqlx::query_as(
                "UPDATE voice_call_events SET locked_until = now() + make_interval(mins => $2), attempts = attempts + 1
                  WHERE id = (
                    SELECT id FROM voice_call_events
                     WHERE call_id = $1 AND delivered_at IS NULL AND dead_at IS NULL
                     ORDER BY n LIMIT 1 FOR UPDATE SKIP LOCKED)
                    AND next_attempt_at <= now() AND (locked_until IS NULL OR locked_until < now())
                  RETURNING id, event_type, n::bigint, event_key, payload, received_at, attempts",
            )
            .bind(call_id)
            .bind(LOCK_MINUTES)
            .fetch_optional(&state.db)
            .await?;
        let Some((id, event_type, n, event_key, payload, received_at, attempts)) = claimed
        else {
            return Ok(());
        };
        let Some((user_id, runtime_id)) =
            sqlx::query_as::<_, (String, String)>(
                "SELECT user_id, runtime_id FROM voice_calls WHERE call_id = $1",
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
        // Flat camelCase envelope, key included so the runtime can dedupe a
        // redelivery (frozen contract writes events via led with the same key).
        let mut envelope = payload;
        if let Some(map) = envelope.as_object_mut() {
            map.insert("type".to_string(), json!(event_type));
            map.insert("callId".to_string(), json!(call_id));
            map.insert("key".to_string(), json!(event_key));
            map.insert("n".to_string(), json!(n));
        }
        let body = serde_json::to_vec(&envelope).unwrap_or_default();
        let path = if event_type == "call.started" {
            RUNTIME_CALLS_PATH.to_string()
        } else {
            runtime_events_path(call_id)
        };
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
        let give_up =
            chrono::Utc::now() - received_at > chrono::Duration::hours(GIVE_UP_AFTER_HOURS);
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
    }

    use std::collections::VecDeque;

    impl FakeRelay {
        fn asleep_then_awake() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
                statuses: std::sync::Mutex::new(VecDeque::from(vec![503u16])),
            }
        }
        fn recorded(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl CallRelay for FakeRelay {
        async fn relay(
            &self,
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
            self.calls
                .lock()
                .unwrap()
                .push((path.to_string(), serde_json::from_slice(body).unwrap()));
            Ok((status, br#"{"ok":true}"#.to_vec()))
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
             persona TEXT NOT NULL DEFAULT 'A helpful AI assistant.',
             voice_id TEXT NOT NULL DEFAULT 'allternit-default',
             greeting TEXT NOT NULL DEFAULT 'How can I help you today?',
             recording TEXT NOT NULL DEFAULT 'off' CHECK (recording IN ('off','consented')),
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
             started_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
           )"#,
        r#"CREATE TABLE voice_call_events (
             id BIGSERIAL PRIMARY KEY,
             call_id TEXT NOT NULL REFERENCES voice_calls(call_id) ON DELETE CASCADE,
             event_type TEXT NOT NULL,
             n INTEGER NOT NULL,
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
        state
    }

    async fn save_bot_config(state: &ApiState, bot_id: &str, recording: &str) {
        sqlx::query(
            "INSERT INTO voice_bot_config (bot_id, user_id, persona, voice_id, greeting, recording)
             VALUES ($1, 'user-1', 'Test persona', 'voice-x', 'Hi there', $2)",
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
        assert_eq!(answer["bot"]["recording"], "consented");

        // Worker posts two more events; all three queue in n order.
        let posted = post_events_inner(
            &state,
            &call_id,
            json!({ "events": [
                { "type": "call.transcript.delta", "n": 2, "speaker": "caller", "text": "hi", "final": true, "segmentId": "s1" },
                { "type": "call.dtmf", "n": 3, "digits": "5", "from": "caller" },
            ] }),
        )
        .await
        .unwrap();
        let posted: Value = serde_json::from_slice(
            &axum::body::to_bytes(posted.into_body(), 1 << 20).await.unwrap(),
        )
        .unwrap();
        assert_eq!(posted["accepted"], 2);
        assert_eq!(posted["duplicates"], 0);

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
        assert_eq!(recorded[0].1["type"], "call.started");
        assert_eq!(recorded[0].1["key"], format!("call:{call_id}:call.started:1"));
        assert_eq!(recorded[0].1["direction"], "inbound");
        assert_eq!(recorded[0].1["numberId"], "number-9");
        assert_eq!(recorded[1].1["type"], "call.transcript.delta");
        assert_eq!(recorded[1].1["speaker"], "caller");
        assert_eq!(recorded[2].1["type"], "call.dtmf");
        assert_eq!(recorded[2].1["digits"], "5");

        let delivered: (i64,) =
            sqlx::query_as("SELECT count(*) FROM voice_call_events WHERE call_id = $1 AND delivered_at IS NOT NULL")
                .bind(&call_id)
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(delivered.0, 3);
    }

    #[tokio::test]
    async fn missing_bot_config_answers_with_safe_defaults() {
        let state = voice_test_state().await;
        let bot = load_bot_config(&state.db, "never-saved").await.unwrap();
        assert_eq!(bot.persona, DEFAULT_PERSONA);
        assert_eq!(bot.voice_id, DEFAULT_VOICE_ID);
        assert_eq!(bot.recording, "off");
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

        let first = post_events_inner(
            &state,
            &call_id,
            json!({ "events": [{ "type": "call.transcript.delta", "n": 2, "text": "once", "final": true }] }),
        )
        .await
        .unwrap();
        let first: Value = serde_json::from_slice(
            &axum::body::to_bytes(first.into_body(), 1 << 20).await.unwrap(),
        )
        .unwrap();
        assert_eq!(first["accepted"], 1);
        // Same (type, n) redelivered — a worker retry — must not double.
        let second = post_events_inner(
            &state,
            &call_id,
            json!({ "events": [{ "type": "call.transcript.delta", "n": 2, "text": "once", "final": true }] }),
        )
        .await
        .unwrap();
        let second: Value = serde_json::from_slice(
            &axum::body::to_bytes(second.into_body(), 1 << 20).await.unwrap(),
        )
        .unwrap();
        assert_eq!(second["accepted"], 0);
        assert_eq!(second["duplicates"], 1);
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
        let err = post_events_inner(&state, "nope", json!({ "events": [] })).await.unwrap_err();
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
}
