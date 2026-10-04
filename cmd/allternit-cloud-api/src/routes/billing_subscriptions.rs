//! Subscription plans (Plus / Super / Ultra): plan catalog, Stripe Checkout subscription
//! creation, and the Stripe customer billing portal.
//!
//! Plans are a static server-side catalog like `CREDIT_PACKS` in `routes::billing_checkout`:
//! prices, monthly credit grants, rollover caps, and the runtime plan tier each subscription
//! grants are fixed here. Only the Stripe price ids live in env (`STRIPE_PRICE_PLUS` / `_SUPER` /
//! `_ULTRA`), since they differ per Stripe account and mode; a missing price id answers 503
//! `billing_not_configured` exactly like a missing `STRIPE_SECRET_KEY`.
//!
//! - `GET /api/v1/billing/plans` (public): the catalog WITHOUT Stripe price ids.
//! - `GET /api/v1/billing/subscription` (Clerk session or API token): the caller's
//!   current Free / Plus / Super / Ultra plan (active Stripe row, else Free).
//! - `POST /api/v1/billing/subscribe` (Clerk session or API token): Checkout Session `mode=subscription` whose
//!   `subscription_data[metadata]` carries `clerk_user_id` / `allternit_plan_tier` /
//!   `allternit_plan_id` — the contract `routes::billing_webhooks` consumes.
//! - `POST /api/v1/billing/portal` (Clerk session or API token): billing-portal session for the user's Stripe
//!   customer, resolved through `user_billing_accounts`.
//!
//! This module also owns the subscription bookkeeping tables (created in
//! `migrations_pg/005_billing_subscriptions.sql`): the upsert/lookup helpers that `routes::billing_webhooks` calls so monthly
//! credit grants can resolve a Stripe subscription id back to a user and plan.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;

use crate::{
    error::ApiError,
    routes::billing_checkout::{
        billing_not_configured_response, billing_upstream_error_response, ReqwestStripeCheckout,
        StripeCheckout,
    },
    ApiState,
};

/// Default landing page after the customer leaves the Stripe billing portal.
const DEFAULT_PORTAL_RETURN_URL: &str = "https://platform.allternit.com/billing";

/// One subscription plan. `price_usd` is informational (Stripe is the source of truth for
/// charging); `monthly_credits_usd` and `rollover_cap_usd` drive the monthly credit grant, and
/// `plan_tier` is the runtime quota tier granted while the subscription is active.
#[derive(Debug, Clone, Copy)]
pub struct SubscriptionPlan {
    pub id: &'static str,
    pub label: &'static str,
    pub price_usd: f64,
    pub monthly_credits_usd: f64,
    pub rollover_cap_usd: f64,
    pub plan_tier: &'static str,
    /// Env var holding this plan's Stripe price id (differs per Stripe account/mode).
    pub price_env: &'static str,
    /// Photo avatars Allternit pays for each UTC calendar month (`routes::photo_avatars`).
    /// No active subscription gets 0.
    pub photo_avatars_per_month: i64,
}

const SUBSCRIPTION_PLANS: &[SubscriptionPlan] = &[
    SubscriptionPlan { id: "plus", label: "Plus", price_usd: 20.00, monthly_credits_usd: 22.00, rollover_cap_usd: 10.00, plan_tier: "pro", price_env: "STRIPE_PRICE_PLUS", photo_avatars_per_month: 5 },
    SubscriptionPlan { id: "super", label: "Super", price_usd: 100.00, monthly_credits_usd: 110.00, rollover_cap_usd: 50.00, plan_tier: "team", price_env: "STRIPE_PRICE_SUPER", photo_avatars_per_month: 25 },
    SubscriptionPlan { id: "ultra", label: "Ultra", price_usd: 200.00, monthly_credits_usd: 220.00, rollover_cap_usd: 100.00, plan_tier: "team", price_env: "STRIPE_PRICE_ULTRA", photo_avatars_per_month: 50 },
];

/// Wire shape of the plan catalog — Stripe price ids deliberately stay server-side.
#[derive(Debug, Serialize)]
pub struct SubscriptionPlanResponse {
    id: &'static str,
    label: &'static str,
    price_usd: f64,
    monthly_credits_usd: f64,
    rollover_cap_usd: f64,
    plan_tier: &'static str,
}

