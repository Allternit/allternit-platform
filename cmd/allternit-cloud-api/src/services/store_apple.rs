//! Apple In-App Purchase: the App Store Server API client and the sync that
//! turns an Apple transaction into a plan row, credits and a plan tier
//! (plan step 2; `handoffs/store-purchases-2026-11-17/PLAN.md`, D3, D4, D6,
//! D7, D8, D13, D14).
//!
//! - **Trust root (D3).** The server never trusts what a client or a
//!   notification says. It reads the transaction from the App Store Server API
//!   over TLS, authenticated with an ES256 JWT made from the In-App Purchase
//!   key (`APPLE_IAP_ISSUER_ID`, `APPLE_IAP_KEY_ID`, `APPLE_IAP_PRIVATE_KEY`;
//!   bundle id `APPLE_BUNDLE_ID`, default `com.allternit.mobile`). The signed
//!   JWS values in Apple's answers are only base64-decoded: the TLS response
//!   is what is trusted, so no certificate-chain code sits in the money path.
//!   Production is asked first; Apple's "not found" (4040010) retries the
//!   sandbox host, Apple's documented pattern for TestFlight and App Review.
//! - **Account binding (D4).** A transaction's `appAccountToken` must be the
//!   caller's store account token, else 409 `owned_by_another_account`.
//! - **Sandbox (D8).** A sandbox transaction grants only to users listed in
//!   `STORE_SANDBOX_GRANT_USER_IDS` (comma separated). Anyone else gets
//!   `sandbox_not_granted`: no credits, no plan row, no plan tier.
//! - **Plans (D1, D14).** The subscription row is keyed
//!   `apple:<originalTransactionId>`, so an upgrade or downgrade inside the
//!   subscription group updates the same row. Apple's status maps to: 1
//!   active, 4 (billing grace period) `past_due` (access stays), 3 (billing
//!   retry after grace) `unpaid`, 2 (expired) and 5 (revoked) `canceled`. The
//!   runtime plan tier follows the row; `unpaid` and `canceled` drop to
//!   another open plan's tier or the deployment default and stop the cloud
//!   computer, as Stripe does.
//! - **Credits (D6).** Every paid transaction (a purchase, each renewal, each
//!   pack) is granted once through `store_billing::grant_store_transaction`.
//! - **Refunds (D7).** A revoked transaction (REFUND, REVOKE) takes its
//!   granted credits back once as a negative `store_refund` ledger row (the
//!   balance may go negative, as with Stripe disputes); a refunded plan drops
//!   to the default tier. A reversal (REFUND_REVERSED) gives them back once as
//!   `store_refund_reversed`.
//! - **No scheduled job (D13).** `GET /billing/subscription` re-reads an
//!   Apple plan whose period end has passed, at most once per 10 minutes per
//!   row ([`refresh_due_plans_from_env`]). That also grants a renewal whose
//!   notification was missed.

use async_trait::async_trait;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{self, EcdsaKeyPair};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::{
    error::ApiError,
    routes::{billing_subscriptions, hosted_entitlements::apply_hosted_entitlement},
    services::{
        store_billing::{
            self, StoreConfig, StoreEnvironment, StoreError, StoreItem, StoreProvider,
            StoreSubscriptionUpdate, StoreTransaction,
        },
        SuspendReason,
    },
};

/// The app's bundle id; a transaction for any other app is refused.
pub const BUNDLE_ID_ENV: &str = "APPLE_BUNDLE_ID";
pub const DEFAULT_BUNDLE_ID: &str = "com.allternit.mobile";
/// Comma-separated user ids whose sandbox purchases grant (D8): the App
/// Review demo account and the internal test account.
pub const SANDBOX_GRANT_ENV: &str = "STORE_SANDBOX_GRANT_USER_IDS";
/// `credit_transactions.source` of the credits a refund takes back.
pub const REFUND_SOURCE: &str = "store_refund";
/// `credit_transactions.source` of the credits a reversed refund gives back.
pub const REFUND_REVERSED_SOURCE: &str = "store_refund_reversed";
/// What the apps show for a sandbox purchase that granted nothing.
pub const SANDBOX_MESSAGE: &str = "Test purchase. No credits were added.";

const PRODUCTION_BASE: &str = "https://api.storekit.itunes.apple.com";
const SANDBOX_BASE: &str = "https://api.storekit-sandbox.itunes.apple.com";
const JWT_AUDIENCE: &str = "appstoreconnect-v1";
/// Apple accepts at most 60 minutes.
const JWT_LIFETIME_SECS: i64 = 30 * 60;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Apple's "transaction id not found" (4040010) and "original transaction id
/// not found" (4040005).
const NOT_FOUND_CODES: [i64; 2] = [4_040_010, 4_040_005];
/// The notification types that change a purchase. Everything else (TEST,
/// CONSUMPTION_REQUEST, PRICE_INCREASE, …) is answered 200 and ignored.
const HANDLED_NOTIFICATIONS: [&str; 10] = [
    "SUBSCRIBED",
    "DID_RENEW",
    "DID_CHANGE_RENEWAL_PREF",
    "DID_CHANGE_RENEWAL_STATUS",
    "DID_FAIL_TO_RENEW",
    "GRACE_PERIOD_EXPIRED",
    "EXPIRED",
    "REFUND",
    "REVOKE",
    "REFUND_REVERSED",
];

pub fn bundle_id_from_env() -> String {
    std::env::var(BUNDLE_ID_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BUNDLE_ID.to_string())
}

pub fn sandbox_grant_user_ids_from_env() -> Vec<String> {
    std::env::var(SANDBOX_GRANT_ENV)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// Apple transaction ids are decimal numbers. Anything else never reaches
/// the API (it is part of the URL path).
pub fn valid_transaction_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_digit())
}

/// The fields of Apple's `JWSTransactionDecodedPayload` the server uses.
/// Dates are Unix milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleTransaction {
    pub transaction_id: String,
    #[serde(default)]
    pub original_transaction_id: String,
    #[serde(default)]
    pub bundle_id: String,
    pub product_id: String,
    /// The store account token the app passed at purchase (D4).
    #[serde(default)]
    pub app_account_token: Option<String>,
    /// `Production`, `Sandbox`, `Xcode` or `LocalTesting`.
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub expires_date: Option<i64>,
    /// Set when Apple refunded or revoked the transaction.
    #[serde(default)]
    pub revocation_date: Option<i64>,
}

