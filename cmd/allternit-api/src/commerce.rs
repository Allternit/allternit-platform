//! MCP App commerce (P5) — Stripe **test mode only**.
//!
//! Model: OpenAI's `requestCheckout` / `complete_checkout` flow, with the
//! payment sheet rendered by the Allternit host instead of the app's View.
//!
//! 1. An MCP App developer onboards through Stripe Connect Express
//!    ([`CommerceService::register_account`]).
//! 2. The app's server returns a checkout session (line items, totals in minor
//!    units, currency, merchant = the connected account, legal links). The host
//!    validates it and **recomputes every total** ([`validate_session`]); the
//!    View never supplies an amount. The validated copy is stored in
//!    `commerce_checkout_sessions` and is the only source of the charge amount.
//! 3. After the user approves in the host sheet, [`CommerceService::pay`]
//!    creates + confirms a PaymentIntent (`application_fee_amount` +
//!    `transfer_data.destination`, idempotency-keyed), then calls the app's
//!    `complete_checkout` tool with `{checkout_session_id, payment_intent_id}`
//!    and records the order.
//!
//! Hard rule: only `sk_test_` / `pk_test_` keys, read from the environment.
//! A live key configured for this feature is a startup error
//! ([`CommerceConfig::from_env`]); nothing here ever falls back to
//! `STRIPE_SECRET_KEY`.

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;

use crate::db::DbHandle;

type HmacSha256 = Hmac<Sha256>;

const MAX_LINE_ITEMS: usize = 100;
const MAX_QUANTITY: i64 = 10_000;
/// Largest single charge the host will attempt (minor units).
const MAX_TOTAL_MINOR: i64 = 99_999_999;
/// Stripe timestamps older than this are rejected (replay window).
const WEBHOOK_TOLERANCE_SECS: i64 = 300;
/// A `fulfilling` claim older than this is treated as abandoned (process died).
const FULFILLING_STALE_SECS: i64 = 300;

// ── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CommerceError {
    /// A live-mode key is configured for this feature. Fatal at startup.
    LiveKeyRefused(String),
    /// Test keys are not configured; the feature is off.
    NotConfigured,
    Invalid(String),
    /// A total the caller supplied does not match the host's recomputation.
    Mismatch(String),
    NotFound(String),
    Forbidden(String),
    Conflict(String),
    Stripe(String),
    Db(String),
}

impl std::fmt::Display for CommerceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LiveKeyRefused(m)
            | Self::Invalid(m)
            | Self::Mismatch(m)
            | Self::NotFound(m)
            | Self::Forbidden(m)
            | Self::Conflict(m)
            | Self::Stripe(m)
            | Self::Db(m) => write!(f, "{m}"),
            Self::NotConfigured => write!(f, "commerce (Stripe test mode) is not configured"),
        }
    }
}
impl std::error::Error for CommerceError {}

impl From<rusqlite::Error> for CommerceError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e.to_string())
    }
}

// ── Config ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CommerceConfig {
    secret_key: String,
    pub publishable_key: Option<String>,
    pub webhook_secret: Option<String>,
    /// Platform fee in basis points of the total. Default 0 until Eoj sets it.
    pub platform_fee_bps: u32,
    /// Flat platform fee added on top of the bps fee (minor units). Default 0.
    pub platform_fee_fixed_minor: i64,
    /// User ids allowed to issue host-initiated refunds.
    pub operator_user_ids: Vec<String>,
    pub stripe_base_url: String,
}

fn is_live_key(k: &str) -> bool {
    let k = k.trim();
    k.starts_with("sk_live_")
        || k.starts_with("rk_live_")
        || k.starts_with("pk_live_")
        || k.contains("_live_")
}

impl CommerceConfig {
    /// Build from `ALLTERNIT_COMMERCE_*` env vars.
    /// `Ok(None)` = feature off (no secret key). `Err(LiveKeyRefused)` = a live
    /// key is set anywhere for this feature — callers must not start.
    pub fn from_env() -> Result<Option<Self>, CommerceError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, CommerceError> {
        let nonempty = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let secret = nonempty("ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY");
        let publishable = nonempty("ALLTERNIT_COMMERCE_STRIPE_PUBLISHABLE_KEY");
        let webhook = nonempty("ALLTERNIT_COMMERCE_STRIPE_WEBHOOK_SECRET");

        // Check every key before anything else so a live publishable key with
        // no secret key is still refused.
        for (name, v) in [
            ("ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY", &secret),
            ("ALLTERNIT_COMMERCE_STRIPE_PUBLISHABLE_KEY", &publishable),
        ] {
            if let Some(v) = v {
                if is_live_key(v) {
                    return Err(CommerceError::LiveKeyRefused(format!(
                        "{name} is a live-mode Stripe key; MCP App commerce is test-mode only \
                         (sk_test_/pk_test_). Refusing to start."
                    )));
                }
            }
        }
        let Some(secret) = secret else { return Ok(None) };
        if !(secret.starts_with("sk_test_") || secret.starts_with("rk_test_")) {
            return Err(CommerceError::LiveKeyRefused(
                "ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY must start with sk_test_ (test mode only). \
                 Refusing to start."
                    .into(),
            ));
        }
        if let Some(p) = &publishable {
            if !p.starts_with("pk_test_") {
                return Err(CommerceError::LiveKeyRefused(
                    "ALLTERNIT_COMMERCE_STRIPE_PUBLISHABLE_KEY must start with pk_test_ \
                     (test mode only). Refusing to start."
                        .into(),
                ));
            }
        }
        let platform_fee_bps = match nonempty("ALLTERNIT_COMMERCE_PLATFORM_FEE_BPS") {
            Some(v) => v
                .parse::<u32>()
                .ok()
                .filter(|b| *b <= 10_000)
                .ok_or_else(|| CommerceError::Invalid("ALLTERNIT_COMMERCE_PLATFORM_FEE_BPS must be 0..=10000".into()))?,
            None => 0,
        };
        let platform_fee_fixed_minor = match nonempty("ALLTERNIT_COMMERCE_PLATFORM_FEE_FIXED_MINOR") {
            Some(v) => v
                .parse::<i64>()
                .ok()
                .filter(|b| *b >= 0)
                .ok_or_else(|| CommerceError::Invalid("ALLTERNIT_COMMERCE_PLATFORM_FEE_FIXED_MINOR must be >= 0".into()))?,
            None => 0,
        };
        let operator_user_ids = nonempty("ALLTERNIT_COMMERCE_OPERATOR_USER_IDS")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        Ok(Some(Self {
            secret_key: secret,
            publishable_key: publishable,
            webhook_secret: webhook,
            platform_fee_bps,
            platform_fee_fixed_minor,
            operator_user_ids,
            stripe_base_url: "https://api.stripe.com".to_string(),
        }))
    }

    /// Test constructor (still enforces the test-key rule).
    pub fn for_test(secret: &str, fee_bps: u32) -> Result<Self, CommerceError> {
        let secret = secret.to_string();
        Self::from_lookup(|k| match k {
            "ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY" => Some(secret.clone()),
            "ALLTERNIT_COMMERCE_PLATFORM_FEE_BPS" => Some(fee_bps.to_string()),
            _ => None,
        })?
        .ok_or(CommerceError::NotConfigured)
    }

    pub fn fee_for(&self, total_minor: i64) -> i64 {
        platform_fee(total_minor, self.platform_fee_bps, self.platform_fee_fixed_minor)
    }
}

// ── Fee math ─────────────────────────────────────────────────────────────────

/// `floor(total * bps / 10_000) + fixed`, clamped to `0..=total`. The
/// platform can never take more than the customer paid.
pub fn platform_fee(total_minor: i64, bps: u32, fixed_minor: i64) -> i64 {
    if total_minor <= 0 {
        return 0;
    }
    let pct = (total_minor as i128 * bps as i128 / 10_000) as i64;
    (pct.saturating_add(fixed_minor.max(0))).clamp(0, total_minor)
}

