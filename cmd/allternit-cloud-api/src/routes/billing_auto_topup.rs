//! Auto top-up settings (`services::auto_topup` holds the trigger).
//!
//! All three verify the Clerk session (or a `billing`-scoped API token)
//! per-request, like the other billing routes:
//!
//! - `GET /api/v1/billing/auto-topup` — settings, state, saved card, the
//!   allowed amounts. When no card is stored yet but the person has a Stripe
//!   customer (a plan or an earlier pack), the newest card on it is picked
//!   up here.
//! - `PUT /api/v1/billing/auto-topup` — `{ enabled, threshold_usd,
//!   amount_usd, monthly_cap_usd }`; omitted fields keep their value.
//!   Threshold at least $5, amount one of the pack sizes, cap at least the
//!   amount and at most $5000. Turning it on needs a saved card (409
//!   `auto_topup_no_card`) and passes the same new-buyer limit as packs.
//!   While a chargeback is open or after one was lost it answers 409
//!   `auto_topup_dispute` (`services::billing_disputes`).
//! - `POST /api/v1/billing/auto-topup/setup` — a setup-mode Stripe Checkout
//!   to save a card; returns `{ checkout_url }`. The
//!   `checkout.session.completed` webhook stores the card.
//!
//! 503 `billing_not_configured` without `STRIPE_SECRET_KEY` (setup only;
//! GET reports `configured: false`).

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::billing_checkout::{
    billing_not_configured_response, billing_upstream_error_response, checkout_return_urls, pack_sizes_usd,
    ReqwestStripeCheckout, ReturnTo, StripeCheckout,
};
use crate::services::auto_topup::{self, AutoTopupRow, Settings};
use crate::services::{CostService, CostServiceImpl};
use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/billing/auto-topup", get(get_auto_topup).put(put_auto_topup))
        .route("/api/v1/billing/auto-topup/setup", post(create_setup_session))
}

#[derive(Debug, Default, Deserialize)]
pub struct AutoTopupUpdate {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    threshold_usd: Option<f64>,
    #[serde(default)]
    amount_usd: Option<f64>,
    #[serde(default)]
    monthly_cap_usd: Option<f64>,
}

/// The settings a PUT ends up saving: the request over the stored row over
/// the defaults.
fn merged_settings(row: Option<&AutoTopupRow>, update: &AutoTopupUpdate) -> Settings {
    Settings {
        enabled: update.enabled.or(row.map(|row| row.enabled)).unwrap_or(false),
        threshold_usd: update
            .threshold_usd
            .or(row.map(|row| row.threshold_usd))
            .unwrap_or(auto_topup::DEFAULT_THRESHOLD_USD),
        amount_usd: update
            .amount_usd
            .or(row.map(|row| row.amount_usd))
            .unwrap_or(auto_topup::DEFAULT_AMOUNT_USD),
        monthly_cap_usd: update
            .monthly_cap_usd
            .or(row.map(|row| row.monthly_cap_usd))
            .unwrap_or(auto_topup::DEFAULT_MONTHLY_CAP_USD),
    }
}

/// The JSON the client reads (`src/lib/billing.ts` `AutoTopup`).
fn view(row: Option<&AutoTopupRow>, configured: bool) -> Value {
    let now = Utc::now();
    let card = row.and_then(|row| {
        row.stripe_payment_method_id.as_ref().map(|_| {
            json!({
                "brand": row.card_brand.clone().unwrap_or_else(|| "card".to_string()),
                "last4": row.card_last4,
            })
        })
    });
    json!({
        "configured": configured,
        "enabled": row.map_or(false, |row| row.enabled),
        "status": row.map_or("disabled", |row| row.status.as_str()),
        "threshold_usd": row.map_or(auto_topup::DEFAULT_THRESHOLD_USD, |row| row.threshold_usd),
        "amount_usd": row.map_or(auto_topup::DEFAULT_AMOUNT_USD, |row| row.amount_usd),
        "monthly_cap_usd": row.map_or(auto_topup::DEFAULT_MONTHLY_CAP_USD, |row| row.monthly_cap_usd),
        "month_spent_usd": row.map_or(0.0, |row| auto_topup::spent_this_month(row, now)),
        "last_attempt_at": row.and_then(|row| row.last_attempt_at),
        "last_error": row.and_then(|row| row.last_error.clone()),
        "card": card,
        "amounts_usd": pack_sizes_usd(),
        "min_threshold_usd": auto_topup::MIN_THRESHOLD_USD,
        "max_monthly_cap_usd": auto_topup::MAX_MONTHLY_CAP_USD,
    })
}

