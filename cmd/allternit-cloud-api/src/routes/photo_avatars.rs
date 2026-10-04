//! Photo avatars included with a plan: the Allternit-paid lane for turning a photo of a person
//! into a bot sprite, limited per plan per UTC calendar month. Migration
//! `044_photo_avatar_usage.sql`.
//!
//! The other two lanes (the user's ChatGPT subscription on a Sessions computer, the user's own
//! OpenAI key) run client-side; this one runs on Allternit's key, so the monthly allowance lives
//! on the plan catalog (`SubscriptionPlan::photo_avatars_per_month`: Plus 5, Super 25, Ultra 50,
//! no active subscription 0).
//!
//! Routes (Clerk session or API token with the `compute` scope):
//! - `GET  /api/v1/avatars/photo/allowance` → `{available, reason?, plan, limit, used, remaining,
//!   resetsAt}`; `reason` is `not_configured` (no server key), `no_subscription`, `paused`
//!   (global monthly budget reached, or the OpenAI account is out of credit), or
//!   `allowance_exhausted`.
//! - `POST /api/v1/avatars/photo` `{photo, style, consentAttestedAt}` (`photo` is a PNG/JPEG/WebP
//!   data URL or bare base64, ≤15 MB) → `{image, remaining, limit, resetsAt}` (`image` is a
//!   base64 PNG). 400 bad input / no consent, 402 `no_subscription`, 429 `allowance_exhausted`,
//!   503 `not_configured` / `paused`, 422 `provider_rejected`, 502 `provider_error`.
//!
//! Key: `OPENAI_API_KEY` (already in the production service's EnvironmentFile,
//! `/opt/allternit-cloud-api/.env`); `ALLTERNIT_AVATAR_OPENAI_API_KEY` overrides it when set.
//!
//! Spend guard: every reservation also checks the lane's global spend this month
//! (`COST_PER_IMAGE_USD` × used-or-pending rows, all users) against
//! `ALLTERNIT_AVATAR_MONTHLY_BUDGET_USD` (default $4.00). At the budget the lane reports `paused`
//! instead of making a paid call. An OpenAI `insufficient_quota` / billing error also pauses the
//! lane for `QUOTA_PAUSE_MINUTES` (the failed attempt is recorded, so every instance sees it).
//!
//! Concurrency: a use is reserved under a lane-wide advisory lock before the provider call, then
//! marked `used` when an image came back or `released` when it didn't, so parallel requests can't
//! exceed the limit and a failure never costs an avatar. A reservation a crashed process never
//! settled stops counting after `RESERVATION_TTL_MINUTES`.
//!
//! Privacy: the source photo is downscaled in memory (≤1024 px) and sent to the provider; it is
//! never stored, and image bytes are never logged. Each attempt logs provider, model, cost
//! estimate and output size (house media rule), and nothing falls back to another paid lane.

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

use crate::{routes::billing_subscriptions::find_plan, routes::me_usage::next_month_start, ApiState};

pub const MAX_PHOTO_BYTES: usize = 15 * 1024 * 1024;
/// Base64 of the largest photo plus the rest of the JSON body.
const POST_BODY_LIMIT: usize = MAX_PHOTO_BYTES / 3 * 4 + 64 * 1024;
/// Longest edge of the photo sent to the provider.
pub const REFERENCE_MAX_EDGE: u32 = 1024;
pub const PROVIDER: &str = "openai";
pub const MODEL: &str = "gpt-image-1";
const SIZE: &str = "1024x1024";
const QUALITY: &str = "medium";
/// gpt-image-1, 1024×1024, quality medium, per image. Counted against the monthly budget.
pub const COST_PER_IMAGE_USD: f64 = 0.042;
/// Global spend cap for this lane when `ALLTERNIT_AVATAR_MONTHLY_BUDGET_USD` is unset or invalid.
pub const DEFAULT_MONTHLY_BUDGET_USD: f64 = 4.00;
pub const RESERVATION_TTL_MINUTES: i64 = 10;
/// How long an OpenAI `insufficient_quota` / billing error pauses the lane before the next try.
pub const QUOTA_PAUSE_MINUTES: i64 = 30;
/// `failure` prefix recorded on a reservation released for a billing error.
const QUOTA_FAILURE: &str = "insufficient_quota";
const OPENAI_EDITS_URL: &str = "https://api.openai.com/v1/images/edits";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/avatars/photo/allowance", get(allowance_route))
        .route("/api/v1/avatars/photo", post(generate_route).layer(DefaultBodyLimit::max(POST_BODY_LIMIT)))
}

