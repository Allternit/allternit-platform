//! Invite links for a bot's phone (cloud side).
//!
//! An owner mints a single-use link for one person. The invitee opens
//! `https://allternit.com/i/<code>`, proves they own a phone number with a texted
//! code (that records explicit consent in `sms_consent_log`), and may also sign up
//! to reach the bot in the app. Migration `035_phone_invites.sql`.
//!
//! Owner routes (Clerk session or `compute`-scoped API key):
//! - `POST   /api/v1/invites` {botId, numberId?, label, about, botName?} → `{id, url, expiresAt}`
//! - `GET    /api/v1/invites?botId=`                                      → `{invites:[…]}`
//! - `DELETE /api/v1/invites/:id`                                         revoke
//! - `GET    /api/v1/phone/contacts?botId=`                               → `[{e164?, userId?, label, reach, consent, invite}]`
//! - `POST   /api/v1/invites/claim` {joinToken}                           links the signed-in user as an in-app contact
//!
//! Public routes (no auth; the code is the credential, rate limited per invite, per number and per IP):
//! - `GET  /i/:code/info`                                                 → `{ownerFirstName, botName, about, numberE164, channels, status}`
//! - `POST /i/:code/phone`   {e164, consentText, consent:true}            texts a 6-digit code (voice call if SMS can't go out)
//! - `POST /i/:code/verify`  {e164, code}                                 records explicit consent, source `invite`
//! - `POST /i/:code/decline`
//! - `POST /i/:code/join`                                                 → `{url, joinToken}` sign-up link
//!
//! Without carrier env the phone step answers 503 `phone_not_configured`; nothing here runs at boot.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::channel_inbound::{new_key, sha256_hex};
use super::livekit_admin::{CreateSipParticipantRequest, LiveKitAdminClient, LiveKitConfig, LiveKitHttpAdmin, CALL_ROOM_PREFIX, SIP_AGENT_NAME};
use super::phone::{carrier, is_opted_out, number_for_user, NumberRow, PhoneError, NUMBER_COLS, OUTBOUND_TRUNK_ENV};
use crate::carriers::{self, Carrier};
use crate::{ApiError, ApiState};

const INVITE_TTL_DAYS: i64 = 14;
const OTP_TTL_MINUTES: i64 = 10;
const OTP_MAX_ATTEMPTS: i32 = 5;
/// Codes sent per invite per hour (`ALLTERNIT_INVITE_OTP_PER_INVITE_HOUR` overrides).
const DEFAULT_OTP_PER_INVITE_HOUR: i64 = 5;
/// Codes sent to one phone number per hour across every invite (`ALLTERNIT_INVITE_OTP_PER_PHONE_HOUR` overrides).
const DEFAULT_OTP_PER_PHONE_HOUR: i64 = 3;
/// Live (unused, unexpired) invites one owner may hold.
const MAX_OPEN_INVITES: i64 = 50;
const MAX_LABEL: usize = 80;
const MAX_ABOUT: usize = 500;
const MAX_CONSENT_TEXT: usize = 2000;
const CONTACT_LIMIT: usize = 500;
const POSITIVE_BASES: &str = "('inbound_text', 'inbound_call', 'opt_in', 'explicit')";

fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).filter(|v| *v > 0).unwrap_or(default)
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/invites", get(list_route).post(create_route))
        .route("/api/v1/invites/claim", post(claim_route))
        .route("/api/v1/invites/:id", delete(revoke_route))
        .route("/api/v1/phone/contacts", get(contacts_route))
        .route("/i/:code/info", get(info_route))
        .route("/i/:code/phone", post(phone_route))
        .route("/i/:code/verify", post(verify_route))
        .route("/i/:code/decline", post(decline_route))
        .route("/i/:code/join", post(join_route))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum InviteError {
    /// 410: the link was used, revoked or has expired.
    Gone(&'static str),
    Phone(PhoneError),
}

impl<E: Into<PhoneError>> From<E> for InviteError {
    fn from(e: E) -> Self {
        Self::Phone(e.into())
    }
}

impl IntoResponse for InviteError {
    fn into_response(self) -> Response {
        match self {
            Self::Gone(code) => (StatusCode::GONE, Json(json!({ "error": code }))).into_response(),
            Self::Phone(e) => e.into_response(),
        }
    }
}

type IResult<T> = Result<T, InviteError>;

fn bad(msg: &str) -> InviteError {
    InviteError::Phone(PhoneError::BadRequest(msg.into()))
}

fn not_found(code: &'static str) -> InviteError {
    InviteError::Phone(PhoneError::NotFound(code))
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InviteRow {
    pub id: String,
    pub user_id: String,
    pub bot_id: String,
    pub bot_name: String,
    pub number_id: String,
    pub label: String,
    pub about: String,
    pub status: String,
    pub phone_e164: Option<String>,
    pub joined_user_id: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

const INVITE_COLS: &str = "id, user_id, bot_id, bot_name, number_id, label, about, status, phone_e164, joined_user_id, expires_at, created_at";

impl InviteRow {
    /// Status as the owner and invitee see it: a link past its date reads `expired`.
    fn shown_status(&self) -> &str {
        if matches!(self.status.as_str(), "pending" | "verified" | "joining") && self.expires_at <= Utc::now() {
            "expired"
        } else {
            &self.status
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id, "botId": self.bot_id, "numberId": self.number_id, "label": self.label, "about": self.about,
            "status": self.shown_status(), "phoneLast4": self.phone_e164.as_deref().map(last4),
            "expiresAt": self.expires_at, "createdAt": self.created_at,
        })
    }
}

fn last4(e164: &str) -> String {
    let n = e164.len();
    e164[n.saturating_sub(4)..].to_string()
}

/// Who is on the other end of a public request, kept only as hashes for the consent record.
#[derive(Debug, Clone, Default)]
pub struct ClientCtx {
    pub ip_hash: Option<String>,
    pub user_agent: Option<String>,
}

impl ClientCtx {
    fn from_headers(headers: &HeaderMap) -> Self {
        let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
        let ip = text("cf-connecting-ip").map(str::to_string).or_else(|| text("x-forwarded-for").and_then(|v| v.split(',').next()).map(|v| v.trim().to_string()));
        let salt = std::env::var("ALLTERNIT_INVITE_IP_SALT").unwrap_or_default();
        Self {
            ip_hash: ip.map(|ip| sha256_hex(&format!("{salt}{ip}"))),
            user_agent: text("user-agent").map(|v| v.chars().take(300).collect()),
        }
    }
}

// ---------------------------------------------------------------------------
// Owner: create, list, revoke
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBody {
    pub bot_id: String,
    pub number_id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub about: String,
    pub bot_name: Option<String>,
}

fn public_invite_base() -> String {
    std::env::var("ALLTERNIT_INVITE_BASE_URL").unwrap_or_else(|_| "https://allternit.com".to_string()).trim_end_matches('/').to_string()
}

fn app_base() -> String {
    std::env::var("ALLTERNIT_INVITE_APP_URL").unwrap_or_else(|_| "https://m.allternit.com".to_string()).trim_end_matches('/').to_string()
}

/// 16 characters from an unambiguous 31-letter alphabet (about 79 bits).
fn new_code() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..16).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

pub async fn create_invite(db: &PgPool, user: &str, body: &CreateBody) -> IResult<Value> {
    let label = body.label.trim();
    let about = body.about.trim();
    if body.bot_id.trim().is_empty() || label.is_empty() {
        return Err(bad("botId and label are required"));
    }
    if label.chars().count() > MAX_LABEL || about.chars().count() > MAX_ABOUT {
        return Err(bad("label or about is too long"));
    }
    let number: NumberRow = match body.number_id.as_deref().filter(|v| !v.is_empty()) {
        Some(id) => number_for_user(db, user, id).await?,
        None => sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE user_id = $1 AND bot_id = $2 AND released_at IS NULL ORDER BY created_at LIMIT 1"))
            .bind(user)
            .bind(&body.bot_id)
            .fetch_optional(db)
            .await?
            .ok_or(PhoneError::Conflict("bot_has_no_number"))?,
    };
    if number.bot_id != body.bot_id {
        return Err(bad("numberId belongs to a different bot"));
    }
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_invites WHERE user_id = $1 AND status IN ('pending', 'verified', 'joining') AND expires_at > now()").bind(user).fetch_one(db).await?;
    if open >= MAX_OPEN_INVITES {
        return Err(PhoneError::TooMany("too_many_invites").into());
    }
    let code = new_code();
    let id = format!("inv_{}", uuid::Uuid::new_v4().simple());
    let expires_at = Utc::now() + Duration::days(INVITE_TTL_DAYS);
    let bot_name = body.bot_name.as_deref().map(str::trim).filter(|v| !v.is_empty()).unwrap_or("Assistant");
    sqlx::query("INSERT INTO phone_invites (id, code_hash, user_id, bot_id, bot_name, number_id, label, about, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
        .bind(&id)
        .bind(sha256_hex(&code))
        .bind(user)
        .bind(&body.bot_id)
        .bind(bot_name.chars().take(MAX_LABEL).collect::<String>())
        .bind(&number.id)
        .bind(label)
        .bind(about)
        .bind(expires_at)
        .execute(db)
        .await?;
    Ok(json!({ "id": id, "url": format!("{}/i/{code}", public_invite_base()), "expiresAt": expires_at }))
}

pub async fn list_invites(db: &PgPool, user: &str, bot_id: Option<&str>) -> IResult<Value> {
    let rows: Vec<InviteRow> = sqlx::query_as(&format!("SELECT {INVITE_COLS} FROM phone_invites WHERE user_id = $1 AND ($2::text IS NULL OR bot_id = $2) ORDER BY created_at DESC LIMIT 200"))
        .bind(user)
        .bind(bot_id)
        .fetch_all(db)
        .await?;
    Ok(json!({ "invites": rows.iter().map(InviteRow::to_json).collect::<Vec<_>>() }))
}

/// Revoke an invite that hasn't been joined or declined yet. Idempotent.
pub async fn revoke_invite(db: &PgPool, user: &str, id: &str) -> IResult<()> {
    let row: Option<String> = sqlx::query_scalar("SELECT status FROM phone_invites WHERE id = $1 AND user_id = $2").bind(id).bind(user).fetch_optional(db).await?;
    match row.as_deref() {
        None => Err(not_found("invite_not_found")),
        Some("joined" | "declined") => Err(PhoneError::Conflict("invite_used").into()),
        Some(_) => {
            sqlx::query("UPDATE phone_invites SET status = 'revoked' WHERE id = $1 AND user_id = $2 AND status NOT IN ('joined', 'declined')").bind(id).bind(user).execute(db).await?;
            Ok(())
        }
    }
}

async fn owner(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    crate::auth::resolve_user_scoped(&state.db, headers, "compute").await.map(|u| u.id)
}

async fn create_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<CreateBody>) -> Response {
    let run = async {
        let user = owner(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<_, InviteError>((StatusCode::CREATED, Json(create_invite(&state.db, &user, &body).await?)).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BotQuery {
    bot_id: Option<String>,
}

async fn list_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(q): Query<BotQuery>) -> Response {
    let run = async {
        let user = owner(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<_, InviteError>(Json(list_invites(&state.db, &user, q.bot_id.as_deref().filter(|v| !v.is_empty())).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn revoke_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let run = async {
        let user = owner(&state, &headers).await.map_err(PhoneError::from)?;
        revoke_invite(&state.db, &user, &id).await?;
        Ok::<_, InviteError>(Json(json!({ "ok": true })).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

// ---------------------------------------------------------------------------
// Public: look up by code
// ---------------------------------------------------------------------------

/// The invite for a code, or the reason it can't be used: 404 unknown, 410 revoked / expired / used.
async fn invite_for_code(db: &PgPool, code: &str) -> IResult<InviteRow> {
    if code.len() > 64 {
        return Err(not_found("invite_not_found"));
    }
    let row: InviteRow = sqlx::query_as(&format!("SELECT {INVITE_COLS} FROM phone_invites WHERE code_hash = $1"))
        .bind(sha256_hex(code))
        .fetch_optional(db)
        .await?
        .ok_or_else(|| not_found("invite_not_found"))?;
    match row.status.as_str() {
        "revoked" => Err(InviteError::Gone("invite_revoked")),
        "joined" | "declined" => Err(InviteError::Gone("invite_used")),
        _ if row.expires_at <= Utc::now() => Err(InviteError::Gone("invite_expired")),
        _ => Ok(row),
    }
}

async fn owner_first_name(db: &PgPool, user: &str) -> String {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM users WHERE id = $1").bind(user).fetch_optional(db).await.ok().flatten().flatten();
    name.as_deref().and_then(|n| n.split_whitespace().next()).map(str::to_string).unwrap_or_else(|| "Someone".to_string())
}

pub async fn invite_info(db: &PgPool, code: &str) -> IResult<Value> {
    let inv = invite_for_code(db, code).await?;
    let number: NumberRow = sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE id = $1 AND released_at IS NULL"))
        .bind(&inv.number_id)
        .fetch_optional(db)
        .await?
        .ok_or(InviteError::Gone("invite_revoked"))?;
    let mut channels = vec!["app"];
    if !matches!(number.sms_state.as_str(), "blocked" | "rejected") {
        channels.push("sms");
    }
    if number.voice_state != "inactive" {
        channels.push("call");
    }
    Ok(json!({
        "ownerFirstName": owner_first_name(db, &inv.user_id).await, "botName": inv.bot_name, "about": inv.about,
        "numberE164": number.e164, "channels": channels, "status": inv.shown_status(),
    }))
}

// ---------------------------------------------------------------------------
// Public: phone verification
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneBody {
    pub e164: String,
    pub consent_text: String,
    pub consent: bool,
}

/// Outbound legs the code can travel on. `carrier` is `None` when carrier env is unset;
/// `voice` is `(client, outbound trunk id)` when LiveKit and the trunk are configured.
pub struct Legs<'a> {
    pub carrier: Option<&'a dyn Carrier>,
    pub voice: Option<(&'a dyn LiveKitAdminClient, &'a str)>,
}

fn otp_hash(invite_id: &str, e164: &str, otp: &str) -> String {
    sha256_hex(&format!("{invite_id}:{e164}:{otp}"))
}

async fn send_otp_sms(db: &PgPool, carrier: &dyn Carrier, number: &NumberRow, to: &str, text: &str) -> bool {
    match carrier.send_sms(&number.e164, to, text, number.messaging_ref.as_deref()).await {
        Ok(sent) => {
            let _ = sqlx::query("INSERT INTO sms_outbound_log (number_id, to_e164, carrier_message_id, chars) VALUES ($1, $2, $3, $4)")
                .bind(&number.id)
                .bind(to)
                .bind(&sent.id)
                .bind(text.chars().count() as i32)
                .execute(db)
                .await;
            true
        }
        Err(e) => {
            tracing::warn!(number = %number.id, "invite code SMS not sent, falling back to a call: {e}");
            false
        }
    }
}

/// Read the code aloud on a call from the bot's number. The voice worker reads `otp`
/// when `purpose` is `invite_code`; the dial is covered by a `call_consents` row whose
/// basis is `invite_otp` (the invitee typed this number into the link themselves).
async fn send_otp_call(db: &PgPool, lk: &dyn LiveKitAdminClient, trunk: &str, inv: &InviteRow, number: &NumberRow, to: &str, otp: &str) -> bool {
    let consent_id = format!("cc_{}", uuid::Uuid::new_v4().simple());
    let expires_at = Utc::now() + Duration::minutes(OTP_TTL_MINUTES);
    if sqlx::query("INSERT INTO call_consents (id, number_id, user_id, bot_id, to_e164, purpose, basis, expires_at) VALUES ($1, $2, $3, $4, $5, 'invite_code', 'invite_otp', $6)")
        .bind(&consent_id)
        .bind(&number.id)
        .bind(&inv.user_id)
        .bind(&inv.bot_id)
        .bind(to)
        .bind(expires_at)
        .execute(db)
        .await
        .is_err()
    {
        return false;
    }
    let room = format!("{CALL_ROOM_PREFIX}otp-{}", uuid::Uuid::new_v4().simple());
    let attrs: HashMap<String, String> = [
        ("direction", "outbound"),
        ("consentRef", consent_id.as_str()),
        ("botId", inv.bot_id.as_str()),
        ("ownerId", inv.user_id.as_str()),
        ("numberId", number.id.as_str()),
        ("to", to),
        ("from", number.e164.as_str()),
        ("purpose", "invite_code"),
        ("otp", otp),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let dial = async {
        lk.create_room_with_agent(&room, SIP_AGENT_NAME, &json!(attrs).to_string()).await?;
        lk.create_sip_participant(CreateSipParticipantRequest {
            trunk_id: trunk.to_string(),
            call_to: to.to_string(),
            room_name: room.clone(),
            participant_identity: format!("sip-otp-{}", uuid::Uuid::new_v4().simple()),
            participant_attributes: attrs.clone(),
            consent_ref: Some(consent_id.clone()),
            from_number: Some(number.e164.clone()),
        })
        .await
    };
    match dial.await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(number = %number.id, "invite code call failed: {e}");
            false
        }
    }
}

/// Text a 6-digit code from the bot's number; when the carrier refuses (10DLC still
/// pending, blocked number) read it on a call instead. Returns the channel used.
pub async fn start_phone(db: &PgPool, legs: &Legs<'_>, code: &str, body: &PhoneBody, ctx: &ClientCtx) -> IResult<Value> {
    let inv = invite_for_code(db, code).await?;
    if inv.status != "pending" {
        return Err(InviteError::Gone("invite_used"));
    }
    if !carriers::is_e164(&body.e164) {
        return Err(bad("e164 must be an E.164 number"));
    }
    let consent_text = body.consent_text.trim();
    if !body.consent || consent_text.is_empty() || consent_text.chars().count() > MAX_CONSENT_TEXT {
        return Err(bad("consent and consentText are required"));
    }
    if legs.carrier.is_none() && legs.voice.is_none() {
        return Err(PhoneError::NotConfigured.into());
    }
    let number: NumberRow = sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE id = $1 AND released_at IS NULL"))
        .bind(&inv.number_id)
        .fetch_optional(db)
        .await?
        .ok_or(InviteError::Gone("invite_revoked"))?;
    // STOP still wins: a verified code doesn't lift an opt-out, only the sender's START does.
    if is_opted_out(db, &number.id, &body.e164).await? {
        return Err(PhoneError::Forbidden("opted_out").into());
    }
    let per_invite: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_invite_otps WHERE invite_id = $1 AND created_at > now() - interval '1 hour'").bind(&inv.id).fetch_one(db).await?;
    let per_phone: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_invite_otps WHERE e164 = $1 AND created_at > now() - interval '1 hour'").bind(&body.e164).fetch_one(db).await?;
    if per_invite >= env_i64("ALLTERNIT_INVITE_OTP_PER_INVITE_HOUR", DEFAULT_OTP_PER_INVITE_HOUR) || per_phone >= env_i64("ALLTERNIT_INVITE_OTP_PER_PHONE_HOUR", DEFAULT_OTP_PER_PHONE_HOUR) {
        return Err(PhoneError::TooMany("otp_rate_limited").into());
    }

    let otp = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
    let expires_at = Utc::now() + Duration::minutes(OTP_TTL_MINUTES);
    // Only the newest code works. The row counts toward the rate limits even if delivery fails.
    sqlx::query("UPDATE phone_invite_otps SET consumed = true WHERE invite_id = $1 AND NOT consumed").bind(&inv.id).execute(db).await?;
    let otp_id: i64 = sqlx::query_scalar("INSERT INTO phone_invite_otps (invite_id, e164, code_hash, channel, consent_text, ip_hash, user_agent, expires_at) VALUES ($1, $2, $3, 'sms', $4, $5, $6, $7) RETURNING id")
        .bind(&inv.id)
        .bind(&body.e164)
        .bind(otp_hash(&inv.id, &body.e164, &otp))
        .bind(consent_text)
        .bind(&ctx.ip_hash)
        .bind(&ctx.user_agent)
        .bind(expires_at)
        .fetch_one(db)
        .await?;

    let text = format!(
        "Allternit: {otp} is your code to connect with {}'s assistant {}. Expires in {OTP_TTL_MINUTES} min. Reply STOP to opt out.",
        owner_first_name(db, &inv.user_id).await,
        inv.bot_name
    );
    let sms_ok = match legs.carrier {
        Some(c) if !matches!(number.sms_state.as_str(), "blocked" | "rejected") => send_otp_sms(db, c, &number, &body.e164, &text).await,
        _ => false,
    };
    let channel = if sms_ok {
        Some("sms")
    } else if let Some((lk, trunk)) = legs.voice {
        send_otp_call(db, lk, trunk, &inv, &number, &body.e164, &otp).await.then_some("call")
    } else {
        None
    };
    let Some(channel) = channel else {
        sqlx::query("UPDATE phone_invite_otps SET consumed = true WHERE id = $1").bind(otp_id).execute(db).await?;
        return Err(PhoneError::Carrier(carriers::CarrierError::Transport("code could not be delivered by text or call".into())).into());
    };
    if channel == "call" {
        sqlx::query("UPDATE phone_invite_otps SET channel = 'call' WHERE id = $1").bind(otp_id).execute(db).await?;
    }
    Ok(json!({ "sent": true, "channel": channel, "expiresAt": expires_at }))
}

#[derive(Deserialize)]
pub struct VerifyBody {
    pub e164: String,
    pub code: String,
}

#[derive(sqlx::FromRow)]
struct OtpRow {
    id: i64,
    code_hash: String,
    consent_text: String,
    expires_at: DateTime<Utc>,
}

/// Check the code. A match uses the invite up (single use) and writes the explicit-consent
/// record: `sms_consent_log` kind `explicit`, source `invite`, evidence JSON.
pub async fn verify_phone(db: &PgPool, code: &str, body: &VerifyBody, ctx: &ClientCtx) -> IResult<Value> {
    let inv = invite_for_code(db, code).await?;
    if inv.status != "pending" {
        return Err(InviteError::Gone("invite_used"));
    }
    if !carriers::is_e164(&body.e164) {
        return Err(bad("e164 must be an E.164 number"));
    }
    let otp: OtpRow = sqlx::query_as("SELECT id, code_hash, consent_text, expires_at FROM phone_invite_otps WHERE invite_id = $1 AND e164 = $2 AND NOT consumed ORDER BY id DESC LIMIT 1")
        .bind(&inv.id)
        .bind(&body.e164)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| bad("no code was sent to this number; request a new one"))?;
    if otp.expires_at <= Utc::now() {
        return Err(InviteError::Gone("code_expired"));
    }
    let attempts: Option<i32> = sqlx::query_scalar("UPDATE phone_invite_otps SET attempts = attempts + 1 WHERE id = $1 AND attempts < $2 AND NOT consumed RETURNING attempts").bind(otp.id).bind(OTP_MAX_ATTEMPTS).fetch_optional(db).await?;
    let Some(attempts) = attempts else {
        return Err(PhoneError::TooMany("too_many_attempts").into());
    };
    if otp.code_hash != otp_hash(&inv.id, &body.e164, body.code.trim()) {
        return Err(PhoneError::BadRequest(format!("wrong_code: {} tries left", OTP_MAX_ATTEMPTS - attempts)).into());
    }
    let now = Utc::now();
    let evidence = json!({
        "inviteId": inv.id, "ipHash": ctx.ip_hash, "userAgent": ctx.user_agent,
        "consentText": otp.consent_text, "timestamp": now,
    })
    .to_string();
    let mut tx = db.begin().await?;
    let used = sqlx::query("UPDATE phone_invites SET status = 'verified', phone_e164 = $2, used_at = now() WHERE id = $1 AND status = 'pending' AND expires_at > now()")
        .bind(&inv.id)
        .bind(&body.e164)
        .execute(&mut *tx)
        .await?;
    if used.rows_affected() == 0 {
        return Err(InviteError::Gone("invite_used"));
    }
    sqlx::query("UPDATE phone_invite_otps SET consumed = true WHERE id = $1").bind(otp.id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO sms_consent_log (number_id, e164, kind, source, evidence) VALUES ($1, $2, 'explicit', 'invite', $3)")
        .bind(&inv.number_id)
        .bind(&body.e164)
        .bind(&evidence)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let opted_out = is_opted_out(db, &inv.number_id, &body.e164).await?;
    Ok(json!({ "ok": true, "optedOut": opted_out }))
}

pub async fn decline_invite(db: &PgPool, code: &str) -> IResult<Value> {
    let inv = invite_for_code(db, code).await?;
    let done = sqlx::query("UPDATE phone_invites SET status = 'declined', used_at = now() WHERE id = $1 AND status = 'pending'").bind(&inv.id).execute(db).await?;
    if done.rows_affected() == 0 {
        return Err(InviteError::Gone("invite_used"));
    }
    Ok(json!({ "ok": true }))
}

fn percent(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Issue a sign-up link. After Clerk sign-up the app calls `POST /api/v1/invites/claim`
/// with the `joinToken` (the `jt` query parameter of the redirect) to link the account.
pub async fn join_invite(db: &PgPool, code: &str) -> IResult<Value> {
    let inv = invite_for_code(db, code).await?;
    let token = new_key();
    let done = sqlx::query("UPDATE phone_invites SET status = 'joining', join_token_hash = $2 WHERE id = $1 AND status IN ('pending', 'verified', 'joining')").bind(&inv.id).bind(sha256_hex(&token)).execute(db).await?;
    if done.rows_affected() == 0 {
        return Err(InviteError::Gone("invite_used"));
    }
    let app = app_base();
    let back = format!("{app}/i/{code}/welcome?jt={token}");
    Ok(json!({ "url": format!("{app}/sign-up?redirect_url={}", percent(&back)), "joinToken": token }))
}

/// Link the signed-in user to the owner's bot as an in-app contact.
pub async fn claim_invite(db: &PgPool, user: &str, join_token: &str) -> IResult<Value> {
    let inv: InviteRow = sqlx::query_as(&format!("SELECT {INVITE_COLS} FROM phone_invites WHERE join_token_hash = $1"))
        .bind(sha256_hex(join_token))
        .fetch_optional(db)
        .await?
        .ok_or_else(|| not_found("invite_not_found"))?;
    match inv.status.as_str() {
        "joining" if inv.expires_at > Utc::now() => {}
        "revoked" => return Err(InviteError::Gone("invite_revoked")),
        "joining" => return Err(InviteError::Gone("invite_expired")),
        _ => return Err(InviteError::Gone("invite_used")),
    }
    if inv.user_id == user {
        return Err(PhoneError::Conflict("own_invite").into());
    }
    let mut tx = db.begin().await?;
    let done = sqlx::query("UPDATE phone_invites SET status = 'joined', joined_user_id = $2, used_at = now() WHERE id = $1 AND status = 'joining'").bind(&inv.id).bind(user).execute(&mut *tx).await?;
    if done.rows_affected() == 0 {
        return Err(InviteError::Gone("invite_used"));
    }
    sqlx::query("INSERT INTO invite_contacts (invite_id, owner_user_id, bot_id, contact_user_id, label) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING")
        .bind(&inv.id)
        .bind(&inv.user_id)
        .bind(&inv.bot_id)
        .bind(user)
        .bind(&inv.label)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(json!({ "ok": true, "botId": inv.bot_id, "label": inv.label }))
}

// ---------------------------------------------------------------------------
// Public route handlers
// ---------------------------------------------------------------------------

async fn info_route(State(state): State<Arc<ApiState>>, Path(code): Path<String>) -> Response {
    invite_info(&state.db, &code).await.map(|v| Json(v).into_response()).unwrap_or_else(IntoResponse::into_response)
}

async fn phone_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(code): Path<String>, Json(body): Json<PhoneBody>) -> Response {
    let run = async {
        let carrier = carrier().ok();
        let trunk = std::env::var(OUTBOUND_TRUNK_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let admin = match (trunk, LiveKitConfig::from_env()) {
            (Some(t), Some(c)) => Some((LiveKitHttpAdmin::new(c), t)),
            _ => None,
        };
        let legs = Legs { carrier: carrier.as_deref(), voice: admin.as_ref().map(|(a, t)| (a as &dyn LiveKitAdminClient, t.as_str())) };
        Ok::<_, InviteError>(Json(start_phone(&state.db, &legs, &code, &body, &ClientCtx::from_headers(&headers)).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

async fn verify_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(code): Path<String>, Json(body): Json<VerifyBody>) -> Response {
    verify_phone(&state.db, &code, &body, &ClientCtx::from_headers(&headers)).await.map(|v| Json(v).into_response()).unwrap_or_else(IntoResponse::into_response)
}

async fn decline_route(State(state): State<Arc<ApiState>>, Path(code): Path<String>) -> Response {
    decline_invite(&state.db, &code).await.map(|v| Json(v).into_response()).unwrap_or_else(IntoResponse::into_response)
}

async fn join_route(State(state): State<Arc<ApiState>>, Path(code): Path<String>) -> Response {
    join_invite(&state.db, &code).await.map(|v| Json(v).into_response()).unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimBody {
    join_token: String,
}

async fn claim_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<ClaimBody>) -> Response {
    let run = async {
        let user = owner(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<_, InviteError>(Json(claim_invite(&state.db, &user, &body.join_token).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

// ---------------------------------------------------------------------------
// Contacts
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct ContactInviteRow {
    status: String,
    expires_at: DateTime<Utc>,
    label: String,
    number_id: String,
    phone_e164: Option<String>,
    contact_user_id: Option<String>,
}

/// Everyone the bot can reach: people who verified through an invite, people who joined the
/// app through one, invites still waiting, and anyone else with a consent record on the number.
pub async fn list_contacts(db: &PgPool, user: &str, bot_id: Option<&str>) -> IResult<Value> {
    let numbers: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, sms_state, voice_state FROM phone_numbers WHERE user_id = $1 AND released_at IS NULL AND ($2::text IS NULL OR bot_id = $2)")
            .bind(user)
            .bind(bot_id)
            .fetch_all(db)
            .await?;
    // (number id, e164) -> (latest positive basis, when); opt-outs per number.
    let mut consent: HashMap<(String, String), (String, DateTime<Utc>)> = HashMap::new();
    let mut opted_out: HashSet<(String, String)> = HashSet::new();
    for (id, _, _) in &numbers {
        let rows: Vec<(String, String, DateTime<Utc>)> = sqlx::query_as(&format!("SELECT DISTINCT ON (e164) e164, kind, created_at FROM sms_consent_log WHERE number_id = $1 AND kind IN {POSITIVE_BASES} ORDER BY e164, id DESC"))
            .bind(id)
            .fetch_all(db)
            .await?;
        for (e164, kind, at) in rows {
            consent.insert((id.clone(), e164), (kind, at));
        }
        let outs: Vec<String> = sqlx::query_scalar("SELECT e164 FROM sms_opt_outs WHERE number_id = $1").bind(id).fetch_all(db).await?;
        opted_out.extend(outs.into_iter().map(|e| (id.clone(), e)));
    }
    let state_of: HashMap<&str, (&str, &str)> = numbers.iter().map(|(id, s, v)| (id.as_str(), (s.as_str(), v.as_str()))).collect();
    let reach = |number_id: &str, e164: &str, app: bool| {
        let live = consent.contains_key(&(number_id.to_string(), e164.to_string())) && !opted_out.contains(&(number_id.to_string(), e164.to_string()));
        let (sms_state, voice_state) = state_of.get(number_id).copied().unwrap_or(("", "inactive"));
        json!({ "app": app, "sms": live && sms_state == "active", "call": live && voice_state != "inactive", "email": false })
    };
    let consent_json = |number_id: &str, e164: &str| match consent.get(&(number_id.to_string(), e164.to_string())) {
        Some((basis, at)) => json!({ "basis": basis, "at": at }),
        None => Value::Null,
    };

    let invites: Vec<ContactInviteRow> = sqlx::query_as(
        "SELECT i.status, i.expires_at, i.label, i.number_id, i.phone_e164, c.contact_user_id \
         FROM phone_invites i LEFT JOIN invite_contacts c ON c.invite_id = i.id \
         WHERE i.user_id = $1 AND i.status <> 'revoked' AND ($2::text IS NULL OR i.bot_id = $2) ORDER BY i.created_at DESC LIMIT 500",
    )
    .bind(user)
    .bind(bot_id)
    .fetch_all(db)
    .await?;
    let mut out: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for inv in &invites {
        let status = if matches!(inv.status.as_str(), "pending" | "verified" | "joining") && inv.expires_at <= Utc::now() { "expired" } else { inv.status.as_str() };
        let mut c = json!({ "label": inv.label, "invite": { "status": status } });
        match &inv.phone_e164 {
            Some(e) => {
                seen.insert(e.clone());
                c["e164"] = json!(e);
                c["reach"] = reach(&inv.number_id, e, inv.contact_user_id.is_some());
                c["consent"] = consent_json(&inv.number_id, e);
            }
            None => {
                c["reach"] = json!({ "app": inv.contact_user_id.is_some(), "sms": false, "call": false, "email": false });
                c["consent"] = Value::Null;
            }
        }
        if let Some(u) = &inv.contact_user_id {
            c["userId"] = json!(u);
            if inv.phone_e164.is_none() {
                c["consent"] = json!({ "basis": "invite", "at": Value::Null });
            }
        }
        out.push(c);
    }
    let mut rest: Vec<(&(String, String), &(String, DateTime<Utc>))> = consent.iter().filter(|((_, e), _)| !seen.contains(e)).collect();
    rest.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    for ((number_id, e164), (basis, at)) in rest {
        if !seen.insert(e164.clone()) {
            continue;
        }
        out.push(json!({
            "e164": e164, "label": e164, "reach": reach(number_id, e164, false),
            "consent": { "basis": basis, "at": at }, "invite": { "status": "none" },
        }));
    }
    out.truncate(CONTACT_LIMIT);
    Ok(Value::Array(out))
}

async fn contacts_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(q): Query<BotQuery>) -> Response {
    let run = async {
        let user = owner(&state, &headers).await.map_err(PhoneError::from)?;
        Ok::<_, InviteError>(Json(list_contacts(&state.db, &user, q.bot_id.as_deref().filter(|v| !v.is_empty())).await?).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carriers::{
        AvailableNumber, BoughtNumber, BuyRequest, CarrierError, InboundEvent, PortStatus, RegistrationForm, RegistrationHandle, RegistrationKind, RegistrationStatus, SearchQuery, SentMessage,
    };
    use crate::routes::livekit_admin::{LiveKitError, ParticipantAccess};
    use crate::routes::phone::consent_basis;
    use crate::routes::test_support::{seed_runtime_device, test_pool};
    use std::sync::Mutex;

    const USER: &str = "user_owner";
    const FRIEND: &str = "+14155550142";

    /// Records texts; `fail` makes every send fail like a carrier that blocks 10DLC-pending numbers.
    struct FakeCarrier {
        sent: Mutex<Vec<(String, String)>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl Carrier for FakeCarrier {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn search(&self, _q: &SearchQuery) -> Result<Vec<AvailableNumber>, CarrierError> {
            Ok(vec![])
        }
        async fn buy(&self, _r: &BuyRequest) -> Result<BoughtNumber, CarrierError> {
            Err(CarrierError::Unsupported("buy"))
        }
        async fn release(&self, _e: &str, _c: Option<&str>, _m: Option<&str>) -> Result<(), CarrierError> {
            Ok(())
        }
        async fn set_messaging_webhook(&self, _m: &str, _u: &str) -> Result<(), CarrierError> {
            Ok(())
        }
        async fn send_sms(&self, _from: &str, to: &str, text: &str, _m: Option<&str>) -> Result<SentMessage, CarrierError> {
            if self.fail {
                return Err(CarrierError::Upstream(403, "10DLC registration pending".into()));
            }
            self.sent.lock().unwrap().push((to.to_string(), text.to_string()));
            Ok(SentMessage { id: "m1".into(), parts: 1 })
        }
        fn parse_inbound(&self, _h: &HashMap<String, String>, _u: &str, _b: &[u8]) -> Result<InboundEvent, CarrierError> {
            Err(CarrierError::Unsupported("inbound"))
        }
        async fn submit_registration(&self, _k: RegistrationKind, _e: &str, _c: Option<&str>, _m: Option<&str>, _f: &RegistrationForm) -> Result<RegistrationHandle, CarrierError> {
            Err(CarrierError::Unsupported("registration"))
        }
        async fn registration_status(&self, _k: RegistrationKind, _e: &str, _h: &RegistrationHandle, _m: Option<&str>) -> Result<RegistrationStatus, CarrierError> {
            Err(CarrierError::Unsupported("registration"))
        }
        async fn port_in_create(&self, _e: &[String], _r: &str, _w: &str) -> Result<PortStatus, CarrierError> {
            Err(CarrierError::Unsupported("port"))
        }
        async fn port_in_status(&self, _id: &str) -> Result<PortStatus, CarrierError> {
            Err(CarrierError::Unsupported("port"))
        }
    }

    impl FakeCarrier {
        fn ok() -> Self {
            Self { sent: Mutex::new(vec![]), fail: false }
        }
        fn blocked() -> Self {
            Self { sent: Mutex::new(vec![]), fail: true }
        }
        /// The 6-digit code in the last text.
        fn last_code(&self) -> String {
            let text = self.sent.lock().unwrap().last().expect("a text was sent").1.clone();
            text.split_whitespace().nth(1).unwrap().to_string()
        }
    }

    /// Records the calls it is asked to place.
    #[derive(Default)]
    struct FakeLiveKit {
        rooms: Mutex<Vec<(String, String)>>,
        dials: Mutex<Vec<(String, Option<String>)>>,
    }

    #[async_trait::async_trait]
    impl LiveKitAdminClient for FakeLiveKit {
        async fn ensure_inbound_trunk(&self, _n: &str, _e: &str) -> Result<String, LiveKitError> {
            Ok("t".into())
        }
        async fn delete_inbound_trunk(&self, _t: &str) -> Result<(), LiveKitError> {
            Ok(())
        }
        async fn ensure_dispatch_rule(&self, _t: &str, _n: &str, _b: &str, _o: &str, _to: &str) -> Result<String, LiveKitError> {
            Ok("r".into())
        }
        async fn delete_dispatch_rule(&self, _r: &str) -> Result<(), LiveKitError> {
            Ok(())
        }
        async fn create_room_with_agent(&self, room: &str, _agent: &str, metadata: &str) -> Result<(), LiveKitError> {
            self.rooms.lock().unwrap().push((room.to_string(), metadata.to_string()));
            Ok(())
        }
        async fn create_sip_participant(&self, r: CreateSipParticipantRequest) -> Result<Value, LiveKitError> {
            self.dials.lock().unwrap().push((r.call_to, r.consent_ref));
            Ok(json!({}))
        }
        async fn send_data(&self, _r: &str, _t: &str, _p: &[u8]) -> Result<(), LiveKitError> {
            Ok(())
        }
        fn participant_access(&self, _r: &str, _i: &str, _p: bool) -> Result<ParticipantAccess, LiveKitError> {
            Err(LiveKitError::NotConfigured)
        }
    }

    async fn pool() -> PgPool {
        let db = test_pool().await;
        for sql in [
            include_str!("../../migrations_pg/020_channel_inbound_queue.sql"),
            include_str!("../../migrations_pg/024_phone_numbers.sql"),
            include_str!("../../migrations_pg/035_phone_invites.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&db).await.unwrap();
        }
        seed_runtime_device(&db, "rt1", USER).await;
        sqlx::query("INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, sms_state) VALUES ('n1', $1, 'rt1', 'bot1', '+14155550100', 'fake', 'active')")
            .bind(USER)
            .execute(&db)
            .await
            .unwrap();
        db
    }

    fn create_body(label: &str) -> CreateBody {
        CreateBody { bot_id: "bot1".into(), number_id: None, label: label.into(), about: "Mia keeps my calendar".into(), bot_name: Some("Mia".into()) }
    }

    async fn new_invite(db: &PgPool, label: &str) -> (String, String) {
        let v = create_invite(db, USER, &create_body(label)).await.unwrap();
        let url = v["url"].as_str().unwrap();
        (v["id"].as_str().unwrap().to_string(), url.rsplit('/').next().unwrap().to_string())
    }

    fn phone_body(e164: &str) -> PhoneBody {
        PhoneBody { e164: e164.into(), consent_text: "I agree to get texts from Mia, Sam's assistant. Reply STOP to opt out.".into(), consent: true }
    }

    fn ctx() -> ClientCtx {
        ClientCtx { ip_hash: Some("iphash".into()), user_agent: Some("TestAgent/1".into()) }
    }

    fn sms_legs(c: &FakeCarrier) -> Legs<'_> {
        Legs { carrier: Some(c), voice: None }
    }

    fn err_status(e: InviteError) -> u16 {
        e.into_response().status().as_u16()
    }

    #[tokio::test]
    async fn create_returns_a_fourteen_day_url_and_lists_it() {
        let db = pool().await;
        let v = create_invite(&db, USER, &create_body("Mom")).await.unwrap();
        assert!(v["url"].as_str().unwrap().starts_with("https://allternit.com/i/"));
        let days = (v["expiresAt"].as_str().unwrap().parse::<DateTime<Utc>>().unwrap() - Utc::now()).num_hours();
        assert!((13 * 24..=14 * 24).contains(&days), "expires in about 14 days, got {days}h");
        let list = list_invites(&db, USER, Some("bot1")).await.unwrap();
        assert_eq!(list["invites"][0]["label"], "Mom");
        assert_eq!(list["invites"][0]["status"], "pending");
        assert!(list["invites"][0].get("url").is_none(), "the code is shown once, at creation");
        assert_eq!(list_invites(&db, USER, Some("other")).await.unwrap()["invites"].as_array().unwrap().len(), 0);
        // The code isn't stored; only its hash.
        let stored: String = sqlx::query_scalar("SELECT code_hash FROM phone_invites").fetch_one(&db).await.unwrap();
        assert!(!v["url"].as_str().unwrap().contains(&stored));
    }

    #[tokio::test]
    async fn create_needs_a_number_on_the_bot_and_validates_input() {
        let db = pool().await;
        let mut other = create_body("Dad");
        other.bot_id = "no-number-bot".into();
        assert_eq!(err_status(create_invite(&db, USER, &other).await.unwrap_err()), 409);
        assert_eq!(err_status(create_invite(&db, USER, &create_body("  ")).await.unwrap_err()), 400);
        let mut foreign = create_body("Sis");
        foreign.number_id = Some("n1".into());
        assert_eq!(err_status(create_invite(&db, "someone_else", &foreign).await.unwrap_err()), 404);
    }

    #[tokio::test]
    async fn info_shows_what_the_page_needs_and_nothing_secret() {
        let db = pool().await;
        sqlx::query("CREATE TABLE users (id text, name text)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO users VALUES ($1, 'Sam Rivera')").bind(USER).execute(&db).await.unwrap();
        let (_, code) = new_invite(&db, "Mom").await;
        let info = invite_info(&db, &code).await.unwrap();
        assert_eq!(info["ownerFirstName"], "Sam");
        assert_eq!(info["botName"], "Mia");
        assert_eq!(info["numberE164"], "+14155550100");
        assert!(info["channels"].as_array().unwrap().contains(&json!("sms")));
        assert!(info.get("label").is_none() && info.get("userId").is_none());
        assert_eq!(err_status(invite_info(&db, "nosuchcode").await.unwrap_err()), 404);
    }

    #[tokio::test]
    async fn otp_text_then_verify_records_explicit_consent_evidence() {
        let db = pool().await;
        let (id, code) = new_invite(&db, "Mom").await;
        let c = FakeCarrier::ok();
        let sent = start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        assert_eq!(sent["channel"], "sms");
        assert_eq!(c.sent.lock().unwrap()[0].0, FRIEND);
        assert_eq!(c.last_code().len(), 6);
        // Not consent yet: the code hasn't been proven.
        assert!(consent_basis(&db, "n1", FRIEND).await.unwrap().is_none());

        let ok = verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: c.last_code() }, &ctx()).await.unwrap();
        assert_eq!(ok["ok"], true);
        let (kind, source, evidence): (String, Option<String>, Option<String>) = sqlx::query_as("SELECT kind, source, evidence FROM sms_consent_log WHERE e164 = $1").bind(FRIEND).fetch_one(&db).await.unwrap();
        assert_eq!((kind.as_str(), source.as_deref()), ("explicit", Some("invite")));
        let ev: Value = serde_json::from_str(&evidence.unwrap()).unwrap();
        assert_eq!(ev["inviteId"], id);
        assert_eq!(ev["ipHash"], "iphash");
        assert_eq!(ev["userAgent"], "TestAgent/1");
        assert!(ev["consentText"].as_str().unwrap().contains("Reply STOP"));
        assert!(ev["timestamp"].as_str().is_some());
        // The bot may now text them first.
        assert_eq!(consent_basis(&db, "n1", FRIEND).await.unwrap().as_deref(), Some("explicit"));
    }

    #[tokio::test]
    async fn an_invite_is_single_use() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let c = FakeCarrier::ok();
        start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        let v = VerifyBody { e164: FRIEND.into(), code: c.last_code() };
        verify_phone(&db, &code, &v, &ctx()).await.unwrap();
        assert_eq!(err_status(verify_phone(&db, &code, &v, &ctx()).await.unwrap_err()), 410);
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &phone_body("+14155550143"), &ctx()).await.unwrap_err()), 410);
        assert_eq!(err_status(decline_invite(&db, &code).await.unwrap_err()), 410);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sms_consent_log WHERE kind = 'explicit'").fetch_one(&db).await.unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn expired_and_revoked_links_are_gone() {
        let db = pool().await;
        let (id, code) = new_invite(&db, "Mom").await;
        sqlx::query("UPDATE phone_invites SET expires_at = now() - interval '1 minute' WHERE id = $1").bind(&id).execute(&db).await.unwrap();
        let c = FakeCarrier::ok();
        assert_eq!(err_status(invite_info(&db, &code).await.unwrap_err()), 410);
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap_err()), 410);
        assert_eq!(err_status(join_invite(&db, &code).await.unwrap_err()), 410);
        assert!(c.sent.lock().unwrap().is_empty(), "no text goes out for an expired link");
        assert_eq!(list_invites(&db, USER, None).await.unwrap()["invites"][0]["status"], "expired");

        let (id2, code2) = new_invite(&db, "Dad").await;
        assert_eq!(err_status(revoke_invite(&db, "someone_else", &id2).await.unwrap_err()), 404);
        revoke_invite(&db, USER, &id2).await.unwrap();
        revoke_invite(&db, USER, &id2).await.unwrap();
        assert_eq!(err_status(invite_info(&db, &code2).await.unwrap_err()), 410);
    }

    #[tokio::test]
    async fn wrong_codes_are_attempt_limited_and_codes_expire() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let c = FakeCarrier::ok();
        start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        let real = c.last_code();
        let wrong = if real == "000000" { "111111" } else { "000000" };
        for _ in 0..OTP_MAX_ATTEMPTS {
            assert_eq!(err_status(verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: wrong.into() }, &ctx()).await.unwrap_err()), 400);
        }
        // Even the right code is refused once the attempts are spent.
        assert_eq!(err_status(verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: real.clone() }, &ctx()).await.unwrap_err()), 429);
        assert!(consent_basis(&db, "n1", FRIEND).await.unwrap().is_none());

        sqlx::query("UPDATE phone_invite_otps SET attempts = 0, expires_at = now() - interval '1 second'").execute(&db).await.unwrap();
        assert_eq!(err_status(verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: real }, &ctx()).await.unwrap_err()), 410);
    }

    #[tokio::test]
    async fn sending_codes_is_rate_limited_per_invite_and_per_phone() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let c = FakeCarrier::ok();
        for _ in 0..DEFAULT_OTP_PER_PHONE_HOUR {
            start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        }
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap_err()), 429);
        // A different number on the same invite still counts against the invite's own budget.
        for i in 0..(DEFAULT_OTP_PER_INVITE_HOUR - DEFAULT_OTP_PER_PHONE_HOUR) {
            start_phone(&db, &sms_legs(&c), &code, &phone_body(&format!("+1415555020{i}")), &ctx()).await.unwrap();
        }
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &phone_body("+14155550299"), &ctx()).await.unwrap_err()), 429);
        assert_eq!(c.sent.lock().unwrap().len() as i64, DEFAULT_OTP_PER_INVITE_HOUR);
    }

    #[tokio::test]
    async fn only_the_newest_code_works() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let c = FakeCarrier::ok();
        start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        let first = c.last_code();
        start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        let second = c.last_code();
        if first != second {
            assert_eq!(err_status(verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: first }, &ctx()).await.unwrap_err()), 400);
        }
        verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: second }, &ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn requires_explicit_consent_and_valid_number() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let c = FakeCarrier::ok();
        let mut no = phone_body(FRIEND);
        no.consent = false;
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &no, &ctx()).await.unwrap_err()), 400);
        let mut blank = phone_body(FRIEND);
        blank.consent_text = "  ".into();
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &blank, &ctx()).await.unwrap_err()), 400);
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &phone_body("4155550142"), &ctx()).await.unwrap_err()), 400);
        assert!(c.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn opted_out_numbers_are_not_texted() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        sqlx::query("INSERT INTO sms_opt_outs (number_id, e164) VALUES ('n1', $1)").bind(FRIEND).execute(&db).await.unwrap();
        let c = FakeCarrier::ok();
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap_err()), 403);
        assert!(c.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn carrier_block_falls_back_to_a_call_that_reads_the_code() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let blocked = FakeCarrier::blocked();
        let lk = FakeLiveKit::default();
        let legs = Legs { carrier: Some(&blocked), voice: Some((&lk, "ST_out")) };
        let sent = start_phone(&db, &legs, &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        assert_eq!(sent["channel"], "call");
        let dials = lk.dials.lock().unwrap();
        assert_eq!(dials[0].0, FRIEND);
        let consent_ref = dials[0].1.clone().expect("the dial carries a consentRef");
        let meta: Value = serde_json::from_str(&lk.rooms.lock().unwrap()[0].1).unwrap();
        assert_eq!(meta["purpose"], "invite_code");
        assert_eq!(meta["to"], FRIEND);
        let otp = meta["otp"].as_str().unwrap().to_string();
        let basis: String = sqlx::query_scalar("SELECT basis FROM call_consents WHERE id = $1").bind(&consent_ref).fetch_one(&db).await.unwrap();
        assert_eq!(basis, "invite_otp");
        drop(dials);
        // The code read on the call verifies like a texted one.
        verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: otp }, &ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn no_way_to_deliver_is_503_or_502() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let none = Legs { carrier: None, voice: None };
        assert_eq!(err_status(start_phone(&db, &none, &code, &phone_body(FRIEND), &ctx()).await.unwrap_err()), 503);
        // Carrier blocks and there is no voice leg: 502, and the code can't be used.
        let blocked = FakeCarrier::blocked();
        assert_eq!(err_status(start_phone(&db, &sms_legs(&blocked), &code, &phone_body(FRIEND), &ctx()).await.unwrap_err()), 502);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM phone_invite_otps WHERE NOT consumed").fetch_one(&db).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn decline_ends_the_invite() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        decline_invite(&db, &code).await.unwrap();
        assert_eq!(err_status(invite_info(&db, &code).await.unwrap_err()), 410);
        let c = FakeCarrier::ok();
        assert_eq!(err_status(start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap_err()), 410);
        assert_eq!(list_invites(&db, USER, None).await.unwrap()["invites"][0]["status"], "declined");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sms_consent_log").fetch_one(&db).await.unwrap();
        assert_eq!(n, 0, "declining records no consent");
    }

    #[tokio::test]
    async fn join_then_claim_links_the_user_as_an_app_contact() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        let joined = join_invite(&db, &code).await.unwrap();
        let url = joined["url"].as_str().unwrap();
        assert!(url.starts_with("https://m.allternit.com/sign-up?redirect_url="));
        let token = joined["joinToken"].as_str().unwrap();
        assert!(url.contains(&percent(&format!("/i/{code}/welcome?jt={token}"))));

        assert_eq!(err_status(claim_invite(&db, USER, token).await.unwrap_err()), 409, "owners can't claim their own link");
        assert_eq!(err_status(claim_invite(&db, "new_user", "bogus").await.unwrap_err()), 404);
        claim_invite(&db, "new_user", token).await.unwrap();
        assert_eq!(err_status(claim_invite(&db, "other_user", token).await.unwrap_err()), 410);
        assert_eq!(err_status(join_invite(&db, &code).await.unwrap_err()), 410);

        let contacts = list_contacts(&db, USER, Some("bot1")).await.unwrap();
        let c = &contacts[0];
        assert_eq!(c["userId"], "new_user");
        assert_eq!(c["label"], "Mom");
        assert_eq!(c["reach"]["app"], true);
        assert_eq!(c["invite"]["status"], "joined");
    }

    #[tokio::test]
    async fn contacts_merge_phone_consent_reach_and_waiting_invites() {
        let db = pool().await;
        let (_, code) = new_invite(&db, "Mom").await;
        new_invite(&db, "Dad").await;
        let c = FakeCarrier::ok();
        start_phone(&db, &sms_legs(&c), &code, &phone_body(FRIEND), &ctx()).await.unwrap();
        verify_phone(&db, &code, &VerifyBody { e164: FRIEND.into(), code: c.last_code() }, &ctx()).await.unwrap();
        // Someone who simply texted the number first is a contact too.
        sqlx::query("INSERT INTO sms_consent_log (number_id, e164, kind, source) VALUES ('n1', '+14155550177', 'inbound_text', 'sms')").execute(&db).await.unwrap();

        let contacts = list_contacts(&db, USER, Some("bot1")).await.unwrap();
        let by_label = |l: &str| contacts.as_array().unwrap().iter().find(|c| c["label"] == l).cloned().unwrap();
        let mom = by_label("Mom");
        assert_eq!(mom["e164"], FRIEND);
        assert_eq!(mom["reach"]["sms"], true);
        assert_eq!(mom["reach"]["app"], false);
        assert_eq!(mom["consent"]["basis"], "explicit");
        assert_eq!(mom["invite"]["status"], "verified");
        let dad = by_label("Dad");
        assert_eq!(dad["invite"]["status"], "pending");
        assert_eq!(dad["reach"]["sms"], false);
        assert!(dad["consent"].is_null() && dad.get("e164").is_none());
        let walk_in = by_label("+14155550177");
        assert_eq!(walk_in["consent"]["basis"], "inbound_text");
        assert_eq!(walk_in["invite"]["status"], "none");
        assert_eq!(contacts.as_array().unwrap().len(), 3);

        // STOP removes reach but keeps the record.
        sqlx::query("INSERT INTO sms_opt_outs (number_id, e164) VALUES ('n1', $1)").bind(FRIEND).execute(&db).await.unwrap();
        let after = list_contacts(&db, USER, None).await.unwrap();
        let mom = after.as_array().unwrap().iter().find(|c| c["label"] == "Mom").unwrap();
        assert_eq!(mom["reach"]["sms"], false);
        // Another owner sees none of it.
        assert_eq!(list_contacts(&db, "someone_else", None).await.unwrap().as_array().unwrap().len(), 0);
    }
}
