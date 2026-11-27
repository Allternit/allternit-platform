//! Google Play Billing: the Play Developer API client and the sync that turns
//! a Play purchase into a plan row, credits and a plan tier (plan step 3;
//! `handoffs/store-purchases-2026-11-17/PLAN.md`, D3, D4, D6, D7, D8, D13,
//! D14).
//!
//! - **Trust root (D3).** The server reads every purchase from the Play
//!   Developer API (`purchases.subscriptionsv2.get`, `purchases.products.get`)
//!   over TLS, authenticated with an OAuth access token from the service
//!   account in `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON` (JWT bearer grant, RS256,
//!   scope `androidpublisher`, cached until it expires). The package is
//!   `GOOGLE_PLAY_PACKAGE_NAME` (default `com.allternit.app`). A Real-time
//!   Developer Notification (RTDN) is only a prompt to re-read.
//! - **Account binding (D4).** The Play app is a Trusted Web Activity and cannot
//!   set `obfuscatedAccountId`, so the first signed-in
//!   `POST …/google/purchases` claims a purchase token. A token already
//!   claimed by another account is refused (409 `owned_by_another_account`).
//!   An RTDN for a token nobody has claimed is stored as `unclaimed`; the PWA
//!   posts its purchases on launch and claims it then. Google refunds a
//!   purchase nobody acknowledged within 3 days.
//! - **Sandbox (D8).** A license-tester purchase (`testPurchase` on a
//!   subscription, `purchaseType` 0 on a product) grants only to users in
//!   `STORE_SANDBOX_GRANT_USER_IDS`.
//! - **Plans (D1, D14).** The row is keyed `google:<sha256 of purchase token>`;
//!   `store_purchase_token` keeps the token for the D13 re-read. Play states
//!   map to: ACTIVE `active`, CANCELED `active` until the paid period ends
//!   then `canceled`, IN_GRACE_PERIOD `past_due`, ON_HOLD and PAUSED `unpaid`,
//!   EXPIRED `canceled`. A replacement purchase (`linkedPurchaseToken`, an
//!   upgrade or downgrade) cancels the old token's row, without stopping the
//!   computer, and opens the new one.
//! - **Credits (D6).** Each paid order (`latestSuccessfulOrderId`: the first
//!   purchase and every renewal; the pack's `orderId`) is granted once,
//!   keyed `google-<orderId>`. The subscription is acknowledged and the pack
//!   consumed only after the grant committed; a failed acknowledge or consume
//!   is logged and retried the next time the purchase is posted or notified.
//! - **Refunds (D7).** A voided purchase takes the order's credits back once
//!   (`store_refund`, keyed `google-<orderId>-refund`) and a voided plan order
//!   that is the plan's latest drops the plan to the default tier.
//! - **No scheduled job (D13).** `GET /billing/subscription` re-reads a Google
//!   plan whose period end has passed, at most once per 10 minutes per row.

use async_trait::async_trait;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{self, RsaKeyPair};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::{
    error::ApiError,
    routes::{billing_subscriptions, hosted_entitlements::apply_hosted_entitlement},
    services::{
        store_apple::{
            self, account_closed, fallback_tier, move_credits, AppleSyncOutcome, StorePlanView,
            Suspension, REFUND_SOURCE, SANDBOX_MESSAGE,
        },
        store_billing::{
            self, StoreConfig, StoreEnvironment, StoreError, StoreItem, StoreProvider,
            StoreSubscriptionUpdate, StoreTransaction,
        },
        SuspendReason,
    },
};

/// The service account key file (JSON) with financial-data access in Play.
pub const SERVICE_ACCOUNT_ENV: &str = "GOOGLE_PLAY_SERVICE_ACCOUNT_JSON";
/// The shared secret on the Pub/Sub push URL (`…/google/rtdn?token=`).
pub const RTDN_TOKEN_ENV: &str = "GOOGLE_PLAY_RTDN_TOKEN";
pub const DEFAULT_PACKAGE: &str = "com.allternit.app";
/// The only base plan each Play subscription sells.
pub const BASE_PLAN_ID: &str = "monthly";
/// What the apps show while Google Play is still collecting the payment.
pub const PENDING_MESSAGE: &str =
    "Payment pending. Your purchase is added when Google Play finishes the payment.";

const API_BASE: &str = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/androidpublisher";
/// Google accepts at most one hour.
const ASSERTION_LIFETIME_SECS: i64 = 3600;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// A cached access token is replaced this long before it expires.
const TOKEN_EARLY_REFRESH_SECS: i64 = 60;

/// The process-wide access token cache (one service account).
static TOKEN_CACHE: std::sync::Mutex<Option<CachedToken>> = std::sync::Mutex::new(None);

#[derive(Clone)]
struct CachedToken {
    client_email: String,
    access_token: String,
    expires_at: i64,
}

pub fn package_name_from_env() -> String {
    std::env::var(store_billing::PLAY_PACKAGE_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_PACKAGE.to_string())
}

/// Play purchase tokens are long URL-safe strings. Anything else never
/// reaches the API (it is part of the URL path).
pub fn valid_purchase_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Constant-time check of the RTDN push secret: both sides are hashed first,
/// so neither the content nor the length of the secret leaks through timing.
/// An unset or short secret never matches.
pub fn rtdn_token_matches(expected: &str, provided: &str) -> bool {
    let expected = expected.trim();
    if expected.len() < 16 {
        return false;
    }
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(provided.as_bytes());
    aws_lc_rs::constant_time::verify_slices_are_equal(&a, &b).is_ok()
}

/// One line item of a `SubscriptionPurchaseV2`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoogleLineItem {
    pub product_id: String,
    /// Unix seconds.
    pub expiry_secs: Option<i64>,
    pub latest_successful_order_id: Option<String>,
    pub base_plan_id: Option<String>,
    pub test_purchase: bool,
}

/// The fields of `purchases.subscriptionsv2.get` the server uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoogleSubscription {
    /// `SUBSCRIPTION_STATE_ACTIVE`, `…_CANCELED`, `…_IN_GRACE_PERIOD`,
    /// `…_ON_HOLD`, `…_PAUSED`, `…_EXPIRED`, `…_PENDING`,
    /// `…_PENDING_PURCHASE_CANCELED`.
    pub state: String,
    pub line_items: Vec<GoogleLineItem>,
    pub latest_order_id: Option<String>,
    /// The purchase this one replaced (an upgrade or downgrade).
    pub linked_purchase_token: Option<String>,
    /// A license-tester purchase (free).
    pub test_purchase: bool,
    pub acknowledged: bool,
}

impl GoogleSubscription {
    pub fn line_item(&self) -> Option<&GoogleLineItem> {
        self.line_items.first()
    }

    /// The order the credits are keyed by: the latest paid order (the first
    /// purchase or the latest renewal).
    pub fn order_id(&self) -> Option<&str> {
        self.line_item()
            .and_then(|item| item.latest_successful_order_id.as_deref())
            .or(self.latest_order_id.as_deref())
    }

    pub fn expiry_secs(&self) -> Option<i64> {
        self.line_items
            .iter()
            .filter_map(|item| item.expiry_secs)
            .max()
    }

