//! `/v1/numbers`: phone numbers owned by a project's account (scope `numbers`;
//! consent also accepts `messaging`).
//!
//! A number belongs to exactly one account. In a **sandbox** project every
//! number is simulated: a fictional +1 555 number, active at once, that never
//! reaches a carrier and costs nothing; `POST /v1/numbers/{id}/simulate_inbound`
//! plays a text arriving so a developer can test webhooks end to end. In a
//! **live** project a number is bought from the carrier; US texting turns on
//! when its carrier registration is approved (`/registration`).
//!
//! Numbers live in `phone_numbers` with `project_id`/`account_id` set and the
//! project owner as `user_id`, so consent, STOP, caps and registration run
//! through the same code as the Allternit app's numbers (`routes::phone`).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::caller::{Plan, ProjectEnv};
use super::{build_page, projects, record_usage, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable, UsageEvent};
use crate::carriers::{NumberType, RegistrationForm, SearchQuery};
use crate::routes::phone::{self, PhoneError};
use crate::ApiState;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/numbers/available", &["GET"], get(available))
        .add("/v1/numbers", &["GET", "POST"], get(list_numbers).post(buy))
        .add("/v1/numbers/:id", &["GET", "PATCH", "DELETE"], get(get_number).patch(update_number).delete(release))
        .add("/v1/numbers/:id/registration", &["GET", "POST"], get(registration_get).post(registration_post))
        .add("/v1/numbers/:id/registration/otp", &["POST"], post(registration_otp))
        .add("/v1/numbers/:id/consent", &["POST"], post(consent))
        .add("/v1/numbers/:id/simulate_inbound", &["POST"], post(simulate_inbound))
}

/// Map the phone module's errors onto the Platform API envelope.
pub fn phone_error(e: PhoneError) -> PlatformError {
    match e {
        PhoneError::NotConfigured => PlatformError::api_error("phone_not_configured", "Phone service is not configured on this deployment."),
        PhoneError::BadRequest(m) => PlatformError::invalid_request("invalid_request", m),
        PhoneError::NotFound(code) => PlatformError::not_found(code, "Not found."),
        PhoneError::Forbidden(code) => PlatformError::permission(code, forbidden_text(code)),
        PhoneError::PlanRequired => PlatformError::permission("plan_required", "This needs a paid plan."),
        PhoneError::Conflict(code) => PlatformError::conflict(code, "That conflicts with the current state."),
        PhoneError::TooMany(code) => PlatformError::rate_limit(code, "Daily sending limit reached for this number."),
        PhoneError::Carrier(crate::carriers::CarrierError::Invalid(m)) => PlatformError::invalid_request("carrier_rejected", m),
        PhoneError::Carrier(e) => {
            tracing::warn!("platform carrier error: {e}");
            PlatformError::api_error("carrier_error", "The carrier refused or failed the request.")
        }
        PhoneError::Db(e) => e.into(),
        PhoneError::Auth(_) => PlatformError::authentication("unauthorized", "Not authorized."),
    }
}

fn forbidden_text(code: &str) -> String {
    match code {
        "sms_not_active" => "Texting isn't active on this number yet. US texting needs an approved carrier registration (POST /v1/numbers/{id}/registration).",
        "recipient_opted_out" => "This person replied STOP to this number.",
        "no_consent" => "This person hasn't texted this number and has no consent recorded.",
        _ => "Not allowed.",
    }
    .to_string()
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ApiNumber {
    pub id: String,
    pub account_id: Option<String>,
    pub e164: String,
    #[sqlx(rename = "type")]
    pub kind: String,
    pub sms_state: String,
    pub simulated: bool,
    /// The agent that answers calls to this number (`None`: calls aren't answered).
    pub agent_id: Option<String>,
    pub voice_state: String,
    pub created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, account_id, e164, type, sms_state, simulated, agent_id, voice_state, created_at";

impl ApiNumber {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id, "object": "phone_number", "account_id": self.account_id, "e164": self.e164,
            "type": self.kind, "sms_state": self.sms_state, "simulated": self.simulated, "agent_id": self.agent_id,
            "voice_state": if self.simulated { "active" } else { self.voice_state.as_str() }, "created_at": self.created_at,
        })
    }
}