/// Server key for the included lane: `ALLTERNIT_AVATAR_OPENAI_API_KEY` when set (override), else
/// `OPENAI_API_KEY`; `None` (lane off) when neither is set.
pub fn server_api_key() -> Option<String> {
    ["ALLTERNIT_AVATAR_OPENAI_API_KEY", "OPENAI_API_KEY"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// Global monthly spend cap for this lane, from `ALLTERNIT_AVATAR_MONTHLY_BUDGET_USD`.
pub fn monthly_budget_usd() -> f64 {
    parse_budget(std::env::var("ALLTERNIT_AVATAR_MONTHLY_BUDGET_USD").ok().as_deref())
}

pub fn parse_budget(value: Option<&str>) -> f64 {
    value
        .and_then(|v| v.trim().trim_start_matches('$').parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(DEFAULT_MONTHLY_BUDGET_USD)
}

/// Whether one more image fits under `budget` after `spent` (small epsilon for float sums).
pub fn budget_allows_one_more(spent: f64, budget: f64) -> bool {
    spent + COST_PER_IMAGE_USD <= budget + 1e-9
}

/// Monthly allowance for a plan id (`free` / unknown → 0).
pub fn monthly_limit(plan_id: &str) -> i64 {
    find_plan(plan_id).map(|plan| plan.photo_avatars_per_month).unwrap_or(0)
}

/// First day of `now`'s UTC calendar month and the instant the next one starts.
pub fn month_window(now: DateTime<Utc>) -> (NaiveDate, DateTime<Utc>) {
    let start = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).expect("first of month is a valid date");
    (start, next_month_start(now))
}

// ---- styles ---------------------------------------------------------------------------------

/// Style clause per sprite style; kept identical to the UI's `PHOTO_SPRITE_STYLES` so the three
/// lanes produce the same look. The prompt is built here, never taken from the client, so the
/// paid lane only ever makes avatars.
fn style_prompt(style: &str) -> Option<&'static str> {
    Some(match style {
        "pixel" => "16-bit pixel-art game sprite, clean limited palette, crisp square pixels, no anti-aliasing, bold dark outline",
        "chibi" => "cute chibi character, oversized head, small body, soft cel shading, clean line art, sticker-like",
        "mascot" => "friendly 3D mascot character, rounded toy-like forms, soft studio lighting, simple matte materials",
        _ => return None,
    })
}

pub fn build_prompt(style: &str) -> Option<String> {
    let clause = style_prompt(style)?;
    Some(
        [
            "Turn the person in this photo into a single character portrait for an app avatar.",
            "Keep their recognizable features: face shape, hairstyle and hair color, skin tone, glasses or facial hair if present.",
            "Head and shoulders, facing the viewer, centered, filling most of a square frame.",
            "Plain flat light background, no text, no border, no other people.",
            &format!("Style: {clause}."),
        ]
        .join(" "),
    )
}

// ---- photo handling -------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum PhotoError {
    Encoding,
    TooLarge,
    UnsupportedType,
    Unreadable,
}

impl PhotoError {
    fn message(&self) -> &'static str {
        match self {
            PhotoError::Encoding => "The photo must be a base64 image.",
            PhotoError::TooLarge => "The photo is larger than 15 MB.",
            PhotoError::UnsupportedType => "Use a PNG, JPEG or WebP photo.",
            PhotoError::Unreadable => "Could not read the photo.",
        }
    }
}

fn sniff_format(bytes: &[u8]) -> Option<image::ImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(image::ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(image::ImageFormat::Jpeg)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(image::ImageFormat::WebP)
    } else {
        None
    }
}

/// Decode a data URL / bare base64 photo, check type and size, and re-encode it as a PNG no
/// larger than `REFERENCE_MAX_EDGE` on its longest edge. The bytes only live in memory.
pub fn prepare_photo(input: &str) -> Result<Vec<u8>, PhotoError> {
    let data = match input.split_once(',') {
        Some((header, data)) if header.starts_with("data:") => {
            if !matches!(header, "data:image/png;base64" | "data:image/jpeg;base64" | "data:image/jpg;base64" | "data:image/webp;base64") {
                return Err(PhotoError::UnsupportedType);
            }
            data
        }
        Some(_) => return Err(PhotoError::Encoding),
        None => input,
    };
    if data.len() > MAX_PHOTO_BYTES / 3 * 4 + 4 {
        return Err(PhotoError::TooLarge);
    }
    let bytes = STANDARD.decode(data.trim()).map_err(|_| PhotoError::Encoding)?;
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(PhotoError::TooLarge);
    }
    let format = sniff_format(&bytes).ok_or(PhotoError::UnsupportedType)?;
    let mut reader = image::io::Reader::with_format(std::io::Cursor::new(&bytes), format);
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| PhotoError::Unreadable)?;
    let resized = if decoded.width().max(decoded.height()) > REFERENCE_MAX_EDGE {
        decoded.resize(REFERENCE_MAX_EDGE, REFERENCE_MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let mut png = Vec::new();
    resized
        .to_rgba8()
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageOutputFormat::Png)
        .map_err(|_| PhotoError::Unreadable)?;
    Ok(png)
}

// ---- provider -------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum ProviderError {
    /// The provider refused this input (e.g. its safety system). Shown to the user.
    Rejected(String),
    /// Network / 5xx / malformed response.
    Failed(String),
    /// The OpenAI account is out of credit (`insufficient_quota` / billing limit). Pauses the lane.
    QuotaExhausted(String),
}

