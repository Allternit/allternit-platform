//! Inference usage settlement (Phase B metering).
//!
//! The model router proxies upstream providers (OpenRouter) with OUR key, so
//! every chat completion is a real cost. This module is the accounting half of
//! `routes::model_router`:
//!
//! - a pre-dispatch gate (`check_inference_allowed`): users with a
//!   `user_credits` row and no balance are blocked; users WITHOUT a row must
//!   have a verified email address (checked against Clerk and cached in
//!   `user_trust`, see `services::user_trust`) and then get a free monthly
//!   allowance (`FREE_INFERENCE_MONTHLY_USD`, default $2.00)
//!   measured from this month's `inference_usage` rows — which
//!   `settle_inference` writes for them even though no deduction happens;
//! - `settle_inference`: always writes an `inference_usage` row (the audit
//!   trail), then deducts the cost via `CostService::deduct_credits_for_usage`
//!   when the user has a credits row. Settlement failures are logged
//!   REVENUE-CRITICAL and swallowed — a metering hiccup must never fail the
//!   user's response;
//! - `meter_json_response`: non-streaming settlement (buffer, parse `usage`,
//!   settle, rebuild the response byte-for-byte);
//! - `UsageMeteringBody`: streaming settlement — an `http_body::Body` adapter
//!   that passes every chunk through untouched while scanning SSE `data:`
//!   payloads for the final `usage` object (injected via `stream_options`
//!   in `model_router::ModelRouter::chat_completions`), settling when the
//!   stream ends or is dropped.

use axum::{body::Bytes, response::Response};
use hyper::body::{Body as HttpBody, Frame};
use serde_json::Value;
use sqlx::PgPool;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tracing::{debug, error};
use uuid::Uuid;

use crate::{
    error::ApiError,
    model_router::RetailPrices,
    services::cost_service::{CostService, CostServiceImpl},
};

/// Cap on buffered non-streaming upstream bodies (they are settled, then
/// rebuilt and forwarded unchanged).
const MAX_BUFFERED_JSON_BODY: usize = 32 * 1024 * 1024;

/// Request header a client sends to name itself (`gizzi-code/<version>`).
pub const CLIENT_HEADER: &str = "x-allternit-client";

/// Who made a completion, written onto its `inference_usage` row
/// (migrations_pg/121). Every field is optional: internal callers that know
/// none of it pass `InferenceAttribution::default()`.
#[derive(Debug, Clone, Default)]
pub struct InferenceAttribution {
    /// Platform project the key belongs to.
    pub project_id: Option<String>,
    /// `api_keys.id` of the key that authenticated (None for device tokens,
    /// `allternit_*` tokens and sessions).
    pub key_id: Option<String>,
    /// `runtime`, `playground`, `gizzi-code` or `api` (see [`derive_client`]).
    pub client: Option<String>,
    /// The `x-request-id` the response carries.
    pub request_id: Option<String>,
    /// When the request started; settlement records the elapsed ms (for a
    /// stream, until the stream ended).
    pub started_at: Option<std::time::Instant>,
    /// The request body to keep beside the usage row for the console Logs
    /// page (Decision D6). Set only when `PLATFORM_LOG_BODIES=1` and the key's
    /// project has `log_bodies` on; see [`record_request_bodies`].
    pub capture_request: Option<Arc<Value>>,
}

/// Largest request + response pair kept for the Logs page, in JSON bytes.
/// A larger pair is not kept at all.
pub const MAX_CAPTURED_BODY_BYTES: usize = 1024 * 1024;

