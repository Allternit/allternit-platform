//! User-facing prepaid credit balance.
//!
//! One authenticated endpoint (`GET /api/v1/billing/credits`) reporting the
//! user's remaining balance, month-to-date hosted usage, and their most
//! recent credit ledger entries (grants and usage debits), plus
//! `cloud_models`: whether Allternit Cloud models may run for this account
//! (`inference_settlement::cloud_models_entitlement`, the same rule as the
//! chat gate; CONTRACT-cloud-credits-2026-11-15), and `open_models`: the
//! Allternit Open entitlement (`services::open_entitlement::load`, the same
//! object as `GET /api/v1/me/usage`; CONTRACT-open-tier-2026-11-26 §3). The
//! caller is verified per request — Clerk session or `allternit_*` API token
//! (`auth::resolve_user_id`).

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use std::sync::Arc;

use crate::{auth, error::ApiError, services, ApiState};

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CreditBalanceResponse {
    balance_usd: f64,
    month_to_date_usage_usd: f64,
    recent_transactions: Vec<CreditTransactionResponse>,
    /// Allternit Cloud models entitlement (contract D4).
    cloud_models: services::CloudModelsEntitlement,
    /// Allternit Open entitlement (open-tier contract §3). Absent when it
    /// could not be read: clients then treat open rows as today.
    #[serde(skip_serializing_if = "Option::is_none")]
    open_models: Option<services::open_entitlement::OpenModelsObject>,
}

#[derive(Debug, Serialize, FromRow)]
struct CreditTransactionRow {
    amount_usd: f64,
    source: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CreditTransactionResponse {
    amount_usd: f64,
    source: String,
    created_at: DateTime<Utc>,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/billing/credits", get(get_credit_balance))
}

async fn get_credit_balance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<CreditBalanceResponse>, ApiError> {
    let user_id = auth::resolve_user_scoped(&state.db, &headers, "billing")
        .await?
        .id;

    let balance_row: Option<f64> =
        sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = $1")
            .bind(&user_id)
            .fetch_optional(&state.db)
            .await?;
    let balance_usd = balance_row.unwrap_or(0.0);
    let cloud_models = services::cloud_models_entitlement(&state.db, &user_id, balance_row).await;
    let open_models = services::open_entitlement::load(&state.db, &user_id)
        .await
        .map_err(|error| tracing::warn!(%error, "open_models unreadable for /billing/credits"))
        .ok();
    let usage = services::hosted_usage_summary(&state.db, &user_id).await?;
    let recent = sqlx::query_as::<_, CreditTransactionRow>(
        r#"
        SELECT amount_usd, source, created_at
        FROM credit_transactions
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT 20
        "#,
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(CreditBalanceResponse {
        balance_usd,
        month_to_date_usage_usd: usage.estimated_cost_usd,
        recent_transactions: recent
            .into_iter()
            .map(|row| CreditTransactionResponse {
                amount_usd: row.amount_usd,
                source: row.source,
                created_at: row.created_at,
            })
            .collect(),
        cloud_models,
        open_models,
    }))
}