// ── Checkout session (from the app's server) ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineItem {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub quantity: i64,
    pub unit_amount_minor: i64,
    /// Optional app-supplied line total; must equal quantity × unit if present.
    #[serde(default)]
    pub total_minor: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Totals {
    pub subtotal_minor: i64,
    #[serde(default)]
    pub tax_minor: i64,
    pub total_minor: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Merchant {
    /// Stripe connected account id (`acct_…`).
    pub account_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LegalLinks {
    pub terms_url: String,
    pub refund_url: String,
    pub support_url: String,
    #[serde(default)]
    pub privacy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckoutSessionInput {
    pub id: String,
    pub currency: String,
    pub line_items: Vec<LineItem>,
    pub totals: Totals,
    pub merchant: Merchant,
    pub links: LegalLinks,
}

/// Pure validation. Returns the session with currency normalised and every
/// total recomputed by the host. Any supplied total that disagrees with the
/// recomputation is a hard [`CommerceError::Mismatch`] — never "corrected".
pub fn validate_session(input: &CheckoutSessionInput) -> Result<CheckoutSessionInput, CommerceError> {
    let bad = |m: &str| CommerceError::Invalid(m.to_string());
    let id = input.id.trim();
    if id.is_empty() || id.len() > 200 || !id.chars().all(|c| c.is_ascii_alphanumeric() || "-_:.".contains(c)) {
        return Err(bad("session id must be 1-200 chars of [A-Za-z0-9-_:.]"));
    }
    let currency = input.currency.trim().to_ascii_lowercase();
    if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_lowercase()) {
        return Err(bad("currency must be a 3-letter ISO code"));
    }
    if input.line_items.is_empty() || input.line_items.len() > MAX_LINE_ITEMS {
        return Err(bad("line_items must contain 1-100 items"));
    }
    if !input.merchant.account_id.starts_with("acct_") || input.merchant.name.trim().is_empty() {
        return Err(bad("merchant needs a Stripe acct_ id and a name"));
    }
    for (label, url) in [
        ("terms_url", Some(&input.links.terms_url)),
        ("refund_url", Some(&input.links.refund_url)),
        ("support_url", Some(&input.links.support_url)),
        ("privacy_url", input.links.privacy_url.as_ref()),
    ] {
        if let Some(u) = url {
            if !is_https_url(u) {
                return Err(CommerceError::Invalid(format!("{label} must be an https URL")));
            }
        }
    }

    let mut subtotal: i64 = 0;
    let mut items = Vec::with_capacity(input.line_items.len());
    for it in &input.line_items {
        if it.name.trim().is_empty() {
            return Err(bad("line item name is required"));
        }
        if it.quantity < 1 || it.quantity > MAX_QUANTITY {
            return Err(bad("line item quantity must be 1..=10000"));
        }
        if it.unit_amount_minor < 0 {
            return Err(bad("line item unit amount must be >= 0"));
        }
        let line = it
            .quantity
            .checked_mul(it.unit_amount_minor)
            .ok_or_else(|| bad("line item total overflows"))?;
        if let Some(claimed) = it.total_minor {
            if claimed != line {
                return Err(CommerceError::Mismatch(format!(
                    "line item '{}' total {claimed} != quantity × unit amount {line}",
                    it.name
                )));
            }
        }
        subtotal = subtotal.checked_add(line).ok_or_else(|| bad("subtotal overflows"))?;
        items.push(LineItem { total_minor: Some(line), ..it.clone() });
    }
    if input.totals.tax_minor < 0 {
        return Err(bad("tax must be >= 0"));
    }
    let total = subtotal
        .checked_add(input.totals.tax_minor)
        .ok_or_else(|| bad("total overflows"))?;
    if input.totals.subtotal_minor != subtotal {
        return Err(CommerceError::Mismatch(format!(
            "subtotal {} != sum of line items {subtotal}",
            input.totals.subtotal_minor
        )));
    }
    if input.totals.total_minor != total {
        return Err(CommerceError::Mismatch(format!(
            "total {} != subtotal + tax {total}",
            input.totals.total_minor
        )));
    }
    if total <= 0 || total > MAX_TOTAL_MINOR {
        return Err(bad("total must be > 0 and within the per-charge limit"));
    }
    Ok(CheckoutSessionInput {
        id: id.to_string(),
        currency,
        line_items: items,
        totals: Totals { subtotal_minor: subtotal, tax_minor: input.totals.tax_minor, total_minor: total },
        merchant: input.merchant.clone(),
        links: input.links.clone(),
    })
}

fn is_https_url(u: &str) -> bool {
    u.len() <= 2048
        && u.starts_with("https://")
        && u.len() > "https://".len()
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
}

// ── Stripe webhook signature ─────────────────────────────────────────────────

/// Verify a `Stripe-Signature` header (`t=<ts>,v1=<hex>[,v1=<hex>…]`):
/// HMAC-SHA256 of `"{t}.{raw_body}"` keyed by the endpoint secret, within
/// [`WEBHOOK_TOLERANCE_SECS`] of `now`. Constant-time comparison.
pub fn verify_stripe_signature(
    secret: &str,
    header: &str,
    payload: &[u8],
    now_unix: i64,
) -> Result<(), CommerceError> {
    let deny = |m: &str| CommerceError::Forbidden(format!("stripe signature: {m}"));
    let mut ts: Option<i64> = None;
    let mut sigs: Vec<Vec<u8>> = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => ts = v.parse().ok(),
            Some(("v1", v)) => {
                if let Ok(b) = hex::decode(v) {
                    sigs.push(b);
                }
            }
            _ => {}
        }
    }
    let ts = ts.ok_or_else(|| deny("missing timestamp"))?;
    if sigs.is_empty() {
        return Err(deny("missing v1 signature"));
    }
    if (now_unix - ts).abs() > WEBHOOK_TOLERANCE_SECS {
        return Err(deny("timestamp outside tolerance"));
    }
    for sig in &sigs {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| deny("bad secret"))?;
        mac.update(ts.to_string().as_bytes());
        mac.update(b".");
        mac.update(payload);
        if mac.verify_slice(sig).is_ok() {
            return Ok(());
        }
    }
    Err(deny("no signature matched"))
}

/// Test helper: header for `payload` signed with `secret` at `ts`.
pub fn sign_stripe_payload(secret: &str, ts: i64, payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(payload);
    format!("t={ts},v1={}", hex::encode(mac.finalize().into_bytes()))
}

