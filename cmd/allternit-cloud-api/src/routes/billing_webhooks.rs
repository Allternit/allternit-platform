//! Stripe webhook receiver that syncs subscription state into hosted-compute
//! entitlements and grants one-off credit purchases.
//!
//! Stripe signs every delivery with the `Stripe-Signature` v1 scheme
//! (`t=<unix>,v1=<hmac-sha256 hex>` over `"{t}.{raw body}"`); the endpoint
//! secret comes from `STRIPE_WEBHOOK_SECRET`. The event `id` is the
//! idempotency key, so Stripe retries reuse the `billing_entitlement_events`
//! dedupe that `routes::hosted_entitlements` already provides — both surfaces
//! mutate through the same `apply_hosted_entitlement`.
//!
//! Metadata convention (set on the Stripe subscription at checkout/portal
//! time):
//! - `clerk_user_id` — the Clerk user that owns the entitlement. If the
//!   subscription metadata lacks it, an expanded customer object's metadata
//!   is consulted as a fallback.
//! - `allternit_plan_tier` — the `plan_tiers.id` to grant while the
//!   subscription is active or trialing (e.g. `pro`, `team`).
//! - `allternit_plan_id` — the subscription plan id (`plus`/`super`/`ultra`),
//!   used to resolve the monthly credit grant and rollover cap.
//!
//! Beyond the tier grant, `customer.subscription.created/updated` also upserts
//! the local subscription mirror (`billing_subscriptions` + `user_billing_accounts`
//! in `routes::billing_subscriptions`) so monthly credit grants and portal sessions
//! can resolve Stripe ids without re-reading subscription metadata; deletion marks
//! the mirror `canceled`. `invoice.paid` with `billing_reason` of
//! `subscription_create`/`subscription_cycle` grants the plan's monthly credits
//! (rollover-capped) — invoice metadata is NOT subscription metadata, which is
//! exactly why the mirror table exists.
//!
//! `customer.subscription.deleted` revokes by moving the user back to the
//! deployment's default tier (`DEFAULT_PLAN_TIER`, default `free`); plan
//! access is additive state in `user_runtime_quotas`, so revocation is the
//! same idempotent mutation with the free tier as its target.
//!
//! One-off credit purchases (G15) use a separate metadata contract on the
//! payment object (the Checkout Session for `checkout.session.completed` /
//! `checkout.session.async_payment_succeeded`, the invoice for
//! `invoice.paid`):
//! - `clerk_user_id` — the Clerk user to credit.
//! - `allternit_credits_usd` — the credit amount in USD (must parse as a
//!   positive number). Written by `routes::billing_checkout` from the static
//!   server-side pack catalog, never from client input.
//! - `allternit_org_id` — the Clerk organization that owns the fabric
//!   credits ledger to grant. Absent on sessions created while the
//!   fabric-ledger bridge was off; those grants fall back to the cloud
//!   wallet.
//!
//! Grant rules for checkout sessions: only `mode = "payment"` and only when
//! `payment_status = "paid"`. `checkout.session.completed` can arrive with
//! `payment_status = "unpaid"` for asynchronous payment methods (bank
//! debits); Stripe then delivers `checkout.session.async_payment_succeeded`
//! when the money actually settles, and THAT event is the grant trigger.
//!
//! Where the credits land depends on the fabric-ledger bridge
//! (`services::fabric_ledger`): when `ALLTERNIT_FABRIC_LEDGER_URL` +
//! `ALLTERNIT_INTERNAL_SERVICE_TOKEN` are configured and the event carries
//! `allternit_org_id`, the grant goes to the allternit-api org ledger over
//! an internal-token-authenticated HTTP call whose idempotency key is
//! `stripe-{event_id}`. Otherwise the grant lands in the local cloud wallet
//! via `CostService::add_credits`, deduped by the
//! `credit_transactions.transaction_id` uniqueness.
//!
//! Either way the processed event id is recorded in `webhook_events`
//! (migration 015) AFTER the grant succeeds: a Stripe retry is acknowledged
//! as a replay without re-calling either ledger, and enabling the bridge
//! later cannot re-deliver an event that already granted somewhere. The
//! ledger idempotency key remains the money guarantee; `webhook_events` is
//! the first-line dedupe and the audit trail. If the bridge URL is set but
//! the token is missing the deployment is misconfigured: the handler errors
//! (Stripe retries, the operator sees the log) instead of silently crediting
//! the wrong ledger.

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;

use super::hosted_entitlements::apply_hosted_entitlement;
use super::{billing_subscriptions, hosted_entitlements::AppliedEntitlement};
use crate::{error::ApiError, services::CostService, ApiState};
use sqlx::PgPool;

const STRIPE_SIGNATURE_HEADER: &str = "stripe-signature";
const SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

/// How one Stripe event maps onto an entitlement change.
#[derive(Debug, Clone, PartialEq)]
enum MappedStripeEvent {
    /// Active/trialing subscription: grant the metadata plan tier.
    Grant {
        event_id: String,
        user_id: String,
        plan_tier_id: String,
    },
    /// Deleted subscription: fall back to the deployment's default tier.
    Revoke { event_id: String, user_id: String },
    /// A renewal payment failed (Eoj 2026-10-01: grace, then suspend).
    /// `past_due`: Stripe is still retrying, so access stays and the mirror
    /// row records it for the "update your card" banner. `unpaid`: Stripe
    /// gave up, so the user drops to the default tier and the cloud computer
    /// is stopped (kept, not scheduled for deletion; paying resumes it).
    PaymentFailed {
        event_id: String,
        user_id: String,
        status: String,
    },
    /// One-off credit purchase: grant prepaid credits to the user (cloud
    /// wallet) or their organization (fabric ledger, when the bridge is
    /// configured and the event names an org).
    GrantCredits {
        event_id: String,
        user_id: String,
        amount_usd: f64,
        organization_id: Option<String>,
    },
    /// Paid subscription invoice (subscription_create / subscription_cycle): grant the
    /// plan's monthly credits with the rollover cap, resolving user and plan through
    /// the billing_subscriptions mirror.
    GrantSubscriptionCredits {
        event_id: String,
        invoice_id: String,
        subscription_id: String,
    },
    /// Any other event type (or a non-billable subscription status).
    Ignored(String),
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/webhooks/stripe", post(stripe_webhook))
}

fn webhook_not_configured_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "webhook_not_configured" })),
    )
        .into_response()
}

fn ignored_response(event_type: &str) -> Response {
    Json(json!({ "received": true, "ignored": event_type })).into_response()
}