async fn get_auto_topup(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let mut row = match auto_topup::load(&state.db, &user.id).await {
        Ok(row) => row,
        Err(error) => return error.into_response(),
    };
    let secret_key = std::env::var("STRIPE_SECRET_KEY").ok();
    // No card stored yet: a plan or an earlier pack may have saved one on
    // the person's Stripe customer. A Stripe hiccup only means "Add a card".
    let has_card = row.as_ref().map_or(false, |row| row.stripe_payment_method_id.is_some());
    if let (false, Some(secret_key)) = (has_card, secret_key.as_deref()) {
        if let Ok(Some(customer_id)) = auto_topup::linked_customer(&state.db, &user.id).await {
            let stripe = ReqwestStripeCheckout::new();
            match auto_topup::sync_card_from_customer(&state.db, &stripe, secret_key, &user.id, &customer_id).await {
                Ok(Some(_)) => {
                    row = auto_topup::load(&state.db, &user.id).await.ok().flatten();
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(user_id = %user.id, %error, "auto top-up: card lookup failed"),
            }
        }
    }
    Json(view(row.as_ref(), secret_key.is_some())).into_response()
}

async fn put_auto_topup(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(update): Json<AutoTopupUpdate>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let row = match auto_topup::load(&state.db, &user.id).await {
        Ok(row) => row,
        Err(error) => return error.into_response(),
    };
    let settings = merged_settings(row.as_ref(), &update);
    if let Err(error) = auto_topup::validate_settings(&settings, &pack_sizes_usd()) {
        return error.into_response();
    }
    if settings.enabled {
        if std::env::var("STRIPE_SECRET_KEY").is_err() {
            return billing_not_configured_response();
        }
        let has_card = row.as_ref().map_or(false, |row| {
            row.stripe_payment_method_id.is_some() && row.stripe_customer_id.is_some()
        });
        // A chargeback on this person's card keeps auto top-up off while it
        // is open and after it was lost (services::billing_disputes).
        match crate::services::billing_disputes::blocks_auto_topup(&state.db, &user.id).await {
            Ok(true) => {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "auto_topup_dispute",
                        "message": "Auto top-up is off because a payment was disputed. Contact support to turn it back on.",
                    })),
                )
                    .into_response();
            }
            Ok(false) => {}
            Err(error) => return error.into_response(),
        }
        if !has_card {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "auto_topup_no_card",
                    "message": "Add a card before turning on auto top-up.",
                })),
            )
                .into_response();
        }
        // Same new-buyer limit as packs: large amounts unlock once the first
        // payment has settled.
        let trusted = match super::billing_subscriptions::is_trusted_purchaser(&state.db, &user.id).await {
            Ok(trusted) => trusted,
            Err(error) => return error.into_response(),
        };
        if let Err(error) = super::billing_subscriptions::ensure_purchase_allowed(
            trusted,
            settings.amount_usd,
            "Larger auto top-up amounts",
        ) {
            return error.into_response();
        }
    }
    if let Err(error) = auto_topup::save_settings(&state.db, &user.id, &settings).await {
        return error.into_response();
    }
    crate::services::audit::write_audit_log(
        &state.db,
        crate::services::audit::AuditEvent {
            action: "billing.auto_topup.updated".to_string(),
            resource_type: "billing_auto_topup".to_string(),
            resource_id: Some(user.id.clone()),
            user_id: Some(user.id.clone()),
            user_email: None,
            details: Some(json!({
                "enabled": settings.enabled,
                "thresholdUsd": settings.threshold_usd,
                "amountUsd": settings.amount_usd,
                "monthlyCapUsd": settings.monthly_cap_usd,
            })),
            success: true,
        },
    )
    .await;
    // Already under the new threshold: top up now rather than at the next debit.
    if settings.enabled {
        let cost_service = CostServiceImpl::new(state.db.clone());
        if let Ok(balance) = cost_service.get_credit_balance(&user.id).await {
            auto_topup::spawn_check(state.db.clone(), user.id.clone(), balance);
        }
    }
    let row = auto_topup::load(&state.db, &user.id).await.ok().flatten();
    Json(view(row.as_ref(), true)).into_response()
}

