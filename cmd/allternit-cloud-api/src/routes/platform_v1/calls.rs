//! `/v1/calls` and `/v1/realtime/sessions`: voice with hosted agents (spec §4
//! Voice and Realtime, §6). Scope `voice`.
//!
//! A call joins one agent, and (for phone calls) one project number in the same
//! account. The voice itself is the existing stack: LiveKit SIP rooms with the
//! `allternit-voice` worker, which starts calls, posts `call.*` events and runs
//! caller turns through cloud-api (`routes::voice_calls_cloud`). For a project
//! number those land here instead of on a user's runtime:
//!
//! * the worker's start answers with the agent's greeting, voice and name
//!   ([`worker_start`]);
//! * each caller turn runs in a hosted-runtime session of the agent, exactly
//!   like a conversation turn ([`worker_turn`]);
//! * events are applied in the cloud: final transcript lines are kept, and
//!   `call.ended` closes the call, meters it and sends the webhooks
//!   ([`apply_worker_event`]).
//!
//! Rules (fail closed):
//! * **Consent:** an outbound call only reaches someone who texted or called the
//!   number first, or whose consent was recorded (`POST /v1/numbers/{id}/consent`).
//!   STOP on the number always wins. US and Canada (+1) only.
//! * **AI disclosure:** the agent's greeting (already required to say it is an
//!   AI) opens every call.
//! * **Business hours:** an agent with `business_hours` places outbound calls
//!   only inside them.
//! * **Spend cap:** [`super::spend::spend_allowed`] is asked before a call starts.
//! * **Concurrency:** one call slot per live call (plan cap, `slots`).
//! * **Recording:** off unless the outbound request asks for it.
//!
//! **Sandbox** projects never reach LiveKit or a carrier: their numbers are
//! simulated, a call is `in_progress` at once, `POST /v1/calls/{id}/simulate_turn`
//! plays what the caller says and answers with the agent's real reply (hosted
//! runtime), `POST /v1/numbers/{id}/simulate_call` plays an inbound call, and
//! ending sends the same webhooks. Simulated calls are not metered.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json,
};
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{
    agents::{self, Agent},
    caller::{Plan, PlatformCaller, ProjectEnv},
    conversations,
    events::emit_event,
    hosting::{host_for, runtime_owner, AgentHost},
    numbers::{api_number, ApiNumber},
    slots::{self, KIND_CALL},
    spend, build_page, new_id, record_usage, ApiJson, ApiQuery, Page, PageParams, PlatformError, RouteTable, UsageEvent,
};
use crate::routes::livekit_admin::{
    CreateSipParticipantRequest, LiveKitAdminClient, LiveKitConfig, LiveKitError, LiveKitHttpAdmin, CALL_ROOM_PREFIX,
    CONTROL_TOPIC, SIP_AGENT_NAME,
};
use crate::routes::phone;
use crate::{ApiError, ApiState};

const MAX_PURPOSE: usize = 500;
/// A realtime client token must be used within this many seconds.
pub const REALTIME_TOKEN_TTL_SECS: i64 = 60;
/// A dialed call nobody picked up is closed as `no_answer` after this long.
const RING_TIMEOUT_SECS: i64 = 120;
/// Room prefix of realtime (in-app) sessions; the worker answers these too.
pub const REALTIME_ROOM_PREFIX: &str = "call-rt-";

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/calls", &["GET", "POST"], get(list_calls).post(create_call))
        .add("/v1/calls/:id", &["GET"], get(get_call))
        .add("/v1/calls/:id/end", &["POST"], post(end_call))
        .add("/v1/calls/:id/transfer", &["POST"], post(transfer_call))
        .add("/v1/calls/:id/transcript", &["GET"], get(get_transcript))
        .add("/v1/calls/:id/recording", &["GET"], get(get_recording))
        .add("/v1/calls/:id/simulate_turn", &["POST"], post(simulate_turn))
        .add("/v1/numbers/:id/simulate_call", &["POST"], post(simulate_inbound_call))
        .add("/v1/realtime/sessions", &["POST"], post(create_realtime_session))
}

// ---------------------------------------------------------------- voice deps

/// LiveKit and the outbound trunk. Production reads the env; tests layer a fake
/// as `Extension<Arc<VoiceDeps>>`.
pub struct VoiceDeps {
    pub livekit: Arc<dyn LiveKitAdminClient>,
    /// `ALLTERNIT_LIVEKIT_OUTBOUND_TRUNK_ID`; outbound calls need it.
    pub outbound_trunk: Option<String>,
}

type DepsExt = Option<Extension<Arc<VoiceDeps>>>;
type HostExt = Option<Extension<Arc<dyn AgentHost>>>;
type StoreExt = Option<Extension<Arc<dyn crate::services::r2::ObjectStore>>>;

pub fn voice_deps(layered: DepsExt) -> Option<Arc<VoiceDeps>> {
    if let Some(Extension(d)) = layered {
        return Some(d);
    }
    let config = LiveKitConfig::from_env()?;
    let trunk = std::env::var(phone::OUTBOUND_TRUNK_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    Some(Arc::new(VoiceDeps { livekit: Arc::new(LiveKitHttpAdmin::new(config)), outbound_trunk: trunk }))
}

fn voice_unavailable() -> PlatformError {
    PlatformError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        kind: "api_error",
        code: "voice_unavailable".into(),
        message: "Live calls aren't available on this deployment right now.".into(),
        param: None, url: None,
    }
}

fn livekit_failed(e: LiveKitError) -> PlatformError {
    tracing::warn!("platform voice: livekit: {e}");
    PlatformError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        kind: "api_error",
        code: "dial_failed".into(),
        message: "The call couldn't be connected. Retry shortly.".into(),
        param: None, url: None,
    }
}

// ---------------------------------------------------------------- the call row