    pub fn store_environment(&self) -> StoreEnvironment {
        if self.test_purchase || self.line_items.iter().any(|item| item.test_purchase) {
            StoreEnvironment::Sandbox
        } else {
            StoreEnvironment::Production
        }
    }
}

/// The fields of `purchases.products.get` the server uses.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoogleProductPurchase {
    pub order_id: Option<String>,
    /// 0 purchased, 1 canceled, 2 pending.
    pub purchase_state: i64,
    /// 0 not consumed yet, 1 consumed.
    pub consumption_state: i64,
    /// 0 test (license tester), 1 promo, 2 rewarded; `None` for a real purchase.
    pub purchase_type: Option<i64>,
}

impl GoogleProductPurchase {
    pub fn store_environment(&self) -> StoreEnvironment {
        if self.purchase_type == Some(0) {
            StoreEnvironment::Sandbox
        } else {
            StoreEnvironment::Production
        }
    }
}

/// D14: a Play subscription state as a `billing_subscriptions.status`.
/// `None` for a purchase that is not paid yet or a state Google may add
/// (nothing is written).
pub fn plan_status(state: &str, expiry_secs: Option<i64>, now: i64) -> Option<&'static str> {
    match state {
        "SUBSCRIPTION_STATE_ACTIVE" => Some("active"),
        // Auto-renew is off; the paid month still runs.
        "SUBSCRIPTION_STATE_CANCELED" => {
            if expiry_secs.is_some_and(|expiry| expiry > now) {
                Some("active")
            } else {
                Some("canceled")
            }
        }
        "SUBSCRIPTION_STATE_IN_GRACE_PERIOD" => Some("past_due"),
        "SUBSCRIPTION_STATE_ON_HOLD" | "SUBSCRIPTION_STATE_PAUSED" => Some("unpaid"),
        "SUBSCRIPTION_STATE_EXPIRED" => Some("canceled"),
        _ => None,
    }
}

fn non_empty(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Google sends some integers as JSON strings.
fn int(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn rfc3339_secs(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.timestamp())
}

/// A `SubscriptionPurchaseV2` answer.
pub fn parse_subscription(body: &Value) -> Result<GoogleSubscription, GoogleApiError> {
    let line_items = body["lineItems"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| -> Result<GoogleLineItem, GoogleApiError> {
            Ok(GoogleLineItem {
                product_id: non_empty(&item["productId"]).ok_or_else(|| {
                    GoogleApiError::Rejected("subscription line item has no productId".to_string())
                })?,
                expiry_secs: rfc3339_secs(&item["expiryTime"]),
                latest_successful_order_id: non_empty(&item["latestSuccessfulOrderId"]),
                base_plan_id: non_empty(&item["offerDetails"]["basePlanId"]),
                test_purchase: !item["testPurchase"].is_null(),
            })
        })
        .collect::<Result<Vec<_>, GoogleApiError>>()?;
    if line_items.is_empty() {
        return Err(GoogleApiError::Rejected(
            "subscription has no line items".to_string(),
        ));
    }
    Ok(GoogleSubscription {
        state: non_empty(&body["subscriptionState"])
            .unwrap_or_else(|| "SUBSCRIPTION_STATE_UNSPECIFIED".to_string()),
        line_items,
        latest_order_id: non_empty(&body["latestOrderId"]),
        linked_purchase_token: non_empty(&body["linkedPurchaseToken"]),
        test_purchase: !body["testPurchase"].is_null(),
        acknowledged: body["acknowledgementState"].as_str()
            == Some("ACKNOWLEDGEMENT_STATE_ACKNOWLEDGED"),
    })
}

/// A `ProductPurchase` answer.
pub fn parse_product(body: &Value) -> GoogleProductPurchase {
    GoogleProductPurchase {
        order_id: non_empty(&body["orderId"]),
        purchase_state: int(&body["purchaseState"]).unwrap_or(0),
        consumption_state: int(&body["consumptionState"]).unwrap_or(0),
        purchase_type: int(&body["purchaseType"]),
    }
}

/// The `DeveloperNotification` inside a Pub/Sub push envelope
/// (`{"message": {"data": "<base64>"}}`), or `None` when it is unreadable.
pub fn decode_rtdn(body: &[u8]) -> Option<Value> {
    let envelope: Value = serde_json::from_slice(body).ok()?;
    let data = envelope["message"]["data"].as_str()?.trim();
    let bytes = STANDARD
        .decode(data)
        .or_else(|_| STANDARD_NO_PAD.decode(data))
        .or_else(|_| URL_SAFE.decode(data))
        .or_else(|_| URL_SAFE_NO_PAD.decode(data))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Debug)]
pub enum GoogleApiError {
    /// The service account is unset or does not parse.
    NotConfigured(String),
    /// Network trouble, 5xx, 429 or an auth failure: try again later.
    Unavailable(String),
    /// Google refused the request or answered something unreadable.
    Rejected(String),
}

impl std::fmt::Display for GoogleApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoogleApiError::NotConfigured(detail) => write!(f, "not configured: {detail}"),
            GoogleApiError::Unavailable(detail) => write!(f, "unavailable: {detail}"),
            GoogleApiError::Rejected(detail) => write!(f, "rejected: {detail}"),
        }
    }
}

/// The Play Developer API calls the sync needs. Tests use a fake.
#[async_trait]
pub trait GooglePlayApi: Send + Sync {
    /// `purchases.subscriptionsv2.get`; `None` when Google knows no such token.
    async fn subscription(
        &self,
        purchase_token: &str,
    ) -> Result<Option<GoogleSubscription>, GoogleApiError>;

    /// `purchases.products.get`; `None` when Google knows no such purchase.
    async fn product(
        &self,
        product_id: &str,
        purchase_token: &str,
    ) -> Result<Option<GoogleProductPurchase>, GoogleApiError>;

    /// `purchases.subscriptions.acknowledge`.
    async fn acknowledge_subscription(
        &self,
        subscription_id: &str,
        purchase_token: &str,
    ) -> Result<(), GoogleApiError>;

    /// `purchases.products.consume`.
    async fn consume_product(
        &self,
        product_id: &str,
        purchase_token: &str,
    ) -> Result<(), GoogleApiError>;
}

/// The real Play Developer API client.
pub struct GooglePlayClient {
    package: String,
    client_email: String,
    private_key_id: Option<String>,
    key: RsaKeyPair,
}