async fn stripe_webhook(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(secret) = std::env::var("STRIPE_WEBHOOK_SECRET") else {
        return webhook_not_configured_response();
    };
    let signature_header = headers
        .get(STRIPE_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if let Err(error) = verify_stripe_signature(
        &secret,
        signature_header,
        &body,
        chrono::Utc::now().timestamp(),
    ) {
        return error.into_response();
    }
    let event: Value = match serde_json::from_slice(&body) {
        Ok(event) => event,
        Err(_) => {
            return ApiError::BadRequest("Invalid Stripe event payload".to_string())
                .into_response();
        }
    };

    let mapped = match map_stripe_event(&event) {
        Ok(mapped) => mapped,
        Err(error) => return error.into_response(),
    };
    match mapped {
        MappedStripeEvent::Ignored(event_type) => ignored_response(&event_type),
        MappedStripeEvent::Grant {
            event_id,
            user_id,
            plan_tier_id,
        } => apply_and_respond(&state, &event["data"]["object"], &event_id, &user_id, &plan_tier_id).await,
        MappedStripeEvent::Revoke { event_id, user_id } => {
            let default_tier =
                std::env::var("DEFAULT_PLAN_TIER").unwrap_or_else(|_| "free".to_string());
            revoke_and_respond(&state, &event["data"]["object"], &event_id, &user_id, &default_tier).await
        }
        MappedStripeEvent::PaymentFailed {
            event_id,
            user_id,
            status,
        } => {
            payment_failed_and_respond(&state, &event["data"]["object"], &event_id, &user_id, &status)
                .await
        }
        MappedStripeEvent::GrantCredits {
            event_id,
            user_id,
            amount_usd,
            organization_id,
        } => {
            let event_type = event["type"].as_str().unwrap_or_default().to_string();
            grant_credits_and_respond(
                &state,
                &event_type,
                &event_id,
                &user_id,
                organization_id.as_deref(),
                amount_usd,
            )
            .await
        }
        MappedStripeEvent::GrantSubscriptionCredits {
            event_id,
            invoice_id,
            subscription_id,
        } => {
            grant_subscription_credits_and_respond(&state, &event_id, &invoice_id, &subscription_id)
                .await
        }
    }
}

async fn apply_and_respond(
    state: &ApiState,
    subscription: &Value,
    event_id: &str,
    user_id: &str,
    plan_tier_id: &str,
) -> Response {
    let provisioner = provision_on_payment_enabled().then_some(&state.provisioning_service);
    match apply_entitlement_and_sync_subscription(
        &state.db,
        subscription,
        event_id,
        user_id,
        plan_tier_id,
        provisioner,
    )
    .await
    {
        Ok((applied, _provisioning)) => {
            crate::services::audit::write_audit_log(
                &state.db,
                crate::services::audit::AuditEvent {
                    action: "billing.subscription.granted".to_string(),
                    resource_type: "billing_entitlement".to_string(),
                    resource_id: Some(event_id.to_string()),
                    user_id: Some(user_id.to_string()),
                    user_email: None,
                    details: Some(json!({
                        "planTierId": plan_tier_id,
                        "idempotentReplay": applied.idempotent_replay,
                    })),
                    success: true,
                },
            )
            .await;
            Json(json!({
                "received": true,
                "eventId": event_id,
                "userId": user_id,
                "planTierId": plan_tier_id,
                "idempotentReplay": applied.idempotent_replay,
            }))
            .into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Grant the entitlement, then mirror the Stripe subscription into the local bookkeeping
/// tables (`billing_subscriptions`, `user_billing_accounts`) so the invoice.paid grant
/// path can resolve the subscription id back to a user and plan — invoice metadata is
/// NOT subscription metadata, which is exactly why the mirror table exists. The event
/// mapping already guaranteed clerk_user_id metadata, so these upserts only ever run for
/// subscriptions from our own checkout flow.
/// Plan B1 go-live switch: `ALLTERNIT_PROVISION_ON_PAYMENT=1` makes an
/// active/trialing subscription create its cloud computer. Off by default so
/// merging (which auto-deploys cloud-api) does not start creating containers
/// on the fleet before the image carries the Desktop app (A1) and the Stripe
/// test-mode end-to-end run (B verify) has passed. Cancel handling is always
/// on: it only acts on computers that already exist.
fn provision_on_payment_enabled() -> bool {
    crate::services::provisioning::provision_on_payment_enabled()
}

/// Every caller of the mirror upsert below is an active/trialing grant
/// (map_stripe_event filters other statuses), so the presence of a
/// provisioner is the only gate here.
async fn apply_entitlement_and_sync_subscription(
    db: &PgPool,
    subscription: &Value,
    event_id: &str,
    user_id: &str,
    plan_tier_id: &str,
    provisioner: Option<&Arc<crate::services::ProvisioningService>>,
) -> Result<(AppliedEntitlement, Option<tokio::task::JoinHandle<()>>), ApiError> {
    let applied =
        apply_hosted_entitlement(db, event_id, user_id, plan_tier_id, None, "stripe").await?;
    let subscription_id = subscription["id"].as_str().unwrap_or_default();
    if !subscription_id.is_empty() {
        let plan_id = crate::routes::billing_subscriptions::current_subscription_plan(subscription)
            .map(|plan| plan.id)
            .unwrap_or_default();
        let status = subscription["status"].as_str().unwrap_or_default();
        let customer_id = subscription_customer_id(subscription);
        billing_subscriptions::upsert_billing_subscription(
            db,
            subscription_id,
            user_id,
            plan_id,
            plan_tier_id,
            status,
            customer_id.as_deref(),
        )
        .await?;
        if let Some(customer_id) = customer_id {
            billing_subscriptions::upsert_user_billing_account(db, user_id, &customer_id).await?;
        }
        // Plan B1: payment cleared -> the user's cloud computer, fire-and-
        // forget after the mirror row exists (sizing reads its plan_id and
        // provisioned_instances.subscription_id references it). Idempotent
        // per subscription, so Stripe retries and .updated events are no-ops.
        if let Some(provisioner) = provisioner {
            let handle = provisioner.spawn_ensure_for_subscription(user_id, subscription_id);
            return Ok((applied, Some(handle)));
        }
    }
    Ok((applied, None))
}

/// The Stripe customer id whether the event carries an id string or an expanded customer object.
fn subscription_customer_id(subscription: &Value) -> Option<String> {
    subscription["customer"]
        .as_str()
        .or_else(|| subscription["customer"]["id"].as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn revoke_and_respond(
    state: &ApiState,
    subscription: &Value,
    event_id: &str,
    user_id: &str,
    plan_tier_id: &str,
) -> Response {
    match apply_hosted_entitlement(&state.db, event_id, user_id, plan_tier_id, None, "stripe")
        .await
    {
        Ok(applied) => {
            crate::services::audit::write_audit_log(
                &state.db,
                crate::services::audit::AuditEvent {
                    action: "billing.subscription.revoked".to_string(),
                    resource_type: "billing_entitlement".to_string(),
                    resource_id: Some(event_id.to_string()),
                    user_id: Some(user_id.to_string()),
                    user_email: None,
                    details: Some(json!({
                        "planTierId": plan_tier_id,
                        "idempotentReplay": applied.idempotent_replay,
                    })),
                    success: true,
                },
            )
            .await;
            let subscription_id = subscription["id"].as_str().unwrap_or_default();
            if !subscription_id.is_empty() {
                // A missing local row (deletion delivered before creation) is fine.
                if let Err(error) = billing_subscriptions::mark_billing_subscription_canceled(
                    &state.db,
                    subscription_id,
                )
                .await
                {
                    return error.into_response();
                }
                // Plan B2: stop the computer now, snapshot it (kept 6
                // months), delete it 30 days out. Fire-and-forget; a no-op
                // when the subscription never had a computer.
                state.provisioning_service.spawn_suspend_for_subscription(
                    user_id,
                    subscription_id,
                    crate::services::SuspendReason::Cancelled,
                );
            }
            Json(json!({
                "received": true,
                "eventId": event_id,
                "userId": user_id,
                "planTierId": plan_tier_id,
                "idempotentReplay": applied.idempotent_replay,
            }))
            .into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// A subscription whose renewal failed (see [`MappedStripeEvent::PaymentFailed`]).
async fn payment_failed_and_respond(
    state: &ApiState,
    subscription: &Value,
    event_id: &str,
    user_id: &str,
    status: &str,
) -> Response {
    let subscription_id = subscription["id"].as_str().unwrap_or_default();
    let unpaid = status == "unpaid";
    let mut idempotent_replay = false;
    if unpaid {
        let default_tier =
            std::env::var("DEFAULT_PLAN_TIER").unwrap_or_else(|_| "free".to_string());
        match apply_hosted_entitlement(&state.db, event_id, user_id, &default_tier, None, "stripe")
            .await
        {
            Ok(applied) => idempotent_replay = applied.idempotent_replay,
            Err(error) => return error.into_response(),
        }
    }
    if !subscription_id.is_empty() {
        // A missing local row (event delivered before creation) is fine.
        if let Err(error) = billing_subscriptions::set_billing_subscription_status(
            &state.db,
            subscription_id,
            status,
        )
        .await
        {
            return error.into_response();
        }
        if unpaid {
            state.provisioning_service.spawn_suspend_for_subscription(
                user_id,
                subscription_id,
                crate::services::SuspendReason::Unpaid,
            );
        }
    }
    crate::services::audit::write_audit_log(
        &state.db,
        crate::services::audit::AuditEvent {
            action: if unpaid {
                "billing.subscription.unpaid".to_string()
            } else {
                "billing.subscription.past_due".to_string()
            },
            resource_type: "billing_entitlement".to_string(),
            resource_id: Some(event_id.to_string()),
            user_id: Some(user_id.to_string()),
            user_email: None,
            details: Some(json!({
                "subscriptionId": subscription_id,
                "status": status,
                "idempotentReplay": idempotent_replay,
            })),
            success: true,
        },
    )
    .await;
    Json(json!({
        "received": true,
        "eventId": event_id,
        "userId": user_id,
        "status": status,
        "idempotentReplay": idempotent_replay,
    }))
    .into_response()
}

/// Grant a subscription plan's monthly credits for a paid invoice. The invoice object carries
/// no user/plan metadata (invoice metadata is NOT subscription metadata), so the grant resolves
/// through the billing_subscriptions mirror written by the subscription lifecycle events.
/// Unknown subscriptions are acknowledged and ignored: Stripe also delivers invoices for
/// subscriptions created outside our checkout flow, and 200 stops the retries.
async fn grant_subscription_credits_and_respond(
    state: &ApiState,
    event_id: &str,
    invoice_id: &str,
    subscription_id: &str,
) -> Response {
    let subscription =
        match billing_subscriptions::billing_subscription_for(&state.db, subscription_id).await {
            Ok(subscription) => subscription,
            Err(error) => return error.into_response(),
        };
    let Some(subscription) = subscription else {
        return ignored_response("invoice.paid");
    };
    let Some(plan) = billing_subscriptions::find_plan(&subscription.plan_id) else {
        return ignored_response("invoice.paid");
    };
    let cost_service = crate::services::CostServiceImpl::new(state.db.clone());
    match cost_service
        .grant_subscription_credits(
            &subscription.user_id,
            invoice_id,
            plan.monthly_credits_usd,
            plan.rollover_cap_usd,
        )
        .await
    {
        Ok(balance_usd) => Json(json!({
            "received": true,
            "eventId": event_id,
            "invoiceId": invoice_id,
            "userId": subscription.user_id,
            "creditsUsd": plan.monthly_credits_usd,
            "balanceUsd": balance_usd,
        }))
        .into_response(),
        Err(error) => error.into_response(),
    }
}

/// The outcome of processing one credit-purchase Stripe event.
struct CreditPurchaseOutcome {
    /// Where the grant landed: the allternit-api fabric ledger when the
    /// bridge consumed it, the local cloud wallet otherwise.
    target: String,
    /// True when the event id was already processed and nothing was granted.
    idempotent_replay: bool,
    /// Cloud-wallet balance after the grant (fabric grants have no local
    /// balance to report).
    balance_usd: Option<f64>,
}

/// Which ledger receives a credit-pack grant.
enum CreditGrantTarget<'a> {
    /// Grant into the allternit-api org fabric credits ledger via the
    /// internal bridge (requires the target org from the session metadata).
    Fabric(&'a dyn crate::services::fabric_ledger::FabricLedger, String),
    /// Grant into the local cloud wallet (bridge off or the session predates
    /// org capture) — pre-G15 behavior.
    CloudWallet,
}

/// Grant a one-off credit purchase. Idempotent per Stripe event id at three
/// layers:
///
/// 1. `webhook_events` — a processed event is acknowledged as a replay
///    without touching either ledger (first-line dedupe + audit trail).
/// 2. The grant call itself carries the idempotency key `stripe-{event_id}`:
///    the fabric ledger's `credit_purchase_idempotency` table and the cloud
///    wallet's `credit_transactions.transaction_id` uniqueness make a
///    concurrent or missed dedupe a no-op — this is the money guarantee.
/// 3. `billing_purchase_trust` bookkeeping only counts a fresh grant, so
///    retries cannot inflate the paid-purchase count.
///
/// The `webhook_events` row is written only AFTER the grant succeeds: a
/// failed grant leaves no record, so Stripe's retry re-attempts it (layer 2
/// keeps that safe).
async fn grant_credits_and_respond(
    state: &ApiState,
    event_type: &str,
    event_id: &str,
    user_id: &str,
    organization_id: Option<&str>,
    amount_usd: f64,
) -> Response {
    if crate::services::fabric_ledger::misconfigured() {
        // The deployment pointed the bridge at a fabric ledger but never
        // provisioned the shared token. Granting to the wallet would
        // silently strand the purchase off the org ledger; error instead so
        // Stripe retries and the operator sees the log.
        tracing::error!(
            "REVENUE-CRITICAL: ALLTERNIT_FABRIC_LEDGER_URL is set but ALLTERNIT_INTERNAL_SERVICE_TOKEN is missing; refusing to grant event {event_id}"
        );
        return ApiError::ServiceUnavailable(
            "Fabric ledger bridge is misconfigured (missing internal service token).".to_string(),
        )
        .into_response();
    }

    let bridge = crate::services::fabric_ledger::from_env();
    let target = match (&bridge, organization_id) {
        (Some(ledger), Some(org)) => CreditGrantTarget::Fabric(ledger, org.to_string()),
        _ => CreditGrantTarget::CloudWallet,
    };

    match process_credit_purchase(&state.db, target, event_type, event_id, user_id, amount_usd)
        .await
    {
        Ok(outcome) => {
            if !outcome.idempotent_replay {
                crate::services::audit::write_audit_log(
                    &state.db,
                    crate::services::audit::AuditEvent {
                        action: "billing.credits.granted".to_string(),
                        resource_type: "credit_transaction".to_string(),
                        resource_id: Some(event_id.to_string()),
                        user_id: Some(user_id.to_string()),
                        user_email: None,
                        details: Some(json!({
                            "creditsUsd": amount_usd,
                            "balanceUsd": outcome.balance_usd,
                            "target": outcome.target,
                        })),
                        success: true,
                    },
                )
                .await;
                // Trust bookkeeping must never fail the webhook: the grant
                // already landed and Stripe would retry on a non-2xx.
                if let Err(error) =
                    billing_subscriptions::record_paid_purchase(&state.db, user_id).await
                {
                    tracing::error!(
                        "REVENUE-CRITICAL: failed to record paid purchase for user {} (event {}): {}",
                        user_id,
                        event_id,
                        error
                    );
                }
            }
            let mut response = json!({
                "received": true,
                "eventId": event_id,
                "userId": user_id,
                "creditsUsd": amount_usd,
                "target": outcome.target,
                "idempotentReplay": outcome.idempotent_replay,
            });
            if let Some(balance_usd) = outcome.balance_usd {
                response["balanceUsd"] = json!(balance_usd);
            }
            Json(response).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// The testable core of `grant_credits_and_respond`: dedupe, grant into the
/// selected ledger, then record the processed event. Returns the outcome so
/// the HTTP shell can build the response and audit trail.
async fn process_credit_purchase(
    db: &PgPool,
    target: CreditGrantTarget<'_>,
    event_type: &str,
    event_id: &str,
    user_id: &str,
    amount_usd: f64,
) -> Result<CreditPurchaseOutcome, ApiError> {
    let existing_target: Option<String> = sqlx::query_scalar(
        "SELECT target FROM webhook_events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_optional(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    if let Some(target) = existing_target {
        return Ok(CreditPurchaseOutcome {
            target,
            idempotent_replay: true,
            balance_usd: None,
        });
    }

    let transaction_id = format!("stripe-{event_id}");
    let (target_name, balance_usd) = match target {
        CreditGrantTarget::Fabric(ledger, organization_id) => {
            let amount_cents = (amount_usd * 100.0).round() as i64;
            ledger
                .grant_pack_credits(&organization_id, amount_cents, &transaction_id, event_id)
                .await?;
            ("fabric_ledger".to_string(), None)
        }
        CreditGrantTarget::CloudWallet => {
            let cost_service = crate::services::CostServiceImpl::new(db.clone());
            let balance_usd = cost_service
                .add_credits(user_id, amount_usd, &transaction_id, "stripe")
                .await?;
            ("cloud_wallet".to_string(), Some(balance_usd))
        }
    };

    // Record only after the grant succeeded; ON CONFLICT makes a concurrent
    // delivery a harmless no-op (layer-2 idempotency already protected the
    // money).
    sqlx::query("INSERT INTO webhook_events (id, event_type, target) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
        .bind(event_id)
        .bind(event_type)
        .bind(&target_name)
        .execute(db)
        .await
        .map_err(ApiError::DatabaseError)?;

    Ok(CreditPurchaseOutcome {
        target: target_name,
        idempotent_replay: false,
        balance_usd,
    })
}

/// Verify a `Stripe-Signature` header against the raw body. Any one `v1`
/// value may match (Stripe can send several during secret rotation), the
/// timestamp must be within tolerance, and digest comparison is constant-time
/// (`Mac::verify_slice`).
fn verify_stripe_signature(
    secret: &str,
    header: &str,
    body: &[u8],
    now_unix: i64,
) -> Result<(), ApiError> {
    let invalid = || ApiError::Unauthorized("Invalid Stripe webhook signature.".to_string());
    let mut timestamp: Option<i64> = None;
    let mut signatures: Vec<&str> = Vec::new();
    for part in header.split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        match key.trim() {
            "t" => timestamp = value.trim().parse().ok(),
            "v1" => signatures.push(value.trim()),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or_else(invalid)?;
    if (now_unix - timestamp).abs() > SIGNATURE_TOLERANCE_SECONDS {
        return Err(invalid());
    }
    if signatures.is_empty() {
        return Err(invalid());
    }

    let mut signed_payload = timestamp.to_string().into_bytes();
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(body);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::Internal("Failed to build webhook verifier".to_string()))?;
    mac.update(&signed_payload);
    for signature in signatures {
        let Ok(expected) = hex::decode(signature) else {
            continue;
        };
        if mac.clone().verify_slice(&expected).is_ok() {
            return Ok(());
        }
    }
    Err(invalid())
}

/// Clerk user id from subscription metadata, falling back to an expanded
/// customer object's metadata.
fn subscription_clerk_user_id(subscription: &Value) -> Option<String> {
    subscription["metadata"]["clerk_user_id"]
        .as_str()
        .or_else(|| subscription["customer"]["metadata"]["clerk_user_id"].as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Map a verified Stripe event onto an entitlement change. Missing metadata
/// on a mapped event is a 400 (a permanent misconfiguration Stripe should
/// surface); unmapped event types and non-billable subscription statuses are
/// acknowledged and ignored so Stripe stops retrying them.
fn map_stripe_event(event: &Value) -> Result<MappedStripeEvent, ApiError> {
    let event_type = event["type"].as_str().unwrap_or_default();
    let event_id = event["id"].as_str().unwrap_or_default().to_string();
    let subscription = &event["data"]["object"];
    match event_type {
        "customer.subscription.created" | "customer.subscription.updated" => {
            let status = subscription["status"].as_str().unwrap_or_default();
            if matches!(status, "past_due" | "unpaid") {
                if event_id.is_empty() {
                    return Err(ApiError::BadRequest(
                        "Stripe event is missing its id.".to_string(),
                    ));
                }
                // Subscriptions from outside our checkout carry no user: ack.
                let Some(user_id) = subscription_clerk_user_id(subscription) else {
                    return Ok(MappedStripeEvent::Ignored(event_type.to_string()));
                };
                return Ok(MappedStripeEvent::PaymentFailed {
                    event_id,
                    user_id,
                    status: status.to_string(),
                });
            }
            if !matches!(status, "active" | "trialing") {
                return Ok(MappedStripeEvent::Ignored(event_type.to_string()));
            }
            if event_id.is_empty() {
                return Err(ApiError::BadRequest(
                    "Stripe event is missing its id.".to_string(),
                ));
            }
            let user_id = subscription_clerk_user_id(subscription).ok_or_else(|| {
                ApiError::BadRequest(
                    "Subscription is missing clerk_user_id metadata.".to_string(),
                )
            })?;
            // The current price decides the tier, so a plan switch in the
            // billing portal grants the new plan's quotas.
            let plan_tier_id = crate::routes::billing_subscriptions::current_subscription_plan(
                subscription,
            )
            .map(|plan| plan.plan_tier)
            .or_else(|| subscription["metadata"]["allternit_plan_tier"].as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ApiError::BadRequest(
                        "Subscription is missing allternit_plan_tier metadata.".to_string(),
                    )
                })?
                .to_string();
            Ok(MappedStripeEvent::Grant {
                event_id,
                user_id,
                plan_tier_id,
            })
        }
        "customer.subscription.deleted" => {
            if event_id.is_empty() {
                return Err(ApiError::BadRequest(
                    "Stripe event is missing its id.".to_string(),
                ));
            }
            let user_id = subscription_clerk_user_id(subscription).ok_or_else(|| {
                ApiError::BadRequest(
                    "Subscription is missing clerk_user_id metadata.".to_string(),
                )
            })?;
            Ok(MappedStripeEvent::Revoke { event_id, user_id })
        }
        "checkout.session.completed" | "checkout.session.async_payment_succeeded" | "invoice.paid" => {
            // Only one-off credit purchases map via object metadata; subscription-mode
            // checkouts are handled by the subscription events above, and subscription
            // invoices are handled below.
            let object = &event["data"]["object"];
            let is_checkout_event = event_type != "invoice.paid";
            if is_checkout_event && object["mode"].as_str().unwrap_or_default() != "payment" {
                return Ok(MappedStripeEvent::Ignored(event_type.to_string()));
            }
            // Grant only after the money actually settled. checkout.session.completed
            // can arrive with payment_status "unpaid" for asynchronous payment
            // methods (e.g. bank debits); Stripe then delivers
            // checkout.session.async_payment_succeeded, which is the grant trigger.
            if is_checkout_event
                && object["payment_status"].as_str().unwrap_or_default() != "paid"
            {
                return Ok(MappedStripeEvent::Ignored(event_type.to_string()));
            }
            let metadata = &object["metadata"];
            let user_id = metadata["clerk_user_id"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let credits_raw = metadata["allternit_credits_usd"].as_str();
            let (Some(user_id), Some(credits_raw)) = (user_id, credits_raw) else {
                // Subscription invoices carry NO clerk/plan metadata — invoice metadata
                // is NOT subscription metadata. Monthly credit grants instead key off
                // billing_reason + the subscription id and resolve the user and plan
                // through the billing_subscriptions mirror.
                if event_type == "invoice.paid" {
                    let billing_reason = object["billing_reason"].as_str().unwrap_or_default();
                    let subscription_id = object["subscription"]
                        .as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    if matches!(billing_reason, "subscription_create" | "subscription_cycle") {
                        if let Some(subscription_id) = subscription_id {
                            if event_id.is_empty() {
                                return Err(ApiError::BadRequest(
                                    "Stripe event is missing its id.".to_string(),
                                ));
                            }
                            let invoice_id = object["id"].as_str().unwrap_or_default();
                            if invoice_id.is_empty() {
                                return Err(ApiError::BadRequest(
                                    "Stripe invoice is missing its id.".to_string(),
                                ));
                            }
                            return Ok(MappedStripeEvent::GrantSubscriptionCredits {
                                event_id,
                                invoice_id: invoice_id.to_string(),
                                subscription_id: subscription_id.to_string(),
                            });
                        }
                    }
                }
                return Ok(MappedStripeEvent::Ignored(event_type.to_string()));
            };
            if event_id.is_empty() {
                return Err(ApiError::BadRequest(
                    "Stripe event is missing its id.".to_string(),
                ));
            }
            // Both keys present but unusable is a permanent misconfiguration:
            // 400 so Stripe surfaces it instead of silently dropping money.
            let amount_usd: f64 = credits_raw.trim().parse().map_err(|_| {
                ApiError::BadRequest(format!(
                    "allternit_credits_usd metadata is not a number: {credits_raw:?}."
                ))
            })?;
            if amount_usd <= 0.0 {
                return Err(ApiError::BadRequest(
                    "allternit_credits_usd metadata must be positive.".to_string(),
                ));
            }
            Ok(MappedStripeEvent::GrantCredits {
                event_id,
                user_id: user_id.to_string(),
                amount_usd,
                organization_id: metadata["allternit_org_id"]
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            })
        }
        other => Ok(MappedStripeEvent::Ignored(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    const TEST_SECRET: &str = "whsec_test_secret";

    fn sign(secret: &str, TIMESTAMPTZ: i64, body: &[u8]) -> String {
        let mut signed_payload = TIMESTAMPTZ.to_string().into_bytes();
        signed_payload.push(b'.');
        signed_payload.extend_from_slice(body);
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&signed_payload);
        format!(
            "t={},v1={}",
            TIMESTAMPTZ,
            hex::encode(mac.finalize().into_bytes())
        )
    }

    fn subscription_event(
        event_id: &str,
        event_type: &str,
        status: &str,
        metadata: Value,
    ) -> Value {
        json!({
            "id": event_id,
            "type": event_type,
            "data": {
                "object": {
                    "id": "sub_123",
                    "status": status,
                    "customer": "cus_123",
                    "metadata": metadata,
                }
            }
        })
    }

    /// Minimal billing schema matching migrations 001/012/014/015 for the
    /// entitlement mutation path.
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
        sqlx::query("DROP TABLE IF EXISTS users CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                last_login_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS plan_tiers CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE plan_tiers (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                max_active_devices BIGINT NOT NULL DEFAULT 1,
                max_pairings_per_day BIGINT NOT NULL DEFAULT 5,
                max_relay_sockets BIGINT NOT NULL DEFAULT 5,
                max_relay_mb_per_day BIGINT NOT NULL DEFAULT 100,
                max_hosted_runtime_hours_monthly BIGINT NOT NULL DEFAULT 0,
                can_create_hosted_runtime BOOLEAN NOT NULL DEFAULT FALSE,
                max_hosted_runtimes BIGINT NOT NULL DEFAULT 0,
                max_hosted_runtime_memory_mb BIGINT NOT NULL DEFAULT 0,
                hard_spend_cap_usd DOUBLE PRECISION
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            INSERT INTO plan_tiers (
                id, display_name, max_active_devices, max_pairings_per_day,
                max_relay_sockets, max_relay_mb_per_day,
                max_hosted_runtime_hours_monthly, can_create_hosted_runtime,
                max_hosted_runtimes, max_hosted_runtime_memory_mb, hard_spend_cap_usd
            ) VALUES
                ('free', 'Free', 1, 5, 5, 100, 0, FALSE, 0, 0, 5.0),
                ('pro', 'Pro', 5, 50, 20, 5000, 100, TRUE, 1, 1024, 100.0)
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS user_runtime_quotas CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE user_runtime_quotas (
                user_id TEXT PRIMARY KEY,
                plan_tier_id TEXT NOT NULL,
                max_active_devices BIGINT NOT NULL,
                max_pairings_per_day BIGINT NOT NULL,
                max_relay_sockets BIGINT NOT NULL,
                max_relay_mb_per_day BIGINT NOT NULL,
                max_hosted_runtime_hours_monthly BIGINT NOT NULL,
                can_create_hosted_runtime BOOLEAN NOT NULL,
                max_hosted_runtimes BIGINT NOT NULL,
                max_hosted_runtime_memory_mb BIGINT NOT NULL,
                hard_spend_cap_usd DOUBLE PRECISION
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS billing_entitlement_events CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE billing_entitlement_events (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                previous_plan_tier_id TEXT,
                plan_tier_id TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'billing',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Credits schema (migration 024) for the one-off credit purchase path.
        sqlx::query("DROP TABLE IF EXISTS user_credits CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE user_credits (
                user_id TEXT PRIMARY KEY,
                balance_usd DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (balance_usd >= 0),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS credit_transactions CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE credit_transactions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                amount_usd DOUBLE PRECISION NOT NULL,
                transaction_id TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Subscription mirror schema (migrations_pg 005) for the monthly grant path.
        sqlx::query("DROP TABLE IF EXISTS billing_subscriptions CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE billing_subscriptions (
                stripe_subscription_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                plan_id TEXT NOT NULL,
                plan_tier TEXT NOT NULL,
                status TEXT NOT NULL,
                stripe_customer_id TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS user_billing_accounts CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
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
        sqlx::query("DROP TABLE IF EXISTS billing_purchase_trust CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
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
        // Processed-Stripe-event dedupe (migration 015) for the credit
        // purchase grant path.
        sqlx::query("DROP TABLE IF EXISTS webhook_events CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE webhook_events (
                id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                target TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn quota_tier(pool: &PgPool, user_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT plan_tier_id FROM user_runtime_quotas WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[test]
    fn signature_verifies_a_correctly_signed_payload() {
        let body = br#"{"id":"evt_1","type":"customer.subscription.created"}"#;
        let now = 1_800_000_000_i64;
        let header = sign(TEST_SECRET, now, body);
        assert!(verify_stripe_signature(TEST_SECRET, &header, body, now).is_ok());
    }

    #[test]
    fn signature_rejects_tampered_body_wrong_secret_and_stale_timestamp() {
        let body = br#"{"id":"evt_1"}"#;
        let now = 1_800_000_000_i64;
        let header = sign(TEST_SECRET, now, body);

        let tampered = br#"{"id":"evt_2"}"#;
        assert!(verify_stripe_signature(TEST_SECRET, &header, tampered, now).is_err());
        assert!(verify_stripe_signature("whsec_other", &header, body, now).is_err());
        assert!(
            verify_stripe_signature(
                TEST_SECRET,
                &header,
                body,
                now + SIGNATURE_TOLERANCE_SECONDS + 1,
            )
            .is_err(),
            "TIMESTAMPTZ outside the 5 minute tolerance must fail"
        );
        assert!(verify_stripe_signature(TEST_SECRET, "", body, now).is_err());
        assert!(verify_stripe_signature(TEST_SECRET, "t=notanumber,v1=aa", body, now).is_err());
    }

    #[test]
    fn signature_accepts_one_matching_v1_among_rotation_candidates() {
        let body = br#"{"id":"evt_1"}"#;
        let now = 1_800_000_000_i64;
        let good = sign(TEST_SECRET, now, body);
        let header = format!("t={now},v1=deadbeef,{}", good.split(',').nth(1).unwrap());
        assert!(verify_stripe_signature(TEST_SECRET, &header, body, now).is_ok());
    }

    #[test]
    fn mapping_grants_active_and_trialing_subscriptions() {
        for (event_type, status) in [
            ("customer.subscription.created", "active"),
            ("customer.subscription.updated", "trialing"),
        ] {
            let event = subscription_event(
                "evt_1",
                event_type,
                status,
                json!({ "clerk_user_id": "user_1", "allternit_plan_tier": "pro" }),
            );
            assert_eq!(
                map_stripe_event(&event).unwrap(),
                MappedStripeEvent::Grant {
                    event_id: "evt_1".to_string(),
                    user_id: "user_1".to_string(),
                    plan_tier_id: "pro".to_string(),
                },
                "{event_type}/{status} must grant"
            );
        }
    }

    #[test]
    fn mapping_revokes_deleted_subscriptions() {
        let event = subscription_event(
            "evt_9",
            "customer.subscription.deleted",
            "canceled",
            json!({ "clerk_user_id": "user_1" }),
        );
        assert_eq!(
            map_stripe_event(&event).unwrap(),
            MappedStripeEvent::Revoke {
                event_id: "evt_9".to_string(),
                user_id: "user_1".to_string(),
            }
        );
    }

    #[test]
    fn mapping_ignores_unmapped_types_and_non_billable_statuses() {
        let unknown = json!({ "id": "evt_2", "type": "invoice.paid", "data": { "object": {} } });
        assert_eq!(
            map_stripe_event(&unknown).unwrap(),
            MappedStripeEvent::Ignored("invoice.paid".to_string())
        );

        let incomplete = subscription_event(
            "evt_3",
            "customer.subscription.updated",
            "incomplete",
            json!({ "clerk_user_id": "user_1", "allternit_plan_tier": "pro" }),
        );
        assert_eq!(
            map_stripe_event(&incomplete).unwrap(),
            MappedStripeEvent::Ignored("customer.subscription.updated".to_string())
        );
    }

    #[test]
    fn mapping_turns_failed_renewals_into_payment_failed() {
        for status in ["past_due", "unpaid"] {
            let event = subscription_event(
                "evt_pf",
                "customer.subscription.updated",
                status,
                json!({ "clerk_user_id": "user_1", "allternit_plan_tier": "pro" }),
            );
            assert_eq!(
                map_stripe_event(&event).unwrap(),
                MappedStripeEvent::PaymentFailed {
                    event_id: "evt_pf".to_string(),
                    user_id: "user_1".to_string(),
                    status: status.to_string(),
                }
            );
        }
        // A subscription from outside our checkout has no user: acknowledged.
        let foreign = subscription_event("evt_pf2", "customer.subscription.updated", "unpaid", json!({}));
        assert_eq!(
            map_stripe_event(&foreign).unwrap(),
            MappedStripeEvent::Ignored("customer.subscription.updated".to_string())
        );
    }

    #[test]
    fn mapping_falls_back_to_expanded_customer_metadata() {
        let mut event = subscription_event("evt_4", "customer.subscription.created", "active", json!({}));
        event["data"]["object"]["customer"] = json!({
            "id": "cus_123",
            "metadata": { "clerk_user_id": "user_7" }
        });
        event["data"]["object"]["metadata"] = json!({ "allternit_plan_tier": "pro" });
        assert_eq!(
            map_stripe_event(&event).unwrap(),
            MappedStripeEvent::Grant {
                event_id: "evt_4".to_string(),
                user_id: "user_7".to_string(),
                plan_tier_id: "pro".to_string(),
            }
        );
    }

    #[test]
    fn mapping_rejects_mapped_events_without_required_metadata() {
        let no_user = subscription_event(
            "evt_5",
            "customer.subscription.created",
            "active",
            json!({ "allternit_plan_tier": "pro" }),
        );
        assert!(map_stripe_event(&no_user).is_err());

        let no_tier = subscription_event(
            "evt_6",
            "customer.subscription.created",
            "active",
            json!({ "clerk_user_id": "user_1" }),
        );
        assert!(map_stripe_event(&no_tier).is_err());

        let deleted_no_user = subscription_event(
            "evt_7",
            "customer.subscription.deleted",
            "canceled",
            json!({}),
        );
        assert!(map_stripe_event(&deleted_no_user).is_err());
    }

    #[tokio::test]
    async fn grant_then_revoke_moves_quota_and_replays_are_noops() {
        let pool = test_pool().await;

        // Active subscription grants the metadata tier.
        let grant = apply_hosted_entitlement(&pool, "evt_1", "user_1", "pro", None, "stripe")
            .await
            .unwrap();
        assert!(!grant.idempotent_replay);
        assert_eq!(grant.previous_plan_tier_id, None);
        assert_eq!(quota_tier(&pool, "user_1").await.as_deref(), Some("pro"));

        // Stripe retries the same delivery: same event id, no second effect.
        let replay = apply_hosted_entitlement(&pool, "evt_1", "user_1", "pro", None, "stripe")
            .await
            .unwrap();
        assert!(replay.idempotent_replay);
        let event_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM billing_entitlement_events")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(event_count, 1, "the event id dedupes Stripe retries");

        // Reusing the event id for a different change is rejected.
        assert!(
            apply_hosted_entitlement(&pool, "evt_1", "user_1", "free", None, "stripe")
                .await
                .is_err()
        );

        // Deletion revokes back to the default tier, recording the previous one.
        let revoke = apply_hosted_entitlement(&pool, "evt_2", "user_1", "free", None, "stripe")
            .await
            .unwrap();
        assert!(!revoke.idempotent_replay);
        assert_eq!(revoke.previous_plan_tier_id.as_deref(), Some("pro"));
        assert_eq!(quota_tier(&pool, "user_1").await.as_deref(), Some("free"));

        let (can_create, max_runtimes): (bool, i64) = sqlx::query_as(
            "SELECT can_create_hosted_runtime, max_hosted_runtimes FROM user_runtime_quotas WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!can_create);
        assert_eq!(max_runtimes, 0, "revocation restores the free tier limits");
    }

    fn payment_event(event_id: &str, event_type: &str, mode: &str, metadata: Value) -> Value {
        json!({
            "id": event_id,
            "type": event_type,
            "data": {
                "object": {
                    "id": "cs_test_123",
                    "mode": mode,
                    "payment_status": "paid",
                    "metadata": metadata,
                }
            }
        })
    }

    #[test]
    fn mapping_grants_credit_purchases_with_full_metadata() {
        let checkout = payment_event(
            "evt_c1",
            "checkout.session.completed",
            "payment",
            json!({ "clerk_user_id": "user_1", "allternit_credits_usd": "25.00" }),
        );
        assert_eq!(
            map_stripe_event(&checkout).unwrap(),
            MappedStripeEvent::GrantCredits {
                event_id: "evt_c1".to_string(),
                user_id: "user_1".to_string(),
                amount_usd: 25.0,
                organization_id: None,
            }
        );

        let invoice = payment_event(
            "evt_c2",
            "invoice.paid",
            "",
            json!({ "clerk_user_id": "user_2", "allternit_credits_usd": "10.5" }),
        );
        assert_eq!(
            map_stripe_event(&invoice).unwrap(),
            MappedStripeEvent::GrantCredits {
                event_id: "evt_c2".to_string(),
                user_id: "user_2".to_string(),
                amount_usd: 10.5,
                organization_id: None,
            }
        );
    }

    #[test]
    fn mapping_carries_the_org_for_fabric_routing() {
        let checkout = payment_event(
            "evt_c1o",
            "checkout.session.completed",
            "payment",
            json!({
                "clerk_user_id": "user_1",
                "allternit_credits_usd": "25.00",
                "allternit_org_id": "org_42"
            }),
        );
        assert_eq!(
            map_stripe_event(&checkout).unwrap(),
            MappedStripeEvent::GrantCredits {
                event_id: "evt_c1o".to_string(),
                user_id: "user_1".to_string(),
                amount_usd: 25.0,
                organization_id: Some("org_42".to_string()),
            }
        );
    }

    #[test]
    fn mapping_grants_only_after_the_payment_settles() {
        // checkout.session.completed with payment_status "unpaid" (async
        // payment methods): no grant yet — Stripe delivers
        // checkout.session.async_payment_succeeded when money settles.
        let mut unpaid = payment_event(
            "evt_u1",
            "checkout.session.completed",
            "payment",
            json!({ "clerk_user_id": "user_1", "allternit_credits_usd": "25.00" }),
        );
        unpaid["data"]["object"]["payment_status"] = json!("unpaid");
        assert_eq!(
            map_stripe_event(&unpaid).unwrap(),
            MappedStripeEvent::Ignored("checkout.session.completed".to_string())
        );

        // The settlement event for the same session IS the grant trigger.
        let settled = payment_event(
            "evt_u2",
            "checkout.session.async_payment_succeeded",
            "payment",
            json!({ "clerk_user_id": "user_1", "allternit_credits_usd": "25.00" }),
        );
        assert_eq!(
            map_stripe_event(&settled).unwrap(),
            MappedStripeEvent::GrantCredits {
                event_id: "evt_u2".to_string(),
                user_id: "user_1".to_string(),
                amount_usd: 25.0,
                organization_id: None,
            }
        );

        // A settlement event that somehow is not paid is still not granted.
        let mut unsettled = settled.clone();
        unsettled["id"] = json!("evt_u3");
        unsettled["data"]["object"]["payment_status"] = json!("unpaid");
        assert_eq!(
            map_stripe_event(&unsettled).unwrap(),
            MappedStripeEvent::Ignored("checkout.session.async_payment_succeeded".to_string())
        );
    }

    #[test]
    fn mapping_ignores_subscription_mode_async_settlement() {
        let subscription_mode = payment_event(
            "evt_a5",
            "checkout.session.async_payment_succeeded",
            "subscription",
            json!({ "clerk_user_id": "user_1", "allternit_credits_usd": "25" }),
        );
        assert_eq!(
            map_stripe_event(&subscription_mode).unwrap(),
            MappedStripeEvent::Ignored("checkout.session.async_payment_succeeded".to_string())
        );
    }

    #[test]
    fn mapping_ignores_credit_event_types_without_full_metadata() {
        // Subscription-invoice traffic has no credit metadata: stays ignored
        // so the subscription flow is untouched.
        let no_metadata = payment_event("evt_c3", "invoice.paid", "", json!({}));
        assert_eq!(
            map_stripe_event(&no_metadata).unwrap(),
            MappedStripeEvent::Ignored("invoice.paid".to_string())
        );

        let partial = payment_event(
            "evt_c4",
            "checkout.session.completed",
            "payment",
            json!({ "clerk_user_id": "user_1" }),
        );
        assert_eq!(
            map_stripe_event(&partial).unwrap(),
            MappedStripeEvent::Ignored("checkout.session.completed".to_string())
        );

        // Subscription-mode checkouts are handled by the subscription events
        // even if they happen to carry credit-shaped metadata.
        let subscription_mode = payment_event(
            "evt_c5",
            "checkout.session.completed",
            "subscription",
            json!({ "clerk_user_id": "user_1", "allternit_credits_usd": "25" }),
        );
        assert_eq!(
            map_stripe_event(&subscription_mode).unwrap(),
            MappedStripeEvent::Ignored("checkout.session.completed".to_string())
        );
    }

    #[test]
    fn mapping_rejects_unusable_credit_amounts() {
        for bad in ["0", "-5", "not-a-number"] {
            let event = payment_event(
                "evt_c6",
                "checkout.session.completed",
                "payment",
                json!({ "clerk_user_id": "user_1", "allternit_credits_usd": bad }),
            );
            assert!(
                map_stripe_event(&event).is_err(),
                "allternit_credits_usd {bad:?} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn credit_purchase_grants_once_and_replays_are_noops() {
        let pool = test_pool().await;
        let cost_service = crate::services::CostServiceImpl::new(pool.clone());

        // The handler path: map the event, then grant with the event id as
        // the idempotency key, exactly as grant_credits_and_respond does.
        let event = payment_event(
            "evt_c7",
            "checkout.session.completed",
            "payment",
            json!({ "clerk_user_id": "user_1", "allternit_credits_usd": "25" }),
        );
        let MappedStripeEvent::GrantCredits {
            event_id,
            user_id,
            amount_usd,
            ..
        } = map_stripe_event(&event).unwrap()
        else {
            panic!("credit purchase must map to a grant");
        };
        let balance = cost_service
            .add_credits(&user_id, amount_usd, &format!("stripe-{event_id}"), "stripe")
            .await
            .unwrap();
        assert!((balance - 25.0).abs() < 1e-9);

        // Stripe retries the same delivery: the transaction id dedupes.
        let balance = cost_service
            .add_credits(&user_id, amount_usd, &format!("stripe-{event_id}"), "stripe")
            .await
            .unwrap();
        assert!((balance - 25.0).abs() < 1e-9, "replay must not double-grant");
        let grants: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM credit_transactions WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(grants, 1);
    }

    fn subscription_invoice_event(
        event_id: &str,
        invoice_id: &str,
        subscription_id: &str,
        billing_reason: &str,
    ) -> Value {
        json!({
            "id": event_id,
            "type": "invoice.paid",
            "data": {
                "object": {
                    "id": invoice_id,
                    "billing_reason": billing_reason,
                    "subscription": subscription_id,
                    "metadata": {},
                }
            }
        })
    }

    #[test]
    fn mapping_routes_subscription_invoices_to_monthly_grants() {
        for reason in ["subscription_create", "subscription_cycle"] {
            let event = subscription_invoice_event("evt_i1", "in_1", "sub_1", reason);
            assert_eq!(
                map_stripe_event(&event).unwrap(),
                MappedStripeEvent::GrantSubscriptionCredits {
                    event_id: "evt_i1".to_string(),
                    invoice_id: "in_1".to_string(),
                    subscription_id: "sub_1".to_string(),
                },
                "billing_reason {reason} must map to a monthly credit grant"
            );
        }
    }

    #[test]
    fn mapping_ignores_non_subscription_invoices_and_missing_subscription_ids() {
        let manual = subscription_invoice_event("evt_i2", "in_2", "sub_1", "manual");
        assert_eq!(
            map_stripe_event(&manual).unwrap(),
            MappedStripeEvent::Ignored("invoice.paid".to_string())
        );

        let no_subscription = subscription_invoice_event("evt_i3", "in_3", "", "subscription_cycle");
        assert_eq!(
            map_stripe_event(&no_subscription).unwrap(),
            MappedStripeEvent::Ignored("invoice.paid".to_string())
        );
    }

    #[test]
    fn one_off_credit_metadata_still_wins_over_the_invoice_path() {
        // An invoice carrying the one-off credit contract stays on the GrantCredits
        // path even if it happens to reference a subscription.
        let mut event = payment_event(
            "evt_i4",
            "invoice.paid",
            "",
            json!({ "clerk_user_id": "user_1", "allternit_credits_usd": "25.00" }),
        );
        event["data"]["object"]["subscription"] = json!("sub_1");
        event["data"]["object"]["billing_reason"] = json!("subscription_cycle");
        assert_eq!(
            map_stripe_event(&event).unwrap(),
            MappedStripeEvent::GrantCredits {
                event_id: "evt_i4".to_string(),
                user_id: "user_1".to_string(),
                amount_usd: 25.0,
                organization_id: None,
            }
        );
    }

    #[derive(Default)]
    struct RecordingFabric {
        calls: std::sync::Mutex<Vec<(String, i64, String, String)>>,
    }

    #[async_trait::async_trait]
    impl crate::services::fabric_ledger::FabricLedger for RecordingFabric {
        async fn grant_pack_credits(
            &self,
            organization_id: &str,
            amount_cents: i64,
            idempotency_key: &str,
            reference_id: &str,
        ) -> Result<(), ApiError> {
            self.calls.lock().unwrap().push((
                organization_id.to_string(),
                amount_cents,
                idempotency_key.to_string(),
                reference_id.to_string(),
            ));
            Ok(())
        }
    }

    struct FailingFabric;

    #[async_trait::async_trait]
    impl crate::services::fabric_ledger::FabricLedger for FailingFabric {
        async fn grant_pack_credits(
            &self,
            _organization_id: &str,
            _amount_cents: i64,
            _idempotency_key: &str,
            _reference_id: &str,
        ) -> Result<(), ApiError> {
            Err(ApiError::Internal("fabric ledger unreachable".to_string()))
        }
    }

    #[tokio::test]
    async fn fabric_grant_happens_once_and_replays_are_deduped() {
        let pool = test_pool().await;
        let fabric = RecordingFabric::default();

        let outcome = process_credit_purchase(
            &pool,
            CreditGrantTarget::Fabric(&fabric, "org_42".to_string()),
            "checkout.session.completed",
            "evt_f1",
            "user_1",
            25.0,
        )
        .await
        .unwrap();
        assert_eq!(outcome.target, "fabric_ledger");
        assert!(!outcome.idempotent_replay);
        assert_eq!(outcome.balance_usd, None);

        // The grant call carries the org, integer cents, the Stripe-event-id
        // idempotency key, and the event id as the ledger reference.
        let calls = fabric.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0],
            ("org_42".to_string(), 2500, "stripe-evt_f1".to_string(), "evt_f1".to_string())
        );

        // Stripe retries the same delivery: acknowledged as a replay without
        // a second grant call.
        let replay = process_credit_purchase(
            &pool,
            CreditGrantTarget::Fabric(&fabric, "org_42".to_string()),
            "checkout.session.completed",
            "evt_f1",
            "user_1",
            25.0,
        )
        .await
        .unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(replay.target, "fabric_ledger");
        assert_eq!(fabric.calls.lock().unwrap().len(), 1, "replay must not re-grant");

        // The processed event is recorded with its target.
        let recorded: (String, String) =
            sqlx::query_as("SELECT event_type, target FROM webhook_events WHERE id = 'evt_f1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(recorded, ("checkout.session.completed".to_string(), "fabric_ledger".to_string()));
    }

    #[tokio::test]
    async fn wallet_fallback_grants_and_records_when_no_org_is_present() {
        let pool = test_pool().await;
        let fabric = RecordingFabric::default();

        // Bridge available but the session predates org capture: the grant
        // falls back to the cloud wallet and is still deduped by event id.
        let outcome = process_credit_purchase(
            &pool,
            CreditGrantTarget::CloudWallet,
            "checkout.session.completed",
            "evt_w1",
            "user_1",
            25.0,
        )
        .await
        .unwrap();
        assert_eq!(outcome.target, "cloud_wallet");
        assert!(!outcome.idempotent_replay);
        assert!((outcome.balance_usd.unwrap() - 25.0).abs() < 1e-9);
        assert_eq!(fabric.calls.lock().unwrap().len(), 0);

        let grants: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM credit_transactions WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(grants, 1);

        let recorded: String =
            sqlx::query_scalar("SELECT target FROM webhook_events WHERE id = 'evt_w1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(recorded, "cloud_wallet");

        // Later, enabling the bridge must NOT re-deliver this event.
        let replay = process_credit_purchase(
            &pool,
            CreditGrantTarget::Fabric(&fabric, "org_42".to_string()),
            "checkout.session.completed",
            "evt_w1",
            "user_1",
            25.0,
        )
        .await
        .unwrap();
        assert!(replay.idempotent_replay);
        assert_eq!(fabric.calls.lock().unwrap().len(), 0, "an event that already granted must not grant again after the bridge is enabled");
    }

    #[tokio::test]
    async fn failed_fabric_grant_records_nothing_and_the_retry_succeeds_once() {
        let pool = test_pool().await;

        // First delivery: the fabric ledger is down — the event must not be
        // recorded, so Stripe's retry re-attempts the grant.
        let failed = process_credit_purchase(
            &pool,
            CreditGrantTarget::Fabric(&FailingFabric, "org_42".to_string()),
            "checkout.session.completed",
            "evt_x1",
            "user_1",
            25.0,
        )
        .await;
        assert!(failed.is_err());
        let recorded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM webhook_events WHERE id = 'evt_x1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(recorded, 0, "a failed grant records nothing");

        // Retry with the ledger healthy: exactly one grant.
        let fabric = RecordingFabric::default();
        let outcome = process_credit_purchase(
            &pool,
            CreditGrantTarget::Fabric(&fabric, "org_42".to_string()),
            "checkout.session.completed",
            "evt_x1",
            "user_1",
            25.0,
        )
        .await
        .unwrap();
        assert!(!outcome.idempotent_replay);
        assert_eq!(fabric.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn subscription_created_grants_tier_and_syncs_mirror_tables() {
        let pool = test_pool().await;
        let event = subscription_event(
            "evt_s1",
            "customer.subscription.created",
            "active",
            json!({
                "clerk_user_id": "user_1",
                "allternit_plan_tier": "pro",
                "allternit_plan_id": "plus",
            }),
        );
        let MappedStripeEvent::Grant {
            event_id,
            user_id,
            plan_tier_id,
        } = map_stripe_event(&event).unwrap()
        else {
            panic!("subscription created must map to a grant");
        };
        apply_entitlement_and_sync_subscription(
            &pool,
            &event["data"]["object"],
            &event_id,
            &user_id,
            &plan_tier_id,
            None,
        )
        .await
        .unwrap();

        // Existing behavior: the plan tier is granted.
        assert_eq!(quota_tier(&pool, "user_1").await.as_deref(), Some("pro"));

        // New behavior: the subscription mirror and customer link are written.
        let mirror = billing_subscriptions::billing_subscription_for(&pool, "sub_123")
            .await
            .unwrap()
            .expect("subscription created must write the mirror row");
        assert_eq!(mirror.user_id, "user_1");
        assert_eq!(mirror.plan_id, "plus");
        assert_eq!(mirror.plan_tier, "pro");
        assert_eq!(mirror.status, "active");
        assert_eq!(mirror.stripe_customer_id.as_deref(), Some("cus_123"));
        let customer: String = sqlx::query_scalar(
            "SELECT stripe_customer_id FROM user_billing_accounts WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(customer, "cus_123");

        // Deletion flips the mirror to canceled (and revocation still works via the
        // existing apply_hosted_entitlement path).
        billing_subscriptions::mark_billing_subscription_canceled(&pool, "sub_123")
            .await
            .unwrap();
        let mirror = billing_subscriptions::billing_subscription_for(&pool, "sub_123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mirror.status, "canceled");
    }

    #[tokio::test]
    async fn paid_subscription_provisions_one_computer_and_cancel_suspends_it() {
        use crate::services::provisioning::pg_tests::{add_provisioning_schema, service, MockBackend};
        let pool = test_pool().await;
        sqlx::query("INSERT INTO users (id, email) VALUES ('user_1', 'user_1@example.com')")
            .execute(&pool)
            .await
            .unwrap();
        add_provisioning_schema(&pool).await;
        let backend = Arc::new(MockBackend::default());
        let provisioner = Arc::new(service(pool.clone(), backend.clone()));
        let metadata = json!({
            "clerk_user_id": "user_1",
            "allternit_plan_tier": "pro",
            "allternit_plan_id": "plus",
        });

        // created, a Stripe retry of it, then .updated (trialing -> active):
        // every grant fires the trigger, exactly one computer results.
        for (event_id, event_type, status) in [
            ("evt_c1", "customer.subscription.created", "trialing"),
            ("evt_c1", "customer.subscription.created", "trialing"),
            ("evt_c2", "customer.subscription.updated", "active"),
        ] {
            let event = subscription_event(event_id, event_type, status, metadata.clone());
            let MappedStripeEvent::Grant { event_id, user_id, plan_tier_id } =
                map_stripe_event(&event).unwrap()
            else {
                panic!("{event_type}/{status} must grant");
            };
            let (_, handle) = apply_entitlement_and_sync_subscription(
                &pool,
                &event["data"]["object"],
                &event_id,
                &user_id,
                &plan_tier_id,
                Some(&provisioner),
            )
            .await
            .unwrap();
            handle.expect("the grant fires the provisioning trigger").await.unwrap();
        }
        let rows: Vec<(String, String, i32, i64, i64)> = sqlx::query_as(
            "SELECT id, status, cpu_cores, memory_mb, disk_gb FROM provisioned_instances WHERE subscription_id = 'sub_123'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 1, "one computer per subscription despite retries");
        assert_eq!((rows[0].2, rows[0].3, rows[0].4), (2, 4096, 20), "sized from the Plus plan");
        assert_eq!(backend.created.lock().unwrap().len(), 1);

        // Without a provisioner (ALLTERNIT_PROVISION_ON_PAYMENT unset) the
        // grant is unchanged and nothing is spawned.
        let event = subscription_event("evt_c3", "customer.subscription.updated", "active", metadata.clone());
        let (_, handle) = apply_entitlement_and_sync_subscription(
            &pool, &event["data"]["object"], "evt_c3", "user_1", "pro", None,
        )
        .await
        .unwrap();
        assert!(handle.is_none());

        // customer.subscription.deleted maps to Revoke; revoke_and_respond
        // then fires exactly this suspend.
        let deleted = subscription_event("evt_d1", "customer.subscription.deleted", "canceled", metadata);
        let MappedStripeEvent::Revoke { user_id, .. } = map_stripe_event(&deleted).unwrap() else {
            panic!("deletion must revoke");
        };
        provisioner
            .spawn_suspend_for_subscription(&user_id, "sub_123", crate::services::SuspendReason::Cancelled)
            .await
            .unwrap();
        let (status, delete_after, snapshot): (String, Option<chrono::DateTime<chrono::Utc>>, Option<String>) =
            sqlx::query_as(
                "SELECT status, delete_after, snapshot_image FROM provisioned_instances WHERE id = $1",
            )
            .bind(&rows[0].0)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "suspended");
        assert!(delete_after.is_some());
        assert!(snapshot.is_some(), "cancel snapshot published");
        assert!(backend.calls.lock().unwrap().iter().any(|call| call.starts_with("stop:")));
    }

    #[tokio::test]
    async fn subscription_invoice_grants_monthly_credits_with_rollover() {
        let pool = test_pool().await;
        let cost_service = crate::services::CostServiceImpl::new(pool.clone());

        // Mirror rows as customer.subscription.created would have written them.
        for user in ["user_1", "user_2", "user_3"] {
            billing_subscriptions::upsert_billing_subscription(
                &pool,
                &format!("sub_{user}"),
                user,
                "plus",
                "pro",
                "active",
                Some("cus_x"),
            )
            .await
            .unwrap();
        }
        // user_1 rolls 8 (under the $10 cap), user_2 rolls 15 (over the cap),
        // user_3 has no credits row yet.
        sqlx::query(
            "INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 8.0), ('user_2', 15.0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Full handler path for user_1: map the invoice event, resolve the mirror
        // row and plan, grant. 8 rolled (cap 10) + 22 grant = 30.
        let event = subscription_invoice_event("evt_i1", "in_1", "sub_user_1", "subscription_cycle");
        let MappedStripeEvent::GrantSubscriptionCredits {
            invoice_id,
            subscription_id,
            ..
        } = map_stripe_event(&event).unwrap()
        else {
            panic!("subscription invoice must map to a monthly grant");
        };
        let subscription = billing_subscriptions::billing_subscription_for(&pool, &subscription_id)
            .await
            .unwrap()
            .unwrap();
        let plan = billing_subscriptions::find_plan(&subscription.plan_id).unwrap();
        let balance = cost_service
            .grant_subscription_credits(
                &subscription.user_id,
                &invoice_id,
                plan.monthly_credits_usd,
                plan.rollover_cap_usd,
            )
            .await
            .unwrap();
        assert!((balance - 30.0).abs() < 1e-9, "8 rolled + 22 grant must be 30");

        // user_2: 15 rolled clips to the 10 cap, then + 22 = 32.
        let balance = cost_service
            .grant_subscription_credits("user_2", "in_2", 22.0, 10.0)
            .await
            .unwrap();
        assert!((balance - 32.0).abs() < 1e-9, "15 clipped to cap 10 + 22 must be 32");

        // user_3: no credits row starts fresh with the grant itself.
        let balance = cost_service
            .grant_subscription_credits("user_3", "in_3", 22.0, 10.0)
            .await
            .unwrap();
        assert!((balance - 22.0).abs() < 1e-9, "a fresh row is the grant itself");

        let sources: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT source FROM credit_transactions WHERE transaction_id LIKE 'stripe-invoice-%'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(sources, vec!["stripe_subscription".to_string()]);
    }

    #[tokio::test]
    async fn subscription_invoice_replay_does_not_double_grant() {
        let pool = test_pool().await;
        let cost_service = crate::services::CostServiceImpl::new(pool.clone());

        let balance = cost_service
            .grant_subscription_credits("user_1", "in_1", 22.0, 10.0)
            .await
            .unwrap();
        assert!((balance - 22.0).abs() < 1e-9);

        // Stripe retries the same delivery: the invoice-keyed ledger row dedupes.
        let balance = cost_service
            .grant_subscription_credits("user_1", "in_1", 22.0, 10.0)
            .await
            .unwrap();
        assert!((balance - 22.0).abs() < 1e-9, "replay must not double-grant");
        let grants: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM credit_transactions WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(grants, 1);
    }

    #[tokio::test]
    async fn subscription_invoice_with_unknown_subscription_is_ignored() {
        let pool = test_pool().await;
        // The event maps (billing_reason + subscription id are present), but the
        // mirror lookup finds nothing — a subscription from outside our flow.
        let event = subscription_invoice_event("evt_i9", "in_9", "sub_foreign", "subscription_create");
        let MappedStripeEvent::GrantSubscriptionCredits {
            subscription_id, ..
        } = map_stripe_event(&event).unwrap()
        else {
            panic!("subscription invoice must map to a monthly grant");
        };
        assert!(
            billing_subscriptions::billing_subscription_for(&pool, &subscription_id)
                .await
                .unwrap()
                .is_none(),
            "the handler acknowledges and ignores unknown subscriptions (200, nothing written)"
        );
        let grants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credit_transactions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(grants, 0, "an unknown subscription grants nothing");
    }

    #[tokio::test]
    async fn credit_purchase_records_trust_and_replay_does_not_inflate_it() {
        let pool = test_pool().await;

        // The handler path from grant_credits_and_respond (wallet target):
        // process the event, then trust bookkeeping only for a fresh grant.
        async fn grant_once(pool: &PgPool, event_id: &str) -> bool {
            let outcome = process_credit_purchase(
                pool,
                CreditGrantTarget::CloudWallet,
                "checkout.session.completed",
                event_id,
                "user_1",
                25.0,
            )
            .await
            .unwrap();
            if !outcome.idempotent_replay {
                billing_subscriptions::record_paid_purchase(pool, "user_1").await.unwrap();
            }
            !outcome.idempotent_replay
        }

        assert!(grant_once(&pool, "evt_p1").await, "first delivery is a fresh grant");
        let count: i32 = sqlx::query_scalar(
            "SELECT paid_purchase_count FROM billing_purchase_trust WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);

        // Stripe retries the same delivery: no second grant, no inflated count.
        assert!(!grant_once(&pool, "evt_p1").await, "replay is not fresh");
        let count: i32 = sqlx::query_scalar(
            "SELECT paid_purchase_count FROM billing_purchase_trust WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "replays must not inflate the paid purchase count");
        let balance: f64 =
            sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = 'user_1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!((balance - 25.0).abs() < 1e-9);

        // A different purchase increments.
        assert!(grant_once(&pool, "evt_p2").await);
        let count: i32 = sqlx::query_scalar(
            "SELECT paid_purchase_count FROM billing_purchase_trust WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 2);
    }
}
