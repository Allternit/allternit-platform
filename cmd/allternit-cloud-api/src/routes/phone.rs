//! Phone numbers and SMS for bots (cloud side).
//!
//! A bot gets a real number from the carrier ([`crate::carriers`]). Each number
//! has a relay address (provider `sms`, [`super::channel_inbound`]) the carrier
//! posts inbound texts to. Before a text is queued for the user's runtime,
//! [`sms_edge`] verifies the carrier signature, dedupes, and handles
//! STOP/HELP/START itself, so an opted-out sender never reaches the runtime.
//!
//! Routes (Clerk session or `compute`-scoped API key, like `channel-inbound-routes`):
//! - `GET    /api/v1/phone/numbers/search?country=US&areaCode=415&locality=&type=local|toll_free&limit=10`
//! - `POST   /api/v1/phone/numbers` {e164, runtimeId, botId}              buy and assign
//! - `POST   /api/v1/phone/numbers/port` {e164, runtimeId, botId}          port-in
//! - `GET    /api/v1/phone/numbers`
//! - `DELETE /api/v1/phone/numbers/:id`
//! - `POST|GET /api/v1/phone/numbers/:id/registration`                     10DLC / toll-free verification
//! - `GET    /api/v1/phone/numbers/:id/port`                               port-in status
//! - `POST   /api/v1/phone/numbers/:id/consent` {e164, source, evidence}   explicit consent record
//! - `POST   /api/v1/phone/calls/outbound` {numberId, to, botId, purpose}  consent gate for calls; with
//!   `ALLTERNIT_LIVEKIT_OUTBOUND_TRUNK_ID` + LiveKit env it also dials and returns `{room, dialing:true}`
//! - `POST   /api/v1/channels/sms/send` {numberId, to, text}               SMS out
//! - `POST   /api/v1/phone/webhooks/:carrier`                              carrier status events (signed)
//!
//! Unset carrier env → 503 `{"error":"phone_not_configured"}`; nothing here runs at boot.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

use crate::carriers::{
    self, Carrier, CarrierError, InboundEvent, NumberType, RegState, RegistrationForm, RegistrationHandle, RegistrationKind, ReqwestHttp, SearchQuery,
};
use super::livekit_admin::{CreateSipParticipantRequest, LiveKitAdminClient, LiveKitConfig, LiveKitError, LiveKitHttpAdmin, CALL_ROOM_PREFIX, SIP_AGENT_NAME};
use crate::services::voice_usage::plan_for_user;
use crate::{ApiError, ApiState};

/// Longest SMS the runtime may send in one call (it splits longer replies).
pub const MAX_SMS_CHARS: usize = 1600;
/// Outbound texts per number per rolling day before 429 (`ALLTERNIT_SMS_DAILY_CAP` overrides).
const DEFAULT_DAILY_CAP: i64 = 1000;
/// Numbers one user may hold, by plan. A plan not listed (free) includes no
/// phone numbers: buying or porting returns 402 `phone_requires_plan`.
/// `ALLTERNIT_PHONE_MAX_PER_USER` overrides the limit for every paid plan.
const PLAN_NUMBER_LIMITS: [(&str, i64); 3] = [("plus", 3), ("super", 5), ("ultra", 10)];

/// Numbers a plan may hold; `None` when the plan does not include phone.
pub fn number_limit_for_plan(plan: &str) -> Option<i64> {
    let base = PLAN_NUMBER_LIMITS.iter().find(|(p, _)| *p == plan).map(|(_, n)| *n)?;
    Some(env_i64("ALLTERNIT_PHONE_MAX_PER_USER", base))
}
/// How long a call consent stays valid for the dial.
const CALL_CONSENT_MINUTES: i64 = 30;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/phone/numbers/search", get(search_numbers))
        .route("/api/v1/phone/numbers", get(list_numbers).post(buy_number_route))
        .route("/api/v1/phone/numbers/port", post(port_create_route))
        .route("/api/v1/phone/numbers/:id", delete(release_number_route))
        .route("/api/v1/phone/numbers/:id/registration", get(registration_get_route).post(registration_post_route))
        .route("/api/v1/phone/numbers/:id/port", get(port_status_route))
        .route("/api/v1/phone/numbers/:id/consent", post(consent_route))
        .route("/api/v1/phone/calls/outbound", post(call_outbound_route))
        .route("/api/v1/channels/sms/send", post(sms_send_route))
        .route("/api/v1/phone/webhooks/:carrier", post(carrier_webhook_route))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PhoneError {
    NotConfigured,
    BadRequest(String),
    NotFound(&'static str),
    Forbidden(&'static str),
    PlanRequired,
    Conflict(&'static str),
    TooMany(&'static str),
    Carrier(CarrierError),
    Db(sqlx::Error),
    Auth(ApiError),
}

impl From<sqlx::Error> for PhoneError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

impl From<CarrierError> for PhoneError {
    fn from(e: CarrierError) -> Self {
        match e {
            CarrierError::NotConfigured => Self::NotConfigured,
            other => Self::Carrier(other),
        }
    }
}

impl From<ApiError> for PhoneError {
    fn from(e: ApiError) -> Self {
        Self::Auth(e)
    }
}

fn err(status: StatusCode, code: &str, message: Option<String>) -> Response {
    let mut body = json!({ "error": code });
    if let Some(m) = message {
        body["message"] = json!(m);
    }
    (status, Json(body)).into_response()
}

impl IntoResponse for PhoneError {
    fn into_response(self) -> Response {
        match self {
            Self::NotConfigured => err(StatusCode::SERVICE_UNAVAILABLE, "phone_not_configured", None),
            Self::BadRequest(m) => err(StatusCode::BAD_REQUEST, "bad_request", Some(m)),
            Self::NotFound(code) => err(StatusCode::NOT_FOUND, code, None),
            Self::Forbidden(code) => err(StatusCode::FORBIDDEN, code, None),
            Self::PlanRequired => err(StatusCode::PAYMENT_REQUIRED, "phone_requires_plan", Some("Phone numbers need a paid plan. Upgrade to add one.".into())),
            Self::Conflict(code) => err(StatusCode::CONFLICT, code, None),
            Self::TooMany(code) => err(StatusCode::TOO_MANY_REQUESTS, code, None),
            Self::Carrier(CarrierError::Invalid(m)) => err(StatusCode::BAD_REQUEST, "bad_request", Some(m)),
            Self::Carrier(CarrierError::Unsupported(what)) => err(StatusCode::NOT_IMPLEMENTED, "carrier_unsupported", Some(format!("{what} is not supported on this carrier"))),
            Self::Carrier(CarrierError::BadSignature) => err(StatusCode::UNAUTHORIZED, "bad_signature", None),
            Self::Carrier(e) => {
                tracing::warn!("phone carrier error: {e}");
                err(StatusCode::BAD_GATEWAY, "carrier_error", Some(e.to_string()))
            }
            Self::Db(e) => {
                tracing::error!("phone db error: {e}");
                err(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", None)
            }
            Self::Auth(e) => e.into_response(),
        }
    }
}

type PResult<T> = Result<T, PhoneError>;

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NumberRow {
    pub id: String,
    pub user_id: String,
    pub runtime_id: String,
    pub bot_id: String,
    pub e164: String,
    pub carrier: String,
    pub carrier_number_id: Option<String>,
    pub messaging_ref: Option<String>,
    #[sqlx(rename = "type")]
    pub kind: String,
    pub sms_state: String,
    pub voice_state: String,
    pub inbound_route_id: Option<String>,
    pub port_order_id: Option<String>,
    pub port_state: Option<String>,
    pub created_at: DateTime<Utc>,
}

const NUMBER_COLS: &str = "id, user_id, runtime_id, bot_id, e164, carrier, carrier_number_id, messaging_ref, type, sms_state, voice_state, inbound_route_id, port_order_id, port_state, created_at";

impl NumberRow {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id, "runtimeId": self.runtime_id, "botId": self.bot_id, "e164": self.e164,
            "carrier": self.carrier, "type": self.kind, "smsState": self.sms_state, "voiceState": self.voice_state,
            "portState": self.port_state, "createdAt": self.created_at,
        })
    }
}

async fn number_for_user(db: &PgPool, user: &str, id: &str) -> PResult<NumberRow> {
    sqlx::query_as::<_, NumberRow>(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE id = $1 AND user_id = $2 AND released_at IS NULL"))
        .bind(id)
        .bind(user)
        .fetch_optional(db)
        .await?
        .ok_or(PhoneError::NotFound("number_not_found"))
}

fn http() -> Arc<dyn carriers::CarrierHttp> {
    Arc::new(ReqwestHttp::new())
}

fn carrier() -> PResult<Arc<dyn Carrier>> {
    Ok(carriers::from_env(http())?)
}

async fn user_id(state: &ApiState, headers: &HeaderMap) -> PResult<String> {
    Ok(crate::auth::resolve_user_scoped(&state.db, headers, "compute").await?.id)
}

fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).filter(|v| *v > 0).unwrap_or(default)
}

// ---------------------------------------------------------------------------
// Numbers: search, buy, list, release
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    country: Option<String>,
    area_code: Option<String>,
    locality: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    limit: Option<u32>,
}