/// Form for a setup-mode Checkout Session that saves a card for off-session use.
fn setup_form_params(customer_id: &str, clerk_user_id: &str, success_url: &str, cancel_url: &str) -> Vec<(String, String)> {
    vec![
        ("mode".to_string(), "setup".to_string()),
        ("customer".to_string(), customer_id.to_string()),
        ("payment_method_types[0]".to_string(), "card".to_string()),
        ("success_url".to_string(), success_url.to_string()),
        ("cancel_url".to_string(), cancel_url.to_string()),
        ("metadata[kind]".to_string(), "auto_topup_setup".to_string()),
        ("metadata[clerk_user_id]".to_string(), clerk_user_id.to_string()),
        ("setup_intent_data[metadata][kind]".to_string(), "auto_topup_setup".to_string()),
        ("setup_intent_data[metadata][clerk_user_id]".to_string(), clerk_user_id.to_string()),
    ]
}

async fn create_setup_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(return_to): Query<ReturnTo>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let (success_url, cancel_url) = match checkout_return_urls(&return_to) {
        Ok(urls) => urls,
        Err(error) => return error.into_response(),
    };
    let stripe = ReqwestStripeCheckout::new();
    let customer_id =
        match auto_topup::ensure_customer(&state.db, &stripe, &secret_key, &user.id, user.email.as_deref()).await {
            Ok(customer_id) => customer_id,
            Err(error) => return billing_upstream_error_response(&error),
        };
    let form = setup_form_params(&customer_id, &user.id, &success_url, &cancel_url);
    match stripe.create_checkout_session(&secret_key, &form).await {
        Ok(url) => Json(json!({ "checkout_url": url })).into_response(),
        Err(error) => billing_upstream_error_response(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_keeps_stored_values_for_omitted_fields() {
        let settings = merged_settings(None, &AutoTopupUpdate { enabled: Some(true), ..Default::default() });
        assert_eq!(
            settings,
            Settings { enabled: true, threshold_usd: 5.0, amount_usd: 25.0, monthly_cap_usd: 200.0 }
        );
        let update = AutoTopupUpdate { amount_usd: Some(50.0), ..Default::default() };
        let row = AutoTopupRow {
            user_id: "u".to_string(),
            enabled: true,
            threshold_usd: 10.0,
            amount_usd: 25.0,
            monthly_cap_usd: 300.0,
            stripe_customer_id: None,
            stripe_payment_method_id: None,
            card_brand: None,
            card_last4: None,
            status: "ok".to_string(),
            last_attempt_at: None,
            last_error: None,
            month_spent_usd: 0.0,
            month_key: String::new(),
            updated_at: Utc::now(),
        };
        let settings = merged_settings(Some(&row), &update);
        assert_eq!(
            settings,
            Settings { enabled: true, threshold_usd: 10.0, amount_usd: 50.0, monthly_cap_usd: 300.0 }
        );
    }

    #[test]
    fn view_without_a_row_is_off_with_defaults() {
        let value = view(None, true);
        assert_eq!(value["enabled"], json!(false));
        assert_eq!(value["status"], json!("disabled"));
        assert_eq!(value["card"], Value::Null);
        assert_eq!(value["amounts_usd"], json!([10.0, 25.0, 50.0, 100.0]));
        assert_eq!(value["monthly_cap_usd"], json!(200.0));
    }

    #[test]
    fn setup_session_is_setup_mode_with_the_customer() {
        let form = setup_form_params("cus_1", "user_1", "https://s", "https://c");
        assert!(form.contains(&("mode".to_string(), "setup".to_string())));
        assert!(form.contains(&("customer".to_string(), "cus_1".to_string())));
        assert!(form.contains(&("metadata[kind]".to_string(), "auto_topup_setup".to_string())));
        assert!(form.contains(&("metadata[clerk_user_id]".to_string(), "user_1".to_string())));
    }
}
