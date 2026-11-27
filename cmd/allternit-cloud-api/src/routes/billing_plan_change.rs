//! Switch, cancel and resume a Stripe-billed plan inside the app (Claude
//! polish C8, E4). Before this, every one of these opened Stripe's billing
//! portal in a browser.
//!
//! All four verify the Clerk session (or a `billing`-scoped API token) and
//! resolve both the Stripe customer (`user_billing_accounts`) and the Stripe
//! subscription (the caller's open `billing_subscriptions` row with provider
//! `stripe`) from that session. No request names a customer or a
//! subscription, and a subscription whose Stripe customer is not the caller's
//! answers 404 like a missing one.
//!
//! - `POST /api/v1/billing/subscription/preview` `{ plan_id }` ->
//!   `{ plan_id, amount_due_usd, currency, proration_date, next_invoice_at,
//!   effective }`. Stripe's preview invoice (`POST /v1/invoices/create_preview`,
//!   Stripe's replacement for `GET /v1/invoices/upcoming`, with the same
//!   subscription item + proration fields under `subscription_details`).
//!   `amount_due_usd` is what the next invoice will charge, prorations
//!   included. Pass `proration_date` back to `/change` so the charge matches.
//! - `POST /api/v1/billing/subscription/change` `{ plan_id, proration_date }`
//!   -> the plan view below.
//!   - **Upgrade** (higher price): `POST /v1/subscriptions/:id` with the new
//!     price on the same item, `proration_behavior=create_prorations` and the
//!     previewed `proration_date`. Takes effect now; the billing cycle anchor
//!     is unchanged, so the prorated difference lands on the next invoice.
//!   - **Downgrade** (lower price): takes effect at the end of the period,
//!     through a subscription schedule — Stripe's documented path for a
//!     change at period end: `POST /v1/subscription_schedules
//!     from_subscription=...` (or the one already attached), then one update
//!     that keeps the current price until `current_period_end` and starts
//!     the new price after it, no proration, `end_behavior=release`. The
//!     person keeps the plan they paid for until it renews.
//!   - Choosing the current plan while a downgrade is pending undoes it
//!     (releases the schedule). Choosing it otherwise is 409 `already_on_plan`.
//! - `POST /api/v1/billing/subscription/cancel` -> `cancel_at_period_end=true`
//!   (a pending downgrade is dropped first; Stripe refuses cancel changes on a
//!   subscription a schedule manages).
//! - `POST /api/v1/billing/subscription/resume` -> `cancel_at_period_end=false`.
//!
//! Plan view: `{ plan_id, label, status, cancel_at_period_end,
//! current_period_end, pending_plan_id, pending_at, effective }`, snake_case
//! (CONTRACT.md), times RFC 3339.
//!
//! Errors: 404 `no_subscription` (Free, or not the caller's); 409
//! `billed_by_store` (the open plan is billed by Apple or Google: change it in
//! that store); 400 `unknown_plan`; 409 `already_on_plan`; 409
//! `preview_expired` (the `proration_date` is older than an hour: preview
//! again); 503 `billing_not_configured` (no `STRIPE_SECRET_KEY` or no price
//! id for a plan); 502 `billing_upstream_error`.
//!
//! The local `billing_subscriptions` mirror is not written here: Stripe's
//! `customer.subscription.updated` webhook already moves it (and the tier) by
//! the subscription's current price (`routes::billing_webhooks`).

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use super::billing_checkout::{
    billing_not_configured_response, billing_upstream_error_response, ReqwestStripeCheckout, StripeCheckout,
};
use super::billing_subscriptions::{find_plan, subscription_period_end, subscription_plans, SubscriptionPlan};
use crate::error::ApiError;
use crate::services::auto_topup;
use crate::ApiState;

/// Stripe statuses that still bill (the set `routes::billing_subscriptions` treats as open).
const OPEN_STATUSES: &[&str] = &["active", "trialing", "past_due", "unpaid"];

/// How old a previewed `proration_date` may be when the change is made.
const PRORATION_DATE_MAX_AGE_SECS: i64 = 60 * 60;
/// Clock skew allowed for a `proration_date` slightly in the future.
const PRORATION_DATE_MAX_SKEW_SECS: i64 = 5 * 60;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/billing/subscription/preview", post(preview_change))
        .route("/api/v1/billing/subscription/change", post(change_plan))
        .route("/api/v1/billing/subscription/cancel", post(cancel_plan))
        .route("/api/v1/billing/subscription/resume", post(resume_plan))
}

#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
    pub plan_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangeRequest {
    pub plan_id: String,
    /// The `proration_date` the preview returned. Missing means now.
    #[serde(default)]
    pub proration_date: Option<i64>,
}

/// `/preview` reply.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanChangePreview {
    pub plan_id: String,
    pub amount_due_usd: f64,
    pub currency: String,
    /// Unix seconds; send back to `/change`.
    pub proration_date: i64,
    pub next_invoice_at: Option<String>,
    /// `now` (upgrade) or `period_end` (downgrade).
    pub effective: &'static str,
}

/// The plan as it stands after a change, cancel or resume.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanView {
    pub plan_id: String,
    pub label: String,
    pub status: String,
    pub cancel_at_period_end: bool,
    pub current_period_end: Option<String>,
    /// The plan a pending downgrade moves to, and when.
    pub pending_plan_id: Option<String>,
    pub pending_at: Option<String>,
    /// What the call did: `now`, `period_end`, or null for cancel/resume.
    pub effective: Option<&'static str>,
}