async fn search_numbers(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(p): Query<SearchParams>) -> Response {
    let run = async {
        user_id(&state, &headers).await?;
        let carrier = carrier()?;
        let kind = match p.kind.as_deref() {
            None => NumberType::Local,
            Some(k) => NumberType::parse(k).ok_or_else(|| PhoneError::BadRequest("type must be local or toll_free".into()))?,
        };
        let country = p.country.unwrap_or_else(|| "US".into()).to_ascii_uppercase();
        if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(PhoneError::BadRequest("country must be a 2-letter code".into()));
        }
        let found = carrier.search(&SearchQuery { country, area_code: p.area_code, locality: p.locality, kind, limit: p.limit.unwrap_or(10) }).await?;
        Ok::<_, PhoneError>(Json(json!({ "carrier": carrier.name(), "numbers": found })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuyBody {
    e164: String,
    runtime_id: String,
    bot_id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}

async fn owns_runtime(db: &PgPool, user: &str, runtime_id: &str) -> PResult<()> {
    let owns: Option<(String,)> = sqlx::query_as("SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL")
        .bind(runtime_id)
        .bind(user)
        .fetch_optional(db)
        .await?;
    owns.map(|_| ()).ok_or(PhoneError::NotFound("runtime_not_found"))
}

/// Create the number's relay address; returns (route id, public url).
async fn create_sms_route(db: &PgPool, user: &str, runtime_id: &str, e164: &str) -> PResult<(String, String)> {
    let key = super::channel_inbound::new_key();
    let route_id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider, label) VALUES ($1, $2, $3, $4, 'sms', $5)")
        .bind(&route_id)
        .bind(super::channel_inbound::sha256_hex(&key))
        .bind(user)
        .bind(runtime_id)
        .bind(e164)
        .execute(db)
        .await?;
    Ok((route_id, format!("{}/channels/in/{}", super::channel_inbound::public_base(), key)))
}

async fn revoke_route(db: &PgPool, route_id: &str) {
    let _ = sqlx::query("UPDATE channel_inbound_routes SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL").bind(route_id).execute(db).await;
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}

/// Reserve the number's row first (the unique index stops a double purchase),
/// then buy it; undo the row and route if the carrier refuses.
pub async fn buy_number(db: &PgPool, carrier: &dyn Carrier, user: &str, body: &BuyInputs) -> PResult<NumberRow> {
    if !carriers::is_e164(&body.e164) {
        return Err(PhoneError::BadRequest("e164 must be a number like +14155550101".into()));
    }
    if body.bot_id.trim().is_empty() {
        return Err(PhoneError::BadRequest("botId is required".into()));
    }
    owns_runtime(db, user, &body.runtime_id).await?;
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_numbers WHERE user_id = $1 AND released_at IS NULL").bind(user).fetch_one(db).await?;
    let limit = number_limit_for_plan(&body.plan).ok_or(PhoneError::PlanRequired)?;
    if held >= limit {
        return Err(PhoneError::Forbidden("number_limit"));
    }
    let kind = body.kind;
    let id = uuid::Uuid::new_v4().to_string();
    let (route_id, webhook_url) = create_sms_route(db, user, &body.runtime_id, &body.e164).await?;
    let reserved = sqlx::query(
        "INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, type, inbound_route_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&id)
    .bind(user)
    .bind(&body.runtime_id)
    .bind(&body.bot_id)
    .bind(&body.e164)
    .bind(carrier.name())
    .bind(kind.as_str())
    .bind(&route_id)
    .execute(db)
    .await;
    if let Err(e) = reserved {
        revoke_route(db, &route_id).await;
        return Err(if is_unique_violation(&e) { PhoneError::Conflict("number_taken") } else { e.into() });
    }
    let bought = carrier.buy(&carriers::BuyRequest { e164: body.e164.clone(), kind, webhook_url, reference: id.clone() }).await;
    let bought = match bought {
        Ok(b) => b,
        Err(e) => {
            let _ = sqlx::query("DELETE FROM phone_numbers WHERE id = $1").bind(&id).execute(db).await;
            revoke_route(db, &route_id).await;
            return Err(e.into());
        }
    };
    sqlx::query("UPDATE phone_numbers SET carrier_number_id = $2, messaging_ref = $3 WHERE id = $1")
        .bind(&id)
        .bind(&bought.carrier_number_id)
        .bind(&bought.messaging_ref)
        .execute(db)
        .await?;
    sqlx::query_as::<_, NumberRow>(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE id = $1")).bind(&id).fetch_one(db).await.map_err(Into::into)
}

pub struct BuyInputs {
    pub e164: String,
    pub runtime_id: String,
    pub bot_id: String,
    pub kind: NumberType,
    /// The user's plan id (`plan_for_user`); gates phone and sets the number limit.
    pub plan: String,
}

fn infer_type(e164: &str) -> NumberType {
    // North American toll-free area codes.
    const TOLL_FREE: [&str; 8] = ["800", "833", "844", "855", "866", "877", "888", "889"];
    match e164.strip_prefix("+1").and_then(|r| r.get(..3)) {
        Some(area) if TOLL_FREE.contains(&area) => NumberType::TollFree,
        _ => NumberType::Local,
    }
}

async fn buy_number_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<BuyBody>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let plan = plan_for_user(&state.db, &user).await?;
        let carrier = carrier()?;
        let kind = match body.kind.as_deref() {
            Some(k) => NumberType::parse(k).ok_or_else(|| PhoneError::BadRequest("type must be local or toll_free".into()))?,
            None => infer_type(&body.e164),
        };
        let row = buy_number(&state.db, carrier.as_ref(), &user, &BuyInputs { e164: body.e164, runtime_id: body.runtime_id, bot_id: body.bot_id, kind, plan }).await?;
        Ok::<_, PhoneError>((StatusCode::CREATED, Json(json!({ "number": row.to_json() }))).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn list_numbers(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let rows: Vec<NumberRow> = sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE user_id = $1 AND released_at IS NULL ORDER BY created_at"))
            .bind(&user)
            .fetch_all(&state.db)
            .await?;
        Ok::<_, PhoneError>(Json(json!({ "numbers": rows.iter().map(NumberRow::to_json).collect::<Vec<_>>() })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

pub async fn release_number(db: &PgPool, carrier: &dyn Carrier, user: &str, id: &str) -> PResult<()> {
    let row = number_for_user(db, user, id).await?;
    carrier.release(&row.e164, row.carrier_number_id.as_deref(), row.messaging_ref.as_deref()).await?;
    sqlx::query("UPDATE phone_numbers SET released_at = now() WHERE id = $1").bind(id).execute(db).await?;
    if let Some(route) = &row.inbound_route_id {
        revoke_route(db, route).await;
    }
    Ok(())
}

async fn release_number_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let carrier = carrier()?;
        release_number(&state.db, carrier.as_ref(), &user, &id).await?;
        Ok::<_, PhoneError>(StatusCode::NO_CONTENT.into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

// ---------------------------------------------------------------------------
// Registration (10DLC / toll-free verification)
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct RegRow {
    id: String,
    kind: String,
    brand_id: Option<String>,
    campaign_id: Option<String>,
    tfv_id: Option<String>,
    state: String,
    rejection_reason: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const REG_COLS: &str = "id, kind, brand_id, campaign_id, tfv_id, state, rejection_reason, created_at, updated_at";

fn sms_state_for(reg: &str) -> &'static str {
    match reg {
        "approved" => "active",
        "rejected" => "rejected",
        _ => "pending_registration",
    }
}

fn reg_json(r: &RegRow, number: &NumberRow) -> Value {
    json!({
        "registration": {
            "id": r.id, "kind": r.kind, "state": r.state, "rejectionReason": r.rejection_reason,
            "brandId": r.brand_id, "campaignId": r.campaign_id, "tfvId": r.tfv_id,
            "submittedAt": r.created_at, "updatedAt": r.updated_at,
        },
        "smsState": number.sms_state,
    })
}

async fn latest_registration(db: &PgPool, number_id: &str) -> PResult<Option<RegRow>> {
    Ok(sqlx::query_as::<_, RegRow>(&format!("SELECT {REG_COLS} FROM sms_registrations WHERE number_id = $1 ORDER BY created_at DESC, id LIMIT 1"))
        .bind(number_id)
        .fetch_optional(db)
        .await?)
}

#[derive(Deserialize)]
struct RegBody {
    kind: Option<String>,
    #[serde(flatten)]
    form: RegistrationForm,
}

fn mask_ein(form: &RegistrationForm) -> Value {
    let mut v = serde_json::to_value(form).unwrap_or(Value::Null);
    if let Some(ein) = form.ein.as_deref().filter(|e| e.len() > 4) {
        v["ein"] = json!(format!("•••{}", &ein[ein.len() - 4..]));
    }
    v
}

pub async fn submit_registration(db: &PgPool, carrier: &dyn Carrier, user: &str, number_id: &str, kind: Option<&str>, form: RegistrationForm) -> PResult<Value> {
    let number = number_for_user(db, user, number_id).await?;
    let kind = match kind {
        Some("10dlc") => RegistrationKind::TenDlc,
        Some("tollfree") | Some("toll_free") => RegistrationKind::TollFree,
        Some(_) => return Err(PhoneError::BadRequest("kind must be 10dlc or tollfree".into())),
        None if number.kind == "toll_free" => RegistrationKind::TollFree,
        None => RegistrationKind::TenDlc,
    };
    if let Some(existing) = latest_registration(db, number_id).await? {
        if existing.state != "rejected" {
            return Err(PhoneError::Conflict("already_registered"));
        }
    }
    let handle = carrier.submit_registration(kind, &number.e164, number.carrier_number_id.as_deref(), number.messaging_ref.as_deref(), &form).await?;
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO sms_registrations (id, number_id, kind, carrier, brand_id, campaign_id, tfv_id, fields) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(&id)
        .bind(number_id)
        .bind(kind.as_str())
        .bind(carrier.name())
        .bind(&handle.brand_id)
        .bind(&handle.campaign_id)
        .bind(&handle.tfv_id)
        .bind(mask_ein(&form))
        .execute(db)
        .await?;
    sqlx::query("UPDATE phone_numbers SET sms_state = 'pending_registration' WHERE id = $1 AND sms_state <> 'blocked'").bind(number_id).execute(db).await?;
    let number = number_for_user(db, user, number_id).await?;
    let row = latest_registration(db, number_id).await?.ok_or(PhoneError::NotFound("no_registration"))?;
    Ok(reg_json(&row, &number))
}

/// Ask the carrier where a pending registration stands and apply it.
pub async fn refresh_registration(db: &PgPool, carrier: &dyn Carrier, number: &NumberRow, reg: &RegRow) -> PResult<()> {
    if reg.state != "pending" {
        return Ok(());
    }
    let kind = if reg.kind == "tollfree" { RegistrationKind::TollFree } else { RegistrationKind::TenDlc };
    let handle = RegistrationHandle { brand_id: reg.brand_id.clone(), campaign_id: reg.campaign_id.clone(), tfv_id: reg.tfv_id.clone() };
    let status = carrier.registration_status(kind, &number.e164, &handle, number.messaging_ref.as_deref()).await?;
    if status.state == RegState::Pending {
        return Ok(());
    }
    sqlx::query("UPDATE sms_registrations SET state = $2, rejection_reason = $3, updated_at = now() WHERE id = $1")
        .bind(&reg.id)
        .bind(status.state.as_str())
        .bind(&status.reason)
        .execute(db)
        .await?;
    sqlx::query("UPDATE phone_numbers SET sms_state = $2 WHERE id = $1 AND sms_state <> 'blocked'").bind(&number.id).bind(sms_state_for(status.state.as_str())).execute(db).await?;
    Ok(())
}

async fn registration_post_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<RegBody>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let carrier = carrier()?;
        let out = submit_registration(&state.db, carrier.as_ref(), &user, &id, body.kind.as_deref(), body.form).await?;
        Ok::<_, PhoneError>((StatusCode::CREATED, Json(out)).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn registration_get_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let number = number_for_user(&state.db, &user, &id).await?;
        let reg = latest_registration(&state.db, &id).await?.ok_or(PhoneError::NotFound("no_registration"))?;
        // A carrier hiccup shows the stored state rather than an error; the next poll retries.
        if let Ok(carrier) = carrier() {
            if let Err(e) = refresh_registration(&state.db, carrier.as_ref(), &number, &reg).await {
                tracing::warn!(number = %id, "registration refresh failed");
                let _ = e;
            }
        }
        let number = number_for_user(&state.db, &user, &id).await?;
        let reg = latest_registration(&state.db, &id).await?.ok_or(PhoneError::NotFound("no_registration"))?;
        Ok::<_, PhoneError>(Json(reg_json(&reg, &number)).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

// ---------------------------------------------------------------------------
// Port-in
// ---------------------------------------------------------------------------

/// Start a port-in. The number gets its row and relay address now, and starts
/// texting through us once the carrier reports it ported.
pub async fn port_create(db: &PgPool, carrier: &dyn Carrier, user: &str, body: &BuyInputs) -> PResult<NumberRow> {
    if !carriers::is_e164(&body.e164) {
        return Err(PhoneError::BadRequest("e164 must be a number like +14155550101".into()));
    }
    if body.bot_id.trim().is_empty() {
        return Err(PhoneError::BadRequest("botId is required".into()));
    }
    owns_runtime(db, user, &body.runtime_id).await?;
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_numbers WHERE user_id = $1 AND released_at IS NULL").bind(user).fetch_one(db).await?;
    let limit = number_limit_for_plan(&body.plan).ok_or(PhoneError::PlanRequired)?;
    if held >= limit {
        return Err(PhoneError::Forbidden("number_limit"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let (route_id, webhook_url) = create_sms_route(db, user, &body.runtime_id, &body.e164).await?;
    let reserved = sqlx::query("INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, type, inbound_route_id, port_state) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'requested')")
        .bind(&id)
        .bind(user)
        .bind(&body.runtime_id)
        .bind(&body.bot_id)
        .bind(&body.e164)
        .bind(carrier.name())
        .bind(body.kind.as_str())
        .bind(&route_id)
        .execute(db)
        .await;
    if let Err(e) = reserved {
        revoke_route(db, &route_id).await;
        return Err(if is_unique_violation(&e) { PhoneError::Conflict("number_taken") } else { e.into() });
    }
    match carrier.port_in_create(&[body.e164.clone()], &id, &webhook_url).await {
        Ok(port) => {
            sqlx::query("UPDATE phone_numbers SET port_order_id = $2, port_state = $3, messaging_ref = $4 WHERE id = $1")
                .bind(&id)
                .bind(&port.id)
                .bind(&port.status)
                .bind(&port.messaging_ref)
                .execute(db)
                .await?;
        }
        Err(e) => {
            let _ = sqlx::query("DELETE FROM phone_numbers WHERE id = $1").bind(&id).execute(db).await;
            revoke_route(db, &route_id).await;
            return Err(e.into());
        }
    }
    sqlx::query_as::<_, NumberRow>(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE id = $1")).bind(&id).fetch_one(db).await.map_err(Into::into)
}

pub async fn port_refresh(db: &PgPool, carrier: &dyn Carrier, number: &NumberRow) -> PResult<()> {
    let Some(order) = number.port_order_id.as_deref() else { return Ok(()) };
    if number.port_state.as_deref() == Some("ported") {
        return Ok(());
    }
    let status = carrier.port_in_status(order).await?;
    sqlx::query("UPDATE phone_numbers SET port_state = $2 WHERE id = $1").bind(&number.id).bind(&status.status).execute(db).await?;
    Ok(())
}

async fn port_create_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<BuyBody>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let plan = plan_for_user(&state.db, &user).await?;
        let carrier = carrier()?;
        let kind = match body.kind.as_deref() {
            Some(k) => NumberType::parse(k).ok_or_else(|| PhoneError::BadRequest("type must be local or toll_free".into()))?,
            None => infer_type(&body.e164),
        };
        let row = port_create(&state.db, carrier.as_ref(), &user, &BuyInputs { e164: body.e164, runtime_id: body.runtime_id, bot_id: body.bot_id, kind, plan }).await?;
        Ok::<_, PhoneError>((StatusCode::CREATED, Json(json!({ "number": row.to_json() }))).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn port_status_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let number = number_for_user(&state.db, &user, &id).await?;
        if number.port_order_id.is_none() {
            return Err(PhoneError::NotFound("no_port"));
        }
        if let Ok(carrier) = carrier() {
            if port_refresh(&state.db, carrier.as_ref(), &number).await.is_err() {
                tracing::warn!(number = %id, "port status refresh failed");
            }
        }
        let number = number_for_user(&state.db, &user, &id).await?;
        Ok::<_, PhoneError>(Json(json!({ "port": { "orderId": number.port_order_id, "state": number.port_state }, "number": number.to_json() })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

// ---------------------------------------------------------------------------
// Consent
// ---------------------------------------------------------------------------

/// Words that opt a sender out, per CTIA/carrier convention.
pub const STOP_WORDS: [&str; 6] = ["STOP", "STOPALL", "UNSUBSCRIBE", "CANCEL", "END", "QUIT"];

#[derive(Debug, PartialEq, Eq)]
pub enum Keyword {
    Stop,
    Help,
    Start,
}

/// A whole-message keyword, ignoring case and surrounding space or punctuation.
pub fn classify_keyword(text: &str) -> Option<Keyword> {
    let word = text.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_uppercase();
    if STOP_WORDS.contains(&word.as_str()) {
        Some(Keyword::Stop)
    } else if word == "HELP" || word == "INFO" {
        Some(Keyword::Help)
    } else if word == "START" || word == "UNSTOP" {
        Some(Keyword::Start)
    } else {
        None
    }
}

async fn log_consent(db: &PgPool, number_id: &str, e164: &str, kind: &str, source: Option<&str>, evidence: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO sms_consent_log (number_id, e164, kind, source, evidence) VALUES ($1, $2, $3, $4, $5)")
        .bind(number_id)
        .bind(e164)
        .bind(kind)
        .bind(source)
        .bind(evidence)
        .execute(db)
        .await?;
    Ok(())
}

async fn is_opted_out(db: &PgPool, number_id: &str, e164: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sms_opt_outs WHERE number_id = $1 AND e164 = $2").bind(number_id).bind(e164).fetch_one(db).await? > 0)
}

/// The latest consent basis for a counterparty, if any.
async fn consent_basis(db: &PgPool, number_id: &str, e164: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT kind FROM sms_consent_log WHERE number_id = $1 AND e164 = $2 AND kind IN ('inbound_text', 'inbound_call', 'opt_in', 'explicit') ORDER BY id DESC LIMIT 1",
    )
    .bind(number_id)
    .bind(e164)
    .fetch_optional(db)
    .await
}

/// Voice calls this when someone rings a number first: it counts as consent to call back.
pub async fn record_inbound_call(db: &PgPool, number_id: &str, caller_e164: &str) -> Result<(), sqlx::Error> {
    if consent_basis(db, number_id, caller_e164).await?.as_deref() != Some("inbound_call") {
        log_consent(db, number_id, caller_e164, "inbound_call", Some("call"), None).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRef {
    pub id: String,
    pub basis: String,
    pub expires_at: DateTime<Utc>,
}

/// Issue a consentRef for an outbound call, or `None` when the callee never texted or
/// called this number first, has no explicit consent record, or has opted out.
/// The dial itself (LiveKit CreateSIPParticipant) is the voice session's.
pub async fn consent_ref_for(db: &PgPool, user: &str, number_id: &str, to_e164: &str, bot_id: &str, purpose: &str) -> PResult<Option<ConsentRef>> {
    let number = number_for_user(db, user, number_id).await?;
    if is_opted_out(db, number_id, to_e164).await? {
        return Ok(None);
    }
    let Some(basis) = consent_basis(db, number_id, to_e164).await? else { return Ok(None) };
    let id = format!("cc_{}", uuid::Uuid::new_v4().simple());
    let expires_at = Utc::now() + chrono::Duration::minutes(CALL_CONSENT_MINUTES);
    sqlx::query("INSERT INTO call_consents (id, number_id, user_id, bot_id, to_e164, purpose, basis, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(&id)
        .bind(&number.id)
        .bind(user)
        .bind(bot_id)
        .bind(to_e164)
        .bind(purpose)
        .bind(&basis)
        .bind(expires_at)
        .execute(db)
        .await?;
    Ok(Some(ConsentRef { id, basis, expires_at }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallBody {
    number_id: String,
    to: String,
    bot_id: String,
    purpose: String,
}

/// Env var holding the LiveKit outbound SIP trunk id (`ST_…`). Unset keeps
/// `/calls/outbound` consent-only (nothing dials).
pub const OUTBOUND_TRUNK_ENV: &str = "ALLTERNIT_LIVEKIT_OUTBOUND_TRUNK_ID";

fn livekit_error_response(e: LiveKitError) -> Response {
    let code = match &e {
        LiveKitError::NotConfigured => "livekit_not_configured",
        LiveKitError::Blocked(_) => "livekit_blocked",
        LiveKitError::Http(_) | LiveKitError::Server(..) => "livekit_dial_failed",
        LiveKitError::ConsentRequired => "consent_ref_required",
        LiveKitError::PublicUrlMissing => "livekit_public_url_not_configured",
    };
    tracing::warn!("outbound dial failed: {e}");
    err(StatusCode::BAD_GATEWAY, code, None)
}

/// Create the call room (dispatching `allternit-voice`) and dial the callee on the
/// outbound trunk. Only reached with a consentRef in hand.
async fn dial_outbound(livekit: &dyn LiveKitAdminClient, trunk_id: &str, from_e164: &str, user: &str, body: &CallBody, consent: &ConsentRef) -> Result<String, LiveKitError> {
    let room = format!("{CALL_ROOM_PREFIX}out-{}", uuid::Uuid::new_v4().simple());
    let attrs: HashMap<String, String> = [
        ("direction", "outbound"),
        ("consentRef", consent.id.as_str()),
        ("botId", body.bot_id.as_str()),
        ("ownerId", user),
        ("numberId", body.number_id.as_str()),
        ("to", body.to.as_str()),
        ("from", from_e164),
        ("purpose", body.purpose.as_str()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    livekit.create_room_with_agent(&room, SIP_AGENT_NAME, &json!(attrs).to_string()).await?;
    livekit
        .create_sip_participant(CreateSipParticipantRequest {
            trunk_id: trunk_id.to_string(),
            call_to: body.to.clone(),
            room_name: room.clone(),
            participant_identity: format!("sip-out-{}", uuid::Uuid::new_v4().simple()),
            participant_attributes: attrs,
            consent_ref: Some(consent.id.clone()),
            from_number: Some(from_e164.to_string()),
        })
        .await?;
    Ok(room)
}

/// `livekit` is `Some((client, trunkId))` only when LiveKit and the outbound trunk are configured.
async fn call_outbound_inner(db: &PgPool, user: &str, body: &CallBody, livekit: Option<(&dyn LiveKitAdminClient, &str)>) -> PResult<Response> {
    if !carriers::is_e164(&body.to) {
        return Err(PhoneError::BadRequest("to must be an E.164 number".into()));
    }
    if body.purpose.trim().is_empty() || body.bot_id.trim().is_empty() {
        return Err(PhoneError::BadRequest("purpose and botId are required".into()));
    }
    let Some(consent) = consent_ref_for(db, user, &body.number_id, &body.to, &body.bot_id, &body.purpose).await? else {
        return Ok(err(StatusCode::FORBIDDEN, "no_consent", None));
    };
    let mut out = json!({ "consentRef": consent.id, "basis": consent.basis, "expiresAt": consent.expires_at });
    if let Some((lk, trunk)) = livekit {
        let number = number_for_user(db, user, &body.number_id).await?;
        match dial_outbound(lk, trunk, &number.e164, user, body, &consent).await {
            Ok(room) => {
                out["room"] = json!(room);
                out["dialing"] = json!(true);
            }
            Err(e) => return Ok(livekit_error_response(e)),
        }
    }
    Ok(Json(out).into_response())
}

async fn call_outbound_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<CallBody>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let trunk = std::env::var(OUTBOUND_TRUNK_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let admin = match (trunk, LiveKitConfig::from_env()) {
            (Some(t), Some(c)) => Some((LiveKitHttpAdmin::new(c), t)),
            _ => None,
        };
        let livekit = admin.as_ref().map(|(a, t)| (a as &dyn LiveKitAdminClient, t.as_str()));
        call_outbound_inner(&state.db, &user, &body, livekit).await
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
struct ConsentBody {
    e164: String,
    source: String,
    evidence: Option<String>,
}

async fn consent_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<ConsentBody>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let number = number_for_user(&state.db, &user, &id).await?;
        if !carriers::is_e164(&body.e164) || body.source.trim().is_empty() {
            return Err(PhoneError::BadRequest("e164 and source are required".into()));
        }
        // STOP still wins: explicit consent doesn't lift an opt-out, only the sender's START does.
        let evidence = format!("recorded by {user}: {}", body.evidence.as_deref().unwrap_or(""));
        log_consent(&state.db, &number.id, &body.e164, "explicit", Some(body.source.trim()), Some(&evidence)).await?;
        let opted_out = is_opted_out(&state.db, &number.id, &body.e164).await?;
        Ok::<_, PhoneError>((StatusCode::CREATED, Json(json!({ "ok": true, "optedOut": opted_out }))).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

// ---------------------------------------------------------------------------
// SMS out
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    number_id: String,
    to: String,
    text: String,
}

/// Send one SMS from a bot's number. Refused (403) when the number's SMS isn't active, the
/// recipient opted out, or the recipient never texted first and has no consent record.
pub async fn send_sms(db: &PgPool, carrier: &dyn Carrier, user: &str, number_id: &str, to: &str, text: &str) -> PResult<Value> {
    if !carriers::is_e164(to) {
        return Err(PhoneError::BadRequest("to must be an E.164 number".into()));
    }
    if text.trim().is_empty() {
        return Err(PhoneError::BadRequest("text is empty".into()));
    }
    if text.chars().count() > MAX_SMS_CHARS {
        return Err(PhoneError::BadRequest(format!("text is over {MAX_SMS_CHARS} characters; split it before sending")));
    }
    let number = number_for_user(db, user, number_id).await?;
    if number.sms_state != "active" {
        return Err(PhoneError::Forbidden("sms_not_active"));
    }
    if is_opted_out(db, &number.id, to).await? {
        return Err(PhoneError::Forbidden("recipient_opted_out"));
    }
    if consent_basis(db, &number.id, to).await?.is_none() {
        return Err(PhoneError::Forbidden("no_consent"));
    }
    let sent_today: i64 = sqlx::query_scalar("SELECT count(*) FROM sms_outbound_log WHERE number_id = $1 AND created_at > now() - interval '24 hours'").bind(&number.id).fetch_one(db).await?;
    if sent_today >= env_i64("ALLTERNIT_SMS_DAILY_CAP", DEFAULT_DAILY_CAP) {
        return Err(PhoneError::TooMany("daily_limit"));
    }
    let sent = carrier.send_sms(&number.e164, to, text, number.messaging_ref.as_deref()).await?;
    sqlx::query("INSERT INTO sms_outbound_log (number_id, to_e164, carrier_message_id, chars) VALUES ($1, $2, $3, $4)")
        .bind(&number.id)
        .bind(to)
        .bind(&sent.id)
        .bind(text.chars().count() as i32)
        .execute(db)
        .await?;
    Ok(json!({ "ok": true, "messageId": sent.id, "parts": sent.parts }))
}

async fn sms_send_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SendBody>) -> Response {
    let run = async {
        let user = user_id(&state, &headers).await?;
        let carrier = carrier()?;
        let out = send_sms(&state.db, carrier.as_ref(), &user, &body.number_id, &body.to, &body.text).await?;
        Ok::<_, PhoneError>(Json(out).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

// ---------------------------------------------------------------------------
// SMS in: the edge
// ---------------------------------------------------------------------------

pub enum Edge {
    /// Answer the carrier now; nothing is queued.
    Respond(Response),
    /// Queue this normalised body for the runtime; if queueing fails, `forget_inbound` undoes the dedupe mark.
    Deliver { body: Vec<u8>, number_id: String, message_id: String },
}

fn ack(status: StatusCode, text: &'static str) -> Edge {
    Edge::Respond((status, text).into_response())
}

const HELP_TEXT: &str = "Allternit: this number is an AI assistant. Reply STOP to unsubscribe. Msg & data rates may apply. More at allternit.com";
const STOP_TEXT: &str = "Allternit: you're unsubscribed and will get no more messages from this number. Reply START to resubscribe.";
const START_TEXT: &str = "Allternit: you're subscribed again. Reply HELP for help, STOP to unsubscribe.";

async fn reply(db: &PgPool, carrier: &dyn Carrier, number: &NumberRow, to: &str, text: &str) {
    match carrier.send_sms(&number.e164, to, text, number.messaging_ref.as_deref()).await {
        Ok(sent) => {
            let _ = sqlx::query("INSERT INTO sms_outbound_log (number_id, to_e164, carrier_message_id, chars) VALUES ($1, $2, $3, $4)")
                .bind(&number.id)
                .bind(to)
                .bind(&sent.id)
                .bind(text.chars().count() as i32)
                .execute(db)
                .await;
        }
        Err(e) => tracing::warn!(number = %number.id, "keyword reply not sent: {e}"),
    }
}

pub async fn edge_core(db: &PgPool, carrier: &dyn Carrier, number: &NumberRow, headers: &HashMap<String, String>, url: &str, body: &[u8]) -> PResult<Edge> {
    let event = match carrier.parse_inbound(headers, url, body) {
        Ok(e) => e,
        Err(CarrierError::BadSignature) => return Ok(ack(StatusCode::UNAUTHORIZED, "bad signature")),
        Err(CarrierError::Invalid(_)) => return Ok(ack(StatusCode::BAD_REQUEST, "bad request")),
        Err(e) => return Err(e.into()),
    };
    let InboundEvent::Message { id, from, to, text } = event else {
        return Ok(ack(StatusCode::OK, "ok"));
    };
    if to != number.e164 {
        return Ok(ack(StatusCode::OK, "ok"));
    }
    let fresh = sqlx::query("INSERT INTO sms_inbound_seen (number_id, message_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(&number.id).bind(&id).execute(db).await?;
    if fresh.rows_affected() == 0 {
        return Ok(ack(StatusCode::OK, "duplicate"));
    }
    match classify_keyword(&text) {
        Some(Keyword::Stop) => {
            sqlx::query("INSERT INTO sms_opt_outs (number_id, e164) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(&number.id).bind(&from).execute(db).await?;
            log_consent(db, &number.id, &from, "opt_out", Some("sms"), Some(&text)).await?;
            reply(db, carrier, number, &from, STOP_TEXT).await;
            return Ok(ack(StatusCode::OK, "ok"));
        }
        Some(Keyword::Help) => {
            reply(db, carrier, number, &from, HELP_TEXT).await;
            return Ok(ack(StatusCode::OK, "ok"));
        }
        Some(Keyword::Start) => {
            sqlx::query("DELETE FROM sms_opt_outs WHERE number_id = $1 AND e164 = $2").bind(&number.id).bind(&from).execute(db).await?;
            log_consent(db, &number.id, &from, "opt_in", Some("sms"), Some(&text)).await?;
            reply(db, carrier, number, &from, START_TEXT).await;
            return Ok(ack(StatusCode::OK, "ok"));
        }
        None => {}
    }
    if number.sms_state == "blocked" || is_opted_out(db, &number.id, &from).await? {
        return Ok(ack(StatusCode::OK, "ok"));
    }
    // Texting first is consent for the bot to answer.
    if consent_basis(db, &number.id, &from).await?.is_none() {
        log_consent(db, &number.id, &from, "inbound_text", Some("sms"), None).await?;
    }
    let normalised = json!({
        "provider": "sms", "messageId": id, "numberId": number.id, "botId": number.bot_id,
        "from": from, "to": to, "text": text, "receivedAt": Utc::now(),
    });
    Ok(Edge::Deliver { body: serde_json::to_vec(&normalised).unwrap_or_default(), number_id: number.id.clone(), message_id: id })
}

/// Called by `/channels/in/:key` for an `sms` route, before anything is queued.
pub async fn sms_edge(state: &ApiState, route_id: &str, key: &str, headers: &HeaderMap, body: &[u8]) -> Result<Edge, PhoneError> {
    let number: Option<NumberRow> = sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE inbound_route_id = $1 AND released_at IS NULL"))
        .bind(route_id)
        .fetch_optional(&state.db)
        .await?;
    let Some(number) = number else { return Ok(ack(StatusCode::OK, "ok")) };
    let carrier = carrier()?;
    let map: HashMap<String, String> = headers.iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_ascii_lowercase(), v.to_string()))).collect();
    let url = format!("{}/channels/in/{}", super::channel_inbound::public_base(), key);
    edge_core(&state.db, carrier.as_ref(), &number, &map, &url, body).await
}

/// Undo the dedupe mark when a verified text couldn't be queued, so the carrier's retry isn't dropped.
pub async fn forget_inbound(db: &PgPool, number_id: &str, message_id: &str) {
    let _ = sqlx::query("DELETE FROM sms_inbound_seen WHERE number_id = $1 AND message_id = $2").bind(number_id).bind(message_id).execute(db).await;
}

// ---------------------------------------------------------------------------
// Carrier status webhooks
// ---------------------------------------------------------------------------

fn collect_strings(v: &Value, out: &mut Vec<String>, depth: u8) {
    if depth > 6 || out.len() > 200 {
        return;
    }
    match v {
        Value::String(s) if s.len() <= 80 => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out, depth + 1)),
        Value::Object(o) => o.values().for_each(|x| collect_strings(x, out, depth + 1)),
        _ => {}
    }
}

/// Verify the carrier's signature, then re-read (from the carrier, the authority) every
/// registration or port order the event mentions. Event names aren't relied on.
pub async fn handle_status_webhook(db: &PgPool, carrier: &dyn Carrier, headers: &HashMap<String, String>, url: &str, body: &[u8]) -> PResult<usize> {
    match carrier.parse_inbound(headers, url, body) {
        Ok(_) => {}
        Err(CarrierError::Invalid(_)) => {} // signed but not a message: fine
        Err(e) => return Err(e.into()),
    }
    let mut ids = Vec::new();
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        collect_strings(&v, &mut ids, 0);
    }
    if ids.is_empty() {
        return Ok(0);
    }
    let mut touched = 0;
    let regs: Vec<(String, String)> = sqlx::query_as("SELECT r.id, r.number_id FROM sms_registrations r WHERE r.state = 'pending' AND (r.brand_id = ANY($1) OR r.campaign_id = ANY($1) OR r.tfv_id = ANY($1))")
        .bind(&ids)
        .fetch_all(db)
        .await?;
    for (reg_id, number_id) in regs {
        let number: Option<NumberRow> = sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE id = $1 AND released_at IS NULL")).bind(&number_id).fetch_optional(db).await?;
        let reg: Option<RegRow> = sqlx::query_as(&format!("SELECT {REG_COLS} FROM sms_registrations WHERE id = $1")).bind(&reg_id).fetch_optional(db).await?;
        if let (Some(number), Some(reg)) = (number, reg) {
            if refresh_registration(db, carrier, &number, &reg).await.is_ok() {
                touched += 1;
            }
        }
    }
    let ports: Vec<NumberRow> = sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE released_at IS NULL AND port_order_id = ANY($1)")).bind(&ids).fetch_all(db).await?;
    for number in ports {
        if port_refresh(db, carrier, &number).await.is_ok() {
            touched += 1;
        }
    }
    Ok(touched)
}

async fn carrier_webhook_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(name): Path<String>, body: Bytes) -> Response {
    let run = async {
        let carrier = carrier()?;
        if carrier.name() != name {
            return Err(PhoneError::NotFound("unknown_carrier"));
        }
        if body.len() > 1024 * 1024 {
            return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
        }
        let map: HashMap<String, String> = headers.iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_ascii_lowercase(), v.to_string()))).collect();
        handle_status_webhook(&state.db, carrier.as_ref(), &map, &carriers::status_webhook_url(&name), &body).await?;
        Ok::<_, PhoneError>((StatusCode::OK, "ok").into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carriers::{BoughtNumber, PortStatus, RegistrationStatus, SentMessage};
    use crate::routes::test_support::{seed_runtime_device, test_pool, test_state, MockGateway};
    use async_trait::async_trait;
    use std::sync::Mutex;

    const USER: &str = "user_a";

    #[derive(Default)]
    struct FakeCarrier {
        sent: Mutex<Vec<(String, String, String)>>,
        bought: Mutex<Vec<String>>,
        buy_fails: bool,
        reg_state: Mutex<Option<RegState>>,
    }

    #[async_trait]
    impl Carrier for FakeCarrier {
        fn name(&self) -> &'static str {
            "telnyx"
        }
        async fn search(&self, _q: &SearchQuery) -> Result<Vec<carriers::AvailableNumber>, CarrierError> {
            Ok(vec![])
        }
        async fn buy(&self, req: &carriers::BuyRequest) -> Result<BoughtNumber, CarrierError> {
            if self.buy_fails {
                return Err(CarrierError::Upstream(422, "number unavailable".into()));
            }
            self.bought.lock().unwrap().push(req.webhook_url.clone());
            Ok(BoughtNumber { carrier_number_id: Some("cn-1".into()), messaging_ref: Some("mp-1".into()) })
        }
        async fn release(&self, _e: &str, _c: Option<&str>, _m: Option<&str>) -> Result<(), CarrierError> {
            Ok(())
        }
        async fn set_messaging_webhook(&self, _m: &str, _u: &str) -> Result<(), CarrierError> {
            Ok(())
        }
        async fn send_sms(&self, from: &str, to: &str, text: &str, _m: Option<&str>) -> Result<SentMessage, CarrierError> {
            self.sent.lock().unwrap().push((from.into(), to.into(), text.into()));
            Ok(SentMessage { id: format!("sent-{}", self.sent.lock().unwrap().len()), parts: 1 })
        }
        /// Body is `{"id","from","to","text"}`; the `x-sig` header must be `ok`.
        fn parse_inbound(&self, headers: &HashMap<String, String>, _url: &str, body: &[u8]) -> Result<InboundEvent, CarrierError> {
            if headers.get("x-sig").map(String::as_str) != Some("ok") {
                return Err(CarrierError::BadSignature);
            }
            let v: Value = serde_json::from_slice(body).map_err(|_| CarrierError::Invalid("json".into()))?;
            let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
            Ok(InboundEvent::Message { id: s("id"), from: s("from"), to: s("to"), text: s("text") })
        }
        async fn submit_registration(&self, _k: RegistrationKind, _e: &str, _c: Option<&str>, _m: Option<&str>, _f: &RegistrationForm) -> Result<RegistrationHandle, CarrierError> {
            Ok(RegistrationHandle { brand_id: Some("brand-1".into()), campaign_id: Some("camp-1".into()), tfv_id: None })
        }
        async fn registration_status(&self, _k: RegistrationKind, _e: &str, _h: &RegistrationHandle, _m: Option<&str>) -> Result<RegistrationStatus, CarrierError> {
            let state = self.reg_state.lock().unwrap().unwrap_or(RegState::Pending);
            Ok(RegistrationStatus { state, reason: (state == RegState::Rejected).then(|| "brand vetting failed".to_string()) })
        }
        async fn port_in_create(&self, _e: &[String], _r: &str, _w: &str) -> Result<PortStatus, CarrierError> {
            Ok(PortStatus { id: "po-1".into(), status: "draft".into(), done: false, failed: false, messaging_ref: Some("mp-2".into()) })
        }
        async fn port_in_status(&self, id: &str) -> Result<PortStatus, CarrierError> {
            Ok(PortStatus { id: id.into(), status: "ported".into(), done: true, failed: false, messaging_ref: None })
        }
    }

    /// A schema-per-test pool with the real 020 (relay) and 024 (phone) migrations.
    async fn pool() -> PgPool {
        let db = test_pool().await;
        for sql in [include_str!("../../migrations_pg/020_channel_inbound_queue.sql"), include_str!("../../migrations_pg/024_phone_numbers.sql")] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&db).await.unwrap();
        }
        seed_runtime_device(&db, "rt1", USER).await;
        db
    }

    fn e164(seed: u32) -> String {
        format!("+1415{:07}", seed)
    }

    async fn buy(db: &PgPool, carrier: &FakeCarrier, n: &str) -> PResult<NumberRow> {
        buy_number(db, carrier, USER, &BuyInputs { e164: n.to_string(), runtime_id: "rt1".into(), bot_id: "bot1".into(), kind: NumberType::Local, plan: "plus".into() }).await
    }

    async fn activate(db: &PgPool, id: &str) {
        sqlx::query("UPDATE phone_numbers SET sms_state = 'active' WHERE id = $1").bind(id).execute(db).await.unwrap();
    }

    fn signed() -> HashMap<String, String> {
        HashMap::from([("x-sig".to_string(), "ok".to_string())])
    }

    fn text(id: &str, from: &str, to: &str, body: &str) -> Vec<u8> {
        json!({ "id": id, "from": from, "to": to, "text": body }).to_string().into_bytes()
    }

    async fn deliver(db: &PgPool, c: &FakeCarrier, n: &NumberRow, id: &str, from: &str, body: &str) -> Edge {
        edge_core(db, c, n, &signed(), "https://x", &text(id, from, &n.e164, body)).await.unwrap()
    }

    fn status_of(edge: &Edge) -> Option<u16> {
        match edge {
            Edge::Respond(r) => Some(r.status().as_u16()),
            Edge::Deliver { .. } => None,
        }
    }

    #[test]
    fn keywords() {
        for w in ["stop", "STOP", " Stop. ", "stopall", "Unsubscribe", "cancel", "END", "quit"] {
            assert_eq!(classify_keyword(w), Some(Keyword::Stop), "{w}");
        }
        assert_eq!(classify_keyword("help"), Some(Keyword::Help));
        assert_eq!(classify_keyword("START"), Some(Keyword::Start));
        assert_eq!(classify_keyword("please stop calling"), None, "only a whole-message keyword counts");
        assert_eq!(classify_keyword("hello"), None);
    }

    #[test]
    #[serial_test::serial]
    fn plans_set_number_limits() {
        std::env::remove_var("ALLTERNIT_PHONE_MAX_PER_USER");
        assert_eq!(number_limit_for_plan("free"), None);
        assert_eq!(number_limit_for_plan("plus"), Some(3));
        assert_eq!(number_limit_for_plan("super"), Some(5));
        assert_eq!(number_limit_for_plan("ultra"), Some(10));
        assert_eq!(PhoneError::PlanRequired.into_response().status(), StatusCode::PAYMENT_REQUIRED);
    }

    #[tokio::test]
    async fn free_plan_cannot_buy_or_port() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let inputs = BuyInputs { e164: e164(80), runtime_id: "rt1".into(), bot_id: "bot1".into(), kind: NumberType::Local, plan: "free".into() };
        assert!(matches!(buy_number(&db, &c, USER, &inputs).await, Err(PhoneError::PlanRequired)));
        assert!(matches!(port_create(&db, &c, USER, &inputs).await, Err(PhoneError::PlanRequired)));
        assert!(c.bought.lock().unwrap().is_empty(), "no carrier purchase without a plan");
        let held: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_numbers").fetch_one(&db).await.unwrap();
        assert_eq!(held, 0);
        let body = axum::body::to_bytes(PhoneError::PlanRequired.into_response().into_body(), 1 << 16).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["error"], "phone_requires_plan");
    }

    #[test]
    fn toll_free_is_inferred() {
        assert_eq!(infer_type("+18885550101"), NumberType::TollFree);
        assert_eq!(infer_type("+14155550101"), NumberType::Local);
    }

    #[test]
    #[serial_test::serial]
    fn unset_carrier_env_is_not_configured() {
        std::env::remove_var("ALLTERNIT_PHONE_CARRIER");
        std::env::remove_var("ALLTERNIT_TELNYX_API_KEY");
        let resp = PhoneError::from(carriers::from_env(Arc::new(ReqwestHttp::new())).err().unwrap()).into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn buy_creates_row_and_relay_route_and_rolls_back_on_failure() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(1)).await.unwrap();
        assert_eq!((n.sms_state.as_str(), n.carrier_number_id.as_deref(), n.messaging_ref.as_deref()), ("pending_registration", Some("cn-1"), Some("mp-1")));
        let provider: String = sqlx::query_scalar("SELECT provider FROM channel_inbound_routes WHERE id = $1").bind(n.inbound_route_id.as_deref().unwrap()).fetch_one(&db).await.unwrap();
        assert_eq!(provider, "sms");
        assert!(c.bought.lock().unwrap()[0].contains("/channels/in/"), "the carrier posts to the relay address");
        assert!(matches!(buy(&db, &c, &e164(1)).await, Err(PhoneError::Conflict("number_taken"))));

        let failing = FakeCarrier { buy_fails: true, ..Default::default() };
        assert!(matches!(buy(&db, &failing, &e164(2)).await, Err(PhoneError::Carrier(_))));
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_numbers WHERE e164 = $1").bind(e164(2)).fetch_one(&db).await.unwrap();
        let live_routes: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbound_routes WHERE label = $1 AND revoked_at IS NULL").bind(e164(2)).fetch_one(&db).await.unwrap();
        assert_eq!((left, live_routes), (0, 0), "a refused purchase leaves nothing behind");
        assert!(matches!(
            buy_number(&db, &c, "someone_else", &BuyInputs { e164: e164(3), runtime_id: "rt1".into(), bot_id: "b".into(), kind: NumberType::Local, plan: "plus".into() }).await,
            Err(PhoneError::NotFound("runtime_not_found"))
        ));
        assert!(matches!(buy(&db, &c, "415").await, Err(PhoneError::BadRequest(_))));
    }

    #[tokio::test]
    async fn inbound_text_is_verified_deduped_and_normalised() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(10)).await.unwrap();
        let from = "+15550001111";
        match deliver(&db, &c, &n, "m1", from, "hello bot").await {
            Edge::Deliver { body, number_id, message_id } => {
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!((v["provider"].as_str(), v["from"].as_str(), v["text"].as_str(), v["botId"].as_str()), (Some("sms"), Some(from), Some("hello bot"), Some("bot1")));
                assert_eq!((number_id, message_id), (n.id.clone(), "m1".to_string()));
            }
            Edge::Respond(_) => panic!("a normal text must be delivered"),
        }
        assert_eq!(status_of(&deliver(&db, &c, &n, "m1", from, "hello bot").await), Some(200), "same message id is a duplicate");
        let bad = edge_core(&db, &c, &n, &HashMap::new(), "https://x", &text("m2", from, &n.e164, "hi")).await.unwrap();
        assert_eq!(status_of(&bad), Some(401));
        let elsewhere = edge_core(&db, &c, &n, &signed(), "https://x", &text("m3", from, "+14150000000", "hi")).await.unwrap();
        assert_eq!(status_of(&elsewhere), Some(200), "a text for another number is dropped");
        assert_eq!(consent_basis(&db, &n.id, from).await.unwrap().as_deref(), Some("inbound_text"));
        forget_inbound(&db, &n.id, "m1").await;
        assert!(matches!(deliver(&db, &c, &n, "m1", from, "hello bot").await, Edge::Deliver { .. }), "a forgotten id can be retried");
    }

    #[tokio::test]
    async fn stop_opts_out_confirms_and_blocks_until_start() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(20)).await.unwrap();
        let from = "+15550002222";
        assert!(matches!(deliver(&db, &c, &n, "a", from, "hi").await, Edge::Deliver { .. }));
        assert_eq!(status_of(&deliver(&db, &c, &n, "b", from, "Stop").await), Some(200));
        assert!(is_opted_out(&db, &n.id, from).await.unwrap());
        assert!(c.sent.lock().unwrap().last().unwrap().2.contains("unsubscribed"), "STOP gets a confirmation");
        let logged: String = sqlx::query_scalar("SELECT kind FROM sms_consent_log WHERE number_id = $1 AND e164 = $2 ORDER BY id DESC LIMIT 1").bind(&n.id).bind(from).fetch_one(&db).await.unwrap();
        assert_eq!(logged, "opt_out");
        assert_eq!(status_of(&deliver(&db, &c, &n, "c", from, "are you there").await), Some(200), "an opted-out sender never reaches the runtime");
        assert_eq!(status_of(&deliver(&db, &c, &n, "d", from, "HELP").await), Some(200));
        assert!(c.sent.lock().unwrap().last().unwrap().2.contains("Reply STOP"));
        assert_eq!(status_of(&deliver(&db, &c, &n, "e", from, "start").await), Some(200));
        assert!(!is_opted_out(&db, &n.id, from).await.unwrap());
        assert!(matches!(deliver(&db, &c, &n, "f", from, "back again").await, Edge::Deliver { .. }));
    }

    #[tokio::test]
    async fn send_is_gated_on_state_consent_and_opt_out() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(30)).await.unwrap();
        let to = "+15550003333";
        let send = |t: &'static str| {
            let (db, c, id) = (db.clone(), &c, n.id.clone());
            async move { send_sms(&db, c, USER, &id, to, t).await }
        };
        assert!(matches!(send("hi").await, Err(PhoneError::Forbidden("sms_not_active"))), "pending registration can't send");
        activate(&db, &n.id).await;
        assert!(matches!(send("hi").await, Err(PhoneError::Forbidden("no_consent"))), "no cold outreach");
        assert!(matches!(deliver(&db, &c, &n, "i1", to, "hello").await, Edge::Deliver { .. }));
        let ok = send("hi back").await.unwrap();
        assert_eq!((ok["ok"].as_bool(), ok["parts"].as_u64()), (Some(true), Some(1)));
        assert_eq!(c.sent.lock().unwrap().last().unwrap(), &(n.e164.clone(), to.to_string(), "hi back".to_string()));
        assert!(matches!(send_sms(&db, &c, USER, &n.id, to, &"x".repeat(1601)).await, Err(PhoneError::BadRequest(_))));
        assert!(matches!(send_sms(&db, &c, "someone_else", &n.id, to, "hi").await, Err(PhoneError::NotFound("number_not_found"))));
        deliver(&db, &c, &n, "i2", to, "STOP").await;
        assert!(matches!(send("hello?").await, Err(PhoneError::Forbidden("recipient_opted_out"))));
        std::env::set_var("ALLTERNIT_SMS_DAILY_CAP", "1");
        deliver(&db, &c, &n, "i3", to, "START").await;
        let capped = send("one more").await;
        std::env::remove_var("ALLTERNIT_SMS_DAILY_CAP");
        assert!(matches!(capped, Err(PhoneError::TooMany("daily_limit"))));
    }

    #[tokio::test]
    async fn explicit_consent_allows_a_first_text_but_not_past_stop() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(40)).await.unwrap();
        activate(&db, &n.id).await;
        let to = "+15550004444";
        log_consent(&db, &n.id, to, "explicit", Some("web_form"), Some("signed up")).await.unwrap();
        assert!(send_sms(&db, &c, USER, &n.id, to, "welcome").await.is_ok());
        deliver(&db, &c, &n, "s1", to, "stop").await;
        assert!(matches!(send_sms(&db, &c, USER, &n.id, to, "again").await, Err(PhoneError::Forbidden("recipient_opted_out"))));
    }

    #[tokio::test]
    async fn registration_submit_status_and_resubmit() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(50)).await.unwrap();
        let form = RegistrationForm { ein: Some("123456789".into()), legal_name: "Acme".into(), ..Default::default() };
        let out = submit_registration(&db, &c, USER, &n.id, None, form.clone()).await.unwrap();
        assert_eq!((out["registration"]["state"].as_str(), out["registration"]["kind"].as_str(), out["smsState"].as_str()), (Some("pending"), Some("10dlc"), Some("pending_registration")));
        let stored: Value = sqlx::query_scalar("SELECT fields FROM sms_registrations WHERE number_id = $1").bind(&n.id).fetch_one(&db).await.unwrap();
        assert_eq!(stored["ein"], "•••6789", "the EIN is stored masked");
        assert!(matches!(submit_registration(&db, &c, USER, &n.id, None, form.clone()).await, Err(PhoneError::Conflict("already_registered"))));

        let number = number_for_user(&db, USER, &n.id).await.unwrap();
        let reg = latest_registration(&db, &n.id).await.unwrap().unwrap();
        refresh_registration(&db, &c, &number, &reg).await.unwrap();
        assert_eq!(number_for_user(&db, USER, &n.id).await.unwrap().sms_state, "pending_registration");
        *c.reg_state.lock().unwrap() = Some(RegState::Rejected);
        refresh_registration(&db, &c, &number, &reg).await.unwrap();
        let rejected = number_for_user(&db, USER, &n.id).await.unwrap();
        assert_eq!(rejected.sms_state, "rejected");
        assert_eq!(latest_registration(&db, &n.id).await.unwrap().unwrap().rejection_reason.as_deref(), Some("brand vetting failed"));

        submit_registration(&db, &c, USER, &n.id, None, form).await.unwrap();
        assert_eq!(number_for_user(&db, USER, &n.id).await.unwrap().sms_state, "pending_registration", "a rejected number can resubmit");
        *c.reg_state.lock().unwrap() = Some(RegState::Approved);
        let reg = latest_registration(&db, &n.id).await.unwrap().unwrap();
        refresh_registration(&db, &c, &number, &reg).await.unwrap();
        assert_eq!(number_for_user(&db, USER, &n.id).await.unwrap().sms_state, "active");
    }

    #[tokio::test]
    async fn status_webhook_refreshes_registrations_it_mentions() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(55)).await.unwrap();
        submit_registration(&db, &c, USER, &n.id, None, RegistrationForm::default()).await.unwrap();
        *c.reg_state.lock().unwrap() = Some(RegState::Approved);
        let body = json!({"data":{"event_type":"anything","payload":{"campaignId":"camp-1"}}}).to_string();
        assert!(matches!(handle_status_webhook(&db, &c, &HashMap::new(), "u", body.as_bytes()).await, Err(PhoneError::Carrier(CarrierError::BadSignature))), "unsigned is refused");
        assert_eq!(handle_status_webhook(&db, &c, &signed(), "u", body.as_bytes()).await.unwrap(), 1);
        assert_eq!(number_for_user(&db, USER, &n.id).await.unwrap().sms_state, "active");
    }

    #[tokio::test]
    async fn call_consent_gate() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(60)).await.unwrap();
        let (a, b, d) = ("+15550005555", "+15550006666", "+15550007777");
        assert!(consent_ref_for(&db, USER, &n.id, a, "bot1", "reminder").await.unwrap().is_none(), "never contacted us: denied");
        deliver(&db, &c, &n, "k1", a, "hello").await;
        let allowed = consent_ref_for(&db, USER, &n.id, a, "bot1", "reminder").await.unwrap().unwrap();
        assert_eq!(allowed.basis, "inbound_text");
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM call_consents WHERE id = $1 AND to_e164 = $2").bind(&allowed.id).bind(a).fetch_one(&db).await.unwrap();
        assert_eq!(stored, 1);
        record_inbound_call(&db, &n.id, b).await.unwrap();
        assert_eq!(consent_ref_for(&db, USER, &n.id, b, "bot1", "callback").await.unwrap().unwrap().basis, "inbound_call");
        log_consent(&db, &n.id, d, "explicit", Some("form"), None).await.unwrap();
        assert!(consent_ref_for(&db, USER, &n.id, d, "bot1", "x").await.unwrap().is_some());
        deliver(&db, &c, &n, "k2", a, "STOP").await;
        assert!(consent_ref_for(&db, USER, &n.id, a, "bot1", "reminder").await.unwrap().is_none(), "STOP withdraws call consent too");
        assert!(matches!(consent_ref_for(&db, "someone_else", &n.id, d, "bot1", "x").await, Err(PhoneError::NotFound(_))));
    }

    #[derive(Default)]
    struct FakeLiveKit {
        calls: Mutex<Vec<String>>,
        rooms: Mutex<Vec<(String, String, Value)>>,
        dials: Mutex<Vec<CreateSipParticipantRequest>>,
        fail_dial: bool,
    }

    #[async_trait]
    impl LiveKitAdminClient for FakeLiveKit {
        async fn ensure_inbound_trunk(&self, _n: &str, _e: &str) -> Result<String, LiveKitError> {
            unimplemented!()
        }
        async fn delete_inbound_trunk(&self, _t: &str) -> Result<(), LiveKitError> {
            unimplemented!()
        }
        async fn ensure_dispatch_rule(&self, _t: &str, _n: &str, _b: &str, _o: &str, _to: &str) -> Result<String, LiveKitError> {
            unimplemented!()
        }
        async fn delete_dispatch_rule(&self, _r: &str) -> Result<(), LiveKitError> {
            unimplemented!()
        }
        async fn create_room_with_agent(&self, room: &str, agent: &str, metadata: &str) -> Result<(), LiveKitError> {
            self.calls.lock().unwrap().push("CreateRoom".into());
            self.rooms.lock().unwrap().push((room.to_string(), agent.to_string(), serde_json::from_str(metadata).unwrap()));
            Ok(())
        }
        async fn create_sip_participant(&self, r: CreateSipParticipantRequest) -> Result<Value, LiveKitError> {
            self.calls.lock().unwrap().push("CreateSIPParticipant".into());
            if self.fail_dial {
                return Err(LiveKitError::Server(500, "boom".into()));
            }
            self.dials.lock().unwrap().push(r);
            Ok(json!({}))
        }
        async fn send_data(&self, _r: &str, _t: &str, _p: &[u8]) -> Result<(), LiveKitError> {
            unimplemented!()
        }
        fn participant_access(&self, _r: &str, _i: &str, _p: bool) -> Result<super::super::livekit_admin::ParticipantAccess, LiveKitError> {
            unimplemented!()
        }
    }

    async fn body_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn call_body(number_id: &str, to: &str) -> CallBody {
        CallBody { number_id: number_id.into(), to: to.into(), bot_id: "bot1".into(), purpose: "confirm the appointment".into() }
    }

    #[tokio::test]
    async fn outbound_call_dials_with_the_consent_ref_when_the_trunk_is_set() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(80)).await.unwrap();
        let to = "+15550008888";
        let lk = FakeLiveKit::default();

        // No consent: 403, and nothing dials.
        let (status, body) = body_json(call_outbound_inner(&db, USER, &call_body(&n.id, to), Some((&lk, "ST_out"))).await.unwrap()).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("no_consent")));
        assert!(lk.calls.lock().unwrap().is_empty(), "a refused call never reaches LiveKit");

        // They texted first: the room opens with the voice agent, then the callee is dialed.
        deliver(&db, &c, &n, "k1", to, "hello").await;
        let (status, body) = body_json(call_outbound_inner(&db, USER, &call_body(&n.id, to), Some((&lk, "ST_out"))).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        let (consent, room) = (body["consentRef"].as_str().unwrap().to_string(), body["room"].as_str().unwrap().to_string());
        assert_eq!((body["dialing"].clone(), body["basis"].as_str()), (json!(true), Some("inbound_text")));
        assert!(room.starts_with("call-out-"));
        assert_eq!(*lk.calls.lock().unwrap(), vec!["CreateRoom", "CreateSIPParticipant"]);
        let rooms = lk.rooms.lock().unwrap();
        assert_eq!((rooms[0].0.as_str(), rooms[0].1.as_str()), (room.as_str(), "allternit-voice"));
        let meta = &rooms[0].2;
        assert_eq!((meta["direction"].as_str(), meta["consentRef"].as_str(), meta["botId"].as_str(), meta["ownerId"].as_str(), meta["numberId"].as_str(), meta["to"].as_str(), meta["purpose"].as_str()),
            (Some("outbound"), Some(consent.as_str()), Some("bot1"), Some(USER), Some(n.id.as_str()), Some(to), Some("confirm the appointment")));
        let dials = lk.dials.lock().unwrap();
        assert_eq!((dials[0].trunk_id.as_str(), dials[0].call_to.as_str(), dials[0].room_name.as_str()), ("ST_out", to, room.as_str()));
        assert_eq!((dials[0].consent_ref.as_deref(), dials[0].from_number.as_deref()), (Some(consent.as_str()), Some(n.e164.as_str())));
        assert_eq!(dials[0].participant_attributes.get("consentRef"), Some(&consent));
    }

    #[tokio::test]
    async fn outbound_call_is_inert_without_the_trunk_and_maps_livekit_failures_to_502() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let n = buy(&db, &c, &e164(81)).await.unwrap();
        let to = "+15550009999";
        deliver(&db, &c, &n, "k1", to, "hello").await;
        // Trunk env unset: today's consent-only answer, no room, no dialing flag.
        let (status, body) = body_json(call_outbound_inner(&db, USER, &call_body(&n.id, to), None).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["consentRef"].is_string() && body.get("room").is_none() && body.get("dialing").is_none());
        // LiveKit failing is a clear 502, not a silent success.
        let lk = FakeLiveKit { fail_dial: true, ..Default::default() };
        let (status, body) = body_json(call_outbound_inner(&db, USER, &call_body(&n.id, to), Some((&lk, "ST_out"))).await.unwrap()).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_GATEWAY, Some("livekit_dial_failed")));
        // An opted-out callee is never dialed.
        deliver(&db, &c, &n, "k2", to, "STOP").await;
        let lk = FakeLiveKit::default();
        let (status, _) = body_json(call_outbound_inner(&db, USER, &call_body(&n.id, to), Some((&lk, "ST_out"))).await.unwrap()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(lk.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn runtime_bearer_callers_use_the_same_auth_as_sms_send() {
        // The route resolves the caller with `user_id` (Clerk session or an `allternit_*` Bearer the
        // runtime stores), exactly like `/channels/sms/send` and the consent route. An anonymous
        // caller is refused before any consent row or dial.
        let state = test_state(Arc::new(MockGateway::new(Some(MockGateway::healthy_node()), vec![]))).await;
        let resp = call_outbound_route(State(state.clone()), HeaderMap::new(), Json(call_body("n1", "+15550001111"))).await;
        assert!(resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN, "got {}", resp.status());
        let resp = sms_send_route(State(state), HeaderMap::new(), Json(SendBody { number_id: "n1".into(), to: "+15550001111".into(), text: "x".into() })).await;
        assert!(resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn port_in_and_release() {
        let db = pool().await;
        let c = FakeCarrier::default();
        let inputs = BuyInputs { e164: e164(70), runtime_id: "rt1".into(), bot_id: "bot1".into(), kind: NumberType::Local, plan: "plus".into() };
        let n = port_create(&db, &c, USER, &inputs).await.unwrap();
        assert_eq!((n.port_order_id.as_deref(), n.port_state.as_deref(), n.messaging_ref.as_deref()), (Some("po-1"), Some("draft"), Some("mp-2")));
        port_refresh(&db, &c, &n).await.unwrap();
        assert_eq!(number_for_user(&db, USER, &n.id).await.unwrap().port_state.as_deref(), Some("ported"));
        release_number(&db, &c, USER, &n.id).await.unwrap();
        assert!(matches!(number_for_user(&db, USER, &n.id).await, Err(PhoneError::NotFound(_))));
        let revoked: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbound_routes WHERE id = $1 AND revoked_at IS NOT NULL").bind(n.inbound_route_id.as_deref().unwrap()).fetch_one(&db).await.unwrap();
        assert_eq!(revoked, 1);
        let again = buy(&db, &c, &e164(70)).await;
        assert!(again.is_ok(), "a released number can be bought again");
    }

    /// The whole inbound path through the public relay address with the real Telnyx adapter:
    /// signature check at the edge, then one normalised request queued for the runtime.
    #[tokio::test]
    #[serial_test::serial]
    async fn inbound_sms_over_the_relay_address_queues_a_normalised_request() {
        use axum::body::Body;
        use axum::http::Request;
        use ed25519_dalek::{Signer, SigningKey};
        use tower::ServiceExt;

        let key = SigningKey::from_bytes(&[5u8; 32]);
        std::env::set_var("ALLTERNIT_PHONE_CARRIER", "telnyx");
        std::env::set_var("ALLTERNIT_TELNYX_API_KEY", "test-key");
        std::env::set_var("ALLTERNIT_TELNYX_PUBLIC_KEY", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key.verifying_key().to_bytes()));

        let state = test_state(Arc::new(MockGateway::new(Some(MockGateway::healthy_node()), vec![]))).await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/020_channel_inbound_queue.sql").replace("public.", "")).execute(&state.db).await.unwrap();
        sqlx::raw_sql(&include_str!("../../migrations_pg/024_phone_numbers.sql").replace("public.", "")).execute(&state.db).await.unwrap();
        seed_runtime_device(&state.db, "rt1", USER).await;

        // A number with a known relay key (what `buy` would have made).
        let relay_key = "k".repeat(64);
        sqlx::query("INSERT INTO channel_inbound_routes (id, key_hash, user_id, runtime_id, provider) VALUES ('r1', $1, $2, 'rt1', 'sms')")
            .bind(super::super::channel_inbound::sha256_hex(&relay_key))
            .bind(USER)
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, inbound_route_id) VALUES ('n1', $1, 'rt1', 'bot1', '+14155559999', 'telnyx', 'r1')").bind(USER).execute(&state.db).await.unwrap();

        let body = json!({"data":{"event_type":"message.received","payload":{"id":"tm-1","direction":"inbound","from":{"phone_number":"+15551112222"},"to":[{"phone_number":"+14155559999"}],"text":"hello"}}}).to_string();
        let ts = Utc::now().timestamp().to_string();
        let sig = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key.sign(format!("{ts}|{body}").as_bytes()).to_bytes());
        let post = |signature: &str| {
            Request::builder()
                .method("POST")
                .uri(format!("/channels/in/{relay_key}"))
                .header("telnyx-signature-ed25519", signature.to_string())
                .header("telnyx-timestamp", ts.clone())
                .body(Body::from(body.clone()))
                .unwrap()
        };
        let app = super::super::channel_inbound::routes().with_state(state.clone());
        let bad = app.clone().oneshot(post("AAAA")).await.unwrap();
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED, "a bad signature is refused at the edge");
        let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbound_queue").fetch_one(&state.db).await.unwrap();
        assert_eq!(queued, 0);

        let ok = app.clone().oneshot(post(&sig)).await.unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let row: (String, String) = sqlx::query_as("SELECT body, headers::text FROM channel_inbound_queue").fetch_one(&state.db).await.unwrap();
        let queued_body: Value = serde_json::from_slice(&base64::Engine::decode(&base64::engine::general_purpose::STANDARD, row.0).unwrap()).unwrap();
        assert_eq!((queued_body["provider"].as_str(), queued_body["numberId"].as_str(), queued_body["from"].as_str(), queued_body["text"].as_str()), (Some("sms"), Some("n1"), Some("+15551112222"), Some("hello")));
        assert!(!row.1.contains("telnyx"), "carrier signature headers don't go on to the runtime");

        let again = app.oneshot(post(&sig)).await.unwrap();
        assert_eq!(again.status(), StatusCode::OK);
        let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_inbound_queue").fetch_one(&state.db).await.unwrap();
        assert_eq!(queued, 1, "a carrier retry is not queued twice");

        for k in ["ALLTERNIT_PHONE_CARRIER", "ALLTERNIT_TELNYX_API_KEY", "ALLTERNIT_TELNYX_PUBLIC_KEY"] {
            std::env::remove_var(k);
        }
    }
}