#[derive(Debug, Serialize)]
pub struct PlansResponse {
    plans: Vec<SubscriptionPlanResponse>,
}

#[derive(Debug, Deserialize)]
pub struct SubscribeRequest {
    plan_id: String,
}

#[derive(Debug, Serialize)]
pub struct SubscribeResponse {
    checkout_url: String,
}

#[derive(Debug, Serialize)]
pub struct PortalResponse {
    portal_url: String,
}

#[derive(Debug, Serialize)]
pub struct CurrentSubscriptionResponse {
    plan_id: String,
    label: String,
    plan_tier: String,
    status: String,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/billing/plans", get(list_plans))
        .route("/api/v1/billing/subscription", get(get_current_subscription))
        .route("/api/v1/billing/subscribe", post(create_subscription))
        .route("/api/v1/billing/portal", post(create_portal_session))
}

fn product_label(plan_id: &str) -> &'static str {
    match plan_id {
        "plus" => "Plus",
        "super" => "Super",
        "ultra" => "Ultra",
        _ => "Free",
    }
}

async fn get_current_subscription(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<CurrentSubscriptionResponse>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "billing")
        .await?
        .id;
    if crate::auth::is_admin_user(&user_id) {
        return Ok(Json(CurrentSubscriptionResponse {
            plan_id: "ultra".to_string(),
            label: "Ultra".to_string(),
            plan_tier: "team".to_string(),
            status: "active".to_string(),
        }));
    }
    let row: Option<BillingSubscription> = sqlx::query_as(
        r#"
        SELECT user_id, plan_id, plan_tier, status, stripe_customer_id
        FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing', 'past_due', 'unpaid')
        ORDER BY (status IN ('active', 'trialing')) DESC, updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(Json(match row {
        Some(sub) => CurrentSubscriptionResponse {
            label: product_label(&sub.plan_id).to_string(),
            plan_id: sub.plan_id,
            plan_tier: sub.plan_tier,
            status: sub.status,
        },
        None => CurrentSubscriptionResponse {
            plan_id: "free".to_string(),
            label: "Free".to_string(),
            plan_tier: "free".to_string(),
            status: "none".to_string(),
        },
    }))
}

async fn list_plans() -> Json<PlansResponse> {
    Json(PlansResponse {
        plans: SUBSCRIPTION_PLANS
            .iter()
            .map(|plan| SubscriptionPlanResponse {
                id: plan.id,
                label: plan.label,
                price_usd: plan.price_usd,
                monthly_credits_usd: plan.monthly_credits_usd,
                rollover_cap_usd: plan.rollover_cap_usd,
                plan_tier: plan.plan_tier,
            })
            .collect(),
    })
}

async fn create_subscription(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(return_to): Query<crate::routes::billing_checkout::ReturnTo>,
    Json(request): Json<SubscribeRequest>,
) -> Response {
    let user_id = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user.id,
        Err(error) => return error.into_response(),
    };
    let Some(plan) = find_plan(&request.plan_id) else {
        return ApiError::BadRequest(format!("Unknown subscription plan: {:?}.", request.plan_id))
            .into_response();
    };
    // Chargeback hold: untrusted buyers start on the smallest plan; the larger
    // ones unlock automatically once their first purchase settles.
    let trusted = match is_trusted_purchaser(&state.db, &user_id).await {
        Ok(trusted) => trusted,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = ensure_purchase_allowed(trusted, plan.price_usd, "The Super and Ultra plans") {
        return error.into_response();
    }
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let Some(price_id) = plan_price_id(plan) else {
        return billing_not_configured_response();
    };
    let (success_url, cancel_url) =
        match crate::routes::billing_checkout::checkout_return_urls(&return_to) {
            Ok(urls) => urls,
            Err(error) => return error.into_response(),
        };
    // One subscription per account: a second checkout would bill twice and
    // create a second cloud computer. Plan changes go through the portal.
    match open_subscription_plan(&state.db, &user_id).await {
        Ok(Some(current_plan_id)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "already_subscribed",
                    "current_plan_id": current_plan_id,
                    "message": "This account already has a plan. Change or cancel it in Manage billing.",
                })),
            )
                .into_response();
        }
        Ok(None) => {}
        Err(error) => return error.into_response(),
    }
    let customer_id: Option<String> = match sqlx::query_scalar(
        "SELECT stripe_customer_id FROM user_billing_accounts WHERE user_id = $1",
    )
    .bind(&user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(customer_id) => customer_id,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let checkout = ReqwestStripeCheckout::new();

    match create_subscription_checkout_url(
        &checkout,
        &secret_key,
        plan,
        &price_id,
        &user_id,
        customer_id.as_deref(),
        &success_url,
        &cancel_url,
    )
    .await
    {
        Ok(url) => Json(SubscribeResponse { checkout_url: url }).into_response(),
        Err(error) => billing_upstream_error_response(&error),
    }
}