impl GooglePlayClient {
    pub fn from_env() -> Result<Self, GoogleApiError> {
        let raw = std::env::var(SERVICE_ACCOUNT_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                GoogleApiError::NotConfigured(format!("{SERVICE_ACCOUNT_ENV} must be set"))
            })?;
        Self::from_service_account(&raw, package_name_from_env())
    }

    /// From a service account key file. Errors never include key material.
    pub fn from_service_account(json: &str, package: String) -> Result<Self, GoogleApiError> {
        let account: Value = serde_json::from_str(json).map_err(|_| {
            GoogleApiError::NotConfigured(format!("{SERVICE_ACCOUNT_ENV} is not JSON"))
        })?;
        let client_email = non_empty(&account["client_email"]).ok_or_else(|| {
            GoogleApiError::NotConfigured(format!("{SERVICE_ACCOUNT_ENV} has no client_email"))
        })?;
        let pem = account["private_key"].as_str().ok_or_else(|| {
            GoogleApiError::NotConfigured(format!("{SERVICE_ACCOUNT_ENV} has no private_key"))
        })?;
        Ok(Self {
            package,
            client_email,
            private_key_id: non_empty(&account["private_key_id"]),
            key: parse_rsa_pem(pem)?,
        })
    }

    /// An OAuth access token for the Play Developer API, from the cache while
    /// it is valid.
    async fn access_token(&self) -> Result<String, GoogleApiError> {
        let now = chrono::Utc::now().timestamp();
        let cached = {
            let guard = TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .as_ref()
                .filter(|cached| {
                    cached.client_email == self.client_email && cached.expires_at > now
                })
                .map(|cached| cached.access_token.clone())
        };
        if let Some(access_token) = cached {
            return Ok(access_token);
        }
        let assertion = jwt_assertion(
            &self.key,
            self.private_key_id.as_deref(),
            &self.client_email,
            now,
        )?;
        // The assertion is base64url plus dots: nothing to form-encode.
        let form = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
        );
        let response = crate::net_client::client()
            .post(TOKEN_URI)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| GoogleApiError::Unavailable(format!("Google OAuth: {error}")))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(GoogleApiError::Unavailable(format!(
                "Google OAuth {status}: {} {}",
                body["error"].as_str().unwrap_or_default(),
                body["error_description"].as_str().unwrap_or_default()
            )));
        }
        let access_token = non_empty(&body["access_token"]).ok_or_else(|| {
            GoogleApiError::Unavailable("Google OAuth answer has no access_token".to_string())
        })?;
        let expires_in = int(&body["expires_in"]).unwrap_or(3600);
        *TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(CachedToken {
            client_email: self.client_email.clone(),
            access_token: access_token.clone(),
            expires_at: now + expires_in - TOKEN_EARLY_REFRESH_SECS,
        });
        Ok(access_token)
    }

    /// One Play Developer API call. `Ok(None)` for 404/410 (Google knows no
    /// such purchase, or it is too old to read).
    async fn call(&self, post: bool, path: &str) -> Result<Option<Value>, GoogleApiError> {
        let token = self.access_token().await?;
        let url = format!("{API_BASE}/{}{path}", self.package);
        let request = if post {
            crate::net_client::client().post(url).body(Vec::<u8>::new())
        } else {
            crate::net_client::client().get(url)
        };
        let response = request
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| GoogleApiError::Unavailable(format!("Play Developer API: {error}")))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(Some(if text.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&text).unwrap_or(Value::Null)
            }));
        }
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Ok(None);
        }
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let detail = format!(
            "Play Developer API {status}: {}",
            body["error"]["message"].as_str().unwrap_or_default()
        );
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // A revoked or rotated key: drop the cached token.
            *TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        if status.is_server_error()
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            Err(GoogleApiError::Unavailable(detail))
        } else {
            Err(GoogleApiError::Rejected(detail))
        }
    }
}

#[async_trait]
impl GooglePlayApi for GooglePlayClient {
    async fn subscription(
        &self,
        purchase_token: &str,
    ) -> Result<Option<GoogleSubscription>, GoogleApiError> {
        if !valid_purchase_token(purchase_token) {
            return Ok(None);
        }
        match self
            .call(
                false,
                &format!("/purchases/subscriptionsv2/tokens/{purchase_token}"),
            )
            .await?
        {
            Some(body) => parse_subscription(&body).map(Some),
            None => Ok(None),
        }
    }

    async fn product(
        &self,
        product_id: &str,
        purchase_token: &str,
    ) -> Result<Option<GoogleProductPurchase>, GoogleApiError> {
        if !valid_purchase_token(purchase_token) || !valid_product_id(product_id) {
            return Ok(None);
        }
        Ok(self
            .call(
                false,
                &format!("/purchases/products/{product_id}/tokens/{purchase_token}"),
            )
            .await?
            .map(|body| parse_product(&body)))
    }

    async fn acknowledge_subscription(
        &self,
        subscription_id: &str,
        purchase_token: &str,
    ) -> Result<(), GoogleApiError> {
        if !valid_purchase_token(purchase_token) || !valid_product_id(subscription_id) {
            return Err(GoogleApiError::Rejected("invalid id".to_string()));
        }
        self.call(
            true,
            &format!(
                "/purchases/subscriptions/{subscription_id}/tokens/{purchase_token}:acknowledge"
            ),
        )
        .await?
        .map(|_| ())
        .ok_or_else(|| GoogleApiError::Rejected("purchase not found".to_string()))
    }

    async fn consume_product(
        &self,
        product_id: &str,
        purchase_token: &str,
    ) -> Result<(), GoogleApiError> {
        if !valid_purchase_token(purchase_token) || !valid_product_id(product_id) {
            return Err(GoogleApiError::Rejected("invalid id".to_string()));
        }
        self.call(
            true,
            &format!("/purchases/products/{product_id}/tokens/{purchase_token}:consume"),
        )
        .await?
        .map(|_| ())
        .ok_or_else(|| GoogleApiError::Rejected("purchase not found".to_string()))
    }
}

/// Play product ids: lowercase letters, digits, `_` and `.`.
fn valid_product_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 150
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.'))
}

/// The RSA key from the service account's PKCS#8 PEM. Literal `\n` escapes
/// are accepted. Errors never include key material.
fn parse_rsa_pem(pem: &str) -> Result<RsaKeyPair, GoogleApiError> {
    let body: String = pem
        .replace("\\n", "\n")
        .replace("-----BEGIN PRIVATE KEY-----", "")
        .replace("-----END PRIVATE KEY-----", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let der = STANDARD.decode(body.as_bytes()).map_err(|_| {
        GoogleApiError::NotConfigured(format!("{SERVICE_ACCOUNT_ENV} private_key is not PEM"))
    })?;
    RsaKeyPair::from_pkcs8(&der).map_err(|_| {
        GoogleApiError::NotConfigured(format!(
            "{SERVICE_ACCOUNT_ENV} private_key is not an RSA PKCS#8 key"
        ))
    })
}

/// The signed JWT for Google's JWT bearer grant (RS256; claims `iss`,
/// `scope`, `aud`, `iat`, `exp`).
fn jwt_assertion(
    key: &RsaKeyPair,
    key_id: Option<&str>,
    client_email: &str,
    now: i64,
) -> Result<String, GoogleApiError> {
    let header = match key_id {
        Some(kid) => serde_json::json!({ "alg": "RS256", "typ": "JWT", "kid": kid }),
        None => serde_json::json!({ "alg": "RS256", "typ": "JWT" }),
    };
    let claims = serde_json::json!({
        "iss": client_email,
        "scope": SCOPE,
        "aud": TOKEN_URI,
        "iat": now,
        "exp": now + ASSERTION_LIFETIME_SECS,
    });
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let mut sig = vec![0_u8; key.public_modulus_len()];
    key.sign(
        &signature::RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut sig,
    )
    .map_err(|_| GoogleApiError::Unavailable("RS256 signing failed".to_string()))?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(&sig)))
}

