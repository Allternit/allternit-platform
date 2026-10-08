//! A project's payment method and plan (Eoj 2026-10-08: card required, no free
//! usage, sandbox included).
//!
//! * **Console routes** (Clerk session, owner or org admin), mounted with the
//!   other console routes:
//!   - `GET  /api/v1/platform/projects/:id/billing`: plan, whether a card is on
//!     file, the subscription state and the plans on offer.
//!   - `POST /api/v1/platform/projects/:id/billing/checkout` `{"plan":"payg"|"growth"}`:
//!     a Stripe Checkout Session in subscription mode with that plan's prices,
//!     found by `lookup_key` (`stripe_plan::plan_lookup_keys`). Answers
//!     `{"checkout_url"}`. Sandbox projects can only choose Pay as you go
//!     (their usage is never reported to Stripe, so it costs $0).
//!   - `POST /api/v1/platform/projects/:id/billing/portal`: a Stripe customer
//!     portal link for the project's customer (`503 portal_not_configured` when
//!     Stripe has no portal configuration).
//! * **Webhook** ([`handle_stripe_event`]): the shared Stripe route
//!   (`billing_webhooks`, same signature check) hands every event here first.
//!   Events carrying `allternit_platform_project_id` metadata (Checkout Session,
//!   subscription) or an invoice of a project's subscription are handled here;
//!   everything else goes on to the hosted-compute handler.
//!   - Checkout completed (paid, or nothing due): sets `stripe_customer_id`,
//!     `stripe_subscription_id`, `plan` and `billing_status = active`; a
//!     previous subscription of the project (plan change) is cancelled, its
//!     usage to date invoiced.
//!   - Subscription created/updated/deleted: `active`/`trialing` → `active`,
//!     `past_due` → `past_due` (Stripe still retrying: work continues),
//!     `unpaid`/`paused` → `unpaid`, `canceled`/`incomplete_expired` →
//!     `canceled` and the plan back to `sandbox`. The last two refuse billable
//!     work with `402 payment_method_required`.
//!   - `invoice.payment_failed` → `past_due`; `invoice.paid` → `active`, and on
//!     a live Growth project's subscription invoice (`subscription_create` /
//!     `subscription_cycle`) a **$300 Stripe credit grant** on its metered
//!     prices for the period (`platform_credit_grants` dedupes per invoice).
//!
//! Each project has its own Stripe customer: Stripe meters sum usage per
//! customer, so two projects sharing one would bill each other's usage.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::{billing, console, projects, stripe_plan, ApiJson, PlatformError, ProjectEnv};
use crate::{
    routes::billing_checkout::{ReqwestStripeCheckout, StripeCheckout},
    ApiState,
};

/// Metadata keys on the Checkout Session and its subscription.
pub const META_PROJECT: &str = "allternit_platform_project_id";
pub const META_PLAN: &str = "allternit_platform_plan";
const META_OWNER: &str = "allternit_platform_owner";
/// Growth's monthly usage credit, in cents (spec §7).
pub const GROWTH_CREDIT_CENTS: i64 = 30_000;
/// Version of the developer terms and AUP a console user accepts.
pub const TERMS_VERSION: &str = "2026-10-08";
pub const TERMS_URL: &str = "https://docs.allternit.com/legal/developer-terms";
pub const AUP_URL: &str = "https://docs.allternit.com/legal/acceptable-use-policy";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/platform/projects/:id/billing", get(get_billing))
        .route("/api/v1/platform/projects/:id/billing/checkout", post(create_checkout))
        .route("/api/v1/platform/projects/:id/billing/portal", post(create_portal))
}

fn console_url() -> String {
    std::env::var("ALLTERNIT_PLATFORM_CONSOLE_URL")
        .unwrap_or_else(|_| "https://platform.allternit.com".into())
        .trim_end_matches('/')
        .to_string()
}

fn stripe_secret() -> Result<String, PlatformError> {
    std::env::var("STRIPE_SECRET_KEY").ok().filter(|k| !k.is_empty()).ok_or_else(|| {
        PlatformError::service_unavailable("billing_not_configured", "Billing isn't set up on this deployment yet.")
    })
}

