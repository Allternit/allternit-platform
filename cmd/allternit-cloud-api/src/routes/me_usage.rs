//! GET /api/v1/me/usage — Allternit subscription + remaining compute.
//!
//! Desktop forwards the paired runtime device token through local
//! `allternit-api` to this route. Clerk sessions and `allternit_*` API tokens
//! also work. The payload is the compute meter the shell should show: plan
//! (Free / Plus / Super / Ultra) and remaining credits or free-inference
//! allowance. Local Ollama is not this signal.

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::Serialize;
use sqlx::FromRow;
use std::sync::Arc;

use crate::{
    auth,
    error::ApiError,
    routes::billing_subscriptions::find_plan,
    routes::runtime_pairing::{device_token_from_headers, runtime_device_for_token},
    services, ApiState,
};

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    pub plan: String,
    pub label: String,
    pub status: String,
    pub plan_tier: String,
    pub weekly_used: f64,
    pub weekly_limit: f64,
    pub resets_at: Option<String>,
    pub credits: Option<f64>,
    pub month_to_date_usage_usd: f64,
    pub recent_transactions: Vec<UsageTransaction>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageTransaction {
    pub amount_usd: f64,
    pub source: String,
    pub created_at: String,
}

#[derive(Debug, FromRow)]
struct CreditTxnRow {
    amount_usd: f64,
    source: String,
    created_at: DateTime<Utc>,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/me/usage", get(get_my_usage))
}

async fn caller_user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    match auth::resolve_user(&state.db, headers).await {
        Ok(user) => Ok(user.id),
        Err(primary) => {
            let Some(token) = device_token_from_headers(headers) else {
                return Err(primary);
            };
            Ok(runtime_device_for_token(&state.db, token, None)
                .await?
                .user_id)
        }
    }
}

pub(crate) fn next_month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let year = now.year();
    let month = now.month();
    if month == 12 {
        Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(now)
    } else {
        Utc.with_ymd_and_hms(year, month + 1, 1, 0, 0, 0)
            .single()
            .unwrap_or(now)
    }
}

fn plan_label(plan_id: &str) -> &'static str {
    match plan_id {
        "plus" => "Plus",
        "super" => "Super",
        "ultra" => "Ultra",
        _ => "Free",
    }
}

fn free_inference_monthly_usd() -> f64 {
    std::env::var("FREE_INFERENCE_MONTHLY_USD")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| *value >= 0.0)
        .unwrap_or(2.0)
}

pub(crate) fn compose_usage(
    plan_id: &str,
    plan_status: &str,
    plan_tier: &str,
    credit_balance: Option<f64>,
    month_to_date_usage_usd: f64,
    free_used_usd: f64,
    free_allowance_usd: f64,
    resets_at: DateTime<Utc>,
    recent_transactions: Vec<UsageTransaction>,
) -> UsageResponse {
    let paid = matches!(plan_id, "plus" | "super" | "ultra")
        && matches!(plan_status, "active" | "trialing");
    let monthly_grant = find_plan(plan_id)
        .map(|plan| plan.monthly_credits_usd)
        .unwrap_or(0.0);
    let (credits, weekly_limit, weekly_used) = if paid || credit_balance.is_some() {
        let balance = credit_balance.unwrap_or(0.0);
        (
            Some(balance),
            if monthly_grant > 0.0 {
                monthly_grant
            } else {
                balance.max(0.0) + month_to_date_usage_usd
            },
            month_to_date_usage_usd,
        )
    } else {
        let remaining = (free_allowance_usd - free_used_usd).max(0.0);
        (Some(remaining), free_allowance_usd, free_used_usd)
    };
    UsageResponse {
        plan: if paid { plan_id.to_string() } else { "free".to_string() },
        label: if paid {
            plan_label(plan_id).to_string()
        } else {
            "Free".to_string()
        },
        status: if paid {
            plan_status.to_string()
        } else {
            "none".to_string()
        },
        plan_tier: if paid {
            plan_tier.to_string()
        } else {
            "free".to_string()
        },
        weekly_used,
        weekly_limit,
        resets_at: Some(resets_at.to_rfc3339()),
        credits,
        month_to_date_usage_usd,
        recent_transactions,
    }
}

async fn get_my_usage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<UsageResponse>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;

    let sub: Option<(String, String, String)> = sqlx::query_as(
        r#"
        SELECT plan_id, plan_tier, status
        FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing')
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let (plan_id, plan_tier, plan_status) = if crate::auth::is_admin_user(&user_id) {
        (
            "ultra".to_string(),
            "team".to_string(),
            "active".to_string(),
        )
    } else {
        sub.unwrap_or_else(|| {
            (
                "free".to_string(),
                "free".to_string(),
                "none".to_string(),
            )
        })
    };

    let credit_balance = services::credit_balance_row(&state.db, &user_id)
        .await
        .ok()
        .flatten();

    let hosted = services::hosted_usage_summary(&state.db, &user_id)
        .await
        .unwrap_or(services::HostedUsageSummary {
            total_seconds: 0,
            estimated_cost_usd: 0.0,
        });

    let free_used: f64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(cost_usd), 0)
        FROM inference_usage
        WHERE user_id = $1 AND created_at >= date_trunc('month', NOW())
        "#,
    )
    .bind(&user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(0.0);

    let recent = sqlx::query_as::<_, CreditTxnRow>(
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
    .await
    .unwrap_or_default();

    Ok(Json(compose_usage(
        &plan_id,
        &plan_status,
        &plan_tier,
        credit_balance,
        hosted.estimated_cost_usd,
        free_used,
        free_inference_monthly_usd(),
        next_month_start(Utc::now()),
        recent
            .into_iter()
            .map(|row| UsageTransaction {
                amount_usd: row.amount_usd,
                source: row.source,
                created_at: row.created_at.to_rfc3339(),
            })
            .collect(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_plan_reports_remaining_inference_allowance() {
        let usage = compose_usage(
            "free",
            "none",
            "free",
            None,
            0.0,
            0.40,
            2.0,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
        );
        assert_eq!(usage.plan, "free");
        assert_eq!(usage.label, "Free");
        assert_eq!(usage.credits, Some(1.6));
        assert_eq!(usage.weekly_limit, 2.0);
        assert_eq!(usage.weekly_used, 0.40);
    }

    #[test]
    fn plus_plan_reports_credit_balance_and_monthly_grant() {
        let usage = compose_usage(
            "plus",
            "active",
            "pro",
            Some(10.5),
            1.25,
            0.0,
            2.0,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
        );
        assert_eq!(usage.plan, "plus");
        assert_eq!(usage.label, "Plus");
        assert_eq!(usage.credits, Some(10.5));
        assert_eq!(usage.weekly_limit, 22.0);
        assert_eq!(usage.weekly_used, 1.25);
        assert_eq!(usage.plan_tier, "pro");
        assert_eq!(usage.status, "active");
    }

    #[test]
    fn canceled_plus_is_treated_as_free() {
        let usage = compose_usage(
            "plus",
            "canceled",
            "pro",
            None,
            0.0,
            2.0,
            2.0,
            Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(),
            vec![],
        );
        assert_eq!(usage.plan, "free");
        assert_eq!(usage.credits, Some(0.0));
        assert_eq!(usage.weekly_limit, 2.0);
    }
}