/// Server switch for body capture (console Decision D6). Off unless
/// `PLATFORM_LOG_BODIES=1`; a project must also opt in with `log_bodies`.
pub fn log_bodies_enabled() -> bool {
    std::env::var("PLATFORM_LOG_BODIES")
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

fn json_len(value: &Value) -> usize {
    serde_json::to_vec(value).map(|bytes| bytes.len()).unwrap_or(usize::MAX)
}

/// Keep a settled completion's request and response bodies for 30 days
/// (`inference_request_bodies`, migrations_pg/125). A no-op unless the
/// attribution carries a captured request, a request id and a project. The
/// pair must fit [`MAX_CAPTURED_BODY_BYTES`]. A second settlement under the
/// same request id (open-lane handoff) replaces the response. Failures are
/// logged and swallowed: capture never fails the user's response.
pub async fn record_request_bodies(
    db: &PgPool,
    attribution: &InferenceAttribution,
    response: Option<Value>,
) {
    let (Some(request), Some(request_id), Some(project_id)) = (
        attribution.capture_request.as_deref(),
        attribution.request_id.as_deref(),
        attribution.project_id.as_deref(),
    ) else {
        return;
    };
    let size = json_len(request).saturating_add(response.as_ref().map(json_len).unwrap_or(0));
    if size > MAX_CAPTURED_BODY_BYTES {
        debug!(request_id, size, "request bodies over the capture cap; not kept");
        return;
    }
    if let Err(error) = sqlx::query(
        "INSERT INTO inference_request_bodies (request_id, project_id, request, response, expires_at) \
         VALUES ($1, $2, $3, $4, NOW() + INTERVAL '30 days') \
         ON CONFLICT (request_id) DO UPDATE \
         SET response = EXCLUDED.response, expires_at = EXCLUDED.expires_at",
    )
    .bind(request_id)
    .bind(project_id)
    .bind(sqlx::types::Json(request))
    .bind(response.map(sqlx::types::Json))
    .execute(db)
    .await
    {
        error!(%error, request_id, "failed to keep request bodies");
    }
}

/// How often [`start_request_body_purge_task`] deletes expired bodies.
pub const BODY_PURGE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Delete kept bodies past their 30 days. Reads already skip them; this makes
/// "kept for 30 days" true in storage too. Returns the rows deleted.
pub async fn purge_expired_request_bodies(db: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM inference_request_bodies WHERE expires_at <= NOW()")
        .execute(db)
        .await?
        .rows_affected())
}

/// Hourly sweep for [`purge_expired_request_bodies`]; the first tick runs at startup.
pub fn start_request_body_purge_task(db: PgPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(BODY_PURGE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match purge_expired_request_bodies(&db).await {
                Ok(0) => {}
                Ok(deleted) => tracing::info!(deleted, "expired request bodies deleted"),
                // A schema without migration 125 has no table: nothing to purge.
                Err(error) => debug!(%error, "request body purge skipped"),
            }
        }
    });
}

/// Raw SSE bytes of a streamed response kept for body capture, up to the cap.
#[derive(Default)]
struct StreamCapture {
    bytes: Vec<u8>,
    overflowed: bool,
}

impl StreamCapture {
    fn push(&mut self, data: &[u8]) {
        if self.overflowed {
            return;
        }
        if self.bytes.len() + data.len() > MAX_CAPTURED_BODY_BYTES {
            self.overflowed = true;
            self.bytes = Vec::new();
            return;
        }
        self.bytes.extend_from_slice(data);
    }

    /// The stream as `{object: "chat.completion.stream", chunks: [...]}`, one
    /// entry per `data:` payload; `None` when it went over the cap.
    fn into_response(self) -> Option<Value> {
        if self.overflowed {
            return None;
        }
        let text = String::from_utf8_lossy(&self.bytes);
        let chunks: Vec<Value> = text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("data:"))
            .map(str::trim)
            .filter(|payload| !payload.is_empty() && *payload != "[DONE]")
            .filter_map(|payload| serde_json::from_str::<Value>(payload).ok())
            .collect();
        Some(serde_json::json!({ "object": "chat.completion.stream", "chunks": chunks }))
    }
}

impl InferenceAttribution {
    fn latency_ms(&self) -> Option<i64> {
        self.started_at
            .map(|started| i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX))
    }
}

/// The `client` a completion is recorded under (console Decision D3): a paired
/// computer's device token is `runtime`; a console playground key is
/// `playground`; `X-Allternit-Client: gizzi-code/<ver>` is `gizzi-code`;
/// anything else is `api`. Earlier rules win, so a header cannot relabel a
/// device or playground call.
pub fn derive_client(device_token: bool, key_kind: Option<&str>, client_header: Option<&str>) -> &'static str {
    if device_token {
        "runtime"
    } else if key_kind == Some(crate::services::api_keys::KEY_KIND_PLAYGROUND) {
        "playground"
    } else if client_header
        .map(|value| value.trim().to_ascii_lowercase().starts_with("gizzi-code"))
        .unwrap_or(false)
    {
        "gizzi-code"
    } else {
        "api"
    }
}

/// The user's credit balance if they have a `user_credits` row. The Some/None
/// distinction matters: None means free-allowance user (deduction no-ops,
/// pre-check enforces the monthly allowance), Some(<= 0) means a paying user
/// who ran out.
pub async fn credit_balance_row(db: &PgPool, user_id: &str) -> Result<Option<f64>, ApiError> {
    sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::DatabaseError)
}

/// Free monthly inference allowance in USD (`FREE_INFERENCE_MONTHLY_USD`,
/// default 2.00). Applies only to users without a credits row.
fn free_inference_monthly_usd() -> f64 {
    std::env::var("FREE_INFERENCE_MONTHLY_USD")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| *value >= 0.0)
        .unwrap_or(2.0)
}

