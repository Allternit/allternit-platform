//! Store purchases (Apple In-App Purchase, Google Play Billing): the shared
//! core the store routes (`routes::billing_store`) build on.
//!
//! Plan: `handoffs/store-purchases-2026-11-17/PLAN.md`, decisions D1–D14.
//!
//! - **Catalog.** Store product ids live next to the web catalog
//!   (`SubscriptionPlan` / `CreditPack`). A store item grants the same credits
//!   as web; only the price differs. [`StoreConfig::offers`] is the `store`
//!   block on `GET /billing/plans` and `/packs`; a store is `enabled` only when
//!   its secrets are set (D10), so merging this changes nothing for users.
//! - **Account binding (D4).** [`issue_account_token`] gives each user one
//!   UUID (`billing_store_accounts`). iOS passes it as `appAccountToken`; the
//!   server refuses a transaction whose token maps to someone else.
//! - **Plans (D1).** A store plan is a `billing_subscriptions` row keyed
//!   `apple:<originalTransactionId>` or `google:<sha256 of purchase token>`
//!   with `provider` set, so every existing reader of that table (cloud
//!   computer, photo avatars, Freeweights, usage, setup checks) works as is.
//!   [`upsert_store_subscription`] never moves a row between users and flags a
//!   second open plan as a conflict (D5).
//! - **Credits (D2, D6).** [`grant_store_transaction`] records the
//!   transaction in `billing_store_transactions` (the idempotency key and the
//!   refund reference), then grants: plan credits through the rollover-capped
//!   plan grant, packs through the same path as Stripe packs (fabric ledger
//!   rule included), both keyed `apple-<transactionId>` / `google-<orderId>`.
//!   Sandbox gating (D8) is the caller's decision: call it only for a
//!   transaction that should grant.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    error::ApiError,
    routes::{
        billing_checkout::{self, CreditPack},
        billing_subscriptions::{self, SubscriptionPlan},
        billing_webhooks::{process_credit_purchase_from, CreditGrantTarget, CreditSource},
    },
    services::CostServiceImpl,
};

/// Secrets that turn on App Store purchases (all three must be non-empty).
pub const APPLE_ENV: [&str; 3] = [
    "APPLE_IAP_ISSUER_ID",
    "APPLE_IAP_KEY_ID",
    "APPLE_IAP_PRIVATE_KEY",
];
/// Secrets that turn on Google Play purchases (both must be non-empty).
pub const GOOGLE_ENV: [&str; 2] = ["GOOGLE_PLAY_SERVICE_ACCOUNT_JSON", "GOOGLE_PLAY_RTDN_TOKEN"];
/// Overrides the Play package name in the manage link.
pub const PLAY_PACKAGE_ENV: &str = "GOOGLE_PLAY_PACKAGE_NAME";
/// The Play app's package (`surfaces/allternit-mobile/android`, play flavor).
const DEFAULT_PLAY_PACKAGE: &str = "com.allternit.app";
/// Apple's page for managing subscriptions (opens Settings on iPhone).
const APPLE_MANAGE_URL: &str = "https://apps.apple.com/account/subscriptions";
/// The web billing page that opens the Stripe portal.
const STRIPE_MANAGE_URL: &str = "https://platform.allternit.com/billing";

/// Which store took the money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreProvider {
    Apple,
    Google,
}

impl StoreProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreProvider::Apple => "apple",
            StoreProvider::Google => "google",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "apple" => Some(StoreProvider::Apple),
            "google" => Some(StoreProvider::Google),
            _ => None,
        }
    }

    fn credit_source(self) -> CreditSource {
        match self {
            StoreProvider::Apple => CreditSource::Apple,
            StoreProvider::Google => CreditSource::Google,
        }
    }

    /// `credit_transactions.source` for a store plan's monthly grant.
    fn plan_grant_source(self) -> &'static str {
        match self {
            StoreProvider::Apple => "apple_subscription",
            StoreProvider::Google => "google_subscription",
        }
    }
}

