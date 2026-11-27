//! Subscription plans (Plus / Max / Ultra; the Max plan keeps the id `super`): plan catalog, Stripe Checkout subscription
//! creation, and the Stripe customer billing portal.
//!
//! Plans are a static server-side catalog like `CREDIT_PACKS` in `routes::billing_checkout`:
//! prices, monthly credit grants, rollover caps, and the runtime plan tier each subscription
//! grants are fixed here. Only the Stripe price ids live in env (`STRIPE_PRICE_PLUS` / `_SUPER` /
//! `_ULTRA`), since they differ per Stripe account and mode; a missing price id answers 503
//! `billing_not_configured` exactly like a missing `STRIPE_SECRET_KEY`.
//!
//! - `GET /api/v1/billing/plans` (public): the catalog WITHOUT Stripe price ids, plus a
//!   top-level `freeweights` block listing the Freeweights plan perks
//!   (`routes::freeweights::PerkConfig::listing`; `status` is `not_offered` unless
//!   `FREEWEIGHTS_PERKS_ENABLED` is on). Each plan carries a `store` block
//!   (`services::store_billing::StoreOffers`): its App Store and Play product ids and
//!   whether that store is switched on.
//! - `GET /api/v1/billing/subscription` (Clerk session or API token): the caller's
//!   current Free / Plus / Max / Ultra plan (open row, else Free), with
//!   `billing_provider` (`stripe` / `apple` / `google` / null), `manage_url` and
//!   `conflict` (more than one open plan). First re-reads an App Store or
//!   Google Play plan whose period end has passed (at most once per 10 minutes
//!   per row; plan D13, `services::store_apple`, `services::store_google`).
//! - `POST /api/v1/billing/subscribe` (Clerk session or API token): Checkout Session `mode=subscription` whose
//!   `subscription_data[metadata]` carries `clerk_user_id` / `allternit_plan_tier` /
//!   `allternit_plan_id` — the contract `routes::billing_webhooks` consumes. 409
//!   `already_subscribed` (with `billing_provider`, `manage_url`) when any plan is open.
//!   Optional body `"ui_mode": "embedded"` answers `{ client_secret, session_id }` for an
//!   in-app Stripe.js form instead of the hosted `{ checkout_url }` (see
//!   `routes::billing_checkout`); the Stripe customer is never taken from the body.
//! - `POST /api/v1/billing/portal` (Clerk session or API token): billing-portal session for the user's Stripe
//!   customer, resolved through `user_billing_accounts`. 409 `store_billed` when the
//!   account's plan is billed by Apple or Google (`routes::billing_store`).
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
        billing_not_configured_response, billing_upstream_error_response, checkout_session_response,
        open_checkout_session, payment_method_fields, CheckoutReturn, CheckoutSession, ReqwestStripeCheckout,
        StripeCheckout, UiMode,
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
    /// App Store auto-renewable subscription product id (group "Allternit Plans").
    /// Permanent once created in App Store Connect.
    pub apple_product_id: &'static str,
    /// Google Play subscription product id; its one base plan is `monthly`.
    pub google_product_id: &'static str,
    /// The store's US reference price. Store plans grant the same monthly
    /// credits as web; the price is higher to cover the store fee. Clients show
    /// the store's own localized price, never this figure.
    pub store_price_usd: f64,
}

const SUBSCRIPTION_PLANS: &[SubscriptionPlan] = &[
    SubscriptionPlan { id: "plus", label: "Plus", price_usd: 20.00, monthly_credits_usd: 22.00, rollover_cap_usd: 10.00, plan_tier: "pro", price_env: "STRIPE_PRICE_PLUS", photo_avatars_per_month: 5, apple_product_id: "com.allternit.mobile.plan.plus.monthly", google_product_id: "plan_plus", store_price_usd: 24.99 },
    SubscriptionPlan { id: "super", label: "Max", price_usd: 100.00, monthly_credits_usd: 110.00, rollover_cap_usd: 50.00, plan_tier: "team", price_env: "STRIPE_PRICE_SUPER", photo_avatars_per_month: 25, apple_product_id: "com.allternit.mobile.plan.max.monthly", google_product_id: "plan_max", store_price_usd: 124.99 },
    SubscriptionPlan { id: "ultra", label: "Ultra", price_usd: 200.00, monthly_credits_usd: 220.00, rollover_cap_usd: 100.00, plan_tier: "team", price_env: "STRIPE_PRICE_ULTRA", photo_avatars_per_month: 50, apple_product_id: "com.allternit.mobile.plan.ultra.monthly", google_product_id: "plan_ultra", store_price_usd: 249.99 },
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
    /// Where the apps can buy this plan. Additive: older clients ignore it.
    store: crate::services::store_billing::StoreOffers,
}