/// The Stripe price id of each plan, read once from `STRIPE_PRICE_*`.
/// Passed in (not read from env inside) so tests need no env.
#[derive(Debug, Clone, Default)]
pub struct PlanPrices(Vec<(&'static SubscriptionPlan, String)>);

impl PlanPrices {
    pub fn from_env() -> Self {
        Self(
            subscription_plans()
                .iter()
                .filter_map(|plan| {
                    let price = std::env::var(plan.price_env).ok()?.trim().to_string();
                    (!price.is_empty()).then_some((plan, price))
                })
                .collect(),
        )
    }

    #[cfg(test)]
    pub fn of(pairs: &[(&str, &str)]) -> Self {
        Self(pairs.iter().filter_map(|(plan, price)| Some((find_plan(plan)?, price.to_string()))).collect())
    }

    fn price_id(&self, plan_id: &str) -> Option<&str> {
        self.0.iter().find(|(plan, _)| plan.id == plan_id).map(|(_, price)| price.as_str())
    }

    fn plan_for_price(&self, price_id: &str) -> Option<&'static SubscriptionPlan> {
        self.0.iter().find(|(_, price)| price == price_id).map(|(plan, _)| *plan)
    }
}

/// The caller's open plan row as the local mirror holds it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct OpenPlanRow {
    pub stripe_subscription_id: String,
    pub plan_id: String,
    /// `stripe`, `apple` or `google`.
    pub provider: String,
    pub stripe_customer_id: Option<String>,
}

/// Why a plan call did not go through. A subscription that is not the
/// caller's is `NoSubscription`, never a 502 that hints it exists.
#[derive(Debug)]
pub enum PlanChangeError {
    NoSubscription,
    BilledByStore { plan_id: String, provider: String },
    UnknownPlan,
    AlreadyOnPlan,
    PreviewExpired,
    NotConfigured,
    Upstream(ApiError),
}

impl From<ApiError> for PlanChangeError {
    fn from(error: ApiError) -> Self {
        PlanChangeError::Upstream(error)
    }
}

impl IntoResponse for PlanChangeError {
    fn into_response(self) -> Response {
        let refuse = |status: StatusCode, body: Value| (status, Json(body)).into_response();
        match self {
            PlanChangeError::NoSubscription => refuse(
                StatusCode::NOT_FOUND,
                json!({ "error": "no_subscription", "message": "You don't have a plan billed here." }),
            ),
            PlanChangeError::BilledByStore { plan_id, provider } => {
                let store = if provider == "apple" { "App Store" } else { "Play Store" };
                refuse(
                    StatusCode::CONFLICT,
                    json!({
                        "error": "billed_by_store",
                        "current_plan_id": plan_id,
                        "billing_provider": provider,
                        "manage_url": crate::services::store_billing::manage_url(&provider, &plan_id),
                        "message": format!("This plan is billed by the {store}. Change or cancel it in your {store} subscriptions."),
                    }),
                )
            }
            PlanChangeError::UnknownPlan => refuse(
                StatusCode::BAD_REQUEST,
                json!({ "error": "unknown_plan", "message": "That plan doesn't exist." }),
            ),
            PlanChangeError::AlreadyOnPlan => refuse(
                StatusCode::CONFLICT,
                json!({ "error": "already_on_plan", "message": "You're already on this plan." }),
            ),
            PlanChangeError::PreviewExpired => refuse(
                StatusCode::CONFLICT,
                json!({ "error": "preview_expired", "message": "This price is out of date. Check the new price and try again." }),
            ),
            PlanChangeError::NotConfigured => billing_not_configured_response(),
            PlanChangeError::Upstream(ApiError::DatabaseError(error)) => ApiError::DatabaseError(error).into_response(),
            PlanChangeError::Upstream(error) => billing_upstream_error_response(&error),
        }
    }
}

/// The Stripe row to act on: the caller's open Stripe plan. With only
/// store-billed plans open the answer is `billed_by_store`; with none, 404.
pub fn pick_stripe_plan(rows: &[OpenPlanRow]) -> Result<&OpenPlanRow, PlanChangeError> {
    if let Some(row) = rows.iter().find(|row| row.provider == "stripe" && !row.stripe_subscription_id.is_empty()) {
        return Ok(row);
    }
    match rows.first() {
        Some(row) => Err(PlanChangeError::BilledByStore { plan_id: row.plan_id.clone(), provider: row.provider.clone() }),
        None => Err(PlanChangeError::NoSubscription),
    }
}

async fn open_plan_rows(db: &PgPool, user_id: &str) -> Result<Vec<OpenPlanRow>, ApiError> {
    sqlx::query_as::<_, OpenPlanRow>(
        r#"
        SELECT stripe_subscription_id, plan_id, COALESCE(provider, 'stripe') AS provider, stripe_customer_id
        FROM billing_subscriptions
        WHERE user_id = $1 AND status IN ('active', 'trialing', 'past_due', 'unpaid')
        ORDER BY (status IN ('active', 'trialing')) DESC, updated_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::DatabaseError)
}

/// The id inside a Stripe field that is either an id string or an expanded object.
fn object_id(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| value["id"].as_str()).filter(|id| !id.is_empty())
}