#[derive(Debug, Clone, FromRow)]
pub struct CallRow {
    pub id: String,
    pub project_id: String,
    pub account_id: String,
    pub agent_id: String,
    pub number_id: Option<String>,
    pub direction: String,
    pub from_e164: Option<String>,
    pub to_e164: Option<String>,
    pub purpose: Option<String>,
    pub status: String,
    pub end_reason: Option<String>,
    pub simulated: bool,
    pub room: Option<String>,
    pub consent_ref: Option<String>,
    pub recording: bool,
    pub recording_ref: Option<String>,
    pub conversation_id: Option<String>,
    pub transferred_to: Option<String>,
    pub slot_id: Option<String>,
    pub duration_seconds: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, project_id, account_id, agent_id, number_id, direction, from_e164, to_e164, purpose, status, end_reason, \
    simulated, room, consent_ref, recording, recording_ref, conversation_id, transferred_to, slot_id, duration_seconds, created_at, answered_at, ended_at";

const LIVE: [&str; 3] = ["queued", "ringing", "in_progress"];

#[derive(Debug, Serialize)]
pub struct Call {
    pub id: String,
    pub object: &'static str,
    pub account_id: String,
    pub agent_id: String,
    pub number_id: Option<String>,
    pub direction: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub purpose: Option<String>,
    pub status: String,
    pub end_reason: Option<String>,
    pub simulated: bool,
    pub recording: bool,
    pub transferred_to: Option<String>,
    pub duration_seconds: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub answered_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

impl From<&CallRow> for Call {
    fn from(r: &CallRow) -> Self {
        Call {
            id: r.id.clone(),
            object: "call",
            account_id: r.account_id.clone(),
            agent_id: r.agent_id.clone(),
            number_id: r.number_id.clone(),
            direction: r.direction.clone(),
            from: r.from_e164.clone(),
            to: r.to_e164.clone(),
            purpose: r.purpose.clone(),
            status: r.status.clone(),
            end_reason: r.end_reason.clone(),
            simulated: r.simulated,
            recording: r.recording,
            transferred_to: r.transferred_to.clone(),
            duration_seconds: r.duration_seconds,
            created_at: r.created_at,
            answered_at: r.answered_at,
            ended_at: r.ended_at,
        }
    }
}

async fn load(db: &PgPool, id: &str) -> Result<Option<CallRow>, sqlx::Error> {
    sqlx::query_as::<_, CallRow>(&format!("SELECT {COLUMNS} FROM platform_calls WHERE id = $1")).bind(id).fetch_optional(db).await
}

/// A call the key may see (its project, its account), or 404.
async fn visible(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<CallRow, PlatformError> {
    sweep_unanswered(db).await;
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, CallRow>(&format!(
        "SELECT {COLUMNS} FROM platform_calls WHERE id = $1 AND project_id = $2 AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PlatformError::not_found("call_not_found", "No such call."))
}

struct NewCall<'a> {
    project_id: &'a str,
    agent_id: &'a str,
    account_id: &'a str,
    number_id: Option<&'a str>,
    direction: &'a str,
    from: Option<&'a str>,
    to: Option<&'a str>,
    purpose: Option<&'a str>,
    status: &'a str,
    simulated: bool,
    room: Option<&'a str>,
    consent_ref: Option<&'a str>,
    recording: bool,
    slot_id: Option<&'a str>,
    client_identity: Option<&'a str>,
}

async fn insert(db: &PgPool, n: NewCall<'_>) -> Result<CallRow, PlatformError> {
    let id = new_id("call_");
    let conversation = conversations::open(db, n.project_id, n.account_id, n.agent_id, &json!({ "call_id": id })).await?;
    let answered = n.status == "in_progress";
    Ok(sqlx::query_as::<_, CallRow>(&format!(
        "INSERT INTO platform_calls (id, project_id, account_id, agent_id, number_id, direction, from_e164, to_e164, purpose, status, simulated, \
           room, consent_ref, recording, conversation_id, slot_id, client_identity, answered_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, CASE WHEN $18 THEN NOW() END) RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(n.project_id)
    .bind(n.account_id)
    .bind(n.agent_id)
    .bind(n.number_id)
    .bind(n.direction)
    .bind(n.from)
    .bind(n.to)
    .bind(n.purpose)
    .bind(n.status)
    .bind(n.simulated)
    .bind(n.room)
    .bind(n.consent_ref)
    .bind(n.recording)
    .bind(&conversation)
    .bind(n.slot_id)
    .bind(n.client_identity)
    .bind(answered)
    .fetch_one(db)
    .await?)
}

fn call_event_data(c: &CallRow) -> Value {
    json!({ "call": Call::from(c) })
}

/// A dialed call or realtime session nobody joined within the ring timeout is
/// closed as `no_answer` and gives its slot back.
async fn sweep_unanswered(db: &PgPool) {
    let stale: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "UPDATE platform_calls SET status = 'no_answer', end_reason = 'not_answered', ended_at = NOW(), updated_at = NOW() \
         WHERE status IN ('queued', 'ringing') AND created_at < NOW() - make_interval(secs => $1::float8) RETURNING project_id, id, slot_id",
    )
    .bind(RING_TIMEOUT_SECS as f64)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    for (project, id, slot) in stale {
        if let Some(slot) = slot {
            let _ = slots::release_slot(db, &project, &slot).await;
        }
        if let Ok(Some(c)) = load(db, &id).await {
            let _ = emit_event(db, &c.project_id, Some(&c.account_id), "call.ended", call_event_data(&c)).await;
        }
    }
}

// ---------------------------------------------------------------- business hours

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn hhmm(v: &Value) -> Option<u32> {
    let s = v.as_str()?;
    let (h, m) = s.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (s.len() == 5 && h <= 24 && m < 60 && h * 60 + m <= 24 * 60).then_some(h * 60 + m)
}

/// `{ "tz": "America/Chicago", "mon": ["09:00", "17:00"], … }`: `tz` is required;
/// days are `mon`…`sun` with `[open, close]` (24-hour, `close` after `open`).
/// With no day listed the agent is always open; once any day is listed, the
/// days left out are closed.
pub fn validate_business_hours(v: &Value) -> Result<(), String> {
    let obj = v.as_object().ok_or("business_hours must be an object")?;
    let tz = obj.get("tz").and_then(Value::as_str).ok_or("business_hours.tz is required (an IANA time zone like America/Chicago)")?;
    tz.parse::<chrono_tz::Tz>().map_err(|_| format!("business_hours.tz \"{tz}\" is not a time zone"))?;
    for (k, val) in obj {
        if k == "tz" {
            continue;
        }
        if !DAYS.contains(&k.as_str()) {
            return Err(format!("business_hours has an unknown key \"{k}\"; use tz and mon…sun"));
        }
        let pair = val.as_array().filter(|a| a.len() == 2).ok_or_else(|| format!("business_hours.{k} must be [\"HH:MM\", \"HH:MM\"]"))?;
        match (hhmm(&pair[0]), hhmm(&pair[1])) {
            (Some(o), Some(c)) if c > o => {}
            _ => return Err(format!("business_hours.{k} must be [open, close] with close after open, like [\"09:00\", \"17:00\"]")),
        }
    }
    Ok(())
}

/// Is `now` inside the hours? Hours that don't parse count as closed (fail closed).
pub fn within_business_hours(v: &Value, now: DateTime<Utc>) -> bool {
    if validate_business_hours(v).is_err() {
        return false;
    }
    let obj = v.as_object().expect("validated");
    let tz: chrono_tz::Tz = obj["tz"].as_str().unwrap_or_default().parse().expect("validated");
    if !DAYS.iter().any(|d| obj.contains_key(*d)) {
        return true;
    }
    let local = now.with_timezone(&tz);
    let day = DAYS[local.weekday().num_days_from_monday() as usize];
    let Some(pair) = obj.get(day).and_then(Value::as_array) else { return false };
    let minute = local.hour() * 60 + local.minute();
    matches!((hhmm(&pair[0]), hhmm(&pair[1])), (Some(o), Some(c)) if minute >= o && minute < c)
}

// ---------------------------------------------------------------- checks

fn is_nanp(e164: &str) -> bool {
    e164.starts_with("+1") && e164.len() == 12
}

/// The consent gate for calling `to` from `number` (numbers docs): STOP wins,
/// then the person must have texted or called first or have consent recorded.
async fn require_call_consent(db: &PgPool, number_id: &str, to: &str) -> Result<(), PlatformError> {
    if phone::is_opted_out(db, number_id, to).await? {
        return Err(PlatformError::permission("recipient_opted_out", "This person replied STOP to this number."));
    }
    if phone::consent_basis(db, number_id, to).await?.is_none() {
        return Err(PlatformError::permission(
            "no_consent",
            "This person hasn't texted or called this number and has no consent recorded (POST /v1/numbers/{id}/consent).",
        ));
    }
    Ok(())
}

async fn require_spend(db: &PgPool, project_id: &str) -> Result<(), PlatformError> {
    if spend::spend_allowed(db, project_id).await? {
        Ok(())
    } else {
        Err(spend::cap_reached())
    }
}

// ---------------------------------------------------------------- outbound

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCall {
    agent_id: String,
    from_number_id: String,
    to: String,
    purpose: String,
    #[serde(default)]
    record: bool,
}

async fn create_call(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    deps: DepsExt,
    ApiJson(body): ApiJson<CreateCall>,
) -> Result<(StatusCode, Json<Call>), PlatformError> {
    caller.require("voice")?;
    let db = &state.db;
    sweep_unanswered(db).await;
    let agent = agents::fetch_visible(&state, &caller, &body.agent_id).await?;
    let number = api_number(db, &caller, &body.from_number_id).await.map_err(|e| e.with_param("from_number_id"))?;
    if number.account_id.as_deref() != Some(agent.account_id.as_str()) {
        return Err(PlatformError::invalid_request("account_mismatch", "The number and the agent belong to different accounts.").with_param("from_number_id"));
    }
    let to = body.to.trim();
    if !crate::carriers::is_e164(to) {
        return Err(PlatformError::invalid_request("invalid_to", "to must be an E.164 number like +14155550101.").with_param("to"));
    }
    if !is_nanp(to) {
        return Err(PlatformError::permission("international_calling_disabled", "Calls go to US and Canadian (+1) numbers only."));
    }
    let purpose = body.purpose.trim();
    if purpose.is_empty() || purpose.chars().count() > MAX_PURPOSE {
        return Err(PlatformError::invalid_request("invalid_purpose", format!("purpose must be 1 to {MAX_PURPOSE} characters: what the call is for.")).with_param("purpose"));
    }
    require_call_consent(db, &number.id, to).await?;
    if let Some(hours) = &agent.business_hours {
        if !within_business_hours(hours, Utc::now()) {
            return Err(PlatformError::conflict("outside_business_hours", "The agent's business hours are closed now, so it doesn't place calls."));
        }
    }
    require_spend(db, &caller.project_id).await?;

    if number.simulated {
        let slot = slots::acquire_slot(db, &caller.project_id, KIND_CALL).await?;
        let row = insert(db, NewCall {
            project_id: &caller.project_id, agent_id: &agent.id, account_id: &agent.account_id, number_id: Some(&number.id), direction: "outbound",
            from: Some(&number.e164), to: Some(to), purpose: Some(purpose), status: "in_progress", simulated: true,
            room: None, consent_ref: None, recording: body.record, slot_id: Some(slot.slot_id()), client_identity: None,
        })
        .await?;
        slot.detach();
        let _ = emit_event(db, &caller.project_id, Some(&row.account_id), "call.started", call_event_data(&row)).await;
        return Ok((StatusCode::CREATED, Json(Call::from(&row))));
    }

    let deps = voice_deps(deps).ok_or_else(voice_unavailable)?;
    let trunk = deps.outbound_trunk.clone().ok_or_else(voice_unavailable)?;
    let slot = slots::acquire_slot(db, &caller.project_id, KIND_CALL).await?;
    // The consent gate's reference, as for the app's numbers: the worker refuses to dial without it.
    let consent = phone::consent_ref_for(db, &caller.owner_user_id, &number.id, to, &agent.id, purpose)
        .await
        .map_err(super::numbers::phone_error)?
        .ok_or_else(|| PlatformError::permission("no_consent", "This person hasn't texted or called this number and has no consent recorded."))?;
    let room = format!("{CALL_ROOM_PREFIX}out-{}", uuid::Uuid::new_v4().simple());
    let owner = runtime_owner(&caller.project_id);
    let row = insert(db, NewCall {
        project_id: &caller.project_id, agent_id: &agent.id, account_id: &agent.account_id, number_id: Some(&number.id), direction: "outbound",
        from: Some(&number.e164), to: Some(to), purpose: Some(purpose), status: "queued", simulated: false,
        room: Some(&room), consent_ref: Some(&consent.id), recording: body.record, slot_id: Some(slot.slot_id()), client_identity: None,
    })
    .await?;
    let attrs: std::collections::HashMap<String, String> = [
        ("direction", "outbound"),
        ("consentRef", consent.id.as_str()),
        ("botId", agent.id.as_str()),
        ("ownerId", owner.as_str()),
        ("numberId", number.id.as_str()),
        ("to", to),
        ("from", number.e164.as_str()),
        ("purpose", purpose),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let dial = async {
        deps.livekit.create_room_with_agent(&room, SIP_AGENT_NAME, &json!(attrs).to_string()).await?;
        deps.livekit
            .create_sip_participant(CreateSipParticipantRequest {
                trunk_id: trunk,
                call_to: to.to_string(),
                room_name: room.clone(),
                participant_identity: format!("sip-out-{}", uuid::Uuid::new_v4().simple()),
                participant_attributes: attrs.clone(),
                consent_ref: Some(consent.id.clone()),
                from_number: Some(number.e164.clone()),
            })
            .await
    };
    if let Err(e) = dial.await {
        sqlx::query("UPDATE platform_calls SET status = 'failed', end_reason = 'dial_failed', ended_at = NOW(), updated_at = NOW() WHERE id = $1")
            .bind(&row.id)
            .execute(db)
            .await?;
        slot.release().await?;
        return Err(livekit_failed(e));
    }
    slot.detach();
    let row = sqlx::query_as::<_, CallRow>(&format!("UPDATE platform_calls SET status = 'ringing', updated_at = NOW() WHERE id = $1 AND status = 'queued' RETURNING {COLUMNS}"))
        .bind(&row.id)
        .fetch_optional(db)
        .await?
        .unwrap_or(row);
    Ok((StatusCode::CREATED, Json(Call::from(&row))))
}

// ---------------------------------------------------------------- read

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    account_id: Option<String>,
    agent_id: Option<String>,
    status: Option<String>,
}

async fn list_calls(State(state): State<Arc<ApiState>>, caller: PlatformCaller, ApiQuery(q): ApiQuery<ListQuery>) -> Result<Json<Page<Call>>, PlatformError> {
    caller.require("voice")?;
    sweep_unanswered(&state.db).await;
    let limit = q.page.limit()?;
    let (after_at, after_id) = match q.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let account = caller.account_filter(q.account_id.as_deref())?;
    let rows = sqlx::query_as::<_, CallRow>(&format!(
        "SELECT {COLUMNS} FROM platform_calls WHERE project_id = $1 AND ($2::text IS NULL OR account_id = $2) \
           AND ($3::text IS NULL OR agent_id = $3) AND ($4::text IS NULL OR status = $4) \
           AND ($5::timestamptz IS NULL OR (created_at, id) > ($5, $6)) ORDER BY created_at, id LIMIT $7"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(&q.agent_id)
    .bind(&q.status)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let calls: Vec<Call> = rows.iter().map(Call::from).collect();
    Ok(Json(build_page(calls, limit, |c| (c.created_at, c.id.clone()))))
}

async fn get_call(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Call>, PlatformError> {
    caller.require("voice")?;
    Ok(Json(Call::from(&visible(&state.db, &caller, &id).await?)))
}

#[derive(Debug, Serialize, FromRow)]
struct Line {
    speaker: String,
    text: String,
    created_at: DateTime<Utc>,
}

async fn transcript_lines(db: &PgPool, call_id: &str) -> Result<Vec<Line>, sqlx::Error> {
    sqlx::query_as::<_, Line>("SELECT speaker, text, created_at FROM platform_call_transcript WHERE call_id = $1 ORDER BY id").bind(call_id).fetch_all(db).await
}

async fn get_transcript(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require("voice")?;
    let call = visible(&state.db, &caller, &id).await?;
    let lines = transcript_lines(&state.db, &call.id).await?;
    let complete = !LIVE.contains(&call.status.as_str());
    Ok(Json(json!({ "object": "call.transcript", "call_id": call.id, "complete": complete, "lines": lines })))
}

async fn get_recording(State(state): State<Arc<ApiState>>, caller: PlatformCaller, store: StoreExt, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require("voice")?;
    let call = visible(&state.db, &caller, &id).await?;
    let Some(key) = call.recording_ref.as_deref().filter(|k| k.starts_with("calls/") && !k.contains("..")) else {
        return Err(PlatformError::not_found("recording_not_found", "This call has no recording. Recording is off unless the call was started with record: true."));
    };
    let owned;
    let store: &dyn crate::services::r2::ObjectStore = match &store {
        Some(Extension(s)) => s.as_ref(),
        None => {
            owned = crate::services::r2::R2Client::from_env().map_err(|_| voice_unavailable())?;
            &owned
        }
    };
    let url = store
        .presign_get(crate::routes::voice_recordings::RECORDINGS_BUCKET, key, crate::routes::voice_recordings::URL_TTL)
        .map_err(|_| voice_unavailable())?;
    let expires = Utc::now() + chrono::Duration::seconds(crate::routes::voice_recordings::URL_TTL.as_secs() as i64);
    Ok(Json(json!({ "object": "call.recording", "call_id": call.id, "url": url, "content_type": "audio/ogg", "expires_at": expires })))
}

// ---------------------------------------------------------------- finish

/// Close a call (once): status, duration, slot, usage, `call.ended`, then
/// `call.transcript.ready`. A second finish of the same call does nothing.
pub(crate) async fn finish(db: &PgPool, call_id: &str, status: &str, reason: &str, duration: Option<i64>, recording_ref: Option<&str>) -> Result<Option<CallRow>, PlatformError> {
    let row = sqlx::query_as::<_, CallRow>(&format!(
        "UPDATE platform_calls SET status = $2, end_reason = COALESCE(end_reason, $3), ended_at = NOW(), updated_at = NOW(), \
           duration_seconds = COALESCE($4, CASE WHEN answered_at IS NULL THEN 0 ELSE GREATEST(0, EXTRACT(EPOCH FROM NOW() - answered_at))::int END), \
           recording_ref = COALESCE($5, recording_ref) \
         WHERE id = $1 AND status IN ('queued', 'ringing', 'in_progress') RETURNING {COLUMNS}"
    ))
    .bind(call_id)
    .bind(status)
    .bind(reason)
    .bind(duration.map(|d| d.clamp(0, i32::MAX as i64) as i32))
    .bind(recording_ref)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    if let Some(slot) = &row.slot_id {
        let _ = slots::release_slot(db, &row.project_id, slot).await;
    }
    let secs = row.duration_seconds.unwrap_or(0);
    if !row.simulated && secs > 0 {
        let byok: Option<(String,)> = sqlx::query_as("SELECT model FROM platform_agents WHERE id = $1").bind(&row.agent_id).fetch_optional(db).await?;
        let meter = if byok.is_some_and(|(m,)| m.contains('/')) { "voice_min_byok" } else { "voice_min_allternit" };
        let _ = record_usage(
            db,
            UsageEvent {
                project_id: row.project_id.clone(),
                account_id: Some(row.account_id.clone()),
                key_id: None,
                meter: meter.into(),
                // Billed per second.
                quantity: f64::from(secs) / 60.0,
                unit: Some("minute".into()),
                ref_id: Some(row.id.clone()),
                idempotency: Some(format!("call:{}", row.id)),
            },
        )
        .await;
    }
    let _ = emit_event(db, &row.project_id, Some(&row.account_id), "call.ended", call_event_data(&row)).await;
    let lines = transcript_lines(db, &row.id).await?;
    let _ = emit_event(
        db,
        &row.project_id,
        Some(&row.account_id),
        "call.transcript.ready",
        json!({ "call_id": row.id, "lines": lines.len(), "transcript": { "object": "call.transcript", "call_id": row.id, "complete": true, "lines": lines } }),
    )
    .await;
    Ok(Some(row))
}

async fn send_control(deps: &VoiceDeps, room: &str, payload: Value) -> Result<(), PlatformError> {
    deps.livekit.send_data(room, CONTROL_TOPIC, payload.to_string().as_bytes()).await.map_err(livekit_failed)
}

async fn end_call(State(state): State<Arc<ApiState>>, caller: PlatformCaller, deps: DepsExt, Path(id): Path<String>) -> Result<(StatusCode, Json<Call>), PlatformError> {
    caller.require("voice")?;
    let db = &state.db;
    let call = visible(db, &caller, &id).await?;
    if !LIVE.contains(&call.status.as_str()) {
        return Ok((StatusCode::OK, Json(Call::from(&call))));
    }
    if call.simulated {
        let row = finish(db, &call.id, "completed", "ended_by_api", None, None).await?.unwrap_or(call);
        return Ok((StatusCode::OK, Json(Call::from(&row))));
    }
    let deps = voice_deps(deps);
    if call.status != "in_progress" {
        // Not answered yet: cancel it here; a hangup tells the worker in case it is mid-dial.
        if let (Some(d), Some(room)) = (&deps, &call.room) {
            let _ = send_control(d, room, json!({ "callId": call.id, "action": "hangup", "by": format!("api:{}", caller.key_id) })).await;
        }
        let row = finish(db, &call.id, "canceled", "canceled_by_api", Some(0), None).await?.unwrap_or(call);
        return Ok((StatusCode::OK, Json(Call::from(&row))));
    }
    let deps = deps.ok_or_else(voice_unavailable)?;
    let room = call.room.clone().ok_or_else(voice_unavailable)?;
    send_control(&deps, &room, json!({ "callId": call.id, "action": "hangup", "by": format!("api:{}", caller.key_id) })).await?;
    sqlx::query("UPDATE platform_calls SET end_reason = COALESCE(end_reason, 'ended_by_api'), updated_at = NOW() WHERE id = $1").bind(&call.id).execute(db).await?;
    // The worker hangs up and its call.ended closes the call (and sends the webhook).
    Ok((StatusCode::ACCEPTED, Json(Call::from(&call))))
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TransferBody {
    to: Option<String>,
    mode: Option<String>,
}

/// Transfer a phone call to one of the agent's `transfer_targets` (the first
/// when `to` is left out). STOP on the number wins.
async fn transfer_call(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    deps: DepsExt,
    Path(id): Path<String>,
    body: Option<ApiJson<TransferBody>>,
) -> Result<(StatusCode, Json<Call>), PlatformError> {
    caller.require("voice")?;
    let body = body.map(|ApiJson(b)| b).unwrap_or_default();
    let db = &state.db;
    let call = visible(db, &caller, &id).await?;
    let (number_id, agent) = match &call.number_id {
        Some(n) if call.direction != "realtime" => (n.clone(), agents::fetch_visible(&state, &caller, &call.agent_id).await?),
        _ => return Err(PlatformError::invalid_request("not_a_phone_call", "Only phone calls can be transferred.")),
    };
    if call.status != "in_progress" {
        return Err(PlatformError::conflict("call_not_in_progress", "Only a call in progress can be transferred."));
    }
    let mode = body.mode.as_deref().unwrap_or("warm");
    if !matches!(mode, "warm" | "cold") {
        return Err(PlatformError::invalid_request("invalid_mode", "mode must be warm or cold.").with_param("mode"));
    }
    let to = match body.to.as_deref().map(str::trim) {
        Some(t) => t.to_string(),
        None => agent.transfer_targets.first().cloned().ok_or_else(|| {
            PlatformError::invalid_request("no_transfer_targets", "This agent has no transfer_targets. Add one with PATCH /v1/agents/{id}.")
        })?,
    };
    if !agent.transfer_targets.iter().any(|t| t == &to) {
        return Err(PlatformError::invalid_request("transfer_target_not_allowed", "to must be one of the agent's transfer_targets.").with_param("to"));
    }
    if phone::is_opted_out(db, &number_id, &to).await? {
        return Err(PlatformError::permission("recipient_opted_out", "That number replied STOP to this number."));
    }
    if call.simulated {
        sqlx::query("UPDATE platform_calls SET transferred_to = $2, updated_at = NOW() WHERE id = $1").bind(&call.id).bind(&to).execute(db).await?;
        let row = finish(db, &call.id, "completed", "transferred", None, None).await?.unwrap_or(call);
        return Ok((StatusCode::OK, Json(Call::from(&row))));
    }
    let deps = voice_deps(deps).ok_or_else(voice_unavailable)?;
    let room = call.room.clone().ok_or_else(voice_unavailable)?;
    let consent_ref = if mode == "warm" { Some(transfer_consent(db, &caller.owner_user_id, &number_id, &agent.id, &to).await?) } else { None };
    send_control(&deps, &room, json!({ "callId": call.id, "action": "transfer", "to": to, "mode": mode, "consentRef": consent_ref, "by": format!("api:{}", caller.key_id) })).await?;
    // The worker reports `call.transferred`; `transferred_to` is set when it succeeds.
    Ok((StatusCode::ACCEPTED, Json(Call::from(&call))))
}

/// A short-lived consent reference for dialing a transfer target the developer
/// approved on the agent (basis `owner_transfer_target`).
async fn transfer_consent(db: &PgPool, user: &str, number_id: &str, agent_id: &str, to: &str) -> Result<String, PlatformError> {
    let id = format!("cc_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO call_consents (id, number_id, user_id, bot_id, to_e164, purpose, basis, expires_at) \
         VALUES ($1, $2, $3, $4, $5, 'transfer', $6, NOW() + INTERVAL '30 minutes')",
    )
    .bind(&id)
    .bind(number_id)
    .bind(user)
    .bind(agent_id)
    .bind(to)
    .bind(phone::TRANSFER_TARGET_SOURCE)
    .execute(db)
    .await?;
    Ok(id)
}

// ---------------------------------------------------------------- sandbox

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimulateTurn {
    text: String,
}

/// Sandbox: the caller says `text`; the agent answers through its hosted
/// runtime, and both lines go in the transcript.
async fn simulate_turn(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: HostExt,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SimulateTurn>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("voice")?;
    let db = &state.db;
    let call = visible(db, &caller, &id).await?;
    if !call.simulated {
        return Err(PlatformError::invalid_request("not_simulated", "simulate_turn works only on a sandbox project's calls."));
    }
    if call.status != "in_progress" {
        return Err(PlatformError::conflict("call_not_in_progress", "This call has ended."));
    }
    let text = body.text.trim();
    if text.is_empty() || text.chars().count() > 4000 {
        return Err(PlatformError::invalid_request("invalid_text", "text must be 1 to 4000 characters.").with_param("text"));
    }
    let conversation = call.conversation_id.clone().ok_or_else(|| PlatformError::api_error("call_session_missing", "This call has no session."))?;
    let _lock = conversations::TurnLock::take(&conversation).ok_or_else(|| PlatformError::conflict("call_busy", "The agent is still answering."))?;
    let host = host_for(&state, layered);
    let (rt, path) = conversations::turn_target(&state, host.as_ref(), &caller, &conversation).await?;
    let upstream = host.stream(&rt, &path, &json!({ "text": text })).await?;
    add_line(db, &call.id, "caller", text, None).await?;
    let reply = conversations::collect_turn(upstream).await.map_err(|m| PlatformError::api_error("turn_failed", m))?;
    add_line(db, &call.id, "agent", &reply, None).await?;
    Ok(Json(json!({ "object": "call.turn", "call_id": call.id, "caller": text, "agent": reply })))
}

async fn add_line(db: &PgPool, call_id: &str, speaker: &str, text: &str, segment: Option<&str>) -> Result<(), sqlx::Error> {
    if text.trim().is_empty() {
        return Ok(());
    }
    sqlx::query("INSERT INTO platform_call_transcript (call_id, speaker, text, segment_id) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
        .bind(call_id)
        .bind(speaker)
        .bind(text)
        .bind(segment)
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimulateCall {
    from: String,
}

/// Sandbox: someone calls a simulated number. Answered by the number's agent;
/// it counts as that person calling first (they may be called back).
async fn simulate_inbound_call(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SimulateCall>,
) -> Result<(StatusCode, Json<Call>), PlatformError> {
    caller.require("voice")?;
    let db = &state.db;
    let number = api_number(db, &caller, &id).await?;
    if !number.simulated {
        return Err(PlatformError::invalid_request("not_simulated", "simulate_call works only on a sandbox project's simulated numbers."));
    }
    if !crate::carriers::is_e164(&body.from) {
        return Err(PlatformError::invalid_request("invalid_from", "from must be an E.164 number.").with_param("from"));
    }
    let agent = bound_agent(&state, &caller, &number).await?;
    require_spend(db, &caller.project_id).await?;
    let slot = slots::acquire_slot(db, &caller.project_id, KIND_CALL).await?;
    phone::record_inbound_call(db, &number.id, &body.from).await?;
    let row = insert(db, NewCall {
        project_id: &caller.project_id, agent_id: &agent.id, account_id: &agent.account_id, number_id: Some(&number.id), direction: "inbound",
        from: Some(&body.from), to: Some(&number.e164), purpose: None, status: "in_progress", simulated: true,
        room: None, consent_ref: None, recording: false, slot_id: Some(slot.slot_id()), client_identity: None,
    })
    .await?;
    slot.detach();
    add_line(db, &row.id, "agent", &agent.greeting, Some("greeting")).await?;
    let _ = emit_event(db, &caller.project_id, Some(&row.account_id), "call.started", call_event_data(&row)).await;
    Ok((StatusCode::CREATED, Json(Call::from(&row))))
}

async fn bound_agent(state: &ApiState, caller: &PlatformCaller, number: &ApiNumber) -> Result<Agent, PlatformError> {
    let agent_id = number
        .agent_id
        .clone()
        .ok_or_else(|| PlatformError::invalid_request("number_has_no_agent", "Bind an agent to this number first: PATCH /v1/numbers/{id} with agent_id."))?;
    agents::fetch_visible(state, caller, &agent_id).await
}

// ---------------------------------------------------------------- realtime

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RealtimeBody {
    agent_id: String,
}

/// An ephemeral client token for talking to an agent from a browser or app:
/// bound to one session (one LiveKit room, one participant identity), usable
/// for 60 seconds to join, so the project key never ships to a client.
async fn create_realtime_session(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    deps: DepsExt,
    ApiJson(body): ApiJson<RealtimeBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require("voice")?;
    let db = &state.db;
    sweep_unanswered(db).await;
    let agent = agents::fetch_visible(&state, &caller, &body.agent_id).await?;
    require_spend(db, &caller.project_id).await?;
    let expires = Utc::now() + chrono::Duration::seconds(REALTIME_TOKEN_TTL_SECS);
    if caller.project_env == ProjectEnv::Sandbox {
        let slot = slots::acquire_slot(db, &caller.project_id, KIND_CALL).await?;
        let row = insert(db, NewCall {
            project_id: &caller.project_id, agent_id: &agent.id, account_id: &agent.account_id, number_id: None, direction: "realtime", from: None, to: None, purpose: None,
            status: "in_progress", simulated: true, room: None, consent_ref: None, recording: false, slot_id: Some(slot.slot_id()), client_identity: None,
        })
        .await?;
        slot.detach();
        add_line(db, &row.id, "agent", &agent.greeting, Some("greeting")).await?;
        let _ = emit_event(db, &caller.project_id, Some(&row.account_id), "call.started", call_event_data(&row)).await;
        return Ok((StatusCode::CREATED, Json(json!({
            "id": row.id, "object": "realtime.session", "call_id": row.id, "agent_id": agent.id, "simulated": true,
            "client_secret": null, "url": null, "room": null, "expires_at": expires,
        }))));
    }
    let deps = voice_deps(deps).ok_or_else(voice_unavailable)?;
    let slot = slots::acquire_slot(db, &caller.project_id, KIND_CALL).await?;
    let room = format!("{REALTIME_ROOM_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let identity = format!("client-{}", uuid::Uuid::new_v4().simple());
    let row = insert(db, NewCall {
        project_id: &caller.project_id, agent_id: &agent.id, account_id: &agent.account_id, number_id: None, direction: "realtime", from: None, to: None, purpose: None,
        status: "queued", simulated: false, room: Some(&room), consent_ref: None, recording: false, slot_id: Some(slot.slot_id()), client_identity: Some(&identity),
    })
    .await?;
    let attrs = json!({ "direction": "realtime", "botId": agent.id, "ownerId": runtime_owner(&caller.project_id), "callerIdentity": identity, "callId": row.id });
    let access = async {
        deps.livekit.create_room_with_agent(&room, SIP_AGENT_NAME, &attrs.to_string()).await?;
        deps.livekit.participant_access_ttl(&room, &identity, true, REALTIME_TOKEN_TTL_SECS)
    }
    .await;
    let access = match access {
        Ok(a) => a,
        Err(e) => {
            finish(db, &row.id, "failed", "session_failed", Some(0), None).await?;
            slot.release().await?;
            return Err(livekit_failed(e));
        }
    };
    slot.detach();
    sqlx::query("UPDATE platform_calls SET status = 'ringing', updated_at = NOW() WHERE id = $1").bind(&row.id).execute(db).await?;
    Ok((StatusCode::CREATED, Json(json!({
        "id": row.id, "object": "realtime.session", "call_id": row.id, "agent_id": agent.id, "simulated": false,
        "client_secret": { "value": access.token, "expires_at": expires }, "url": access.url, "room": room, "expires_at": expires,
    }))))
}

// ---------------------------------------------------------------- the worker side

/// The caller of record for work the voice worker starts (no API key is
/// involved): the project, bound to the call's account.
async fn internal_caller(db: &PgPool, project_id: &str, account_id: &str) -> Result<PlatformCaller, PlatformError> {
    let (owner, org, env, plan): (String, Option<String>, String, String) =
        sqlx::query_as("SELECT owner_user_id, org_id, env, plan FROM platform_projects WHERE id = $1 AND archived_at IS NULL")
            .bind(project_id)
            .fetch_optional(db)
            .await?
            .ok_or_else(|| PlatformError::not_found("project_not_found", "The project does not exist."))?;
    Ok(PlatformCaller {
        project_id: project_id.to_string(),
        project_env: ProjectEnv::parse(&env).unwrap_or(ProjectEnv::Sandbox),
        account_id: Some(account_id.to_string()),
        key_id: "voice_worker".into(),
        scopes: vec!["voice".into(), "agents".into()],
        owner_user_id: owner,
        org_id: org,
        plan: Plan::parse(&plan),
        rpm_override: None,
        call_cap_override: None,
    })
}

fn worker_err(e: PlatformError) -> ApiError {
    match e.status {
        StatusCode::NOT_FOUND => ApiError::NotFound(e.message),
        StatusCode::FORBIDDEN | StatusCode::PAYMENT_REQUIRED | StatusCode::TOO_MANY_REQUESTS => ApiError::Forbidden(e.code),
        StatusCode::CONFLICT => ApiError::Conflict(e.code),
        _ => ApiError::Internal(e.message),
    }
}

/// What the worker's `POST /api/v1/voice/calls` sent.
pub struct WorkerStart<'a> {
    pub number_id: &'a str,
    pub room: &'a str,
    pub direction: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    pub sip_call_id: Option<&'a str>,
    pub consent_ref: Option<&'a str>,
}

/// The worker started a call. `Ok(None)` when it isn't a Platform API call
/// (an app number), so the caller's own path runs. Otherwise the call is
/// registered for the worker (`voice_calls`, owner `platform:<project>`) and
/// the answer is the agent's voice config.
pub async fn worker_start(db: &PgPool, s: WorkerStart<'_>) -> Result<Option<Value>, ApiError> {
    let row = if s.room.starts_with(REALTIME_ROOM_PREFIX) {
        let call = sqlx::query_as::<_, CallRow>(&format!("SELECT {COLUMNS} FROM platform_calls WHERE room = $1 AND direction = 'realtime'"))
            .bind(s.room)
            .fetch_optional(db)
            .await?
            .ok_or_else(|| ApiError::NotFound("realtime session not found".into()))?;
        if !matches!(call.status.as_str(), "queued" | "ringing") {
            return Err(ApiError::Conflict("session_not_open".into()));
        }
        call
    } else {
        let number: Option<(Option<String>, Option<String>, Option<String>, String)> =
            sqlx::query_as("SELECT project_id, account_id, agent_id, e164 FROM phone_numbers WHERE id = $1 AND released_at IS NULL")
                .bind(s.number_id)
                .fetch_optional(db)
                .await?;
        let Some((Some(project_id), Some(account_id), agent_id, _e164)) = number else { return Ok(None) };
        if s.direction == "outbound" {
            let consent = s.consent_ref.ok_or_else(|| ApiError::Forbidden("consent_ref_required".into()))?;
            let call = sqlx::query_as::<_, CallRow>(&format!("SELECT {COLUMNS} FROM platform_calls WHERE consent_ref = $1 AND number_id = $2"))
                .bind(consent)
                .bind(s.number_id)
                .fetch_optional(db)
                .await?
                .ok_or_else(|| ApiError::NotFound("call not found".into()))?;
            if !matches!(call.status.as_str(), "queued" | "ringing") {
                return Err(ApiError::Conflict("call_not_open".into()));
            }
            call
        } else {
            // Inbound: the number's agent answers, inside the project's limits.
            let agent_id = agent_id.ok_or_else(|| ApiError::NotFound("number_has_no_agent".into()))?;
            let agent = sqlx::query_scalar::<_, String>("SELECT id FROM platform_agents WHERE id = $1 AND account_id = $2 AND deleted_at IS NULL")
                .bind(&agent_id)
                .bind(&account_id)
                .fetch_optional(db)
                .await?;
            let Some(agent_id) = agent else { return Err(ApiError::NotFound("agent not found".into())) };
            if !spend::spend_allowed(db, &project_id).await.map_err(worker_err)? {
                return Err(ApiError::Forbidden("spend_cap_reached".into()));
            }
            let slot = slots::acquire_slot(db, &project_id, KIND_CALL).await.map_err(worker_err)?;
            phone::record_inbound_call(db, s.number_id, s.from).await?;
            let call = insert(db, NewCall {
                project_id: &project_id, agent_id: &agent_id, account_id: &account_id, number_id: Some(s.number_id), direction: "inbound", from: Some(s.from), to: Some(s.to),
                purpose: None, status: "ringing", simulated: false, room: Some(s.room), consent_ref: None, recording: false,
                slot_id: Some(slot.slot_id()), client_identity: None,
            })
            .await
            .map_err(worker_err)?;
            slot.detach();
            call
        }
    };
    let agent: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT name, instructions, voice, greeting FROM platform_agents WHERE id = $1").bind(&row.agent_id).fetch_optional(db).await?;
    let Some((name, instructions, voice, greeting)) = agent else { return Err(ApiError::NotFound("agent not found".into())) };
    sqlx::query(
        "INSERT INTO voice_calls (call_id, user_id, runtime_id, number_id, bot_id, room, direction, from_e164, to_e164, sip_call_id, consent_ref) \
         VALUES ($1, $2, '', $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (call_id) DO NOTHING",
    )
    .bind(&row.id)
    .bind(runtime_owner(&row.project_id))
    .bind(row.number_id.as_deref().unwrap_or(""))
    .bind(&row.agent_id)
    .bind(s.room)
    .bind(if s.direction == "outbound" { "outbound" } else { "inbound" })
    .bind(row.from_e164.as_deref().unwrap_or(s.from))
    .bind(row.to_e164.as_deref().unwrap_or(s.to))
    .bind(s.sip_call_id)
    .bind(row.consent_ref.as_deref())
    .execute(db)
    .await?;
    let row = sqlx::query_as::<_, CallRow>(&format!(
        "UPDATE platform_calls SET status = 'in_progress', answered_at = COALESCE(answered_at, NOW()), room = COALESCE(room, $2), updated_at = NOW() WHERE id = $1 RETURNING {COLUMNS}"
    ))
    .bind(&row.id)
    .bind(s.room)
    .fetch_one(db)
    .await?;
    let _ = emit_event(db, &row.project_id, Some(&row.account_id), "call.started", call_event_data(&row)).await;
    let persona: String = instructions.chars().take(2000).collect();
    Ok(Some(json!({
        "callId": row.id,
        "bot": { "name": name, "persona": if persona.is_empty() { "A helpful AI assistant.".to_string() } else { persona }, "voiceId": voice, "greeting": greeting, "recording": row.recording },
    })))
}

/// A worker turn on a Platform API call: run it in the agent's hosted-runtime
/// session. `Ok(None)` when the call isn't one of ours.
pub async fn worker_turn(state: &Arc<ApiState>, call_id: &str, text: &str) -> Result<Option<crate::routes::voice_calls_cloud::RelayStream>, ApiError> {
    let Some(call) = load(&state.db, call_id).await? else { return Ok(None) };
    if call.status != "in_progress" {
        return Err(ApiError::Conflict("call ended".into()));
    }
    let conversation = call.conversation_id.clone().ok_or_else(|| ApiError::Internal("call has no session".into()))?;
    let caller = internal_caller(&state.db, &call.project_id, &call.account_id).await.map_err(worker_err)?;
    let host = host_for(state, None);
    let (rt, path) = conversations::turn_target(state, host.as_ref(), &caller, &conversation).await.map_err(|e| {
        if e.code == "runtime_starting" {
            ApiError::ServiceUnavailable("runtime_starting".into())
        } else {
            worker_err(e)
        }
    })?;
    let stream = host.stream(&rt, &path, &json!({ "text": text })).await.map_err(|e| ApiError::ServiceUnavailable(e.code))?;
    Ok(Some(stream))
}

/// Apply one worker event to a Platform API call (instead of relaying it to a
/// user's runtime). Unknown types are ignored.
pub async fn apply_worker_event(db: &PgPool, call_id: &str, event_type: &str, payload: &Value) -> Result<(), ApiError> {
    match event_type {
        "call.transcript.delta" if payload["final"].as_bool() == Some(true) => {
            let speaker = match payload["speaker"].as_str() {
                Some("caller") => "caller",
                Some("human") => "human",
                _ => "agent",
            };
            add_line(db, call_id, speaker, payload["text"].as_str().unwrap_or(""), payload["segmentId"].as_str()).await?;
        }
        "call.transferred" if payload["ok"].as_bool() == Some(true) => {
            sqlx::query("UPDATE platform_calls SET transferred_to = $2, end_reason = 'transferred', updated_at = NOW() WHERE id = $1")
                .bind(call_id)
                .bind(payload["to"].as_str())
                .execute(db)
                .await?;
        }
        "call.ended" => {
            let answered = payload["answered"].as_bool().unwrap_or(true);
            let status = if answered { "completed" } else { "no_answer" };
            let reason = payload["reason"].as_str().unwrap_or("hangup");
            let duration = payload["durationSec"].as_i64();
            finish(db, call_id, status, reason, duration, payload["recordingRef"].as_str()).await.map_err(worker_err)?;
        }
        _ => {}
    }
    Ok(())
}

/// Is this `voice_calls` owner a Platform API project's hosted runtime?
pub fn is_platform_owner(user_id: &str) -> bool {
    user_id.starts_with("platform:")
}

// ---------------------------------------------------------------- number binding

/// Bind (or unbind, `None`) the agent that answers a number. The agent must be
/// in the number's account. A live number gets its LiveKit inbound trunk and
/// dispatch rule the first time an agent is bound (when LiveKit is configured).
pub async fn bind_agent(state: &ApiState, caller: &PlatformCaller, number: &ApiNumber, agent_id: Option<&str>, deps: DepsExt) -> Result<(), PlatformError> {
    let db = &state.db;
    if let Some(agent_id) = agent_id {
        let agent = agents::fetch_visible(state, caller, agent_id).await.map_err(|e| e.with_param("agent_id"))?;
        if number.account_id.as_deref() != Some(agent.account_id.as_str()) {
            return Err(PlatformError::invalid_request("account_mismatch", "The number and the agent belong to different accounts.").with_param("agent_id"));
        }
    }
    sqlx::query("UPDATE phone_numbers SET agent_id = $2 WHERE id = $1").bind(&number.id).bind(agent_id).execute(db).await?;
    if number.simulated || agent_id.is_none() {
        return Ok(());
    }
    let (trunk, rule): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT livekit_trunk_id, livekit_dispatch_rule_id FROM phone_numbers WHERE id = $1").bind(&number.id).fetch_one(db).await?;
    if trunk.is_some() && rule.is_some() {
        return Ok(());
    }
    let Some(deps) = voice_deps(deps) else {
        tracing::warn!(number = %number.id, "platform voice: LiveKit isn't configured; the number can't answer calls yet");
        return Ok(());
    };
    // The rule's botId is informational: the call start reads the bound agent from the number.
    let provisioned = async {
        let trunk = deps.livekit.ensure_inbound_trunk(&number.id, &number.e164).await?;
        let rule = deps.livekit.ensure_dispatch_rule(&trunk, &number.id, "platform", &runtime_owner(&caller.project_id), &number.e164).await?;
        Ok::<_, LiveKitError>((trunk, rule))
    }
    .await;
    match provisioned {
        Ok((trunk, rule)) => {
            sqlx::query("UPDATE phone_numbers SET livekit_trunk_id = $2, livekit_dispatch_rule_id = $3, voice_state = 'active' WHERE id = $1")
                .bind(&number.id)
                .bind(trunk)
                .bind(rule)
                .execute(db)
                .await?;
            Ok(())
        }
        Err(e) => Err(livekit_failed(e)),
    }
}

/// Remove a released number's LiveKit trunk and rule (best effort).
pub async fn unprovision(db: &PgPool, number_id: &str, deps: DepsExt) {
    let ids: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT livekit_trunk_id, livekit_dispatch_rule_id FROM phone_numbers WHERE id = $1").bind(number_id).fetch_optional(db).await.ok().flatten();
    let Some((trunk, rule)) = ids else { return };
    if trunk.is_none() && rule.is_none() {
        return;
    }
    let Some(deps) = voice_deps(deps) else { return };
    if let Some(r) = rule {
        let _ = deps.livekit.delete_dispatch_rule(&r).await;
    }
    if let Some(t) = trunk {
        let _ = deps.livekit.delete_inbound_trunk(&t).await;
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn business_hours_parse_and_apply() {
        let hours = json!({ "tz": "America/Chicago", "mon": ["09:00", "17:00"] });
        assert!(validate_business_hours(&hours).is_ok());
        // Monday 2026-10-05 15:00 UTC = 10:00 in Chicago (CDT).
        assert!(within_business_hours(&hours, Utc.with_ymd_and_hms(2026, 10, 5, 15, 0, 0).unwrap()));
        // 23:30 UTC = 18:30 Chicago: closed.
        assert!(!within_business_hours(&hours, Utc.with_ymd_and_hms(2026, 10, 5, 23, 30, 0).unwrap()));
        // Tuesday isn't listed: closed.
        assert!(!within_business_hours(&hours, Utc.with_ymd_and_hms(2026, 10, 6, 15, 0, 0).unwrap()));
        // Only a time zone: always open.
        assert!(within_business_hours(&json!({ "tz": "UTC" }), Utc::now()));
        for bad in [json!({}), json!({ "tz": "Mars/Base" }), json!({ "tz": "UTC", "monday": ["09:00", "17:00"] }), json!({ "tz": "UTC", "mon": ["17:00", "09:00"] }), json!({ "tz": "UTC", "mon": ["9am", "5pm"] })] {
            assert!(validate_business_hours(&bad).is_err(), "{bad}");
            assert!(!within_business_hours(&bad, Utc::now()), "unparseable hours are closed: {bad}");
        }
    }

    #[test]
    fn only_us_and_canada() {
        assert!(is_nanp("+14155550100"));
        assert!(!is_nanp("+447700900123"));
        assert!(!is_nanp("+1415555010"));
    }
}