// ── Stripe + app seams ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PaymentIntentRequest {
    pub amount_minor: i64,
    pub currency: String,
    pub application_fee_minor: i64,
    pub destination_account: String,
    pub payment_method: String,
    pub checkout_session_id: String,
    pub app_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaymentIntent {
    pub id: String,
    /// Stripe status: `succeeded`, `requires_action`, `requires_payment_method`, …
    pub status: String,
    pub amount_minor: i64,
    pub currency: String,
    pub application_fee_minor: Option<i64>,
    pub destination: Option<String>,
    pub checkout_session_id: Option<String>,
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StripeAccount {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StripeRefund {
    pub id: String,
    pub status: String,
}

#[async_trait]
pub trait StripeApi: Send + Sync {
    async fn create_express_account(
        &self,
        email: Option<&str>,
        country: Option<&str>,
        idempotency_key: &str,
    ) -> Result<StripeAccount, CommerceError>;
    async fn create_account_link(
        &self,
        account_id: &str,
        refresh_url: &str,
        return_url: &str,
    ) -> Result<String, CommerceError>;
    async fn create_payment_intent(
        &self,
        req: &PaymentIntentRequest,
        idempotency_key: &str,
    ) -> Result<PaymentIntent, CommerceError>;
    async fn retrieve_payment_intent(&self, id: &str) -> Result<PaymentIntent, CommerceError>;
    /// Full refund; reverses the transfer and refunds the application fee.
    async fn create_full_refund(
        &self,
        payment_intent_id: &str,
        idempotency_key: &str,
    ) -> Result<StripeRefund, CommerceError>;
}

/// How the host reaches the app's server. Production: the attached MCP server
/// `<app_id>` via [`crate::mcp_dispatcher::McpDispatcher`].
#[async_trait]
pub trait AppCaller: Send + Sync {
    /// Call the app's `complete_checkout` tool; returns the app's order result.
    async fn complete_checkout(&self, app_id: &str, args: Value) -> Result<Value, String>;
}

pub struct DispatcherAppCaller(pub crate::mcp_dispatcher::McpDispatcher);

#[async_trait]
impl AppCaller for DispatcherAppCaller {
    async fn complete_checkout(&self, app_id: &str, args: Value) -> Result<Value, String> {
        self.0.dispatch_call(&format!("{app_id}.complete_checkout"), args).await
    }
}

/// Real Stripe API client (test keys only — enforced by [`CommerceConfig`]).
pub struct StripeHttp {
    secret_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl StripeHttp {
    pub fn new(cfg: &CommerceConfig) -> Self {
        Self {
            secret_key: cfg.secret_key.clone(),
            base_url: cfg.stripe_base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        form: &[(String, String)],
        idempotency_key: Option<&str>,
    ) -> Result<Value, CommerceError> {
        let mut req = self
            .client
            .request(method.clone(), format!("{}{path}", self.base_url))
            .basic_auth(&self.secret_key, Some(""))
            .timeout(std::time::Duration::from_secs(30));
        if method != reqwest::Method::GET {
            req = req.form(form);
        }
        if let Some(k) = idempotency_key {
            req = req.header("Idempotency-Key", k);
        }
        let resp = req.send().await.map_err(|e| CommerceError::Stripe(format!("stripe request failed: {e}")))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| CommerceError::Stripe(format!("stripe response decode failed: {e}")))?;
        if !status.is_success() {
            let msg = body["error"]["message"].as_str().unwrap_or("unknown stripe error");
            return Err(CommerceError::Stripe(format!("stripe {status}: {msg}")));
        }
        Ok(body)
    }
}

fn kv(k: &str, v: impl ToString) -> (String, String) {
    (k.to_string(), v.to_string())
}

fn parse_pi(v: &Value) -> Result<PaymentIntent, CommerceError> {
    let s = |p: &Value| p.as_str().map(str::to_string);
    Ok(PaymentIntent {
        id: s(&v["id"]).ok_or_else(|| CommerceError::Stripe("payment intent missing id".into()))?,
        status: s(&v["status"]).unwrap_or_default(),
        amount_minor: v["amount"].as_i64().unwrap_or(-1),
        currency: s(&v["currency"]).unwrap_or_default(),
        application_fee_minor: v["application_fee_amount"].as_i64(),
        destination: s(&v["transfer_data"]["destination"]),
        checkout_session_id: s(&v["metadata"]["checkout_session_id"]),
        client_secret: s(&v["client_secret"]),
    })
}

#[async_trait]
impl StripeApi for StripeHttp {
    async fn create_express_account(
        &self,
        email: Option<&str>,
        country: Option<&str>,
        idempotency_key: &str,
    ) -> Result<StripeAccount, CommerceError> {
        let mut f = vec![
            kv("type", "express"),
            kv("capabilities[transfers][requested]", "true"),
            kv("capabilities[card_payments][requested]", "true"),
        ];
        if let Some(e) = email {
            f.push(kv("email", e));
        }
        if let Some(c) = country {
            f.push(kv("country", c));
        }
        let v = self.call(reqwest::Method::POST, "/v1/accounts", &f, Some(idempotency_key)).await?;
        Ok(StripeAccount {
            id: v["id"].as_str().ok_or_else(|| CommerceError::Stripe("account missing id".into()))?.to_string(),
        })
    }

    async fn create_account_link(
        &self,
        account_id: &str,
        refresh_url: &str,
        return_url: &str,
    ) -> Result<String, CommerceError> {
        let f = vec![
            kv("account", account_id),
            kv("refresh_url", refresh_url),
            kv("return_url", return_url),
            kv("type", "account_onboarding"),
        ];
        let v = self.call(reqwest::Method::POST, "/v1/account_links", &f, None).await?;
        v["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| CommerceError::Stripe("account link missing url".into()))
    }

    async fn create_payment_intent(
        &self,
        r: &PaymentIntentRequest,
        idempotency_key: &str,
    ) -> Result<PaymentIntent, CommerceError> {
        let f = vec![
            kv("amount", r.amount_minor),
            kv("currency", &r.currency),
            kv("application_fee_amount", r.application_fee_minor),
            kv("transfer_data[destination]", &r.destination_account),
            kv("payment_method", &r.payment_method),
            kv("confirm", "true"),
            kv("automatic_payment_methods[enabled]", "true"),
            kv("automatic_payment_methods[allow_redirects]", "never"),
            kv("metadata[checkout_session_id]", &r.checkout_session_id),
            kv("metadata[app_id]", &r.app_id),
            kv("metadata[host]", "allternit"),
        ];
        let v = self.call(reqwest::Method::POST, "/v1/payment_intents", &f, Some(idempotency_key)).await?;
        parse_pi(&v)
    }

    async fn retrieve_payment_intent(&self, id: &str) -> Result<PaymentIntent, CommerceError> {
        let v = self
            .call(reqwest::Method::GET, &format!("/v1/payment_intents/{}", urlencoding::encode(id)), &[], None)
            .await?;
        parse_pi(&v)
    }

    async fn create_full_refund(
        &self,
        payment_intent_id: &str,
        idempotency_key: &str,
    ) -> Result<StripeRefund, CommerceError> {
        let f = vec![
            kv("payment_intent", payment_intent_id),
            kv("reverse_transfer", "true"),
            kv("refund_application_fee", "true"),
        ];
        let v = self.call(reqwest::Method::POST, "/v1/refunds", &f, Some(idempotency_key)).await?;
        Ok(StripeRefund {
            id: v["id"].as_str().ok_or_else(|| CommerceError::Stripe("refund missing id".into()))?.to_string(),
            status: v["status"].as_str().unwrap_or_default().to_string(),
        })
    }
}

// ── Service ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct OnboardingResult {
    pub app_id: String,
    pub stripe_account_id: String,
    pub onboarding_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredSession {
    pub session: CheckoutSessionInput,
    pub application_fee_minor: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Order {
    pub id: String,
    pub checkout_session_id: String,
    pub app_id: String,
    pub payment_intent_id: String,
    pub currency: String,
    pub total_minor: i64,
    pub application_fee_minor: i64,
    pub status: String,
    pub app_result: Option<Value>,
    pub fulfillment_error: Option<String>,
    pub refund_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PayOutcome {
    /// Charge succeeded; `order.status` says whether the app has fulfilled it.
    Paid { order: Order },
    /// 3-D Secure etc.: the sheet must run `stripe.handleNextAction`, then
    /// call `complete`.
    RequiresAction { payment_intent_id: String, client_secret: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WebhookOutcome {
    pub event_id: String,
    pub event_type: String,
    pub handled: bool,
    pub duplicate: bool,
}

pub struct CommerceService {
    pub cfg: CommerceConfig,
    pub db: DbHandle,
    pub stripe: Arc<dyn StripeApi>,
    pub apps: Arc<dyn AppCaller>,
}

const ORDER_COLS: &str = "id, checkout_session_id, app_id, payment_intent_id, currency, total_minor, \
     application_fee_minor, status, app_result_json, fulfillment_error, refund_id";

fn row_to_order(r: &rusqlite::Row<'_>) -> rusqlite::Result<Order> {
    let result: Option<String> = r.get(8)?;
    Ok(Order {
        id: r.get(0)?,
        checkout_session_id: r.get(1)?,
        app_id: r.get(2)?,
        payment_intent_id: r.get(3)?,
        currency: r.get(4)?,
        total_minor: r.get(5)?,
        application_fee_minor: r.get(6)?,
        status: r.get(7)?,
        app_result: result.and_then(|s| serde_json::from_str(&s).ok()),
        fulfillment_error: r.get(9)?,
        refund_id: r.get(10)?,
    })
}

struct SessionRow {
    session: CheckoutSessionInput,
    app_id: String,
    buyer: String,
    fee: i64,
    status: String,
    payment_intent_id: Option<String>,
}

impl CommerceService {
    pub fn new(cfg: CommerceConfig, db: DbHandle, stripe: Arc<dyn StripeApi>, apps: Arc<dyn AppCaller>) -> Self {
        Self { cfg, db, stripe, apps }
    }

    // 1. Connect Express ------------------------------------------------------

    /// Create (or reuse) the Express account for `app_id` and mint an
    /// onboarding link. Only the app's registered owner can re-request.
    pub async fn register_account(
        &self,
        user_id: &str,
        app_id: &str,
        email: Option<&str>,
        country: Option<&str>,
        refresh_url: &str,
        return_url: &str,
    ) -> Result<OnboardingResult, CommerceError> {
        validate_app_id(app_id)?;
        if !is_https_url(refresh_url) || !is_https_url(return_url) {
            return Err(CommerceError::Invalid("refresh_url and return_url must be https URLs".into()));
        }
        let existing: Option<(String, String)> = {
            let conn = self.db.connect()?;
            conn.query_row(
                "SELECT owner_user_id, stripe_account_id FROM commerce_connected_accounts WHERE app_id = ?1",
                params![app_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };
        let account_id = match existing {
            Some((owner, _)) if owner != user_id => {
                return Err(CommerceError::Forbidden("this app belongs to another developer".into()))
            }
            Some((_, acct)) => acct,
            None => {
                let acct = self
                    .stripe
                    .create_express_account(email, country, &format!("allternit-commerce-acct-{app_id}"))
                    .await?;
                let conn = self.db.connect()?;
                conn.execute(
                    "INSERT OR IGNORE INTO commerce_connected_accounts (app_id, owner_user_id, stripe_account_id) \
                     VALUES (?1, ?2, ?3)",
                    params![app_id, user_id, acct.id],
                )?;
                // Idempotency key makes a lost-race retry return the same account.
                conn.query_row(
                    "SELECT stripe_account_id FROM commerce_connected_accounts WHERE app_id = ?1",
                    params![app_id],
                    |r| r.get(0),
                )?
            }
        };
        let url = self.stripe.create_account_link(&account_id, refresh_url, return_url).await?;
        Ok(OnboardingResult { app_id: app_id.to_string(), stripe_account_id: account_id, onboarding_url: url })
    }

    // 2. Sessions -------------------------------------------------------------

    /// Validate the app's session, check the merchant is this app's ready
    /// connected account, compute the platform fee, and store the host's copy.
    pub async fn open_session(
        &self,
        buyer: &str,
        app_id: &str,
        input: &CheckoutSessionInput,
    ) -> Result<StoredSession, CommerceError> {
        validate_app_id(app_id)?;
        let session = validate_session(input)?;
        let conn = self.db.connect()?;
        let acct: Option<(String, i64)> = conn
            .query_row(
                "SELECT stripe_account_id, charges_enabled FROM commerce_connected_accounts WHERE app_id = ?1",
                params![app_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (acct_id, charges_enabled) =
            acct.ok_or_else(|| CommerceError::NotFound("app has no connected Stripe account".into()))?;
        if acct_id != session.merchant.account_id {
            return Err(CommerceError::Mismatch(
                "session merchant is not this app's connected account".into(),
            ));
        }
        if charges_enabled == 0 {
            return Err(CommerceError::Conflict("merchant has not finished Stripe onboarding".into()));
        }
        let fee = self.cfg.fee_for(session.totals.total_minor);
        let json = serde_json::to_string(&session).map_err(|e| CommerceError::Db(e.to_string()))?;
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO commerce_checkout_sessions \
             (id, app_id, buyer_user_id, merchant_account, currency, subtotal_minor, tax_minor, total_minor, \
              application_fee_minor, session_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                session.id, app_id, buyer, acct_id, session.currency, session.totals.subtotal_minor,
                session.totals.tax_minor, session.totals.total_minor, fee, json
            ],
        )?;
        let row = load_session(&conn, &session.id)?.ok_or_else(|| CommerceError::NotFound("session".into()))?;
        if inserted == 0 && (row.app_id != app_id || row.buyer != buyer || row.session != session) {
            return Err(CommerceError::Conflict(
                "a different checkout session already exists with this id".into(),
            ));
        }
        Ok(StoredSession { session: row.session, application_fee_minor: row.fee, status: row.status })
    }

    // 3. Pay / complete -------------------------------------------------------

    /// After explicit user approval: charge the **stored** amount. The request
    /// carries only a session id and a Stripe payment-method id.
    pub async fn pay(
        &self,
        buyer: &str,
        session_id: &str,
        payment_method: &str,
    ) -> Result<PayOutcome, CommerceError> {
        if !payment_method.starts_with("pm_") {
            return Err(CommerceError::Invalid("payment_method must be a Stripe pm_ id".into()));
        }
        let row = self.owned_session(buyer, session_id)?;
        if row.status == "paid" {
            let conn = self.db.connect()?;
            return order_for_session(&conn, session_id)?
                .map(|order| PayOutcome::Paid { order })
                .ok_or_else(|| CommerceError::Conflict("session already paid".into()));
        }
        let req = PaymentIntentRequest {
            amount_minor: row.session.totals.total_minor,
            currency: row.session.currency.clone(),
            application_fee_minor: row.fee,
            destination_account: row.session.merchant.account_id.clone(),
            payment_method: payment_method.to_string(),
            checkout_session_id: session_id.to_string(),
            app_id: row.app_id.clone(),
        };
        // Same session ⇒ same key ⇒ Stripe returns the same intent on retry.
        let pi = self
            .stripe
            .create_payment_intent(&req, &format!("allternit-commerce-pi-{session_id}"))
            .await?;
        {
            let conn = self.db.connect()?;
            conn.execute(
                "UPDATE commerce_checkout_sessions SET payment_intent_id = ?2 WHERE id = ?1",
                params![session_id, pi.id],
            )?;
        }
        match pi.status.as_str() {
            "succeeded" => self.finalize(&row, &pi).await.map(|order| PayOutcome::Paid { order }),
            "requires_action" => Ok(PayOutcome::RequiresAction {
                payment_intent_id: pi.id.clone(),
                client_secret: pi.client_secret.clone().unwrap_or_default(),
            }),
            other => Err(CommerceError::Stripe(format!("payment not completed (status: {other})"))),
        }
    }

    /// Finish after a client-side next action (or retry a failed fulfilment):
    /// re-reads the intent from Stripe and re-checks every field against the
    /// stored session before telling the app.
    pub async fn complete(&self, buyer: &str, session_id: &str) -> Result<Order, CommerceError> {
        let row = self.owned_session(buyer, session_id)?;
        let pi_id = row
            .payment_intent_id
            .clone()
            .ok_or_else(|| CommerceError::Conflict("no payment has been started for this session".into()))?;
        let pi = self.stripe.retrieve_payment_intent(&pi_id).await?;
        if pi.status != "succeeded" {
            return Err(CommerceError::Conflict(format!("payment not completed (status: {})", pi.status)));
        }
        self.finalize(&row, &pi).await
    }

    async fn finalize(&self, row: &SessionRow, pi: &PaymentIntent) -> Result<Order, CommerceError> {
        let s = &row.session;
        // Never trust the intent blindly: it must match the stored session.
        if pi.amount_minor != s.totals.total_minor
            || pi.currency != s.currency
            || pi.destination.as_deref() != Some(s.merchant.account_id.as_str())
            || pi.application_fee_minor.unwrap_or(0) != row.fee
            || pi.checkout_session_id.as_deref() != Some(s.id.as_str())
        {
            return Err(CommerceError::Mismatch(
                "payment intent does not match the stored checkout session".into(),
            ));
        }
        let order_id = format!("ord_{}", uuid::Uuid::new_v4().simple());
        let claimed = {
            let conn = self.db.connect()?;
            conn.execute(
                "INSERT OR IGNORE INTO commerce_orders (id, checkout_session_id, app_id, buyer_user_id, \
                 merchant_account, payment_intent_id, currency, total_minor, application_fee_minor, status) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'paid')",
                params![
                    order_id, s.id, row.app_id, row.buyer, s.merchant.account_id, pi.id, s.currency,
                    s.totals.total_minor, row.fee
                ],
            )?;
            conn.execute("UPDATE commerce_checkout_sessions SET status = 'paid' WHERE id = ?1", params![s.id])?;
            // Claim the fulfilment so concurrent pay/complete calls tell the
            // app once. A stale claim (crashed process) can be re-taken.
            let n = conn.execute(
                "UPDATE commerce_orders SET status = 'fulfilling', updated_at = datetime('now') \
                 WHERE checkout_session_id = ?1 AND (status IN ('paid','fulfillment_failed') \
                   OR (status = 'fulfilling' AND updated_at < datetime('now', ?2)))",
                params![s.id, format!("-{FULFILLING_STALE_SECS} seconds")],
            )?;
            n == 1
        };
        if claimed {
            let args = json!({ "checkout_session_id": s.id, "payment_intent_id": pi.id });
            let result = self.apps.complete_checkout(&row.app_id, args).await;
            let conn = self.db.connect()?;
            match result {
                Ok(v) => conn.execute(
                    "UPDATE commerce_orders SET status='fulfilled', app_result_json=?2, fulfillment_error=NULL, \
                     updated_at=datetime('now') WHERE checkout_session_id=?1",
                    params![s.id, v.to_string()],
                )?,
                Err(e) => conn.execute(
                    "UPDATE commerce_orders SET status='fulfillment_failed', fulfillment_error=?2, \
                     updated_at=datetime('now') WHERE checkout_session_id=?1",
                    params![s.id, e],
                )?,
            };
        }
        let conn = self.db.connect()?;
        order_for_session(&conn, &s.id)?.ok_or_else(|| CommerceError::NotFound("order".into()))
    }

    fn owned_session(&self, buyer: &str, session_id: &str) -> Result<SessionRow, CommerceError> {
        let conn = self.db.connect()?;
        let row = load_session(&conn, session_id)?
            .ok_or_else(|| CommerceError::NotFound("checkout session not found".into()))?;
        // Same error for "not yours" so ids cannot be probed.
        if row.buyer != buyer {
            return Err(CommerceError::NotFound("checkout session not found".into()));
        }
        Ok(row)
    }

    // 4. Refunds --------------------------------------------------------------

    /// Host-initiated full refund. Operators only.
    pub async fn refund_order(&self, operator: &str, order_id: &str) -> Result<Order, CommerceError> {
        if !self.cfg.operator_user_ids.iter().any(|u| u == operator) {
            return Err(CommerceError::Forbidden("refunds are host-initiated by an operator".into()));
        }
        let order: Order = {
            let conn = self.db.connect()?;
            conn.query_row(
                &format!("SELECT {ORDER_COLS} FROM commerce_orders WHERE id = ?1"),
                params![order_id],
                row_to_order,
            )
            .optional()?
            .ok_or_else(|| CommerceError::NotFound("order not found".into()))?
        };
        if order.status == "refunded" {
            return Ok(order); // idempotent
        }
        let refund = self
            .stripe
            .create_full_refund(&order.payment_intent_id, &format!("allternit-commerce-refund-{}", order.id))
            .await?;
        let conn = self.db.connect()?;
        conn.execute(
            "UPDATE commerce_orders SET status='refunded', refund_id=?2, refunded_by=?3, \
             updated_at=datetime('now') WHERE id=?1",
            params![order_id, refund.id, operator],
        )?;
        conn.query_row(
            &format!("SELECT {ORDER_COLS} FROM commerce_orders WHERE id = ?1"),
            params![order_id],
            row_to_order,
        )
        .map_err(Into::into)
    }

    pub fn get_order(&self, buyer: &str, order_id: &str) -> Result<Order, CommerceError> {
        let conn = self.db.connect()?;
        conn.query_row(
            &format!("SELECT {ORDER_COLS} FROM commerce_orders WHERE id = ?1 AND buyer_user_id = ?2"),
            params![order_id, buyer],
            row_to_order,
        )
        .optional()?
        .ok_or_else(|| CommerceError::NotFound("order not found".into()))
    }

    // 5. Webhooks -------------------------------------------------------------

    /// Verify and process a Stripe webhook. Rejects when no webhook secret is
    /// configured (there is no unauthenticated fallback).
    pub fn handle_webhook(
        &self,
        signature_header: Option<&str>,
        payload: &[u8],
        now_unix: i64,
    ) -> Result<WebhookOutcome, CommerceError> {
        let secret = self
            .cfg
            .webhook_secret
            .as_deref()
            .ok_or_else(|| CommerceError::Forbidden("webhook secret is not configured".into()))?;
        let header = signature_header.ok_or_else(|| CommerceError::Forbidden("missing Stripe-Signature".into()))?;
        verify_stripe_signature(secret, header, payload, now_unix)?;

        let event: Value =
            serde_json::from_slice(payload).map_err(|e| CommerceError::Invalid(format!("bad event json: {e}")))?;
        let event_id = event["id"].as_str().ok_or_else(|| CommerceError::Invalid("event id missing".into()))?.to_string();
        let event_type = event["type"].as_str().unwrap_or_default().to_string();
        let obj = &event["data"]["object"];
        let conn = self.db.connect()?;

        let seen: bool = conn
            .query_row("SELECT 1 FROM commerce_webhook_events WHERE event_id = ?1", params![event_id], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if seen {
            return Ok(WebhookOutcome { event_id, event_type, handled: false, duplicate: true });
        }

        let handled = match event_type.as_str() {
            "account.updated" => {
                let acct = obj["id"].as_str().unwrap_or_default();
                conn.execute(
                    "UPDATE commerce_connected_accounts SET details_submitted=?2, charges_enabled=?3, \
                     payouts_enabled=?4, updated_at=datetime('now') WHERE stripe_account_id=?1",
                    params![
                        acct,
                        obj["details_submitted"].as_bool().unwrap_or(false) as i64,
                        obj["charges_enabled"].as_bool().unwrap_or(false) as i64,
                        obj["payouts_enabled"].as_bool().unwrap_or(false) as i64,
                    ],
                )?;
                true
            }
            "charge.dispute.created" | "charge.dispute.updated" | "charge.dispute.closed" => {
                let dispute_id = obj["id"].as_str().ok_or_else(|| CommerceError::Invalid("dispute id missing".into()))?;
                let pi = obj["payment_intent"].as_str();
                conn.execute(
                    "INSERT INTO commerce_disputes (id, payment_intent_id, charge_id, amount_minor, currency, reason, \
                     status, last_event_id, raw_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) \
                     ON CONFLICT(id) DO UPDATE SET status=excluded.status, reason=excluded.reason, \
                     last_event_id=excluded.last_event_id, raw_json=excluded.raw_json, updated_at=datetime('now')",
                    params![
                        dispute_id, pi, obj["charge"].as_str(), obj["amount"].as_i64(), obj["currency"].as_str(),
                        obj["reason"].as_str(), obj["status"].as_str(), event_id, obj.to_string()
                    ],
                )?;
                tracing::warn!(dispute = dispute_id, event = %event_type, "commerce dispute event");
                if event_type == "charge.dispute.created" {
                    if let Some(pi) = pi {
                        conn.execute(
                            "UPDATE commerce_orders SET status='disputed', updated_at=datetime('now') \
                             WHERE payment_intent_id=?1 AND status <> 'refunded'",
                            params![pi],
                        )?;
                    }
                }
                true
            }
            _ => false,
        };
        conn.execute(
            "INSERT OR IGNORE INTO commerce_webhook_events (event_id, event_type) VALUES (?1, ?2)",
            params![event_id, event_type],
        )?;
        Ok(WebhookOutcome { event_id, event_type, handled, duplicate: false })
    }
}

fn validate_app_id(app_id: &str) -> Result<(), CommerceError> {
    // `.` is the MCP dispatcher's server/tool separator.
    if app_id.is_empty() || app_id.len() > 100 || !app_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(CommerceError::Invalid("app_id must be 1-100 chars of [A-Za-z0-9-_]".into()));
    }
    Ok(())
}

fn load_session(conn: &rusqlite::Connection, id: &str) -> Result<Option<SessionRow>, CommerceError> {
    let raw: Option<(String, String, String, i64, String, Option<String>)> = conn
        .query_row(
            "SELECT session_json, app_id, buyer_user_id, application_fee_minor, status, payment_intent_id \
             FROM commerce_checkout_sessions WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    raw.map(|(json, app_id, buyer, fee, status, payment_intent_id)| {
        Ok(SessionRow {
            session: serde_json::from_str(&json).map_err(|e| CommerceError::Db(e.to_string()))?,
            app_id,
            buyer,
            fee,
            status,
            payment_intent_id,
        })
    })
    .transpose()
}

fn order_for_session(conn: &rusqlite::Connection, session_id: &str) -> Result<Option<Order>, CommerceError> {
    Ok(conn
        .query_row(
            &format!("SELECT {ORDER_COLS} FROM commerce_orders WHERE checkout_session_id = ?1"),
            params![session_id],
            row_to_order,
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── fee math ─────────────────────────────────────────────────────────────

    #[test]
    fn fee_math_floors_and_clamps() {
        assert_eq!(platform_fee(1000, 0, 0), 0); // default: no fee
        assert_eq!(platform_fee(1000, 250, 0), 25); // 2.5%
        assert_eq!(platform_fee(999, 250, 0), 24); // floors 24.975
        assert_eq!(platform_fee(1000, 250, 30), 55); // pct + fixed
        assert_eq!(platform_fee(100, 0, 500), 100); // never above the total
        assert_eq!(platform_fee(100, 10_000, 0), 100);
        assert_eq!(platform_fee(0, 250, 30), 0);
        assert_eq!(platform_fee(i64::MAX, 10_000, i64::MAX), i64::MAX); // no overflow
    }

    // ── live-key refusal ─────────────────────────────────────────────────────

    fn cfg_from(pairs: &'static [(&'static str, &'static str)]) -> Result<Option<CommerceConfig>, CommerceError> {
        CommerceConfig::from_lookup(|k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()))
    }

    #[test]
    fn live_and_unknown_keys_are_refused() {
        for bad in ["sk_live_abc", "rk_live_abc", "pk_live_abc", "whatever_123"] {
            let cfg = CommerceConfig::for_test(bad, 0);
            assert!(matches!(cfg, Err(CommerceError::LiveKeyRefused(_))), "{bad} must be refused");
        }
        // Live publishable key alone is refused even with no secret key.
        let r = cfg_from(&[("ALLTERNIT_COMMERCE_STRIPE_PUBLISHABLE_KEY", "pk_live_x")]);
        assert!(matches!(r, Err(CommerceError::LiveKeyRefused(_))));
        // Test secret + live publishable is refused.
        let r = cfg_from(&[
            ("ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY", "sk_test_1"),
            ("ALLTERNIT_COMMERCE_STRIPE_PUBLISHABLE_KEY", "pk_live_x"),
        ]);
        assert!(matches!(r, Err(CommerceError::LiveKeyRefused(_))));
    }

    #[test]
    fn test_keys_accepted_and_unset_is_off() {
        assert!(cfg_from(&[]).unwrap().is_none());
        let c = cfg_from(&[
            ("ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY", "sk_test_1"),
            ("ALLTERNIT_COMMERCE_STRIPE_PUBLISHABLE_KEY", "pk_test_1"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(c.platform_fee_bps, 0, "platform fee defaults to 0");
        assert_eq!(c.fee_for(5000), 0);
        // The generic STRIPE_SECRET_KEY is never consulted.
        assert!(cfg_from(&[("STRIPE_SECRET_KEY", "sk_live_zzz")]).unwrap().is_none());
    }

    // ── session validation / total mismatch ─────────────────────────────────

    fn sample() -> CheckoutSessionInput {
        CheckoutSessionInput {
            id: "cs_1".into(),
            currency: "USD".into(),
            line_items: vec![
                LineItem { id: None, name: "Pro plan".into(), quantity: 2, unit_amount_minor: 1500, total_minor: Some(3000) },
                LineItem { id: None, name: "Add-on".into(), quantity: 1, unit_amount_minor: 499, total_minor: None },
            ],
            totals: Totals { subtotal_minor: 3499, tax_minor: 280, total_minor: 3779 },
            merchant: Merchant { account_id: "acct_123".into(), name: "Acme".into() },
            links: LegalLinks {
                terms_url: "https://acme.test/terms".into(),
                refund_url: "https://acme.test/refunds".into(),
                support_url: "https://acme.test/support".into(),
                privacy_url: None,
            },
        }
    }

    #[test]
    fn valid_session_is_normalised() {
        let v = validate_session(&sample()).unwrap();
        assert_eq!(v.currency, "usd");
        assert_eq!(v.line_items[1].total_minor, Some(499));
    }

    #[test]
    fn total_mismatch_is_rejected() {
        let mut s = sample();
        s.totals.total_minor = 100; // View/app claims a cheaper total
        assert!(matches!(validate_session(&s), Err(CommerceError::Mismatch(_))));
        let mut s = sample();
        s.totals.subtotal_minor += 1;
        assert!(matches!(validate_session(&s), Err(CommerceError::Mismatch(_))));
        let mut s = sample();
        s.line_items[0].total_minor = Some(1);
        assert!(matches!(validate_session(&s), Err(CommerceError::Mismatch(_))));
    }

    #[test]
    fn malformed_sessions_are_rejected() {
        let mut s = sample();
        s.line_items[0].quantity = 0;
        assert!(validate_session(&s).is_err());
        let mut s = sample();
        s.line_items[0].unit_amount_minor = -5;
        assert!(validate_session(&s).is_err());
        let mut s = sample();
        s.links.refund_url = "http://insecure.test".into();
        assert!(validate_session(&s).is_err());
        let mut s = sample();
        s.merchant.account_id = "cus_1".into();
        assert!(validate_session(&s).is_err());
        let mut s = sample();
        s.line_items[0].quantity = i64::MAX;
        assert!(validate_session(&s).is_err());
    }

    // ── webhook signature ────────────────────────────────────────────────────

    #[test]
    fn webhook_signature_checks() {
        let body = br#"{"id":"evt_1"}"#;
        let good = sign_stripe_payload("whsec_x", 1_000, body);
        assert!(verify_stripe_signature("whsec_x", &good, body, 1_100).is_ok());
        // wrong secret, tampered body, stale timestamp, garbage header
        assert!(verify_stripe_signature("whsec_y", &good, body, 1_100).is_err());
        assert!(verify_stripe_signature("whsec_x", &good, b"{\"id\":\"evt_2\"}", 1_100).is_err());
        assert!(verify_stripe_signature("whsec_x", &good, body, 1_000 + 301).is_err());
        assert!(verify_stripe_signature("whsec_x", "nonsense", body, 1_000).is_err());
        assert!(verify_stripe_signature("whsec_x", "t=1000", body, 1_000).is_err());
        // A second, valid v1 alongside a bogus one still verifies (key rotation).
        let rotated = format!("{good},v1=deadbeef");
        assert!(verify_stripe_signature("whsec_x", &rotated, body, 1_000).is_ok());
    }

    // ── service: fakes ───────────────────────────────────────────────────────

    #[derive(Default)]
    struct FakeStripe {
        /// idempotency key → intent already created (Stripe returns the same one).
        intents: Mutex<std::collections::HashMap<String, PaymentIntent>>,
        pi_creations: Mutex<u32>,
        refunds: Mutex<Vec<String>>,
        pi_status: Mutex<Option<String>>,
        last_req: Mutex<Option<PaymentIntentRequest>>,
    }

    #[async_trait]
    impl StripeApi for FakeStripe {
        async fn create_express_account(&self, _e: Option<&str>, _c: Option<&str>, _k: &str) -> Result<StripeAccount, CommerceError> {
            Ok(StripeAccount { id: "acct_new1".into() })
        }
        async fn create_account_link(&self, a: &str, _r: &str, _u: &str) -> Result<String, CommerceError> {
            Ok(format!("https://connect.stripe.test/setup/{a}"))
        }
        async fn create_payment_intent(&self, r: &PaymentIntentRequest, key: &str) -> Result<PaymentIntent, CommerceError> {
            *self.last_req.lock().unwrap() = Some(r.clone());
            let mut m = self.intents.lock().unwrap();
            if let Some(pi) = m.get(key) {
                return Ok(pi.clone());
            }
            *self.pi_creations.lock().unwrap() += 1;
            let status = self.pi_status.lock().unwrap().clone().unwrap_or_else(|| "succeeded".into());
            let pi = PaymentIntent {
                id: format!("pi_{}", m.len() + 1),
                status,
                amount_minor: r.amount_minor,
                currency: r.currency.clone(),
                application_fee_minor: Some(r.application_fee_minor),
                destination: Some(r.destination_account.clone()),
                checkout_session_id: Some(r.checkout_session_id.clone()),
                client_secret: Some("pi_secret".into()),
            };
            m.insert(key.to_string(), pi.clone());
            Ok(pi)
        }
        async fn retrieve_payment_intent(&self, id: &str) -> Result<PaymentIntent, CommerceError> {
            let mut pi = self
                .intents
                .lock()
                .unwrap()
                .values()
                .find(|p| p.id == id)
                .cloned()
                .ok_or_else(|| CommerceError::Stripe("no such intent".into()))?;
            pi.status = "succeeded".into(); // next action completed
            Ok(pi)
        }
        async fn create_full_refund(&self, pi: &str, key: &str) -> Result<StripeRefund, CommerceError> {
            self.refunds.lock().unwrap().push(format!("{pi}|{key}"));
            Ok(StripeRefund { id: "re_1".into(), status: "succeeded".into() })
        }
    }

    #[derive(Default)]
    struct FakeApp {
        calls: Mutex<Vec<(String, Value)>>,
        fail: Mutex<bool>,
    }

    #[async_trait]
    impl AppCaller for FakeApp {
        async fn complete_checkout(&self, app_id: &str, args: Value) -> Result<Value, String> {
            self.calls.lock().unwrap().push((app_id.to_string(), args));
            if *self.fail.lock().unwrap() {
                Err("app down".into())
            } else {
                Ok(json!({"order_id": "app-order-1", "status": "confirmed"}))
            }
        }
    }

    struct Harness {
        svc: CommerceService,
        stripe: Arc<FakeStripe>,
        app: Arc<FakeApp>,
        _tmp: tempfile::TempDir,
    }

    fn harness(fee_bps: u32) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let db = DbHandle::new(tmp.path().join("commerce.db")).expect("db");
        let mut cfg = CommerceConfig::for_test("sk_test_1", fee_bps).unwrap();
        cfg.webhook_secret = Some("whsec_t".into());
        cfg.operator_user_ids = vec!["op-1".into()];
        let stripe = Arc::new(FakeStripe::default());
        let app = Arc::new(FakeApp::default());
        let svc = CommerceService::new(cfg, db, stripe.clone(), app.clone());
        Harness { svc, stripe, app, _tmp: tmp }
    }

    async fn ready_app(h: &Harness) {
        h.svc.register_account("dev-1", "acme", None, None, "https://x.test/r", "https://x.test/d").await.unwrap();
        let conn = h.svc.db.connect().unwrap();
        conn.execute("UPDATE commerce_connected_accounts SET charges_enabled=1 WHERE app_id='acme'", []).unwrap();
    }

    fn session_for(h: &Harness) -> CheckoutSessionInput {
        let _ = h;
        let mut s = sample();
        s.merchant.account_id = "acct_new1".into();
        s
    }

    // ── service: tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn onboarding_is_owner_scoped_and_reuses_account() {
        let h = harness(0);
        let r = h.svc.register_account("dev-1", "acme", Some("d@x.test"), Some("US"), "https://x.test/r", "https://x.test/d").await.unwrap();
        assert_eq!(r.stripe_account_id, "acct_new1");
        assert!(r.onboarding_url.starts_with("https://connect.stripe.test/"));
        let again = h.svc.register_account("dev-1", "acme", None, None, "https://x.test/r", "https://x.test/d").await.unwrap();
        assert_eq!(again.stripe_account_id, "acct_new1");
        let other = h.svc.register_account("dev-2", "acme", None, None, "https://x.test/r", "https://x.test/d").await;
        assert!(matches!(other, Err(CommerceError::Forbidden(_))));
        let bad = h.svc.register_account("dev-1", "a.b", None, None, "https://x.test/r", "https://x.test/d").await;
        assert!(matches!(bad, Err(CommerceError::Invalid(_))));
    }

    #[tokio::test]
    async fn open_session_requires_ready_matching_merchant_and_correct_totals() {
        let h = harness(250);
        h.svc.register_account("dev-1", "acme", None, None, "https://x.test/r", "https://x.test/d").await.unwrap();
        // not onboarded yet
        let r = h.svc.open_session("buyer-1", "acme", &session_for(&h)).await;
        assert!(matches!(r, Err(CommerceError::Conflict(_))));
        ready_app(&h).await;
        // merchant swapped to someone else's account
        let mut s = session_for(&h);
        s.merchant.account_id = "acct_evil".into();
        assert!(matches!(h.svc.open_session("buyer-1", "acme", &s).await, Err(CommerceError::Mismatch(_))));
        // total mismatch
        let mut s = session_for(&h);
        s.totals.total_minor = 1;
        assert!(matches!(h.svc.open_session("buyer-1", "acme", &s).await, Err(CommerceError::Mismatch(_))));
        // ok, fee applied
        let ok = h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        assert_eq!(ok.application_fee_minor, platform_fee(3779, 250, 0));
        // same session again is idempotent; a changed one is a conflict
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        let mut changed = session_for(&h);
        changed.line_items[1].unit_amount_minor = 1;
        changed.totals.subtotal_minor = 3001;
        changed.totals.total_minor = 3281;
        assert!(matches!(h.svc.open_session("buyer-1", "acme", &changed).await, Err(CommerceError::Conflict(_))));
    }

    #[tokio::test]
    async fn pay_charges_stored_amount_with_fee_and_destination_then_completes() {
        let h = harness(250);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        let out = h.svc.pay("buyer-1", "cs_1", "pm_card_visa").await.unwrap();
        let PayOutcome::Paid { order } = out else { panic!("expected paid") };
        assert_eq!(order.status, "fulfilled");
        assert_eq!(order.total_minor, 3779);
        assert_eq!(order.application_fee_minor, 94); // floor(3779 * 2.5%)
        assert_eq!(order.app_result.as_ref().unwrap()["order_id"], "app-order-1");
        let req = h.stripe.last_req.lock().unwrap().clone().unwrap();
        assert_eq!(req.amount_minor, 3779);
        assert_eq!(req.destination_account, "acct_new1");
        assert_eq!(req.application_fee_minor, 94);
        let calls = h.app.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "acme");
        assert_eq!(calls[0].1, json!({"checkout_session_id": "cs_1", "payment_intent_id": order.payment_intent_id}));
    }

    #[tokio::test]
    async fn pay_is_idempotent_one_intent_one_order_one_app_call() {
        let h = harness(0);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        let a = h.svc.pay("buyer-1", "cs_1", "pm_card_visa").await.unwrap();
        let b = h.svc.pay("buyer-1", "cs_1", "pm_card_visa").await.unwrap();
        let (PayOutcome::Paid { order: a }, PayOutcome::Paid { order: b }) = (a, b) else { panic!() };
        assert_eq!(a.id, b.id);
        assert_eq!(*h.stripe.pi_creations.lock().unwrap(), 1);
        assert_eq!(h.app.calls.lock().unwrap().len(), 1);
        let n: i64 = h.svc.db.connect().unwrap().query_row("SELECT COUNT(*) FROM commerce_orders", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // complete() after success does not re-notify the app either
        h.svc.complete("buyer-1", "cs_1").await.unwrap();
        assert_eq!(h.app.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pay_uses_stable_idempotency_key_per_session() {
        // Recreate the intent through the fake with a wiped order table to show
        // Stripe-side dedupe is keyed on the session, not the request.
        let h = harness(0);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        h.svc.pay("buyer-1", "cs_1", "pm_card_visa").await.unwrap();
        let keys: Vec<String> = h.stripe.intents.lock().unwrap().keys().cloned().collect();
        assert_eq!(keys, vec!["allternit-commerce-pi-cs_1".to_string()]);
    }

    #[tokio::test]
    async fn failed_fulfilment_is_recorded_and_retryable_via_complete() {
        let h = harness(0);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        *h.app.fail.lock().unwrap() = true;
        let PayOutcome::Paid { order } = h.svc.pay("buyer-1", "cs_1", "pm_card_visa").await.unwrap() else { panic!() };
        assert_eq!(order.status, "fulfillment_failed");
        assert_eq!(order.fulfillment_error.as_deref(), Some("app down"));
        *h.app.fail.lock().unwrap() = false;
        let retried = h.svc.complete("buyer-1", "cs_1").await.unwrap();
        assert_eq!(retried.status, "fulfilled");
        assert_eq!(h.app.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn requires_action_returns_secret_then_complete_finishes() {
        let h = harness(0);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        *h.stripe.pi_status.lock().unwrap() = Some("requires_action".into());
        let out = h.svc.pay("buyer-1", "cs_1", "pm_card_authenticationRequired").await.unwrap();
        assert!(matches!(out, PayOutcome::RequiresAction { .. }));
        assert_eq!(h.app.calls.lock().unwrap().len(), 0, "app is not told before payment succeeds");
        let order = h.svc.complete("buyer-1", "cs_1").await.unwrap();
        assert_eq!(order.status, "fulfilled");
    }

    #[tokio::test]
    async fn other_buyers_cannot_pay_or_read_a_session() {
        let h = harness(0);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        assert!(matches!(h.svc.pay("buyer-2", "cs_1", "pm_x").await, Err(CommerceError::NotFound(_))));
        assert!(matches!(h.svc.pay("buyer-1", "cs_1", "card_number").await, Err(CommerceError::Invalid(_))));
    }

    #[tokio::test]
    async fn refund_is_operator_only_full_and_idempotent() {
        let h = harness(0);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        let PayOutcome::Paid { order } = h.svc.pay("buyer-1", "cs_1", "pm_card_visa").await.unwrap() else { panic!() };
        assert!(matches!(h.svc.refund_order("buyer-1", &order.id).await, Err(CommerceError::Forbidden(_))));
        let r = h.svc.refund_order("op-1", &order.id).await.unwrap();
        assert_eq!(r.status, "refunded");
        assert_eq!(r.refund_id.as_deref(), Some("re_1"));
        h.svc.refund_order("op-1", &order.id).await.unwrap();
        assert_eq!(h.stripe.refunds.lock().unwrap().len(), 1, "second refund is a no-op");
        assert!(h.stripe.refunds.lock().unwrap()[0].ends_with(&format!("allternit-commerce-refund-{}", order.id)));
        assert!(matches!(h.svc.refund_order("op-1", "ord_missing").await, Err(CommerceError::NotFound(_))));
    }

    #[tokio::test]
    async fn webhook_rejects_bad_signature_and_processes_account_and_disputes_once() {
        let h = harness(0);
        ready_app(&h).await;
        h.svc.open_session("buyer-1", "acme", &session_for(&h)).await.unwrap();
        let PayOutcome::Paid { order } = h.svc.pay("buyer-1", "cs_1", "pm_card_visa").await.unwrap() else { panic!() };
        let now = 5_000;

        let acct_evt = json!({"id":"evt_a","type":"account.updated","data":{"object":{
            "id":"acct_new1","details_submitted":true,"charges_enabled":false,"payouts_enabled":true}}})
            .to_string();
        // forged / missing / unsigned
        assert!(h.svc.handle_webhook(Some("t=5000,v1=00"), acct_evt.as_bytes(), now).is_err());
        assert!(h.svc.handle_webhook(None, acct_evt.as_bytes(), now).is_err());
        let sig = sign_stripe_payload("whsec_t", now, acct_evt.as_bytes());
        let out = h.svc.handle_webhook(Some(&sig), acct_evt.as_bytes(), now).unwrap();
        assert!(out.handled && !out.duplicate);
        let (ce, po): (i64, i64) = h.svc.db.connect().unwrap()
            .query_row("SELECT charges_enabled, payouts_enabled FROM commerce_connected_accounts WHERE app_id='acme'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((ce, po), (0, 1));
        // replay of the same event id is a no-op
        assert!(h.svc.handle_webhook(Some(&sig), acct_evt.as_bytes(), now).unwrap().duplicate);

        let dispute = json!({"id":"evt_d","type":"charge.dispute.created","data":{"object":{
            "id":"dp_1","charge":"ch_1","payment_intent":order.payment_intent_id,"amount":3779,
            "currency":"usd","reason":"fraudulent","status":"needs_response"}}})
            .to_string();
        let sig = sign_stripe_payload("whsec_t", now, dispute.as_bytes());
        h.svc.handle_webhook(Some(&sig), dispute.as_bytes(), now).unwrap();
        let conn = h.svc.db.connect().unwrap();
        let reason: String = conn.query_row("SELECT reason FROM commerce_disputes WHERE id='dp_1'", [], |r| r.get(0)).unwrap();
        assert_eq!(reason, "fraudulent");
        let st: String = conn.query_row("SELECT status FROM commerce_orders WHERE id=?1", params![order.id], |r| r.get(0)).unwrap();
        assert_eq!(st, "disputed");
    }

    #[tokio::test]
    async fn webhook_without_configured_secret_is_rejected() {
        let mut h = harness(0);
        h.svc.cfg.webhook_secret = None;
        let body = b"{}";
        let sig = sign_stripe_payload("", 1, body);
        assert!(matches!(h.svc.handle_webhook(Some(&sig), body, 1), Err(CommerceError::Forbidden(_))));
    }
}