/// Pre-dispatch gate. Paying users (`Some` balance) are blocked at or below
/// zero with a 403 (closest existing variant to 402). Free users (`None` — no
/// credits row) get `FREE_INFERENCE_MONTHLY_USD` per calendar month, measured
/// from the `inference_usage` rows `settle_inference` writes for them; at or
/// over the allowance they are 403'd with an upgrade nudge.
pub async fn check_inference_allowed(
    db: &PgPool,
    user_id: &str,
    balance: Option<f64>,
) -> Result<(), ApiError> {
    match balance {
        Some(balance) if balance > 0.0 => Ok(()),
        Some(_) => Err(ApiError::Forbidden(
            "Inference credits exhausted — add credits to keep using hosted models.".to_string(),
        )),
        None => {
            // Free-allowance users: the Clerk session JWT carries no
            // email-verified flag, so consult Clerk (cached in user_trust)
            // before spending our money on a possibly throwaway signup.
            // Unverified or unconfirmable → 403 (services::user_trust).
            crate::services::user_trust::email_verification_gate(db, user_id).await?;
            let month_cost: f64 = sqlx::query_scalar(
                r#"
                SELECT COALESCE(SUM(cost_usd), 0)
                FROM inference_usage
                WHERE user_id = $1 AND created_at >= date_trunc('month', NOW())
                "#,
            )
            .bind(user_id)
            .fetch_one(db)
            .await
            .map_err(ApiError::DatabaseError)?;
            let allowance = free_inference_monthly_usd();
            if month_cost >= allowance {
                return Err(ApiError::Forbidden(format!(
                    "Free inference allowance of ${allowance:.2}/month is used up — buy credits or subscribe to keep going."
                )));
            }
            Ok(())
        }
    }
}

/// Cost of one completion in USD: `(prompt/1M) * prompt_price + (completion/1M) * completion_price`,
/// plus the wholesale equivalent when live upstream pricing was available.
fn inference_cost(
    prices: &RetailPrices,
    prompt_tokens: u64,
    completion_tokens: u64,
) -> (f64, Option<f64>) {
    let cost = (prompt_tokens as f64 / 1_000_000.0) * prices.prompt_per_1m
        + (completion_tokens as f64 / 1_000_000.0) * prices.completion_per_1m;
    let wholesale = match (
        prices.wholesale_prompt_per_1m,
        prices.wholesale_completion_per_1m,
    ) {
        (Some(prompt), Some(completion)) => Some(
            (prompt_tokens as f64 / 1_000_000.0) * prompt
                + (completion_tokens as f64 / 1_000_000.0) * completion,
        ),
        _ => None,
    };
    (cost, wholesale)
}

