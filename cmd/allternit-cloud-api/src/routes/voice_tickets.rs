//! Cloud Voice tickets and minutes metering (track G).
//!
//! * `POST /api/v1/voice/tickets` (Clerk user): mints a 60 s signed ticket for the
//!   voice service, or 402 `no-cloud-minutes` when the plan has no Cloud Voice.
//! * `POST /api/v1/voice/tickets/redeem` (voice service, bearer
//!   `ALLTERNIT_VOICE_WORKER_TOKEN`): verifies a ticket and burns its nonce.
//! * `POST /api/v1/voice/usage` (voice service, same bearer): records session seconds.
//! * `GET /api/v1/voice/usage/month` (Clerk user): this month's usage and allowance.
//!
//! Metering and exposure only: nothing here calls Stripe or charges anyone.
//! Ticket format: `v1.<b64url(json)>.<b64url(HMAC-SHA256(secret, "v1."+<b64url(json)>))>`.

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, TimeZone, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use std::sync::Arc;

use crate::{
    auth,
    error::ApiError,
    services::voice_billing,
    services::voice_usage::{
        self, included_cloud_voice_seconds, plan_allows_overage, plan_for_user,
        CLOUD_VOICE_RATE_USD_PER_MIN, ENGINE_CLOUD, ENGINE_PHONE, PHONE_RATE_USD_PER_MIN,
    },
    ApiState,
};

type HmacSha256 = Hmac<Sha256>;

/// A ticket is redeemable for this long after it is minted.
pub const TICKET_TTL_SECONDS: i64 = 60;
/// Longest single Cloud Voice session a ticket allows.
pub const MAX_SESSION_SECONDS: i64 = 30 * 60;
/// Shortest secret/token accepted as "configured" (matches the billing secrets).
const MIN_SECRET_LEN: usize = 32;
/// Sanity cap on one usage report (a ticket allows 30 min; calls can be longer).
const MAX_REPORT_SECONDS: i64 = 6 * 60 * 60;

const ENV_SECRET: &str = "ALLTERNIT_VOICE_TICKET_SECRET";
const ENV_WS_URL: &str = "ALLTERNIT_VOICE_CLOUD_WS_URL";
const ENV_WORKER_TOKEN: &str = "ALLTERNIT_VOICE_WORKER_TOKEN";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/voice/tickets", post(mint_ticket))
        .route("/api/v1/voice/tickets/redeem", post(redeem_ticket))
        .route("/api/v1/voice/usage", post(report_usage))
        .route("/api/v1/voice/usage/month", get(month_usage))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketClaims {
    pub sub: String,
    pub plan: String,
    /// Unix seconds.
    pub exp: i64,
    pub nonce: String,
    pub max_seconds: i64,
}

#[derive(Debug, PartialEq)]
pub enum TicketError {
    Malformed,
    BadSignature,
    Expired,
}

fn sign(secret: &str, signing_input: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(signing_input.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub fn mint(secret: &str, claims: &TicketClaims) -> String {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims serialize"));
    let signing_input = format!("v1.{payload}");
    let sig = URL_SAFE_NO_PAD.encode(sign(secret, &signing_input));
    format!("{signing_input}.{sig}")
}

/// Verify the signature (constant time) and expiry, returning the claims.
pub fn verify(secret: &str, ticket: &str, now: DateTime<Utc>) -> Result<TicketClaims, TicketError> {
    let mut parts = ticket.split('.');
    let (Some("v1"), Some(payload), Some(sig), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(TicketError::Malformed);
    };
    let sig = URL_SAFE_NO_PAD.decode(sig).map_err(|_| TicketError::Malformed)?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(format!("v1.{payload}").as_bytes());
    mac.verify_slice(&sig).map_err(|_| TicketError::BadSignature)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| TicketError::Malformed)?;
    let claims: TicketClaims = serde_json::from_slice(&bytes).map_err(|_| TicketError::Malformed)?;
    if claims.exp <= now.timestamp() {
        return Err(TicketError::Expired);
    }
    Ok(claims)
}

fn coded(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "code": code, "message": message }))).into_response()
}

