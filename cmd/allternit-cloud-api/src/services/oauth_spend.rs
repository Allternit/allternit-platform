//! "Sign in with Allternit": who pays for an `alt_oat_` inference call
//! (CONTRACT-oauth-linked-billing-2026-11.md, A8 "Reserve, then settle", D4, D5).
//!
//! The consenting person's wallet (`user_credits`) pays, under the monthly cap
//! they set for the app (`oauth_grants.monthly_cap_cents`). Each call:
//!
//! 1. **Bound.** [`bound_output`] fixes the most output the call can produce:
//!    the request's `max_tokens` / `max_completion_tokens`, else
//!    [`DEFAULT_MAX_TOKENS`], written back into the request so the upstream is
//!    held to the same number the reservation was priced on (`n` multiplies it).
//! 2. **Reserve** ([`reserve`]), one transaction: lock this month's
//!    `oauth_grant_spend` row (created when missing), then the wallet row,
//!    both `FOR UPDATE`. `spent + reserved + estimate > cap` is a 402
//!    [`APP_SPEND_CAP_REACHED`]; a wallet whose balance less every hold the
//!    person's grants carry is under the estimate is a 402
//!    [`USER_CREDITS_EXHAUSTED`]. Both refuse before any upstream call. Nothing
//!    is clamped and no auto top-up runs. Otherwise the estimate is added to
//!    `reserved_cents`.
//! 3. **Settle** ([`settle`]), one transaction, same lock order: the hold is
//!    released, and `min(actual, held)` is taken from the wallet by the strict
//!    debit (`cost_service::debit_wallet_strict`: refuses rather than clamps,
//!    never starts auto top-up) and added to `spent_cents`. An upstream error
//!    or a non-2xx answer settles at 0: a full release, no charge.
//!
//! Lock order is always the grant's spend row, then the wallet row, so reserve
//! and settle cannot deadlock each other. Every change to any `reserved_cents`
//! happens while the person's wallet row is locked, so the hold total read in
//! [`reserve`] is consistent.
//!
//! **Units.** The cap and `spent_cents` are whole cents; the wallet is USD. The
//! estimate is rounded up to whole cents (at least 1). The wallet is debited the
//! exact cost; `spent_cents` gets that cost rounded up to the cent, so the cap
//! is never under-counted and the app can never spend past it. The overrun is
//! at most one reservation, and in practice zero, because the charge is capped
//! at the hold.
//!
//! **Streams.** [`meter_stream`] wraps the body. Settlement fires once, when
//! the stream ends or when the body is dropped (client abort, disconnect,
//! upstream error). It charges the upstream's reported usage when the final
//! usage chunk arrived. Otherwise (aborted mid-way) it charges the
//! reservation's input part plus the output already streamed to the client
//! (its text, at 4 characters a token); a stream aborted before any output is
//! charged the input part only. Always capped at the hold.
//!
//! **No leaks.** A [`ReservationGuard`] owns each hold. If it is dropped
//! without settling (an early return, an error, a panic), it spawns a full
//! release.
//!
//! **Records.** Each settled call writes the usual `inference_usage` row
//! through `inference_settlement::settle_inference` with `charge_user = false`
//! (the strict debit already took the money): `user_id` is the person,
//! `client` is `oauth_app:<client_id>` (see [`usage_client`]), `project_id`
//! and `key_id` are empty (the app's project does not pay). The wallet ledger
//! row (`credit_transactions`) has source [`WALLET_SOURCE`] and
//! `transaction_id` `oauth:<grant_id>:<request_id>` ([`ledger_ref`]), which
//! ties the debit to the grant and to the usage row's `request_id`.
//! The row also carries `billed_to = 'user_wallet'`, `oauth_app_id` and
//! `oauth_grant_id` (migrations_pg/151).

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::{
    body::{Body, Bytes},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use hyper::body::{Body as HttpBody, Frame};
use serde_json::{json, Value};
use sqlx::PgPool;
use tracing::{error, warn};
use uuid::Uuid;

use crate::{
    model_router::{ChatCompletionRequest, RetailPrices},
    services::{
        cost_service,
        inference_settlement::{self, InferenceAttribution},
        oauth_apps::{self, AuditCtx, AuditEvent},
        oauth_tokens::{month_resets_at, spend_month},
    },
    ApiError,
};

/// 402 code: the call would take the grant past its monthly cap (A9).
pub const APP_SPEND_CAP_REACHED: &str = "app_spend_cap_reached";
/// 402 code: the person's wallet cannot cover the call (A9).
pub const USER_CREDITS_EXHAUSTED: &str = "user_credits_exhausted";
/// 401 code: the grant was revoked between the token check and the reservation.
pub const GRANT_REVOKED: &str = "oauth_grant_revoked";
/// Output tokens a call may produce when the app sent no `max_tokens`. Sent
/// upstream, so the reservation really bounds the cost.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;
/// `credit_transactions.source` of an OAuth wallet debit.
pub const WALLET_SOURCE: &str = "oauth_app_inference";
/// Who pays an `alt_oat_` call (contract A8).
pub const BILLED_TO: &str = "user_wallet";
/// Input tokens counted for each non-text content part (an image, audio).
const NON_TEXT_PART_TOKENS: u64 = 1_500;
/// Input tokens counted for each message's framing.
const MESSAGE_OVERHEAD_TOKENS: u64 = 4;
/// Cap on a buffered non-streaming upstream body (as `inference_settlement`).
const MAX_BUFFERED_JSON_BODY: usize = 32 * 1024 * 1024;

// ─── estimate ─────────────────────────────────────────────────────────────────

/// The most output tokens this call can produce, with the request rewritten so
/// the upstream is held to it. The per-choice bound is the smaller of
/// `max_tokens` and `max_completion_tokens` when the app sent either, else
/// [`DEFAULT_MAX_TOKENS`]; it is written back to the field(s) the app used
/// (`max_tokens` when it sent neither). `n` choices multiply it.
pub fn bound_output(request: &mut ChatCompletionRequest) -> u64 {
    let max_completion = request.extra.get("max_completion_tokens").and_then(Value::as_u64);
    let per_choice = match (request.max_tokens.map(u64::from), max_completion) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => u64::from(DEFAULT_MAX_TOKENS),
    }
    .clamp(1, u64::from(u32::MAX));
    let per_choice = u32::try_from(per_choice).unwrap_or(u32::MAX);
    if max_completion.is_some() {
        request.extra.insert("max_completion_tokens".to_string(), json!(per_choice));
        if request.max_tokens.is_some() {
            request.max_tokens = Some(per_choice);
        }
    } else {
        request.max_tokens = Some(per_choice);
    }
    let choices = request.extra.get("n").and_then(Value::as_u64).unwrap_or(1).clamp(1, 128);
    u64::from(per_choice) * choices
}

/// Input tokens for the reservation: message text and tool definitions at 3
/// bytes a token (denser than the usual 4, so non-English text is covered),
/// plus framing per message and a flat amount per non-text part.
pub fn input_token_estimate(request: &ChatCompletionRequest) -> u64 {
    let text_bytes: usize = request.messages.iter().map(|m| m.content_chars()).sum();
    let tool_bytes = request.extra.get("tools").map(|tools| tools.to_string().len()).unwrap_or(0);
    let non_text_parts: u64 = request
        .messages
        .iter()
        .filter_map(|m| m.content.as_array())
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str).is_some_and(|kind| kind != "text"))
        .count() as u64;
    ((text_bytes + tool_bytes) as u64).div_ceil(3)
        + MESSAGE_OVERHEAD_TOKENS * request.messages.len() as u64
        + NON_TEXT_PART_TOKENS * non_text_parts
}