#[derive(Debug, sqlx::FromRow)]
struct BillingRow {
    plan: String,
    env: String,
    stripe_customer_id: Option<String>,
    stripe_subscription_id: Option<String>,
    billing_status: Option<String>,
    payment_method_added_at: Option<DateTime<Utc>>,
    spend_cap_cents: i64,
}

async fn billing_row(db: &PgPool, project_id: &str) -> Result<BillingRow, PlatformError> {
    Ok(sqlx::query_as::<_, BillingRow>(
        "SELECT plan, env, stripe_customer_id, stripe_subscription_id, billing_status, payment_method_added_at, spend_cap_cents \
         FROM platform_projects WHERE id = $1",
    )
    .bind(project_id)
    .fetch_one(db)
    .await?)
}

fn plans_json(env: &str) -> Value {
    json!([
        { "id": "payg", "name": "Pay as you go", "monthly_cents": 0, "usage_credit_cents": 0, "available": true,
          "summary": "No monthly fee. Usage at list prices, $100 default monthly spend cap." },
        { "id": "growth", "name": "Growth", "monthly_cents": stripe_plan::GROWTH_BASE_CENTS, "usage_credit_cents": GROWTH_CREDIT_CENTS,
          "available": env == "live",
          "summary": "$249 a month with $300 of usage credit each month and 10% off usage. Live projects only." },
    ])
}

