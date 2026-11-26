//! Review & sign: the `sign_requests` audit record (migration 167).
//!
//! Plan `handoffs/review-sign-2026-11-26/PLAN.md`, section "Contract". JSON is
//! snake_case. Every route is scoped to the caller; another user's id is 404.
//!
//! * `POST /api/v1/sign-requests` -> 201 SignRequest. Any account credential
//!   (session or API key) may ask; an API key may not claim `requester_kind:
//!   self`. The document must be one of the caller's `user_files` rows.
//! * `GET  /api/v1/sign-requests?status=&thread_id=&limit=` -> `{items}`.
//! * `GET  /api/v1/sign-requests/:id` -> SignRequest.
//! * `GET  /api/v1/sign-requests/:id/signature` -> image/png. Interactive
//!   session only (an agent never gets the signature image).
//! * `POST /api/v1/sign-requests/:id/sign` -> SignRequest. Interactive session
//!   only, see [`interactive_signer`]. Stores signer name + email from the
//!   account, time, IP and user agent.
//! * `POST /api/v1/sign-requests/:id/decline` `{reason?}` -> SignRequest.
//!   Interactive session only (declining is the signer's answer).
//! * `POST /api/v1/sign-requests/:id/cancel` -> SignRequest. Any credential
//!   (the asker may withdraw).
//! * `POST /api/v1/sign-requests/:id/sent` `{to}` -> SignRequest, only when
//!   signed.
//!
//! State machine ([`transition`]): `pending -> signed | declined | cancelled`,
//! `pending -> expired` lazily (a pending row past `expires_at` is written as
//! expired on the owner's next read). End states never change: an action on
//! one is 409 `not-pending {status}`, except sign/decline/cancel on an expired
//! request, which is 410 `expired`. `sent` on a request that is not signed is
//! 409 `not-signed {status}`.
//!
//! Refusals are `{code, message}` (+ `status` for 409/410) with the HTTP
//! status, like `routes::files`.

use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing, Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{net::SocketAddr, sync::Arc};
use uuid::Uuid;

use crate::{
    auth,
    db::cowork_models::ApprovalPriority,
    error::ApiError,
    services::{r2::R2Client, user_files, user_trust},
    ApiState,
};

/// The consent line the sheet shows above Sign & send (plan decision 7).
pub const CONSENT_VERSION: &str = "v1";
pub const CONSENT_TEXT_V1: &str = "By tapping Sign & send, you agree to sign this document electronically. This adds an image of your signature and a signing record. It is not a certificate-based digital signature.";
pub const MAX_SIGNATURE_PNG_BYTES: usize = 256 * 1024;
pub const MAX_DETAILS: usize = 8;
pub const MAX_SLOTS: usize = 32;
pub const DEFAULT_EXPIRY_DAYS: i64 = 14;
pub const MAX_EXPIRY_DAYS: i64 = 90;
pub const DEFAULT_LIST_LIMIT: i64 = 50;
pub const MAX_LIST_LIMIT: i64 = 100;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/sign-requests", routing::get(list).post(create))
        .route("/api/v1/sign-requests/:id", routing::get(get))
        .route("/api/v1/sign-requests/:id/signature", routing::get(signature))
        .route("/api/v1/sign-requests/:id/sign", routing::post(sign))
        .route("/api/v1/sign-requests/:id/decline", routing::post(decline))
        .route("/api/v1/sign-requests/:id/cancel", routing::post(cancel))
        .route("/api/v1/sign-requests/:id/sent", routing::post(sent))
}

// ============================================================================
// State machine
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Pending,
    Signed,
    Declined,
    Cancelled,
    Expired,
}