/// A live number of this project the key may see, or 404.
pub async fn api_number(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<ApiNumber, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, ApiNumber>(&format!(
        "SELECT {COLUMNS} FROM phone_numbers WHERE id = $1 AND project_id = $2 AND released_at IS NULL AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PlatformError::not_found("number_not_found", "No such number."))
}

/// Live projects buy real numbers only once they're on a paid plan, or are
/// allow-listed in `ALLTERNIT_PLATFORM_LIVE_PROJECTS` during the beta.
fn live_allowed(caller: &PlatformCaller) -> bool {
    caller.plan != Plan::Sandbox
        || std::env::var("ALLTERNIT_PLATFORM_LIVE_PROJECTS")
            .map(|v| v.split(',').any(|p| p.trim() == caller.project_id))
            .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
struct AvailableQuery {
    country: Option<String>,
    area_code: Option<String>,
    locality: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    limit: Option<String>,
}

async fn available(caller: PlatformCaller, ApiQuery(q): ApiQuery<AvailableQuery>) -> Result<Json<Value>, PlatformError> {
    caller.require("numbers")?;
    if caller.project_env == ProjectEnv::Sandbox {
        return Err(PlatformError::invalid_request("sandbox_project", "A sandbox project gets simulated numbers: POST /v1/numbers with just an account_id."));
    }
    let kind = parse_kind(q.kind.as_deref())?.unwrap_or(NumberType::Local);
    let country = q.country.unwrap_or_else(|| "US".into()).to_ascii_uppercase();
    if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(PlatformError::invalid_request("invalid_country", "country must be a 2-letter code.").with_param("country"));
    }
    let limit = q.limit.as_deref().map(|l| l.parse::<u32>().unwrap_or(10)).unwrap_or(10).clamp(1, 50);
    let carrier = phone::carrier().map_err(phone_error)?;
    let found = carrier
        .search(&SearchQuery { country, area_code: q.area_code, locality: q.locality, kind, limit })
        .await
        .map_err(|e| phone_error(e.into()))?;
    Ok(Json(json!({ "data": found, "has_more": false, "next_cursor": null })))
}

fn parse_kind(raw: Option<&str>) -> Result<Option<NumberType>, PlatformError> {
    raw.map(|k| NumberType::parse(k).ok_or_else(|| PlatformError::invalid_request("invalid_type", "type must be local or toll_free.").with_param("type")))
        .transpose()
}

#[derive(Debug, Deserialize)]
struct BuyBody {
    account_id: String,
    e164: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    /// The agent that answers calls to the number; it must be in the same account.
    agent_id: Option<String>,
}

async fn buy(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    voice: Option<axum::Extension<Arc<super::calls::VoiceDeps>>>,
    ApiJson(body): ApiJson<BuyBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require("numbers")?;
    let account = caller.account_filter(Some(&body.account_id))?.unwrap_or(body.account_id.clone());
    projects::account_in_project(&state.db, &caller.project_id, &account).await?;
    if let Some(agent_id) = body.agent_id.as_deref() {
        // Checked before buying, so a bad agent never leaves a paid number behind.
        let agent = super::agents::fetch_visible(&state, &caller, agent_id).await.map_err(|e| e.with_param("agent_id"))?;
        if agent.account_id != account {
            return Err(PlatformError::invalid_request("account_mismatch", "The number and the agent belong to different accounts.").with_param("agent_id"));
        }
    }
    let sandbox = caller.project_env == ProjectEnv::Sandbox;
    if !sandbox && !live_allowed(&caller) {
        return Err(PlatformError::permission("plan_required", "Buying real numbers needs a paid plan with a card on file."));
    }
    let kind = parse_kind(body.kind.as_deref())?.unwrap_or_else(|| body.e164.as_deref().map(phone::infer_type).unwrap_or(NumberType::Local));
    let carrier = if sandbox { None } else { Some(phone::carrier().map_err(phone_error)?) };
    let row = phone::buy_platform_number(&state.db, carrier.as_deref(), &caller.owner_user_id, &caller.project_id, &account, body.e164.as_deref(), kind)
        .await
        .map_err(phone_error)?;
    let mut number = api_number(&state.db, &caller, &row.id).await?;
    if let Some(agent_id) = body.agent_id.as_deref() {
        // The number is bought either way: if voice setup fails it stays `voice_state: pending`
        // and PATCH /v1/numbers/{id} with the same agent_id retries it.
        if let Err(e) = super::calls::bind_agent(&state, &caller, &number, Some(agent_id), voice).await {
            tracing::warn!(number = %number.id, code = %e.code, "platform: number bought, voice setup failed");
        }
        number = api_number(&state.db, &caller, &row.id).await?;
    }
    if !number.simulated {
        let month = Utc::now().format("%Y-%m");
        let _ = record_usage(
            &state.db,
            UsageEvent {
                project_id: caller.project_id.clone(),
                account_id: number.account_id.clone(),
                key_id: Some(caller.key_id.clone()),
                meter: if number.kind == "toll_free" { "number_tollfree_month" } else { "number_local_month" }.into(),
                quantity: 1.0,
                unit: Some("month".into()),
                ref_id: Some(number.id.clone()),
                idempotency: Some(format!("number:{}:{month}", number.id)),
            },
        )
        .await;
    }
    Ok((StatusCode::CREATED, Json(number.to_json())))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    account_id: Option<String>,
}

async fn list_numbers(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Page<Value>>, PlatformError> {
    caller.require("numbers")?;
    let limit = q.page.limit()?;
    let (after_at, after_id) = match q.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let account = caller.account_filter(q.account_id.as_deref())?;
    let rows = sqlx::query_as::<_, ApiNumber>(&format!(
        "SELECT {COLUMNS} FROM phone_numbers WHERE project_id = $1 AND released_at IS NULL AND ($2::text IS NULL OR account_id = $2) \
           AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4)) ORDER BY created_at, id LIMIT $5"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let page = build_page(rows, limit, |n| (n.created_at, n.id.clone()));
    Ok(Json(Page { data: page.data.iter().map(ApiNumber::to_json).collect(), has_more: page.has_more, next_cursor: page.next_cursor }))
}

async fn get_number(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require("numbers")?;
    Ok(Json(api_number(&state.db, &caller, &id).await?.to_json()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateNumber {
    /// `null` unbinds; the agent must be in the number's account.
    #[serde(default, deserialize_with = "some_value")]
    agent_id: Option<Option<String>>,
}

fn some_value<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(de).map(Some)
}

/// `PATCH /v1/numbers/{id}` `{agent_id}`: choose the agent that answers calls to the number.
async fn update_number(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    voice: Option<axum::Extension<Arc<super::calls::VoiceDeps>>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateNumber>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("numbers")?;
    let number = api_number(&state.db, &caller, &id).await?;
    if let Some(agent_id) = body.agent_id {
        super::calls::bind_agent(&state, &caller, &number, agent_id.as_deref(), voice).await?;
    }
    Ok(Json(api_number(&state.db, &caller, &id).await?.to_json()))
}

async fn release(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    voice: Option<axum::Extension<Arc<super::calls::VoiceDeps>>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("numbers")?;
    let number = api_number(&state.db, &caller, &id).await?;
    super::calls::unprovision(&state.db, &number.id, voice).await;
    let row = phone::number_for_user(&state.db, &caller.owner_user_id, &number.id).await.map_err(phone_error)?;
    let carrier = if number.simulated { None } else { Some(phone::carrier().map_err(phone_error)?) };
    phone::release_platform_number(&state.db, carrier.as_deref(), &row).await.map_err(phone_error)?;
    Ok(Json(json!({ "id": number.id, "object": "phone_number", "deleted": true })))
}

#[derive(Debug, Deserialize)]
struct RegistrationBody {
    kind: Option<String>,
    #[serde(flatten)]
    form: RegistrationForm,
}

async fn registration_post(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<RegistrationBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    caller.require("numbers")?;
    let number = api_number(&state.db, &caller, &id).await?;
    if number.simulated {
        return Err(PlatformError::invalid_request("simulated_number", "Simulated numbers text without registration."));
    }
    let carrier = phone::carrier().map_err(phone_error)?;
    let out = phone::submit_registration(&state.db, carrier.as_ref(), &caller.owner_user_id, &number.id, body.kind.as_deref(), body.form)
        .await
        .map_err(phone_error)?;
    Ok((StatusCode::CREATED, Json(out)))
}

async fn registration_get(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require("numbers")?;
    let number = api_number(&state.db, &caller, &id).await?;
    let carrier = if number.simulated { None } else { phone::carrier().ok() };
    let out = phone::registration_status(&state.db, carrier.as_deref(), &caller.owner_user_id, &number.id).await.map_err(phone_error)?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize, Default)]
struct OtpBody {
    pin: Option<String>,
}

/// Sole proprietor registrations: `{ "pin": "123456" }` checks the code the
/// person received; an empty body sends a new code.
async fn registration_otp(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    body: Option<ApiJson<OtpBody>>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("numbers")?;
    let number = api_number(&state.db, &caller, &id).await?;
    if number.simulated {
        return Err(PlatformError::invalid_request("simulated_number", "Simulated numbers text without registration."));
    }
    let carrier = phone::carrier().map_err(phone_error)?;
    let pin = body.and_then(|ApiJson(b)| b.pin);
    let out = phone::registration_otp(&state.db, carrier.as_ref(), &caller.owner_user_id, &number.id, pin.as_deref()).await.map_err(phone_error)?;
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct ConsentBody {
    e164: String,
    source: String,
    evidence: Option<String>,
}

/// Record that a person agreed to be texted or called by this number. STOP
/// still wins: consent never lifts an opt-out; only the person's START does.
async fn consent(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ConsentBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    if !caller.has_scope("numbers") && !caller.has_scope("messaging") {
        caller.require("numbers")?;
    }
    let number = api_number(&state.db, &caller, &id).await?;
    if !crate::carriers::is_e164(&body.e164) {
        return Err(PlatformError::invalid_request("invalid_e164", "e164 must be a number like +14155550101.").with_param("e164"));
    }
    if body.source.trim().is_empty() {
        return Err(PlatformError::invalid_request("missing_source", "source is required: how the person agreed (a form, a call, an existing customer record).").with_param("source"));
    }
    let evidence = format!("recorded by API key {}: {}", caller.key_id, body.evidence.as_deref().unwrap_or(""));
    phone::log_consent(&state.db, &number.id, &body.e164, "explicit", Some(body.source.trim()), Some(&evidence)).await?;
    let opted_out = phone::is_opted_out(&state.db, &number.id, &body.e164).await?;
    Ok((StatusCode::CREATED, Json(json!({ "number_id": number.id, "e164": body.e164, "recorded": true, "opted_out": opted_out }))))
}

#[derive(Debug, Deserialize)]
struct SimulateBody {
    from: String,
    body: String,
}

/// Sandbox only: play a text arriving on a simulated number. STOP, START and HELP
/// behave as they would from a carrier; anything else is kept and sent as
/// `message.received`, and counts as the sender texting first.
async fn simulate_inbound(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SimulateBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    if !caller.has_scope("numbers") && !caller.has_scope("messaging") {
        caller.require("messaging")?;
    }
    let number = api_number(&state.db, &caller, &id).await?;
    if !number.simulated {
        return Err(PlatformError::invalid_request("not_simulated", "simulate_inbound works only on a sandbox project's simulated numbers."));
    }
    if !crate::carriers::is_e164(&body.from) {
        return Err(PlatformError::invalid_request("invalid_from", "from must be an E.164 number.").with_param("from"));
    }
    let db = &state.db;
    let handled = match phone::classify_keyword(&body.body) {
        Some(phone::Keyword::Stop) => {
            sqlx::query("INSERT INTO sms_opt_outs (number_id, e164) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(&number.id).bind(&body.from).execute(db).await?;
            phone::log_consent(db, &number.id, &body.from, "opt_out", Some("sms"), Some(&body.body)).await?;
            Some("opted_out")
        }
        Some(phone::Keyword::Start) => {
            sqlx::query("DELETE FROM sms_opt_outs WHERE number_id = $1 AND e164 = $2").bind(&number.id).bind(&body.from).execute(db).await?;
            phone::log_consent(db, &number.id, &body.from, "opt_in", Some("sms"), Some(&body.body)).await?;
            Some("opted_in")
        }
        Some(phone::Keyword::Help) => Some("help"),
        None => None,
    };
    if let Some(what) = handled {
        return Ok((StatusCode::OK, Json(json!({ "number_id": number.id, "handled": what }))));
    }
    if phone::is_opted_out(db, &number.id, &body.from).await? {
        return Ok((StatusCode::OK, Json(json!({ "number_id": number.id, "handled": "ignored_opted_out" }))));
    }
    if phone::consent_basis(db, &number.id, &body.from).await?.is_none() {
        phone::log_consent(db, &number.id, &body.from, "inbound_text", Some("sms"), None).await?;
    }
    let normalised = json!({ "provider": "sms", "messageId": format!("sim_{}", uuid::Uuid::new_v4().simple()), "numberId": number.id, "from": body.from, "to": number.e164, "text": body.body });
    super::messages::record_inbound(db, &number.id, &serde_json::to_vec(&normalised).unwrap_or_default()).await?;
    Ok((StatusCode::OK, Json(json!({ "number_id": number.id, "handled": "received" }))))
}
