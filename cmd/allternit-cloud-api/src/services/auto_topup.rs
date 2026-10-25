//! Automatic top-up of the cloud wallet.
//!
//! A person turns it on in Settings → Billing (`routes::billing_auto_topup`)
//! with a threshold (at least $5), an amount (one of the credit pack sizes)
//! and a monthly cap (default $200). After every wallet debit
//! (`CostService::deduct_credits_for_usage`, which inference settlement and
//! hosted-runtime session billing both go through) [`spawn_check`] looks at
//! the new balance. When it is under the threshold, no charge is in flight,
//! and this month's top-ups plus the amount stay within the cap, cloud-api
//! charges the saved card off-session: a Stripe PaymentIntent with
//! `off_session=true, confirm=true`, `Idempotency-Key:
//! autotopup-{user}-{attempt_id}` and metadata `kind=auto_topup`,
//! `clerk_user_id`, `allternit_credits_usd`, `allternit_auto_topup_attempt`.
//!
//! The check runs in its own task, so a slow or failing Stripe call never
//! blocks or fails the request that spent the credits. The credits are
//! granted by the `payment_intent.succeeded` webhook
//! (`routes::billing_webhooks`), with the same `stripe-{event_id}` key and
//! `webhook_events` dedupe as credit packs, always into the cloud wallet.
//!
//! A failed charge (declined card, `authentication_required`,
//! `requires_action`), from the API answer or the
//! `payment_intent.payment_failed` / `payment_intent.requires_action`
//! webhook, turns auto top-up off, records the reason, and tells the person
//! by push and email. Turning it back on is their decision.
//!
//! The saved card comes from a setup-mode Checkout
//! (`POST /api/v1/billing/auto-topup/setup`), from a credit pack bought with
//! `setup_future_usage=off_session`, or from the cards already on the
//! person's Stripe customer (a plan subscription saves one).

use chrono::{DateTime, Datelike, Duration, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tracing::{error, info, warn};

use crate::error::ApiError;
use crate::routes::billing_checkout::{ReqwestStripeCheckout, StripeCheckout};

/// Smallest threshold a person can set.
pub const MIN_THRESHOLD_USD: f64 = 5.0;
/// Monthly cap when none is set.
pub const DEFAULT_MONTHLY_CAP_USD: f64 = 200.0;
/// Largest monthly cap a person can set.
pub const MAX_MONTHLY_CAP_USD: f64 = 5000.0;
/// Default top-up amount.
pub const DEFAULT_AMOUNT_USD: f64 = 25.0;
/// Default threshold.
pub const DEFAULT_THRESHOLD_USD: f64 = 5.0;
/// A pending attempt with no answer after this long no longer blocks a new
/// one (the webhook still grants the first if it ever succeeds).
pub const STALE_PENDING_HOURS: i64 = 24;

/// One person's settings and state (`billing_auto_topup`), amounts as f64.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AutoTopupRow {
    pub user_id: String,
    pub enabled: bool,
    pub threshold_usd: f64,
    pub amount_usd: f64,
    pub monthly_cap_usd: f64,
    pub stripe_customer_id: Option<String>,
    pub stripe_payment_method_id: Option<String>,
    pub card_brand: Option<String>,
    pub card_last4: Option<String>,
    pub status: String,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub month_spent_usd: f64,
    pub month_key: String,
    pub updated_at: DateTime<Utc>,
}

const SELECT_ROW: &str = r#"
    SELECT user_id, enabled, threshold_usd::float8 AS threshold_usd, amount_usd::float8 AS amount_usd,
           monthly_cap_usd::float8 AS monthly_cap_usd, stripe_customer_id, stripe_payment_method_id,
           card_brand, card_last4, status, last_attempt_at, last_error,
           month_spent_usd::float8 AS month_spent_usd, month_key, updated_at
    FROM billing_auto_topup WHERE user_id = $1
"#;

/// `YYYY-MM` in UTC: the month a top-up counts against the cap.
pub fn month_key(now: DateTime<Utc>) -> String {
    format!("{:04}-{:02}", now.year(), now.month())
}

/// What the person has spent on top-ups this month (0 once the month rolled).
pub fn spent_this_month(row: &AutoTopupRow, now: DateTime<Utc>) -> f64 {
    if row.month_key == month_key(now) {
        row.month_spent_usd
    } else {
        0.0
    }
}