impl AppleTransaction {
    /// Anything but `Production` is a free test purchase.
    pub fn store_environment(&self) -> StoreEnvironment {
        if self.environment == "Production" {
            StoreEnvironment::Production
        } else {
            StoreEnvironment::Sandbox
        }
    }

    pub fn revoked(&self) -> bool {
        self.revocation_date.is_some()
    }

    /// The plan's first transaction id (the key of its row); a pack is its own original.
    pub fn original_id(&self) -> &str {
        if self.original_transaction_id.is_empty() {
            &self.transaction_id
        } else {
            &self.original_transaction_id
        }
    }
}

/// The fields of Apple's `JWSRenewalInfoDecodedPayload` kept for logs and
/// later use; the plan state comes from the subscription `status`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleRenewalInfo {
    #[serde(default)]
    pub auto_renew_status: Option<i64>,
    #[serde(default)]
    pub auto_renew_product_id: Option<String>,
    #[serde(default)]
    pub grace_period_expires_date: Option<i64>,
    #[serde(default)]
    pub is_in_billing_retry_period: Option<bool>,
}

/// One subscription's state from `GET /inApps/v1/subscriptions/{id}`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppleSubscriptionStatus {
    /// 1 active, 2 expired, 3 billing retry, 4 billing grace period, 5 revoked.
    pub status: i64,
    /// The latest transaction (its product is the plan the person is on now).
    pub transaction: AppleTransaction,
    pub renewal: Option<AppleRenewalInfo>,
}

/// D14: Apple's subscription status as a `billing_subscriptions.status`.
/// `None` for a status Apple may add later (the row is left as it is).
pub fn plan_status(status: i64) -> Option<&'static str> {
    match status {
        1 => Some("active"),
        4 => Some("past_due"),
        3 => Some("unpaid"),
        2 | 5 => Some("canceled"),
        _ => None,
    }
}

#[derive(Debug)]
pub enum AppleApiError {
    /// The secrets are unset or the key does not parse.
    NotConfigured(String),
    /// Network trouble, 5xx, 429 or an auth failure: try again later.
    Unavailable(String),
    /// Apple refused the request or answered something unreadable.
    Rejected(String),
}

impl std::fmt::Display for AppleApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppleApiError::NotConfigured(detail) => write!(f, "not configured: {detail}"),
            AppleApiError::Unavailable(detail) => write!(f, "unavailable: {detail}"),
            AppleApiError::Rejected(detail) => write!(f, "rejected: {detail}"),
        }
    }
}

/// The App Store Server API calls the sync needs. Tests use a fake.
#[async_trait]
pub trait AppleStoreApi: Send + Sync {
    /// `GET /inApps/v1/transactions/{transactionId}`; `None` when neither
    /// production nor sandbox knows it.
    async fn transaction(
        &self,
        transaction_id: &str,
    ) -> Result<Option<AppleTransaction>, AppleApiError>;

    /// `GET /inApps/v1/subscriptions/{originalTransactionId}`.
    async fn subscription(
        &self,
        original_transaction_id: &str,
    ) -> Result<Option<AppleSubscriptionStatus>, AppleApiError>;
}

/// The real App Store Server API client.
pub struct AppleStoreClient {
    issuer_id: String,
    key_id: String,
    bundle_id: String,
    key: EcdsaKeyPair,
}

impl AppleStoreClient {
    pub fn from_env() -> Result<Self, AppleApiError> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let (Some(issuer_id), Some(key_id), Some(pem)) = (
            var("APPLE_IAP_ISSUER_ID"),
            var("APPLE_IAP_KEY_ID"),
            var("APPLE_IAP_PRIVATE_KEY"),
        ) else {
            return Err(AppleApiError::NotConfigured(
                "APPLE_IAP_ISSUER_ID, APPLE_IAP_KEY_ID and APPLE_IAP_PRIVATE_KEY must all be set"
                    .to_string(),
            ));
        };
        Ok(Self {
            issuer_id,
            key_id,
            bundle_id: bundle_id_from_env(),
            key: parse_p8(&pem)?,
        })
    }

    /// The API bearer token (ES256; header `kid`, claims `iss`, `iat`, `exp`,
    /// `aud`, `bid`).
    fn token(&self, now: i64) -> Result<String, AppleApiError> {
        api_token(
            &self.key,
            &self.key_id,
            &self.issuer_id,
            &self.bundle_id,
            now,
        )
    }

    async fn get_once(&self, base: &str, path: &str) -> Result<Option<Value>, AppleApiError> {
        let token = self.token(chrono::Utc::now().timestamp())?;
        let response = crate::net_client::client()
            .get(format!("{base}{path}"))
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                AppleApiError::Unavailable(format!("App Store Server API: {error}"))
            })?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(Some(body));
        }
        let code = body["errorCode"].as_i64();
        if status == reqwest::StatusCode::NOT_FOUND
            && code.map_or(true, |code| NOT_FOUND_CODES.contains(&code))
        {
            return Ok(None);
        }
        let detail = format!(
            "App Store Server API {status} (error {}): {}",
            code.map(|code| code.to_string()).unwrap_or_default(),
            body["errorMessage"].as_str().unwrap_or_default()
        );
        if status.is_server_error()
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::UNAUTHORIZED
        {
            Err(AppleApiError::Unavailable(detail))
        } else {
            Err(AppleApiError::Rejected(detail))
        }
    }

    /// Production first; Apple's "not found" there retries the sandbox host.
    async fn get(&self, path: &str) -> Result<Option<Value>, AppleApiError> {
        match self.get_once(PRODUCTION_BASE, path).await? {
            Some(body) => Ok(Some(body)),
            None => self.get_once(SANDBOX_BASE, path).await,
        }
    }
}