/// `{granted, reason?, message?, subscription?, balance_usd?, idempotent_replay}`:
/// the same answer as the Apple route.
pub type GoogleSyncOutcome = AppleSyncOutcome;

fn not_granted(reason: &'static str, message: Option<&'static str>) -> GoogleSyncOutcome {
    GoogleSyncOutcome {
        reason: Some(reason),
        message,
        ..Default::default()
    }
}

/// Why a sync was refused.
#[derive(Debug)]
pub enum GoogleSyncError {
    /// 409 `owned_by_another_account` (D4).
    OwnedByAnotherAccount,
    /// A product (or base plan) this server does not sell, or a plan posted
    /// as a pack.
    UnknownProduct(String),
    /// Google knows no such purchase.
    NotFound,
    Store(GoogleApiError),
    Api(ApiError),
}

impl From<ApiError> for GoogleSyncError {
    fn from(error: ApiError) -> Self {
        GoogleSyncError::Api(error)
    }
}

impl From<sqlx::Error> for GoogleSyncError {
    fn from(error: sqlx::Error) -> Self {
        GoogleSyncError::Api(ApiError::DatabaseError(error))
    }
}

impl From<StoreError> for GoogleSyncError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::OwnedByAnotherAccount => GoogleSyncError::OwnedByAnotherAccount,
            StoreError::Api(error) => GoogleSyncError::Api(error),
        }
    }
}

impl std::fmt::Display for GoogleSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoogleSyncError::OwnedByAnotherAccount => write!(f, "owned by another account"),
            GoogleSyncError::UnknownProduct(product) => write!(f, "unknown product {product}"),
            GoogleSyncError::NotFound => write!(f, "purchase not found"),
            GoogleSyncError::Store(error) => write!(f, "Google Play: {error}"),
            GoogleSyncError::Api(error) => write!(f, "{error}"),
        }
    }
}

/// What an RTDN did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RtdnOutcome {
    /// `subscription`, `one_time_product`, `voided_purchase`, `test` or `unknown`.
    pub kind: &'static str,
    /// False for test and unknown notifications and for ones that name
    /// nothing of ours.
    pub handled: bool,
    pub sync: Option<GoogleSyncOutcome>,
}

/// The ledger key of a Play order: the `orderId`, or (a promo purchase has
/// none) the token's hash.
fn order_key(order_id: Option<&str>, purchase_token: &str) -> String {
    match order_id {
        Some(order) => order.to_string(),
        None => format!("token-{}", store_billing::sha256_hex(purchase_token)),
    }
}

/// The Google sync over one database and one Play Developer API client.
pub struct GoogleSync<'a> {
    pub db: &'a PgPool,
    pub api: &'a dyn GooglePlayApi,
    pub package_name: String,
    pub sandbox_grant_user_ids: Vec<String>,
}

impl<'a> GoogleSync<'a> {
    /// Package and sandbox allow-list from the environment.
    pub fn from_env(db: &'a PgPool, api: &'a dyn GooglePlayApi) -> Self {
        Self {
            db,
            api,
            package_name: package_name_from_env(),
            sandbox_grant_user_ids: store_apple::sandbox_grant_user_ids_from_env(),
        }
    }

    fn sandbox_allowed(&self, user_id: &str) -> bool {
        self.sandbox_grant_user_ids.iter().any(|id| id == user_id)
    }

    /// `POST /billing/store/google/purchases`: read the purchase from Google
    /// and grant it to the signed-in caller, who claims the token (D4).
    pub async fn verify_purchase(
        &self,
        user_id: &str,
        item: StoreItem,
        purchase_token: &str,
    ) -> Result<GoogleSyncOutcome, GoogleSyncError> {
        match item {
            StoreItem::Plan(_) => {
                self.sync_subscription(purchase_token, Some(user_id), None)
                    .await
            }
            StoreItem::Pack(pack) => {
                self.sync_product(pack.google_product_id, purchase_token, Some(user_id))
                    .await
            }
        }
    }

    /// A Pub/Sub push body. Decoded for the token only, then re-read from
    /// Google (D3).
    pub async fn handle_rtdn(&self, body: &[u8]) -> Result<RtdnOutcome, GoogleSyncError> {
        let Some(notification) = decode_rtdn(body) else {
            tracing::warn!("google rtdn: unreadable Pub/Sub envelope, acknowledged");
            return Ok(RtdnOutcome {
                kind: "unknown",
                ..Default::default()
            });
        };
        if let Some(package) = notification["packageName"].as_str() {
            if package != self.package_name {
                tracing::warn!(package, "google rtdn for another package, ignored");
                return Ok(RtdnOutcome {
                    kind: "unknown",
                    ..Default::default()
                });
            }
        }
        let sub = &notification["subscriptionNotification"];
        let one_time = &notification["oneTimeProductNotification"];
        let voided = &notification["voidedPurchaseNotification"];
        let mut outcome = RtdnOutcome::default();
        if !notification["testNotification"].is_null() {
            outcome.kind = "test";
            tracing::info!("google rtdn test notification received");
            return Ok(outcome);
        }
        if sub.is_object() {
            outcome.kind = "subscription";
            let Some(token) = non_empty(&sub["purchaseToken"]).filter(|t| valid_purchase_token(t))
            else {
                return Ok(outcome);
            };
            let sync = self.sync_subscription(&token, None, None).await?;
            tracing::info!(
                notification_type = int(&sub["notificationType"]).unwrap_or_default(),
                granted = sync.granted,
                reason = sync.reason.unwrap_or_default(),
                "google rtdn: subscription synced"
            );
            outcome.handled = true;
            outcome.sync = Some(sync);
        } else if one_time.is_object() {
            outcome.kind = "one_time_product";
            let (Some(token), Some(sku)) = (
                non_empty(&one_time["purchaseToken"]).filter(|t| valid_purchase_token(t)),
                non_empty(&one_time["sku"]),
            ) else {
                return Ok(outcome);
            };
            // 2 = ONE_TIME_PRODUCT_CANCELED (a pending purchase that never paid).
            if int(&one_time["notificationType"]) == Some(2) {
                tracing::info!(%sku, "google rtdn: pending one-time purchase canceled");
                return Ok(outcome);
            }
            let sync = self.sync_product(&sku, &token, None).await?;
            outcome.handled = true;
            outcome.sync = Some(sync);
        } else if voided.is_object() {
            outcome.kind = "voided_purchase";
            let (Some(token), Some(order_id)) = (
                non_empty(&voided["purchaseToken"]).filter(|t| valid_purchase_token(t)),
                non_empty(&voided["orderId"]),
            ) else {
                return Ok(outcome);
            };
            // productType 1 = subscription, 2 = one-time product.
            let subscription = int(&voided["productType"]) == Some(1);
            outcome.sync = Some(self.voided(&token, &order_id, subscription).await?);
            outcome.handled = true;
        } else {
            outcome.kind = "unknown";
            tracing::info!("google rtdn of an unknown kind, acknowledged");
        }
        Ok(outcome)
    }

