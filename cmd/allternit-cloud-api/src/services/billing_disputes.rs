//! Chargebacks: hold disputed credits, give them back after a win.
//!
//! Driven by `routes::billing_webhooks` (`charge.dispute.created` /
//! `charge.dispute.closed`). One `billing_disputes` row per Stripe dispute
//! (migration 106):
//!
//! - Opened: the disputed amount (never more than the charge) leaves the
//!   cloud wallet as a negative `credit_transactions` row with source
//!   [`HOLD_SOURCE`], keyed `stripe-{event_id}`. The balance may go negative,
//!   so credits bought with a charge that is being pulled back cannot be
//!   spent. Auto top-up is turned off with `last_error = "dispute"` and
//!   cannot be turned back on while the dispute is open or after it was lost.
//! - Won (or an inquiry closed without a chargeback, `warning_closed`): the
//!   held amount comes back as a positive row with source
//!   [`RELEASE_SOURCE`], keyed `stripe-{event_id}`, once per dispute.
//! - Lost: the hold stays; only the row status changes.
//!
//! The row insert, ledger row, balance change and auto top-up switch happen
//! in one database transaction, so a crash never leaves half a hold.

use chrono::Utc;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;

/// `credit_transactions.source` of the hold taken when a dispute opens.
pub const HOLD_SOURCE: &str = "dispute_hold";
/// `credit_transactions.source` of the give-back after a won dispute.
pub const RELEASE_SOURCE: &str = "dispute_release";
/// `billing_auto_topup.last_error` while a dispute blocks auto top-up.
pub const AUTO_TOPUP_DISPUTE_ERROR: &str = "dispute";

/// What a closed dispute does to the hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeOutcome {
    /// `won`: give the held amount back.
    Won,
    /// `warning_closed`: an inquiry that never became a chargeback; give the
    /// held amount back.
    InquiryClosed,
    /// `lost`: the money is gone; the hold stays.
    Lost,
}

impl DisputeOutcome {
    /// The outcome for a closed dispute's Stripe status; `None` = ignored.
    pub fn from_stripe_status(status: &str) -> Option<Self> {
        match status {
            "won" => Some(Self::Won),
            "warning_closed" => Some(Self::InquiryClosed),
            "lost" => Some(Self::Lost),
            _ => None,
        }
    }

    fn row_status(self) -> &'static str {
        match self {
            Self::Won => "won",
            Self::InquiryClosed => "closed",
            Self::Lost => "lost",
        }
    }

    fn releases_hold(self) -> bool {
        matches!(self, Self::Won | Self::InquiryClosed)
    }
}

/// The disputed payment as read from a PaymentIntent or Charge object.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisputedPayment {
    /// `metadata.clerk_user_id` (packs and auto top-ups write it).
    pub user_id: Option<String>,
    /// The Stripe customer id, for the mirror-table fallback.
    pub customer_id: Option<String>,
    /// What was actually charged, in cents.
    pub charged_cents: Option<i64>,
    /// `metadata.allternit_org_id`: the pack's credits went to this org's
    /// fabric ledger when the bridge was on.
    pub organization_id: Option<String>,
    /// `metadata.kind` (`credit_pack`, `auto_topup`, ...).
    pub kind: Option<String>,
}