fn cloud_unavailable() -> Response {
    coded(
        StatusCode::SERVICE_UNAVAILABLE,
        "cloud-unavailable",
        "Cloud Voice is not available right now.",
    )
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The ticket secret, only when set and long enough to be a real secret.
fn ticket_secret() -> Option<String> {
    env_nonempty(ENV_SECRET).filter(|v| v.len() >= MIN_SECRET_LEN)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0_u8, |d, (l, r)| d | (l ^ r))
        == 0
}

/// Check the voice service's bearer token. `Err` is the response to send.
pub(crate) fn require_worker(headers: &HeaderMap) -> Result<(), Response> {
    let Some(expected) = env_nonempty(ENV_WORKER_TOKEN).filter(|v| v.len() >= MIN_SECRET_LEN)
    else {
        return Err(cloud_unavailable());
    };
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if constant_time_eq(&expected, provided) {
        Ok(())
    } else {
        Err(coded(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid worker credential."))
    }
}

/// Mint a ticket for `user_id`. Split from the handler so it can be tested
/// without a Clerk session.
pub(crate) async fn mint_for_user(
    state: &ApiState,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let (Some(secret), Some(ws_url)) = (ticket_secret(), env_nonempty(ENV_WS_URL)) else {
        return Ok(cloud_unavailable());
    };
    let plan = plan_for_user(&state.db, user_id).await?;
    let included = included_cloud_voice_seconds(&plan);
    let used = voice_usage::month_usage_at(&state.db, user_id, ENGINE_CLOUD, now).await?;
    let remaining = (included - used).max(0);
    if remaining == 0 && !plan_allows_overage(&plan) {
        return Ok(coded(
            StatusCode::PAYMENT_REQUIRED,
            "no-cloud-minutes",
            "Your plan has no Cloud Voice minutes. Upgrade to use Cloud Voice.",
        ));
    }
    let exp = now.timestamp() + TICKET_TTL_SECONDS;
    let claims = TicketClaims {
        sub: user_id.to_string(),
        plan,
        exp,
        nonce: uuid::Uuid::new_v4().simple().to_string(),
        max_seconds: MAX_SESSION_SECONDS,
    };
    let expires_at = Utc.timestamp_opt(exp, 0).single().unwrap_or(now).to_rfc3339();
    Ok(Json(json!({
        "ticket": mint(&secret, &claims),
        "wsUrl": ws_url,
        "expiresAt": expires_at,
    }))
    .into_response())
}

async fn mint_ticket(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user_id = auth::resolve_user_id(&state.db, &headers).await?;
    mint_for_user(&state, &user_id, Utc::now()).await
}

#[derive(Debug, Deserialize)]
struct RedeemRequest {
    ticket: String,
}

pub(crate) async fn redeem(
    state: &ApiState,
    ticket: &str,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    let Some(secret) = ticket_secret() else {
        return Ok(cloud_unavailable());
    };
    let invalid = || coded(StatusCode::UNAUTHORIZED, "invalid-ticket", "Invalid or expired ticket.");
    let Ok(claims) = verify(&secret, ticket, now) else {
        return Ok(invalid());
    };
    // Opportunistic purge; a ticket is dead once past exp, with slack for clock skew.
    sqlx::query("DELETE FROM voice_tickets_used WHERE expires_at < $1")
        .bind(now - chrono::Duration::minutes(5))
        .execute(&state.db)
        .await?;
    let expires_at = Utc.timestamp_opt(claims.exp, 0).single().unwrap_or(now);
    let inserted = sqlx::query(
        r#"
        INSERT INTO voice_tickets_used (nonce, user_id, expires_at, used_at)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (nonce) DO NOTHING
        "#,
    )
    .bind(&claims.nonce)
    .bind(&claims.sub)
    .bind(expires_at)
    .bind(now)
    .execute(&state.db)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(coded(StatusCode::UNAUTHORIZED, "ticket-used", "Ticket already used."));
    }
    Ok(Json(json!({
        "sub": claims.sub,
        "plan": claims.plan,
        "maxSeconds": claims.max_seconds,
    }))
    .into_response())
}

async fn redeem_ticket(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<RedeemRequest>,
) -> Result<Response, ApiError> {
    if let Err(response) = require_worker(&headers) {
        return Ok(response);
    }
    redeem(&state, &body.ticket, Utc::now()).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageReport {
    sub: String,
    session_id: String,
    seconds: i64,
    engine: String,
}

async fn report_usage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<UsageReport>,
) -> Result<Response, ApiError> {
    if let Err(response) = require_worker(&headers) {
        return Ok(response);
    }
    // The phone path records through `voice_usage::record` directly.
    if body.engine != ENGINE_CLOUD {
        return Err(ApiError::BadRequest("engine must be \"cloud\".".to_string()));
    }
    if body.sub.trim().is_empty() || body.sub.len() > 160 {
        return Err(ApiError::BadRequest("sub is required (160 characters max).".to_string()));
    }
    if body.session_id.trim().is_empty() || body.session_id.len() > 160 {
        return Err(ApiError::BadRequest(
            "sessionId is required (160 characters max).".to_string(),
        ));
    }
    if !(0..=MAX_REPORT_SECONDS).contains(&body.seconds) {
        return Err(ApiError::BadRequest(format!(
            "seconds must be between 0 and {MAX_REPORT_SECONDS}."
        )));
    }
    voice_usage::record(&state.db, &body.sub, ENGINE_CLOUD, body.seconds as i32, &body.session_id)
        .await?;
    Ok(Json(json!({ "ok": true })).into_response())
}

pub(crate) async fn month_for_user(
    state: &ApiState,
    user_id: &str,
    now: DateTime<Utc>,
) -> Result<serde_json::Value, ApiError> {
    let plan = plan_for_user(&state.db, user_id).await?;
    let cloud = voice_usage::month_usage_at(&state.db, user_id, ENGINE_CLOUD, now).await?;
    let phone = voice_usage::month_usage_at(&state.db, user_id, ENGINE_PHONE, now).await?;
    // Estimate of this month's voice charges (cloud minutes over the allowance,
    // phone minutes, phone numbers). `billingEnabled` is false while overage
    // billing isn't live, so clients can hide the line instead of promising a charge.
    // A display-only figure must not take the usage endpoint down with it.
    let est = voice_billing::estimate_month(&state.db, user_id, now).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "voice overage estimate unavailable");
        voice_billing::compute_amounts("free", 0, 0, 0)
    });
    Ok(json!({
        "engine": ENGINE_CLOUD,
        "usedSeconds": cloud,
        "includedSeconds": included_cloud_voice_seconds(&plan),
        "overageRateUsdPerMin": CLOUD_VOICE_RATE_USD_PER_MIN,
        "phone": { "usedSeconds": phone, "rateUsdPerMin": PHONE_RATE_USD_PER_MIN },
        "estimatedOverageCents": est.amount_cents,
        "estimatedCloudOverageCents": est.cloud_cents,
        // Only live billing can charge; dry_run computes periods but never calls Stripe.
        "billingEnabled": voice_billing::Mode::from_env() == voice_billing::Mode::Live,
    }))
}