/// Map a non-success OpenAI response to a provider error. Billing errors (`insufficient_quota`,
/// `billing_hard_limit_reached`, 429 billing messages) pause the lane; a plain rate limit doesn't.
pub fn classify_failure(status: u16, body: &serde_json::Value) -> ProviderError {
    let message = body["error"]["message"].as_str().unwrap_or("").to_string();
    let code = body["error"]["code"].as_str().unwrap_or("");
    let kind = body["error"]["type"].as_str().unwrap_or("");
    let billing = ["insufficient_quota", "billing_hard_limit_reached", "billing_not_active"];
    let lower = message.to_lowercase();
    if billing.contains(&code)
        || billing.contains(&kind)
        || (status == 429 && (lower.contains("quota") || lower.contains("billing")))
    {
        return ProviderError::QuotaExhausted(if code.is_empty() { kind.to_string() } else { code.to_string() });
    }
    if status == 400 {
        ProviderError::Rejected(message)
    } else {
        ProviderError::Failed(format!("{status}: {message}"))
    }
}

#[async_trait]
pub trait AvatarImageProvider: Send + Sync {
    /// Restyle `png` per `prompt`; returns the generated PNG bytes.
    async fn edit(&self, api_key: &str, png: Vec<u8>, prompt: &str) -> Result<Vec<u8>, ProviderError>;
}

/// OpenAI images edit (`gpt-image-1`, 1024×1024, quality medium).
pub struct OpenAiImagesEdit {
    client: reqwest::Client,
}

impl OpenAiImagesEdit {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .unwrap_or_default(),
        }
    }
}

/// multipart/form-data body for images/edits (built by hand; reqwest's multipart feature is off
/// in this workspace).
pub fn edit_form(boundary: &str, png: &[u8], prompt: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(png.len() + 1024);
    for (name, value) in [("model", MODEL), ("prompt", prompt), ("size", SIZE), ("quality", QUALITY), ("n", "1")] {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(png);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[async_trait]
impl AvatarImageProvider for OpenAiImagesEdit {
    async fn edit(&self, api_key: &str, png: Vec<u8>, prompt: &str) -> Result<Vec<u8>, ProviderError> {
        let boundary = format!("allternit-{}", uuid::Uuid::new_v4().simple());
        let response = self
            .client
            .post(OPENAI_EDITS_URL)
            .bearer_auth(api_key)
            .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
            .body(edit_form(&boundary, &png, prompt))
            .send()
            .await
            .map_err(|e| ProviderError::Failed(format!("request failed: {e}")))?;
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
        if !status.is_success() {
            return Err(classify_failure(status.as_u16(), &body));
        }
        let b64 = body["data"][0]["b64_json"]
            .as_str()
            .ok_or_else(|| ProviderError::Failed("response had no image".into()))?;
        STANDARD.decode(b64).map_err(|_| ProviderError::Failed("response image was not base64".into()))
    }
}

// ---- allowance ledger -----------------------------------------------------------------------

/// The caller's plan for the allowance: an active/trialing subscription, else `free`.
pub async fn active_plan_id(db: &PgPool, user_id: &str) -> Result<String, sqlx::Error> {
    if crate::auth::is_admin_user(user_id) {
        return Ok("ultra".to_string());
    }
    let plan: Option<String> = sqlx::query_scalar(
        "SELECT plan_id FROM billing_subscriptions WHERE user_id = $1 AND status IN ('active', 'trialing')
         ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(plan.unwrap_or_else(|| "free".to_string()))
}

const COUNTED: &str = "user_id = $1 AND period_start = $2
     AND (status = 'used' OR (status = 'reserved' AND created_at > now() - make_interval(mins => $3)))";

pub async fn used_this_month(db: &PgPool, user_id: &str, now: DateTime<Utc>) -> Result<i64, sqlx::Error> {
    let (start, _) = month_window(now);
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM photo_avatar_usage WHERE {COUNTED}"))
        .bind(user_id)
        .bind(start)
        .bind(RESERVATION_TTL_MINUTES as i32)
        .fetch_one(db)
        .await
}

/// Estimated spend on this lane this UTC month across all users (used + pending reservations).
pub async fn spent_this_month<'e, E: sqlx::PgExecutor<'e>>(db: E, now: DateTime<Utc>) -> Result<f64, sqlx::Error> {
    let (start, _) = month_window(now);
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(cost_estimate_usd), 0)::float8 FROM photo_avatar_usage WHERE period_start = $1
         AND (status = 'used' OR (status = 'reserved' AND created_at > now() - make_interval(mins => $2)))",
    )
    .bind(start)
    .bind(RESERVATION_TTL_MINUTES as i32)
    .fetch_one(db)
    .await
}

/// Whether an OpenAI billing error in the last `QUOTA_PAUSE_MINUTES` has paused the lane.
pub async fn quota_paused(db: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM photo_avatar_usage WHERE status = 'released' AND failure LIKE $1
         AND settled_at > now() - make_interval(mins => $2))",
    )
    .bind(format!("{QUOTA_FAILURE}%"))
    .bind(QUOTA_PAUSE_MINUTES as i32)
    .fetch_one(db)
    .await
}

/// Paused when an OpenAI billing error is recent or the monthly budget has no room for one more.
pub async fn lane_paused(db: &PgPool, budget: f64, now: DateTime<Utc>) -> Result<bool, sqlx::Error> {
    Ok(quota_paused(db).await? || !budget_allows_one_more(spent_this_month(db, now).await?, budget))
}