fn non_empty(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn id_of(value: &Value) -> Option<String> {
    non_empty(value).or_else(|| non_empty(&value["id"]))
}

impl DisputedPayment {
    /// Read a PaymentIntent (`amount_received`) or Charge (`amount_captured`)
    /// object. Returns `None` for a bare id string (not expanded).
    pub fn from_object(object: &Value) -> Option<Self> {
        if !object.is_object() {
            return None;
        }
        let metadata = &object["metadata"];
        let charged_cents = ["amount_received", "amount_captured", "amount"]
            .iter()
            .filter_map(|key| object[*key].as_i64())
            .find(|cents| *cents > 0);
        Some(Self {
            user_id: non_empty(&metadata["clerk_user_id"]),
            customer_id: id_of(&object["customer"]),
            charged_cents,
            organization_id: non_empty(&metadata["allternit_org_id"]),
            kind: non_empty(&metadata["kind"]),
        })
    }
}

/// The amount to hold, in USD: the disputed cents, never more than was
/// charged (when the charge amount is known), never negative.
pub fn hold_amount_usd(dispute_cents: i64, charged_cents: Option<i64>) -> f64 {
    let mut cents = dispute_cents.max(0);
    if let Some(charged) = charged_cents.filter(|charged| *charged >= 0) {
        cents = cents.min(charged);
    }
    cents as f64 / 100.0
}

/// The person behind a Stripe customer, through the subscription mirror,
/// the billing account link, or the auto top-up card.
pub async fn user_for_customer(db: &PgPool, customer_id: &str) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT user_id FROM (
            SELECT user_id, 1 AS rank, updated_at FROM billing_subscriptions WHERE stripe_customer_id = $1
            UNION ALL
            SELECT user_id, 2 AS rank, updated_at FROM user_billing_accounts WHERE stripe_customer_id = $1
            UNION ALL
            SELECT user_id, 3 AS rank, updated_at FROM billing_auto_topup WHERE stripe_customer_id = $1
        ) matches
        ORDER BY rank, updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(customer_id)
    .fetch_optional(db)
    .await
    .map_err(ApiError::DatabaseError)
}

/// A newly opened dispute, resolved as far as it could be.
#[derive(Debug, Clone)]
pub struct OpenedDispute {
    pub event_id: String,
    pub dispute_id: String,
    /// `None` = the charge could not be traced to a person.
    pub user_id: Option<String>,
    pub charge_id: Option<String>,
    pub payment_intent_id: Option<String>,
    pub customer_id: Option<String>,
    pub amount_cents: i64,
    pub currency: Option<String>,
    pub reason: Option<String>,
    pub stripe_status: Option<String>,
    /// The amount to take out of the wallet (see [`hold_amount_usd`]).
    pub hold_usd: f64,
}

/// What recording an opened dispute did.
#[derive(Debug, Clone, PartialEq)]
pub struct HoldOutcome {
    /// False when this dispute was already recorded (a Stripe retry).
    pub fresh: bool,
    /// What is held for this dispute.
    pub held_usd: f64,
    /// Wallet balance afterwards, when a person was found.
    pub balance_usd: Option<f64>,
}