    /// D13: re-read the caller's Google plans whose period end has passed,
    /// at most once per 10 minutes per row.
    pub async fn refresh_due_plans(
        &self,
        user_id: &str,
    ) -> Result<Vec<Suspension>, GoogleSyncError> {
        let due: Vec<Option<String>> = sqlx::query_scalar(
            r#"
            UPDATE billing_subscriptions SET store_checked_at = NOW()
            WHERE user_id = $1 AND provider = 'google'
              AND status IN ('active', 'trialing', 'past_due', 'unpaid')
              AND current_period_end IS NOT NULL AND current_period_end < NOW()
              AND (store_checked_at IS NULL OR store_checked_at < NOW() - INTERVAL '10 minutes')
            RETURNING store_purchase_token
            "#,
        )
        .bind(user_id)
        .fetch_all(self.db)
        .await?;
        let mut suspend = Vec::new();
        for token in due.into_iter().flatten() {
            match self.sync_subscription(&token, None, None).await {
                Ok(outcome) => suspend.extend(outcome.suspend),
                Err(GoogleSyncError::NotFound) => {
                    // Google forgets subscriptions 60 days after they end.
                    suspend.extend(self.cancel_plan(&token).await?);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(suspend)
    }

    /// Who a purchase token belongs to (D4): the account a ledger row or the
    /// plan row of this token (else of the purchase it replaced) records.
    async fn token_owner(
        &self,
        purchase_token: &str,
        linked_token: Option<&str>,
    ) -> Result<Option<String>, GoogleSyncError> {
        for token in std::iter::once(purchase_token).chain(linked_token) {
            let hash = store_billing::sha256_hex(token);
            let owner: Option<String> = sqlx::query_scalar(
                r#"
                SELECT user_id FROM billing_store_transactions
                WHERE provider = 'google' AND purchase_token_sha256 = $1 AND user_id IS NOT NULL
                UNION ALL
                SELECT user_id FROM billing_subscriptions WHERE stripe_subscription_id = $2
                LIMIT 1
                "#,
            )
            .bind(&hash)
            .bind(store_billing::subscription_key(
                StoreProvider::Google,
                token,
            ))
            .fetch_optional(self.db)
            .await?;
            if owner.is_some() {
                return Ok(owner);
            }
        }
        Ok(None)
    }

    /// The account to grant to. With a caller: the token must be unclaimed or
    /// the caller's. Without one: the recorded owner, if any.
    async fn resolve_owner(
        &self,
        purchase_token: &str,
        linked_token: Option<&str>,
        caller: Option<&str>,
    ) -> Result<Option<String>, GoogleSyncError> {
        let owner = self.token_owner(purchase_token, linked_token).await?;
        match (caller, owner) {
            (Some(caller), Some(owner)) if owner != caller => {
                Err(GoogleSyncError::OwnedByAnotherAccount)
            }
            (Some(caller), _) => Ok(Some(caller.to_string())),
            (None, owner) => Ok(owner),
        }
    }

    /// The ledger state of one order, if recorded.
    async fn order_state(&self, order: &str) -> Result<Option<String>, GoogleSyncError> {
        Ok(sqlx::query_scalar(
            "SELECT state FROM billing_store_transactions WHERE provider = 'google' AND transaction_id = $1",
        )
        .bind(order)
        .fetch_optional(self.db)
        .await?)
    }

    /// A subscription token as Google reports it: owner (D4), sandbox (D8),
    /// the plan row and tier (D1, D14), the month's credits (D6), then the
    /// acknowledge. `refunded_order` is a voided order of this plan (D7).
    async fn sync_subscription(
        &self,
        purchase_token: &str,
        caller: Option<&str>,
        refunded_order: Option<&str>,
    ) -> Result<GoogleSyncOutcome, GoogleSyncError> {
        let sub = self
            .api
            .subscription(purchase_token)
            .await
            .map_err(GoogleSyncError::Store)?
            .ok_or(GoogleSyncError::NotFound)?;
        let line = sub
            .line_item()
            .ok_or_else(|| GoogleSyncError::UnknownProduct(String::new()))?;
        let plan = match store_billing::item_for_product(StoreProvider::Google, &line.product_id) {
            Some(StoreItem::Plan(plan)) => plan,
            _ => return Err(GoogleSyncError::UnknownProduct(line.product_id.clone())),
        };
        if let Some(base_plan) = line.base_plan_id.as_deref() {
            if base_plan != BASE_PLAN_ID {
                return Err(GoogleSyncError::UnknownProduct(format!(
                    "{}/{base_plan}",
                    line.product_id
                )));
            }
        }
        let item = StoreItem::Plan(plan);
        let environment = sub.store_environment();
        let order = order_key(sub.order_id(), purchase_token);
        let linked = sub
            .linked_purchase_token
            .as_deref()
            .filter(|linked| *linked != purchase_token);

        let Some(user_id) = self.resolve_owner(purchase_token, linked, caller).await? else {
            if plan_status(
                &sub.state,
                sub.expiry_secs(),
                chrono::Utc::now().timestamp(),
            )
            .is_some()
            {
                self.record(&order, purchase_token, None, item, environment, "unclaimed")
                    .await?;
            }
            tracing::info!(product = %line.product_id, "google subscription not claimed yet, stored as unclaimed");
            return Ok(not_granted("unclaimed", None));
        };
        if account_closed(self.db, &user_id).await? {
            tracing::warn!(%user_id, "google purchase for an erased account, nothing granted");
            return Ok(not_granted("account_closed", None));
        }

        let now = chrono::Utc::now().timestamp();
        let Some(mut mapped) = plan_status(&sub.state, sub.expiry_secs(), now) else {
            if sub.state == "SUBSCRIPTION_STATE_PENDING" {
                if caller.is_some() {
                    self.record(
                        &order,
                        purchase_token,
                        Some(&user_id),
                        item,
                        environment,
                        "pending",
                    )
                    .await?;
                }
                return Ok(not_granted("pending", Some(PENDING_MESSAGE)));
            }
            tracing::info!(state = %sub.state, "google subscription not paid; nothing written");
            return Ok(not_granted("canceled", None));
        };

        if environment == StoreEnvironment::Sandbox && !self.sandbox_allowed(&user_id) {
            self.record(
                &order,
                purchase_token,
                Some(&user_id),
                item,
                environment,
                "sandbox_not_granted",
            )
            .await?;
            // A test purchase: acknowledge so the tester's subscription runs
            // its test course; nothing was granted.
            if !sub.acknowledged {
                self.acknowledge(&line.product_id, purchase_token).await;
            }
            return Ok(not_granted("sandbox_not_granted", Some(SANDBOX_MESSAGE)));
        }

        let refunded = refunded_order.is_some_and(|voided| voided == order)
            || self.order_state(&order).await?.as_deref() == Some("refunded");
        if refunded {
            mapped = "canceled";
        }

        let mut outcome = GoogleSyncOutcome::default();
        // A replacement (upgrade or downgrade) ends the old purchase's row.
        if let (Some(old), "active" | "past_due") = (linked, mapped) {
            self.supersede(old, &user_id).await?;
        }
        // The plan row first, so a token that belongs to someone else stops
        // here before any credits move.
        let (view, suspension) = self
            .write_plan(
                purchase_token,
                &user_id,
                plan,
                mapped,
                environment,
                sub.expiry_secs(),
                &order,
            )
            .await?;
        outcome.subscription = Some(view);
        outcome.suspend.extend(suspension);
        if refunded {
            outcome.reason = Some("refunded");
            return Ok(outcome);
        }
        if !matches!(mapped, "active" | "past_due") || sub.order_id().is_none() {
            return Ok(outcome);
        }

        alert_if_over_first_purchase_cap(self.db, &user_id, &order, item, environment).await;
        let grant = store_billing::grant_store_transaction(
            self.db,
            &StoreTransaction {
                provider: StoreProvider::Google,
                transaction_id: &order,
                original_transaction_id: None,
                purchase_token: Some(purchase_token),
                user_id: &user_id,
                item,
                environment,
                organization_id: None,
            },
        )
        .await?;
        outcome.granted = true;
        outcome.balance_usd = grant.balance_usd;
        outcome.idempotent_replay = grant.idempotent_replay;
        // Only now that the grant committed.
        if !sub.acknowledged {
            self.acknowledge(&line.product_id, purchase_token).await;
        }
        Ok(outcome)
    }

    /// A one-time product (credit pack) as Google reports it: owner (D4),
    /// sandbox (D8), the credits once (D6), then the consume.
    async fn sync_product(
        &self,
        product_id: &str,
        purchase_token: &str,
        caller: Option<&str>,
    ) -> Result<GoogleSyncOutcome, GoogleSyncError> {
        let item = match store_billing::item_for_product(StoreProvider::Google, product_id) {
            Some(item @ StoreItem::Pack(_)) => item,
            _ => return Err(GoogleSyncError::UnknownProduct(product_id.to_string())),
        };
        let purchase = self
            .api
            .product(product_id, purchase_token)
            .await
            .map_err(GoogleSyncError::Store)?
            .ok_or(GoogleSyncError::NotFound)?;
        let environment = purchase.store_environment();
        let order = order_key(purchase.order_id.as_deref(), purchase_token);

        let Some(user_id) = self.resolve_owner(purchase_token, None, caller).await? else {
            if purchase.purchase_state == 0 {
                self.record(&order, purchase_token, None, item, environment, "unclaimed")
                    .await?;
            }
            tracing::info!(%product_id, "google pack not claimed yet, stored as unclaimed");
            return Ok(not_granted("unclaimed", None));
        };
        match purchase.purchase_state {
            0 => {}
            2 => {
                if caller.is_some() {
                    self.record(
                        &order,
                        purchase_token,
                        Some(&user_id),
                        item,
                        environment,
                        "pending",
                    )
                    .await?;
                }
                return Ok(not_granted("pending", Some(PENDING_MESSAGE)));
            }
            _ => return Ok(not_granted("canceled", None)),
        }
        if self.order_state(&order).await?.as_deref() == Some("refunded") {
            return Ok(not_granted("refunded", None));
        }
        if account_closed(self.db, &user_id).await? {
            tracing::warn!(%user_id, "google purchase for an erased account, nothing granted");
            return Ok(not_granted("account_closed", None));
        }
        if environment == StoreEnvironment::Sandbox && !self.sandbox_allowed(&user_id) {
            self.record(
                &order,
                purchase_token,
                Some(&user_id),
                item,
                environment,
                "sandbox_not_granted",
            )
            .await?;
            // Consumed so the tester can buy the pack again; nothing granted.
            if purchase.consumption_state == 0 {
                self.consume(product_id, purchase_token).await;
            }
            return Ok(not_granted("sandbox_not_granted", Some(SANDBOX_MESSAGE)));
        }

        alert_if_over_first_purchase_cap(self.db, &user_id, &order, item, environment).await;
        let organization_id = store_billing::organization_for_user(self.db, &user_id).await?;
        let grant = store_billing::grant_store_transaction(
            self.db,
            &StoreTransaction {
                provider: StoreProvider::Google,
                transaction_id: &order,
                original_transaction_id: None,
                purchase_token: Some(purchase_token),
                user_id: &user_id,
                item,
                environment,
                organization_id: organization_id.as_deref(),
            },
        )
        .await?;
        // Only now that the grant committed.
        if purchase.consumption_state == 0 {
            self.consume(product_id, purchase_token).await;
        }
        Ok(GoogleSyncOutcome {
            granted: true,
            balance_usd: grant.balance_usd,
            idempotent_replay: grant.idempotent_replay,
            ..Default::default()
        })
    }

    /// D7: a voided order takes its credits back once; a voided plan order
    /// re-reads the plan, which is canceled when that order is its latest.
    async fn voided(
        &self,
        purchase_token: &str,
        order_id: &str,
        subscription: bool,
    ) -> Result<GoogleSyncOutcome, GoogleSyncError> {
        let mut dbtx = self.db.begin().await?;
        let refunded: Option<(Option<String>, f64, String)> = sqlx::query_as(
            r#"
            UPDATE billing_store_transactions
            SET state = 'refunded', refunded_at = NOW(), updated_at = NOW()
            WHERE provider = 'google' AND transaction_id = $1 AND refunded_at IS NULL
            RETURNING user_id, credits_granted_usd, kind
            "#,
        )
        .bind(order_id)
        .fetch_optional(&mut *dbtx)
        .await?;
        let mut is_plan = subscription;
        match &refunded {
            Some((owner, credits, kind)) => {
                is_plan |= kind == "plan";
                if let (Some(owner), true) = (owner, *credits > 0.0) {
                    let key = store_billing::grant_key(StoreProvider::Google, order_id);
                    move_credits(
                        &mut dbtx,
                        owner,
                        -credits,
                        &format!("{key}-refund"),
                        REFUND_SOURCE,
                        kind == "pack",
                    )
                    .await?;
                    tracing::warn!(user_id = %owner, %key, credits, "google voided purchase: credits taken back");
                }
            }
            None => {
                tracing::info!(%order_id, "google voided purchase: no unrefunded order recorded");
            }
        }
        dbtx.commit().await?;

        let mut outcome = not_granted("refunded", None);
        if is_plan {
            match self
                .sync_subscription(purchase_token, None, Some(order_id))
                .await
            {
                Ok(synced) => {
                    outcome.subscription = synced.subscription;
                    outcome.suspend = synced.suspend;
                }
                Err(GoogleSyncError::NotFound) => {
                    outcome
                        .suspend
                        .extend(self.cancel_plan(purchase_token).await?);
                }
                Err(GoogleSyncError::UnknownProduct(product)) => {
                    tracing::warn!(%product, "google voided purchase of an unknown product");
                }
                Err(error) => return Err(error),
            }
        }
        Ok(outcome)
    }

    /// Record a Play order without granting it (`unclaimed`, `pending`,
    /// `sandbox_not_granted`), once. With a user, an unclaimed row is claimed
    /// (and takes the new state); a row of another account is refused.
    async fn record(
        &self,
        order: &str,
        purchase_token: &str,
        user_id: Option<&str>,
        item: StoreItem,
        environment: StoreEnvironment,
        state: &str,
    ) -> Result<(), GoogleSyncError> {
        sqlx::query(
            r#"
            INSERT INTO billing_store_transactions (
                provider, transaction_id, purchase_token_sha256, user_id,
                product_id, kind, item_id, environment, state, grant_key
            ) VALUES ('google', $1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (provider, transaction_id) DO NOTHING
            "#,
        )
        .bind(order)
        .bind(store_billing::sha256_hex(purchase_token))
        .bind(user_id)
        .bind(item.product_id(StoreProvider::Google))
        .bind(item.kind())
        .bind(item.id())
        .bind(environment.as_str())
        .bind(state)
        .bind(store_billing::grant_key(StoreProvider::Google, order))
        .execute(self.db)
        .await?;
        let Some(user_id) = user_id else {
            return Ok(());
        };
        sqlx::query(
            r#"
            UPDATE billing_store_transactions
            SET user_id = $2, state = CASE WHEN state = 'unclaimed' THEN $3 ELSE state END,
                updated_at = NOW()
            WHERE provider = 'google' AND transaction_id = $1 AND user_id IS NULL
            "#,
        )
        .bind(order)
        .bind(user_id)
        .bind(state)
        .execute(self.db)
        .await?;
        let owner: Option<Option<String>> = sqlx::query_scalar(
            "SELECT user_id FROM billing_store_transactions WHERE provider = 'google' AND transaction_id = $1",
        )
        .bind(order)
        .fetch_optional(self.db)
        .await?;
        match owner.flatten() {
            Some(owner) if owner != user_id => Err(GoogleSyncError::OwnedByAnotherAccount),
            _ => Ok(()),
        }
    }

    /// The purchase `old_token` was replaced by a new one of the same account
    /// (D14): its row is canceled without stopping the computer, since the
    /// new plan takes over.
    async fn supersede(&self, old_token: &str, user_id: &str) -> Result<(), GoogleSyncError> {
        let key = store_billing::subscription_key(StoreProvider::Google, old_token);
        let owner: Option<String> = sqlx::query_scalar(
            "SELECT user_id FROM billing_subscriptions WHERE stripe_subscription_id = $1",
        )
        .bind(&key)
        .fetch_optional(self.db)
        .await?;
        match owner {
            Some(owner) if owner != user_id => Err(GoogleSyncError::OwnedByAnotherAccount),
            Some(_) => {
                sqlx::query(
                    "UPDATE billing_subscriptions SET status = 'canceled', updated_at = CURRENT_TIMESTAMP \
                     WHERE stripe_subscription_id = $1 AND status <> 'canceled'",
                )
                .bind(&key)
                .execute(self.db)
                .await?;
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Cancel a plan row Google can no longer read (D7 / D13 fallback).
    async fn cancel_plan(&self, purchase_token: &str) -> Result<Vec<Suspension>, GoogleSyncError> {
        let key = store_billing::subscription_key(StoreProvider::Google, purchase_token);
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT user_id, plan_id, store_environment FROM billing_subscriptions WHERE stripe_subscription_id = $1",
        )
        .bind(&key)
        .fetch_optional(self.db)
        .await?;
        let Some((user_id, plan_id, environment)) = row else {
            return Ok(Vec::new());
        };
        let Some(plan) = billing_subscriptions::find_plan(&plan_id) else {
            return Ok(Vec::new());
        };
        let environment = environment
            .as_deref()
            .and_then(StoreEnvironment::parse)
            .unwrap_or(StoreEnvironment::Production);
        let (_, suspension) = self
            .write_plan(
                purchase_token,
                &user_id,
                plan,
                "canceled",
                environment,
                None,
                "gone",
            )
            .await?;
        Ok(suspension.into_iter().collect())
    }

    /// Write the plan row and move the plan tier with it (D14), as the Apple
    /// sync does. `event_ref` names the change in the entitlement event id.
    #[allow(clippy::too_many_arguments)]
    async fn write_plan(
        &self,
        purchase_token: &str,
        user_id: &str,
        plan: &'static billing_subscriptions::SubscriptionPlan,
        mapped: &str,
        environment: StoreEnvironment,
        period_end: Option<i64>,
        event_ref: &str,
    ) -> Result<(StorePlanView, Option<Suspension>), GoogleSyncError> {
        let key = store_billing::subscription_key(StoreProvider::Google, purchase_token);
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT status FROM billing_subscriptions WHERE stripe_subscription_id = $1",
        )
        .bind(&key)
        .fetch_optional(self.db)
        .await?;
        let written = store_billing::upsert_store_subscription(
            self.db,
            &StoreSubscriptionUpdate {
                provider: StoreProvider::Google,
                store_id: purchase_token,
                user_id,
                plan,
                status: mapped,
                environment,
                period_end,
            },
        )
        .await?;
        sqlx::query(
            "UPDATE billing_subscriptions SET store_purchase_token = $2, store_checked_at = NOW() \
             WHERE stripe_subscription_id = $1",
        )
        .bind(&written.key)
        .bind(purchase_token)
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
        if current_tier.flatten().as_deref() != Some(tier.as_str()) {
            let event_id = format!("google-{event_ref}-{tier}-{}", Uuid::new_v4().simple());
            apply_hosted_entitlement(self.db, &event_id, user_id, &tier, None, "google").await?;
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
            billing_provider: StoreProvider::Google.as_str(),
            renews_at: period_end
                .and_then(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0))
                .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            manage_url: store_billing::manage_url(StoreProvider::Google.as_str(), plan.id),
            conflict: written.conflict,
        };
        Ok((view, suspension))
    }

    /// Acknowledge a subscription after its grant committed. A failure is
    /// logged; the next post or notification of the token retries it, and
    /// Google refunds after 3 days if it never succeeds (a voided-purchase
    /// notification then takes the credits back).
    async fn acknowledge(&self, subscription_id: &str, purchase_token: &str) {
        if let Err(error) = self
            .api
            .acknowledge_subscription(subscription_id, purchase_token)
            .await
        {
            tracing::error!(%subscription_id, %error, "google acknowledge failed; retried on the next post or notification");
        }
    }

    /// Consume a pack after its grant committed (same retry story).
    async fn consume(&self, product_id: &str, purchase_token: &str) {
        if let Err(error) = self.api.consume_product(product_id, purchase_token).await {
            tracing::error!(%product_id, %error, "google consume failed; retried on the next post or notification");
        }
    }
}

/// D13 for `GET /billing/subscription`: refresh the user's due Google plans
/// when Play purchases are switched on. Never fails the read.
pub async fn refresh_due_plans_from_env(db: &PgPool, user_id: &str) -> Vec<Suspension> {
    let due: Result<bool, sqlx::Error> = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM billing_subscriptions
            WHERE user_id = $1 AND provider = 'google'
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
            tracing::warn!(%error, %user_id, "google plan refresh: due check failed");
            return Vec::new();
        }
    }
    if !StoreConfig::from_env().google {
        return Vec::new();
    }
    let client = match GooglePlayClient::from_env() {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "google plan refresh: Play client not configured");
            return Vec::new();
        }
    };
    match GoogleSync::from_env(db, &client)
        .refresh_due_plans(user_id)
        .await
    {
        Ok(suspend) => suspend,
        Err(error) => {
            tracing::warn!(%error, %user_id, "google plan refresh failed; the next read retries in 10 minutes");
            Vec::new()
        }
    }
}