async fn create_portal_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(return_to): Query<crate::routes::billing_checkout::ReturnTo>,
) -> Response {
    let user_id = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user.id,
        Err(error) => return error.into_response(),
    };
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let return_url = match crate::routes::billing_checkout::client_return_url(
        return_to.return_url.as_deref(),
    ) {
        Ok(Some(url)) => url,
        Ok(None) => std::env::var("STRIPE_PORTAL_RETURN_URL")
            .unwrap_or_else(|_| DEFAULT_PORTAL_RETURN_URL.to_string()),
        Err(error) => return error.into_response(),
    };
    let checkout = ReqwestStripeCheckout::new();

    match portal_url_for(&checkout, &state.db, &user_id, &secret_key, &return_url).await {
        Ok(url) => Json(PortalResponse { portal_url: url }).into_response(),
        Err(PortalError::NoCustomer) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "billing_no_customer" })),
        )
            .into_response(),
        Err(PortalError::Upstream(error)) => billing_upstream_error_response(&error),
    }
}

/// Look up a plan in the static catalog. Unknown ids are a 400 at the endpoint and an ignored
/// grant in the webhook — the server never invents plan terms for ids it does not sell.
pub(crate) fn find_plan(plan_id: &str) -> Option<&'static SubscriptionPlan> {
    SUBSCRIPTION_PLANS.iter().find(|plan| plan.id == plan_id)
}

/// The plan a Stripe subscription is on *now*: its item's price id, mapped
/// through the `STRIPE_PRICE_*` env, falling back to the `allternit_plan_id`
/// metadata stamped at checkout. The price wins because a plan switch in the
/// Stripe billing portal changes the price but leaves the metadata as it was.
pub(crate) fn current_subscription_plan(
    subscription: &serde_json::Value,
) -> Option<&'static SubscriptionPlan> {
    let price_id = subscription["items"]["data"][0]["price"]["id"]
        .as_str()
        .or_else(|| subscription["plan"]["id"].as_str());
    price_id
        .and_then(|price_id| {
            SUBSCRIPTION_PLANS
                .iter()
                .find(|plan| plan_price_id(plan).as_deref() == Some(price_id))
        })
        .or_else(|| {
            subscription["metadata"]["allternit_plan_id"]
                .as_str()
                .and_then(find_plan)
        })
}

/// Resolve the Stripe price id for a plan from its env var. Empty values count as unset so a
/// placeholder `STRIPE_PRICE_PLUS=` in a deployment manifest surfaces as billing_not_configured
/// instead of a cryptic Stripe error.
fn plan_price_id(plan: &SubscriptionPlan) -> Option<String> {
    std::env::var(plan.price_env)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Build the subscription-mode Checkout Session for a validated plan and return its hosted URL.
#[allow(clippy::too_many_arguments)]
async fn create_subscription_checkout_url(
    checkout: &dyn StripeCheckout,
    secret_key: &str,
    plan: &SubscriptionPlan,
    price_id: &str,
    clerk_user_id: &str,
    customer_id: Option<&str>,
    success_url: &str,
    cancel_url: &str,
) -> Result<String, ApiError> {
    let mut form = subscribe_form_params(plan, price_id, clerk_user_id, success_url, cancel_url);
    // Reuse the account's Stripe customer (from an earlier plan or credit
    // pack), so one person stays one customer and the portal shows it all.
    if let Some(customer_id) = customer_id {
        form.push(("customer".to_string(), customer_id.to_string()));
    }
    checkout.create_checkout_session(secret_key, &form).await
}

/// The plan of the account's open subscription, if it has one. `past_due`
/// and `unpaid` count: the subscription still exists and comes back when
/// the card is fixed.
async fn open_subscription_plan(db: &PgPool, user_id: &str) -> Result<Option<String>, ApiError> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT plan_id FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing', 'past_due', 'unpaid')
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?)
}