fn rfc3339(secs: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// A Stripe subscription read live and checked against the caller.
#[derive(Debug, Clone)]
pub struct LiveSubscription {
    pub id: String,
    pub status: String,
    pub item_id: String,
    pub price_id: String,
    pub plan: &'static SubscriptionPlan,
    pub period_end: Option<i64>,
    pub cancel_at_period_end: bool,
    pub schedule_id: Option<String>,
}

fn parse_subscription(subscription: &Value, prices: &PlanPrices) -> Result<LiveSubscription, PlanChangeError> {
    let id = object_id(&subscription["id"]).ok_or(PlanChangeError::NoSubscription)?.to_string();
    let item = &subscription["items"]["data"][0];
    let item_id = object_id(&item["id"])
        .ok_or_else(|| ApiError::Internal("Stripe subscription has no item".to_string()))?
        .to_string();
    let price_id = object_id(&item["price"])
        .ok_or_else(|| ApiError::Internal("Stripe subscription item has no price".to_string()))?
        .to_string();
    let plan = prices
        .plan_for_price(&price_id)
        .or_else(|| subscription["metadata"]["allternit_plan_id"].as_str().and_then(find_plan))
        .ok_or_else(|| ApiError::Internal("Stripe subscription is on a price this server doesn't sell".to_string()))?;
    Ok(LiveSubscription {
        id,
        status: subscription["status"].as_str().unwrap_or_default().to_string(),
        item_id,
        price_id,
        plan,
        period_end: subscription_period_end(subscription),
        cancel_at_period_end: subscription["cancel_at_period_end"].as_bool().unwrap_or(false),
        schedule_id: object_id(&subscription["schedule"]).map(str::to_string),
    })
}

/// Read the subscription from Stripe. 404 unless it is open and on
/// `customer_id` — the guard that keeps one account off another's plan.
pub async fn load_subscription(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    subscription_id: &str,
    customer_id: &str,
    prices: &PlanPrices,
) -> Result<LiveSubscription, PlanChangeError> {
    let subscription = stripe.get_object(secret_key, &format!("/v1/subscriptions/{subscription_id}"), &[]).await?;
    if object_id(&subscription["customer"]) != Some(customer_id) {
        return Err(PlanChangeError::NoSubscription);
    }
    let live = parse_subscription(&subscription, prices)?;
    if !OPEN_STATUSES.contains(&live.status.as_str()) {
        return Err(PlanChangeError::NoSubscription);
    }
    Ok(live)
}

/// The pending downgrade a schedule holds: the first phase that starts
/// after `now`, its plan and start.
async fn pending_change(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    schedule_id: &str,
    prices: &PlanPrices,
    now: i64,
) -> Result<Option<(&'static SubscriptionPlan, i64)>, ApiError> {
    let schedule = stripe.get_object(secret_key, &format!("/v1/subscription_schedules/{schedule_id}"), &[]).await?;
    Ok(schedule["phases"].as_array().into_iter().flatten().find_map(|phase| {
        let start = phase["start_date"].as_i64().filter(|start| *start > now)?;
        let plan = prices.plan_for_price(object_id(&phase["items"][0]["price"])?)?;
        Some((plan, start))
    }))
}

async fn plan_view(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    live: &LiveSubscription,
    prices: &PlanPrices,
    now: i64,
    effective: Option<&'static str>,
) -> Result<PlanView, ApiError> {
    let pending = match &live.schedule_id {
        Some(schedule_id) => pending_change(stripe, secret_key, schedule_id, prices, now).await?,
        None => None,
    };
    Ok(PlanView {
        plan_id: live.plan.id.to_string(),
        label: live.plan.label.to_string(),
        status: live.status.clone(),
        cancel_at_period_end: live.cancel_at_period_end,
        current_period_end: live.period_end.and_then(rfc3339),
        pending_plan_id: pending.map(|(plan, _)| plan.id.to_string()),
        pending_at: pending.and_then(|(_, at)| rfc3339(at)),
        effective,
    })
}

fn target_plan<'a>(plan_id: &str, prices: &'a PlanPrices) -> Result<(&'static SubscriptionPlan, &'a str), PlanChangeError> {
    let plan = find_plan(plan_id.trim()).ok_or(PlanChangeError::UnknownPlan)?;
    let price = prices.price_id(plan.id).ok_or(PlanChangeError::NotConfigured)?;
    Ok((plan, price))
}

fn is_upgrade(from: &SubscriptionPlan, to: &SubscriptionPlan) -> bool {
    to.price_usd > from.price_usd
}

/// Form for `POST /v1/invoices/create_preview`: the next invoice if the
/// subscription item moved to `price_id`.
pub fn preview_form(
    customer_id: &str,
    live: &LiveSubscription,
    price_id: &str,
    upgrade: bool,
    proration_date: i64,
) -> Vec<(String, String)> {
    let mut form = vec![
        ("customer".to_string(), customer_id.to_string()),
        ("subscription".to_string(), live.id.clone()),
        ("subscription_details[items][0][id]".to_string(), live.item_id.clone()),
        ("subscription_details[items][0][price]".to_string(), price_id.to_string()),
    ];
    if upgrade {
        form.push(("subscription_details[proration_behavior]".to_string(), "create_prorations".to_string()));
        form.push(("subscription_details[proration_date]".to_string(), proration_date.to_string()));
    } else {
        // A downgrade starts at renewal: the next invoice is the new price, no proration.
        form.push(("subscription_details[proration_behavior]".to_string(), "none".to_string()));
    }
    form
}

pub async fn preview(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
    live: &LiveSubscription,
    plan_id: &str,
    prices: &PlanPrices,
    now: i64,
) -> Result<PlanChangePreview, PlanChangeError> {
    let (plan, price_id) = target_plan(plan_id, prices)?;
    if plan.id == live.plan.id && live.schedule_id.is_none() {
        return Err(PlanChangeError::AlreadyOnPlan);
    }
    let upgrade = is_upgrade(live.plan, plan);
    let invoice = stripe
        .post_object(secret_key, "/v1/invoices/create_preview", None, &preview_form(customer_id, live, price_id, upgrade, now))
        .await?;
    let amount_due = invoice["amount_due"]
        .as_i64()
        .ok_or_else(|| ApiError::Internal("Stripe preview invoice has no amount_due".to_string()))?;
    let next_invoice_at = invoice["next_payment_attempt"].as_i64().or(live.period_end).and_then(rfc3339);
    Ok(PlanChangePreview {
        plan_id: plan.id.to_string(),
        amount_due_usd: amount_due as f64 / 100.0,
        currency: invoice["currency"].as_str().unwrap_or("usd").to_string(),
        proration_date: now,
        next_invoice_at,
        effective: if upgrade { "now" } else { "period_end" },
    })
}