#[derive(Debug, Serialize)]
pub struct PlansResponse {
    plans: Vec<SubscriptionPlanResponse>,
    /// Freeweights perks per plan (free, plus, super, ultra), listed whether or not they are
    /// offered yet. Additive: older clients ignore it.
    freeweights: crate::routes::freeweights::PerkListing,
}

/// The body names only the plan and the ui mode (`hosted` default, or
/// `embedded`; see `routes::billing_checkout`). There is no customer field:
/// the Stripe customer comes from the signed-in account's
/// `user_billing_accounts` row, and unknown fields are ignored.
#[derive(Debug, Deserialize)]
pub struct SubscribeRequest {
    plan_id: String,
    #[serde(default)]
    ui_mode: Option<UiMode>,
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
    /// When the plan renews (RFC 3339), once a Stripe event has reported it.
    #[serde(skip_serializing_if = "Option::is_none")]
    renews_at: Option<String>,
}

/// `GET /api/v1/billing/subscription`: the plan plus who bills it. The three
/// added fields are additive (older clients ignore them) and always present.
#[derive(Debug, Serialize)]
pub struct SubscriptionStatusResponse {
    #[serde(flatten)]
    plan: CurrentSubscriptionResponse,
    /// `stripe`, `apple`, `google`, or null when no paid plan is open.
    billing_provider: Option<String>,
    /// Where the person manages or cancels the plan: the App Store or Play
    /// subscriptions page for store plans, the platform billing page for
    /// Stripe. Null without a paid plan.
    manage_url: Option<String>,
    /// True when the account has more than one open plan (for example a
    /// Stripe plan and an App Store plan bought on the device). Clients show
    /// "You have two plans. Cancel one."
    conflict: bool,
    /// Stripe plans: the plan ends at the period end (a pending cancel, so
    /// the app shows "Resume plan"). False for store plans and free.
    cancel_at_period_end: bool,
    /// The same instant as `renews_at`, under the name the plan calls use.
    current_period_end: Option<String>,
    /// The plan a scheduled downgrade switches to at the period end; null
    /// when none, and for store plans.
    pending_plan_id: Option<String>,
}

/// What Stripe says about the caller's subscription right now.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct StripeLiveState {
    pub cancel_at_period_end: bool,
    pub pending_plan_id: Option<String>,
}

/// Read the pending cancel and the scheduled downgrade from the Stripe
/// subscription (and its schedule). `None` when the subscription is not on
/// `customer_id`. Errors are the caller's to swallow: this read is additive.
pub(crate) async fn stripe_live_state(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    subscription_id: &str,
    customer_id: &str,
    now: i64,
) -> Result<Option<StripeLiveState>, ApiError> {
    let subscription = stripe
        .get_object(secret_key, &format!("/v1/subscriptions/{subscription_id}"), &[])
        .await?;
    let customer = subscription["customer"].as_str().or_else(|| subscription["customer"]["id"].as_str());
    if customer != Some(customer_id) {
        return Ok(None);
    }
    let schedule_id = subscription["schedule"].as_str().or_else(|| subscription["schedule"]["id"].as_str());
    let mut pending_plan_id = None;
    if let Some(schedule_id) = schedule_id.filter(|id| !id.is_empty()) {
        let schedule = stripe
            .get_object(secret_key, &format!("/v1/subscription_schedules/{schedule_id}"), &[])
            .await?;
        // The first phase that starts after now is the pending change.
        pending_plan_id = schedule["phases"].as_array().into_iter().flatten().find_map(|phase| {
            phase["start_date"].as_i64().filter(|start| *start > now)?;
            let price = &phase["items"][0]["price"];
            let price_id = price.as_str().or_else(|| price["id"].as_str())?;
            SUBSCRIPTION_PLANS
                .iter()
                .find(|plan| plan_price_id(plan).as_deref() == Some(price_id))
                .map(|plan| plan.id.to_string())
        });
    }
    Ok(Some(StripeLiveState {
        cancel_at_period_end: subscription["cancel_at_period_end"].as_bool().unwrap_or(false),
        pending_plan_id,
    }))
}