#[async_trait]
impl AppleStoreApi for AppleStoreClient {
    async fn transaction(
        &self,
        transaction_id: &str,
    ) -> Result<Option<AppleTransaction>, AppleApiError> {
        if !valid_transaction_id(transaction_id) {
            return Ok(None);
        }
        let Some(body) = self
            .get(&format!("/inApps/v1/transactions/{transaction_id}"))
            .await?
        else {
            return Ok(None);
        };
        let signed = body["signedTransactionInfo"].as_str().ok_or_else(|| {
            AppleApiError::Rejected("no signedTransactionInfo in the answer".to_string())
        })?;
        decode_jws(signed).map(Some)
    }

    async fn subscription(
        &self,
        original_transaction_id: &str,
    ) -> Result<Option<AppleSubscriptionStatus>, AppleApiError> {
        if !valid_transaction_id(original_transaction_id) {
            return Ok(None);
        }
        let Some(body) = self
            .get(&format!(
                "/inApps/v1/subscriptions/{original_transaction_id}"
            ))
            .await?
        else {
            return Ok(None);
        };
        parse_subscription_status(&body, original_transaction_id)
    }
}

/// The P-256 key from the `.p8` PEM (PKCS#8). Literal `\n` escapes and a PEM
/// squashed onto one line are accepted. Errors never include key material.
fn parse_p8(pem: &str) -> Result<EcdsaKeyPair, AppleApiError> {
    let body: String = pem
        .replace("\\n", "\n")
        .replace("-----BEGIN PRIVATE KEY-----", "")
        .replace("-----END PRIVATE KEY-----", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let der = STANDARD.decode(body.as_bytes()).map_err(|_| {
        AppleApiError::NotConfigured("APPLE_IAP_PRIVATE_KEY is not valid PEM".to_string())
    })?;
    EcdsaKeyPair::from_private_key_der(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &der).map_err(
        |_| AppleApiError::NotConfigured("APPLE_IAP_PRIVATE_KEY is not a P-256 key".to_string()),
    )
}

/// An App Store Server API token.
fn api_token(
    key: &EcdsaKeyPair,
    key_id: &str,
    issuer_id: &str,
    bundle_id: &str,
    now: i64,
) -> Result<String, AppleApiError> {
    let header = serde_json::json!({ "alg": "ES256", "kid": key_id, "typ": "JWT" });
    let claims = serde_json::json!({
        "iss": issuer_id,
        "iat": now,
        "exp": now + JWT_LIFETIME_SECS,
        "aud": JWT_AUDIENCE,
        "bid": bundle_id,
    });
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let sig = key
        .sign(&SystemRandom::new(), input.as_bytes())
        .map_err(|_| AppleApiError::Unavailable("ES256 signing failed".to_string()))?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref())))
}

/// The payload of a JWS from Apple. Payload decode only (D3): the value came
/// from Apple over TLS, or (a notification) only names ids that are re-read.
pub fn decode_jws<T: DeserializeOwned>(jws: &str) -> Result<T, AppleApiError> {
    let (_, claims) = crate::auth::es256::decode_unverified(jws)
        .ok_or_else(|| AppleApiError::Rejected("unreadable JWS".to_string()))?;
    serde_json::from_value(claims)
        .map_err(|error| AppleApiError::Rejected(format!("unexpected JWS payload: {error}")))
}

/// The entry for `original_id` in a subscription status answer (or the only
/// one, when it was asked by a later transaction id).
pub fn parse_subscription_status(
    body: &Value,
    original_id: &str,
) -> Result<Option<AppleSubscriptionStatus>, AppleApiError> {
    let entries: Vec<&Value> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["lastTransactions"].as_array().into_iter().flatten())
        .collect();
    let Some(entry) = entries
        .iter()
        .find(|entry| entry["originalTransactionId"].as_str() == Some(original_id))
        .or_else(|| entries.first())
    else {
        return Ok(None);
    };
    let status = entry["status"]
        .as_i64()
        .ok_or_else(|| AppleApiError::Rejected("subscription status has no status".to_string()))?;
    let signed = entry["signedTransactionInfo"].as_str().ok_or_else(|| {
        AppleApiError::Rejected("subscription status has no signedTransactionInfo".to_string())
    })?;
    let renewal = match entry["signedRenewalInfo"].as_str() {
        Some(signed) => Some(decode_jws::<AppleRenewalInfo>(signed)?),
        None => None,
    };
    Ok(Some(AppleSubscriptionStatus {
        status,
        transaction: decode_jws(signed)?,
        renewal,
    }))
}

/// A cloud computer to stop after a plan went `unpaid` or `canceled`; the
/// route runs it (`ProvisioningService::spawn_suspend_for_subscription`).
#[derive(Debug, Clone, PartialEq)]
pub struct Suspension {
    pub user_id: String,
    pub subscription_key: String,
    pub reason: SuspendReason,
}

/// The `subscription` object of a purchase answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StorePlanView {
    pub plan_id: &'static str,
    pub label: &'static str,
    pub status: String,
    pub billing_provider: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renews_at: Option<String>,
    pub manage_url: Option<String>,
    /// The account has another open plan too (D5).
    pub conflict: bool,
}

/// `{granted, reason?, message?, subscription?, balance_usd?, idempotent_replay}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AppleSyncOutcome {
    /// The transaction's credits are in the account (now or before).
    pub granted: bool,
    /// Why nothing was granted: `sandbox_not_granted`, `refunded`,
    /// `unknown_account`, `account_closed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<StorePlanView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_usd: Option<f64>,
    /// The transaction was granted before; nothing was added this time.
    pub idempotent_replay: bool,
    #[serde(skip)]
    pub suspend: Vec<Suspension>,
}

fn not_granted(reason: &'static str, message: Option<&'static str>) -> AppleSyncOutcome {
    AppleSyncOutcome {
        reason: Some(reason),
        message,
        ..Default::default()
    }
}

/// Why a sync was refused.
#[derive(Debug)]
pub enum AppleSyncError {
    /// 409 `owned_by_another_account` (D4).
    OwnedByAnotherAccount,
    /// The transaction carries no account token and was never recorded for
    /// the caller, so it cannot be bound safely.
    AccountTokenMissing,
    /// The transaction is for another app.
    WrongApp,
    /// A product this server does not sell.
    UnknownProduct(String),
    /// Apple knows no such transaction (production or sandbox).
    NotFound,
    Store(AppleApiError),
    Api(ApiError),
}