/// Production or sandbox (App Review, TestFlight, Play license testers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreEnvironment {
    Production,
    Sandbox,
}

impl StoreEnvironment {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreEnvironment::Production => "production",
            StoreEnvironment::Sandbox => "sandbox",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "production" => Some(StoreEnvironment::Production),
            "sandbox" => Some(StoreEnvironment::Sandbox),
            _ => None,
        }
    }
}

/// Which stores are switched on (D10): a store is on only when every one of
/// its secrets is set and non-blank.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreConfig {
    pub apple: bool,
    pub google: bool,
}

impl StoreConfig {
    pub fn from_env() -> Self {
        let set = |name: &str| {
            std::env::var(name)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        };
        Self {
            apple: APPLE_ENV.iter().all(|name| set(name)),
            google: GOOGLE_ENV.iter().all(|name| set(name)),
        }
    }

    pub fn enabled(&self, provider: StoreProvider) -> bool {
        match provider {
            StoreProvider::Apple => self.apple,
            StoreProvider::Google => self.google,
        }
    }

    /// The `store` block of one plan or pack in the catalog responses.
    pub fn offers(
        &self,
        apple_product_id: &'static str,
        google_product_id: &'static str,
        reference_price_usd: f64,
    ) -> StoreOffers {
        StoreOffers {
            reference_price_usd,
            apple: StoreOffer {
                enabled: self.apple,
                product_id: apple_product_id,
            },
            google: StoreOffer {
                enabled: self.google,
                product_id: google_product_id,
            },
        }
    }
}

/// `store: { reference_price_usd, apple: { enabled, product_id }, google: { … } }`
/// on each item of `GET /billing/plans` and `GET /billing/packs`.
#[derive(Debug, Clone, Serialize)]
pub struct StoreOffers {
    /// The US store price, for reference only. Clients show the store's own
    /// localized price (D11).
    pub reference_price_usd: f64,
    pub apple: StoreOffer,
    pub google: StoreOffer,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreOffer {
    /// True only when the server can verify this store's purchases. Clients
    /// show a store button only when this is true and the store returns the
    /// product.
    pub enabled: bool,
    pub product_id: &'static str,
}

/// A plan or pack a store sells.
#[derive(Debug, Clone, Copy)]
pub enum StoreItem {
    Plan(&'static SubscriptionPlan),
    Pack(&'static CreditPack),
}

impl StoreItem {
    pub fn kind(&self) -> &'static str {
        match self {
            StoreItem::Plan(_) => "plan",
            StoreItem::Pack(_) => "pack",
        }
    }

    /// The server catalog id (`plus`, `super`, `ultra`, `credits_25`, …).
    pub fn id(&self) -> &'static str {
        match self {
            StoreItem::Plan(plan) => plan.id,
            StoreItem::Pack(pack) => pack.id,
        }
    }

    /// Credits one purchase (or one plan month) grants: the same as web.
    pub fn credits_usd(&self) -> f64 {
        match self {
            StoreItem::Plan(plan) => plan.monthly_credits_usd,
            StoreItem::Pack(pack) => pack.credits_usd,
        }
    }

    pub fn product_id(&self, provider: StoreProvider) -> &'static str {
        match (self, provider) {
            (StoreItem::Plan(plan), StoreProvider::Apple) => plan.apple_product_id,
            (StoreItem::Plan(plan), StoreProvider::Google) => plan.google_product_id,
            (StoreItem::Pack(pack), StoreProvider::Apple) => pack.apple_product_id,
            (StoreItem::Pack(pack), StoreProvider::Google) => pack.google_product_id,
        }
    }
}