/// Record an opened dispute, hold its amount and turn auto top-up off, in
/// one transaction. Idempotent per dispute id and per `stripe-{event_id}`.
pub async fn open(db: &PgPool, dispute: &OpenedDispute) -> Result<HoldOutcome, ApiError> {
    let mut tx = db.begin().await.map_err(ApiError::DatabaseError)?;
    let row_status = if dispute.user_id.is_some() { "open" } else { "unmatched" };
    let hold_usd = if dispute.user_id.is_some() { dispute.hold_usd.max(0.0) } else { 0.0 };
    let inserted: Option<String> = sqlx::query_scalar(
        r#"
        INSERT INTO billing_disputes (
            stripe_dispute_id, user_id, stripe_charge_id, stripe_payment_intent_id,
            stripe_customer_id, amount_cents, currency, held_usd, status, stripe_status,
            reason, opened_event_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        ON CONFLICT (stripe_dispute_id) DO NOTHING
        RETURNING stripe_dispute_id
        "#,
    )
    .bind(&dispute.dispute_id)
    .bind(&dispute.user_id)
    .bind(&dispute.charge_id)
    .bind(&dispute.payment_intent_id)
    .bind(&dispute.customer_id)
    .bind(dispute.amount_cents)
    .bind(&dispute.currency)
    .bind(hold_usd)
    .bind(row_status)
    .bind(&dispute.stripe_status)
    .bind(&dispute.reason)
    .bind(&dispute.event_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::DatabaseError)?;

    if inserted.is_none() {
        tx.commit().await.map_err(ApiError::DatabaseError)?;
        let existing: Option<(Option<String>, f64)> = sqlx::query_as(
            "SELECT user_id, held_usd FROM billing_disputes WHERE stripe_dispute_id = $1",
        )
        .bind(&dispute.dispute_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::DatabaseError)?;
        let (user_id, held_usd) = existing.unwrap_or((None, 0.0));
        let balance_usd = match user_id {
            Some(user_id) => Some(balance(db, &user_id).await?),
            None => None,
        };
        return Ok(HoldOutcome { fresh: false, held_usd, balance_usd });
    }

    let Some(user_id) = dispute.user_id.as_deref() else {
        tx.commit().await.map_err(ApiError::DatabaseError)?;
        return Ok(HoldOutcome { fresh: true, held_usd: 0.0, balance_usd: None });
    };

    if hold_usd > 0.0 {
        let transaction_id = format!("stripe-{}", dispute.event_id);
        let ledger = sqlx::query(
            r#"
            INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (transaction_id) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(-hold_usd)
        .bind(&transaction_id)
        .bind(HOLD_SOURCE)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;
        if ledger.rows_affected() > 0 {
            // No clamp at zero: the person must not keep spending credits
            // whose payment is being pulled back.
            sqlx::query(
                r#"
                INSERT INTO user_credits (user_id, balance_usd)
                VALUES ($1, $2)
                ON CONFLICT (user_id) DO UPDATE SET
                    balance_usd = user_credits.balance_usd + excluded.balance_usd,
                    updated_at = now()
                "#,
            )
            .bind(user_id)
            .bind(-hold_usd)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::DatabaseError)?;
        }
        sqlx::query("UPDATE billing_disputes SET hold_transaction_id = $2, updated_at = now() WHERE stripe_dispute_id = $1")
            .bind(&dispute.dispute_id)
            .bind(&transaction_id)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::DatabaseError)?;
    }

    // A disputed card must not be charged again automatically.
    sqlx::query(
        "UPDATE billing_auto_topup SET enabled = false, status = 'failed', last_error = $2, updated_at = now() WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(AUTO_TOPUP_DISPUTE_ERROR)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::DatabaseError)?;

    tx.commit().await.map_err(ApiError::DatabaseError)?;
    Ok(HoldOutcome {
        fresh: true,
        held_usd: hold_usd,
        balance_usd: Some(balance(db, user_id).await?),
    })
}

/// What closing a dispute did.
#[derive(Debug, Clone, PartialEq)]
pub enum CloseOutcome {
    /// The held amount went back into the wallet.
    Released { user_id: String, amount_usd: f64, balance_usd: f64 },
    /// Lost: the hold stays.
    KeptRemoved { user_id: Option<String>, held_usd: f64 },
    /// Already closed with a give-back (a Stripe retry), or nothing was held.
    NothingToDo { user_id: Option<String> },
    /// No record of this dispute (opened before this code, or never ours).
    Unknown,
}

/// Apply a closed dispute's outcome. A give-back happens at most once per
/// dispute (`release_transaction_id`) and per `stripe-{event_id}`.
pub async fn close(
    db: &PgPool,
    dispute_id: &str,
    outcome: DisputeOutcome,
    stripe_status: &str,
    event_id: &str,
) -> Result<CloseOutcome, ApiError> {
    let mut tx = db.begin().await.map_err(ApiError::DatabaseError)?;
    let row: Option<(Option<String>, f64, Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT user_id, held_usd, hold_transaction_id, release_transaction_id
        FROM billing_disputes WHERE stripe_dispute_id = $1 FOR UPDATE
        "#,
    )
    .bind(dispute_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::DatabaseError)?;
    let Some((user_id, held_usd, hold_transaction_id, release_transaction_id)) = row else {
        tx.commit().await.map_err(ApiError::DatabaseError)?;
        return Ok(CloseOutcome::Unknown);
    };

    let mut released: Option<String> = None;
    if outcome.releases_hold() && release_transaction_id.is_none() && hold_transaction_id.is_some() && held_usd > 0.0 {
        if let Some(user_id) = user_id.as_deref() {
            let transaction_id = format!("stripe-{event_id}");
            let ledger = sqlx::query(
                r#"
                INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (transaction_id) DO NOTHING
                "#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(user_id)
            .bind(held_usd)
            .bind(&transaction_id)
            .bind(RELEASE_SOURCE)
            .bind(Utc::now())
            .execute(&mut *tx)
            .await
            .map_err(ApiError::DatabaseError)?;
            if ledger.rows_affected() > 0 {
                sqlx::query(
                    r#"
                    INSERT INTO user_credits (user_id, balance_usd)
                    VALUES ($1, $2)
                    ON CONFLICT (user_id) DO UPDATE SET
                        balance_usd = user_credits.balance_usd + excluded.balance_usd,
                        updated_at = now()
                    "#,
                )
                .bind(user_id)
                .bind(held_usd)
                .execute(&mut *tx)
                .await
                .map_err(ApiError::DatabaseError)?;
            }
            released = Some(transaction_id);
        }
    }

    sqlx::query(
        r#"
        UPDATE billing_disputes SET
            status = $2,
            stripe_status = $3,
            closed_event_id = COALESCE(closed_event_id, $4),
            release_transaction_id = COALESCE(release_transaction_id, $5),
            updated_at = now()
        WHERE stripe_dispute_id = $1
        "#,
    )
    .bind(dispute_id)
    .bind(outcome.row_status())
    .bind(stripe_status)
    .bind(event_id)
    .bind(&released)
    .execute(&mut *tx)
    .await
    .map_err(ApiError::DatabaseError)?;
    tx.commit().await.map_err(ApiError::DatabaseError)?;

    match (released, user_id) {
        (Some(_), Some(user_id)) => {
            let balance_usd = balance(db, &user_id).await?;
            Ok(CloseOutcome::Released { user_id, amount_usd: held_usd, balance_usd })
        }
        (_, user_id) if outcome == DisputeOutcome::Lost => Ok(CloseOutcome::KeptRemoved { user_id, held_usd }),
        (_, user_id) => Ok(CloseOutcome::NothingToDo { user_id }),
    }
}

/// Whether an open or lost dispute keeps auto top-up off for this person.
pub async fn blocks_auto_topup(db: &PgPool, user_id: &str) -> Result<bool, ApiError> {
    let blocked: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM billing_disputes WHERE user_id = $1 AND status IN ('open', 'lost') LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    Ok(blocked.is_some())
}

async fn balance(db: &PgPool, user_id: &str) -> Result<f64, ApiError> {
    let balance: Option<f64> = sqlx::query_scalar("SELECT balance_usd::float8 FROM user_credits WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::DatabaseError)?;
    Ok(balance.unwrap_or(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hold_never_exceeds_the_charge_and_never_goes_negative() {
        assert_eq!(hold_amount_usd(2500, Some(2500)), 25.0);
        assert_eq!(hold_amount_usd(5000, Some(2500)), 25.0, "capped at what was charged");
        assert_eq!(hold_amount_usd(1000, Some(2500)), 10.0, "partial disputes hold the disputed part");
        assert_eq!(hold_amount_usd(1234, None), 12.34);
        assert_eq!(hold_amount_usd(-5, None), 0.0);
    }

    #[test]
    fn closed_statuses_map_to_outcomes() {
        assert_eq!(DisputeOutcome::from_stripe_status("won"), Some(DisputeOutcome::Won));
        assert_eq!(DisputeOutcome::from_stripe_status("lost"), Some(DisputeOutcome::Lost));
        assert_eq!(DisputeOutcome::from_stripe_status("warning_closed"), Some(DisputeOutcome::InquiryClosed));
        assert_eq!(DisputeOutcome::from_stripe_status("under_review"), None);
        assert!(DisputeOutcome::Won.releases_hold());
        assert!(DisputeOutcome::InquiryClosed.releases_hold());
        assert!(!DisputeOutcome::Lost.releases_hold());
    }

    #[test]
    fn disputed_payment_reads_intents_and_charges() {
        let intent = json!({
            "id": "pi_1",
            "amount": 2500,
            "amount_received": 2500,
            "customer": "cus_1",
            "metadata": { "clerk_user_id": "user_1", "kind": "credit_pack", "allternit_org_id": "org_1" },
        });
        assert_eq!(
            DisputedPayment::from_object(&intent),
            Some(DisputedPayment {
                user_id: Some("user_1".to_string()),
                customer_id: Some("cus_1".to_string()),
                charged_cents: Some(2500),
                organization_id: Some("org_1".to_string()),
                kind: Some("credit_pack".to_string()),
            })
        );
        let charge = json!({ "id": "ch_1", "amount": 1000, "amount_captured": 1000, "customer": { "id": "cus_2" }, "metadata": {} });
        let parsed = DisputedPayment::from_object(&charge).unwrap();
        assert_eq!(parsed.user_id, None);
        assert_eq!(parsed.customer_id.as_deref(), Some("cus_2"));
        assert_eq!(parsed.charged_cents, Some(1000));
        assert_eq!(DisputedPayment::from_object(&json!("pi_1")), None, "a bare id is not expanded");
    }
}