impl From<ApiError> for AppleSyncError {
    fn from(error: ApiError) -> Self {
        AppleSyncError::Api(error)
    }
}

impl From<sqlx::Error> for AppleSyncError {
    fn from(error: sqlx::Error) -> Self {
        AppleSyncError::Api(ApiError::DatabaseError(error))
    }
}

impl From<StoreError> for AppleSyncError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::OwnedByAnotherAccount => AppleSyncError::OwnedByAnotherAccount,
            StoreError::Api(error) => AppleSyncError::Api(error),
        }
    }
}

impl std::fmt::Display for AppleSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppleSyncError::OwnedByAnotherAccount => write!(f, "owned by another account"),
            AppleSyncError::AccountTokenMissing => write!(f, "no account token"),
            AppleSyncError::WrongApp => write!(f, "transaction for another app"),
            AppleSyncError::UnknownProduct(product) => write!(f, "unknown product {product}"),
            AppleSyncError::NotFound => write!(f, "transaction not found"),
            AppleSyncError::Store(error) => write!(f, "App Store: {error}"),
            AppleSyncError::Api(error) => write!(f, "{error}"),
        }
    }
}

/// What a notification did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotificationOutcome {
    pub notification_type: String,
    /// False for types that change nothing (TEST, unknown types) and for
    /// payloads that name nothing of ours.
    pub handled: bool,
    pub sync: Option<AppleSyncOutcome>,
}

/// The Apple sync over one database and one App Store Server API client.
pub struct AppleSync<'a> {
    pub db: &'a PgPool,
    pub api: &'a dyn AppleStoreApi,
    pub bundle_id: String,
    pub sandbox_grant_user_ids: Vec<String>,
}

impl<'a> AppleSync<'a> {
    /// Bundle id and sandbox allow-list from the environment.
    pub fn from_env(db: &'a PgPool, api: &'a dyn AppleStoreApi) -> Self {
        Self {
            db,
            api,
            bundle_id: bundle_id_from_env(),
            sandbox_grant_user_ids: sandbox_grant_user_ids_from_env(),
        }
    }

    fn sandbox_allowed(&self, user_id: &str) -> bool {
        self.sandbox_grant_user_ids.iter().any(|id| id == user_id)
    }

    /// `POST /billing/store/apple/transactions`: read the transaction from
    /// Apple and grant it to the signed-in caller.
    pub async fn verify_transaction(
        &self,
        user_id: &str,
        transaction_id: &str,
    ) -> Result<AppleSyncOutcome, AppleSyncError> {
        let tx = self
            .api
            .transaction(transaction_id)
            .await
            .map_err(AppleSyncError::Store)?
            .ok_or(AppleSyncError::NotFound)?;
        self.sync(&tx, Some(user_id)).await
    }