/// Why a debit did not start a charge.
#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    Disabled,
    NoCard,
    AboveThreshold,
    Pending,
    OverMonthlyCap,
}

/// The pure trigger decision after a debit.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Charge { amount_usd: f64 },
    Skip(SkipReason),
}

/// Decide whether `balance_usd` (after a debit) should start a top-up.
pub fn decide(row: &AutoTopupRow, balance_usd: f64, now: DateTime<Utc>) -> Decision {
    if !row.enabled {
        return Decision::Skip(SkipReason::Disabled);
    }
    if row.stripe_payment_method_id.as_deref().map_or(true, str::is_empty)
        || row.stripe_customer_id.as_deref().map_or(true, str::is_empty)
    {
        return Decision::Skip(SkipReason::NoCard);
    }
    if balance_usd >= row.threshold_usd {
        return Decision::Skip(SkipReason::AboveThreshold);
    }
    if row.status == "pending" {
        let fresh = row
            .last_attempt_at
            .map_or(false, |at| now - at < Duration::hours(STALE_PENDING_HOURS));
        if fresh {
            return Decision::Skip(SkipReason::Pending);
        }
    }
    // Cents, so 150.00 + 50.00 against 200.00 is not lost to float error.
    let spent = (spent_this_month(row, now) * 100.0).round() as i64;
    let amount = (row.amount_usd * 100.0).round() as i64;
    let cap = (row.monthly_cap_usd * 100.0).round() as i64;
    if spent + amount > cap {
        return Decision::Skip(SkipReason::OverMonthlyCap);
    }
    Decision::Charge { amount_usd: row.amount_usd }
}

/// Settings a person can change.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub threshold_usd: f64,
    pub amount_usd: f64,
    pub monthly_cap_usd: f64,
}

/// Check a settings change. `amount_usd` must be a credit pack size.
pub fn validate_settings(settings: &Settings, pack_sizes_usd: &[f64]) -> Result<(), ApiError> {
    let finite = settings.threshold_usd.is_finite()
        && settings.amount_usd.is_finite()
        && settings.monthly_cap_usd.is_finite();
    if !finite {
        return Err(ApiError::BadRequest("Amounts must be numbers.".to_string()));
    }
    if settings.threshold_usd < MIN_THRESHOLD_USD {
        return Err(ApiError::BadRequest(format!(
            "The threshold must be at least ${MIN_THRESHOLD_USD:.0}."
        )));
    }
    if settings.threshold_usd > 1000.0 {
        return Err(ApiError::BadRequest("The threshold can be at most $1000.".to_string()));
    }
    if !pack_sizes_usd.iter().any(|size| (size - settings.amount_usd).abs() < 0.005) {
        let sizes = pack_sizes_usd
            .iter()
            .map(|size| format!("${size:.0}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ApiError::BadRequest(format!("The top-up amount must be one of {sizes}.")));
    }
    if settings.monthly_cap_usd < settings.amount_usd {
        return Err(ApiError::BadRequest(
            "The monthly cap must be at least the top-up amount.".to_string(),
        ));
    }
    if settings.monthly_cap_usd > MAX_MONTHLY_CAP_USD {
        return Err(ApiError::BadRequest(format!(
            "The monthly cap can be at most ${MAX_MONTHLY_CAP_USD:.0}."
        )));
    }
    Ok(())
}

/// A saved card (or Link) as shown to the person.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SavedCard {
    pub payment_method_id: String,
    pub brand: String,
    pub last4: Option<String>,
}

/// The display fields of a Stripe PaymentMethod object. `None` for anything
/// that cannot be charged off-session here (no id, or not card/link).
pub fn saved_card_from_payment_method(payment_method: &Value) -> Option<SavedCard> {
    let id = payment_method["id"].as_str().filter(|id| !id.is_empty())?;
    match payment_method["type"].as_str()? {
        "card" => Some(SavedCard {
            payment_method_id: id.to_string(),
            brand: payment_method["card"]["brand"].as_str().unwrap_or("card").to_string(),
            last4: payment_method["card"]["last4"].as_str().map(str::to_string),
        }),
        "link" => Some(SavedCard {
            payment_method_id: id.to_string(),
            brand: "link".to_string(),
            last4: None,
        }),
        _ => None,
    }
}

/// Form for `POST /v1/payment_intents`: an off-session, confirm-now charge of
/// the saved card with the webhook's grant contract in the metadata.
pub fn payment_intent_form(
    user_id: &str,
    attempt_id: &str,
    customer_id: &str,
    payment_method_id: &str,
    amount_usd: f64,
) -> Vec<(String, String)> {
    vec![
        ("amount".to_string(), ((amount_usd * 100.0).round() as u64).to_string()),
        ("currency".to_string(), "usd".to_string()),
        ("customer".to_string(), customer_id.to_string()),
        ("payment_method".to_string(), payment_method_id.to_string()),
        ("off_session".to_string(), "true".to_string()),
        ("confirm".to_string(), "true".to_string()),
        (
            "description".to_string(),
            format!("Allternit Cloud credits — auto top-up ${amount_usd:.2}"),
        ),
        ("metadata[kind]".to_string(), "auto_topup".to_string()),
        ("metadata[clerk_user_id]".to_string(), user_id.to_string()),
        ("metadata[allternit_credits_usd]".to_string(), format!("{amount_usd:.2}")),
        ("metadata[allternit_auto_topup_attempt]".to_string(), attempt_id.to_string()),
    ]
}

/// The Stripe idempotency key for one attempt.
pub fn idempotency_key(user_id: &str, attempt_id: &str) -> String {
    format!("autotopup-{user_id}-{attempt_id}")
}

/// What the PaymentIntent create answer means for the attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum IntentOutcome {
    /// Settled or settling: the webhook grants the credits.
    InFlight,
    /// The charge cannot finish without the person.
    Failed(String),
}