#[derive(Debug, PartialEq)]
pub struct Allowance {
    pub available: bool,
    pub reason: Option<&'static str>,
    pub plan: String,
    pub limit: i64,
    pub used: i64,
    pub resets_at: DateTime<Utc>,
}

impl Allowance {
    pub fn remaining(&self) -> i64 {
        (self.limit - self.used).max(0)
    }
}

/// Pure allowance math: configured first (no key = lane off), then plan, then the lane-wide
/// pause (budget / billing), then the user's usage.
pub fn compute_allowance(configured: bool, paused: bool, plan: &str, used: i64, now: DateTime<Utc>) -> Allowance {
    let limit = monthly_limit(plan);
    let (_, resets_at) = month_window(now);
    let reason = if !configured {
        Some("not_configured")
    } else if limit == 0 {
        Some("no_subscription")
    } else if paused {
        Some("paused")
    } else if used >= limit {
        Some("allowance_exhausted")
    } else {
        None
    };
    Allowance { available: reason.is_none(), reason, plan: plan.to_string(), limit, used, resets_at }
}

#[derive(Debug, PartialEq)]
pub enum ReserveOutcome {
    Reserved { id: String, used_after: i64 },
    Exhausted { used: i64 },
    /// The lane's global monthly budget has no room for one more image.
    BudgetReached { spent: f64 },
}

/// Take one slot of this month's allowance and budget, or report which ran out. A lane-wide
/// advisory lock makes check-then-insert atomic across parallel requests (and users), so neither
/// the per-user limit nor the global budget can be overrun.
pub async fn reserve(
    db: &PgPool,
    user_id: &str,
    plan: &str,
    limit: i64,
    budget: f64,
    style: &str,
    consent_attested_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<ReserveOutcome, sqlx::Error> {
    let (start, _) = month_window(now);
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('photo_avatar:lane', 0))")
        .execute(&mut *tx)
        .await?;
    let used: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM photo_avatar_usage WHERE {COUNTED}"))
        .bind(user_id)
        .bind(start)
        .bind(RESERVATION_TTL_MINUTES as i32)
        .fetch_one(&mut *tx)
        .await?;
    if used >= limit {
        tx.rollback().await?;
        return Ok(ReserveOutcome::Exhausted { used });
    }
    let spent = spent_this_month(&mut *tx, now).await?;
    if !budget_allows_one_more(spent, budget) {
        tx.rollback().await?;
        return Ok(ReserveOutcome::BudgetReached { spent });
    }
    let id = format!("pav_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO photo_avatar_usage (id, user_id, period_start, status, plan_id, style, provider, model, cost_estimate_usd, consent_attested_at)
         VALUES ($1, $2, $3, 'reserved', $4, $5, $6, $7, $8, $9)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(start)
    .bind(plan)
    .bind(style)
    .bind(PROVIDER)
    .bind(MODEL)
    .bind(COST_PER_IMAGE_USD)
    .bind(consent_attested_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ReserveOutcome::Reserved { id, used_after: used + 1 })
}

pub async fn confirm(db: &PgPool, id: &str, output_bytes: usize) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE photo_avatar_usage SET status = 'used', output_bytes = $2, settled_at = now() WHERE id = $1")
        .bind(id)
        .bind(output_bytes.min(i32::MAX as usize) as i32)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn release(db: &PgPool, id: &str, failure: &str) -> Result<(), sqlx::Error> {
    let failure: String = failure.chars().take(300).collect();
    sqlx::query("UPDATE photo_avatar_usage SET status = 'released', failure = $2, settled_at = now() WHERE id = $1")
        .bind(id)
        .bind(failure)
        .execute(db)
        .await?;
    Ok(())
}

// ---- generation -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateRequest {
    #[serde(default)]
    pub photo: String,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub consent_attested_at: Option<String>,
}

fn err(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

fn internal(error: sqlx::Error) -> Response {
    tracing::error!("photo avatars: {error}");
    err(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "internal" }))
}