/// A catalog item by kind (`plan` / `pack`) and id. The id may be the server
/// catalog id or the store's product id for `provider`.
pub fn find_item(kind: &str, id: &str, provider: StoreProvider) -> Option<StoreItem> {
    let item = match kind {
        "plan" => billing_subscriptions::find_plan(id).map(StoreItem::Plan),
        "pack" => billing_checkout::find_pack(id).map(StoreItem::Pack),
        _ => return None,
    };
    item.or_else(|| item_for_product(provider, id).filter(|item| item.kind() == kind))
}

/// The catalog item a store product id sells, if any.
pub fn item_for_product(provider: StoreProvider, product_id: &str) -> Option<StoreItem> {
    billing_subscriptions::subscription_plans()
        .iter()
        .map(StoreItem::Plan)
        .chain(billing_checkout::credit_packs().iter().map(StoreItem::Pack))
        .find(|item| item.product_id(provider) == product_id)
}

/// Where the person manages or cancels a plan billed by `provider`. `None`
/// for an unknown provider.
pub fn manage_url(provider: &str, plan_id: &str) -> Option<String> {
    match provider {
        "stripe" => Some(STRIPE_MANAGE_URL.to_string()),
        "apple" => Some(APPLE_MANAGE_URL.to_string()),
        "google" => {
            let package = std::env::var(PLAY_PACKAGE_ENV)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| DEFAULT_PLAY_PACKAGE.to_string());
            Some(match billing_subscriptions::find_plan(plan_id) {
                Some(plan) => format!(
                    "https://play.google.com/store/account/subscriptions?sku={}&package={package}",
                    plan.google_product_id
                ),
                None => "https://play.google.com/store/account/subscriptions".to_string(),
            })
        }
        _ => None,
    }
}

/// Lowercase hex SHA-256. Play purchase tokens are long and secret-ish, so
/// only their hash is stored or used as a key.
pub fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// The `billing_subscriptions.stripe_subscription_id` key of a store plan
/// (D1): `apple:<originalTransactionId>` or `google:<sha256 of purchase token>`.
pub fn subscription_key(provider: StoreProvider, store_id: &str) -> String {
    match provider {
        StoreProvider::Apple => format!("apple:{store_id}"),
        StoreProvider::Google => format!("google:{}", sha256_hex(store_id)),
    }
}

/// Why a store write was refused.
#[derive(Debug)]
pub enum StoreError {
    /// The transaction, token or plan already belongs to another account.
    /// Purchases never move between accounts (D4): 409 `owned_by_another_account`.
    OwnedByAnotherAccount,
    Api(ApiError),
}

impl From<ApiError> for StoreError {
    fn from(error: ApiError) -> Self {
        StoreError::Api(error)
    }
}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        StoreError::Api(ApiError::DatabaseError(error))
    }
}

impl IntoResponse for StoreError {
    fn into_response(self) -> Response {
        match self {
            StoreError::OwnedByAnotherAccount => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "owned_by_another_account",
                    "message": "This purchase belongs to a different Allternit account. Sign in to that account to use it.",
                })),
            )
                .into_response(),
            StoreError::Api(error) => error.into_response(),
        }
    }
}

/// The account a store account token belongs to.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct StoreAccount {
    pub user_id: String,
    /// The Clerk organization captured at the latest preflight: where pack
    /// credits go when the fabric ledger bridge is on (D6).
    pub organization_id: Option<String>,
}

/// The user's store account token (D4), created on first use and the same on
/// every later call. A newer organization replaces the stored one; a missing
/// one keeps it.
pub async fn issue_account_token(
    db: &PgPool,
    user_id: &str,
    organization_id: Option<&str>,
) -> Result<Uuid, ApiError> {
    let token: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO billing_store_accounts (user_id, account_token, organization_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id) DO UPDATE SET
            organization_id = COALESCE(excluded.organization_id, billing_store_accounts.organization_id),
            updated_at = NOW()
        RETURNING account_token
        "#,
    )
    .bind(user_id)
    .bind(Uuid::new_v4())
    .bind(organization_id)
    .fetch_one(db)
    .await?;
    Ok(token)
}