async fn release_schedule(stripe: &dyn StripeCheckout, secret_key: &str, schedule_id: &str) -> Result<(), ApiError> {
    stripe
        .post_object(secret_key, &format!("/v1/subscription_schedules/{schedule_id}/release"), None, &[])
        .await
        .map(|_| ())
}

async fn update_subscription(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    subscription_id: &str,
    idempotency_key: Option<&str>,
    form: &[(String, String)],
) -> Result<Value, ApiError> {
    stripe.post_object(secret_key, &format!("/v1/subscriptions/{subscription_id}"), idempotency_key, form).await
}

/// Form for the immediate upgrade (`POST /v1/subscriptions/:id`).
pub fn upgrade_form(live: &LiveSubscription, plan: &SubscriptionPlan, price_id: &str, proration_date: i64) -> Vec<(String, String)> {
    vec![
        ("items[0][id]".to_string(), live.item_id.clone()),
        ("items[0][price]".to_string(), price_id.to_string()),
        ("proration_behavior".to_string(), "create_prorations".to_string()),
        ("proration_date".to_string(), proration_date.to_string()),
        // Choosing a plan means keeping one: a pending cancel is undone.
        ("cancel_at_period_end".to_string(), "false".to_string()),
        ("metadata[allternit_plan_id]".to_string(), plan.id.to_string()),
        ("metadata[allternit_plan_tier]".to_string(), plan.plan_tier.to_string()),
    ]
}

/// Form for the schedule update that moves to `price_id` at `period_end`.
pub fn downgrade_schedule_form(
    live: &LiveSubscription,
    phase_start: i64,
    period_end: i64,
    plan: &SubscriptionPlan,
    price_id: &str,
) -> Vec<(String, String)> {
    vec![
        ("end_behavior".to_string(), "release".to_string()),
        ("phases[0][items][0][price]".to_string(), live.price_id.clone()),
        ("phases[0][items][0][quantity]".to_string(), "1".to_string()),
        ("phases[0][start_date]".to_string(), phase_start.to_string()),
        ("phases[0][end_date]".to_string(), period_end.to_string()),
        ("phases[0][proration_behavior]".to_string(), "none".to_string()),
        ("phases[1][items][0][price]".to_string(), price_id.to_string()),
        ("phases[1][items][0][quantity]".to_string(), "1".to_string()),
        ("phases[1][iterations]".to_string(), "1".to_string()),
        ("phases[1][proration_behavior]".to_string(), "none".to_string()),
        ("phases[1][metadata][allternit_plan_id]".to_string(), plan.id.to_string()),
        ("phases[1][metadata][allternit_plan_tier]".to_string(), plan.plan_tier.to_string()),
    ]
}

#[allow(clippy::too_many_arguments)]
pub async fn change(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
    live: LiveSubscription,
    plan_id: &str,
    proration_date: Option<i64>,
    prices: &PlanPrices,
    now: i64,
) -> Result<PlanView, PlanChangeError> {
    let (plan, price_id) = target_plan(plan_id, prices)?;

    if plan.id == live.plan.id {
        // Back to the current plan: undo a pending downgrade, if any.
        let Some(schedule_id) = live.schedule_id.as_deref() else {
            return Err(PlanChangeError::AlreadyOnPlan);
        };
        release_schedule(stripe, secret_key, schedule_id).await?;
        let live = load_subscription(stripe, secret_key, &live.id, customer_id, prices).await?;
        return Ok(plan_view(stripe, secret_key, &live, prices, now, Some("now")).await?);
    }

    if is_upgrade(live.plan, plan) {
        let proration_date = proration_date.unwrap_or(now);
        if proration_date < now - PRORATION_DATE_MAX_AGE_SECS || proration_date > now + PRORATION_DATE_MAX_SKEW_SECS {
            return Err(PlanChangeError::PreviewExpired);
        }
        // A pending downgrade would undo the upgrade at renewal.
        if let Some(schedule_id) = live.schedule_id.as_deref() {
            release_schedule(stripe, secret_key, schedule_id).await?;
        }
        let idempotency_key = format!("plan-change:{}:{}:{}", live.id, plan.id, proration_date);
        let updated = update_subscription(
            stripe,
            secret_key,
            &live.id,
            Some(idempotency_key.as_str()),
            &upgrade_form(&live, plan, price_id, proration_date),
        )
        .await?;
        let mut after = parse_subscription(&updated, prices)?;
        after.schedule_id = None;
        return Ok(plan_view(stripe, secret_key, &after, prices, now, Some("now")).await?);
    }

    // Downgrade at period end through a subscription schedule.
    let period_end = live
        .period_end
        .ok_or_else(|| ApiError::Internal("Stripe subscription has no current period end".to_string()))?;
    if live.cancel_at_period_end {
        // A canceling subscription would turn into a schedule that ends; keep it.
        update_subscription(stripe, secret_key, &live.id, None, &[("cancel_at_period_end".to_string(), "false".to_string())])
            .await?;
    }
    let schedule = match live.schedule_id.as_deref() {
        Some(schedule_id) => {
            stripe.get_object(secret_key, &format!("/v1/subscription_schedules/{schedule_id}"), &[]).await?
        }
        None => {
            stripe
                .post_object(
                    secret_key,
                    "/v1/subscription_schedules",
                    None,
                    &[("from_subscription".to_string(), live.id.clone())],
                )
                .await?
        }
    };
    let schedule_id = object_id(&schedule["id"])
        .ok_or_else(|| ApiError::Internal("Stripe returned no subscription schedule id".to_string()))?
        .to_string();
    // Stripe requires the current phase to keep its start date.
    let phase_start = schedule["current_phase"]["start_date"]
        .as_i64()
        .or_else(|| schedule["phases"][0]["start_date"].as_i64())
        .ok_or_else(|| ApiError::Internal("Stripe subscription schedule has no current phase".to_string()))?;
    stripe
        .post_object(
            secret_key,
            &format!("/v1/subscription_schedules/{schedule_id}"),
            None,
            &downgrade_schedule_form(&live, phase_start, period_end, plan, price_id),
        )
        .await?;
    // The pending change is the one just written; no need to read it back.
    let current = LiveSubscription { cancel_at_period_end: false, schedule_id: None, ..live };
    let mut view = plan_view(stripe, secret_key, &current, prices, now, Some("period_end")).await?;
    view.pending_plan_id = Some(plan.id.to_string());
    view.pending_at = rfc3339(period_end);
    Ok(view)
}