/// D9: a paid production purchase over the first-purchase cap is granted
/// (the money is paid) and the team is told.
async fn alert_if_over_first_purchase_cap(
    db: &PgPool,
    user_id: &str,
    order: &str,
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
            &format!("store-cap-bypass:google:{order}"),
            format!("Google Play purchase over the first-purchase cap: {user_id}"),
            format!(
                "User {user_id} paid Google Play for {} {} (${:.2} of credits, order {order}) without the \
                 first-purchase trust preflight requires. It was granted (the money is paid); check the account.",
                item.kind(),
                item.id(),
                item.credits_usd(),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::encoding::AsDer;
    use aws_lc_rs::signature::{KeyPair as _, UnparsedPublicKey};

    #[test]
    fn play_states_map_per_d14() {
        let now = 1_800_000_000;
        assert_eq!(
            plan_status("SUBSCRIPTION_STATE_ACTIVE", None, now),
            Some("active")
        );
        assert_eq!(
            plan_status("SUBSCRIPTION_STATE_CANCELED", Some(now + 60), now),
            Some("active"),
            "auto-renew off keeps the paid month"
        );
        assert_eq!(
            plan_status("SUBSCRIPTION_STATE_CANCELED", Some(now - 60), now),
            Some("canceled")
        );
        assert_eq!(
            plan_status("SUBSCRIPTION_STATE_IN_GRACE_PERIOD", None, now),
            Some("past_due")
        );
        assert_eq!(
            plan_status("SUBSCRIPTION_STATE_ON_HOLD", None, now),
            Some("unpaid")
        );
        assert_eq!(
            plan_status("SUBSCRIPTION_STATE_EXPIRED", None, now),
            Some("canceled")
        );
        assert_eq!(plan_status("SUBSCRIPTION_STATE_PENDING", None, now), None);
    }

    #[test]
    fn subscriptionsv2_and_product_answers_parse() {
        let sub = parse_subscription(&serde_json::json!({
            "kind": "androidpublisher#subscriptionPurchaseV2",
            "subscriptionState": "SUBSCRIPTION_STATE_ACTIVE",
            "latestOrderId": "GPA.1111-2222-3333-44444..1",
            "linkedPurchaseToken": "old-token",
            "acknowledgementState": "ACKNOWLEDGEMENT_STATE_PENDING",
            "testPurchase": {},
            "lineItems": [{
                "productId": "plan_max",
                "expiryTime": "2027-01-17T10:00:00.000Z",
                "latestSuccessfulOrderId": "GPA.1111-2222-3333-44444..1",
                "offerDetails": { "basePlanId": "monthly" }
            }]
        }))
        .unwrap();
        assert_eq!(sub.state, "SUBSCRIPTION_STATE_ACTIVE");
        assert_eq!(sub.order_id(), Some("GPA.1111-2222-3333-44444..1"));
        assert_eq!(sub.line_item().unwrap().product_id, "plan_max");
        assert_eq!(
            sub.line_item().unwrap().base_plan_id.as_deref(),
            Some("monthly")
        );
        assert_eq!(
            sub.expiry_secs(),
            Some(
                chrono::DateTime::parse_from_rfc3339("2027-01-17T10:00:00Z")
                    .unwrap()
                    .timestamp()
            )
        );
        assert_eq!(sub.linked_purchase_token.as_deref(), Some("old-token"));
        assert!(!sub.acknowledged);
        assert_eq!(sub.store_environment(), StoreEnvironment::Sandbox);
        assert!(parse_subscription(&serde_json::json!({ "lineItems": [] })).is_err());

        let real = parse_product(&serde_json::json!({
            "orderId": "GPA.9999-0000",
            "purchaseState": 0,
            "consumptionState": 0
        }));
        assert_eq!(real.order_id.as_deref(), Some("GPA.9999-0000"));
        assert_eq!(real.store_environment(), StoreEnvironment::Production);
        let test = parse_product(&serde_json::json!({ "purchaseState": "2", "purchaseType": 0 }));
        assert_eq!(test.purchase_state, 2);
        assert_eq!(test.store_environment(), StoreEnvironment::Sandbox);
    }

    #[test]
    fn rtdn_envelope_decodes_and_the_token_check_is_strict() {
        let inner = serde_json::json!({
            "version": "1.0",
            "packageName": "com.allternit.app",
            "testNotification": { "version": "1.0" }
        });
        let body = serde_json::json!({
            "message": { "data": STANDARD.encode(inner.to_string()), "messageId": "1" },
            "subscription": "projects/p/subscriptions/s"
        });
        assert_eq!(decode_rtdn(body.to_string().as_bytes()), Some(inner));
        assert_eq!(decode_rtdn(b"not json"), None);

        let secret = "a-long-rtdn-secret-0123456789";
        assert!(rtdn_token_matches(secret, secret));
        assert!(!rtdn_token_matches(secret, "a-long-rtdn-secret-012345678"));
        assert!(!rtdn_token_matches(secret, ""));
        assert!(!rtdn_token_matches("", ""), "an unset secret never matches");
        assert!(
            !rtdn_token_matches("short", "short"),
            "a short secret never matches"
        );
    }

    #[test]
    fn purchase_tokens_are_path_safe() {
        assert!(valid_purchase_token("abcDEF.123-_xyz"));
        assert!(!valid_purchase_token(""));
        assert!(!valid_purchase_token("../products/x"));
        assert!(!valid_purchase_token("a/b"));
        assert!(!valid_purchase_token(&"a".repeat(4097)));
    }

    /// The assertion is RS256 over Google's documented claims and verifies
    /// with the key's public half.
    #[test]
    fn jwt_assertion_is_rs256_with_google_claims() {
        let generated = RsaKeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let der = generated.as_der().unwrap();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\\n{}\\n-----END PRIVATE KEY-----\\n",
            STANDARD.encode(der.as_ref())
        );
        let account = serde_json::json!({
            "type": "service_account",
            "client_email": "play@allternit.iam.gserviceaccount.com",
            "private_key_id": "kid-1",
            "private_key": pem,
        });
        let client = GooglePlayClient::from_service_account(
            &account.to_string(),
            DEFAULT_PACKAGE.to_string(),
        )
        .expect("escaped newlines are accepted");
        let token = jwt_assertion(
            &client.key,
            client.private_key_id.as_deref(),
            &client.client_email,
            1_800_000_000,
        )
        .unwrap();
        let mut parts = token.split('.');
        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts.next().unwrap()).unwrap())
                .unwrap();
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts.next().unwrap()).unwrap())
                .unwrap();
        assert_eq!(header["alg"], "RS256");
        assert_eq!(header["kid"], "kid-1");
        assert_eq!(claims["iss"], "play@allternit.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], SCOPE);
        assert_eq!(claims["aud"], TOKEN_URI);
        assert_eq!(
            claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
            3600
        );
        let (input, sig) = token.rsplit_once('.').unwrap();
        UnparsedPublicKey::new(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            generated.public_key().as_ref(),
        )
        .verify(input.as_bytes(), &URL_SAFE_NO_PAD.decode(sig).unwrap())
        .expect("signature verifies");
        assert!(GooglePlayClient::from_service_account("{}", DEFAULT_PACKAGE.to_string()).is_err());
    }
}