/// Who a store account token (Apple `appAccountToken`) belongs to.
pub async fn account_for_token(db: &PgPool, token: Uuid) -> Result<Option<StoreAccount>, ApiError> {
    Ok(sqlx::query_as::<_, StoreAccount>(
        "SELECT user_id, organization_id FROM billing_store_accounts WHERE account_token = $1",
    )
    .bind(token)
    .fetch_optional(db)
    .await?)
}

/// The organization captured for a user at preflight, if any.
pub async fn organization_for_user(db: &PgPool, user_id: &str) -> Result<Option<String>, ApiError> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT organization_id FROM billing_store_accounts WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .flatten())
}

/// One store plan's current state, as read from the store.
#[derive(Debug, Clone)]
pub struct StoreSubscriptionUpdate<'a> {
    pub provider: StoreProvider,
    /// Apple `originalTransactionId`, or the Play purchase token (hashed into the key).
    pub store_id: &'a str,
    pub user_id: &'a str,
    pub plan: &'static SubscriptionPlan,
    /// Mapped per D14: `active`, `past_due` (grace), `unpaid` (grace over /
    /// on hold) or `canceled` (expired).
    pub status: &'a str,
    pub environment: StoreEnvironment,
    /// Unix seconds of the period end, when the store reported one.
    pub period_end: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreSubscriptionOutcome {
    /// The `billing_subscriptions` key of the row.
    pub key: String,
    /// The account has another open plan besides this one (D5).
    pub conflict: bool,
}

/// Write a store plan into `billing_subscriptions` (D1). The row keeps its
/// user: a store plan that already belongs to another account is refused,
/// never moved. A plan change inside the store (Apple subscription group, Play
/// replacement) updates the same row (D14). When the plan is open and the
/// account has another open plan, the team is alerted and the outcome says
/// `conflict` (D5); both plans stay recorded.
pub async fn upsert_store_subscription(
    db: &PgPool,
    update: &StoreSubscriptionUpdate<'_>,
) -> Result<StoreSubscriptionOutcome, StoreError> {
    let key = subscription_key(update.provider, update.store_id);
    let written: Option<String> = sqlx::query_scalar(
        r#"
        INSERT INTO billing_subscriptions (
            stripe_subscription_id, user_id, plan_id, plan_tier, status, provider,
            store_product_id, store_environment, current_period_end, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, to_timestamp($9::BIGINT), CURRENT_TIMESTAMP)
        ON CONFLICT (stripe_subscription_id) DO UPDATE SET
            plan_id = excluded.plan_id,
            plan_tier = excluded.plan_tier,
            status = excluded.status,
            store_product_id = excluded.store_product_id,
            store_environment = excluded.store_environment,
            current_period_end = COALESCE(excluded.current_period_end, billing_subscriptions.current_period_end),
            updated_at = CURRENT_TIMESTAMP
        WHERE billing_subscriptions.user_id = excluded.user_id
        RETURNING stripe_subscription_id
        "#,
    )
    .bind(&key)
    .bind(update.user_id)
    .bind(update.plan.id)
    .bind(update.plan.plan_tier)
    .bind(update.status)
    .bind(update.provider.as_str())
    .bind(StoreItem::Plan(update.plan).product_id(update.provider))
    .bind(update.environment.as_str())
    .bind(update.period_end)
    .fetch_optional(db)
    .await?;
    if written.is_none() {
        return Err(StoreError::OwnedByAnotherAccount);
    }

    let open = matches!(update.status, "active" | "trialing" | "past_due" | "unpaid");
    let others: Vec<(String, String)> = if open {
        sqlx::query_as(
            r#"
            SELECT stripe_subscription_id, COALESCE(provider, 'stripe')
            FROM billing_subscriptions
            WHERE user_id = $1 AND stripe_subscription_id <> $2
              AND status IN ('active', 'trialing', 'past_due', 'unpaid')
            "#,
        )
        .bind(update.user_id)
        .bind(&key)
        .fetch_all(db)
        .await?
    } else {
        Vec::new()
    };
    let conflict = !others.is_empty();
    if conflict {
        let other_list = others
            .iter()
            .map(|(id, provider)| format!("{provider} {id}"))
            .collect::<Vec<_>>()
            .join(", ");
        tracing::error!(
            user_id = update.user_id,
            store_key = %key,
            others = %other_list,
            "BILLING CONFLICT: account has two open plans"
        );
        crate::services::ops_alert::send_once(
            &format!("store-billing-conflict:{}:{key}", update.user_id),
            format!("Billing conflict: {} has two open plans", update.user_id),
            format!(
                "User {} has a {} plan ({}, {}) and also: {}.\nBoth are recorded and paid credits were granted. \
                 The apps ask the person to cancel one; refund if they ask.",
                update.user_id,
                update.provider.as_str(),
                update.plan.id,
                key,
                other_list
            ),
        );
    }
    Ok(StoreSubscriptionOutcome { key, conflict })
}