/// Retail cost in USD of `prompt` input and `completion` output tokens.
pub fn cost_usd(prices: &RetailPrices, prompt: u64, completion: u64) -> f64 {
    (prompt as f64 / 1_000_000.0) * prices.prompt_per_1m + (completion as f64 / 1_000_000.0) * prices.completion_per_1m
}

/// USD rounded up to whole cents. Float noise under a billionth of a cent is
/// ignored, so $0.30 is 30 cents, not 31. Zero, negative and NaN are 0.
pub fn usd_to_cents_ceil(usd: f64) -> i64 {
    if !(usd > 0.0) {
        return 0;
    }
    (usd * 100.0 - 1e-9).ceil().max(0.0) as i64
}

/// What one call may cost at most.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostEstimate {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The input part alone, in USD (what an aborted stream with no output pays).
    pub input_usd: f64,
    /// The hold: input plus the output bound, rounded up to cents, at least 1.
    pub cents: i64,
}

/// The reservation for `input_tokens` in and `output_tokens` out at `prices`.
pub fn estimate(prices: &RetailPrices, input_tokens: u64, output_tokens: u64) -> CostEstimate {
    let input_usd = cost_usd(prices, input_tokens, 0);
    let total = input_usd + cost_usd(prices, 0, output_tokens);
    CostEstimate { input_tokens, output_tokens, input_usd, cents: usd_to_cents_ceil(total).max(1) }
}

// ─── reserve ──────────────────────────────────────────────────────────────────

/// A hold on a grant's month and on the person's wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub id: Uuid,
    pub grant_id: Uuid,
    pub user_id: String,
    /// The `oauth_grant_spend.month` the hold sits on. Settlement uses the same
    /// row, even when the call ends after midnight on the 1st.
    pub month: String,
    pub reserved_cents: i64,
}

/// Why a reservation was refused. Nothing was held and no upstream was called.
#[derive(Debug)]
pub enum SpendError {
    CapReached { cap_cents: i64, spent_cents: i64, reserved_cents: i64, estimate_cents: i64, resets_at: DateTime<Utc> },
    CreditsExhausted { available_cents: i64, estimate_cents: i64 },
    GrantRevoked,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for SpendError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl IntoResponse for SpendError {
    fn into_response(self) -> Response {
        let refusal = |status: StatusCode, body: Value| (status, Json(json!({ "error": body }))).into_response();
        match self {
            SpendError::CapReached { cap_cents, spent_cents, reserved_cents, estimate_cents, resets_at } => refusal(
                StatusCode::PAYMENT_REQUIRED,
                json!({
                    "type": "billing_error",
                    "code": APP_SPEND_CAP_REACHED,
                    "message": "This app has reached the monthly spending limit you set for it. Raise the limit in your Allternit settings under apps with access, or wait for it to reset on the 1st (UTC).",
                    "cap_cents": cap_cents,
                    "spent_cents": spent_cents,
                    "reserved_cents": reserved_cents,
                    "estimate_cents": estimate_cents,
                    "resets_at": resets_at.to_rfc3339(),
                }),
            ),
            SpendError::CreditsExhausted { available_cents, estimate_cents } => refusal(
                StatusCode::PAYMENT_REQUIRED,
                json!({
                    "type": "billing_error",
                    "code": USER_CREDITS_EXHAUSTED,
                    "message": "Your Allternit credits are too low for this request. Add credits to keep using this app.",
                    "available_cents": available_cents,
                    "estimate_cents": estimate_cents,
                    "billing_url": inference_settlement::CLOUD_CREDITS_BILLING_URL,
                }),
            ),
            SpendError::GrantRevoked => refusal(
                StatusCode::UNAUTHORIZED,
                json!({
                    "type": "authentication_error",
                    "code": GRANT_REVOKED,
                    "message": "This app's access was revoked. Sign in with Allternit again.",
                }),
            ),
            SpendError::Database(error) => ApiError::DatabaseError(error).into_response(),
        }
    }
}

/// Hold `est_cents` on the grant's current month and the person's wallet, or
/// refuse (module doc, step 2).
pub async fn reserve(db: &PgPool, grant_id: Uuid, user_id: &str, est_cents: i64) -> Result<Reservation, SpendError> {
    reserve_at(db, grant_id, user_id, est_cents, Utc::now()).await
}

/// [`reserve`] with the clock passed in (the month window is `spend_month(now)`).
pub async fn reserve_at(
    db: &PgPool,
    grant_id: Uuid,
    user_id: &str,
    est_cents: i64,
    now: DateTime<Utc>,
) -> Result<Reservation, SpendError> {
    let est_cents = est_cents.max(0);
    let month = spend_month(now);
    let mut tx = db.begin().await?;

    // The cap as it is now: a cap change applies to the current month at once.
    let grant: Option<(Uuid, i32, bool, String)> = sqlx::query_as(
        "SELECT app_id, monthly_cap_cents, revoked_at IS NOT NULL, user_id FROM oauth_grants WHERE id = $1",
    )
    .bind(grant_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((app_id, cap_cents, revoked, owner)) = grant else {
        return Err(SpendError::GrantRevoked);
    };
    if revoked || owner != user_id {
        return Err(SpendError::GrantRevoked);
    }
    let cap_cents = i64::from(cap_cents);

    sqlx::query("INSERT INTO oauth_grant_spend (grant_id, month) VALUES ($1, $2) ON CONFLICT (grant_id, month) DO NOTHING")
        .bind(grant_id)
        .bind(&month)
        .execute(&mut *tx)
        .await?;
    let (spent_cents, reserved_cents): (i64, i64) = sqlx::query_as(
        "SELECT spent_cents, reserved_cents FROM oauth_grant_spend WHERE grant_id = $1 AND month = $2 FOR UPDATE",
    )
    .bind(grant_id)
    .bind(&month)
    .fetch_one(&mut *tx)
    .await?;

    if spent_cents + reserved_cents + est_cents > cap_cents {
        tx.rollback().await?;
        let ctx = AuditCtx { actor: user_id.to_string(), ..Default::default() };
        let detail = json!({
            "month": month,
            "cap_cents": cap_cents,
            "spent_cents": spent_cents,
            "reserved_cents": reserved_cents,
            "estimate_cents": est_cents,
        });
        if let Err(error) =
            oauth_apps::record_audit(db, &ctx, Some(app_id), Some(grant_id), AuditEvent::CapReached, detail).await
        {
            warn!(%error, %grant_id, "oauth: cap_reached audit row not written");
        }
        return Err(SpendError::CapReached {
            cap_cents,
            spent_cents,
            reserved_cents,
            estimate_cents: est_cents,
            resets_at: month_resets_at(now),
        });
    }

    let balance: Option<f64> =
        sqlx::query_scalar("SELECT COALESCE(balance_usd, 0)::float8 FROM user_credits WHERE user_id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    // Every hold the person's grants carry, this one included, in any month.
    let held_cents: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(s.reserved_cents), 0)::bigint
         FROM oauth_grant_spend s JOIN oauth_grants g ON g.id = s.grant_id
         WHERE g.user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    let balance_cents = balance.map(|usd| (usd * 100.0 + 1e-9).floor() as i64).unwrap_or(0);
    let available_cents = balance_cents - held_cents;
    if available_cents < est_cents {
        return Err(SpendError::CreditsExhausted { available_cents: available_cents.max(0), estimate_cents: est_cents });
    }

    sqlx::query(
        "UPDATE oauth_grant_spend SET reserved_cents = reserved_cents + $3, updated_at = now() WHERE grant_id = $1 AND month = $2",
    )
    .bind(grant_id)
    .bind(&month)
    .bind(est_cents)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Reservation { id: Uuid::new_v4(), grant_id, user_id: user_id.to_string(), month, reserved_cents: est_cents })
}

// ─── settle ───────────────────────────────────────────────────────────────────

/// What settlement took.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settled {
    /// Debited from the wallet, in USD: `min(actual, held)`, never more than the wallet held.
    pub charged_usd: f64,
    /// Added to `spent_cents`.
    pub spent_cents: i64,
}