    /// An App Store Server Notification (version 2): decoded for its ids
    /// only, then the transaction is re-read from Apple (D3).
    pub async fn handle_notification(
        &self,
        signed_payload: &str,
    ) -> Result<NotificationOutcome, AppleSyncError> {
        let payload: Value = match decode_jws(signed_payload) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(%error, "apple notification: unreadable signedPayload");
                return Ok(NotificationOutcome::default());
            }
        };
        let mut outcome = NotificationOutcome {
            notification_type: payload["notificationType"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            ..Default::default()
        };
        let data = &payload["data"];
        if let Some(bundle_id) = data["bundleId"].as_str() {
            if bundle_id != self.bundle_id {
                tracing::warn!(bundle_id, "apple notification for another app, ignored");
                return Ok(outcome);
            }
        }
        if !HANDLED_NOTIFICATIONS.contains(&outcome.notification_type.as_str()) {
            tracing::info!(
                notification_type = %outcome.notification_type,
                subtype = payload["subtype"].as_str().unwrap_or_default(),
                "apple notification acknowledged, nothing to do"
            );
            return Ok(outcome);
        }
        let Some(transaction_id) = data["signedTransactionInfo"]
            .as_str()
            .and_then(|signed| decode_jws::<Value>(signed).ok())
            .and_then(|tx| tx["transactionId"].as_str().map(str::to_string))
        else {
            tracing::warn!(notification_type = %outcome.notification_type, "apple notification names no transaction");
            return Ok(outcome);
        };
        let Some(tx) = self
            .api
            .transaction(&transaction_id)
            .await
            .map_err(AppleSyncError::Store)?
        else {
            tracing::warn!(%transaction_id, "apple notification: Apple does not know the transaction");
            return Ok(outcome);
        };
        let sync = self.sync(&tx, None).await?;
        tracing::info!(
            notification_type = %outcome.notification_type,
            subtype = payload["subtype"].as_str().unwrap_or_default(),
            %transaction_id,
            granted = sync.granted,
            reason = sync.reason.unwrap_or_default(),
            "apple notification synced"
        );
        outcome.handled = true;
        outcome.sync = Some(sync);
        Ok(outcome)
    }

    /// D13: re-read the caller's Apple plans whose period end has passed, at
    /// most once per 10 minutes per row. A renewal whose notification was
    /// missed is granted; an expiry is recorded.
    pub async fn refresh_due_plans(
        &self,
        user_id: &str,
    ) -> Result<Vec<Suspension>, AppleSyncError> {
        let due: Vec<String> = sqlx::query_scalar(
            r#"
            UPDATE billing_subscriptions SET store_checked_at = NOW()
            WHERE user_id = $1 AND provider = 'apple'
              AND status IN ('active', 'trialing', 'past_due', 'unpaid')
              AND current_period_end IS NOT NULL AND current_period_end < NOW()
              AND (store_checked_at IS NULL OR store_checked_at < NOW() - INTERVAL '10 minutes')
            RETURNING stripe_subscription_id
            "#,
        )
        .bind(user_id)
        .fetch_all(self.db)
        .await?;
        let mut suspend = Vec::new();
        for key in due {
            let Some(original_id) = key.strip_prefix("apple:") else {
                continue;
            };
            let Some(status) = self
                .api
                .subscription(original_id)
                .await
                .map_err(AppleSyncError::Store)?
            else {
                tracing::warn!(%key, "apple plan refresh: Apple does not know the subscription");
                continue;
            };
            suspend.extend(self.sync(&status.transaction, None).await?.suspend);
        }
        Ok(suspend)
    }

    /// Apply one transaction as Apple reports it: owner (D4), sandbox (D8),
    /// refund or reversal (D7), the plan row and tier (D1, D14), then the
    /// credits (D6). `caller` is the signed-in user on the verify route, or
    /// `None` for a notification or a refresh.
    async fn sync(
        &self,
        tx: &AppleTransaction,
        caller: Option<&str>,
    ) -> Result<AppleSyncOutcome, AppleSyncError> {
        if tx.bundle_id != self.bundle_id {
            return Err(AppleSyncError::WrongApp);
        }
        let item = store_billing::item_for_product(StoreProvider::Apple, &tx.product_id)
            .ok_or_else(|| AppleSyncError::UnknownProduct(tx.product_id.clone()))?;
        let Some(user_id) = self.owner_of(tx, caller).await? else {
            tracing::warn!(
                transaction_id = %tx.transaction_id,
                "apple transaction with no known account, nothing granted"
            );
            return Ok(not_granted("unknown_account", None));
        };
        if account_closed(self.db, &user_id).await? {
            tracing::warn!(%user_id, transaction_id = %tx.transaction_id, "apple transaction for an erased account, nothing granted");
            return Ok(not_granted("account_closed", None));
        }
        let environment = tx.store_environment();
        if environment == StoreEnvironment::Sandbox && !self.sandbox_allowed(&user_id) {
            record_without_grant(self.db, tx, &user_id, item, "sandbox_not_granted").await?;
            return Ok(not_granted("sandbox_not_granted", Some(SANDBOX_MESSAGE)));
        }

        let mut outcome = AppleSyncOutcome::default();
        if tx.revoked() {
            take_back_refund(self.db, tx, &user_id, item).await?;
            outcome.reason = Some("refunded");
        } else {
            give_back_reversed_refund(self.db, &tx.transaction_id).await?;
        }

        // The plan row first, so a plan that belongs to someone else stops
        // here before any credits move.
        if matches!(item, StoreItem::Plan(_)) {
            let refunded = tx.revoked().then_some(tx.transaction_id.as_str());
            if let Some((view, suspension)) =
                self.sync_plan(tx.original_id(), &user_id, refunded).await?
            {
                outcome.subscription = Some(view);
                outcome.suspend.extend(suspension);
            }
        }
        if tx.revoked() {
            return Ok(outcome);
        }

        alert_if_over_first_purchase_cap(self.db, &user_id, tx, item, environment).await;
        let organization_id = match item {
            StoreItem::Pack(_) => store_billing::organization_for_user(self.db, &user_id).await?,
            StoreItem::Plan(_) => None,
        };
        let grant = store_billing::grant_store_transaction(
            self.db,
            &StoreTransaction {
                provider: StoreProvider::Apple,
                transaction_id: &tx.transaction_id,
                original_transaction_id: Some(tx.original_id()),
                purchase_token: None,
                user_id: &user_id,
                item,
                environment,
                organization_id: organization_id.as_deref(),
            },
        )
        .await?;
        outcome.granted = true;
        outcome.balance_usd = grant.balance_usd;
        outcome.idempotent_replay = grant.idempotent_replay;
        Ok(outcome)
    }

    /// Who a transaction belongs to (D4). With a caller: the transaction's
    /// account token must be the caller's; a transaction without one is
    /// accepted only when it was already recorded for the caller. Without a
    /// caller: the token's account, else whoever it was recorded for.
    async fn owner_of(
        &self,
        tx: &AppleTransaction,
        caller: Option<&str>,
    ) -> Result<Option<String>, AppleSyncError> {
        let token = tx
            .app_account_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        // Some(None): a token that is not one of ours.
        let token_owner: Option<Option<String>> = match token {
            None => None,
            Some(raw) => Some(match Uuid::parse_str(raw) {
                Ok(token) => store_billing::account_for_token(self.db, token)
                    .await?
                    .map(|account| account.user_id),
                Err(_) => None,
            }),
        };
        let recorded: Option<String> = sqlx::query_scalar(
            r#"
            SELECT user_id FROM billing_store_transactions
            WHERE provider = 'apple' AND original_transaction_id = $1 AND user_id IS NOT NULL
            UNION ALL
            SELECT user_id FROM billing_subscriptions WHERE stripe_subscription_id = $2
            LIMIT 1
            "#,
        )
        .bind(tx.original_id())
        .bind(store_billing::subscription_key(
            StoreProvider::Apple,
            tx.original_id(),
        ))
        .fetch_optional(self.db)
        .await?;
        match caller {
            Some(caller) => match token_owner {
                Some(Some(owner)) if owner == caller => Ok(Some(owner)),
                Some(_) => Err(AppleSyncError::OwnedByAnotherAccount),
                None => match recorded {
                    Some(owner) if owner == caller => Ok(Some(owner)),
                    Some(_) => Err(AppleSyncError::OwnedByAnotherAccount),
                    None => Err(AppleSyncError::AccountTokenMissing),
                },
            },
            None => Ok(token_owner.flatten().or(recorded)),
        }
    }

    /// Write the plan row from Apple's subscription status and move the plan
    /// tier with it (D14). `refunded_transaction` is a refunded transaction of
    /// this plan: when it is the latest one, the plan is canceled (D7).
    async fn sync_plan(
        &self,
        original_id: &str,
        user_id: &str,
        refunded_transaction: Option<&str>,
    ) -> Result<Option<(StorePlanView, Option<Suspension>)>, AppleSyncError> {
        let Some(status) = self
            .api
            .subscription(original_id)
            .await
            .map_err(AppleSyncError::Store)?
        else {
            tracing::warn!(%original_id, "apple subscription status not found");
            return Ok(None);
        };
        let latest = &status.transaction;
        if latest.bundle_id != self.bundle_id {
            return Err(AppleSyncError::WrongApp);
        }
        let Some(StoreItem::Plan(plan)) =
            store_billing::item_for_product(StoreProvider::Apple, &latest.product_id)
        else {
            return Err(AppleSyncError::UnknownProduct(latest.product_id.clone()));
        };
        let Some(mut mapped) = plan_status(status.status) else {
            tracing::warn!(%original_id, status = status.status, "unknown Apple subscription status; plan row left as it is");
            return Ok(None);
        };
        if latest.revoked() || refunded_transaction == Some(latest.transaction_id.as_str()) {
            mapped = "canceled";
        }

        let key = store_billing::subscription_key(StoreProvider::Apple, original_id);
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT status FROM billing_subscriptions WHERE stripe_subscription_id = $1",
        )
        .bind(&key)
        .fetch_optional(self.db)
        .await?;
        let period_end = latest.expires_date.map(|ms| ms.div_euclid(1000));
        let written = store_billing::upsert_store_subscription(
            self.db,
            &StoreSubscriptionUpdate {
                provider: StoreProvider::Apple,
                store_id: original_id,
                user_id,
                plan,
                status: mapped,
                environment: latest.store_environment(),
                period_end,
            },
        )
        .await?;
        sqlx::query(
            "UPDATE billing_subscriptions SET store_checked_at = NOW() WHERE stripe_subscription_id = $1",
        )
        .bind(&written.key)
        .execute(self.db)
        .await?;

        let open = matches!(mapped, "active" | "past_due");
        let tier = if open {
            plan.plan_tier.to_string()
        } else {
            fallback_tier(self.db, user_id, &written.key).await?
        };
        let current_tier: Option<Option<String>> =
            sqlx::query_scalar("SELECT plan_tier_id FROM user_runtime_quotas WHERE user_id = $1")
                .bind(user_id)
                .fetch_optional(self.db)
                .await?;
        // Only a change is written. Each change gets its own event id, so a
        // tier that comes back (a reversed refund) is applied again.
        if current_tier.flatten().as_deref() != Some(tier.as_str()) {
            let event_id = format!(
                "apple-{}-{}-{}",
                latest.transaction_id,
                tier,
                Uuid::new_v4().simple()
            );
            apply_hosted_entitlement(self.db, &event_id, user_id, &tier, None, "apple").await?;
        }

        let was_open = matches!(
            previous.as_deref(),
            Some("active" | "trialing" | "past_due")
        );
        let suspension = match mapped {
            "unpaid" if was_open => Some(SuspendReason::Unpaid),
            "canceled" if was_open || previous.as_deref() == Some("unpaid") => {
                Some(SuspendReason::Cancelled)
            }
            _ => None,
        }
        .map(|reason| Suspension {
            user_id: user_id.to_string(),
            subscription_key: written.key.clone(),
            reason,
        });

        let view = StorePlanView {
            plan_id: plan.id,
            label: plan.label,
            status: mapped.to_string(),
            billing_provider: StoreProvider::Apple.as_str(),
            renews_at: period_end
                .and_then(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0))
                .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            manage_url: store_billing::manage_url(StoreProvider::Apple.as_str(), plan.id),
            conflict: written.conflict,
        };
        Ok(Some((view, suspension)))
    }
}

