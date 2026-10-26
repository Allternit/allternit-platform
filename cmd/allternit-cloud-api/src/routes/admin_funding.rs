//! Upstream funding admin routes (admin only, `ALLTERNIT_ADMIN_USER_IDS`).
//!
//! - `GET /api/v1/admin/funding` — per provider: balance, source, 24 h burn,
//!   days left, reserve days, floor, below-floor flag, last error; plus the
//!   total customer wallet liability and the coverage ratio (upstream money
//!   on hand per dollar customers hold).
//! - `PUT /api/v1/admin/funding/:provider` — set a manual balance (for
//!   providers with no balance API), reserve days and/or floor. The provider
//!   is refreshed right away and its new row returned.
//!
//! The data comes from `services::upstream_funding`.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{
    error::ApiError,
    services::upstream_funding::{self, FundingRow},
    ApiState,
};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/admin/funding", get(get_funding))
        .route("/api/v1/admin/funding/:provider", axum::routing::put(put_funding))
}

async fn require_admin(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "account").await?;
    if !crate::auth::is_admin_user(&user.id) {
        return Err(ApiError::Forbidden("Admins only".to_string()));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFunding {
    #[serde(flatten)]
    pub row: FundingRow,
    pub days_left: Option<f64>,
    pub below_floor: bool,
    /// Days left under reserve_days, or balance under the floor.
    pub short: bool,
    /// Whether this process has the provider's production key set.
    pub configured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingResponse {
    pub providers: Vec<ProviderFunding>,
    pub upstream_total_usd: f64,
    pub wallet_liability_usd: f64,
    /// upstream_total_usd / wallet_liability_usd; null when nobody holds credits.
    pub coverage_ratio: Option<f64>,
    pub burn_24h_total_usd: f64,
}

fn provider_view(row: FundingRow, configured: &[&str]) -> ProviderFunding {
    let short = upstream_funding::shortfall(row.balance_usd, row.burn_24h_usd, row.reserve_days, row.floor_usd);
    ProviderFunding {
        days_left: upstream_funding::days_left(row.balance_usd, row.burn_24h_usd),
        below_floor: matches!(short, Some(upstream_funding::Shortfall::BelowFloor { .. })),
        short: short.is_some(),
        configured: configured.contains(&row.provider.as_str()),
        row,
    }
}

async fn get_funding(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<FundingResponse>, ApiError> {
    require_admin(&state, &headers).await?;
    let configured = upstream_funding::configured_providers();
    let rows = upstream_funding::load_rows(&state.db).await?;
    let wallet_liability_usd = upstream_funding::wallet_liability(&state.db).await?;
    let upstream_total_usd: f64 = rows.iter().filter_map(|r| r.balance_usd).map(|b| b.max(0.0)).sum();
    let burn_24h_total_usd: f64 = rows.iter().map(|r| r.burn_24h_usd).sum();
    Ok(Json(FundingResponse {
        providers: rows.into_iter().map(|row| provider_view(row, &configured)).collect(),
        coverage_ratio: upstream_funding::coverage_ratio(upstream_total_usd, wallet_liability_usd),
        upstream_total_usd,
        wallet_liability_usd,
        burn_24h_total_usd,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FundingUpdate {
    /// The balance shown in the provider's dashboard right now (USD).
    pub balance_usd: Option<f64>,
    pub reserve_days: Option<i32>,
    pub floor_usd: Option<f64>,
}

async fn put_funding(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(update): Json<FundingUpdate>,
) -> Result<Json<ProviderFunding>, ApiError> {
    require_admin(&state, &headers).await?;
    let provider = provider.trim().to_lowercase();
    if !upstream_funding::is_known_provider(&provider) {
        return Err(ApiError::NotFound(format!("Unknown provider: {provider}")));
    }
    if update.balance_usd.is_some_and(|b| !b.is_finite()) {
        return Err(ApiError::BadRequest("balanceUsd must be a number".to_string()));
    }
    if update.reserve_days.is_some_and(|d| !(0..=365).contains(&d)) {
        return Err(ApiError::BadRequest("reserveDays must be 0 to 365".to_string()));
    }
    if update.floor_usd.is_some_and(|f| !f.is_finite() || f < 0.0) {
        return Err(ApiError::BadRequest("floorUsd must be 0 or more".to_string()));
    }

    let manual_set_at = update.balance_usd.map(|_| Utc::now());
    sqlx::query(
        r#"
        INSERT INTO upstream_balances (provider, source, reserve_days, floor_usd, manual_balance_usd, manual_set_at)
        VALUES ($1, 'manual', COALESCE($2, 3), COALESCE($3, 0), $4, $5)
        ON CONFLICT (provider) DO UPDATE SET
            reserve_days = COALESCE($2, upstream_balances.reserve_days),
            floor_usd = COALESCE($3, upstream_balances.floor_usd),
            manual_balance_usd = COALESCE($4, upstream_balances.manual_balance_usd),
            manual_set_at = COALESCE($5, upstream_balances.manual_set_at),
            updated_at = now()
        "#,
    )
    .bind(&provider)
    .bind(update.reserve_days)
    .bind(update.floor_usd)
    .bind(update.balance_usd)
    .bind(manual_set_at)
    .execute(&state.db)
    .await?;

    let row = upstream_funding::refresh_provider(&state.db, &reqwest::Client::new(), &provider).await?;
    Ok(Json(provider_view(row, &upstream_funding::configured_providers())))
}