/// The wallet ledger key of one call: `oauth:<grant_id>:<request_id>`.
pub fn ledger_ref(grant_id: Uuid, request_id: &str) -> String {
    format!("oauth:{grant_id}:{request_id}")
}

/// Release `reservation` and charge `min(actual_usd, held)` (module doc, step
/// 3). `actual_usd = 0` is a full release with no charge. `ledger_ref` keys the
/// wallet debit, so a replay cannot charge twice.
pub async fn settle(db: &PgPool, reservation: &Reservation, actual_usd: f64, ledger_ref: &str) -> Result<Settled, ApiError> {
    let mut tx = db.begin().await?;
    // Same lock order as `reserve`: the spend row, then the wallet.
    sqlx::query("SELECT reserved_cents FROM oauth_grant_spend WHERE grant_id = $1 AND month = $2 FOR UPDATE")
        .bind(reservation.grant_id)
        .bind(&reservation.month)
        .fetch_one(&mut *tx)
        .await?;
    let wallet: Option<f64> =
        sqlx::query_scalar("SELECT COALESCE(balance_usd, 0)::float8 FROM user_credits WHERE user_id = $1 FOR UPDATE")
            .bind(&reservation.user_id)
            .fetch_optional(&mut *tx)
            .await?;

    let held_usd = reservation.reserved_cents as f64 / 100.0;
    let mut charge = if actual_usd.is_finite() { actual_usd.clamp(0.0, held_usd) } else { 0.0 };
    let available = wallet.unwrap_or(0.0).max(0.0);
    if charge > available {
        // Only possible when the person's own (non-app) spending drew the
        // wallet down under the hold mid-call. Take what is there; never
        // go negative.
        error!(
            "REVENUE-CRITICAL: oauth settle short for user {} (grant {}, {}): ${:.6} due, ${:.6} in the wallet",
            reservation.user_id, reservation.grant_id, ledger_ref, charge, available
        );
        charge = available;
    }
    if charge > 0.0 {
        cost_service::debit_wallet_strict(&mut *tx, &reservation.user_id, ledger_ref, charge, WALLET_SOURCE).await?;
    }
    let spent_cents = usd_to_cents_ceil(charge).min(reservation.reserved_cents);
    sqlx::query(
        "UPDATE oauth_grant_spend
         SET reserved_cents = reserved_cents - $3, spent_cents = spent_cents + $4, updated_at = now()
         WHERE grant_id = $1 AND month = $2",
    )
    .bind(reservation.grant_id)
    .bind(&reservation.month)
    .bind(reservation.reserved_cents)
    .bind(spent_cents)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Settled { charged_usd: charge, spent_cents })
}

/// [`settle`] that never fails its caller: a failed charge is logged
/// REVENUE-CRITICAL and the hold is still released, so a settlement error can
/// never leave the cap blocked.
async fn settle_or_release(db: &PgPool, reservation: &Reservation, actual_usd: f64, ledger_ref: &str) -> Settled {
    match settle(db, reservation, actual_usd, ledger_ref).await {
        Ok(settled) => settled,
        Err(error) => {
            error!(
                "REVENUE-CRITICAL: oauth settle failed for user {} (grant {}, {}, ${:.6}): {}",
                reservation.user_id, reservation.grant_id, ledger_ref, actual_usd, error
            );
            if actual_usd > 0.0 {
                if let Err(error) = settle(db, reservation, 0.0, ledger_ref).await {
                    error!(
                        "oauth: reservation of {} cents on grant {} ({}) left held: {}",
                        reservation.reserved_cents, reservation.grant_id, reservation.month, error
                    );
                }
            }
            Settled { charged_usd: 0.0, spent_cents: 0 }
        }
    }
}

/// Owns one [`Reservation`] until it is settled. Dropped unsettled (an early
/// return, an error, a panic, a dropped stream task), it spawns a full release.
pub struct ReservationGuard {
    db: PgPool,
    reservation: Option<Reservation>,
    ledger_ref: String,
}

impl ReservationGuard {
    pub fn new(db: PgPool, reservation: Reservation, ledger_ref: String) -> Self {
        Self { db, reservation: Some(reservation), ledger_ref }
    }

    /// Charge `min(actual_usd, held)` and release the hold.
    pub async fn settle(mut self, actual_usd: f64) -> Settled {
        match self.reservation.take() {
            Some(reservation) => settle_or_release(&self.db, &reservation, actual_usd, &self.ledger_ref).await,
            None => Settled { charged_usd: 0.0, spent_cents: 0 },
        }
    }

    /// Release the hold with no charge (an upstream error or a non-2xx answer).
    pub async fn release(self) {
        self.settle(0.0).await;
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        let Some(reservation) = self.reservation.take() else { return };
        let db = self.db.clone();
        let ledger_ref = std::mem::take(&mut self.ledger_ref);
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    settle_or_release(&db, &reservation, 0.0, &ledger_ref).await;
                });
            }
            Err(_) => error!(
                "oauth: no runtime to release {} cents on grant {} ({}); the hold stays",
                reservation.reserved_cents, reservation.grant_id, reservation.month
            ),
        }
    }
}

// ─── the call's usage row and response metering ──────────────────────────────

/// `inference_usage.client` for an app's call: `oauth_app:<client_id>`, so the
/// person's usage report groups spend by app.
pub fn usage_client(client_id: &str) -> String {
    format!("oauth_app:{client_id}")
}

/// The usage row's attribution for an app's call: no project and no key (the
/// app's project does not pay), the app as the client, the request id, and
/// the payer columns of migrations_pg/151 (`billed_to = user_wallet`, the app
/// and the grant).
pub fn attribution(
    client_id: &str,
    app_id: Uuid,
    grant_id: Uuid,
    request_id: String,
    started_at: std::time::Instant,
) -> InferenceAttribution {
    InferenceAttribution {
        project_id: None,
        key_id: None,
        client: Some(usage_client(client_id)),
        request_id: Some(request_id),
        started_at: Some(started_at),
        capture_request: None,
        person_turn: false,
        billed_to: Some(inference_settlement::BILLED_TO_USER_WALLET),
        oauth_app_id: Some(app_id),
        oauth_grant_id: Some(grant_id),
    }
}