/// The caller's open Stripe row (subscription id, customer id), if any.
async fn open_stripe_row(db: &PgPool, user_id: &str) -> Result<Option<(String, String)>, ApiError> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT stripe_subscription_id, stripe_customer_id
        FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing', 'past_due', 'unpaid')
          AND COALESCE(provider, 'stripe') = 'stripe' AND COALESCE(stripe_subscription_id, '') <> ''
        ORDER BY (status IN ('active', 'trialing')) DESC, updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(|(subscription_id, customer_id)| Some((subscription_id, customer_id.filter(|id| !id.is_empty())?))))
}

/// One open (`active`, `trialing`, `past_due`, `unpaid`) plan row with who
/// bills it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct OpenSubscription {
    pub plan_id: String,
    /// `stripe`, `apple` or `google`.
    pub provider: String,
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
        "super" => "Max",
        "ultra" => "Ultra",
        _ => "Free",
    }
}

async fn get_current_subscription(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<SubscriptionStatusResponse>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "billing")
        .await?
        .id;
    let admin = crate::auth::is_admin_user(&user_id);
    if !admin {
        // Plan D13 (no scheduled job): an App Store or Google Play plan whose
        // period end has passed is re-read from the store, at most once per
        // 10 minutes per row, which heals a missed renewal or expiry
        // notification. Never fails this read.
        let suspend =
            crate::services::store_apple::refresh_due_plans_from_env(&state.db, &user_id).await;
        super::billing_store::run_suspensions(&state, &suspend);
        let suspend =
            crate::services::store_google::refresh_due_plans_from_env(&state.db, &user_id).await;
        super::billing_store::run_suspensions(&state, &suspend);
    }
    let plan = current_subscription_for(&state.db, &user_id).await?;
    // Admins read as Ultra without a row; they have no provider.
    let open = if admin {
        Vec::new()
    } else {
        open_subscriptions(&state.db, &user_id).await?
    };
    let mut status = subscription_status(plan, &open);
    if !admin && open.first().is_some_and(|sub| sub.provider == "stripe") {
        // Additive: a Stripe outage or missing key leaves the defaults.
        if let (Ok(secret_key), Ok(Some((subscription_id, customer_id)))) =
            (std::env::var("STRIPE_SECRET_KEY"), open_stripe_row(&state.db, &user_id).await)
        {
            let stripe = ReqwestStripeCheckout::new();
            match stripe_live_state(&stripe, &secret_key, &subscription_id, &customer_id, chrono::Utc::now().timestamp()).await {
                Ok(Some(live)) => {
                    status.cancel_at_period_end = live.cancel_at_period_end;
                    status.pending_plan_id = live.pending_plan_id;
                }
                Ok(None) => {}
                Err(error) => tracing::warn!("billing subscription: Stripe state unavailable: {error}"),
            }
        }
    }
    Ok(Json(status))
}

/// The subscription response from the plan and the open rows (newest-first,
/// the order [`open_subscriptions`] returns).
fn subscription_status(plan: CurrentSubscriptionResponse, open: &[OpenSubscription]) -> SubscriptionStatusResponse {
    let current = open.first();
    SubscriptionStatusResponse {
        billing_provider: current.map(|sub| sub.provider.clone()),
        manage_url: current.and_then(|sub| crate::services::store_billing::manage_url(&sub.provider, &sub.plan_id)),
        conflict: open.len() > 1,
        cancel_at_period_end: false,
        current_period_end: plan.renews_at.clone(),
        pending_plan_id: None,
        plan,
    }
}

/// The plan id an account is on now (`free`, `plus`, `super` (Max) or `ultra`): the same answer
/// `GET /api/v1/billing/subscription` gives, for other routes that sign or gate on the plan
/// (`routes::freeweights` entitlement claim). Caller authentication is the caller's job.
pub(crate) async fn current_plan_id(db: &PgPool, user_id: &str) -> Result<String, ApiError> {
    Ok(current_subscription_for(db, user_id).await?.plan_id)
}

/// Source of truth for "which plan is this account on": admins read as Ultra, else the newest
/// open subscription row (Stripe or store, active/trialing first), else Free.
async fn current_subscription_for(
    db: &PgPool,
    user_id: &str,
) -> Result<CurrentSubscriptionResponse, ApiError> {
    if crate::auth::is_admin_user(user_id) {
        return Ok(CurrentSubscriptionResponse {
            plan_id: "ultra".to_string(),
            label: "Ultra".to_string(),
            plan_tier: "team".to_string(),
            status: "active".to_string(),
            renews_at: None,
        });
    }
    let row: Option<BillingSubscription> = sqlx::query_as(
        r#"
        SELECT user_id, plan_id, plan_tier, status, stripe_customer_id,
               EXTRACT(EPOCH FROM current_period_end)::BIGINT AS renews_at_epoch
        FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing', 'past_due', 'unpaid')
        ORDER BY (status IN ('active', 'trialing')) DESC, updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some(sub) => CurrentSubscriptionResponse {
            label: product_label(&sub.plan_id).to_string(),
            plan_id: sub.plan_id,
            plan_tier: sub.plan_tier,
            status: sub.status,
            renews_at: sub
                .renews_at_epoch
                .and_then(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0))
                .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        },
        None => CurrentSubscriptionResponse {
            plan_id: "free".to_string(),
            label: "Free".to_string(),
            plan_tier: "free".to_string(),
            status: "none".to_string(),
            renews_at: None,
        },
    })
}