async fn get_billing(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    let (who, _) = console::principal(&headers).await?;
    let project = projects::get_project(&state.db, &who, &id).await?;
    let row = billing_row(&state.db, &project.id).await?;
    let on_file = row.stripe_customer_id.is_some() && billing::status_allows_work(row.billing_status.as_deref());
    Ok(Json(json!({
        "object": "project_billing",
        "project_id": project.id,
        "env": row.env,
        "plan": row.plan,
        "payment_method": on_file,
        "status": row.billing_status,
        "subscription_id": row.stripe_subscription_id,
        "payment_method_added_at": row.payment_method_added_at,
        "portal_available": row.stripe_customer_id.is_some(),
        "checkout_available": stripe_secret().is_ok(),
        "spend_cap_cents": row.spend_cap_cents,
        "plans": plans_json(&row.env),
        "terms_url": TERMS_URL,
        "acceptable_use_url": AUP_URL,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckoutBody {
    plan: String,
}

/// Form fields of `POST /v1/checkout/sessions` for a project's plan.
pub fn checkout_form(
    project_id: &str,
    owner: &str,
    plan: &str,
    price_ids: &[(String, String)],
    customer: Option<&str>,
    email: Option<&str>,
    console: &str,
) -> Vec<(String, String)> {
    let page = format!("{console}/platform/billing?project={project_id}");
    let mut form = vec![
        ("mode".to_string(), "subscription".to_string()),
        ("success_url".to_string(), format!("{page}&checkout=success")),
        ("cancel_url".to_string(), format!("{page}&checkout=cancelled")),
        ("client_reference_id".to_string(), project_id.to_string()),
        ("payment_method_types[0]".to_string(), "card".to_string()),
        ("payment_method_collection".to_string(), "always".to_string()),
        (format!("metadata[{META_PROJECT}]"), project_id.to_string()),
        (format!("metadata[{META_PLAN}]"), plan.to_string()),
        (format!("subscription_data[metadata][{META_PROJECT}]"), project_id.to_string()),
        (format!("subscription_data[metadata][{META_PLAN}]"), plan.to_string()),
        (format!("subscription_data[metadata][{META_OWNER}]"), owner.to_string()),
    ];
    for (i, (lookup_key, price_id)) in price_ids.iter().enumerate() {
        form.push((format!("line_items[{i}][price]"), price_id.clone()));
        // Metered prices take no quantity; the Growth base fee is one per month.
        if lookup_key == stripe_plan::GROWTH_BASE_LOOKUP_KEY {
            form.push((format!("line_items[{i}][quantity]"), "1".to_string()));
        }
    }
    match (customer, email) {
        (Some(c), _) => form.push(("customer".to_string(), c.to_string())),
        (None, Some(e)) => form.push(("customer_email".to_string(), e.to_string())),
        (None, None) => {}
    }
    form
}

/// The plan's prices by lookup key, in plan order. Stripe takes at most 10
/// lookup keys per list request.
async fn lookup_prices(stripe: &dyn StripeCheckout, secret: &str, plan: &str) -> Result<Vec<(String, String)>, PlatformError> {
    let keys = stripe_plan::plan_lookup_keys(plan);
    let mut found = std::collections::HashMap::new();
    for chunk in keys.chunks(10) {
        let mut query = vec![("active".to_string(), "true".to_string()), ("limit".to_string(), "100".to_string())];
        query.extend(chunk.iter().map(|k| ("lookup_keys[]".to_string(), k.clone())));
        let list = stripe.get_object(secret, "/v1/prices", &query).await.map_err(|e| {
            tracing::error!("platform checkout: price lookup failed: {e}");
            PlatformError::service_unavailable("billing_unavailable", "Stripe couldn't be reached. Retry shortly.")
        })?;
        for price in list["data"].as_array().into_iter().flatten() {
            if let (Some(k), Some(id)) = (price["lookup_key"].as_str(), price["id"].as_str()) {
                found.insert(k.to_string(), id.to_string());
            }
        }
    }
    let missing: Vec<&String> = keys.iter().filter(|k| !found.contains_key(*k)).collect();
    if !missing.is_empty() {
        tracing::error!(?missing, "platform checkout: Stripe prices not set up");
        return Err(PlatformError::service_unavailable("billing_not_configured", "This plan's prices aren't set up in Stripe yet."));
    }
    Ok(keys.into_iter().map(|k| { let id = found[&k].clone(); (k, id) }).collect())
}

async fn create_checkout(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CheckoutBody>,
) -> Result<Json<Value>, PlatformError> {
    let (who, email) = console::principal(&headers).await?;
    let project = projects::get_project(&state.db, &who, &id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::conflict("project_archived", "This project is archived."));
    }
    let plan = body.plan.as_str();
    if !matches!(plan, "payg" | "growth") {
        return Err(PlatformError::invalid_request("invalid_plan", "plan must be 'payg' or 'growth'.").with_param("plan"));
    }
    if plan == "growth" && ProjectEnv::parse(&project.env) != Some(ProjectEnv::Live) {
        return Err(PlatformError::invalid_request("growth_needs_live_project", "Growth is for live projects. A sandbox project needs Pay as you go, which costs nothing there.").with_param("plan"));
    }
    let row = billing_row(&state.db, &project.id).await?;
    if billing::status_allows_work(row.billing_status.as_deref()) && row.plan == plan {
        return Err(PlatformError::conflict("plan_already_active", "This project is already on that plan."));
    }
    let secret = stripe_secret()?;
    let stripe = ReqwestStripeCheckout::new();
    let prices = lookup_prices(&stripe, &secret, plan).await?;
    let form = checkout_form(&project.id, &who.user_id, plan, &prices, row.stripe_customer_id.as_deref(), email.as_deref(), &console_url());
    let url = stripe.create_checkout_session(&secret, &form).await.map_err(|e| {
        tracing::error!(project = %project.id, "platform checkout: session not created: {e}");
        PlatformError::service_unavailable("billing_unavailable", "Stripe couldn't start checkout. Retry shortly.")
    })?;
    Ok(Json(json!({ "object": "checkout_session", "checkout_url": url, "plan": plan })))
}

async fn create_portal(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    let (who, _) = console::principal(&headers).await?;
    let project = projects::get_project(&state.db, &who, &id).await?;
    let row = billing_row(&state.db, &project.id).await?;
    let Some(customer) = row.stripe_customer_id else {
        return Err(PlatformError::not_found("billing_no_customer", "This project has no payment method yet. Choose a plan first."));
    };
    let secret = stripe_secret()?;
    let form = vec![
        ("customer".to_string(), customer),
        ("return_url".to_string(), format!("{}/platform/billing?project={}", console_url(), project.id)),
    ];
    let url = ReqwestStripeCheckout::new().create_billing_portal_session(&secret, &form).await.map_err(|e| {
        tracing::warn!(project = %project.id, "platform billing portal not available: {e}");
        PlatformError::service_unavailable("portal_not_configured", "The billing portal isn't available yet. Contact support to change your card or cancel.")
    })?;
    Ok(Json(json!({ "object": "portal_session", "portal_url": url })))
}

// ─── Webhook ────────────────────────────────────────────────────────────────

/// A Stripe id that may come expanded (`{"id": …}`) or as a string.
fn id_of(v: &Value) -> Option<String> {
    v.as_str().or_else(|| v["id"].as_str()).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Our billing status for a Stripe subscription status (`None` = leave as is).
pub fn status_for(stripe_status: &str) -> Option<&'static str> {
    Some(match stripe_status {
        "active" | "trialing" => "active",
        "past_due" => "past_due",
        "unpaid" | "paused" => "unpaid",
        "canceled" | "incomplete_expired" => "canceled",
        _ => return None,
    })
}

fn valid_plan(plan: Option<&str>) -> Option<&str> {
    plan.filter(|p| matches!(*p, "payg" | "growth"))
}

/// Checkout completed: the project has a card on file and a plan (a sandbox
/// project keeps plan `sandbox` and its sandbox limits). Returns the
/// project's previous subscription when this one replaces it.
pub async fn apply_checkout(db: &PgPool, project_id: &str, plan: &str, customer: &str, subscription: &str) -> Result<Option<String>, sqlx::Error> {
    let previous: Option<Option<String>> = sqlx::query_scalar("SELECT stripe_subscription_id FROM platform_projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(db)
        .await?;
    sqlx::query(
        "UPDATE platform_projects SET stripe_customer_id = $2, stripe_subscription_id = $3, \
         plan = CASE WHEN env = 'live' THEN $4 ELSE plan END, billing_status = 'active', \
         payment_method_added_at = COALESCE(payment_method_added_at, NOW()) WHERE id = $1",
    )
    .bind(project_id)
    .bind(customer)
    .bind(subscription)
    .bind(plan)
    .execute(db)
    .await?;
    Ok(previous.flatten().filter(|p| p != subscription))
}

/// A subscription changed state. An event for a subscription that is no longer
/// the project's is ignored unless it is the active one (plan change).
pub async fn apply_subscription(db: &PgPool, project_id: &str, subscription: &str, customer: Option<&str>, plan: Option<&str>, status: &str) -> Result<u64, sqlx::Error> {
    let plan = if status == "canceled" { Some("sandbox") } else if status == "active" { valid_plan(plan) } else { None };
    let done = sqlx::query(
        "UPDATE platform_projects SET billing_status = $3, stripe_subscription_id = $2, \
         stripe_customer_id = COALESCE($4, stripe_customer_id), \
         plan = CASE WHEN env = 'live' OR $5 = 'sandbox' THEN COALESCE($5, plan) ELSE plan END, \
         payment_method_added_at = CASE WHEN $3 = 'active' THEN COALESCE(payment_method_added_at, NOW()) ELSE payment_method_added_at END \
         WHERE id = $1 AND (stripe_subscription_id IS NULL OR stripe_subscription_id = $2 OR $3 = 'active')",
    )
    .bind(project_id)
    .bind(subscription)
    .bind(status)
    .bind(customer)
    .bind(plan)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// The subscription an invoice belongs to (old and 2025+ API shapes).
fn invoice_subscription(invoice: &Value) -> Option<String> {
    id_of(&invoice["subscription"]).or_else(|| id_of(&invoice["parent"]["subscription_details"]["subscription"]))
}

/// The project whose current subscription is `subscription`.
async fn project_for_subscription(db: &PgPool, subscription: &str) -> Option<(String, String, String, Option<String>)> {
    sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT id, plan, env, stripe_customer_id FROM platform_projects WHERE stripe_subscription_id = $1",
    )
    .bind(subscription)
    .fetch_optional(db)
    .await
    .unwrap_or_else(|error| {
        tracing::debug!(%error, "platform subscription lookup skipped");
        None
    })
}

/// Growth credit grant form for one paid subscription invoice.
pub fn credit_grant_form(project_id: &str, customer: &str, invoice_id: &str, expires_at: i64) -> Vec<(String, String)> {
    vec![
        ("customer".into(), customer.into()),
        ("name".into(), "Growth plan monthly usage credit".into()),
        ("category".into(), "paid".into()),
        ("amount[type]".into(), "monetary".into()),
        ("amount[monetary][currency]".into(), "usd".into()),
        ("amount[monetary][value]".into(), GROWTH_CREDIT_CENTS.to_string()),
        ("applicability_config[scope][price_type]".into(), "metered".into()),
        ("expires_at".into(), expires_at.to_string()),
        (format!("metadata[{META_PROJECT}]"), project_id.into()),
        ("metadata[allternit_invoice]".into(), invoice_id.into()),
    ]
}

/// When a period's credit expires: the end of the period the invoice opened
/// (the latest line period end), plus two days so the invoice that bills that
/// period's usage can still use it.
fn credit_expiry(invoice: &Value) -> i64 {
    let end = invoice["lines"]["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["period"]["end"].as_i64())
        .max()
        .filter(|e| *e > Utc::now().timestamp())
        .unwrap_or_else(|| Utc::now().timestamp() + 31 * 86_400);
    end + 2 * 86_400
}

async fn grant_growth_credit(db: &PgPool, project_id: &str, customer: &str, invoice: &Value) -> Result<bool, String> {
    let invoice_id = invoice["id"].as_str().ok_or("invoice without id")?;
    let done: Option<String> = sqlx::query_scalar("SELECT invoice_id FROM platform_credit_grants WHERE invoice_id = $1")
        .bind(invoice_id)
        .fetch_optional(db)
        .await
        .map_err(|e| e.to_string())?;
    if done.is_some() {
        return Ok(false);
    }
    let secret = stripe_secret().map_err(|e| e.message)?;
    let expires = credit_expiry(invoice);
    let form = credit_grant_form(project_id, customer, invoice_id, expires);
    let grant = ReqwestStripeCheckout::new()
        .post_object(&secret, "/v1/billing/credit_grants", Some(&format!("allternit-platform-credit-{invoice_id}")), &form)
        .await
        .map_err(|e| e.to_string())?;
    let grant_id = grant["id"].as_str().unwrap_or_default().to_string();
    sqlx::query(
        "INSERT INTO platform_credit_grants (invoice_id, project_id, stripe_customer_id, stripe_credit_grant_id, amount_cents, expires_at) \
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6)) ON CONFLICT (invoice_id) DO NOTHING",
    )
    .bind(invoice_id)
    .bind(project_id)
    .bind(customer)
    .bind(&grant_id)
    .bind(GROWTH_CREDIT_CENTS)
    .bind(expires as f64)
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(true)
}

fn handled(kind: &str, details: Value) -> Response {
    let mut body = json!({ "received": true, "platform": true, "type": kind });
    if let (Some(b), Some(d)) = (body.as_object_mut(), details.as_object()) {
        b.extend(d.clone());
    }
    Json(body).into_response()
}

fn failed(kind: &str, why: impl std::fmt::Display) -> Response {
    tracing::error!(event = kind, "platform stripe webhook failed: {why}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "platform_billing_update_failed" }))).into_response()
}

/// Handle a verified Stripe event if it belongs to a Platform API project.
/// `None` = not ours; the caller carries on with its own handling.
pub async fn handle_stripe_event(state: &ApiState, event: &Value) -> Option<Response> {
    let kind = event["type"].as_str()?;
    let obj = &event["data"]["object"];
    let db = &state.db;
    let meta_project = obj["metadata"][META_PROJECT].as_str().filter(|s| !s.is_empty());
    match kind {
        "checkout.session.completed" | "checkout.session.async_payment_succeeded" => {
            let project = meta_project?;
            if obj["mode"].as_str() != Some("subscription") {
                return Some(handled(kind, json!({ "ignored": "not a subscription checkout" })));
            }
            if !matches!(obj["payment_status"].as_str(), Some("paid" | "no_payment_required")) {
                // An async payment method: async_payment_succeeded follows.
                return Some(handled(kind, json!({ "ignored": "payment not settled" })));
            }
            let (Some(customer), Some(subscription)) = (id_of(&obj["customer"]), id_of(&obj["subscription"])) else {
                return Some(failed(kind, "checkout session without customer or subscription"));
            };
            let plan = valid_plan(obj["metadata"][META_PLAN].as_str()).unwrap_or("payg");
            let previous = match apply_checkout(db, project, plan, &customer, &subscription).await {
                Ok(p) => p,
                Err(e) => return Some(failed(kind, e)),
            };
            if let Some(old) = previous {
                // A plan change: end the old subscription now and invoice its usage to date,
                // so the same meters aren't billed by two subscriptions.
                let cancelled = match stripe_secret() {
                    Ok(secret) => ReqwestStripeCheckout::new()
                        .delete_object(&secret, &format!("/v1/subscriptions/{old}"), &[("invoice_now".into(), "true".into()), ("prorate".into(), "true".into())])
                        .await
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.message),
                };
                if let Err(e) = cancelled {
                    tracing::error!(project, old = %old, "platform: previous subscription not cancelled, cancel it in Stripe: {e}");
                }
            }
            write_audit(state, project, "platform_billing.checkout_completed", json!({ "plan": plan, "subscription": subscription })).await;
            Some(handled(kind, json!({ "project_id": project, "plan": plan, "billing_status": "active" })))
        }
        "customer.subscription.created" | "customer.subscription.updated" | "customer.subscription.deleted" => {
            let project = meta_project?;
            let subscription = id_of(&obj["id"])?;
            let stripe_status = if kind == "customer.subscription.deleted" { "canceled" } else { obj["status"].as_str().unwrap_or_default() };
            let Some(status) = status_for(stripe_status) else {
                return Some(handled(kind, json!({ "ignored": format!("status {stripe_status}") })));
            };
            let customer = id_of(&obj["customer"]);
            match apply_subscription(db, project, &subscription, customer.as_deref(), obj["metadata"][META_PLAN].as_str(), status).await {
                Ok(n) => {
                    if n > 0 && status != "active" {
                        write_audit(state, project, "platform_billing.status", json!({ "status": status, "subscription": subscription })).await;
                    }
                    Some(handled(kind, json!({ "project_id": project, "billing_status": status, "updated": n > 0 })))
                }
                Err(e) => Some(failed(kind, e)),
            }
        }
        "invoice.paid" | "invoice.payment_failed" => {
            let subscription = invoice_subscription(obj)?;
            let (project, plan, env, customer) = project_for_subscription(db, &subscription).await?;
            if kind == "invoice.payment_failed" {
                return Some(match apply_subscription(db, &project, &subscription, None, None, "past_due").await {
                    Ok(_) => {
                        write_audit(state, &project, "platform_billing.payment_failed", json!({ "invoice": obj["id"] })).await;
                        handled(kind, json!({ "project_id": project, "billing_status": "past_due" }))
                    }
                    Err(e) => failed(kind, e),
                });
            }
            if let Err(e) = sqlx::query("UPDATE platform_projects SET billing_status = 'active' WHERE id = $1 AND billing_status = 'past_due'")
                .bind(&project)
                .execute(db)
                .await
            {
                return Some(failed(kind, e));
            }
            let reason = obj["billing_reason"].as_str().unwrap_or_default();
            let mut granted = false;
            if plan == "growth" && env == "live" && matches!(reason, "subscription_create" | "subscription_cycle") {
                let Some(customer) = customer.or_else(|| id_of(&obj["customer"])) else {
                    return Some(failed(kind, "growth invoice without a customer"));
                };
                match grant_growth_credit(db, &project, &customer, obj).await {
                    Ok(g) => granted = g,
                    Err(e) => return Some(failed(kind, e)),
                }
            }
            Some(handled(kind, json!({ "project_id": project, "credit_granted": granted })))
        }
        _ => None,
    }
}

async fn write_audit(state: &ApiState, project_id: &str, action: &str, details: Value) {
    crate::services::audit::write_audit_log(
        &state.db,
        crate::services::audit::AuditEvent {
            action: action.to_string(),
            resource_type: "platform_project".to_string(),
            resource_id: Some(project_id.to_string()),
            user_id: None,
            user_email: None,
            details: Some(details),
            success: true,
        },
    )
    .await;
}

/// Tests: give a project a card on file, as a completed Checkout would.
/// Leaves the plan alone, so plan-specific tests keep their plan.
#[cfg(test)]
pub async fn put_test_card(db: &PgPool, project_id: &str) {
    sqlx::query("UPDATE platform_projects SET stripe_customer_id = 'cus_test_' || id, stripe_subscription_id = 'sub_test_' || id, billing_status = 'active' WHERE id = $1")
        .bind(project_id)
        .execute(db)
        .await
        .expect("test card");
}