/// Form fields for a subscription Checkout Session. The metadata lives under
/// `subscription_data` so it lands on the *subscription* object — the webhook's
/// `customer.subscription.*` mapping reads `clerk_user_id` / `allternit_plan_tier` /
/// `allternit_plan_id` from there, so these names must stay stable.
fn subscribe_form_params(
    plan: &SubscriptionPlan,
    price_id: &str,
    clerk_user_id: &str,
    success_url: &str,
    cancel_url: &str,
) -> Vec<(String, String)> {
    vec![
        ("mode".to_string(), "subscription".to_string()),
        // Pay-now methods only (cards, incl. Apple Pay / Google Pay, and
        // Link): no buy-now-pay-later or delayed methods for Allternit Cloud.
        // Services Payment Links keep the account's wider method list.
        ("payment_method_types[0]".to_string(), "card".to_string()),
        ("payment_method_types[1]".to_string(), "link".to_string()),
        ("success_url".to_string(), success_url.to_string()),
        ("cancel_url".to_string(), cancel_url.to_string()),
        ("line_items[0][price]".to_string(), price_id.to_string()),
        ("line_items[0][quantity]".to_string(), "1".to_string()),
        (
            "subscription_data[metadata][clerk_user_id]".to_string(),
            clerk_user_id.to_string(),
        ),
        (
            "subscription_data[metadata][allternit_plan_tier]".to_string(),
            plan.plan_tier.to_string(),
        ),
        (
            "subscription_data[metadata][allternit_plan_id]".to_string(),
            plan.id.to_string(),
        ),
    ]
}

/// Why a portal session could not be created: the user has no Stripe customer link yet (404),
/// or Stripe/config failed (502).
#[derive(Debug)]
enum PortalError {
    NoCustomer,
    Upstream(ApiError),
}

/// Resolve the user's Stripe customer through `user_billing_accounts` and open a billing-portal
/// session. Users who never subscribed or bought credits have no customer row — a 404, not an
/// empty success, so the client can show "subscribe first".
async fn portal_url_for(
    checkout: &dyn StripeCheckout,
    db: &PgPool,
    user_id: &str,
    secret_key: &str,
    return_url: &str,
) -> Result<String, PortalError> {
    let customer: Option<String> = sqlx::query_scalar(
        "SELECT stripe_customer_id FROM user_billing_accounts WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(|error| PortalError::Upstream(ApiError::DatabaseError(error)))?;
    let Some(customer) = customer else {
        return Err(PortalError::NoCustomer);
    };
    let form = portal_form_params(&customer, return_url);
    checkout
        .create_billing_portal_session(secret_key, &form)
        .await
        .map_err(PortalError::Upstream)
}

fn portal_form_params(customer_id: &str, return_url: &str) -> Vec<(String, String)> {
    vec![
        ("customer".to_string(), customer_id.to_string()),
        ("return_url".to_string(), return_url.to_string()),
    ]
}

/// One `billing_subscriptions` row: what the webhook needs to grant monthly credits and keep
/// the local mirror of the Stripe subscription lifecycle current.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct BillingSubscription {
    pub user_id: String,
    pub plan_id: String,
    pub plan_tier: String,
    pub status: String,
    pub stripe_customer_id: Option<String>,
}