/// D13 for `GET /billing/subscription`: refresh the user's due Apple plans
/// when Apple purchases are switched on. Never fails the read; problems are
/// logged. Returns the computers to stop.
pub async fn refresh_due_plans_from_env(db: &PgPool, user_id: &str) -> Vec<Suspension> {
    let due: Result<bool, sqlx::Error> = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM billing_subscriptions
            WHERE user_id = $1 AND provider = 'apple'
              AND status IN ('active', 'trialing', 'past_due', 'unpaid')
              AND current_period_end IS NOT NULL AND current_period_end < NOW()
              AND (store_checked_at IS NULL OR store_checked_at < NOW() - INTERVAL '10 minutes')
        )
        "#,
    )
    .bind(user_id)
    .fetch_one(db)
    .await;
    match due {
        Ok(true) => {}
        Ok(false) => return Vec::new(),
        Err(error) => {
            tracing::warn!(%error, %user_id, "apple plan refresh: due check failed");
            return Vec::new();
        }
    }
    if !StoreConfig::from_env().apple {
        return Vec::new();
    }
    let client = match AppleStoreClient::from_env() {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "apple plan refresh: App Store client not configured");
            return Vec::new();
        }
    };
    match AppleSync::from_env(db, &client)
        .refresh_due_plans(user_id)
        .await
    {
        Ok(suspend) => suspend,
        Err(error) => {
            tracing::warn!(%error, %user_id, "apple plan refresh failed; the next read retries in 10 minutes");
            Vec::new()
        }
    }
}

/// An erased account keeps an inactive tombstone row; a store event must not
/// grant to it or bring it back.
pub(crate) async fn account_closed(db: &PgPool, user_id: &str) -> Result<bool, ApiError> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(status.as_deref() == Some("inactive"))
}