/// One store transaction to grant (a plan purchase or renewal, or a pack).
#[derive(Debug, Clone)]
pub struct StoreTransaction<'a> {
    pub provider: StoreProvider,
    /// Apple `transactionId`, or the Play `orderId`.
    pub transaction_id: &'a str,
    /// Apple `originalTransactionId` (a plan's first transaction).
    pub original_transaction_id: Option<&'a str>,
    /// The Play purchase token; only its hash is stored.
    pub purchase_token: Option<&'a str>,
    pub user_id: &'a str,
    pub item: StoreItem,
    pub environment: StoreEnvironment,
    /// Fabric ledger target for packs (captured at preflight).
    pub organization_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreGrantOutcome {
    pub credits_usd: f64,
    /// Cloud-wallet balance after the grant; `None` for a fabric grant or a replay.
    pub balance_usd: Option<f64>,
    /// `cloud_wallet` or `fabric_ledger`.
    pub target: String,
    /// The transaction was already granted; nothing was added this time.
    pub idempotent_replay: bool,
}

/// The wallet / fabric key a store transaction grants under (D6).
pub fn grant_key(provider: StoreProvider, transaction_id: &str) -> String {
    format!("{}-{transaction_id}", provider.as_str())
}

/// Record one store transaction in the ledger (D2) and grant its credits
/// (D6), once. A transaction recorded for another account is refused. A
/// transaction already granted returns a replay. A row left `pending` by a
/// crash is granted again safely: every grant underneath is keyed.
///
/// Callers decide whether a transaction should grant at all (sandbox, D8)
/// before calling this.
pub async fn grant_store_transaction(
    db: &PgPool,
    tx: &StoreTransaction<'_>,
) -> Result<StoreGrantOutcome, StoreError> {
    let key = grant_key(tx.provider, tx.transaction_id);
    let token_hash = tx.purchase_token.map(sha256_hex);
    sqlx::query(
        r#"
        INSERT INTO billing_store_transactions (
            provider, transaction_id, original_transaction_id, purchase_token_sha256, user_id,
            product_id, kind, item_id, environment, state, grant_key
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending', $10)
        ON CONFLICT (provider, transaction_id) DO NOTHING
        "#,
    )
    .bind(tx.provider.as_str())
    .bind(tx.transaction_id)
    .bind(tx.original_transaction_id)
    .bind(token_hash.as_deref())
    .bind(tx.user_id)
    .bind(tx.item.product_id(tx.provider))
    .bind(tx.item.kind())
    .bind(tx.item.id())
    .bind(tx.environment.as_str())
    .bind(&key)
    .execute(db)
    .await?;

    let (owner, state, granted): (Option<String>, String, f64) = sqlx::query_as(
        "SELECT user_id, state, credits_granted_usd FROM billing_store_transactions WHERE provider = $1 AND transaction_id = $2",
    )
    .bind(tx.provider.as_str())
    .bind(tx.transaction_id)
    .fetch_one(db)
    .await?;
    match owner.as_deref() {
        Some(owner) if owner != tx.user_id => return Err(StoreError::OwnedByAnotherAccount),
        Some(_) => {}
        // An unclaimed row (a Play notification seen before the app posted
        // it, D4): this authenticated caller claims it.
        None => {
            let claimed = sqlx::query(
                "UPDATE billing_store_transactions SET user_id = $3, updated_at = NOW() \
                 WHERE provider = $1 AND transaction_id = $2 AND user_id IS NULL",
            )
            .bind(tx.provider.as_str())
            .bind(tx.transaction_id)
            .bind(tx.user_id)
            .execute(db)
            .await?;
            if claimed.rows_affected() == 0 {
                return Err(StoreError::OwnedByAnotherAccount);
            }
        }
    }
    if state == "granted" {
        return Ok(StoreGrantOutcome {
            credits_usd: granted,
            balance_usd: None,
            target: String::new(),
            idempotent_replay: true,
        });
    }

    let credits = tx.item.credits_usd();
    let (target, balance_usd) = match tx.item {
        StoreItem::Plan(plan) => {
            let balance = CostServiceImpl::new(db.clone())
                .grant_plan_credits_keyed(
                    tx.user_id,
                    &key,
                    tx.provider.plan_grant_source(),
                    plan.monthly_credits_usd,
                    plan.rollover_cap_usd,
                )
                .await?;
            ("cloud_wallet".to_string(), Some(balance))
        }
        StoreItem::Pack(pack) => {
            if crate::services::fabric_ledger::misconfigured() {
                tracing::error!(
                    "REVENUE-CRITICAL: ALLTERNIT_FABRIC_LEDGER_URL is set but ALLTERNIT_INTERNAL_SERVICE_TOKEN is missing; refusing to grant store transaction {key}"
                );
                return Err(ApiError::ServiceUnavailable(
                    "Fabric ledger bridge is misconfigured (missing internal service token)."
                        .to_string(),
                )
                .into());
            }
            let bridge = crate::services::fabric_ledger::from_env();
            let target = match (&bridge, tx.organization_id) {
                (Some(ledger), Some(org)) => CreditGrantTarget::Fabric(ledger, org.to_string()),
                _ => CreditGrantTarget::CloudWallet,
            };
            let outcome = process_credit_purchase_from(
                db,
                target,
                tx.provider.credit_source(),
                &format!("{}.pack", tx.provider.as_str()),
                tx.transaction_id,
                tx.user_id,
                pack.credits_usd,
            )
            .await?;
            (outcome.target, outcome.balance_usd)
        }
    };

    let marked = sqlx::query(
        r#"
        UPDATE billing_store_transactions
        SET state = 'granted', credits_granted_usd = $3, granted_at = NOW(), updated_at = NOW()
        WHERE provider = $1 AND transaction_id = $2 AND state <> 'granted'
        "#,
    )
    .bind(tx.provider.as_str())
    .bind(tx.transaction_id)
    .bind(credits)
    .execute(db)
    .await?;
    // A concurrent call marked it first: the keyed grants above made this one
    // a no-op, so report it as the replay it was.
    let fresh = marked.rows_affected() > 0;

    if fresh {
        crate::services::audit::write_audit_log(
            db,
            crate::services::audit::AuditEvent {
                action: "billing.store.granted".to_string(),
                resource_type: "credit_transaction".to_string(),
                resource_id: Some(key.clone()),
                user_id: Some(tx.user_id.to_string()),
                user_email: None,
                details: Some(serde_json::json!({
                    "provider": tx.provider.as_str(),
                    "kind": tx.item.kind(),
                    "itemId": tx.item.id(),
                    "environment": tx.environment.as_str(),
                    "creditsUsd": credits,
                    "balanceUsd": balance_usd,
                    "target": target,
                })),
                success: true,
            },
        )
        .await;
        // Only real money builds purchase trust; sandbox purchases are free.
        if tx.environment == StoreEnvironment::Production {
            if let Err(error) = billing_subscriptions::record_paid_purchase(db, tx.user_id).await {
                tracing::error!(
                    "REVENUE-CRITICAL: failed to record paid purchase for user {} (store {}): {}",
                    tx.user_id,
                    key,
                    error
                );
            }
        }
    }
    Ok(StoreGrantOutcome {
        credits_usd: credits,
        balance_usd,
        target,
        idempotent_replay: !fresh,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_ids_map_to_the_web_catalog() {
        let max = find_item(
            "plan",
            "com.allternit.mobile.plan.max.monthly",
            StoreProvider::Apple,
        )
        .unwrap();
        assert_eq!(
            (max.kind(), max.id(), max.credits_usd()),
            ("plan", "super", 110.0),
            "Max is server plan `super`"
        );
        let max_play = find_item("plan", "plan_max", StoreProvider::Google).unwrap();
        assert_eq!(max_play.id(), "super");
        let pack = find_item("pack", "credits_25", StoreProvider::Apple).unwrap();
        assert_eq!(
            pack.product_id(StoreProvider::Apple),
            "com.allternit.mobile.credits.25"
        );
        assert_eq!(pack.credits_usd(), 25.0, "same credits as web");
        // A product id under the wrong kind or store is not a match.
        assert!(find_item(
            "pack",
            "com.allternit.mobile.plan.plus.monthly",
            StoreProvider::Apple
        )
        .is_none());
        assert!(find_item(
            "plan",
            "com.allternit.mobile.plan.plus.monthly",
            StoreProvider::Google
        )
        .is_none());
        assert!(find_item("bundle", "plus", StoreProvider::Apple).is_none());
    }

    #[test]
    fn config_needs_every_secret_of_a_store() {
        let _guard = crate::routes::billing_store::STORE_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for name in APPLE_ENV.iter().chain(GOOGLE_ENV.iter()) {
            std::env::remove_var(name);
        }
        assert_eq!(
            StoreConfig::from_env(),
            StoreConfig {
                apple: false,
                google: false
            }
        );
        std::env::set_var("APPLE_IAP_ISSUER_ID", "issuer");
        std::env::set_var("APPLE_IAP_KEY_ID", "key");
        std::env::set_var("APPLE_IAP_PRIVATE_KEY", "  ");
        assert!(
            !StoreConfig::from_env().apple,
            "a blank secret counts as unset"
        );
        std::env::set_var("APPLE_IAP_PRIVATE_KEY", "-----BEGIN PRIVATE KEY-----");
        assert_eq!(
            StoreConfig::from_env(),
            StoreConfig {
                apple: true,
                google: false
            }
        );
        for name in APPLE_ENV {
            std::env::remove_var(name);
        }
    }

    #[test]
    fn manage_urls_name_the_store_that_bills() {
        assert_eq!(
            manage_url("apple", "plus").as_deref(),
            Some("https://apps.apple.com/account/subscriptions")
        );
        assert_eq!(
            manage_url("google", "super").as_deref(),
            Some("https://play.google.com/store/account/subscriptions?sku=plan_max&package=com.allternit.app")
        );
        assert_eq!(
            manage_url("stripe", "plus").as_deref(),
            Some("https://platform.allternit.com/billing")
        );
        assert_eq!(manage_url("other", "plus"), None);
    }

    #[test]
    fn subscription_keys_follow_d1() {
        assert_eq!(
            subscription_key(StoreProvider::Apple, "2000000123"),
            "apple:2000000123"
        );
        let google = subscription_key(StoreProvider::Google, "token-abc");
        assert_eq!(google, format!("google:{}", sha256_hex("token-abc")));
        assert!(
            !google.contains("token-abc"),
            "the Play token itself is never the key"
        );
    }
}