/// The whole generate flow after auth, with the key and provider injected (tests mock both).
pub async fn generate_for_user(
    db: &PgPool,
    provider: &dyn AvatarImageProvider,
    api_key: Option<&str>,
    budget: f64,
    user_id: &str,
    request: GenerateRequest,
    now: DateTime<Utc>,
) -> Response {
    let Some(consent_attested_at) = request
        .consent_attested_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value.trim()).ok())
        .map(|value| value.with_timezone(&Utc))
    else {
        return err(
            StatusCode::BAD_REQUEST,
            json!({ "error": "consent_required", "message": "Confirm you have permission to use this person's likeness first." }),
        );
    };
    let Some(prompt) = build_prompt(&request.style) else {
        return err(StatusCode::BAD_REQUEST, json!({ "error": "invalid_style", "message": "Style must be pixel, chibi or mascot." }));
    };
    let Some(api_key) = api_key else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "error": "not_configured", "message": "Photo avatars included with your plan aren't available right now." }),
        );
    };
    let plan = match active_plan_id(db, user_id).await {
        Ok(plan) => plan,
        Err(e) => return internal(e),
    };
    let limit = monthly_limit(&plan);
    let (_, resets_at) = month_window(now);
    if limit == 0 {
        return err(
            StatusCode::PAYMENT_REQUIRED,
            json!({ "error": "no_subscription", "limit": 0, "message": "Photo avatars are included with Plus, Super and Ultra." }),
        );
    }
    let paused = || {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "error": "paused", "message": "Included avatars are paused this month.", "resetsAt": resets_at.to_rfc3339() }),
        )
    };
    match quota_paused(db).await {
        Ok(true) => return paused(),
        Ok(false) => {}
        Err(e) => return internal(e),
    }
    let png = match prepare_photo(&request.photo) {
        Ok(png) => png,
        Err(e) => return err(StatusCode::BAD_REQUEST, json!({ "error": "invalid_photo", "message": e.message() })),
    };
    let id = match reserve(db, user_id, &plan, limit, budget, &request.style, consent_attested_at, now).await {
        Ok(ReserveOutcome::Reserved { id, .. }) => id,
        Ok(ReserveOutcome::BudgetReached { spent }) => {
            tracing::warn!(spent_usd = spent, budget_usd = budget, "photo avatar: monthly budget reached, lane paused");
            return paused();
        }
        Ok(ReserveOutcome::Exhausted { .. }) => {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                json!({ "error": "allowance_exhausted", "limit": limit, "resetsAt": resets_at.to_rfc3339() }),
            )
        }
        Err(e) => return internal(e),
    };
    let input_bytes = png.len();
    let outcome = match provider.edit(api_key, png, &prompt).await {
        Ok(image) if sniff_format(&image).is_some() => Ok(image),
        Ok(_) => Err(ProviderError::Failed("response was not an image".into())),
        Err(error) => Err(error),
    };
    let image = match outcome {
        Ok(image) => image,
        Err(ProviderError::QuotaExhausted(code)) => {
            tracing::error!(user_id, reservation = %id, provider = PROVIDER, model = MODEL, code = %code, "photo avatar: OpenAI account out of credit, released and paused");
            if let Err(e) = release(db, &id, &format!("{QUOTA_FAILURE}: {code}")).await {
                return internal(e);
            }
            return paused();
        }
        Err(error) => {
            let (status, code, message) = match &error {
                ProviderError::Rejected(m) if !m.is_empty() => (StatusCode::UNPROCESSABLE_ENTITY, "provider_rejected", m.clone()),
                ProviderError::Rejected(_) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "provider_rejected",
                    "The image provider declined this photo. Try a different one.".to_string(),
                ),
                ProviderError::Failed(_) | ProviderError::QuotaExhausted(_) => (
                    StatusCode::BAD_GATEWAY,
                    "provider_error",
                    "Could not make the avatar. Try again; this didn't use one of your avatars.".to_string(),
                ),
            };
            tracing::warn!(user_id, reservation = %id, provider = PROVIDER, model = MODEL, input_bytes, ?error, "photo avatar: provider failed, released");
            if let Err(e) = release(db, &id, &format!("{error:?}")).await {
                return internal(e);
            }
            return err(status, json!({ "error": code, "message": message }));
        }
    };
    if let Err(e) = confirm(db, &id, image.len()).await {
        return internal(e);
    }
    let used = match used_this_month(db, user_id, now).await {
        Ok(used) => used,
        Err(e) => return internal(e),
    };
    tracing::info!(
        user_id,
        reservation = %id,
        plan = %plan,
        style = %request.style,
        provider = PROVIDER,
        model = MODEL,
        cost_estimate_usd = COST_PER_IMAGE_USD,
        input_bytes,
        output_bytes = image.len(),
        "photo avatar generated (included lane)"
    );
    Json(json!({
        "image": STANDARD.encode(&image),
        "remaining": (limit - used).max(0),
        "limit": limit,
        "resetsAt": resets_at.to_rfc3339(),
    }))
    .into_response()
}

async fn allowance_for_user(db: &PgPool, configured: bool, budget: f64, user_id: &str, now: DateTime<Utc>) -> Result<Allowance, sqlx::Error> {
    let plan = active_plan_id(db, user_id).await?;
    let subscribed = monthly_limit(&plan) > 0;
    let used = if subscribed { used_this_month(db, user_id, now).await? } else { 0 };
    let paused = configured && subscribed && lane_paused(db, budget, now).await?;
    Ok(compute_allowance(configured, paused, &plan, used, now))
}

fn allowance_json(a: &Allowance) -> serde_json::Value {
    json!({
        "available": a.available,
        "reason": a.reason,
        "plan": a.plan,
        "limit": a.limit,
        "used": a.used.min(a.limit),
        "remaining": a.remaining(),
        "resetsAt": a.resets_at.to_rfc3339(),
    })
}

async fn allowance_route(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(user) => user.id,
        Err(e) => return e.into_response(),
    };
    match allowance_for_user(&state.db, server_api_key().is_some(), monthly_budget_usd(), &user, Utc::now()).await {
        Ok(allowance) => Json(allowance_json(&allowance)).into_response(),
        Err(e) => internal(e),
    }
}