/// The plan tier when this store plan no longer grants one: another open
/// plan's tier, else the deployment default (`DEFAULT_PLAN_TIER`, `free`).
pub(crate) async fn fallback_tier(db: &PgPool, user_id: &str, key: &str) -> Result<String, ApiError> {
    let other: Option<String> = sqlx::query_scalar(
        r#"
        SELECT plan_tier FROM billing_subscriptions
        WHERE user_id = $1 AND stripe_subscription_id <> $2
          AND status IN ('active', 'trialing', 'past_due')
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(key)
    .fetch_optional(db)
    .await?;
    Ok(other.unwrap_or_else(|| {
        std::env::var("DEFAULT_PLAN_TIER").unwrap_or_else(|_| "free".to_string())
    }))
}

/// Record a transaction that grants nothing (a sandbox purchase outside the
/// allow-list), once. A transaction recorded for another account is refused.
async fn record_without_grant(
    db: &PgPool,
    tx: &AppleTransaction,
    user_id: &str,
    item: StoreItem,
    state: &str,
) -> Result<(), AppleSyncError> {
    sqlx::query(
        r#"
        INSERT INTO billing_store_transactions (
            provider, transaction_id, original_transaction_id, user_id,
            product_id, kind, item_id, environment, state, grant_key
        ) VALUES ('apple', $1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (provider, transaction_id) DO NOTHING
        "#,
    )
    .bind(&tx.transaction_id)
    .bind(tx.original_id())
    .bind(user_id)
    .bind(&tx.product_id)
    .bind(item.kind())
    .bind(item.id())
    .bind(tx.store_environment().as_str())
    .bind(state)
    .bind(store_billing::grant_key(
        StoreProvider::Apple,
        &tx.transaction_id,
    ))
    .execute(db)
    .await?;
    let owner: Option<Option<String>> = sqlx::query_scalar(
        "SELECT user_id FROM billing_store_transactions WHERE provider = 'apple' AND transaction_id = $1",
    )
    .bind(&tx.transaction_id)
    .fetch_optional(db)
    .await?;
    match owner.flatten() {
        Some(owner) if owner != user_id => Err(AppleSyncError::OwnedByAnotherAccount),
        _ => Ok(()),
    }
}

/// D7: a refunded or revoked transaction gives back the credits it granted,
/// once: the ledger row goes `granted` → `refunded`, and the negative
/// `store_refund` row is keyed `apple-<transactionId>-refund`. The balance may
/// go negative (the credits may be spent already), as with Stripe disputes.
/// A refund of a transaction never granted is recorded and takes nothing.
async fn take_back_refund(
    db: &PgPool,
    tx: &AppleTransaction,
    user_id: &str,
    item: StoreItem,
) -> Result<(), AppleSyncError> {
    let grant_key = store_billing::grant_key(StoreProvider::Apple, &tx.transaction_id);
    sqlx::query(
        r#"
        INSERT INTO billing_store_transactions (
            provider, transaction_id, original_transaction_id, user_id,
            product_id, kind, item_id, environment, state, grant_key, refunded_at
        ) VALUES ('apple', $1, $2, $3, $4, $5, $6, $7, 'refunded', $8, NOW())
        ON CONFLICT (provider, transaction_id) DO NOTHING
        "#,
    )
    .bind(&tx.transaction_id)
    .bind(tx.original_id())
    .bind(user_id)
    .bind(&tx.product_id)
    .bind(item.kind())
    .bind(item.id())
    .bind(tx.store_environment().as_str())
    .bind(&grant_key)
    .execute(db)
    .await?;

    let mut dbtx = db.begin().await?;
    let refunded: Option<(Option<String>, f64)> = sqlx::query_as(
        r#"
        UPDATE billing_store_transactions
        SET state = 'refunded', refunded_at = NOW(), updated_at = NOW()
        WHERE provider = 'apple' AND transaction_id = $1
          AND state = 'granted' AND refunded_at IS NULL
        RETURNING user_id, credits_granted_usd
        "#,
    )
    .bind(&tx.transaction_id)
    .fetch_optional(&mut *dbtx)
    .await?;
    if let Some((Some(owner), credits)) = refunded {
        if credits > 0.0 {
            move_credits(
                &mut dbtx,
                &owner,
                -credits,
                &format!("{grant_key}-refund"),
                REFUND_SOURCE,
                matches!(item, StoreItem::Pack(_)),
            )
            .await?;
            tracing::warn!(user_id = %owner, %grant_key, credits, "apple refund: credits taken back");
        }
    }
    dbtx.commit().await?;
    Ok(())
}

/// D7: a reversed refund gives the credits back once (`refunded` →
/// `granted`, keyed `apple-<transactionId>-refund-reversed`). A refunded
/// transaction that never granted goes back to `pending`, so the grant that
/// follows adds its credits.
async fn give_back_reversed_refund(
    db: &PgPool,
    transaction_id: &str,
) -> Result<(), AppleSyncError> {
    let mut dbtx = db.begin().await?;
    let reversed: Option<(Option<String>, f64, String)> = sqlx::query_as(
        r#"
        UPDATE billing_store_transactions
        SET state = CASE WHEN credits_granted_usd > 0 THEN 'granted' ELSE 'pending' END,
            refund_reversed_at = NOW(), updated_at = NOW()
        WHERE provider = 'apple' AND transaction_id = $1
          AND state = 'refunded' AND refund_reversed_at IS NULL
        RETURNING user_id, credits_granted_usd, kind
        "#,
    )
    .bind(transaction_id)
    .fetch_optional(&mut *dbtx)
    .await?;
    if let Some((Some(owner), credits, kind)) = reversed {
        if credits > 0.0 {
            let grant_key = store_billing::grant_key(StoreProvider::Apple, transaction_id);
            move_credits(
                &mut dbtx,
                &owner,
                credits,
                &format!("{grant_key}-refund-reversed"),
                REFUND_REVERSED_SOURCE,
                kind == "pack",
            )
            .await?;
            tracing::info!(user_id = %owner, %grant_key, credits, "apple refund reversed: credits given back");
        }
    }
    dbtx.commit().await?;
    Ok(())
}

/// One keyed cloud-wallet ledger row and its balance change, in the caller's
/// database transaction. A key already used changes nothing.
///
/// Callers are store refunds (negative) and reversed refunds (positive),
/// never a plan renewal (renewals go through `grant_plan_credits_keyed`).
/// `bought` = the transaction was a credit pack (`kind = 'pack'`): the change
/// moves `bought_usd` too, in the same statement (contract D2), read clamped
/// per D1 and never below zero, so a pack refund takes back bought credits
/// and its reversal gives them back. A plan transaction's refund or reversal
/// moves only the plan part.
pub(crate) async fn move_credits(
    dbtx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: &str,
    amount_usd: f64,
    ledger_key: &str,
    source: &str,
    bought: bool,
) -> Result<(), sqlx::Error> {
    let ledger = sqlx::query(
        r#"
        INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
        VALUES ($1, $2, $3, $4, $5, NOW())
        ON CONFLICT (transaction_id) DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(amount_usd)
    .bind(ledger_key)
    .bind(source)
    .execute(&mut **dbtx)
    .await?;
    if ledger.rows_affected() > 0 {
        sqlx::query(
            r#"
            INSERT INTO user_credits (user_id, balance_usd, bought_usd)
            VALUES ($1, $2, CASE WHEN $3::boolean THEN GREATEST($2::float8, 0) ELSE 0 END)
            ON CONFLICT (user_id) DO UPDATE SET
                balance_usd = user_credits.balance_usd + excluded.balance_usd,
                bought_usd = CASE WHEN $3::boolean THEN GREATEST(
                        LEAST(user_credits.bought_usd, GREATEST(user_credits.balance_usd, 0))
                            + excluded.balance_usd,
                        0
                    ) ELSE user_credits.bought_usd END,
                updated_at = now()
            "#,
        )
        .bind(user_id)
        .bind(amount_usd)
        .bind(bought)
        .execute(&mut **dbtx)
        .await?;
    }
    Ok(())
}

/// D9: preflight enforces the first-purchase cap, but a paid production
/// transaction is always granted. When one arrives over the cap (a purchase
/// made without preflight), the team is told.
async fn alert_if_over_first_purchase_cap(
    db: &PgPool,
    user_id: &str,
    tx: &AppleTransaction,
    item: StoreItem,
    environment: StoreEnvironment,
) {
    if environment != StoreEnvironment::Production {
        return;
    }
    let trusted = billing_subscriptions::is_trusted_purchaser(db, user_id)
        .await
        .unwrap_or(true);
    if billing_subscriptions::ensure_purchase_allowed(trusted, item.credits_usd(), "").is_err() {
        crate::services::ops_alert::send_once(
            &format!("store-cap-bypass:apple:{}", tx.transaction_id),
            format!("App Store purchase over the first-purchase cap: {user_id}"),
            format!(
                "User {user_id} paid Apple for {} {} (${:.2} of credits, transaction {}) without the first-purchase \
                 trust preflight requires. It was granted (the money is paid); check the account.",
                item.kind(),
                item.id(),
                item.credits_usd(),
                tx.transaction_id
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{KeyPair as _, UnparsedPublicKey};

    fn jws(payload: &Value) -> String {
        format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(r#"{"alg":"ES256"}"#),
            URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }

    #[test]
    fn apple_status_maps_per_d14() {
        assert_eq!(plan_status(1), Some("active"));
        assert_eq!(
            plan_status(4),
            Some("past_due"),
            "grace period keeps access"
        );
        assert_eq!(plan_status(3), Some("unpaid"), "billing retry after grace");
        assert_eq!(plan_status(2), Some("canceled"));
        assert_eq!(plan_status(5), Some("canceled"));
        assert_eq!(plan_status(9), None);
    }

    #[test]
    fn transaction_ids_are_digits_only() {
        assert!(valid_transaction_id("2000000123456789"));
        assert!(!valid_transaction_id(""));
        assert!(!valid_transaction_id("../subscriptions/1"));
        assert!(!valid_transaction_id("12a"));
    }

    /// The API token is ES256 over the documented header and claims, and
    /// verifies with the key's public half.
    #[test]
    fn api_token_is_es256_with_apple_claims() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .unwrap();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\\n{}\\n-----END PRIVATE KEY-----",
            STANDARD.encode(pkcs8.as_ref())
        );
        let key = parse_p8(&pem).expect("escaped newlines are accepted");
        let token = api_token(
            &key,
            "KEY123",
            "issuer-uuid",
            "com.allternit.mobile",
            1_800_000_000,
        )
        .unwrap();
        let (header, claims) = crate::auth::es256::decode_unverified(&token).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "KEY123");
        assert_eq!(claims["iss"], "issuer-uuid");
        assert_eq!(claims["aud"], "appstoreconnect-v1");
        assert_eq!(claims["bid"], "com.allternit.mobile");
        let lifetime = claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap();
        assert!(
            lifetime > 0 && lifetime <= 3600,
            "Apple allows at most 60 minutes"
        );
        let (input, sig) = token.rsplit_once('.').unwrap();
        UnparsedPublicKey::new(
            &signature::ECDSA_P256_SHA256_FIXED,
            key.public_key().as_ref(),
        )
        .verify(input.as_bytes(), &URL_SAFE_NO_PAD.decode(sig).unwrap())
        .expect("signature verifies");
        assert!(parse_p8("-----BEGIN PRIVATE KEY-----").is_err());
    }

    #[test]
    fn subscription_status_picks_the_plan_and_decodes_both_payloads() {
        let body = serde_json::json!({
            "environment": "Sandbox",
            "bundleId": "com.allternit.mobile",
            "data": [{
                "subscriptionGroupIdentifier": "21000000",
                "lastTransactions": [
                    {
                        "originalTransactionId": "100",
                        "status": 4,
                        "signedTransactionInfo": jws(&serde_json::json!({
                            "transactionId": "105",
                            "originalTransactionId": "100",
                            "bundleId": "com.allternit.mobile",
                            "productId": "com.allternit.mobile.plan.max.monthly",
                            "environment": "Sandbox",
                            "expiresDate": 1_800_000_000_000_i64,
                            "type": "Auto-Renewable Subscription"
                        })),
                        "signedRenewalInfo": jws(&serde_json::json!({
                            "autoRenewStatus": 1,
                            "gracePeriodExpiresDate": 1_801_000_000_000_i64,
                            "isInBillingRetryPeriod": true
                        }))
                    }
                ]
            }]
        });
        let status = parse_subscription_status(&body, "100").unwrap().unwrap();
        assert_eq!(status.status, 4);
        assert_eq!(status.transaction.transaction_id, "105");
        assert_eq!(
            status.transaction.store_environment(),
            StoreEnvironment::Sandbox
        );
        assert_eq!(status.transaction.expires_date, Some(1_800_000_000_000));
        assert_eq!(
            status.renewal.unwrap().is_in_billing_retry_period,
            Some(true)
        );
        assert!(
            parse_subscription_status(&serde_json::json!({ "data": [] }), "100")
                .unwrap()
                .is_none()
        );
    }
}
