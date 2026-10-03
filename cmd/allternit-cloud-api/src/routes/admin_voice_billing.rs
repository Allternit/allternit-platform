//! Admin-only Cloud Voice / phone overage billing (ships off, see
//! `services::voice_billing`).
//!
//! * `GET  /api/v1/admin/voice-billing/periods?period=YYYY-MM`: computed rows.
//! * `POST /api/v1/admin/voice-billing/periods/:period/compute`: (re)compute a
//!   period's rows (no-op while the mode is `off`; never calls Stripe).
//! * `POST /api/v1/admin/voice-billing/periods/:period/approve`: send that
//!   period's invoice items to Stripe. Only in mode `live`, only after the
//!   period has ended, only for an admin (`ALLTERNIT_ADMIN_USER_IDS`).

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{
    error::ApiError,
    routes::billing_checkout::ReqwestStripeCheckout,
    services::voice_billing::{self, Mode},
    ApiState,
};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/admin/voice-billing/periods", get(list))
        .route("/api/v1/admin/voice-billing/periods/:period/compute", post(compute))
        .route("/api/v1/admin/voice-billing/periods/:period/approve", post(approve))
}

async fn admin(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "account").await?;
    if !crate::auth::is_admin_user(&user.id) {
        return Err(ApiError::Forbidden("Admin only.".to_string()));
    }
    Ok(user.id)
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "off",
        Mode::DryRun => "dry_run",
        Mode::Live => "live",
    }
}

#[derive(Deserialize)]
struct ListQuery {
    period: Option<String>,
}

async fn list(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    admin(&state, &headers).await?;
    let period = q.period.unwrap_or_else(|| voice_billing::period_label(Utc::now()));
    let rows = voice_billing::list_periods(&state.db, &period).await?;
    let total: i64 = rows.iter().map(|r| r.amount_cents).sum();
    Ok(Json(json!({
        "mode": mode_name(Mode::from_env()),
        "period": period,
        "totalCents": total,
        "periods": rows,
    })))
}

async fn compute(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(period): Path<String>,
) -> Result<Json<Value>, ApiError> {
    admin(&state, &headers).await?;
    let mode = Mode::from_env();
    let written = voice_billing::compute_period(&state.db, mode, &period).await?;
    Ok(Json(json!({ "mode": mode_name(mode), "period": period, "rowsWritten": written })))
}

async fn approve(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(period): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let admin_id = admin(&state, &headers).await?;
    let outcome = voice_billing::approve_period(
        &state.db,
        Mode::from_env(),
        &ReqwestStripeCheckout::new(),
        &period,
        &admin_id,
        Utc::now(),
    )
    .await?;
    tracing::info!(admin = %admin_id, period = %period, invoiced = outcome.invoiced, failed = outcome.failed, "voice billing period approved");
    Ok(Json(json!({ "period": period, "outcome": outcome })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn every_route_needs_a_signed_in_admin() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let router = routes().with_state(state);
        for (method, uri) in [
            ("GET", "/api/v1/admin/voice-billing/periods?period=2026-10"),
            ("POST", "/api/v1/admin/voice-billing/periods/2026-10/compute"),
            ("POST", "/api/v1/admin/voice-billing/periods/2026-10/approve"),
        ] {
            let r = router
                .clone()
                .oneshot(Request::builder().method(method).uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert!(
                r.status() == StatusCode::UNAUTHORIZED || r.status() == StatusCode::FORBIDDEN,
                "{method} {uri} -> {}",
                r.status()
            );
        }
    }
}