/// Everything a finished call needs to settle and write its usage row.
pub struct CallSettlement {
    pub guard: ReservationGuard,
    pub db: PgPool,
    /// The person (wallet owner).
    pub user_id: String,
    /// The alias the call ran on.
    pub model: String,
    pub pool_id: Option<String>,
    pub prices: RetailPrices,
    /// The reservation's input tokens: what a call with no usage report pays for input.
    pub input_tokens_estimate: u64,
    pub attribution: InferenceAttribution,
}

impl CallSettlement {
    /// Settle the money, then write the usage row (no second deduction).
    async fn finish(self, prompt_tokens: u64, completion_tokens: u64, estimated: bool) -> Settled {
        let actual = cost_usd(&self.prices, prompt_tokens, completion_tokens);
        let settled = self.guard.settle(actual).await;
        inference_settlement::settle_inference(
            &self.db,
            &self.user_id,
            &self.model,
            self.pool_id.as_deref(),
            false,
            &self.prices,
            prompt_tokens,
            completion_tokens,
            estimated,
            &self.attribution,
        )
        .await;
        settled
    }
}

/// Settle a non-streaming upstream answer and pass it on unchanged. A non-2xx
/// answer or an unreadable body releases the hold with no charge. A 2xx answer
/// without `usage` pays the input estimate only (as `meter_json_response`).
pub async fn settle_json_response(response: Response, call: CallSettlement) -> Result<Response, ApiError> {
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BUFFERED_JSON_BODY).await {
        Ok(bytes) => bytes,
        Err(error) => {
            call.guard.release().await;
            return Err(ApiError::Internal(format!("Failed to buffer the upstream response: {error}")));
        }
    };
    if !parts.status.is_success() {
        call.guard.release().await;
        return Ok(Response::from_parts(parts, Body::from(bytes)));
    }
    let usage = serde_json::from_slice::<Value>(&bytes).ok().and_then(|value| {
        let prompt = value["usage"]["prompt_tokens"].as_u64();
        let completion = value["usage"]["completion_tokens"].as_u64();
        (prompt.is_some() || completion.is_some()).then(|| (prompt.unwrap_or(0), completion.unwrap_or(0)))
    });
    let (prompt, completion, estimated) = match usage {
        Some((prompt, completion)) => (prompt, completion, false),
        None => (call.input_tokens_estimate, 0, true),
    };
    call.finish(prompt, completion, estimated).await;
    Ok(Response::from_parts(parts, Body::from(bytes)))
}

/// Wrap a 2xx streaming answer: every chunk passes through untouched, and the
/// call settles once, at the end of the stream or when the body is dropped.
pub fn meter_stream(response: Response, call: CallSettlement) -> Response {
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        Body::new(OAuthStreamBody { inner: Box::pin(body), scanner: SseUsage::default(), call: Some(call) }),
    )
}

/// The streaming adapter behind [`meter_stream`].
struct OAuthStreamBody {
    inner: Pin<Box<Body>>,
    scanner: SseUsage,
    call: Option<CallSettlement>,
}

impl OAuthStreamBody {
    /// Fire settlement once. The upstream's usage when it arrived; else the
    /// input estimate plus the output already streamed (module doc, Streams).
    fn finish(&mut self) {
        let Some(call) = self.call.take() else { return };
        let (prompt, completion, estimated) = match self.scanner.usage {
            Some((prompt, completion)) => (prompt, completion, false),
            None => (call.input_tokens_estimate, (self.scanner.completion_chars / 4) as u64, true),
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    call.finish(prompt, completion, estimated).await;
                });
            }
            // Dropping `call` drops its guard, which logs the held reservation.
            Err(_) => drop(call),
        }
    }
}

impl HttpBody for OAuthStreamBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_frame(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.scanner.push(data);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish();
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                this.finish();
                Poll::Ready(None)
            }
        }
    }
}

impl Drop for OAuthStreamBody {
    fn drop(&mut self) {
        // Client abort or disconnect: settle with what streamed so far.
        self.finish();
    }
}

/// SSE scan for the final `usage` object and the streamed text length
/// (the same rules as `inference_settlement`'s scanner).
#[derive(Default)]
struct SseUsage {
    buf: Vec<u8>,
    usage: Option<(u64, u64)>,
    completion_chars: usize,
}

impl SseUsage {
    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.scan_line(&line);
        }
    }

    fn scan_line(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else { return };
        let Some(payload) = line.trim().strip_prefix("data:") else { return };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(payload) else { return };
        let usage = &value["usage"];
        if usage.is_object() {
            let prompt = usage["prompt_tokens"].as_u64();
            let completion = usage["completion_tokens"].as_u64();
            if prompt.is_some() || completion.is_some() {
                self.usage = Some((prompt.unwrap_or(0), completion.unwrap_or(0)));
            }
        }
        if let Some(choices) = value["choices"].as_array() {
            for choice in choices {
                if let Some(text) = choice["delta"]["content"].as_str() {
                    self.completion_chars += text.chars().count();
                }
            }
        }
    }
}