/// Settle one completion: write the `inference_usage` row unconditionally,
/// then deduct the retail cost when the user has a credits row and
/// `charge_user` is set (BYOK calls pass false: their key pays upstream, so we
/// only meter tokens). Every failure is logged REVENUE-CRITICAL and swallowed
/// — metering must never fail the user's response; a missed deduction is
/// recoverable from the usage table, a failed completion is not.
#[allow(clippy::too_many_arguments)]
pub async fn settle_inference(
    db: &PgPool,
    user_id: &str,
    model: &str,
    pool_id: Option<&str>,
    charge_user: bool,
    prices: &RetailPrices,
    prompt_tokens: u64,
    completion_tokens: u64,
    estimated: bool,
    attribution: &InferenceAttribution,
) {
    let (cost_usd, wholesale_cost_usd) = inference_cost(prices, prompt_tokens, completion_tokens);
    let usage_id = Uuid::new_v4().to_string();
    let ref_id = format!("inference-{}", Uuid::new_v4().simple());

    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO inference_usage (
            id, user_id, model, prompt_tokens, completion_tokens, cost_usd, wholesale_cost_usd, estimated, pool_id,
            project_id, key_id, client, request_id, latency_ms
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(&usage_id)
    .bind(user_id)
    .bind(model)
    .bind(prompt_tokens as i64)
    .bind(completion_tokens as i64)
    .bind(cost_usd)
    .bind(wholesale_cost_usd)
    .bind(estimated)
    .bind(pool_id)
    .bind(&attribution.project_id)
    .bind(&attribution.key_id)
    .bind(&attribution.client)
    .bind(&attribution.request_id)
    .bind(attribution.latency_ms())
    .execute(db)
    .await
    {
        error!(
            "REVENUE-CRITICAL: failed to record inference usage for user {} (model {}, ${:.6}, ref {}): {}",
            user_id, model, cost_usd, ref_id, error
        );
        return;
    }

    match (credit_balance_row(db, user_id).await, charge_user) {
        (Ok(Some(_)), true) => {
            let cost_service = CostServiceImpl::new(db.clone());
            if let Err(error) = cost_service
                .deduct_credits_for_usage(user_id, &ref_id, cost_usd, "inference")
                .await
            {
                error!(
                    "REVENUE-CRITICAL: failed to deduct ${:.6} inference credits from user {} ({}): {}",
                    cost_usd, user_id, ref_id, error
                );
            }
        }
        (Ok(Some(_)), false) => {
            debug!(
                user_id,
                "BYOK inference usage recorded without deduction (user's own provider key pays upstream)"
            );
        }
        (Ok(None), _) => {
            debug!(
                user_id,
                "inference usage recorded without deduction (plan-cap user, no credits row)"
            );
        }
        (Err(error), _) => {
            error!(
                "REVENUE-CRITICAL: failed to check the credit balance before inference deduction for user {} ({}): {}",
                user_id, ref_id, error
            );
        }
    }
}

/// Settle a non-streaming upstream response: buffer the JSON body, read the
/// `usage` object, settle, and rebuild the response with identical
/// status/headers/body. Missing usage falls back to an honest prompt-only
/// estimate (request chars/4, completion 0) marked `estimated` — the response
/// text is not re-tokenized here.
pub async fn meter_json_response(
    db: &PgPool,
    user_id: &str,
    model: &str,
    pool_id: Option<&str>,
    charge_user: bool,
    prices: &RetailPrices,
    prompt_chars: usize,
    response: Response,
    attribution: &InferenceAttribution,
) -> Result<Response, ApiError> {
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, MAX_BUFFERED_JSON_BODY)
        .await
        .map_err(|error| ApiError::Internal(format!("Failed to buffer the upstream response: {error}")))?;

    if parts.status.is_success() {
        let parsed = serde_json::from_slice::<Value>(&bytes).ok();
        let usage = parsed.as_ref().and_then(|value| {
            let usage = &value["usage"];
            let prompt = usage["prompt_tokens"].as_u64();
            let completion = usage["completion_tokens"].as_u64();
            (prompt.is_some() || completion.is_some())
                .then(|| (prompt.unwrap_or(0), completion.unwrap_or(0)))
        });
        let (prompt_tokens, completion_tokens, estimated) = match usage {
            Some((prompt, completion)) => (prompt, completion, false),
            None => ((prompt_chars / 4) as u64, 0, true),
        };
        settle_inference(
            db,
            user_id,
            model,
            pool_id,
            charge_user,
            prices,
            prompt_tokens,
            completion_tokens,
            estimated,
            attribution,
        )
        .await;
        if attribution.capture_request.is_some() {
            record_request_bodies(db, attribution, parsed).await;
        }
    }

    Ok(Response::from_parts(parts, axum::body::Body::from(bytes)))
}

/// Everything end-of-stream settlement needs, owned so it can move into the
/// spawned settlement task.
pub struct StreamSettlement {
    pub db: Arc<PgPool>,
    pub user_id: String,
    pub model: String,
    /// Pool the request routed to (None for unseeded providers and BYOK);
    /// settled rows carry it so pool budgets can be summed.
    pub pool_id: Option<String>,
    /// false for BYOK (user's own key pays upstream): meter, never deduct.
    pub charge_user: bool,
    pub prices: RetailPrices,
    /// chars/4 estimate of the request prompt, used only when the upstream
    /// never reports usage.
    pub prompt_token_estimate: u64,
    /// Project, key, client and request id for the usage row.
    pub attribution: InferenceAttribution,
}

/// Wrap a streaming (SSE) response body in the metering adapter. Every chunk
/// is forwarded untouched; settlement fires when the stream ends or the body
/// is dropped.
pub fn meter_stream_response(response: Response, settlement: StreamSettlement) -> Response {
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        axum::body::Body::new(UsageMeteringBody::new(body, settlement)),
    )
}

/// `http_body::Body` adapter: poll_frame passthrough plus SSE byte scanning.
/// The scanner accumulates raw bytes, splits on newlines, and only fully
/// JSON-parses `data:` payloads that contain `"usage"` (the final chunk per
/// OpenAI `stream_options.include_usage`); completion text chars are counted
/// with a tolerant string scan for the no-usage fallback estimate.
pub struct UsageMeteringBody {
    inner: Pin<Box<axum::body::Body>>,
    scanner: SseUsageScanner,
    settlement: Option<StreamSettlement>,
    /// Set when the request's bodies are being kept (see
    /// [`record_request_bodies`]).
    capture: Option<StreamCapture>,
}