/// Map a PaymentIntent `status` from the create answer.
pub fn classify_intent_status(status: &str) -> IntentOutcome {
    match status {
        "succeeded" | "processing" | "requires_capture" => IntentOutcome::InFlight,
        "requires_action" => IntentOutcome::Failed(
            "Your bank asked to confirm this payment, which can't happen automatically.".to_string(),
        ),
        "requires_payment_method" => IntentOutcome::Failed("The card was declined.".to_string()),
        "canceled" => IntentOutcome::Failed("The payment was canceled.".to_string()),
        other => IntentOutcome::Failed(format!("Unexpected payment status: {other}.")),
    }
}

/// The reason to show for a failed PaymentIntent object from a webhook.
pub fn failure_reason(payment_intent: &Value, event_type: &str) -> String {
    if event_type == "payment_intent.requires_action" {
        return "Your bank asked to confirm this payment, which can't happen automatically.".to_string();
    }
    let error = &payment_intent["last_payment_error"];
    if error["code"].as_str() == Some("authentication_required") {
        return "Your bank asked to confirm this payment, which can't happen automatically.".to_string();
    }
    error["message"]
        .as_str()
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "The card was declined.".to_string())
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

pub async fn load(db: &PgPool, user_id: &str) -> Result<Option<AutoTopupRow>, ApiError> {
    sqlx::query_as::<_, AutoTopupRow>(SELECT_ROW)
        .bind(user_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::DatabaseError)
}

/// Save a settings change. Turning it on clears a `failed`/`disabled` state.
pub async fn save_settings(db: &PgPool, user_id: &str, settings: &Settings) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO billing_auto_topup (user_id, enabled, threshold_usd, amount_usd, monthly_cap_usd, status, updated_at)
        VALUES ($1, $2, $3::float8, $4::float8, $5::float8, CASE WHEN $2 THEN 'ok' ELSE 'disabled' END, now())
        ON CONFLICT (user_id) DO UPDATE SET
            enabled = excluded.enabled,
            threshold_usd = excluded.threshold_usd,
            amount_usd = excluded.amount_usd,
            monthly_cap_usd = excluded.monthly_cap_usd,
            status = CASE
                WHEN billing_auto_topup.status = 'pending' THEN 'pending'
                WHEN excluded.enabled THEN 'ok'
                ELSE 'disabled'
            END,
            last_error = CASE WHEN excluded.enabled THEN NULL ELSE billing_auto_topup.last_error END,
            updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(settings.enabled)
    .bind(settings.threshold_usd)
    .bind(settings.amount_usd)
    .bind(settings.monthly_cap_usd)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// Store the card to charge. A row that does not exist yet starts off.