async fn month_usage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = auth::resolve_user_id(&state.db, &headers).await?;
    Ok(Json(month_for_user(&state, &user_id, Utc::now()).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use axum::body::to_bytes;
    use serial_test::serial;

    const SECRET: &str = "test-voice-ticket-secret-0123456789abcdef";
    const WORKER: &str = "test-voice-worker-token-0123456789abcdef";

    fn claims(exp: i64) -> TicketClaims {
        TicketClaims {
            sub: "user_1".into(),
            plan: "plus".into(),
            exp,
            nonce: "n1".into(),
            max_seconds: 1800,
        }
    }

    fn set_env() {
        std::env::set_var(ENV_SECRET, SECRET);
        std::env::set_var(ENV_WS_URL, "wss://voice.example.test/v1/voice/session");
        std::env::set_var(ENV_WORKER_TOKEN, WORKER);
    }

    fn clear_env() {
        std::env::remove_var(ENV_SECRET);
        std::env::remove_var(ENV_WS_URL);
        std::env::remove_var(ENV_WORKER_TOKEN);
    }

    async fn state() -> Arc<ApiState> {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(
            &include_str!("../../migrations_pg/029_voice_usage.sql").replace("public.", ""),
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::raw_sql(
            &include_str!("../../migrations_pg/024_phone_numbers.sql").replace("public.", ""),
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE billing_subscriptions (user_id TEXT, plan_id TEXT, plan_tier TEXT, \
             status TEXT, updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        )
        .execute(&state.db)
        .await
        .unwrap();
        state
    }

    async fn subscribe(state: &ApiState, user: &str, plan: &str) {
        sqlx::query(
            "INSERT INTO billing_subscriptions (user_id, plan_id, plan_tier, status) \
             VALUES ($1, $2, 'pro', 'active')",
        )
        .bind(user)
        .bind(plan)
        .execute(&state.db)
        .await
        .unwrap();
    }

    async fn body_json(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(json!(null)))
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        h
    }

    #[test]
    fn ticket_good_tampered_expired_wrong_secret() {
        let now = Utc::now();
        let exp = now.timestamp() + 60;
        let ticket = mint(SECRET, &claims(exp));
        assert!(ticket.starts_with("v1."));
        assert_eq!(verify(SECRET, &ticket, now).unwrap(), claims(exp));

        // Tampered payload (swap in another user, keep the old signature).
        let mut parts: Vec<&str> = ticket.split('.').collect();
        let forged = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&TicketClaims { sub: "attacker".into(), ..claims(exp) }).unwrap(),
        );
        parts[1] = &forged;
        assert_eq!(verify(SECRET, &parts.join("."), now), Err(TicketError::BadSignature));

        // Tampered signature, and garbage.
        assert_eq!(verify(SECRET, &format!("{ticket}A"), now), Err(TicketError::BadSignature));
        assert_eq!(verify(SECRET, "nonsense", now), Err(TicketError::Malformed));
        assert_eq!(verify(SECRET, "v2.a.b", now), Err(TicketError::Malformed));

        // Expired (exp is exclusive) and wrong secret.
        assert_eq!(verify(SECRET, &ticket, now + chrono::Duration::seconds(60)), Err(TicketError::Expired));
        assert_eq!(verify("another-secret-another-secret-12345", &ticket, now), Err(TicketError::BadSignature));
    }

    #[tokio::test]
    #[serial]
    async fn mint_402_for_free_and_ok_for_plus_and_503_unconfigured() {
        let state = state().await;
        let now = Utc::now();

        clear_env();
        let (status, body) = body_json(mint_for_user(&state, "free_user", now).await.unwrap()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "cloud-unavailable");

        // Each env var missing on its own is also unavailable (never mint unsigned).
        std::env::set_var(ENV_SECRET, SECRET);
        let (status, _) = body_json(mint_for_user(&state, "free_user", now).await.unwrap()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        clear_env();
        std::env::set_var(ENV_WS_URL, "wss://x.test/s");
        let (status, _) = body_json(mint_for_user(&state, "free_user", now).await.unwrap()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        set_env();
        let (status, body) = body_json(mint_for_user(&state, "free_user", now).await.unwrap()).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(body["code"], "no-cloud-minutes");

        subscribe(&state, "plus_user", "plus").await;
        let (status, body) = body_json(mint_for_user(&state, "plus_user", now).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["wsUrl"], "wss://voice.example.test/v1/voice/session");
        let c = verify(SECRET, body["ticket"].as_str().unwrap(), now).unwrap();
        assert_eq!((c.sub.as_str(), c.plan.as_str(), c.max_seconds), ("plus_user", "plus", MAX_SESSION_SECONDS));
        assert_eq!(c.exp, now.timestamp() + TICKET_TTL_SECONDS);

        // Plus past its included minutes goes into overage, not 402.
        voice_usage::record_at(&state.db, "plus_user", "cloud", 7000, "big", now).await.unwrap();
        let (status, _) = body_json(mint_for_user(&state, "plus_user", now).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn redeem_is_single_use_and_rejects_bad_tickets() {
        let state = state().await;
        set_env();
        let now = Utc::now();
        let ticket = mint(SECRET, &claims(now.timestamp() + 60));

        let (status, body) = body_json(redeem(&state, &ticket, now).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"sub": "user_1", "plan": "plus", "maxSeconds": 1800}));

        let (status, body) = body_json(redeem(&state, &ticket, now).await.unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "ticket-used");

        let expired = mint(SECRET, &TicketClaims { nonce: "n2".into(), ..claims(now.timestamp() - 1) });
        assert_eq!(body_json(redeem(&state, &expired, now).await.unwrap()).await.0, StatusCode::UNAUTHORIZED);
        let wrong = mint("another-secret-another-secret-12345", &TicketClaims { nonce: "n3".into(), ..claims(now.timestamp() + 60) });
        assert_eq!(body_json(redeem(&state, &wrong, now).await.unwrap()).await.0, StatusCode::UNAUTHORIZED);

        // Old rows are purged on a later redeem.
        let later = now + chrono::Duration::minutes(10);
        let t2 = mint(SECRET, &TicketClaims { nonce: "n4".into(), ..claims(later.timestamp() + 60) });
        assert_eq!(body_json(redeem(&state, &t2, later).await.unwrap()).await.0, StatusCode::OK);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM voice_tickets_used")
            .fetch_one(&state.db).await.unwrap();
        assert_eq!(rows, 1);
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn worker_endpoints_need_the_bearer_token() {
        let state = state().await;
        clear_env();
        let r = redeem_ticket(State(state.clone()), bearer(WORKER), Json(RedeemRequest { ticket: "x".into() })).await.unwrap();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);

        set_env();
        let r = redeem_ticket(State(state.clone()), HeaderMap::new(), Json(RedeemRequest { ticket: "x".into() })).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = redeem_ticket(State(state.clone()), bearer("wrong"), Json(RedeemRequest { ticket: "x".into() })).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = redeem_ticket(State(state.clone()), bearer(WORKER), Json(RedeemRequest { ticket: "x".into() })).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED); // authorised, but not a valid ticket

        let report = |seconds, engine: &str| UsageReport {
            sub: "u1".into(), session_id: "s1".into(), seconds, engine: engine.into(),
        };
        let r = report_usage(State(state.clone()), HeaderMap::new(), Json(report(60, "cloud"))).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert!(report_usage(State(state.clone()), bearer(WORKER), Json(report(60, "phone"))).await.is_err());
        assert!(report_usage(State(state.clone()), bearer(WORKER), Json(report(-5, "cloud"))).await.is_err());
        for _ in 0..2 {
            let r = report_usage(State(state.clone()), bearer(WORKER), Json(report(60, "cloud"))).await.unwrap();
            assert_eq!(r.status(), StatusCode::OK);
        }
        assert_eq!(voice_usage::month_usage(&state.db, "u1", "cloud").await.unwrap(), 60);
        clear_env();
    }

    #[tokio::test]
    async fn month_endpoint_reports_allowance_and_usage() {
        let state = state().await;
        let now = Utc::now();
        subscribe(&state, "plus_user", "plus").await;
        voice_usage::record_at(&state.db, "plus_user", "cloud", 90, "a", now).await.unwrap();
        voice_usage::record_at(&state.db, "plus_user", "phone", 30, "c1", now).await.unwrap();
        let body = month_for_user(&state, "plus_user", now).await.unwrap();
        assert_eq!(body["engine"], "cloud");
        assert_eq!(body["usedSeconds"], 90);
        assert_eq!(body["includedSeconds"], 6000);
        assert_eq!(body["overageRateUsdPerMin"], 0.08);
        assert_eq!(body["phone"], json!({"usedSeconds": 30, "rateUsdPerMin": 0.08}));
        // 90 s of cloud is inside the allowance; 30 s of phone rounds up to 1 min.
        assert_eq!(body["estimatedCloudOverageCents"], 0);
        assert_eq!(body["estimatedOverageCents"], 8);
        assert!(body["billingEnabled"].is_boolean());
        let free = month_for_user(&state, "nobody", now).await.unwrap();
        assert_eq!(free["includedSeconds"], 0);
    }

    /// Building the full router panics on overlapping routes; also proves the
    /// voice routes are mounted and that the user routes reject anonymous calls.
    #[tokio::test]
    #[serial]
    async fn full_router_builds_and_serves_voice_routes() {
        use tower::ServiceExt;
        clear_env();
        let state = state().await;
        let router = crate::create_router(state);
        let call = |method: &str, path: &str| {
            axum::http::Request::builder().method(method).uri(path)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"ticket":"x"}"#)).unwrap()
        };
        let r = router.clone().oneshot(call("POST", "/api/v1/voice/tickets")).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = router.clone().oneshot(call("GET", "/api/v1/voice/usage/month")).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = router.clone().oneshot(call("POST", "/api/v1/voice/tickets/redeem")).await.unwrap();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