impl UsageMeteringBody {
    pub fn new(inner: axum::body::Body, settlement: StreamSettlement) -> Self {
        let capture = settlement
            .attribution
            .capture_request
            .is_some()
            .then(StreamCapture::default);
        Self {
            inner: Box::pin(inner),
            scanner: SseUsageScanner::default(),
            settlement: Some(settlement),
            capture,
        }
    }

    /// Fire exactly once — at end of stream or on drop (client disconnect,
    /// upstream abort). Settlement spawns onto the runtime; the stream itself
    /// is never blocked by accounting.
    fn fire_settlement(&mut self) {
        let Some(settlement) = self.settlement.take() else {
            return;
        };
        let (prompt_tokens, completion_tokens, estimated) = match self.scanner.usage {
            Some((prompt, completion)) => (prompt, completion, false),
            None => (
                settlement.prompt_token_estimate,
                (self.scanner.completion_chars / 4) as u64,
                true,
            ),
        };
        let capture = self.capture.take();
        tokio::spawn(async move {
            settle_inference(
                &settlement.db,
                &settlement.user_id,
                &settlement.model,
                settlement.pool_id.as_deref(),
                settlement.charge_user,
                &settlement.prices,
                prompt_tokens,
                completion_tokens,
                estimated,
                &settlement.attribution,
            )
            .await;
            if let Some(response) = capture.and_then(StreamCapture::into_response) {
                record_request_bodies(&settlement.db, &settlement.attribution, Some(response)).await;
            }
        });
    }
}

impl HttpBody for UsageMeteringBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_frame(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.scanner.push(data);
                    if let Some(capture) = this.capture.as_mut() {
                        capture.push(data);
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(None) => {
                this.fire_settlement();
                Poll::Ready(None)
            }
        }
    }
}

impl Drop for UsageMeteringBody {
    fn drop(&mut self) {
        // A dropped body (client disconnect, upstream abort) still settles
        // with whatever was captured — partial usage is real usage.
        self.fire_settlement();
    }
}

/// Incremental SSE scanner: byte buffer, last `usage` object seen, and a
/// running count of completion text chars for the fallback estimate.
#[derive(Default)]
struct SseUsageScanner {
    buf: Vec<u8>,
    usage: Option<(u64, u64)>,
    completion_chars: usize,
}

impl SseUsageScanner {
    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.scan_line(&line);
        }
    }

    fn scan_line(&mut self, line: &[u8]) {
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        let Some(payload) = line.trim().strip_prefix("data:") else {
            return;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        if payload.contains("\"usage\"") {
            // The usage chunk carries no content; parse it fully. Tolerate
            // malformed payloads — a missed usage just means the estimate
            // fallback fires.
            if let Ok(value) = serde_json::from_str::<Value>(payload) {
                let usage = &value["usage"];
                if usage.is_object() {
                    let prompt = usage["prompt_tokens"].as_u64();
                    let completion = usage["completion_tokens"].as_u64();
                    if prompt.is_some() || completion.is_some() {
                        self.usage = Some((prompt.unwrap_or(0), completion.unwrap_or(0)));
                    }
                }
            }
            return;
        }
        self.completion_chars += content_chars(payload);
    }
}