async fn list_plans() -> Json<PlansResponse> {
    Json(plans_response(&crate::services::store_billing::StoreConfig::from_env()))
}

fn plans_response(config: &crate::services::store_billing::StoreConfig) -> PlansResponse {
    PlansResponse {
        plans: SUBSCRIPTION_PLANS
            .iter()
            .map(|plan| SubscriptionPlanResponse {
                id: plan.id,
                label: plan.label,
                price_usd: plan.price_usd,
                monthly_credits_usd: plan.monthly_credits_usd,
                rollover_cap_usd: plan.rollover_cap_usd,
                plan_tier: plan.plan_tier,
                store: config.offers(plan.apple_product_id, plan.google_product_id, plan.store_price_usd),
            })
            .collect(),
        freeweights: crate::routes::freeweights::PerkConfig::from_env().listing(),
    }
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
    if let Err(error) = ensure_purchase_allowed(trusted, plan.price_usd, "The Max and Ultra plans") {
        return error.into_response();
    }
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let Some(price_id) = plan_price_id(plan) else {
        return billing_not_configured_response();
    };
    let checkout_return = match CheckoutReturn::for_mode(request.ui_mode.unwrap_or_default(), &return_to) {
        Ok(checkout_return) => checkout_return,
        Err(error) => return error.into_response(),
    };
    // One subscription per account: a second checkout would bill twice and
    // create a second cloud computer. Plan changes go through the portal, or
    // through the store that bills a store plan.
    match open_subscriptions(&state.db, &user_id).await {
        Ok(open) => {
            if let Some(current) = open.first() {
                return already_subscribed_response(current);
            }
        }
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

    match create_subscription_checkout(
        &checkout,
        &secret_key,
        plan,
        &price_id,
        &user_id,
        customer_id.as_deref(),
        &checkout_return,
    )
    .await
    {
        Ok(session) => checkout_session_response(session),
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
    // A plan billed by Apple or Google is managed in that store: the Stripe
    // portal cannot change or cancel it. Checked before the Stripe config so
    // the answer is the same on every deployment.
    match open_subscriptions(&state.db, &user_id).await {
        Ok(open) => {
            if let Some(store_plan) = store_billed_plan(&open) {
                return store_billed_response(store_plan);
            }
        }
        Err(error) => return error.into_response(),
    }
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

/// Every plan in the catalog (store product id lookups in `services::store_billing`).
pub(crate) fn subscription_plans() -> &'static [SubscriptionPlan] {
    SUBSCRIPTION_PLANS
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

/// Build the subscription-mode Checkout Session for a validated plan: the
/// hosted URL, or the embedded client secret and session id.
async fn create_subscription_checkout(
    checkout: &dyn StripeCheckout,
    secret_key: &str,
    plan: &SubscriptionPlan,
    price_id: &str,
    clerk_user_id: &str,
    customer_id: Option<&str>,
    checkout_return: &CheckoutReturn,
) -> Result<CheckoutSession, ApiError> {
    let mut form = subscribe_form_params(plan, price_id, clerk_user_id, checkout_return);
    // Reuse the account's Stripe customer (from an earlier plan or credit
    // pack), so one person stays one customer and the portal shows it all.
    if let Some(customer_id) = customer_id {
        form.push(("customer".to_string(), customer_id.to_string()));
    }
    open_checkout_session(checkout, secret_key, &form, checkout_return.ui_mode()).await
}

/// The plan of the account's open subscription, if it has one. `past_due`
/// and `unpaid` count: the subscription still exists and comes back when
/// the card is fixed.
pub(crate) async fn open_subscription_plan(db: &PgPool, user_id: &str) -> Result<Option<String>, ApiError> {
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

/// Every open plan row of an account, newest first (active/trialing before
/// past_due/unpaid, then by `updated_at`), the same order
/// [`current_subscription_for`] picks the current plan by. More than one row
/// is a double-billing conflict (plan D5).
pub(crate) async fn open_subscriptions(db: &PgPool, user_id: &str) -> Result<Vec<OpenSubscription>, ApiError> {
    Ok(sqlx::query_as::<_, OpenSubscription>(
        r#"
        SELECT plan_id, COALESCE(provider, 'stripe') AS provider
        FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing', 'past_due', 'unpaid')
        ORDER BY (status IN ('active', 'trialing')) DESC, updated_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?)
}

/// The store plan that makes the Stripe portal useless: the account's open
/// plans are all store-billed. With a Stripe plan open too (a conflict), the
/// portal stays open so the person can cancel the Stripe one.
pub(crate) fn store_billed_plan(open: &[OpenSubscription]) -> Option<&OpenSubscription> {
    if open.iter().any(|sub| sub.provider == "stripe") {
        return None;
    }
    open.first()
}

/// 409 `already_subscribed`: the account has an open plan. Names who bills it
/// and where to manage it, so a client can send the person to the right place
/// (shared by `POST /billing/subscribe` and `POST /billing/store/preflight`).
pub(crate) fn already_subscribed_response(current: &OpenSubscription) -> Response {
    let message = match current.provider.as_str() {
        "apple" => "This account already has a plan billed by Apple. Change or cancel it in your App Store subscriptions.",
        "google" => "This account already has a plan billed by Google Play. Change or cancel it in your Play Store subscriptions.",
        _ => "This account already has a plan. Change or cancel it in Manage billing.",
    };
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "already_subscribed",
            "current_plan_id": current.plan_id,
            "billing_provider": current.provider,
            "manage_url": crate::services::store_billing::manage_url(&current.provider, &current.plan_id),
            "message": message,
        })),
    )
        .into_response()
}

/// 409 `store_billed` from `POST /billing/portal` for a plan Apple or Google bills.
fn store_billed_response(plan: &OpenSubscription) -> Response {
    let store = if plan.provider == "apple" { "App Store" } else { "Play Store" };
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "store_billed",
            "current_plan_id": plan.plan_id,
            "billing_provider": plan.provider,
            "manage_url": crate::services::store_billing::manage_url(&plan.provider, &plan.plan_id),
            "message": format!("This plan is billed by the {store}. Change or cancel it in your {store} subscriptions."),
        })),
    )
        .into_response()
}