/// Real Postgres (schema-per-test), the real `auth_middleware` and chat route,
/// a fake upstream model. Assertions are on HTTP status and the stored rows:
/// `user_credits`, `credit_transactions`, `oauth_grant_spend`, `inference_usage`.
#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use axum::{http::Request, Router};
    use chrono::{Datelike, TimeZone};
    use futures::StreamExt;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::model_router::{catalog::starter_catalog, ModelInfo, ModelRouter, ModelRouterError, UpstreamProvider};
    use crate::routes::platform_v1::{project_billing, projects, ProjectEnv};
    use crate::routes::test_support::{platform_schema, test_state, MockGateway};
    use crate::services::api_keys::{self, hash_token, CreateProjectKeyInput};
    use crate::services::oauth_apps::{ClientType, CreateOAuthAppInput};
    use crate::ApiState;

    const OWNER: &str = "user_app_owner";
    const PERSON: &str = "user_signed_in";

    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    enum Mode {
        /// 1,000 input and 500 output tokens, JSON or a full stream.
        #[default]
        Ok,
        /// The provider call fails (`UpstreamError` 500).
        Fail,
        /// The provider answers 503.
        Status503,
        /// A stream that sends one chunk of text and then never ends.
        StreamThenHang,
    }

    /// `gpt-4o` (upstream `openai/gpt-4o` on `openrouter`) at a wholesale $10 /
    /// $30 per 1M tokens. Counts calls and records the `max_tokens` it was sent.
    #[derive(Default)]
    struct FakeUpstream {
        calls: AtomicUsize,
        delay_ms: AtomicU64,
        mode: Mutex<Mode>,
        max_tokens_seen: Mutex<Vec<Option<u32>>>,
    }

    impl FakeUpstream {
        fn set_mode(&self, mode: Mode) {
            *self.mode.lock().unwrap() = mode;
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl UpstreamProvider for FakeUpstream {
        fn provider_id(&self) -> &str {
            "openrouter"
        }
        async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelRouterError> {
            let mut extra = serde_json::Map::new();
            extra.insert("prompt_price".into(), json!(0.00001));
            extra.insert("completion_price".into(), json!(0.00003));
            Ok(vec![ModelInfo {
                id: "openai/gpt-4o".into(),
                object: "model".into(),
                created: 0,
                owned_by: "openai".into(),
                upstream_id: None,
                provider: Some("openrouter".into()),
                aliases: None,
                extra,
            }])
        }
        async fn chat_completions(&self, request: ChatCompletionRequest) -> Result<axum::http::Response<Body>, ModelRouterError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.max_tokens_seen.lock().unwrap().push(request.max_tokens);
            let delay = self.delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            let mode = *self.mode.lock().unwrap();
            let usage = json!({ "prompt_tokens": 1000, "completion_tokens": 500 });
            let build = |status: u16, body: Body| {
                axum::http::Response::builder().status(status).body(body).map_err(|e| ModelRouterError::Internal(e.to_string()))
            };
            match mode {
                Mode::Fail => Err(ModelRouterError::UpstreamError { status: 500, message: "upstream fell over".into() }),
                Mode::Status503 => build(503, Body::from(json!({ "error": { "message": "overloaded" } }).to_string())),
                Mode::StreamThenHang => {
                    let first = Bytes::from(format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "hello world" } }] })));
                    let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(futures::stream::pending());
                    build(200, Body::from_stream(stream))
                }
                Mode::Ok if request.stream == Some(true) => {
                    let mut sse = format!("data: {}\n\n", json!({ "choices": [{ "delta": { "content": "hello" } }] }));
                    sse.push_str(&format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }], "usage": usage })
                    ));
                    build(200, Body::from(sse))
                }
                Mode::Ok => build(
                    200,
                    Body::from(
                        json!({ "choices": [{ "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }], "usage": usage })
                            .to_string(),
                    ),
                ),
            }
        }
    }

    struct Ctx {
        state: Arc<ApiState>,
        db: PgPool,
        upstream: Arc<FakeUpstream>,
        /// Production's shape: `auth_middleware` in front of the real chat handler.
        gateway: Router,
        app_id: Uuid,
        client_id: String,
    }

    /// A pool on the test state's schema with room for parallel requests
    /// (`test_state`'s pool has one connection, which would serialize them).
    async fn wide_pool(schema: &str) -> PgPool {
        let url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string());
        let schema = schema.to_string();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(6)
            .after_connect(move |conn, _meta| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path TO {schema}")).execute(&mut *conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap()
    }

    async fn ctx() -> Ctx {
        let base = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let schema: String = sqlx::query_scalar("SELECT current_schema()::text").fetch_one(&base.db).await.unwrap();
        let mut state = Arc::try_unwrap(base).unwrap_or_else(|_| panic!("fresh test state is not shared"));
        let db = wide_pool(&schema).await;
        state.db = db.clone();
        state.inference_pool_service = Arc::new(crate::services::InferencePoolService::new(db.clone()));
        let upstream = Arc::new(FakeUpstream::default());
        state.model_router = ModelRouter::new(vec![upstream.clone() as Arc<dyn UpstreamProvider>], starter_catalog());
        let state = Arc::new(state);

        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT)").execute(&db).await.unwrap();
        platform_schema(&db).await;
        for sql in [
            include_str!("../../migrations_pg/007_inference_usage.sql"),
            include_str!("../../migrations_pg/009_inference_pools.sql"),
            include_str!("../../migrations_pg/125_inference_attribution.sql"),
            include_str!("../../migrations_pg/150_oauth_apps.sql"),
            include_str!("../../migrations_pg/151_anthropic_links.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&db).await.expect("migration applies");
        }
        // The wallet tables (migrations_pg/001 + 002) and auto top-up (103), with
        // no CHECK on the balance, so a negative balance would show.
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS user_credits (user_id TEXT PRIMARY KEY, balance_usd DOUBLE PRECISION DEFAULT 0,
                 created_at TIMESTAMPTZ DEFAULT now(), updated_at TIMESTAMPTZ DEFAULT now());
             CREATE TABLE IF NOT EXISTS credit_transactions (id TEXT PRIMARY KEY, user_id TEXT, amount_usd DOUBLE PRECISION,
                 transaction_id TEXT UNIQUE, source TEXT, created_at TIMESTAMPTZ DEFAULT now());
             CREATE TABLE IF NOT EXISTS billing_auto_topup (user_id TEXT PRIMARY KEY, enabled BOOLEAN NOT NULL DEFAULT false,
                 threshold_usd NUMERIC(12,2) NOT NULL DEFAULT 5, amount_usd NUMERIC(12,2) NOT NULL DEFAULT 25,
                 monthly_cap_usd NUMERIC(12,2) NOT NULL DEFAULT 200, stripe_customer_id TEXT, stripe_payment_method_id TEXT,
                 card_brand TEXT, card_last4 TEXT, status TEXT NOT NULL DEFAULT 'ok', last_attempt_at TIMESTAMPTZ,
                 last_error TEXT, month_spent_usd NUMERIC(12,2) NOT NULL DEFAULT 0, month_key TEXT NOT NULL DEFAULT '',
                 updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
             CREATE TABLE IF NOT EXISTS billing_auto_topup_attempts (id TEXT PRIMARY KEY, user_id TEXT NOT NULL,
                 payment_intent_id TEXT UNIQUE, amount_usd NUMERIC(12,2) NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
                 error TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now());",
        )
        .execute(&db)
        .await
        .unwrap();
        for (id, email) in [(OWNER, "owner@example.com"), (PERSON, "person@example.com")] {
            sqlx::query("INSERT INTO users (id, email) VALUES ($1, $2)").bind(id).bind(email).execute(&db).await.unwrap();
        }

        let owner = projects::Principal { user_id: OWNER.to_string(), org_id: None, org_admin: false };
        let project = projects::create_project(&db, &owner, "Notes", ProjectEnv::Sandbox).await.unwrap();
        let audit = AuditCtx { actor: OWNER.to_string(), ..Default::default() };
        let app = oauth_apps::create_app(
            &db,
            CreateOAuthAppInput {
                project_id: project.id.clone(),
                owner_user_id: OWNER.to_string(),
                name: "Notes Sync".to_string(),
                client_type: ClientType::Public,
                logo_url: None,
                homepage_url: Some("https://notes.example.com".to_string()),
                privacy_url: None,
                scopes: ["inference", "usage"].iter().map(|s| s.to_string()).collect(),
                redirect_uris: vec!["https://notes.example.com/cb".to_string()],
            },
            &audit,
        )
        .await
        .unwrap();
        oauth_apps::add_test_user(&db, &project.id, app.app.id, "person@example.com", &audit).await.unwrap();

        let gateway = Router::new()
            .route("/v1/chat/completions", axum::routing::post(crate::routes::model_router::chat_completions))
            .layer(axum::middleware::from_fn_with_state(state.clone(), crate::auth::middleware::auth_middleware))
            .with_state(state.clone());
        Ctx { state, db, upstream, gateway, app_id: app.app.id, client_id: app.app.client_id.clone() }
    }

    /// A live grant for the person with `cap_cents` and one access token.
    async fn grant(c: &Ctx, cap_cents: i32) -> (Uuid, String) {
        let grant_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO oauth_grants (id, app_id, user_id, scopes, monthly_cap_cents, sub) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(grant_id)
        .bind(c.app_id)
        .bind(PERSON)
        .bind(vec!["inference".to_string(), "usage".to_string()])
        .bind(cap_cents)
        .bind(format!("usr_{}", Uuid::new_v4().simple()))
        .execute(&c.db)
        .await
        .unwrap();
        let token = format!("alt_oat_{}", hex::encode(rand::random::<[u8; 32]>()));
        sqlx::query(
            "INSERT INTO oauth_tokens (id, grant_id, kind, token_hash, prefix, expires_at)
             VALUES ($1, $2, 'access', $3, $4, now() + interval '1 hour')",
        )
        .bind(Uuid::new_v4())
        .bind(grant_id)
        .bind(hash_token(&token))
        .bind(&token[..16])
        .execute(&c.db)
        .await
        .unwrap();
        (grant_id, token)
    }

    async fn set_wallet(c: &Ctx, user: &str, usd: f64) {
        sqlx::query(
            "INSERT INTO user_credits (user_id, balance_usd) VALUES ($1, $2)
             ON CONFLICT (user_id) DO UPDATE SET balance_usd = EXCLUDED.balance_usd",
        )
        .bind(user)
        .bind(usd)
        .execute(&c.db)
        .await
        .unwrap();
    }

    async fn balance(c: &Ctx, user: &str) -> Option<f64> {
        sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = $1")
            .bind(user)
            .fetch_optional(&c.db)
            .await
            .unwrap()
    }

    /// `(month, spent_cents, reserved_cents)` rows of a grant, by month.
    async fn spend(c: &Ctx, grant_id: Uuid) -> Vec<(String, i64, i64)> {
        sqlx::query_as("SELECT month, spent_cents, reserved_cents FROM oauth_grant_spend WHERE grant_id = $1 ORDER BY month")
            .bind(grant_id)
            .fetch_all(&c.db)
            .await
            .unwrap()
    }

    /// `(amount_usd, source)` wallet ledger rows of a user.
    async fn ledger(c: &Ctx, user: &str) -> Vec<(f64, String)> {
        sqlx::query_as("SELECT amount_usd, source FROM credit_transactions WHERE user_id = $1 ORDER BY created_at")
            .bind(user)
            .fetch_all(&c.db)
            .await
            .unwrap()
    }

    fn chat_request(token: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn chat(c: &Ctx, token: &str, body: &Value) -> (StatusCode, Value) {
        let response = c.gateway.clone().oneshot(chat_request(token, body)).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::String(text)))
    }

    fn body(max_tokens: Option<u32>, stream: bool) -> Value {
        let mut body = json!({ "model": "gpt-4o", "stream": stream, "messages": [{ "role": "user", "content": "hi" }] });
        if let Some(max_tokens) = max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        body
    }

    async fn prices(c: &Ctx) -> RetailPrices {
        c.state.model_router.retail_prices("gpt-4o").await.unwrap()
    }

    /// The hold the route takes for `body` (same functions, same prices).
    fn hold_for(prices: &RetailPrices, body: &Value) -> CostEstimate {
        let mut request: ChatCompletionRequest = serde_json::from_value(body.clone()).unwrap();
        let output = bound_output(&mut request);
        estimate(prices, input_token_estimate(&request), output)
    }

    /// Wait (up to 5 s) for spawned settlement to land.
    async fn eventually<F, Fut>(what: &str, mut check: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        for _ in 0..200 {
            if check().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for {what}");
    }

    async fn wait_released(c: &Ctx, grant_id: Uuid) {
        eventually("the hold to be released", move || async move {
            spend(c, grant_id).await.iter().all(|(_, _, reserved)| *reserved == 0)
        })
        .await;
    }

    async fn usage_rows(c: &Ctx) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM inference_usage WHERE user_id = $1").bind(PERSON).fetch_one(&c.db).await.unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[tokio::test]
    async fn oauth_over_cap_402_zero_upstream_calls() {
        let c = ctx().await;
        set_wallet(&c, PERSON, 5.0).await;
        let request = body(Some(10_000), false);
        let hold = hold_for(&prices(&c).await, &request);
        assert!(hold.cents > 10, "the test needs a hold over the cap: {hold:?}");
        let (grant_id, token) = grant(&c, 10).await;

        let (status, reply) = chat(&c, &token, &request).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{reply}");
        assert_eq!(reply["error"]["code"], APP_SPEND_CAP_REACHED, "{reply}");
        assert_eq!(reply["error"]["cap_cents"], 10);
        assert_eq!(c.upstream.calls(), 0, "refused before any upstream call");
        assert_eq!(balance(&c, PERSON).await, Some(5.0));
        // The refused reservation rolled back: nothing held, nothing spent.
        assert!(spend(&c, grant_id).await.is_empty());
        assert!(ledger(&c, PERSON).await.is_empty());
        let audited: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_audit_log WHERE grant_id = $1 AND event = 'cap_reached'")
            .bind(grant_id)
            .fetch_one(&c.db)
            .await
            .unwrap();
        assert_eq!(audited, 1);
    }

    #[tokio::test]
    async fn oauth_insufficient_credits_402_no_negative_balance() {
        let c = ctx().await;
        let request = body(Some(10_000), false);
        let hold = hold_for(&prices(&c).await, &request);
        let (grant_id, token) = grant(&c, 5_000).await;

        // A wallet under the hold.
        let short = (hold.cents - 1) as f64 / 100.0;
        set_wallet(&c, PERSON, short).await;
        let (status, reply) = chat(&c, &token, &request).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{reply}");
        assert_eq!(reply["error"]["code"], USER_CREDITS_EXHAUSTED, "{reply}");

        // Enough for one hold, but another grant of the same person already holds it.
        let (other_grant, _) = grant(&c, 5_000).await;
        set_wallet(&c, PERSON, (hold.cents + hold.cents / 2) as f64 / 100.0).await;
        let held = reserve(&c.db, other_grant, PERSON, hold.cents).await.unwrap();
        let (status, reply) = chat(&c, &token, &request).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::PAYMENT_REQUIRED, Some(USER_CREDITS_EXHAUSTED)), "{reply}");
        settle(&c.db, &held, 0.0, "release-other").await.unwrap();

        // No wallet at all.
        sqlx::query("DELETE FROM user_credits WHERE user_id = $1").bind(PERSON).execute(&c.db).await.unwrap();
        let (status, reply) = chat(&c, &token, &request).await;
        assert_eq!((status, reply["error"]["code"].as_str()), (StatusCode::PAYMENT_REQUIRED, Some(USER_CREDITS_EXHAUSTED)), "{reply}");

        assert_eq!(c.upstream.calls(), 0, "refused before any upstream call");
        assert_eq!(balance(&c, PERSON).await, None, "no wallet row was created");
        let negative: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_credits WHERE balance_usd < 0").fetch_one(&c.db).await.unwrap();
        assert_eq!(negative, 0);
        assert!(ledger(&c, PERSON).await.is_empty());
        assert!(spend(&c, grant_id).await.is_empty(), "refused reservations roll back");
        assert_eq!(spend(&c, other_grant).await, vec![(spend_month(Utc::now()), 0, 0)]);
    }

    #[tokio::test]
    async fn oauth_success_debits_wallet_and_spend() {
        let c = ctx().await;
        set_wallet(&c, PERSON, 5.0).await;
        let (grant_id, token) = grant(&c, 5_000).await;
        let prices = prices(&c).await;
        let actual = cost_usd(&prices, 1000, 500);
        let per_call_cents = usd_to_cents_ceil(actual);

        // JSON, with the app's own max_tokens.
        let (status, reply) = chat(&c, &token, &body(Some(10_000), false)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["choices"][0]["message"]["content"], "hello");
        // JSON without max_tokens: the default bound is sent upstream.
        let (status, reply) = chat(&c, &token, &body(None, false)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(*c.upstream.max_tokens_seen.lock().unwrap(), vec![Some(10_000), Some(DEFAULT_MAX_TOKENS)]);
        // A full stream settles when it ends.
        let (status, _) = chat(&c, &token, &body(Some(10_000), true)).await;
        assert_eq!(status, StatusCode::OK);
        let cr = &c;
        eventually("three usage rows", move || async move { usage_rows(cr).await == 3 }).await;
        wait_released(&c, grant_id).await;

        let wallet = balance(&c, PERSON).await.unwrap();
        assert!(close(wallet, 5.0 - 3.0 * actual), "wallet {wallet}, expected {}", 5.0 - 3.0 * actual);
        assert_eq!(spend(&c, grant_id).await, vec![(spend_month(Utc::now()), 3 * per_call_cents, 0)]);
        let ledger = ledger(&c, PERSON).await;
        assert_eq!(ledger.len(), 3);
        assert!(ledger.iter().all(|(amount, source)| close(*amount, -actual) && source == WALLET_SOURCE), "{ledger:?}");
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, Option<String>, Option<String>, f64, bool, Option<String>, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT model, client, project_id, cost_usd, estimated, billed_to, oauth_app_id, oauth_grant_id \
             FROM inference_usage WHERE user_id = $1",
        )
        .bind(PERSON)
        .fetch_all(&c.db)
        .await
        .unwrap();
        for (model, client, project_id, cost, estimated, billed_to, app_id, row_grant) in rows {
            assert_eq!(model, "gpt-4o");
            assert_eq!(client, Some(usage_client(&c.client_id)));
            assert_eq!(project_id, None, "the app's project does not pay");
            assert!(close(cost, actual) && !estimated);
            assert_eq!(billed_to.as_deref(), Some("user_wallet"));
            assert_eq!((app_id, row_grant), (Some(c.app_id), Some(grant_id)), "the row names the app and the grant");
        }
        let ledger_keys: Vec<String> =
            sqlx::query_scalar("SELECT transaction_id FROM credit_transactions WHERE user_id = $1").bind(PERSON).fetch_all(&c.db).await.unwrap();
        assert!(ledger_keys.iter().all(|key| key.starts_with(&format!("oauth:{grant_id}:req_"))), "{ledger_keys:?}");
    }

    #[tokio::test]
    async fn oauth_upstream_error_releases_reservation_no_charge() {
        let c = ctx().await;
        set_wallet(&c, PERSON, 5.0).await;
        let (grant_id, token) = grant(&c, 5_000).await;

        c.upstream.set_mode(Mode::Fail);
        let (status, reply) = chat(&c, &token, &body(Some(10_000), false)).await;
        assert!(status.is_server_error(), "{status} {reply}");
        c.upstream.set_mode(Mode::Status503);
        let (status, reply) = chat(&c, &token, &body(Some(10_000), true)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{reply}");
        let (status, reply) = chat(&c, &token, &body(Some(10_000), false)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{reply}");

        assert_eq!(c.upstream.calls(), 3);
        wait_released(&c, grant_id).await;
        assert_eq!(spend(&c, grant_id).await, vec![(spend_month(Utc::now()), 0, 0)]);
        assert_eq!(balance(&c, PERSON).await, Some(5.0));
        assert!(ledger(&c, PERSON).await.is_empty());
        assert_eq!(usage_rows(&c).await, 0);
    }

    #[tokio::test]
    async fn oauth_never_triggers_auto_topup() {
        let c = ctx().await;
        // Auto top-up is live on this "deploy", and the person has it on with a
        // card and a threshold above any balance this test reaches. A debit
        // through `deduct_credits_for_usage` would claim a pending attempt here.
        // The Stripe key is ALWAYS an obviously fake value here, even when a real
        // one is in the environment, so a regression can never charge a real
        // card. The original values are restored when the test ends.
        struct EnvRestore(Vec<(&'static str, Option<String>)>);
        impl Drop for EnvRestore {
            fn drop(&mut self) {
                for (k, v) in self.0.drain(..) {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
        let _restore = EnvRestore(vec![
            ("AUTO_TOPUP_LIVE", std::env::var("AUTO_TOPUP_LIVE").ok()),
            ("STRIPE_SECRET_KEY", std::env::var("STRIPE_SECRET_KEY").ok()),
        ]);
        std::env::set_var("AUTO_TOPUP_LIVE", "1");
        std::env::set_var("STRIPE_SECRET_KEY", "sk_test_FAKE_oauth_spend_never_used");
        sqlx::query(
            "INSERT INTO billing_auto_topup (user_id, enabled, threshold_usd, amount_usd, monthly_cap_usd, stripe_customer_id, stripe_payment_method_id, status)
             VALUES ($1, true, 100, 25, 200, 'cus_test', 'pm_test', 'ok')",
        )
        .bind(PERSON)
        .execute(&c.db)
        .await
        .unwrap();
        set_wallet(&c, PERSON, 5.0).await;
        let (grant_id, token) = grant(&c, 5_000).await;

        let (status, reply) = chat(&c, &token, &body(Some(10_000), false)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        let (status, _) = chat(&c, &token, &body(Some(10_000), true)).await;
        assert_eq!(status, StatusCode::OK);
        wait_released(&c, grant_id).await;
        // Give any spawned top-up check time to claim its attempt.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        assert!(balance(&c, PERSON).await.unwrap() < 5.0, "the calls were charged");
        let attempts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM billing_auto_topup_attempts").fetch_one(&c.db).await.unwrap();
        assert_eq!(attempts, 0, "an app's spend never starts a card charge");
        let status: String = sqlx::query_scalar("SELECT status FROM billing_auto_topup WHERE user_id = $1").bind(PERSON).fetch_one(&c.db).await.unwrap();
        assert_eq!(status, "ok");
    }

    #[tokio::test]
    async fn oauth_concurrent_requests_cannot_exceed_cap() {
        let c = ctx().await;
        set_wallet(&c, PERSON, 5.0).await;
        let request = body(Some(10_000), false);
        let hold = hold_for(&prices(&c).await, &request);
        // One hold fits; two at once do not.
        let (grant_id, token) = grant(&c, (hold.cents + hold.cents / 2) as i32).await;
        // Both calls are in flight together: the upstream answers after 300 ms.
        c.upstream.delay_ms.store(300, Ordering::SeqCst);

        let (a, b) = tokio::join!(chat(&c, &token, &request), chat(&c, &token, &request));
        let mut statuses = vec![a.0.as_u16(), b.0.as_u16()];
        statuses.sort_unstable();
        assert_eq!(statuses, vec![200, 402], "{} / {}", a.1, b.1);
        let refused = if a.0 == StatusCode::PAYMENT_REQUIRED { &a.1 } else { &b.1 };
        assert_eq!(refused["error"]["code"], APP_SPEND_CAP_REACHED, "{refused}");
        assert_eq!(c.upstream.calls(), 1);

        wait_released(&c, grant_id).await;
        let actual = cost_usd(&prices(&c).await, 1000, 500);
        assert_eq!(spend(&c, grant_id).await, vec![(spend_month(Utc::now()), usd_to_cents_ceil(actual), 0)]);
        assert!(close(balance(&c, PERSON).await.unwrap(), 5.0 - actual));
    }

    /// Choice documented in the module doc: no usage report arrived, so the
    /// call pays the reservation's input part plus the text already streamed.
    #[tokio::test]
    async fn oauth_stream_abort_settles() {
        let c = ctx().await;
        set_wallet(&c, PERSON, 5.0).await;
        let (grant_id, token) = grant(&c, 5_000).await;
        c.upstream.set_mode(Mode::StreamThenHang);
        let request = body(Some(10_000), true);
        let prices = prices(&c).await;
        let hold = hold_for(&prices, &request);

        let response = c.gateway.clone().oneshot(chat_request(&token, &request)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body();
        let frame = stream.frame().await.unwrap().unwrap();
        assert!(String::from_utf8_lossy(frame.data_ref().unwrap()).contains("hello world"));
        // While the stream is open the hold stands.
        assert_eq!(spend(&c, grant_id).await, vec![(spend_month(Utc::now()), 0, hold.cents)]);
        // The client goes away mid-stream.
        drop(stream);

        wait_released(&c, grant_id).await;
        let cr = &c;
        eventually("the usage row", move || async move { usage_rows(cr).await == 1 }).await;
        // "hello world" is 11 characters: 2 output tokens at 4 characters a token.
        let charged = cost_usd(&prices, hold.input_tokens, 2);
        assert!(charged < hold.cents as f64 / 100.0);
        let wallet = balance(&c, PERSON).await.unwrap();
        assert!(close(wallet, 5.0 - charged), "wallet {wallet}, expected {}", 5.0 - charged);
        assert_eq!(spend(&c, grant_id).await, vec![(spend_month(Utc::now()), usd_to_cents_ceil(charged), 0)]);
        let (prompt, completion, estimated): (i64, i64, bool) =
            sqlx::query_as("SELECT prompt_tokens, completion_tokens, estimated FROM inference_usage WHERE user_id = $1")
                .bind(PERSON)
                .fetch_one(&c.db)
                .await
                .unwrap();
        assert_eq!((prompt as u64, completion, estimated), (hold.input_tokens, 2, true));
    }

    #[tokio::test]
    async fn oauth_month_rollover_resets_spend() {
        let c = ctx().await;
        set_wallet(&c, PERSON, 5.0).await;
        let request = body(Some(10_000), false);
        let hold = hold_for(&prices(&c).await, &request);
        let cap = hold.cents as i32;

        // Last month used the whole cap; this month starts empty.
        let (grant_id, token) = grant(&c, cap).await;
        let now = Utc::now();
        let last_month = spend_month(now.with_day(1).unwrap() - chrono::Duration::days(1));
        sqlx::query("INSERT INTO oauth_grant_spend (grant_id, month, spent_cents) VALUES ($1, $2, $3)")
            .bind(grant_id)
            .bind(&last_month)
            .bind(i64::from(cap))
            .execute(&c.db)
            .await
            .unwrap();
        let (status, reply) = chat(&c, &token, &request).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        let actual = cost_usd(&prices(&c).await, 1000, 500);
        assert_eq!(
            spend(&c, grant_id).await,
            vec![(last_month, i64::from(cap), 0), (spend_month(now), usd_to_cents_ceil(actual), 0)]
        );

        // The window turns at 00:00 UTC on the 1st.
        let (edge_grant, _) = grant(&c, 50).await;
        let before = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        let after = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap();
        let full = reserve_at(&c.db, edge_grant, PERSON, 50, before).await.unwrap();
        settle(&c.db, &full, 0.50, "edge-december").await.unwrap();
        assert!(matches!(reserve_at(&c.db, edge_grant, PERSON, 1, before).await, Err(SpendError::CapReached { .. })));
        let january = reserve_at(&c.db, edge_grant, PERSON, 1, after).await.unwrap();
        assert_eq!(january.month, "2027-01");
        settle(&c.db, &january, 0.0, "edge-january").await.unwrap();
        assert_eq!(spend(&c, edge_grant).await, vec![("2026-12".to_string(), 50, 0), ("2027-01".to_string(), 0, 0)]);
    }

    /// Regression guard: a Platform project key on the chat route still bills
    /// the key owner's wallet through the old clamping debit (never refused
    /// after serving, clamped at 0, not negative), and no grant spend is touched.
    #[tokio::test]
    async fn project_key_billing_unchanged() {
        let c = ctx().await;
        let owner = projects::Principal { user_id: OWNER.to_string(), org_id: None, org_admin: false };
        let project = projects::create_project(&c.db, &owner, "Billing", ProjectEnv::Live).await.unwrap();
        project_billing::put_test_card(&c.db, &project.id).await;
        let token = api_keys::create_project_key(
            &c.db,
            CreateProjectKeyInput {
                user_id: OWNER.to_string(),
                organization_id: None,
                project_id: project.id.clone(),
                account_id: None,
                env: ProjectEnv::Live,
                name: "k".to_string(),
                scopes: vec!["inference".to_string()],
            },
        )
        .await
        .unwrap()
        .token;
        // Less than the call costs.
        set_wallet(&c, OWNER, 0.01).await;

        let (status, reply) = chat(&c, &token, &body(Some(10_000), false)).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        let actual = cost_usd(&prices(&c).await, 1000, 500);
        assert!(actual > 0.01);
        assert_eq!(balance(&c, OWNER).await, Some(0.0), "clamped at zero, as before");
        let ledger = ledger(&c, OWNER).await;
        assert_eq!(ledger.len(), 1);
        assert!(close(ledger[0].0, -actual) && ledger[0].1 == "inference", "{ledger:?}");
        let (project_id, client): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT project_id, client FROM inference_usage WHERE user_id = $1")
                .bind(OWNER)
                .fetch_one(&c.db)
                .await
                .unwrap();
        assert_eq!(project_id, Some(project.id.clone()));
        assert_ne!(client, Some(usage_client(&c.client_id)));
        let grant_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_grant_spend").fetch_one(&c.db).await.unwrap();
        assert_eq!(grant_rows, 0);
        assert_eq!(c.upstream.calls(), 1);
    }

    #[test]
    fn output_bound_is_written_back_and_multiplied_by_n() {
        let mut request: ChatCompletionRequest =
            serde_json::from_value(json!({ "model": "gpt-4o", "messages": [], "max_completion_tokens": 300, "n": 2 })).unwrap();
        assert_eq!(bound_output(&mut request), 600);
        assert_eq!(request.extra["max_completion_tokens"], 300);
        assert_eq!(request.max_tokens, None, "max_tokens is not added beside max_completion_tokens");
        let mut request: ChatCompletionRequest =
            serde_json::from_value(json!({ "model": "gpt-4o", "messages": [], "max_tokens": 900, "max_completion_tokens": 300 })).unwrap();
        assert_eq!(bound_output(&mut request), 300);
        assert_eq!((request.max_tokens, request.extra["max_completion_tokens"].as_u64()), (Some(300), Some(300)));
        assert_eq!(usd_to_cents_ceil(0.30), 30);
        assert_eq!(usd_to_cents_ceil(0.0001), 1);
        assert_eq!(usd_to_cents_ceil(0.0), 0);
    }
}