pub async fn save_card(db: &PgPool, user_id: &str, customer_id: &str, card: &SavedCard) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO billing_auto_topup (user_id, enabled, threshold_usd, amount_usd, monthly_cap_usd,
                                        stripe_customer_id, stripe_payment_method_id, card_brand, card_last4,
                                        status, updated_at)
        VALUES ($1, false, $6::float8, $7::float8, $8::float8, $2, $3, $4, $5, 'disabled', now())
        ON CONFLICT (user_id) DO UPDATE SET
            stripe_customer_id = excluded.stripe_customer_id,
            stripe_payment_method_id = excluded.stripe_payment_method_id,
            card_brand = excluded.card_brand,
            card_last4 = excluded.card_last4,
            updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(customer_id)
    .bind(&card.payment_method_id)
    .bind(&card.brand)
    .bind(&card.last4)
    .bind(DEFAULT_THRESHOLD_USD)
    .bind(DEFAULT_AMOUNT_USD)
    .bind(DEFAULT_MONTHLY_CAP_USD)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(())
}

/// The person's Stripe customer id, if one is linked.
pub async fn linked_customer(db: &PgPool, user_id: &str) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar("SELECT stripe_customer_id FROM user_billing_accounts WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::DatabaseError)
}

/// The person's Stripe customer: the linked one (from an earlier plan or
/// pack), else a new one created with an idempotency key per user and
/// linked in `user_billing_accounts`, so one person stays one customer.
pub async fn ensure_customer(
    db: &PgPool,
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    user_id: &str,
    email: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(customer_id) = linked_customer(db, user_id).await? {
        return Ok(customer_id);
    }
    let mut form = vec![("metadata[clerk_user_id]".to_string(), user_id.to_string())];
    if let Some(email) = email.filter(|email| !email.trim().is_empty()) {
        form.push(("email".to_string(), email.trim().to_string()));
    }
    let customer = stripe
        .post_object(secret_key, "/v1/customers", Some(&format!("customer-{user_id}")), &form)
        .await?;
    let customer_id = customer["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::Internal("Stripe returned no customer id".to_string()))?
        .to_string();
    crate::routes::billing_subscriptions::upsert_user_billing_account(db, user_id, &customer_id).await?;
    Ok(customer_id)
}

/// The newest card on the Stripe customer (a plan subscription or an
/// earlier pack saved it), stored for auto top-up. `None` when there is none.
pub async fn sync_card_from_customer(
    db: &PgPool,
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    user_id: &str,
    customer_id: &str,
) -> Result<Option<SavedCard>, ApiError> {
    let list = stripe
        .get_object(
            secret_key,
            &format!("/v1/customers/{customer_id}/payment_methods"),
            &[("type".to_string(), "card".to_string()), ("limit".to_string(), "1".to_string())],
        )
        .await?;
    let Some(card) = list["data"].as_array().and_then(|data| data.first()).and_then(saved_card_from_payment_method)
    else {
        return Ok(None);
    };
    save_card(db, user_id, customer_id, &card).await?;
    Ok(Some(card))
}

/// After a credit pack bought with `setup_future_usage=off_session`: store
/// the card it saved. Best effort in its own task; the grant already landed.
pub fn spawn_capture_from_payment_intent(db: PgPool, user_id: String, payment_intent_id: String) {
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else { return };
    tokio::spawn(async move {
        let stripe = ReqwestStripeCheckout::new();
        let intent = match stripe
            .get_object(
                &secret_key,
                &format!("/v1/payment_intents/{payment_intent_id}"),
                &[("expand[]".to_string(), "payment_method".to_string())],
            )
            .await
        {
            Ok(intent) => intent,
            Err(error) => {
                warn!(%user_id, %payment_intent_id, %error, "auto top-up: could not read the pack's payment method");
                return;
            }
        };
        let customer_id = intent["customer"].as_str().or_else(|| intent["customer"]["id"].as_str());
        let (Some(customer_id), Some(card)) = (customer_id, saved_card_from_payment_method(&intent["payment_method"]))
        else {
            return;
        };
        // Only a payment method attached to the customer can be charged later.
        let attached = intent["payment_method"]["customer"].as_str() == Some(customer_id)
            || intent["setup_future_usage"].as_str() == Some("off_session");
        if !attached {
            return;
        }
        if let Err(error) = save_card(&db, &user_id, customer_id, &card).await {
            warn!(%user_id, %error, "auto top-up: could not store the pack's card");
        }
    });
}

/// `checkout.session.completed` for a setup-mode session: read the
/// SetupIntent's payment method and store it. Errors make Stripe retry.
pub async fn store_setup_payment_method(
    db: &PgPool,
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    user_id: &str,
    setup_intent_id: &str,
    customer_hint: Option<&str>,
) -> Result<SavedCard, ApiError> {
    let intent = stripe
        .get_object(
            secret_key,
            &format!("/v1/setup_intents/{setup_intent_id}"),
            &[("expand[]".to_string(), "payment_method".to_string())],
        )
        .await?;
    let card = saved_card_from_payment_method(&intent["payment_method"])
        .ok_or_else(|| ApiError::BadRequest("The saved payment method can't be charged automatically.".to_string()))?;
    let customer_id = intent["customer"]
        .as_str()
        .or_else(|| intent["customer"]["id"].as_str())
        .or(customer_hint)
        .ok_or_else(|| ApiError::BadRequest("Setup session has no customer.".to_string()))?
        .to_string();
    save_card(db, user_id, &customer_id, &card).await?;
    Ok(card)
}

// ---------------------------------------------------------------------------
// Trigger
// ---------------------------------------------------------------------------

/// After a wallet debit: start a top-up in the background when one is due.
/// Never blocks the caller and never fails it.
pub fn spawn_check(db: PgPool, user_id: String, balance_usd: f64) {
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else { return };
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    tokio::spawn(async move {
        let stripe = ReqwestStripeCheckout::new();
        if let Err(error) = run_check(&db, &stripe, &secret_key, &user_id, balance_usd).await {
            error!(%user_id, %error, "auto top-up check failed");
        }
    });
}

/// The trigger: decide, claim the one pending slot, charge, record.
pub async fn run_check(
    db: &PgPool,
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    user_id: &str,
    balance_usd: f64,
) -> Result<Option<String>, ApiError> {
    let Some(row) = load(db, user_id).await? else { return Ok(None) };
    let now = Utc::now();
    let Decision::Charge { amount_usd } = decide(&row, balance_usd, now) else {
        return Ok(None);
    };
    let (Some(customer_id), Some(payment_method_id)) =
        (row.stripe_customer_id.clone(), row.stripe_payment_method_id.clone())
    else {
        return Ok(None);
    };

    // A pending attempt past STALE_PENDING_HOURS no longer holds the slot.
    sqlx::query(
        r#"
        UPDATE billing_auto_topup_attempts
        SET status = 'failed', error = 'No answer from Stripe within 24 hours.', updated_at = now()
        WHERE user_id = $1 AND status = 'pending' AND created_at < now() - make_interval(hours => $2)
        "#,
    )
    .bind(user_id)
    .bind(STALE_PENDING_HOURS as i32)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Claim: the partial unique index allows one pending attempt per person,
    // so two debits racing here start one charge.
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let claimed = sqlx::query(
        "INSERT INTO billing_auto_topup_attempts (id, user_id, amount_usd, status) VALUES ($1, $2, $3::float8, 'pending') ON CONFLICT DO NOTHING",
    )
    .bind(&attempt_id)
    .bind(user_id)
    .bind(amount_usd)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    if claimed.rows_affected() == 0 {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE billing_auto_topup SET status = 'pending', last_attempt_at = now(), updated_at = now() WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;

    let form = payment_intent_form(user_id, &attempt_id, &customer_id, &payment_method_id, amount_usd);
    let key = idempotency_key(user_id, &attempt_id);
    match stripe.post_object(secret_key, "/v1/payment_intents", Some(&key), &form).await {
        Ok(intent) => {
            let intent_id = intent["id"].as_str().unwrap_or_default().to_string();
            if !intent_id.is_empty() {
                sqlx::query(
                    "UPDATE billing_auto_topup_attempts SET payment_intent_id = $2, updated_at = now() WHERE id = $1 AND payment_intent_id IS NULL",
                )
                .bind(&attempt_id)
                .bind(&intent_id)
                .execute(db)
                .await
                .map_err(ApiError::DatabaseError)?;
            }
            match classify_intent_status(intent["status"].as_str().unwrap_or_default()) {
                IntentOutcome::InFlight => {
                    info!(%user_id, %attempt_id, %intent_id, amount_usd, "auto top-up charge started");
                }
                IntentOutcome::Failed(reason) => {
                    if intent["status"].as_str() == Some("requires_action") && !intent_id.is_empty() {
                        // Don't leave a payment waiting for a person who isn't there.
                        let _ = stripe
                            .post_object(secret_key, &format!("/v1/payment_intents/{intent_id}/cancel"), None, &[])
                            .await;
                    }
                    record_failure(db, user_id, Some(&attempt_id), None, amount_usd, &reason).await?;
                }
            }
        }
        Err(error) => {
            let reason = stripe_error_reason(&error);
            record_failure(db, user_id, Some(&attempt_id), None, amount_usd, &reason).await?;
        }
    }
    Ok(Some(attempt_id))
}

/// The person-facing reason from a Stripe API error.
fn stripe_error_reason(error: &ApiError) -> String {
    let text = error.to_string();
    let message = text
        .split_once("Stripe rejected the session: ")
        .map(|(_, message)| message.to_string())
        .unwrap_or(text);
    if message.contains("authentication") {
        "Your bank asked to confirm this payment, which can't happen automatically.".to_string()
    } else {
        message
    }
}

/// A successful auto top-up (webhook, after the grant): close the attempt
/// and count it against the month. Returns false when already recorded.
pub async fn record_success(
    db: &PgPool,
    user_id: &str,
    attempt_id: Option<&str>,
    payment_intent_id: &str,
    amount_usd: f64,
) -> Result<bool, ApiError> {
    let updated = sqlx::query(
        r#"
        UPDATE billing_auto_topup_attempts
        SET status = 'succeeded', payment_intent_id = COALESCE(payment_intent_id, $3), error = NULL, updated_at = now()
        WHERE user_id = $1 AND status <> 'succeeded'
          AND (id = $2 OR payment_intent_id = $3)
        "#,
    )
    .bind(user_id)
    .bind(attempt_id.unwrap_or_default())
    .bind(payment_intent_id)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    if updated.rows_affected() == 0 {
        return Ok(false);
    }
    let key = month_key(Utc::now());
    sqlx::query(
        r#"
        UPDATE billing_auto_topup SET
            month_spent_usd = CASE WHEN month_key = $2 THEN month_spent_usd + $3::float8 ELSE $3::float8 END,
            month_key = $2,
            status = CASE WHEN status = 'pending' THEN 'ok' ELSE status END,
            last_error = NULL,
            updated_at = now()
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .bind(&key)
    .bind(amount_usd)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(true)
}

/// A failed auto top-up (API answer or webhook): close the attempt, turn
/// auto top-up off, keep the reason, and tell the person once.
pub async fn record_failure(
    db: &PgPool,
    user_id: &str,
    attempt_id: Option<&str>,
    payment_intent_id: Option<&str>,
    amount_usd: f64,
    reason: &str,
) -> Result<(), ApiError> {
    let closed: Option<String> = sqlx::query_scalar(
        r#"
        UPDATE billing_auto_topup_attempts
        SET status = 'failed', error = $4, payment_intent_id = COALESCE(payment_intent_id, $3), updated_at = now()
        WHERE user_id = $1 AND status = 'pending'
          AND (id = $2 OR ($3::text IS NOT NULL AND payment_intent_id = $3))
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(attempt_id.unwrap_or_default())
    .bind(payment_intent_id)
    .bind(reason)
    .fetch_optional(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    sqlx::query(
        "UPDATE billing_auto_topup SET enabled = false, status = 'failed', last_error = $2, updated_at = now() WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(reason)
    .execute(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    warn!(%user_id, ?attempt_id, ?payment_intent_id, reason, "auto top-up failed; turned off");
    if let Some(attempt_id) = closed {
        notify_failure(db.clone(), user_id.to_string(), attempt_id, amount_usd, reason.to_string());
    }
    Ok(())
}

/// Push and email: "auto top-up failed and is off". Once per attempt.
fn notify_failure(db: PgPool, user_id: String, attempt_id: String, amount_usd: f64, reason: String) {
    let message = crate::routes::web_push::PushMessage {
        kind: "event",
        title: "Auto top-up failed".to_string(),
        body: format!("We couldn't add ${amount_usd:.2} in credits. Auto top-up is off until you turn it back on."),
        url: "/settings/billing".to_string(),
        tag: "auto-topup-failed".to_string(),
        data: json!({ "kind": "auto_topup_failed", "attempt_id": attempt_id }),
        ttl_secs: 24 * 3600,
        high_urgency: false,
    };
    crate::routes::web_push::spawn_notify(db.clone(), user_id.clone(), message, false);
    if !crate::services::customer_emails::enabled() {
        return;
    }
    tokio::spawn(async move {
        let Some(recipient) = crate::services::customer_emails::fetch_recipient(&user_id).await else {
            warn!(%user_id, "auto top-up failed: no email from Clerk; no email");
            return;
        };
        let email = crate::services::customer_emails::auto_topup_failed_email(&recipient, amount_usd, &reason);
        crate::services::customer_emails::deliver(&db, &user_id, "auto_topup_failed", &attempt_id, &recipient, &email)
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 25, 12, 0, 0).unwrap()
    }

    fn row() -> AutoTopupRow {
        AutoTopupRow {
            user_id: "user_1".to_string(),
            enabled: true,
            threshold_usd: 5.0,
            amount_usd: 25.0,
            monthly_cap_usd: 200.0,
            stripe_customer_id: Some("cus_1".to_string()),
            stripe_payment_method_id: Some("pm_1".to_string()),
            card_brand: Some("visa".to_string()),
            card_last4: Some("4242".to_string()),
            status: "ok".to_string(),
            last_attempt_at: None,
            last_error: None,
            month_spent_usd: 0.0,
            month_key: "2026-10".to_string(),
            updated_at: now(),
        }
    }

    #[test]
    fn charges_when_balance_drops_under_the_threshold() {
        assert_eq!(decide(&row(), 4.99, now()), Decision::Charge { amount_usd: 25.0 });
        assert_eq!(decide(&row(), 5.0, now()), Decision::Skip(SkipReason::AboveThreshold));
        assert_eq!(decide(&row(), 80.0, now()), Decision::Skip(SkipReason::AboveThreshold));
    }

    #[test]
    fn skips_when_off_or_without_a_card() {
        let mut off = row();
        off.enabled = false;
        assert_eq!(decide(&off, 0.0, now()), Decision::Skip(SkipReason::Disabled));
        let mut no_card = row();
        no_card.stripe_payment_method_id = None;
        assert_eq!(decide(&no_card, 0.0, now()), Decision::Skip(SkipReason::NoCard));
        let mut no_customer = row();
        no_customer.stripe_customer_id = Some(String::new());
        assert_eq!(decide(&no_customer, 0.0, now()), Decision::Skip(SkipReason::NoCard));
    }

    #[test]
    fn one_pending_attempt_at_a_time_until_it_goes_stale() {
        let mut pending = row();
        pending.status = "pending".to_string();
        pending.last_attempt_at = Some(now() - Duration::minutes(5));
        assert_eq!(decide(&pending, 1.0, now()), Decision::Skip(SkipReason::Pending));
        pending.last_attempt_at = Some(now() - Duration::hours(STALE_PENDING_HOURS + 1));
        assert_eq!(decide(&pending, 1.0, now()), Decision::Charge { amount_usd: 25.0 });
    }

    #[test]
    fn respects_the_monthly_cap_and_resets_each_month() {
        let mut spent = row();
        spent.month_spent_usd = 175.0;
        assert_eq!(decide(&spent, 1.0, now()), Decision::Charge { amount_usd: 25.0 }, "175 + 25 = cap is allowed");
        spent.month_spent_usd = 175.01;
        assert_eq!(decide(&spent, 1.0, now()), Decision::Skip(SkipReason::OverMonthlyCap));
        spent.month_key = "2026-09".to_string();
        assert_eq!(decide(&spent, 1.0, now()), Decision::Charge { amount_usd: 25.0 }, "last month's spend does not count");
        assert_eq!(month_key(now()), "2026-10");
    }

    #[test]
    fn settings_validation() {
        let sizes = [10.0, 25.0, 50.0, 100.0];
        let ok = Settings { enabled: true, threshold_usd: 5.0, amount_usd: 25.0, monthly_cap_usd: 200.0 };
        assert!(validate_settings(&ok, &sizes).is_ok());
        for bad in [
            Settings { threshold_usd: 4.99, ..ok.clone() },
            Settings { amount_usd: 20.0, ..ok.clone() },
            Settings { monthly_cap_usd: 10.0, ..ok.clone() },
            Settings { monthly_cap_usd: MAX_MONTHLY_CAP_USD + 1.0, ..ok.clone() },
            Settings { threshold_usd: f64::NAN, ..ok.clone() },
        ] {
            assert!(validate_settings(&bad, &sizes).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn payment_intent_form_is_off_session_with_the_grant_contract() {
        let form = payment_intent_form("user_1", "att_1", "cus_1", "pm_1", 25.0);
        let field = |key: &str| form.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(field("amount"), "2500");
        assert_eq!(field("currency"), "usd");
        assert_eq!(field("customer"), "cus_1");
        assert_eq!(field("payment_method"), "pm_1");
        assert_eq!(field("off_session"), "true");
        assert_eq!(field("confirm"), "true");
        assert_eq!(field("metadata[kind]"), "auto_topup");
        assert_eq!(field("metadata[clerk_user_id]"), "user_1");
        assert_eq!(field("metadata[allternit_credits_usd]"), "25.00");
        assert_eq!(field("metadata[allternit_auto_topup_attempt]"), "att_1");
        assert_eq!(idempotency_key("user_1", "att_1"), "autotopup-user_1-att_1");
    }

    #[test]
    fn intent_status_mapping() {
        assert_eq!(classify_intent_status("succeeded"), IntentOutcome::InFlight);
        assert_eq!(classify_intent_status("processing"), IntentOutcome::InFlight);
        assert!(matches!(classify_intent_status("requires_action"), IntentOutcome::Failed(_)));
        assert!(matches!(classify_intent_status("requires_payment_method"), IntentOutcome::Failed(_)));
    }

    #[test]
    fn failure_reasons() {
        let auth = json!({ "last_payment_error": { "code": "authentication_required", "message": "x" } });
        assert!(failure_reason(&auth, "payment_intent.payment_failed").contains("bank"));
        let declined = json!({ "last_payment_error": { "code": "card_declined", "message": "Your card has insufficient funds." } });
        assert_eq!(failure_reason(&declined, "payment_intent.payment_failed"), "Your card has insufficient funds.");
        assert!(failure_reason(&json!({}), "payment_intent.requires_action").contains("bank"));
        assert_eq!(failure_reason(&json!({}), "payment_intent.payment_failed"), "The card was declined.");
    }

    #[test]
    fn saved_card_display() {
        let card = json!({ "id": "pm_1", "type": "card", "card": { "brand": "visa", "last4": "4242" } });
        assert_eq!(
            saved_card_from_payment_method(&card),
            Some(SavedCard { payment_method_id: "pm_1".to_string(), brand: "visa".to_string(), last4: Some("4242".to_string()) })
        );
        let link = json!({ "id": "pm_2", "type": "link" });
        assert_eq!(saved_card_from_payment_method(&link).unwrap().brand, "link");
        assert_eq!(saved_card_from_payment_method(&json!({ "id": "pm_3", "type": "us_bank_account" })), None);
        assert_eq!(saved_card_from_payment_method(&json!("pm_4")), None, "an unexpanded id is not enough");
    }

    #[test]
    fn stripe_error_reason_strips_the_helper_prefix() {
        let error = ApiError::Internal("Stripe rejected the session: Your card was declined.".to_string());
        assert_eq!(stripe_error_reason(&error), "Your card was declined.");
        let auth = ApiError::Internal("Stripe rejected the session: This payment requires authentication.".to_string());
        assert!(stripe_error_reason(&auth).contains("bank"));
    }
}