impl Status {
    pub(crate) const ALL: [Status; 5] = [
        Status::Pending,
        Status::Signed,
        Status::Declined,
        Status::Cancelled,
        Status::Expired,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Signed => "signed",
            Status::Declined => "declined",
            Status::Cancelled => "cancelled",
            Status::Expired => "expired",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        Status::ALL.into_iter().find(|s| s.as_str() == raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Sign,
    Decline,
    Cancel,
    Sent,
}

#[cfg(test)]
impl Action {
    pub(crate) const ALL: [Action; 4] = [Action::Sign, Action::Decline, Action::Cancel, Action::Sent];
}

/// A refusal the route answers with `{code, message}` (+ `status`).
#[derive(Debug, PartialEq)]
pub(crate) struct Refusal {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// The request's status, for 409 / 410.
    pub current: Option<Status>,
}

pub(crate) fn refuse(status: StatusCode, code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { status, code, message: message.into(), current: None }
}

fn bad_request(message: impl Into<String>) -> Refusal {
    refuse(StatusCode::BAD_REQUEST, "bad-request", message)
}

fn bad_slot() -> Refusal {
    refuse(StatusCode::BAD_REQUEST, "bad-slot", "The signature slot is not valid for this document.")
}

fn bad_signature(message: impl Into<String>) -> Refusal {
    refuse(StatusCode::BAD_REQUEST, "bad-signature", message)
}

fn not_found() -> Refusal {
    refuse(StatusCode::NOT_FOUND, "not-found", "No such sign request.")
}

fn file_not_found() -> Refusal {
    refuse(StatusCode::NOT_FOUND, "file-not-found", "That file is not in your files.")
}

pub(crate) fn session_required() -> Refusal {
    refuse(
        StatusCode::FORBIDDEN,
        "user-session-required",
        "Only you can do this, from a signed-in Allternit session. Agents, runtimes and API keys cannot sign.",
    )
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let mut body = json!({ "code": self.code, "message": self.message });
        if let Some(current) = self.current {
            body["status"] = json!(current.as_str());
        }
        (self.status, Json(body)).into_response()
    }
}

/// Handler error: a coded refusal, or an `ApiError` (auth, database).
#[derive(Debug)]
pub(crate) enum Fail {
    Refused(Refusal),
    Api(ApiError),
}

impl From<Refusal> for Fail {
    fn from(r: Refusal) -> Self {
        Fail::Refused(r)
    }
}
impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Self {
        Fail::Api(e)
    }
}
impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        Fail::Api(ApiError::DatabaseError(e))
    }
}
impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        match self {
            Fail::Refused(r) => r.into_response(),
            Fail::Api(e) => e.into_response(),
        }
    }
}

type Reply = Result<Response, Fail>;

/// The status as read: a pending request at or past `expires_at` is expired.
pub(crate) fn effective_status(stored: Status, expires_at: DateTime<Utc>, now: DateTime<Utc>) -> Status {
    if stored == Status::Pending && expires_at <= now {
        Status::Expired
    } else {
        stored
    }
}

/// The one state machine every write goes through. `Ok` is the status after
/// the action (`sent` keeps `signed`).
pub(crate) fn transition(
    stored: Status,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
    action: Action,
) -> Result<Status, Refusal> {
    let current = effective_status(stored, expires_at, now);
    match (current, action) {
        (Status::Pending, Action::Sign) => Ok(Status::Signed),
        (Status::Pending, Action::Decline) => Ok(Status::Declined),
        (Status::Pending, Action::Cancel) => Ok(Status::Cancelled),
        (Status::Signed, Action::Sent) => Ok(Status::Signed),
        (Status::Expired, Action::Sign | Action::Decline | Action::Cancel) => Err(Refusal {
            current: Some(Status::Expired),
            ..refuse(StatusCode::GONE, "expired", "This request expired. Ask for a new one.")
        }),
        (Status::Pending, Action::Sent) => Err(Refusal {
            current: Some(Status::Pending),
            ..refuse(StatusCode::CONFLICT, "not-signed", "Only a signed request can be marked as sent.")
        }),
        (other, _) => Err(Refusal {
            current: Some(other),
            ..refuse(StatusCode::CONFLICT, "not-pending", format!("This request is already {}.", other.as_str()))
        }),
    }
}

// ============================================================================
// Contract types + validation
// ============================================================================

/// A place to sign: PDF points, origin bottom-left, `page` 0-based.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SignSlot {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Detail {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SendTo {
    #[serde(default)]
    pub name: Option<String>,
    pub email: String,
}