/// Turn `cancel_at_period_end` on or off. Cancelling drops a pending downgrade first.
pub async fn set_cancel_at_period_end(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    live: LiveSubscription,
    cancel: bool,
    prices: &PlanPrices,
    now: i64,
) -> Result<PlanView, PlanChangeError> {
    if cancel {
        if let Some(schedule_id) = live.schedule_id.as_deref() {
            release_schedule(stripe, secret_key, schedule_id).await?;
        }
    }
    let updated = update_subscription(
        stripe,
        secret_key,
        &live.id,
        None,
        &[("cancel_at_period_end".to_string(), cancel.to_string())],
    )
    .await?;
    let mut after = parse_subscription(&updated, prices)?;
    if cancel {
        after.schedule_id = None;
    }
    Ok(plan_view(stripe, secret_key, &after, prices, now, None).await?)
}

async fn audit(db: &PgPool, user_id: &str, action: &str, subscription_id: &str, details: Option<Value>) {
    crate::services::audit::write_audit_log(
        db,
        crate::services::audit::AuditEvent {
            action: action.to_string(),
            resource_type: "billing_subscription".to_string(),
            resource_id: Some(subscription_id.to_string()),
            user_id: Some(user_id.to_string()),
            user_email: None,
            details,
            success: true,
        },
    )
    .await;
}

/// Everything a handler needs, resolved from the session: the user, the
/// key, the prices, the customer and the live subscription.
struct Resolved {
    user_id: String,
    secret_key: String,
    customer_id: String,
    prices: PlanPrices,
    live: LiveSubscription,
}