async fn generate_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(request): Json<GenerateRequest>) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(user) => user.id,
        Err(e) => return e.into_response(),
    };
    let key = server_api_key();
    generate_for_user(&state.db, &OpenAiImagesEdit::new(), key.as_deref(), monthly_budget_usd(), &user, request, Utc::now()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
    }

    fn tiny_png(side: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(side, side, image::Rgba([200, 120, 40, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageOutputFormat::Png)
            .unwrap();
        out
    }

    fn data_url(png: &[u8]) -> String {
        format!("data:image/png;base64,{}", STANDARD.encode(png))
    }

    struct MockProvider {
        calls: AtomicUsize,
        fail: Option<ProviderError>,
    }

    impl MockProvider {
        fn ok() -> Self {
            Self { calls: AtomicUsize::new(0), fail: None }
        }
        fn failing(error: ProviderError) -> Self {
            Self { calls: AtomicUsize::new(0), fail: Some(error) }
        }
    }

    #[async_trait]
    impl AvatarImageProvider for MockProvider {
        async fn edit(&self, _key: &str, png: Vec<u8>, prompt: &str) -> Result<Vec<u8>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(prompt.contains("Style:"));
            assert!(sniff_format(&png) == Some(image::ImageFormat::Png));
            match &self.fail {
                Some(ProviderError::Rejected(m)) => Err(ProviderError::Rejected(m.clone())),
                Some(ProviderError::Failed(m)) => Err(ProviderError::Failed(m.clone())),
                Some(ProviderError::QuotaExhausted(m)) => Err(ProviderError::QuotaExhausted(m.clone())),
                None => Ok(tiny_png(8)),
            }
        }
    }

    async fn pool_with_tables() -> PgPool {
        let pool = crate::routes::test_support::test_pool().await;
        sqlx::query(
            "CREATE TABLE billing_subscriptions (user_id TEXT NOT NULL, plan_id TEXT NOT NULL, status TEXT NOT NULL,
             updated_at TIMESTAMPTZ NOT NULL DEFAULT now())",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations_pg/044_photo_avatar_usage.sql"))
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn subscribe(pool: &PgPool, user: &str, plan: &str, status: &str) {
        sqlx::query("INSERT INTO billing_subscriptions (user_id, plan_id, status) VALUES ($1, $2, $3)")
            .bind(user)
            .bind(plan)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
    }

    fn request(style: &str, consent: Option<&str>) -> GenerateRequest {
        GenerateRequest {
            photo: data_url(&tiny_png(32)),
            style: style.to_string(),
            consent_attested_at: consent.map(str::to_string),
        }
    }

    async fn body(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    const CONSENT: Option<&str> = Some("2026-10-04T10:00:00Z");
    /// Budget roomy enough that per-user tests never hit it.
    const B: f64 = 100.0;

    #[test]
    fn plan_limits_are_on_the_plan_catalog() {
        assert_eq!(monthly_limit("plus"), 5);
        assert_eq!(monthly_limit("super"), 25);
        assert_eq!(monthly_limit("ultra"), 50);
        assert_eq!(monthly_limit("free"), 0);
        assert_eq!(monthly_limit("nonsense"), 0);
    }

    #[test]
    fn month_window_is_the_utc_calendar_month() {
        let (start, next) = month_window(Utc.with_ymd_and_hms(2026, 10, 31, 23, 59, 59).unwrap());
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
        let (start, next) = month_window(at(2026, 12, 15));
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
        assert_eq!(next, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn allowance_math() {
        let now = at(2026, 10, 4);
        let a = compute_allowance(true, false, "plus", 2, now);
        assert!(a.available);
        assert_eq!((a.limit, a.remaining(), a.reason), (5, 3, None));
        assert_eq!(a.resets_at, Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
        let a = compute_allowance(true, false, "plus", 5, now);
        assert_eq!((a.available, a.reason, a.remaining()), (false, Some("allowance_exhausted"), 0));
        let a = compute_allowance(true, false, "free", 0, now);
        assert_eq!((a.available, a.reason, a.limit), (false, Some("no_subscription"), 0));
        let a = compute_allowance(true, true, "plus", 0, now);
        assert_eq!((a.available, a.reason), (false, Some("paused")));
        // No subscription wins over paused: the user can't use the lane either way.
        let a = compute_allowance(true, true, "free", 0, now);
        assert_eq!(a.reason, Some("no_subscription"));
        let a = compute_allowance(false, false, "ultra", 0, now);
        assert_eq!((a.available, a.reason, a.limit), (false, Some("not_configured"), 50));
    }

    #[test]
    fn photo_is_validated_and_downscaled() {
        let big = tiny_png(1500);
        let png = prepare_photo(&data_url(&big)).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 1024));
        // Bare base64 works; small photos keep their size.
        let png = prepare_photo(&STANDARD.encode(tiny_png(40))).unwrap();
        assert_eq!(image::load_from_memory(&png).unwrap().width(), 40);
        assert_eq!(prepare_photo("data:image/gif;base64,R0lGOD"), Err(PhotoError::UnsupportedType));
        assert_eq!(prepare_photo(&STANDARD.encode(b"GIF89a....")), Err(PhotoError::UnsupportedType));
        assert_eq!(prepare_photo("data:image/png;base64,***"), Err(PhotoError::Encoding));
        let huge = "A".repeat(MAX_PHOTO_BYTES / 3 * 4 + 400);
        assert_eq!(prepare_photo(&huge), Err(PhotoError::TooLarge));
    }

    #[test]
    fn edit_form_carries_the_fixed_parameters() {
        let form = String::from_utf8_lossy(&edit_form("B", b"PNGDATA", "make a sprite")).to_string();
        for needle in ["name=\"model\"\r\n\r\ngpt-image-1", "name=\"size\"\r\n\r\n1024x1024", "name=\"quality\"\r\n\r\nmedium", "filename=\"photo.png\"", "PNGDATA", "--B--"] {
            assert!(form.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn only_known_styles_build_a_prompt() {
        assert!(build_prompt("pixel").unwrap().contains("pixel-art"));
        assert!(build_prompt("chibi").is_some() && build_prompt("mascot").is_some());
        assert!(build_prompt("anything you like").is_none());
    }

    #[tokio::test]
    async fn not_configured_answers_503_without_calling_the_provider() {
        let pool = pool_with_tables().await;
        subscribe(&pool, "u1", "plus", "active").await;
        let provider = MockProvider::ok();
        let (status, json) = body(generate_for_user(&pool, &provider, None, B, "u1", request("pixel", CONSENT), at(2026, 10, 4)).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"], "not_configured");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let a = allowance_for_user(&pool, false, B, "u1", at(2026, 10, 4)).await.unwrap();
        assert_eq!(a.reason, Some("not_configured"));
    }

    #[tokio::test]
    async fn consent_is_required() {
        let pool = pool_with_tables().await;
        subscribe(&pool, "u1", "plus", "active").await;
        let provider = MockProvider::ok();
        for consent in [None, Some("yesterday")] {
            let (status, json) = body(generate_for_user(&pool, &provider, Some("k"), B, "u1", request("pixel", consent), at(2026, 10, 4)).await).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(json["error"], "consent_required");
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_subscription_gets_zero() {
        let pool = pool_with_tables().await;
        subscribe(&pool, "u2", "plus", "canceled").await;
        let provider = MockProvider::ok();
        let (status, json) = body(generate_for_user(&pool, &provider, Some("k"), B, "u2", request("chibi", CONSENT), at(2026, 10, 4)).await).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(json["error"], "no_subscription");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let a = allowance_for_user(&pool, true, B, "u2", at(2026, 10, 4)).await.unwrap();
        assert_eq!((a.plan.as_str(), a.limit, a.reason), ("free", 0, Some("no_subscription")));
    }

    #[tokio::test]
    async fn plus_exhausts_after_five_and_resets_next_month() {
        let pool = pool_with_tables().await;
        subscribe(&pool, "u3", "plus", "active").await;
        let provider = MockProvider::ok();
        let now = at(2026, 10, 4);
        for expected_remaining in (0..5).rev() {
            let (status, json) = body(generate_for_user(&pool, &provider, Some("k"), B, "u3", request("mascot", CONSENT), now).await).await;
            assert_eq!(status, StatusCode::OK, "{json}");
            assert_eq!(json["remaining"], expected_remaining);
            assert_eq!(json["limit"], 5);
            assert_eq!(json["resetsAt"], "2026-11-01T00:00:00+00:00");
            assert!(image::load_from_memory(&STANDARD.decode(json["image"].as_str().unwrap()).unwrap()).is_ok());
        }
        let (status, json) = body(generate_for_user(&pool, &provider, Some("k"), B, "u3", request("mascot", CONSENT), now).await).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(json["error"], "allowance_exhausted");
        assert_eq!(json["limit"], 5);
        assert_eq!(json["resetsAt"], "2026-11-01T00:00:00+00:00");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 5);
        let a = allowance_for_user(&pool, true, B, "u3", now).await.unwrap();
        assert_eq!((a.available, a.reason, a.remaining()), (false, Some("allowance_exhausted"), 0));
        // A new month has a fresh allowance.
        let a = allowance_for_user(&pool, true, B, "u3", at(2026, 11, 1)).await.unwrap();
        assert_eq!((a.available, a.used, a.remaining()), (true, 0, 5));
    }

    #[tokio::test]
    async fn failure_releases_the_reservation() {
        let pool = pool_with_tables().await;
        subscribe(&pool, "u4", "plus", "active").await;
        let now = at(2026, 10, 4);
        let failing = MockProvider::failing(ProviderError::Failed("503".into()));
        let (status, json) = body(generate_for_user(&pool, &failing, Some("k"), B, "u4", request("pixel", CONSENT), now).await).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"], "provider_error");
        let rejecting = MockProvider::failing(ProviderError::Rejected("safety system".into()));
        let (status, json) = body(generate_for_user(&pool, &rejecting, Some("k"), B, "u4", request("pixel", CONSENT), now).await).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["message"], "safety system");
        assert_eq!(used_this_month(&pool, "u4", now).await.unwrap(), 0);
        let released: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photo_avatar_usage WHERE user_id = 'u4' AND status = 'released'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(released, 2);
    }

    #[tokio::test]
    async fn reservations_cannot_overrun_the_limit() {
        let pool = pool_with_tables().await;
        let now = at(2026, 10, 4);
        let consent = at(2026, 10, 4);
        let mut reserved = 0;
        for _ in 0..7 {
            match reserve(&pool, "u5", "plus", 5, B, "pixel", consent, now).await.unwrap() {
                ReserveOutcome::Reserved { .. } => reserved += 1,
                ReserveOutcome::Exhausted { used } => assert_eq!(used, 5),
                ReserveOutcome::BudgetReached { spent } => panic!("budget reached at {spent}"),
            }
        }
        assert_eq!(reserved, 5);
        // Pending reservations count; a stale one (crashed process) stops counting.
        sqlx::query("UPDATE photo_avatar_usage SET created_at = now() - interval '11 minutes' WHERE user_id = 'u5'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(used_this_month(&pool, "u5", now).await.unwrap(), 0);
    }

    #[test]
    fn budget_defaults_and_math() {
        assert_eq!(parse_budget(None), 4.0);
        assert_eq!(parse_budget(Some("10")), 10.0);
        assert_eq!(parse_budget(Some(" $2.50 ")), 2.5);
        assert_eq!(parse_budget(Some("lots")), 4.0);
        assert_eq!(parse_budget(Some("-1")), 4.0);
        assert_eq!(COST_PER_IMAGE_USD, 0.042);
        // $4.00 buys 95 images at $0.042; the 96th would pass the cap.
        assert!(budget_allows_one_more(94.0 * COST_PER_IMAGE_USD, 4.0));
        assert!(!budget_allows_one_more(95.0 * COST_PER_IMAGE_USD, 4.0));
        assert!(!budget_allows_one_more(0.0, 0.0));
    }

    #[test]
    fn billing_errors_are_told_apart_from_rate_limits() {
        let quota = json!({ "error": { "message": "You exceeded your current quota, please check your plan and billing details.", "type": "insufficient_quota", "code": "insufficient_quota" } });
        assert_eq!(classify_failure(429, &quota), ProviderError::QuotaExhausted("insufficient_quota".into()));
        let hard = json!({ "error": { "message": "Billing hard limit has been reached", "code": "billing_hard_limit_reached" } });
        assert_eq!(classify_failure(400, &hard), ProviderError::QuotaExhausted("billing_hard_limit_reached".into()));
        let rate = json!({ "error": { "message": "Rate limit reached for images per minute", "code": "rate_limit_exceeded" } });
        assert!(matches!(classify_failure(429, &rate), ProviderError::Failed(_)));
        let safety = json!({ "error": { "message": "rejected by the safety system", "code": "moderation_blocked" } });
        assert_eq!(classify_failure(400, &safety), ProviderError::Rejected("rejected by the safety system".into()));
    }

    #[tokio::test]
    async fn budget_reached_pauses_the_lane_without_a_paid_call() {
        let pool = pool_with_tables().await;
        subscribe(&pool, "u6", "ultra", "active").await;
        subscribe(&pool, "u7", "plus", "active").await;
        let now = at(2026, 10, 4);
        // Budget for exactly two images, spread across two users.
        let budget = 2.0 * COST_PER_IMAGE_USD;
        let provider = MockProvider::ok();
        for user in ["u6", "u7"] {
            let (status, json) = body(generate_for_user(&pool, &provider, Some("k"), budget, user, request("pixel", CONSENT), now).await).await;
            assert_eq!(status, StatusCode::OK, "{json}");
        }
        let (status, json) = body(generate_for_user(&pool, &provider, Some("k"), budget, "u6", request("pixel", CONSENT), now).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"], "paused");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert!((spent_this_month(&pool, now).await.unwrap() - budget).abs() < 1e-9);
        let a = allowance_for_user(&pool, true, budget, "u6", now).await.unwrap();
        assert_eq!((a.available, a.reason, a.remaining()), (false, Some("paused"), 49));
        // Raising the budget reopens it; a new month starts from zero spend.
        assert!(allowance_for_user(&pool, true, 1.0, "u6", now).await.unwrap().available);
        assert!(allowance_for_user(&pool, true, budget, "u6", at(2026, 11, 2)).await.unwrap().available);
    }

    #[tokio::test]
    async fn insufficient_quota_releases_and_pauses() {
        let pool = pool_with_tables().await;
        subscribe(&pool, "u8", "plus", "active").await;
        let now = at(2026, 10, 4);
        let broke = MockProvider::failing(ProviderError::QuotaExhausted("insufficient_quota".into()));
        let (status, json) = body(generate_for_user(&pool, &broke, Some("k"), B, "u8", request("pixel", CONSENT), now).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"], "paused");
        assert_eq!(used_this_month(&pool, "u8", now).await.unwrap(), 0);
        let a = allowance_for_user(&pool, true, B, "u8", now).await.unwrap();
        assert_eq!((a.reason, a.remaining()), (Some("paused"), 5));
        // While paused, nothing reaches the provider.
        let provider = MockProvider::ok();
        let (status, _) = body(generate_for_user(&pool, &provider, Some("k"), B, "u8", request("pixel", CONSENT), now).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        // After the pause window the next request tries again.
        sqlx::query("UPDATE photo_avatar_usage SET settled_at = now() - interval '31 minutes'")
            .execute(&pool)
            .await
            .unwrap();
        let (status, _) = body(generate_for_user(&pool, &provider, Some("k"), B, "u8", request("pixel", CONSENT), now).await).await;
        assert_eq!(status, StatusCode::OK);
    }
}