/// Upsert the local mirror of a Stripe subscription from a `customer.subscription.*` event.
pub(crate) async fn upsert_billing_subscription(
    db: &PgPool,
    stripe_subscription_id: &str,
    user_id: &str,
    plan_id: &str,
    plan_tier: &str,
    status: &str,
    stripe_customer_id: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO billing_subscriptions (
            stripe_subscription_id, user_id, plan_id, plan_tier, status, stripe_customer_id, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, CURRENT_TIMESTAMP)
        ON CONFLICT (stripe_subscription_id) DO UPDATE SET
            user_id = excluded.user_id,
            plan_id = excluded.plan_id,
            plan_tier = excluded.plan_tier,
            status = excluded.status,
            stripe_customer_id = excluded.stripe_customer_id,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(stripe_subscription_id)
    .bind(user_id)
    .bind(plan_id)
    .bind(plan_tier)
    .bind(status)
    .bind(stripe_customer_id)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// Link a Clerk user id to their Stripe customer id (portal sessions, future support tooling).
pub(crate) async fn upsert_user_billing_account(
    db: &PgPool,
    user_id: &str,
    stripe_customer_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO user_billing_accounts (user_id, stripe_customer_id, updated_at)
        VALUES ($1, $2, CURRENT_TIMESTAMP)
        ON CONFLICT (user_id) DO UPDATE SET
            stripe_customer_id = excluded.stripe_customer_id,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(user_id)
    .bind(stripe_customer_id)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// Mark the local subscription mirror canceled on `customer.subscription.deleted`. A missing
/// row (deletion delivered before creation) is not an error.
pub(crate) async fn mark_billing_subscription_canceled(
    db: &PgPool,
    stripe_subscription_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE billing_subscriptions SET status = 'canceled', updated_at = CURRENT_TIMESTAMP WHERE stripe_subscription_id = $1",
    )
    .bind(stripe_subscription_id)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// Record a failed-renewal status (`past_due` / `unpaid`) on the local mirror. A missing row
/// is not an error; a later active/trialing grant overwrites the status.
pub(crate) async fn set_billing_subscription_status(
    db: &PgPool,
    stripe_subscription_id: &str,
    status: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE billing_subscriptions SET status = $2, updated_at = CURRENT_TIMESTAMP WHERE stripe_subscription_id = $1",
    )
    .bind(stripe_subscription_id)
    .bind(status)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// Look up the local mirror for a Stripe subscription id (used by the invoice.paid grant path).
pub(crate) async fn billing_subscription_for(
    db: &PgPool,
    stripe_subscription_id: &str,
) -> Result<Option<BillingSubscription>, ApiError> {
    sqlx::query_as::<_, BillingSubscription>(
        r#"
        SELECT user_id, plan_id, plan_tier, status, stripe_customer_id
        FROM billing_subscriptions
        WHERE stripe_subscription_id = $1
        "#,
    )
    .bind(stripe_subscription_id)
    .fetch_optional(db)
    .await
    .map_err(ApiError::DatabaseError)
}

/// Chargeback-hold window in days (`CHARGEBACK_HOLD_DAYS`, default 14): how
/// long a first purchase must settle before the buyer is trusted with larger
/// packs/plans.
fn chargeback_hold_days() -> i64 {
    std::env::var("CHARGEBACK_HOLD_DAYS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(14)
}

/// Maximum first-purchase size in USD for untrusted buyers
/// (`FIRST_PURCHASE_MAX_USD`, default 25).
fn first_purchase_max_usd() -> f64 {
    std::env::var("FIRST_PURCHASE_MAX_USD")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| *value >= 0.0)
        .unwrap_or(25.0)
}

/// Record one successful paid purchase for the chargeback-hold trust table.
/// Called from the credit-grant webhook path only when the grant was fresh
/// (Stripe retries replaying the same event do not inflate the count).
pub(crate) async fn record_paid_purchase(db: &PgPool, user_id: &str) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO billing_purchase_trust (user_id, first_paid_at, paid_purchase_count)
        VALUES ($1, NOW(), 1)
        ON CONFLICT (user_id) DO UPDATE SET
            paid_purchase_count = billing_purchase_trust.paid_purchase_count + 1,
            first_paid_at = COALESCE(billing_purchase_trust.first_paid_at, NOW())
        "#,
    )
    .bind(user_id)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// Trusted = at least one paid purchase whose first purchase is older than the
/// chargeback-hold window. Users with no row are untrusted.
pub(crate) async fn is_trusted_purchaser(db: &PgPool, user_id: &str) -> Result<bool, ApiError> {
    let trusted: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT paid_purchase_count >= 1 AND first_paid_at <= NOW() - $2 * INTERVAL '1 day'
        FROM billing_purchase_trust
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .bind(chargeback_hold_days())
    .fetch_optional(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(trusted.unwrap_or(false))
}

/// Chargeback-hold gate shared by credit packs and subscriptions: untrusted
/// buyers are limited to small first purchases while their payment history
/// settles. `unlock_hint` names what the gate unlocks (packs or plans).
pub(crate) fn ensure_purchase_allowed(trusted: bool, price_usd: f64, unlock_hint: &str) -> Result<(), ApiError> {
    if !trusted && price_usd > first_purchase_max_usd() {
        return Err(ApiError::Forbidden(format!(
            "First purchases are limited to ${:.0} while your payment history settles ({} days). {} unlock automatically.",
            first_purchase_max_usd(),
            chargeback_hold_days(),
            unlock_hint,
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Guards env mutation in tests that resolve plan price ids from `STRIPE_PRICE_*`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    async fn test_pool() -> PgPool {
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
        sqlx::query("DROP TABLE IF EXISTS user_billing_accounts CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE user_billing_accounts (
                user_id TEXT PRIMARY KEY,
                stripe_customer_id TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS billing_purchase_trust CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE billing_purchase_trust (
                user_id TEXT PRIMARY KEY,
                first_paid_at TIMESTAMPTZ,
                paid_purchase_count INT NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    struct RecordingStripe {
        checkout_form: Mutex<Option<Vec<(String, String)>>>,
        portal_form: Mutex<Option<Vec<(String, String)>>>,
    }

    impl RecordingStripe {
        fn new() -> Self {
            Self {
                checkout_form: Mutex::new(None),
                portal_form: Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl StripeCheckout for RecordingStripe {
        async fn create_checkout_session(
            &self,
            _secret_key: &str,
            form: &[(String, String)],
        ) -> Result<String, ApiError> {
            *self.checkout_form.lock().unwrap() = Some(form.to_vec());
            Ok("https://checkout.stripe.com/c/pay/test_sub".to_string())
        }

        async fn create_billing_portal_session(
            &self,
            _secret_key: &str,
            form: &[(String, String)],
        ) -> Result<String, ApiError> {
            *self.portal_form.lock().unwrap() = Some(form.to_vec());
            Ok("https://billing.stripe.com/p/session/test_portal".to_string())
        }
    }

    fn field(form: &[(String, String)], key: &str) -> String {
        form.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap()
    }

    #[test]
    fn plan_catalog_is_the_three_static_plans() {
        assert_eq!(SUBSCRIPTION_PLANS.len(), 3);
        let plus = find_plan("plus").unwrap();
        assert_eq!(plus.price_usd, 20.00);
        assert_eq!(plus.monthly_credits_usd, 22.00);
        assert_eq!(plus.rollover_cap_usd, 10.00);
        assert_eq!(plus.plan_tier, "pro");
        assert_eq!(plus.price_env, "STRIPE_PRICE_PLUS");
        let super_plan = find_plan("super").unwrap();
        assert_eq!(super_plan.monthly_credits_usd, 110.00);
        assert_eq!(super_plan.rollover_cap_usd, 50.00);
        assert_eq!(super_plan.plan_tier, "team");
        let ultra = find_plan("ultra").unwrap();
        assert_eq!(ultra.monthly_credits_usd, 220.00);
        assert_eq!(ultra.rollover_cap_usd, 100.00);
        assert_eq!(ultra.plan_tier, "team");
    }

    #[test]
    fn plan_validation_rejects_unknown_ids() {
        for id in ["pro", "PLUS", "", "plus ", "credits_10"] {
            assert!(find_plan(id).is_none(), "{id:?} must not be a known plan");
        }
    }

    #[tokio::test]
    async fn plans_endpoint_serializes_catalog_without_stripe_price_ids() {
        let Json(plans) = list_plans().await;
        let value = serde_json::to_value(&plans).unwrap();
        assert_eq!(value["plans"].as_array().unwrap().len(), 3);
        let first = &value["plans"][0];
        assert_eq!(first["id"], serde_json::json!("plus"));
        assert_eq!(first["price_usd"], serde_json::json!(20.0));
        assert_eq!(first["monthly_credits_usd"], serde_json::json!(22.0));
        assert_eq!(first["rollover_cap_usd"], serde_json::json!(10.0));
        assert_eq!(first["plan_tier"], serde_json::json!("pro"));
        assert!(first.get("price_id").is_none(), "Stripe price ids stay server-side");
        assert!(first.get("price_env").is_none(), "env var names stay server-side");
    }

    #[tokio::test]
    async fn subscribe_builds_subscription_form_with_price_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("STRIPE_PRICE_PLUS", "price_test_plus");
        let plan = find_plan("plus").unwrap();
        let price_id = plan_price_id(plan).expect("env price id must resolve");
        let checkout = RecordingStripe::new();
        let url = create_subscription_checkout_url(
            &checkout, "sk_test", plan, &price_id, "user_1", None, "https://s.example", "https://c.example",
        )
        .await
        .unwrap();
        assert_eq!(url, "https://checkout.stripe.com/c/pay/test_sub");
        let form = checkout.checkout_form.lock().unwrap().clone().unwrap();
        assert!(form.iter().all(|(key, _)| key != "customer"));
        assert_eq!(field(&form, "mode"), "subscription");
        assert_eq!(field(&form, "payment_method_types[0]"), "card", "pay-now methods only");
        assert_eq!(field(&form, "payment_method_types[1]"), "link");
        assert_eq!(field(&form, "line_items[0][price]"), "price_test_plus");
        assert_eq!(field(&form, "line_items[0][quantity]"), "1");
        assert_eq!(field(&form, "subscription_data[metadata][clerk_user_id]"), "user_1");
        assert_eq!(field(&form, "subscription_data[metadata][allternit_plan_tier]"), "pro");
        assert_eq!(field(&form, "subscription_data[metadata][allternit_plan_id]"), "plus");
        assert_eq!(field(&form, "success_url"), "https://s.example");
        assert_eq!(field(&form, "cancel_url"), "https://c.example");
        std::env::remove_var("STRIPE_PRICE_PLUS");
    }

    #[tokio::test]
    async fn subscribe_reuses_the_accounts_stripe_customer() {
        let _guard = ENV_LOCK.lock().unwrap();
        let plan = find_plan("plus").unwrap();
        let checkout = RecordingStripe::new();
        create_subscription_checkout_url(
            &checkout, "sk_test", plan, "price_x", "user_1", Some("cus_123"), "https://s.example", "https://c.example",
        )
        .await
        .unwrap();
        let form = checkout.checkout_form.lock().unwrap().clone().unwrap();
        assert_eq!(field(&form, "customer"), "cus_123");
    }

    #[test]
    fn current_plan_follows_the_price_not_the_checkout_metadata() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("STRIPE_PRICE_PLUS", "price_plus");
        std::env::set_var("STRIPE_PRICE_SUPER", "price_super");
        // Bought Plus, switched to Super in the billing portal: the metadata
        // still says plus, the item's price says super.
        let switched = serde_json::json!({
            "metadata": { "allternit_plan_id": "plus", "allternit_plan_tier": "pro" },
            "items": { "data": [ { "price": { "id": "price_super" } } ] },
        });
        let plan = current_subscription_plan(&switched).unwrap();
        assert_eq!((plan.id, plan.plan_tier), ("super", "team"));
        // An unknown price falls back to the metadata.
        let unknown = serde_json::json!({
            "metadata": { "allternit_plan_id": "plus" },
            "items": { "data": [ { "price": { "id": "price_other" } } ] },
        });
        assert_eq!(current_subscription_plan(&unknown).unwrap().id, "plus");
        std::env::remove_var("STRIPE_PRICE_PLUS");
        std::env::remove_var("STRIPE_PRICE_SUPER");
    }

    #[test]
    fn plan_price_id_treats_missing_and_blank_env_as_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        let plan = find_plan("plus").unwrap();
        std::env::remove_var("STRIPE_PRICE_PLUS");
        assert!(plan_price_id(plan).is_none());
        std::env::set_var("STRIPE_PRICE_PLUS", "  ");
        assert!(plan_price_id(plan).is_none(), "blank values count as unset");
        std::env::remove_var("STRIPE_PRICE_PLUS");
    }

    #[tokio::test]
    async fn portal_without_customer_row_is_no_customer() {
        let pool = test_pool().await;
        let checkout = RecordingStripe::new();
        let error = portal_url_for(&checkout, &pool, "user_1", "sk_test", "https://platform.allternit.com/billing")
            .await
            .unwrap_err();
        assert!(matches!(error, PortalError::NoCustomer));
        assert!(checkout.portal_form.lock().unwrap().is_none(), "no Stripe call without a customer");
    }

    #[tokio::test]
    async fn portal_with_customer_row_returns_portal_url() {
        let pool = test_pool().await;
        upsert_user_billing_account(&pool, "user_1", "cus_123").await.unwrap();
        let checkout = RecordingStripe::new();
        let url = portal_url_for(&checkout, &pool, "user_1", "sk_test", "https://return.example")
            .await
            .unwrap();
        assert_eq!(url, "https://billing.stripe.com/p/session/test_portal");
        let form = checkout.portal_form.lock().unwrap().clone().unwrap();
        assert_eq!(field(&form, "customer"), "cus_123");
        assert_eq!(field(&form, "return_url"), "https://return.example");
    }

    #[tokio::test]
    async fn billing_account_upsert_updates_the_customer_link() {
        let pool = test_pool().await;
        upsert_user_billing_account(&pool, "user_1", "cus_old").await.unwrap();
        upsert_user_billing_account(&pool, "user_1", "cus_new").await.unwrap();
        let customer: String = sqlx::query_scalar(
            "SELECT stripe_customer_id FROM user_billing_accounts WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(customer, "cus_new");
    }

    #[tokio::test]
    async fn paid_purchase_upsert_counts_and_keeps_the_first_timestamp() {
        let pool = test_pool().await;
        record_paid_purchase(&pool, "user_1").await.unwrap();
        record_paid_purchase(&pool, "user_1").await.unwrap();

        let (count, first_paid_at): (i32, Option<chrono::DateTime<chrono::Utc>>) =
            sqlx::query_as(
                "SELECT paid_purchase_count, first_paid_at FROM billing_purchase_trust WHERE user_id = 'user_1'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        assert!(first_paid_at.is_some());

        // A later purchase must not move first_paid_at backwards/forwards.
        let before = first_paid_at.unwrap();
        record_paid_purchase(&pool, "user_1").await.unwrap();
        let after: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT first_paid_at FROM billing_purchase_trust WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, Some(before));
    }

    #[tokio::test]
    async fn trust_requires_a_settled_first_purchase() {
        let pool = test_pool().await;
        assert!(
            !is_trusted_purchaser(&pool, "user_none").await.unwrap(),
            "no row is untrusted"
        );

        record_paid_purchase(&pool, "user_fresh").await.unwrap();
        assert!(
            !is_trusted_purchaser(&pool, "user_fresh").await.unwrap(),
            "a purchase inside the 14-day hold is untrusted"
        );

        sqlx::query(
            "UPDATE billing_purchase_trust SET first_paid_at = NOW() - INTERVAL '20 days' WHERE user_id = 'user_fresh'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            is_trusted_purchaser(&pool, "user_fresh").await.unwrap(),
            "a first purchase older than the hold window is trusted"
        );
    }

    #[test]
    fn purchase_gate_limits_untrusted_buyers_to_the_first_purchase_max() {
        // Untrusted: $20 Plus and $25 pack pass, $50 pack / $100 Super blocked.
        assert!(ensure_purchase_allowed(false, 20.0, "The Super and Ultra plans").is_ok());
        assert!(ensure_purchase_allowed(false, 25.0, "The $50 and $100 packs").is_ok());
        let error = ensure_purchase_allowed(false, 50.0, "The $50 and $100 packs").unwrap_err();
        assert!(error.to_string().contains("First purchases are limited to $25"), "{error}");
        assert!(error.to_string().contains("$50 and $100 packs unlock automatically"), "{error}");
        assert!(ensure_purchase_allowed(false, 100.0, "The Super and Ultra plans").is_err());
        // Trusted buyers are never gated.
        assert!(ensure_purchase_allowed(true, 100.0, "The $50 and $100 packs").is_ok());
        assert!(ensure_purchase_allowed(true, 200.0, "The Super and Ultra plans").is_ok());
    }
}