/// Sum the char counts of every `"content": "…"` string value in a delta
/// payload. Deliberately a tolerant scan rather than a full JSON parse (per
/// the lazy-parse contract): a missed escape only skews a fallback estimate,
/// never a billed number.
fn content_chars(payload: &str) -> usize {
    let mut total = 0;
    let mut rest = payload;
    while let Some(idx) = rest.find("\"content\":") {
        rest = &rest[idx + "\"content\":".len()..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('"') {
            rest = trimmed;
            continue;
        }
        // Walk to the closing quote, honoring backslash escapes.
        let bytes = trimmed.as_bytes();
        let mut end = 1;
        while end < bytes.len() {
            match bytes[end] {
                b'\\' => end += 2,
                b'"' => break,
                _ => end += 1,
            }
        }
        if end >= bytes.len() {
            break;
        }
        if let Ok(text) = serde_json::from_str::<String>(&trimmed[..=end]) {
            total += text.chars().count();
        }
        rest = &trimmed[end + 1..];
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn test_pool() -> PgPool {
        // The free-allowance path consults Clerk via services::user_trust;
        // unit tests here exercise the accounting, not the trust gate, so
        // bypass it (no test in this suite unsets the variable).
        std::env::set_var("ALLTERNIT_SKIP_EMAIL_VERIFICATION", "1");
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        // Inference usage schema (migrations_pg 007 + 009 + 121), minus the users FK.
        sqlx::query("DROP TABLE IF EXISTS inference_usage CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE inference_usage (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                model TEXT NOT NULL,
                prompt_tokens BIGINT NOT NULL,
                completion_tokens BIGINT NOT NULL,
                cost_usd DOUBLE PRECISION NOT NULL,
                wholesale_cost_usd DOUBLE PRECISION,
                estimated BOOLEAN NOT NULL DEFAULT FALSE,
                pool_id TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                -- migrations_pg/121: who made each completion.
                project_id TEXT,
                key_id TEXT,
                client TEXT,
                request_id TEXT,
                latency_ms BIGINT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS user_credits CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE user_credits (
                user_id TEXT PRIMARY KEY,
                balance_usd DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (balance_usd >= 0),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS credit_transactions CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE credit_transactions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                amount_usd DOUBLE PRECISION NOT NULL,
                transaction_id TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn test_prices() -> RetailPrices {
        // Live-pricing shape: wholesale 0.10/0.20 per 1M with the 1.5x default markup.
        RetailPrices {
            prompt_per_1m: 0.15,
            completion_per_1m: 0.30,
            wholesale_prompt_per_1m: Some(0.10),
            wholesale_completion_per_1m: Some(0.20),
        }
    }

    #[tokio::test]
    async fn settle_inference_records_usage_and_deducts_exact_cost() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 25.0)")
            .execute(&pool)
            .await
            .unwrap();

        settle_inference(&pool, "user_1", "gpt-4o", Some("pool_test"), true, &test_prices(), 1_000_000, 500_000, false, &InferenceAttribution::default()).await;

        // (1M/1M)*0.15 + (0.5M/1M)*0.30 = 0.30 retail; wholesale = 0.10 + 0.10 = 0.20.
        let balance = credit_balance_row(&pool, "user_1").await.unwrap().unwrap();
        assert!((balance - 24.70).abs() < 1e-9, "balance after deduction: {balance}");

        let (model, prompt, completion, cost, wholesale, estimated, pool_id): (
            String,
            i64,
            i64,
            f64,
            Option<f64>,
            bool,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT model, prompt_tokens, completion_tokens, cost_usd, wholesale_cost_usd, estimated, pool_id FROM inference_usage WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(model, "gpt-4o");
        assert_eq!(prompt, 1_000_000);
        assert_eq!(completion, 500_000);
        assert!((cost - 0.30).abs() < 1e-9);
        assert!((wholesale.unwrap() - 0.20).abs() < 1e-9);
        assert!(!estimated);
        assert_eq!(pool_id.as_deref(), Some("pool_test"), "the pool is recorded on the usage row");

        let (source, ref_like): (String, i64) = sqlx::query_as(
            "SELECT source, COUNT(*)::BIGINT FROM credit_transactions WHERE user_id = 'user_1' AND transaction_id LIKE 'inference-%' GROUP BY source",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(source, "inference");
        assert_eq!(ref_like, 1);
    }

    #[tokio::test]
    async fn byok_settlement_meters_tokens_but_deducts_nothing() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 25.0)")
            .execute(&pool)
            .await
            .unwrap();

        // Zero-priced retail: BYOK charges nothing, so cost_usd records 0.00
        // while the token counts stay exact (documented choice: the price is
        // cosmetic, the user's own key paid upstream).
        let zero_prices = RetailPrices {
            prompt_per_1m: 0.0,
            completion_per_1m: 0.0,
            wholesale_prompt_per_1m: None,
            wholesale_completion_per_1m: None,
        };
        settle_inference(&pool, "user_1", "gpt-4o", None, false, &zero_prices, 1_000_000, 500_000, false, &InferenceAttribution::default()).await;

        let (cost, prompt, completion): (f64, i64, i64) = sqlx::query_as(
            "SELECT cost_usd, prompt_tokens, completion_tokens FROM inference_usage WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cost, 0.0);
        assert_eq!(prompt, 1_000_000);
        assert_eq!(completion, 500_000);

        let debits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credit_transactions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(debits, 0, "BYOK never writes a ledger deduction");
        let balance: f64 =
            sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = 'user_1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!((balance - 25.0).abs() < 1e-9, "balance untouched: {balance}");
    }

    #[tokio::test]
    async fn settle_inference_without_credits_row_records_only() {
        let pool = test_pool().await;

        settle_inference(&pool, "user_plan_cap", "gpt-4o", None, true, &test_prices(), 1000, 500, true, &InferenceAttribution::default()).await;

        let rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM inference_usage WHERE user_id = 'user_plan_cap'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rows, 1, "the accounting trail is written even without a deduction");
        let debits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credit_transactions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(debits, 0, "plan-cap users are recorded only");
    }

    #[tokio::test]
    async fn pre_check_blocks_exhausted_balances_and_allows_paying_users() {
        let pool = test_pool().await;
        assert!(
            check_inference_allowed(&pool, "user_1", Some(0.0)).await.is_err()
        );
        assert!(
            check_inference_allowed(&pool, "user_1", Some(-1.0)).await.is_err()
        );
        assert!(
            check_inference_allowed(&pool, "user_1", Some(0.01)).await.is_ok()
        );
    }

    #[tokio::test]
    async fn free_allowance_boundary_under_at_over() {
        let pool = test_pool().await;
        // Under the default $2.00 allowance → allowed.
        settle_inference(&pool, "user_free", "gpt-4o", None, true, &test_prices(), 6_000_000, 2_000_000, false, &InferenceAttribution::default()).await; // $1.50
        assert!(
            check_inference_allowed(&pool, "user_free", None).await.is_ok(),
            "$1.50 < $2.00 must pass"
        );
        // Exactly at the allowance → blocked (>=).
        settle_inference(&pool, "user_free", "gpt-4o", None, true, &test_prices(), 2_000_000, 1_000_000, false, &InferenceAttribution::default()).await; // +$0.60 → $2.10
        let error = check_inference_allowed(&pool, "user_free", None).await.unwrap_err();
        assert!(error.to_string().contains("Free inference allowance"), "{error}");
        // Over → still blocked.
        assert!(check_inference_allowed(&pool, "user_free", None).await.is_err());
    }

    #[tokio::test]
    async fn free_allowance_counts_only_the_current_month() {
        let pool = test_pool().await;
        // A heavy usage row from last month must not count against this month.
        sqlx::query(
            r#"
            INSERT INTO inference_usage (id, user_id, model, prompt_tokens, completion_tokens, cost_usd, estimated, created_at)
            VALUES ($1, 'user_free', 'gpt-4o', 0, 0, 100.0, FALSE, date_trunc('month', NOW()) - INTERVAL '1 day')
            "#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            check_inference_allowed(&pool, "user_free", None).await.is_ok(),
            "last month's spend does not count"
        );
    }

    #[tokio::test]
    async fn meter_json_response_settles_and_preserves_the_response() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 10.0)")
            .execute(&pool)
            .await
            .unwrap();
        let body = serde_json::json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "created": 1,
            "model": "openai/gpt-4o",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" } }],
            "usage": { "prompt_tokens": 1_000_000, "completion_tokens": 500_000, "total_tokens": 1_500_000 },
        })
        .to_string();
        let response = Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.clone()))
            .unwrap();

        let response = meter_json_response(&pool, "user_1", "gpt-4o", None, true, &test_prices(), 400, response, &InferenceAttribution::default())
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["content-type"].to_str().unwrap(),
            "application/json"
        );
        let bytes = axum::body::to_bytes(response.into_body(), MAX_BUFFERED_JSON_BODY)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), body.as_bytes(), "the body is forwarded unchanged");

        let balance = credit_balance_row(&pool, "user_1").await.unwrap().unwrap();
        assert!((balance - 9.70).abs() < 1e-9, "exact usage math deducted: {balance}");
    }

    #[test]
    fn scanner_captures_usage_split_across_chunks() {
        let mut scanner = SseUsageScanner::default();
        // Split mid-JSON: the scanner must reassemble lines across chunk boundaries.
        scanner.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\nda");
        scanner.push(b"ta: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\ndata: {\"choices\":[],\"us");
        scanner.push(b"age\":{\"prompt_tokens\":33,\"completion_tokens\":17,\"total_tokens\":50}}\n\ndata: [DONE]\n\n");
        assert_eq!(scanner.usage, Some((33, 17)));
    }

    #[test]
    fn scanner_accumulates_completion_chars_when_no_usage_arrives() {
        let mut scanner = SseUsageScanner::default();
        scanner.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n");
        scanner.push(b"data: {\"choices\":[{\"delta\":{\"content\":\" wor\"}}]}\n\n");
        scanner.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"ld!\"}}]}\n\n");
        scanner.push(b"data: [DONE]\n\n");
        assert_eq!(scanner.usage, None);
        assert_eq!(scanner.completion_chars, "Hello world!".len());
    }

    #[test]
    fn content_chars_tolerates_escapes_and_non_string_values() {
        assert_eq!(content_chars("{\"content\":\"a\\\"b\"}"), 3);
        assert_eq!(content_chars("{\"content\":null}"), 0);
        assert_eq!(content_chars("{\"content\": \"spaced\"}"), 6);
        assert_eq!(content_chars("{\"role\":\"assistant\"}"), 0);
    }

    #[tokio::test]
    async fn metering_body_settles_once_and_forwards_all_bytes() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 25.0)")
            .execute(&pool)
            .await
            .unwrap();
        let sse = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"usage\":{\"prompt_tokens\":1000000,\"completion_tokens\":500000}}\n\ndata: [DONE]\n\n";
        let inner = axum::body::Body::from(&sse[..]);
        let settlement = StreamSettlement {
            db: Arc::new(pool.clone()),
            user_id: "user_1".to_string(),
            model: "gpt-4o".to_string(),
            pool_id: None,
            charge_user: true,
            prices: test_prices(),
            prompt_token_estimate: 0,
            attribution: InferenceAttribution {
                project_id: Some("proj_stream".to_string()),
                key_id: Some("ak_stream".to_string()),
                client: Some("gizzi-code".to_string()),
                request_id: Some("req_stream".to_string()),
                started_at: Some(std::time::Instant::now()),
                capture_request: None,
            },
        };
        let body = axum::body::Body::new(UsageMeteringBody::new(inner, settlement));

        // Draining the wrapped body drives settlement at end of stream.
        let forwarded = axum::body::to_bytes(body, MAX_BUFFERED_JSON_BODY)
            .await
            .unwrap();
        assert_eq!(forwarded.as_ref(), &sse[..], "chunks pass through untouched");

        // Settlement is spawned; give it a moment, then assert exactly one
        // usage row and one deduction (the second fire — Drop — is a no-op).
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inference_usage")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
        let balance = credit_balance_row(&pool, "user_1").await.unwrap().unwrap();
        assert!((balance - 24.70).abs() < 1e-9, "stream settled exactly once: {balance}");
        let (project, key, client, request, latency): (Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT project_id, key_id, client, request_id, latency_ms FROM inference_usage")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(project.as_deref(), Some("proj_stream"));
        assert_eq!(key.as_deref(), Some("ak_stream"));
        assert_eq!(client.as_deref(), Some("gizzi-code"));
        assert_eq!(request.as_deref(), Some("req_stream"), "the streamed row carries the response's request id");
        assert!(latency.is_some_and(|ms| ms >= 0));
    }

    #[test]
    fn client_is_derived_device_then_playground_then_header() {
        assert_eq!(derive_client(true, Some("playground"), Some("gizzi-code/2.3.0")), "runtime");
        assert_eq!(derive_client(false, Some("playground"), Some("gizzi-code/2.3.0")), "playground");
        assert_eq!(derive_client(false, Some("standard"), Some("gizzi-code/2.3.0")), "gizzi-code");
        assert_eq!(derive_client(false, None, Some(" Gizzi-Code/1.0")), "gizzi-code");
        assert_eq!(derive_client(false, None, Some("curl/8.4")), "api");
        assert_eq!(derive_client(false, Some("standard"), None), "api");
    }

    #[tokio::test]
    async fn settled_row_carries_project_key_client_and_request_id() {
        let pool = test_pool().await;
        let attribution = InferenceAttribution {
            project_id: Some("proj_1".to_string()),
            key_id: Some("ak_1".to_string()),
            client: Some("api".to_string()),
            request_id: Some("req_1".to_string()),
            started_at: Some(std::time::Instant::now()),
            capture_request: None,
        };
        settle_inference(&pool, "user_1", "gpt-4o", None, true, &test_prices(), 10, 5, false, &attribution).await;
        settle_inference(&pool, "user_2", "gpt-4o", None, true, &test_prices(), 10, 5, false, &InferenceAttribution::default()).await;

        let row = |user: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>)>(
                    "SELECT project_id, key_id, client, request_id, latency_ms FROM inference_usage WHERE user_id = $1",
                )
                .bind(user)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let (project, key, client, request, latency) = row("user_1").await;
        assert_eq!(project.as_deref(), Some("proj_1"));
        assert_eq!(key.as_deref(), Some("ak_1"));
        assert_eq!(client.as_deref(), Some("api"));
        assert_eq!(request.as_deref(), Some("req_1"));
        assert!(latency.is_some_and(|ms| ms >= 0));
        // An internal caller with no attribution still records the usage, NULLs alongside.
        assert_eq!(row("user_2").await, (None, None, None, None, None));
    }
}