/// Lowercase 64-hex SHA-256, or `None`.
pub(crate) fn normalize_sha256(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase();
    (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

pub(crate) fn check_slot(s: &SignSlot) -> Result<(), Refusal> {
    let finite = [s.x, s.y, s.w, s.h].iter().all(|n| n.is_finite());
    let ok = finite
        && !s.id.trim().is_empty()
        && s.id.len() <= 100
        && s.label.len() <= 200
        && s.page <= 10_000
        && s.x >= 0.0
        && s.y >= 0.0
        && s.w > 0.0
        && s.h > 0.0
        && s.w <= 5_000.0
        && s.h <= 5_000.0;
    if ok {
        Ok(())
    } else {
        Err(bad_slot())
    }
}

pub(crate) fn check_slots(slots: &[SignSlot]) -> Result<(), Refusal> {
    if slots.len() > MAX_SLOTS {
        return Err(bad_slot());
    }
    for (i, slot) in slots.iter().enumerate() {
        check_slot(slot)?;
        if slots[..i].iter().any(|other| other.id == slot.id) {
            return Err(bad_slot());
        }
    }
    Ok(())
}

/// With no slots on the request the signer picks the spot; otherwise the
/// chosen slot must be one of them (same id and page).
pub(crate) fn slot_allowed(slots: &[SignSlot], chosen: &SignSlot) -> bool {
    slots.is_empty() || slots.iter().any(|s| s.id == chosen.id && s.page == chosen.page)
}

fn check_email(raw: &str) -> Result<(), Refusal> {
    let email = raw.trim();
    let ok = email.len() <= 320
        && !email.chars().any(char::is_whitespace)
        && email.split_once('@').map_or(false, |(user, domain)| !user.is_empty() && domain.contains('.'));
    if ok {
        Ok(())
    } else {
        Err(bad_request("That email address is not valid."))
    }
}

fn check_len(value: Option<&str>, max: usize, field: &str) -> Result<(), Refusal> {
    match value {
        Some(v) if v.len() > max => Err(bad_request(format!("{field} is too long ({max} characters max)."))),
        _ => Ok(()),
    }
}

fn trimmed(value: &Option<String>) -> Option<String> {
    value.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateBody {
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub details: Vec<Detail>,
    pub document_file_id: Uuid,
    pub document_sha256: String,
    #[serde(default)]
    pub slots: Vec<SignSlot>,
    #[serde(default)]
    pub send_to: Option<SendTo>,
    pub requester_kind: String,
    #[serde(default)]
    pub requester_name: Option<String>,
    pub source_surface: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub expires_in_days: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct CheckedCreate {
    pub document_sha256: String,
    pub requester_kind: &'static str,
    pub expires_in_days: i64,
}

/// Validates a create body. `from_api_key`: the caller used an API key, not a
/// session, so it cannot record the request as the person's own (`self`).
pub(crate) fn check_create(b: &CreateBody, from_api_key: bool) -> Result<CheckedCreate, Refusal> {
    let title = b.title.trim();
    if title.is_empty() || title.len() > 200 {
        return Err(bad_request("title is required (200 characters max)."));
    }
    check_len(b.summary.as_deref(), 2_000, "summary")?;
    if b.details.len() > MAX_DETAILS {
        return Err(bad_request(format!("details has {MAX_DETAILS} rows at most.")));
    }
    for d in &b.details {
        if d.label.trim().is_empty() || d.label.len() > 80 || d.value.len() > 500 {
            return Err(bad_request("Each detail needs a label (80 characters max) and a value (500 max)."));
        }
    }
    let document_sha256 = normalize_sha256(&b.document_sha256)
        .ok_or_else(|| bad_request("document_sha256 must be 64 hex characters."))?;
    check_slots(&b.slots)?;
    if let Some(to) = &b.send_to {
        check_email(&to.email)?;
        check_len(to.name.as_deref(), 200, "send_to.name")?;
    }
    let requester_kind = match b.requester_kind.as_str() {
        "self" => "self",
        "agent" => "agent",
        "mcp_app" => "mcp_app",
        _ => return Err(bad_request("requester_kind must be self, agent or mcp_app.")),
    };
    if from_api_key && requester_kind == "self" {
        return Err(bad_request("An API key cannot create a self request; use requester_kind agent."));
    }
    let surface = b.source_surface.trim();
    if surface.is_empty() || surface.len() > 40 {
        return Err(bad_request("source_surface is required (40 characters max)."));
    }
    check_len(b.requester_name.as_deref(), 200, "requester_name")?;
    check_len(b.thread_id.as_deref(), 200, "thread_id")?;
    let expires_in_days = b.expires_in_days.unwrap_or(DEFAULT_EXPIRY_DAYS);
    if !(1..=MAX_EXPIRY_DAYS).contains(&expires_in_days) {
        return Err(bad_request(format!("expires_in_days must be 1 to {MAX_EXPIRY_DAYS}.")));
    }
    Ok(CheckedCreate { document_sha256, requester_kind, expires_in_days })
}

#[derive(Debug, Deserialize)]
pub(crate) struct SignBody {
    #[serde(default)]
    pub consent: bool,
    #[serde(default)]
    pub consent_version: String,
    pub method: String,
    pub signature_png_base64: String,
    pub slot: SignSlot,
    pub document_sha256: String,
    pub signed_file_id: Uuid,
    pub signed_sha256: String,
    #[serde(default)]
    pub surface: String,
}

#[derive(Debug)]
pub(crate) struct CheckedSign {
    pub method: &'static str,
    pub png: Vec<u8>,
    pub document_sha256: String,
    pub signed_sha256: String,
}

/// A PNG of at most 256 KB, as base64 (a `data:image/png;base64,` prefix is
/// allowed).
pub(crate) fn decode_signature_png(raw: &str) -> Result<Vec<u8>, Refusal> {
    let b64 = raw.trim();
    let b64 = b64.strip_prefix("data:image/png;base64,").unwrap_or(b64);
    if b64.len() > MAX_SIGNATURE_PNG_BYTES / 3 * 4 + 8 {
        return Err(bad_signature("The signature image is over 256 KB."));
    }
    let bytes = STANDARD
        .decode(b64)
        .map_err(|_| bad_signature("The signature image is not valid base64."))?;
    if bytes.len() > MAX_SIGNATURE_PNG_BYTES {
        return Err(bad_signature("The signature image is over 256 KB."));
    }
    if !bytes.starts_with(PNG_MAGIC) {
        return Err(bad_signature("The signature image must be a PNG."));
    }
    Ok(bytes)
}

/// Everything about a sign body that does not need the stored request.
pub(crate) fn check_sign(b: &SignBody) -> Result<CheckedSign, Refusal> {
    if !b.consent || b.consent_version != CONSENT_VERSION {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "consent-required",
            "Agree to sign electronically (consent true, consent_version v1).",
        ));
    }
    let method = match b.method.as_str() {
        "draw" => "draw",
        "type" => "type",
        "upload" => "upload",
        _ => return Err(bad_signature("method must be draw, type or upload.")),
    };
    let png = decode_signature_png(&b.signature_png_base64)?;
    check_slot(&b.slot)?;
    let document_sha256 = normalize_sha256(&b.document_sha256)
        .ok_or_else(|| bad_request("document_sha256 must be 64 hex characters."))?;
    let signed_sha256 = normalize_sha256(&b.signed_sha256)
        .ok_or_else(|| bad_request("signed_sha256 must be 64 hex characters."))?;
    check_len(Some(b.surface.as_str()), 40, "surface")?;
    Ok(CheckedSign { method, png, document_sha256, signed_sha256 })
}

/// What `/sign` decides from the stored request, in contract order: the
/// state machine, then the document hash, then the slot.
pub(crate) fn check_sign_against(
    stored: Status,
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
    stored_sha256: &str,
    slots: &[SignSlot],
    body_sha256: &str,
    slot: &SignSlot,
) -> Result<(), Refusal> {
    transition(stored, expires_at, now, Action::Sign)?;
    if stored_sha256 != body_sha256 {
        return Err(refuse(
            StatusCode::CONFLICT,
            "document-changed",
            "The document changed after this request was made. Ask for a new request.",
        ));
    }
    if !slot_allowed(slots, slot) {
        return Err(bad_slot());
    }
    Ok(())
}

// ============================================================================
// Who may sign
// ============================================================================

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Could this bearer be a Clerk JWT at all? API tokens (`allternit_…`),
/// runtime device tokens (`allternit_runtime_…`), platform keys and OAuth
/// secrets (`alt_…`) never are (same shape test as `auth::middleware`).
pub(crate) fn may_be_session_token(token: &str) -> bool {
    token.split('.').count() == 3 && !token.starts_with("alt_") && !token.starts_with("allternit_")
}

/// Verified Clerk claims that came from a person's interactive session.
///
/// `auth::require_session_user` is not enough here: Clerk-issued OAuth
/// access tokens (what an outside MCP client such as a Claude connector
/// holds, see `routes::mcp_edge` `verify`) pass the same signature + issuer
/// check as a session token. A Clerk session token always carries `sid` (the
/// session id) and never `scope` / `client_id`; an OAuth access token carries
/// the client and scopes and no session.
pub(crate) fn is_interactive_session(claims: &Value) -> bool {
    let has_session = claims
        .get("sid")
        .and_then(Value::as_str)
        .map_or(false, |sid| !sid.trim().is_empty());
    let oauth_client = ["scope", "scp", "client_id", "cid"]
        .iter()
        .any(|key| claims.get(*key).map_or(false, |v| !v.is_null()));
    has_session && !oauth_client
}

/// The signed-in person, from their interactive Clerk session.
#[derive(Debug, Clone)]
pub(crate) struct Signer {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// The caller as a signer: a person's interactive Clerk session, never an
/// agent, runtime, OAuth client or API key (403 `user-session-required`). No
/// or an invalid credential stays 401.
pub(crate) async fn interactive_signer(headers: &HeaderMap) -> Result<Signer, Fail> {
    let Some(token) = bearer(headers) else {
        return Err(ApiError::Unauthorized("Authorization header with Bearer token required".to_string()).into());
    };
    if !may_be_session_token(token) {
        return Err(session_required().into());
    }
    let claims = auth::clerk::verified_claims(token).await?;
    if !is_interactive_session(&claims) {
        return Err(session_required().into());
    }
    let claim = |key: &str| {
        claims
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let id = claim("sub").ok_or_else(|| ApiError::Unauthorized("Clerk token has no subject".to_string()))?;
    Ok(Signer {
        id,
        name: claim("name"),
        email: claim("email").or_else(|| claim("email_address")),
    })
}

// ============================================================================
// Store
// ============================================================================

const COLS: &str = "id, user_id, status, requester_kind, requester_name, source_surface, thread_id, \
    title, summary, details, document_file_id, document_name, document_sha256, slots, send_to, \
    created_at, expires_at, resolved_at, signature_method, signature_slot, signed_file_id, \
    signed_sha256, consent_version, signer_name, signer_email, signed_surface, sent_at, sent_to";

/// A `sign_requests` row without the signing record's private columns
/// (`signature_png`, `signer_ip`, `signer_user_agent`, `consent_text`,
/// `decline_reason`).
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct Row {
    pub id: Uuid,
    pub user_id: String,
    pub status: String,
    pub requester_kind: String,
    pub requester_name: Option<String>,
    pub source_surface: String,
    pub thread_id: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub details: Value,
    pub document_file_id: Uuid,
    pub document_name: String,
    pub document_sha256: String,
    pub slots: Value,
    pub send_to: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub signature_method: Option<String>,
    pub signature_slot: Option<Value>,
    pub signed_file_id: Option<Uuid>,
    pub signed_sha256: Option<String>,
    pub consent_version: Option<String>,
    pub signer_name: Option<String>,
    pub signer_email: Option<String>,
    pub signed_surface: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub sent_to: Option<String>,
}

/// The stored status. An unknown value (the CHECK forbids it) reads as an
/// end state so nothing can act on it.
fn row_status(row: &Row) -> Status {
    Status::parse(&row.status).unwrap_or(Status::Cancelled)
}

fn row_slots(row: &Row) -> Vec<SignSlot> {
    serde_json::from_value(row.slots.clone()).unwrap_or_default()
}

/// Writes every due pending request of the user as expired (lazy expiry,
/// plan decision 8: no sweep job).
pub(crate) async fn expire_due(db: &PgPool, user_id: &str, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE sign_requests SET status = 'expired', resolved_at = expires_at \
         WHERE user_id = $1 AND status = 'pending' AND expires_at <= $2",
    )
    .bind(user_id)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

async fn load(db: &PgPool, user_id: &str, id: Uuid) -> Result<Row, Fail> {
    sqlx::query_as::<_, Row>(&format!("SELECT {COLS} FROM sign_requests WHERE id = $1 AND user_id = $2"))
        .bind(id)
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| not_found().into())
}

/// The name of one of the user's live `user_files` rows, or `None`.
pub(crate) async fn owned_file_name(db: &PgPool, user_id: &str, id: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT name FROM user_files WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await
}

/// A conditional update matched nothing: someone else moved the request
/// first. Answer with what the state machine says about it now.
async fn lost_race(db: &PgPool, user_id: &str, id: Uuid, now: DateTime<Utc>, action: Action) -> Fail {
    match load(db, user_id, id).await {
        Ok(row) => match transition(row_status(&row), row.expires_at, now, action) {
            Err(refusal) => refusal.into(),
            Ok(_) => refuse(StatusCode::CONFLICT, "not-pending", "This request changed. Reload it.").into(),
        },
        Err(fail) => fail,
    }
}

pub(crate) async fn insert_request(
    db: &PgPool,
    user_id: &str,
    from_api_key: bool,
    b: &CreateBody,
    now: DateTime<Utc>,
) -> Result<Row, Fail> {
    let checked = check_create(b, from_api_key)?;
    let document_name = owned_file_name(db, user_id, b.document_file_id)
        .await?
        .ok_or_else(file_not_found)?;
    let expires_at = now + Duration::days(checked.expires_in_days);
    let row = sqlx::query_as::<_, Row>(&format!(
        "INSERT INTO sign_requests (id, user_id, status, requester_kind, requester_name, source_surface, \
         thread_id, title, summary, details, document_file_id, document_name, document_sha256, slots, \
         send_to, created_at, expires_at) \
         VALUES ($1, $2, 'pending', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) \
         RETURNING {COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(checked.requester_kind)
    .bind(trimmed(&b.requester_name))
    .bind(b.source_surface.trim())
    .bind(trimmed(&b.thread_id))
    .bind(b.title.trim())
    .bind(trimmed(&b.summary))
    .bind(sqlx::types::Json(&b.details))
    .bind(b.document_file_id)
    .bind(&document_name)
    .bind(&checked.document_sha256)
    .bind(sqlx::types::Json(&b.slots))
    .bind(b.send_to.clone().map(sqlx::types::Json))
    .bind(now)
    .bind(expires_at)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// The signing write. `signer` must come from [`interactive_signer`].
pub(crate) async fn apply_sign(
    db: &PgPool,
    signer: &Signer,
    id: Uuid,
    b: &SignBody,
    ip: Option<String>,
    user_agent: Option<String>,
    now: DateTime<Utc>,
) -> Result<Row, Fail> {
    let checked = check_sign(b)?;
    expire_due(db, &signer.id, now).await?;
    let row = load(db, &signer.id, id).await?;
    check_sign_against(
        row_status(&row),
        row.expires_at,
        now,
        &row.document_sha256,
        &row_slots(&row),
        &checked.document_sha256,
        &b.slot,
    )?;
    if owned_file_name(db, &signer.id, row.document_file_id).await?.is_none()
        || owned_file_name(db, &signer.id, b.signed_file_id).await?.is_none()
    {
        return Err(file_not_found().into());
    }
    let surface = Some(b.surface.trim()).filter(|s| !s.is_empty());
    let updated = sqlx::query_as::<_, Row>(&format!(
        "UPDATE sign_requests SET status = 'signed', resolved_at = $3, signature_method = $4, \
         signature_slot = $5, signature_png = $6, signed_file_id = $7, signed_sha256 = $8, \
         consent_version = $9, consent_text = $10, signer_name = $11, signer_email = $12, \
         signer_ip = $13, signer_user_agent = $14, signed_surface = $15 \
         WHERE id = $1 AND user_id = $2 AND status = 'pending' AND expires_at > $3 \
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(&signer.id)
    .bind(now)
    .bind(checked.method)
    .bind(sqlx::types::Json(&b.slot))
    .bind(&checked.png)
    .bind(b.signed_file_id)
    .bind(&checked.signed_sha256)
    .bind(CONSENT_VERSION)
    .bind(CONSENT_TEXT_V1)
    .bind(signer.name.as_deref())
    .bind(signer.email.as_deref())
    .bind(ip.as_deref())
    .bind(user_agent.as_deref())
    .bind(surface)
    .fetch_optional(db)
    .await?;
    match updated {
        Some(row) => Ok(row),
        None => Err(lost_race(db, &signer.id, id, now, Action::Sign).await),
    }
}

/// Decline or cancel: a pending request moves to that end state.
pub(crate) async fn apply_end(
    db: &PgPool,
    user_id: &str,
    id: Uuid,
    action: Action,
    reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Row, Fail> {
    let target = match action {
        Action::Decline => Status::Declined,
        Action::Cancel => Status::Cancelled,
        Action::Sign | Action::Sent => {
            return Err(ApiError::Internal("apply_end takes decline or cancel".to_string()).into())
        }
    };
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    check_len(reason, 1_000, "reason")?;
    expire_due(db, user_id, now).await?;
    let row = load(db, user_id, id).await?;
    transition(row_status(&row), row.expires_at, now, action)?;
    let updated = sqlx::query_as::<_, Row>(&format!(
        "UPDATE sign_requests SET status = $3, resolved_at = $4, decline_reason = $5 \
         WHERE id = $1 AND user_id = $2 AND status = 'pending' AND expires_at > $4 \
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(user_id)
    .bind(target.as_str())
    .bind(now)
    .bind(reason)
    .fetch_optional(db)
    .await?;
    match updated {
        Some(row) => Ok(row),
        None => Err(lost_race(db, user_id, id, now, action).await),
    }
}

/// Records that the signed copy was emailed (`to`), only when signed.
pub(crate) async fn apply_sent(db: &PgPool, user_id: &str, id: Uuid, to: &str, now: DateTime<Utc>) -> Result<Row, Fail> {
    check_email(to)?;
    expire_due(db, user_id, now).await?;
    let row = load(db, user_id, id).await?;
    transition(row_status(&row), row.expires_at, now, Action::Sent)?;
    let updated = sqlx::query_as::<_, Row>(&format!(
        "UPDATE sign_requests SET sent_at = $3, sent_to = $4 \
         WHERE id = $1 AND user_id = $2 AND status = 'signed' \
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(user_id)
    .bind(now)
    .bind(to.trim())
    .fetch_optional(db)
    .await?;
    match updated {
        Some(row) => Ok(row),
        None => Err(lost_race(db, user_id, id, now, Action::Sent).await),
    }
}

// ============================================================================
// JSON view
// ============================================================================

/// The contract's `SignRequest`.
#[derive(Debug, Serialize)]
pub(crate) struct SignRequestView {
    pub id: Uuid,
    pub status: &'static str,
    pub requester_kind: String,
    pub requester_name: Option<String>,
    pub source_surface: String,
    pub thread_id: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub details: Value,
    pub document_file_id: Uuid,
    pub document_name: String,
    pub document_sha256: String,
    pub slots: Value,
    pub send_to: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub signature_method: Option<String>,
    pub signature_slot: Option<Value>,
    pub signed_file_id: Option<Uuid>,
    pub signed_sha256: Option<String>,
    pub consent_version: Option<String>,
    pub signer_name: Option<String>,
    pub signer_email: Option<String>,
    pub signed_surface: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub sent_to: Option<String>,
    /// 10-minute presigned GET of the document; `None` when storage is off.
    pub document_url: Option<String>,
    /// 10-minute presigned GET of the signed copy, when signed.
    pub signed_url: Option<String>,
}

pub(crate) fn view(row: &Row, document_url: Option<String>, signed_url: Option<String>, now: DateTime<Utc>) -> SignRequestView {
    SignRequestView {
        id: row.id,
        status: effective_status(row_status(row), row.expires_at, now).as_str(),
        requester_kind: row.requester_kind.clone(),
        requester_name: row.requester_name.clone(),
        source_surface: row.source_surface.clone(),
        thread_id: row.thread_id.clone(),
        title: row.title.clone(),
        summary: row.summary.clone(),
        details: row.details.clone(),
        document_file_id: row.document_file_id,
        document_name: row.document_name.clone(),
        document_sha256: row.document_sha256.clone(),
        slots: row.slots.clone(),
        send_to: row.send_to.clone(),
        created_at: row.created_at,
        expires_at: row.expires_at,
        resolved_at: row.resolved_at,
        signature_method: row.signature_method.clone(),
        signature_slot: row.signature_slot.clone(),
        signed_file_id: row.signed_file_id,
        signed_sha256: row.signed_sha256.clone(),
        consent_version: row.consent_version.clone(),
        signer_name: row.signer_name.clone(),
        signer_email: row.signer_email.clone(),
        signed_surface: row.signed_surface.clone(),
        sent_at: row.sent_at,
        sent_to: row.sent_to.clone(),
        document_url,
        signed_url,
    }
}

async fn file_url(db: &PgPool, store: Option<&R2Client>, user_id: &str, id: Uuid) -> Option<String> {
    let store = store?;
    match user_files::download_url(db, store, user_id, id, None).await {
        Ok(v) => v.get("url").and_then(Value::as_str).map(str::to_owned),
        Err(_) => None,
    }
}

async fn render(db: &PgPool, store: Option<&R2Client>, row: &Row, now: DateTime<Utc>) -> SignRequestView {
    let document_url = file_url(db, store, &row.user_id, row.document_file_id).await;
    let signed_url = match (row.status.as_str(), row.signed_file_id) {
        ("signed", Some(id)) => file_url(db, store, &row.user_id, id).await,
        _ => None,
    };
    view(row, document_url, signed_url, now)
}

// ============================================================================
// Handlers
// ============================================================================

async fn create(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(b): Json<CreateBody>) -> Reply {
    let user = auth::resolve_user(&state.db, &headers).await?;
    let now = Utc::now();
    let row = insert_request(&state.db, &user.id, user.token_permissions.is_some(), &b, now).await?;

    // The same live notice approvals use (routes/approvals.rs create_approval):
    // clients attached to the thread's session hear about the ask.
    let session_key = row.thread_id.clone().unwrap_or_else(|| row.id.to_string());
    let _ = state
        .session_manager
        .notify_approval_needed(
            &session_key,
            &row.id.to_string(),
            "sign_request",
            &json!({
                "sign_request_id": row.id,
                "title": row.title,
                "document_name": row.document_name,
                "requester_kind": row.requester_kind,
                "requester_name": row.requester_name,
            }),
            row.summary.as_deref(),
            ApprovalPriority::Normal,
            None,
        )
        .await;

    let store = R2Client::from_env().ok();
    Ok((StatusCode::CREATED, Json(render(&state.db, store.as_ref(), &row, now).await)).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn list(State(state): State<Arc<ApiState>>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Reply {
    let user = auth::resolve_user(&state.db, &headers).await?;
    let status = match q.status.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => Some(
            Status::parse(raw)
                .ok_or_else(|| bad_request("status must be pending, signed, declined, cancelled or expired."))?
                .as_str(),
        ),
        None => None,
    };
    let thread_id = q.thread_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let limit = q.limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
    let now = Utc::now();
    expire_due(&state.db, &user.id, now).await?;
    let rows = sqlx::query_as::<_, Row>(&format!(
        "SELECT {COLS} FROM sign_requests \
         WHERE user_id = $1 AND ($2::text IS NULL OR status = $2) AND ($3::text IS NULL OR thread_id = $3) \
         ORDER BY created_at DESC LIMIT $4"
    ))
    .bind(&user.id)
    .bind(status)
    .bind(thread_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    let store = R2Client::from_env().ok();
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(render(&state.db, store.as_ref(), row, now).await);
    }
    Ok(Json(json!({ "items": items })).into_response())
}

async fn get(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>) -> Reply {
    let user = auth::resolve_user(&state.db, &headers).await?;
    let now = Utc::now();
    expire_due(&state.db, &user.id, now).await?;
    let row = load(&state.db, &user.id, id).await?;
    let store = R2Client::from_env().ok();
    Ok(Json(render(&state.db, store.as_ref(), &row, now).await).into_response())
}

async fn signature(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>) -> Reply {
    let signer = interactive_signer(&headers).await?;
    let png = sqlx::query_scalar::<_, Option<Vec<u8>>>(
        "SELECT signature_png FROM sign_requests WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(&signer.id)
    .fetch_optional(&state.db)
    .await?;
    match png {
        Some(Some(bytes)) => Ok((
            [(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, no-store")],
            bytes,
        )
            .into_response()),
        Some(None) => Err(refuse(StatusCode::NOT_FOUND, "not-found", "This request has no signature.").into()),
        None => Err(not_found().into()),
    }
}

async fn sign(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    Json(b): Json<SignBody>,
) -> Reply {
    let mut signer = interactive_signer(&headers).await?;
    if signer.email.is_none() {
        signer.email = user_trust::clerk_primary_email_status(&signer.id)
            .await
            .ok()
            .flatten()
            .map(|(email, _verified)| email);
    }
    let ip = crate::services::api_keys::client_ip(&headers, peer.map(|c| c.0)).map(|ip| ip.to_string());
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|ua| ua.chars().take(500).collect::<String>());
    let now = Utc::now();
    let row = apply_sign(&state.db, &signer, id, &b, ip, user_agent, now).await?;
    let store = R2Client::from_env().ok();
    Ok(Json(render(&state.db, store.as_ref(), &row, now).await).into_response())
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DeclineBody {
    #[serde(default)]
    pub reason: Option<String>,
}

async fn decline(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>, body: Bytes) -> Reply {
    let signer = interactive_signer(&headers).await?;
    let b: DeclineBody = if body.iter().all(u8::is_ascii_whitespace) {
        DeclineBody::default()
    } else {
        serde_json::from_slice(&body).map_err(|_| bad_request("The body must be JSON: {\"reason\": \"...\"}."))?
    };
    let now = Utc::now();
    let row = apply_end(&state.db, &signer.id, id, Action::Decline, b.reason.as_deref(), now).await?;
    let store = R2Client::from_env().ok();
    Ok(Json(render(&state.db, store.as_ref(), &row, now).await).into_response())
}

async fn cancel(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>) -> Reply {
    let user = auth::resolve_user(&state.db, &headers).await?;
    let now = Utc::now();
    let row = apply_end(&state.db, &user.id, id, Action::Cancel, None, now).await?;
    let store = R2Client::from_env().ok();
    Ok(Json(render(&state.db, store.as_ref(), &row, now).await).into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct SentBody {
    pub to: String,
}

async fn sent(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<SentBody>) -> Reply {
    let user = auth::resolve_user(&state.db, &headers).await?;
    let now = Utc::now();
    let row = apply_sent(&state.db, &user.id, id, &b.to, now).await?;
    let store = R2Client::from_env().ok();
    Ok(Json(render(&state.db, store.as_ref(), &row, now).await).into_response())
}

#[cfg(test)]
#[path = "sign_requests_tests.rs"]
mod tests;