async fn resolve(state: &ApiState, headers: &HeaderMap, stripe: &dyn StripeCheckout) -> Result<Resolved, Response> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "billing").await.map_err(IntoResponse::into_response)?;
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return Err(billing_not_configured_response());
    };
    let rows = open_plan_rows(&state.db, &user.id).await.map_err(IntoResponse::into_response)?;
    let row = pick_stripe_plan(&rows).map_err(IntoResponse::into_response)?;
    let customer_id = match auto_topup::linked_customer(&state.db, &user.id).await {
        Ok(Some(customer_id)) => customer_id,
        Ok(None) => match row.stripe_customer_id.clone().filter(|id| !id.is_empty()) {
            Some(customer_id) => customer_id,
            None => return Err(PlanChangeError::NoSubscription.into_response()),
        },
        Err(error) => return Err(error.into_response()),
    };
    let prices = PlanPrices::from_env();
    let live = load_subscription(stripe, &secret_key, &row.stripe_subscription_id, &customer_id, &prices)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(Resolved { user_id: user.id, secret_key, customer_id, prices, live })
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn preview_change(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<PreviewRequest>,
) -> Response {
    let stripe = ReqwestStripeCheckout::new();
    let resolved = match resolve(&state, &headers, &stripe).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    match preview(
        &stripe,
        &resolved.secret_key,
        &resolved.customer_id,
        &resolved.live,
        &body.plan_id,
        &resolved.prices,
        now_secs(),
    )
    .await
    {
        Ok(preview) => Json(preview).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn change_plan(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<ChangeRequest>,
) -> Response {
    let stripe = ReqwestStripeCheckout::new();
    let resolved = match resolve(&state, &headers, &stripe).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    let subscription_id = resolved.live.id.clone();
    let from = resolved.live.plan.id;
    match change(
        &stripe,
        &resolved.secret_key,
        &resolved.customer_id,
        resolved.live,
        &body.plan_id,
        body.proration_date,
        &resolved.prices,
        now_secs(),
    )
    .await
    {
        Ok(view) => {
            let details = json!({ "from": from, "to": body.plan_id, "effective": view.effective });
            audit(&state.db, &resolved.user_id, "billing.subscription.changed", &subscription_id, Some(details)).await;
            Json(view).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn cancel_or_resume(state: Arc<ApiState>, headers: HeaderMap, cancel: bool) -> Response {
    let stripe = ReqwestStripeCheckout::new();
    let resolved = match resolve(&state, &headers, &stripe).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    let subscription_id = resolved.live.id.clone();
    match set_cancel_at_period_end(&stripe, &resolved.secret_key, resolved.live, cancel, &resolved.prices, now_secs()).await {
        Ok(view) => {
            let action = if cancel { "billing.subscription.cancel_scheduled" } else { "billing.subscription.resumed" };
            audit(&state.db, &resolved.user_id, action, &subscription_id, None).await;
            Json(view).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn cancel_plan(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    cancel_or_resume(state, headers, true).await
}

async fn resume_plan(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    cancel_or_resume(state, headers, false).await
}

#[cfg(test)]
mod tests {
    //! Uses #3097's in-memory Stripe (`billing_payment_methods::test_support`)
    //! for subscriptions and the write log, plus the subscription, preview and
    //! schedule endpoints this module calls.

    use super::*;
    use crate::routes::billing_payment_methods::test_support::FakeStripe;
    use std::collections::HashMap;
    use std::sync::Mutex;

    const KEY: &str = "sk_test_fake";
    const NOW: i64 = 1_800_000_000;
    const PERIOD_START: i64 = NOW - 10 * 86_400;
    const PERIOD_END: i64 = NOW + 20 * 86_400;

    fn prices() -> PlanPrices {
        PlanPrices::of(&[("plus", "price_plus"), ("super", "price_max"), ("ultra", "price_ultra")])
    }

    fn cents(price_id: &str) -> i64 {
        match price_id {
            "price_plus" => 2_000,
            "price_max" => 10_000,
            "price_ultra" => 20_000,
            _ => 0,
        }
    }

    fn field<'a>(form: &'a [(String, String)], key: &str) -> Option<&'a str> {
        form.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// #3097's FakeStripe plus the plan-change endpoints.
    struct PlanStripe {
        inner: FakeStripe,
        schedules: Mutex<HashMap<String, Value>>,
    }

    fn subscription(id: &str, customer: &str, price: &str) -> Value {
        json!({
            "id": id, "customer": customer, "status": "active", "cancel_at_period_end": false,
            "schedule": null, "current_period_end": PERIOD_END,
            "items": { "data": [{ "id": format!("si_{id}"), "price": { "id": price } }] },
        })
    }

    impl PlanStripe {
        /// `cus_a` is on Plus (`sub_a`); `cus_b` is on Ultra (`sub_b`).
        fn new() -> Self {
            let inner = FakeStripe::two_customers();
            inner.subscriptions.lock().unwrap().insert("cus_a".into(), vec![subscription("sub_a", "cus_a", "price_plus")]);
            inner.subscriptions.lock().unwrap().insert("cus_b".into(), vec![subscription("sub_b", "cus_b", "price_ultra")]);
            Self { inner, schedules: Mutex::new(HashMap::new()) }
        }

        fn with_sub<T>(&self, id: &str, f: impl FnOnce(&mut Value) -> T) -> Option<T> {
            let mut all = self.inner.subscriptions.lock().unwrap();
            all.values_mut().flatten().find(|sub| sub["id"].as_str() == Some(id)).map(f)
        }

        fn sub(&self, id: &str) -> Value {
            self.with_sub(id, |sub| sub.clone()).unwrap()
        }

        fn posts(&self) -> Vec<(String, Vec<(String, String)>)> {
            self.inner.posts.lock().unwrap().clone()
        }

        fn post_to(&self, path: &str) -> Option<Vec<(String, String)>> {
            self.posts().into_iter().rev().find(|(p, _)| p == path).map(|(_, form)| form)
        }

        async fn live(&self, id: &str, customer: &str) -> LiveSubscription {
            load_subscription(self, KEY, id, customer, &prices()).await.unwrap()
        }
    }

    #[async_trait::async_trait]
    impl StripeCheckout for PlanStripe {
        async fn create_checkout_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            Err(ApiError::Internal("not used".into()))
        }

        async fn create_billing_portal_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            Err(ApiError::Internal("not used".into()))
        }

        async fn get_object(&self, key: &str, path: &str, query: &[(String, String)]) -> Result<Value, ApiError> {
            let parts: Vec<&str> = path.trim_start_matches("/v1/").split('/').collect();
            match parts.as_slice() {
                ["subscriptions", id] => {
                    self.inner.gets.lock().unwrap().push((path.to_string(), query.to_vec()));
                    self.with_sub(id, |sub| sub.clone()).ok_or_else(|| ApiError::Internal("No such subscription".into()))
                }
                ["subscription_schedules", id] => {
                    self.schedules.lock().unwrap().get(*id).cloned().ok_or_else(|| ApiError::Internal("No such schedule".into()))
                }
                _ => self.inner.get_object(key, path, query).await,
            }
        }

        async fn post_object(
            &self,
            key: &str,
            path: &str,
            idempotency_key: Option<&str>,
            form: &[(String, String)],
        ) -> Result<Value, ApiError> {
            let parts: Vec<&str> = path.trim_start_matches("/v1/").split('/').collect();
            match parts.as_slice() {
                ["invoices", "create_preview"] => {
                    self.inner.posts.lock().unwrap().push((path.to_string(), form.to_vec()));
                    let sub = self.sub(field(form, "subscription").unwrap());
                    let old = cents(sub["items"]["data"][0]["price"]["id"].as_str().unwrap());
                    let new = cents(field(form, "subscription_details[items][0][price]").unwrap());
                    // Fake proration: two thirds of the period are left.
                    let proration = if field(form, "subscription_details[proration_behavior]") == Some("create_prorations") {
                        (new - old) * 2 / 3
                    } else {
                        0
                    };
                    Ok(json!({ "amount_due": new + proration, "currency": "usd", "next_payment_attempt": PERIOD_END }))
                }
                ["subscriptions", id] => {
                    self.inner.posts.lock().unwrap().push((path.to_string(), form.to_vec()));
                    let schedule = self.with_sub(id, |sub| sub["schedule"].clone()).unwrap();
                    if field(form, "cancel_at_period_end").is_some() && !schedule.is_null() {
                        return Err(ApiError::Internal("subscription is managed by a schedule".into()));
                    }
                    self.with_sub(id, |sub| {
                        if let Some(price) = field(form, "items[0][price]") {
                            sub["items"]["data"][0]["price"] = json!({ "id": price });
                        }
                        if let Some(cancel) = field(form, "cancel_at_period_end") {
                            sub["cancel_at_period_end"] = json!(cancel == "true");
                        }
                        sub.clone()
                    })
                    .ok_or_else(|| ApiError::Internal("No such subscription".into()))
                }
                ["subscription_schedules"] => {
                    self.inner.posts.lock().unwrap().push((path.to_string(), form.to_vec()));
                    let sub_id = field(form, "from_subscription").unwrap().to_string();
                    let price = self.sub(&sub_id)["items"]["data"][0]["price"]["id"].clone();
                    let schedule = json!({
                        "id": "sub_sched_1", "subscription": sub_id,
                        "current_phase": { "start_date": PERIOD_START, "end_date": PERIOD_END },
                        "phases": [{ "start_date": PERIOD_START, "end_date": PERIOD_END, "items": [{ "price": price }] }],
                    });
                    self.schedules.lock().unwrap().insert("sub_sched_1".into(), schedule.clone());
                    self.with_sub(&sub_id, |sub| sub["schedule"] = json!("sub_sched_1"));
                    Ok(schedule)
                }
                ["subscription_schedules", id] => {
                    self.inner.posts.lock().unwrap().push((path.to_string(), form.to_vec()));
                    let mut schedules = self.schedules.lock().unwrap();
                    let schedule = schedules.get_mut(*id).ok_or_else(|| ApiError::Internal("No such schedule".into()))?;
                    let end: i64 = field(form, "phases[0][end_date]").unwrap().parse().unwrap();
                    schedule["phases"] = json!([
                        { "start_date": field(form, "phases[0][start_date]").unwrap().parse::<i64>().unwrap(), "end_date": end,
                          "items": [{ "price": field(form, "phases[0][items][0][price]").unwrap() }] },
                        { "start_date": end, "items": [{ "price": field(form, "phases[1][items][0][price]").unwrap() }] },
                    ]);
                    Ok(schedule.clone())
                }
                ["subscription_schedules", id, "release"] => {
                    self.inner.posts.lock().unwrap().push((path.to_string(), form.to_vec()));
                    let schedule = self.schedules.lock().unwrap().remove(*id).ok_or_else(|| ApiError::Internal("No such schedule".into()))?;
                    let sub_id = schedule["subscription"].as_str().unwrap().to_string();
                    self.with_sub(&sub_id, |sub| sub["schedule"] = Value::Null);
                    Ok(json!({ "id": id, "status": "released" }))
                }
                _ => self.inner.post_object(key, path, idempotency_key, form).await,
            }
        }
    }

    #[tokio::test]
    async fn preview_upgrade_is_the_prorated_next_invoice() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_a", "cus_a").await;
        let preview = preview(&stripe, KEY, "cus_a", &live, "super", &prices(), NOW).await.unwrap();
        // $100 next period + two thirds of the $80 difference.
        assert_eq!(preview.amount_due_usd, 153.33);
        assert_eq!(preview.proration_date, NOW);
        assert_eq!(preview.effective, "now");
        assert_eq!(preview.next_invoice_at, rfc3339(PERIOD_END));
        let form = stripe.post_to("/v1/invoices/create_preview").unwrap();
        assert_eq!(field(&form, "customer"), Some("cus_a"));
        assert_eq!(field(&form, "subscription"), Some("sub_a"));
        assert_eq!(field(&form, "subscription_details[items][0][id]"), Some("si_sub_a"));
        assert_eq!(field(&form, "subscription_details[items][0][price]"), Some("price_max"));
        assert_eq!(field(&form, "subscription_details[proration_behavior]"), Some("create_prorations"));
        assert_eq!(field(&form, "subscription_details[proration_date]"), Some(NOW.to_string().as_str()));
    }

    #[tokio::test]
    async fn preview_downgrade_is_the_new_price_at_renewal() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_b", "cus_b").await;
        let preview = preview(&stripe, KEY, "cus_b", &live, "plus", &prices(), NOW).await.unwrap();
        assert_eq!(preview.amount_due_usd, 20.0);
        assert_eq!(preview.effective, "period_end");
        let form = stripe.post_to("/v1/invoices/create_preview").unwrap();
        assert_eq!(field(&form, "subscription_details[proration_behavior]"), Some("none"));
    }

    #[tokio::test]
    async fn change_up_takes_effect_now_with_prorations() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_a", "cus_a").await;
        let view = change(&stripe, KEY, "cus_a", live, "super", Some(NOW - 30), &prices(), NOW).await.unwrap();
        assert_eq!(view.plan_id, "super");
        assert_eq!(view.effective, Some("now"));
        assert_eq!(view.pending_plan_id, None);
        let form = stripe.post_to("/v1/subscriptions/sub_a").unwrap();
        assert_eq!(field(&form, "items[0][id]"), Some("si_sub_a"));
        assert_eq!(field(&form, "items[0][price]"), Some("price_max"));
        assert_eq!(field(&form, "proration_behavior"), Some("create_prorations"));
        assert_eq!(field(&form, "proration_date"), Some((NOW - 30).to_string().as_str()));
        assert_eq!(field(&form, "metadata[allternit_plan_id]"), Some("super"));
        assert!(stripe.post_to("/v1/subscription_schedules").is_none(), "an upgrade needs no schedule");
        assert_eq!(stripe.sub("sub_a")["items"]["data"][0]["price"]["id"], json!("price_max"));
    }

    #[tokio::test]
    async fn change_up_with_a_stale_preview_is_refused() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_a", "cus_a").await;
        let result = change(&stripe, KEY, "cus_a", live, "ultra", Some(NOW - 2 * 3600), &prices(), NOW).await;
        assert!(matches!(result, Err(PlanChangeError::PreviewExpired)));
        assert!(stripe.post_to("/v1/subscriptions/sub_a").is_none());
    }

    #[tokio::test]
    async fn change_down_waits_for_the_period_end() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_b", "cus_b").await;
        let view = change(&stripe, KEY, "cus_b", live, "plus", None, &prices(), NOW).await.unwrap();
        assert_eq!(view.plan_id, "ultra", "the paid-for plan stays until renewal");
        assert_eq!(view.effective, Some("period_end"));
        assert_eq!(view.pending_plan_id.as_deref(), Some("plus"));
        assert_eq!(view.pending_at, rfc3339(PERIOD_END));
        // The price on the subscription did not move.
        assert!(stripe.post_to("/v1/subscriptions/sub_b").is_none());
        assert_eq!(stripe.sub("sub_b")["items"]["data"][0]["price"]["id"], json!("price_ultra"));
        let created = stripe.post_to("/v1/subscription_schedules").unwrap();
        assert_eq!(field(&created, "from_subscription"), Some("sub_b"));
        let phases = stripe.post_to("/v1/subscription_schedules/sub_sched_1").unwrap();
        assert_eq!(field(&phases, "end_behavior"), Some("release"));
        assert_eq!(field(&phases, "phases[0][items][0][price]"), Some("price_ultra"));
        assert_eq!(field(&phases, "phases[0][start_date]"), Some(PERIOD_START.to_string().as_str()));
        assert_eq!(field(&phases, "phases[0][end_date]"), Some(PERIOD_END.to_string().as_str()));
        assert_eq!(field(&phases, "phases[1][items][0][price]"), Some("price_plus"));
        assert_eq!(field(&phases, "phases[1][proration_behavior]"), Some("none"));

        // Reading it back shows the same pending change; picking Ultra again undoes it.
        let live = stripe.live("sub_b", "cus_b").await;
        assert_eq!(live.schedule_id.as_deref(), Some("sub_sched_1"));
        let view = change(&stripe, KEY, "cus_b", live, "ultra", None, &prices(), NOW).await.unwrap();
        assert_eq!(view.pending_plan_id, None);
        assert!(stripe.post_to("/v1/subscription_schedules/sub_sched_1/release").is_some());
        assert!(stripe.sub("sub_b")["schedule"].is_null());
    }

    #[tokio::test]
    async fn cancel_then_resume() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_a", "cus_a").await;
        let view = set_cancel_at_period_end(&stripe, KEY, live, true, &prices(), NOW).await.unwrap();
        assert!(view.cancel_at_period_end);
        assert_eq!(view.plan_id, "plus");
        assert_eq!(view.current_period_end, rfc3339(PERIOD_END));
        assert_eq!(stripe.sub("sub_a")["cancel_at_period_end"], json!(true));
        assert_eq!(field(&stripe.post_to("/v1/subscriptions/sub_a").unwrap(), "cancel_at_period_end"), Some("true"));

        let live = stripe.live("sub_a", "cus_a").await;
        let view = set_cancel_at_period_end(&stripe, KEY, live, false, &prices(), NOW).await.unwrap();
        assert!(!view.cancel_at_period_end);
        assert_eq!(stripe.sub("sub_a")["cancel_at_period_end"], json!(false));
    }

    #[tokio::test]
    async fn cancel_drops_a_pending_downgrade_first() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_b", "cus_b").await;
        change(&stripe, KEY, "cus_b", live, "plus", None, &prices(), NOW).await.unwrap();
        let live = stripe.live("sub_b", "cus_b").await;
        let view = set_cancel_at_period_end(&stripe, KEY, live, true, &prices(), NOW).await.unwrap();
        assert!(view.cancel_at_period_end);
        assert_eq!(view.pending_plan_id, None);
        assert!(stripe.sub("sub_b")["schedule"].is_null());
    }

    #[tokio::test]
    async fn another_users_subscription_is_unreachable() {
        let stripe = PlanStripe::new();
        // cus_a's session pointed at cus_b's subscription id.
        let result = load_subscription(&stripe, KEY, "sub_b", "cus_a", &prices()).await;
        assert!(matches!(result, Err(PlanChangeError::NoSubscription)));
        assert!(stripe.posts().is_empty(), "nothing is written for a foreign subscription");
        let response = PlanChangeError::NoSubscription.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], json!("no_subscription"));
        assert!(body.get("subscription").is_none() && !body.to_string().contains("sub_b"));
    }

    #[tokio::test]
    async fn store_billed_plan_is_409_billed_by_store() {
        let rows = vec![OpenPlanRow {
            stripe_subscription_id: "apple:500".into(),
            plan_id: "plus".into(),
            provider: "apple".into(),
            stripe_customer_id: None,
        }];
        let error = pick_stripe_plan(&rows).unwrap_err();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], json!("billed_by_store"));
        assert_eq!(body["billing_provider"], json!("apple"));
        assert!(body["message"].as_str().unwrap().contains("App Store"));

        // With a Stripe plan open too, the Stripe one is acted on.
        let mut both = rows.clone();
        both.push(OpenPlanRow {
            stripe_subscription_id: "sub_a".into(),
            plan_id: "super".into(),
            provider: "stripe".into(),
            stripe_customer_id: Some("cus_a".into()),
        });
        assert_eq!(pick_stripe_plan(&both).unwrap().stripe_subscription_id, "sub_a");
        assert!(matches!(pick_stripe_plan(&[]), Err(PlanChangeError::NoSubscription)));
    }

    #[tokio::test]
    async fn same_plan_and_unknown_plan_are_refused() {
        let stripe = PlanStripe::new();
        let live = stripe.live("sub_a", "cus_a").await;
        assert!(matches!(
            change(&stripe, KEY, "cus_a", live.clone(), "plus", None, &prices(), NOW).await,
            Err(PlanChangeError::AlreadyOnPlan)
        ));
        assert!(matches!(
            preview(&stripe, KEY, "cus_a", &live, "gold", &prices(), NOW).await,
            Err(PlanChangeError::UnknownPlan)
        ));
        let no_prices = PlanPrices::of(&[("plus", "price_plus")]);
        assert!(matches!(
            preview(&stripe, KEY, "cus_a", &live, "ultra", &no_prices, NOW).await,
            Err(PlanChangeError::NotConfigured)
        ));
        assert!(stripe.posts().is_empty());
    }
}