/// Form fields for a subscription Checkout Session. The metadata lives under
/// `subscription_data` so it lands on the *subscription* object — the webhook's
/// `customer.subscription.*` mapping reads `clerk_user_id` / `allternit_plan_tier` /
/// `allternit_plan_id` from there, so these names must stay stable.
fn subscribe_form_params(
    plan: &SubscriptionPlan,
    price_id: &str,
    clerk_user_id: &str,
    checkout_return: &CheckoutReturn,
) -> Vec<(String, String)> {
    let mut form = vec![("mode".to_string(), "subscription".to_string())];
    // Pay-now methods only (card and Link, decision E4): no buy-now-pay-later
    // or delayed methods for Allternit Cloud. Services Payment Links keep the
    // account's wider method list.
    form.extend(payment_method_fields());
    form.extend(checkout_return.form_fields());
    form.extend([
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
    ]);
    form
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
    /// `current_period_end` as unix seconds (NULL until a Stripe event reports it).
    pub renews_at_epoch: Option<i64>,
}

/// A subscription's `current_period_end` (unix seconds). Newer Stripe API
/// versions report it per item rather than on the subscription.
pub(crate) fn subscription_period_end(subscription: &serde_json::Value) -> Option<i64> {
    subscription["current_period_end"]
        .as_i64()
        .or_else(|| subscription["items"]["data"][0]["current_period_end"].as_i64())
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
    current_period_end: Option<i64>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO billing_subscriptions (
            stripe_subscription_id, user_id, plan_id, plan_tier, status, stripe_customer_id,
            current_period_end, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7::BIGINT), CURRENT_TIMESTAMP)
        ON CONFLICT (stripe_subscription_id) DO UPDATE SET
            user_id = excluded.user_id,
            plan_id = excluded.plan_id,
            plan_tier = excluded.plan_tier,
            status = excluded.status,
            stripe_customer_id = excluded.stripe_customer_id,
            current_period_end = COALESCE(excluded.current_period_end, billing_subscriptions.current_period_end),
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(stripe_subscription_id)
    .bind(user_id)
    .bind(plan_id)
    .bind(plan_tier)
    .bind(status)
    .bind(stripe_customer_id)
    .bind(current_period_end)
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
        SELECT user_id, plan_id, plan_tier, status, stripe_customer_id,
               EXTRACT(EPOCH FROM current_period_end)::BIGINT AS renews_at_epoch
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
    #[test]
    fn period_end_reads_the_subscription_or_its_first_item() {
        assert_eq!(subscription_period_end(&serde_json::json!({ "current_period_end": 1798761600 })), Some(1798761600));
        // Newer Stripe API versions report it per item.
        assert_eq!(
            subscription_period_end(&serde_json::json!({ "items": { "data": [{ "current_period_end": 1798761601 }] } })),
            Some(1798761601)
        );
        assert_eq!(subscription_period_end(&serde_json::json!({})), None);
    }

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

        /// Embedded sessions come back as the whole object, as Stripe returns
        /// it for `ui_mode=embedded`: a client secret and no hosted URL.
        async fn post_object(
            &self,
            _secret_key: &str,
            path: &str,
            _idempotency_key: Option<&str>,
            form: &[(String, String)],
        ) -> Result<serde_json::Value, ApiError> {
            assert_eq!(path, "/v1/checkout/sessions");
            *self.checkout_form.lock().unwrap() = Some(form.to_vec());
            Ok(serde_json::json!({
                "id": "cs_test_sub_1",
                "object": "checkout.session",
                "client_secret": "cs_test_sub_1_secret_xyz",
                "url": null,
            }))
        }
    }

    fn hosted(success_url: &str, cancel_url: &str) -> CheckoutReturn {
        CheckoutReturn::Hosted { success_url: success_url.to_string(), cancel_url: cancel_url.to_string() }
    }

    async fn response_json(response: Response) -> (StatusCode, serde_json::Value) {
        use http_body_util::BodyExt;
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn embedded_subscribe_returns_the_client_secret_for_every_plan() {
        let checkout_return = CheckoutReturn::for_mode(
            UiMode::Embedded,
            &crate::routes::billing_checkout::ReturnTo {
                return_url: Some("https://ai.allternit.com/settings/billing".to_string()),
            },
        )
        .unwrap();
        for plan in SUBSCRIPTION_PLANS {
            let checkout = RecordingStripe::new();
            let session = create_subscription_checkout(
                &checkout, "sk_test", plan, "price_x", "user_1", Some("cus_own"), &checkout_return,
            )
            .await
            .unwrap();
            assert_eq!(
                session,
                CheckoutSession::Embedded {
                    client_secret: "cs_test_sub_1_secret_xyz".to_string(),
                    session_id: "cs_test_sub_1".to_string(),
                },
                "{}",
                plan.id
            );
            let form = checkout.checkout_form.lock().unwrap().clone().unwrap();
            let get = |key: &str| form.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            assert_eq!(get("mode").as_deref(), Some("subscription"));
            assert_eq!(get("ui_mode").as_deref(), Some("embedded"));
            assert_eq!(
                get("return_url").as_deref(),
                Some("https://ai.allternit.com/settings/billing?checkout=success&session_id={CHECKOUT_SESSION_ID}")
            );
            assert_eq!(get("redirect_on_completion").as_deref(), Some("if_required"));
            assert_eq!(get("success_url"), None);
            assert_eq!(get("cancel_url"), None);
            assert_eq!(get("payment_method_types[0]").as_deref(), Some("card"));
            assert_eq!(get("payment_method_types[1]").as_deref(), Some("link"));
            assert_eq!(get("payment_method_types[2]"), None, "no wallets or other methods");
            assert_eq!(get("customer").as_deref(), Some("cus_own"));
            assert_eq!(get("subscription_data[metadata][allternit_plan_id]").as_deref(), Some(plan.id));
            assert_eq!(get("subscription_data[metadata][allternit_plan_tier]").as_deref(), Some(plan.plan_tier));
            assert_eq!(get("subscription_data[metadata][clerk_user_id]").as_deref(), Some("user_1"));

            let (status, body) = response_json(checkout_session_response(session)).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                body,
                serde_json::json!({ "client_secret": "cs_test_sub_1_secret_xyz", "session_id": "cs_test_sub_1" })
            );
        }
    }

    #[tokio::test]
    async fn hosted_subscribe_reply_is_unchanged() {
        let plan = find_plan("plus").unwrap();
        let checkout = RecordingStripe::new();
        let session = create_subscription_checkout(
            &checkout, "sk_test", plan, "price_x", "user_1", None, &hosted("https://s.example", "https://c.example"),
        )
        .await
        .unwrap();
        let form = checkout.checkout_form.lock().unwrap().clone().unwrap();
        assert!(form.iter().all(|(key, _)| key != "ui_mode" && key != "return_url" && key != "redirect_on_completion"));
        let (status, body) = response_json(checkout_session_response(session)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "checkout_url": "https://checkout.stripe.com/c/pay/test_sub" }));
    }

    #[test]
    fn subscribe_request_ignores_a_customer_and_defaults_to_hosted() {
        let request: SubscribeRequest =
            serde_json::from_str(r#"{"plan_id":"plus","customer":"cus_victim","customer_id":"cus_victim"}"#).unwrap();
        assert_eq!(request.plan_id, "plus");
        assert_eq!(request.ui_mode.unwrap_or_default(), UiMode::Hosted);
        let embedded: SubscribeRequest = serde_json::from_str(r#"{"plan_id":"plus","ui_mode":"embedded"}"#).unwrap();
        assert_eq!(embedded.ui_mode, Some(UiMode::Embedded));
        assert!(serde_json::from_str::<SubscribeRequest>(r#"{"plan_id":"plus","ui_mode":"wallet"}"#).is_err());
        // The form carries only the customer the server resolved.
        let form = subscribe_form_params(find_plan("plus").unwrap(), "price_x", "user_1", &hosted("s", "c"));
        assert!(form.iter().all(|(key, value)| key != "customer" && !value.contains("cus_victim")));
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
        // Freeweights perks are listed, not offered (no test sets FREEWEIGHTS_PERKS_ENABLED).
        assert_eq!(value["freeweights"]["status"], serde_json::json!("not_offered"));
        for plan in ["free", "plus", "super", "ultra"] {
            assert!(!value["freeweights"]["plans"][plan].as_array().unwrap().is_empty(), "{plan}");
        }
        assert_eq!(
            value["freeweights"]["plans"]["free"][0],
            serde_json::json!({"key": "public_storage_bytes", "label": "Public upload storage", "value": 10_000_000_000u64, "unit": "bytes"})
        );
    }

    /// An old client decodes plans into a struct with only the fields it knew
    /// before store billing; the added `store` block must not break it.
    #[test]
    fn plans_with_store_block_still_parse_for_old_clients() {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct OldPlan {
            id: String,
            label: String,
            price_usd: f64,
            monthly_credits_usd: f64,
            rollover_cap_usd: f64,
            plan_tier: String,
        }
        let config = crate::services::store_billing::StoreConfig { apple: false, google: true };
        let mut value = serde_json::to_value(plans_response(&config)).unwrap();
        let plans = value["plans"].as_array_mut().unwrap();
        assert_eq!(plans.len(), 3);
        let stores: Vec<serde_json::Value> = plans
            .iter_mut()
            .map(|plan| plan.as_object_mut().unwrap().remove("store").unwrap())
            .collect();
        // Without the new block every plan is exactly the old shape.
        let old: Vec<OldPlan> = serde_json::from_value(value["plans"].clone()).unwrap();
        assert_eq!(old[1].id, "super");
        assert_eq!(old[1].price_usd, 100.0, "web price is unchanged");
        assert_eq!(stores[1]["apple"], serde_json::json!({ "enabled": false, "product_id": "com.allternit.mobile.plan.max.monthly" }));
        assert_eq!(stores[1]["google"], serde_json::json!({ "enabled": true, "product_id": "plan_max" }));
        assert_eq!(stores[1]["reference_price_usd"], serde_json::json!(124.99));
        assert_eq!(stores[0]["reference_price_usd"], serde_json::json!(24.99));
        assert_eq!(stores[2]["reference_price_usd"], serde_json::json!(249.99));
    }

    #[test]
    fn subscription_status_names_who_bills_and_flags_two_plans() {
        let plan = || CurrentSubscriptionResponse {
            plan_id: "plus".to_string(),
            label: "Plus".to_string(),
            plan_tier: "pro".to_string(),
            status: "active".to_string(),
            renews_at: None,
        };
        let free = serde_json::to_value(subscription_status(plan(), &[])).unwrap();
        assert_eq!(free["billing_provider"], serde_json::Value::Null);
        assert_eq!(free["manage_url"], serde_json::Value::Null);
        assert_eq!(free["conflict"], false);
        let both = [
            OpenSubscription { plan_id: "plus".to_string(), provider: "google".to_string() },
            OpenSubscription { plan_id: "super".to_string(), provider: "stripe".to_string() },
        ];
        let value = serde_json::to_value(subscription_status(plan(), &both)).unwrap();
        assert_eq!(value["plan_id"], "plus", "the plan fields stay at the top level");
        assert_eq!(value["billing_provider"], "google");
        assert_eq!(
            value["manage_url"],
            "https://play.google.com/store/account/subscriptions?sku=plan_plus&package=com.allternit.app"
        );
        assert_eq!(value["conflict"], true);
        assert_eq!(value["cancel_at_period_end"], false);
        assert_eq!(value["pending_plan_id"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn subscribe_builds_subscription_form_with_price_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("STRIPE_PRICE_PLUS", "price_test_plus");
        let plan = find_plan("plus").unwrap();
        let price_id = plan_price_id(plan).expect("env price id must resolve");
        let checkout = RecordingStripe::new();
        let session = create_subscription_checkout(
            &checkout, "sk_test", plan, &price_id, "user_1", None, &hosted("https://s.example", "https://c.example"),
        )
        .await
        .unwrap();
        assert_eq!(session, CheckoutSession::Hosted { url: "https://checkout.stripe.com/c/pay/test_sub".to_string() });
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
        create_subscription_checkout(
            &checkout, "sk_test", plan, "price_x", "user_1", Some("cus_123"), &hosted("https://s.example", "https://c.example"),
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
        // Bought Plus, switched to Max in the billing portal: the metadata
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

    struct LiveStripe(std::collections::HashMap<String, serde_json::Value>);

    #[async_trait::async_trait]
    impl StripeCheckout for LiveStripe {
        async fn create_checkout_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            Err(ApiError::Internal("not used".into()))
        }
        async fn create_billing_portal_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            Err(ApiError::Internal("not used".into()))
        }
        async fn get_object(&self, _: &str, path: &str, _: &[(String, String)]) -> Result<serde_json::Value, ApiError> {
            self.0.get(path).cloned().ok_or_else(|| ApiError::Internal("no such object".into()))
        }
    }

    #[tokio::test]
    async fn stripe_state_shows_pending_cancel_and_scheduled_downgrade() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("STRIPE_PRICE_PLUS", "price_plus");
        std::env::set_var("STRIPE_PRICE_SUPER", "price_super");
        let now = 1_800_000_000;
        let canceling = serde_json::json!({ "customer": "cus_a", "cancel_at_period_end": true, "schedule": null });
        let downgrading = serde_json::json!({ "customer": "cus_a", "cancel_at_period_end": false, "schedule": "sub_sched_1" });
        let schedule = serde_json::json!({ "phases": [
            { "start_date": now - 100, "items": [{ "price": "price_super" }] },
            { "start_date": now + 100, "items": [{ "price": { "id": "price_plus" } }] },
        ] });
        let stripe = LiveStripe(
            [
                ("/v1/subscriptions/sub_cancel".to_string(), canceling),
                ("/v1/subscriptions/sub_down".to_string(), downgrading),
                ("/v1/subscription_schedules/sub_sched_1".to_string(), schedule),
            ]
            .into(),
        );
        let cancel = stripe_live_state(&stripe, "sk_test_fake", "sub_cancel", "cus_a", now).await.unwrap().unwrap();
        assert_eq!(cancel, StripeLiveState { cancel_at_period_end: true, pending_plan_id: None });
        let down = stripe_live_state(&stripe, "sk_test_fake", "sub_down", "cus_a", now).await.unwrap().unwrap();
        assert_eq!(down, StripeLiveState { cancel_at_period_end: false, pending_plan_id: Some("plus".to_string()) });
        // Another account's subscription reads as nothing.
        assert_eq!(stripe_live_state(&stripe, "sk_test_fake", "sub_down", "cus_b", now).await.unwrap(), None);
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
        // Untrusted: $20 Plus and $25 pack pass, $50 pack / $100 Max blocked.
        assert!(ensure_purchase_allowed(false, 20.0, "The Max and Ultra plans").is_ok());
        assert!(ensure_purchase_allowed(false, 25.0, "The $50 and $100 packs").is_ok());
        let error = ensure_purchase_allowed(false, 50.0, "The $50 and $100 packs").unwrap_err();
        assert!(error.to_string().contains("First purchases are limited to $25"), "{error}");
        assert!(error.to_string().contains("$50 and $100 packs unlock automatically"), "{error}");
        assert!(ensure_purchase_allowed(false, 100.0, "The Max and Ultra plans").is_err());
        // Trusted buyers are never gated.
        assert!(ensure_purchase_allowed(true, 100.0, "The $50 and $100 packs").is_ok());
        assert!(ensure_purchase_allowed(true, 200.0, "The Max and Ultra plans").is_ok());
    }
}
