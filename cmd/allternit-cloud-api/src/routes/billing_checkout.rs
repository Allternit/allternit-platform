//! Prepaid credit packs and Stripe Checkout session creation.
//!
//! `GET /api/v1/billing/packs` is public and returns the static credit pack catalog
//! (credits_10 / credits_25 / credits_50 / credits_100 at a 1:1 USD-to-credit price).
//!
//! `POST /api/v1/billing/checkout` is authenticated (Clerk session or API token) and creates a Stripe Checkout Session
//! for a single pack purchase (mode = payment) via the Stripe REST API. The secret key comes from
//! STRIPE_SECRET_KEY; when it is unset the endpoint answers 503 billing_not_configured like the
//! other operator-gated billing surfaces (mesh, webhooks), so a misconfigured deployment is visible
//! instead of a broken redirect. Success/cancel URLs default to the platform billing page and can
//! be overridden with STRIPE_CHECKOUT_SUCCESS_URL / STRIPE_CHECKOUT_CANCEL_URL.
//!
//! Embedded checkout (opt-in): a body with `"ui_mode": "embedded"` on `/checkout`
//! (and `/subscribe` in `routes::billing_subscriptions`) creates the session with
//! Stripe `ui_mode=embedded`, a `return_url` (the success page plus
//! `session_id={CHECKOUT_SESSION_ID}`) and `redirect_on_completion=if_required`,
//! and answers `{ client_secret, session_id }` for Stripe.js to mount in the app.
//! Without the field (or with `"hosted"`) the reply is the hosted
//! `{ checkout_url }` exactly as before, so released apps keep working. The
//! Stripe customer always comes from the signed-in account, never the body.
//!
//! `GET /api/v1/billing/config` is public and returns what Stripe.js needs:
//! `{ publishable_key, mode: "test" | "live", payment_method_types: ["card", "link"] }`.
//! The key is STRIPE_PUBLISHABLE_KEY; the mode comes from its prefix and must
//! match STRIPE_SECRET_KEY's, otherwise the route answers 503
//! billing_not_configured. Card and Link only: no wallets.
//!
//! The Checkout Session metadata carries the credit-grant contract that
//! routes::billing_webhooks consumes when the payment completes
//! (checkout.session.completed / checkout.session.async_payment_succeeded
//! with mode = payment and payment_status = paid):
//! - `clerk_user_id` — the Clerk user that owns the purchase.
//! - `allternit_credits_usd` — the credit amount (two-decimal USD), from the
//!   static server-side catalog never from the client.
//! - `allternit_org_id` — the Clerk organization selected on the session,
//!   i.e. the org the fabric ledger grant targets. Captured here because the
//!   webhook has no Clerk session of its own. When the fabric-ledger bridge
//!   is configured (ALLTERNIT_FABRIC_LEDGER_URL) an org is required: the
//!   endpoint answers 400 rather than selling credits that cannot be routed.
//!
//! The two files define the two halves of the metadata contract; keep them
//! in sync.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::{error::ApiError, ApiState};

const STRIPE_CHECKOUT_SESSIONS_URL: &str = "https://api.stripe.com/v1/checkout/sessions";
const STRIPE_INVOICE_ITEMS_URL: &str = "https://api.stripe.com/v1/invoiceitems";
const STRIPE_PORTAL_SESSIONS_URL: &str = "https://api.stripe.com/v1/billing_portal/sessions";
pub(crate) const DEFAULT_SUCCESS_URL: &str = "https://platform.allternit.com/billing?checkout=success";
pub(crate) const DEFAULT_CANCEL_URL: &str = "https://platform.allternit.com/billing?checkout=cancelled";

#[derive(Debug, Clone, Copy)]
pub struct CreditPack {
    pub id: &'static str,
    pub label: &'static str,
    pub credits_usd: f64,
    /// App Store consumable product id (permanent once created in App Store Connect).
    pub apple_product_id: &'static str,
    /// Google Play one-time product id (permanent once created in the Play Console).
    pub google_product_id: &'static str,
    /// The store's US reference price. The pack grants the same `credits_usd` as
    /// web; the store price is higher to cover the store fee. Clients show the
    /// store's own localized price, never this figure.
    pub store_price_usd: f64,
}

const CREDIT_PACKS: &[CreditPack] = &[
    CreditPack { id: "credits_10", label: "$10", credits_usd: 10.00, apple_product_id: "com.allternit.mobile.credits.10", google_product_id: "credits_10", store_price_usd: 11.99 },
    CreditPack { id: "credits_25", label: "$25", credits_usd: 25.00, apple_product_id: "com.allternit.mobile.credits.25", google_product_id: "credits_25", store_price_usd: 29.99 },
    CreditPack { id: "credits_50", label: "$50", credits_usd: 50.00, apple_product_id: "com.allternit.mobile.credits.50", google_product_id: "credits_50", store_price_usd: 59.99 },
    CreditPack { id: "credits_100", label: "$100", credits_usd: 100.00, apple_product_id: "com.allternit.mobile.credits.100", google_product_id: "credits_100", store_price_usd: 119.99 },
];

#[derive(Debug, Serialize)]
pub struct CreditPackResponse {
    id: &'static str,
    label: &'static str,
    credits_usd: f64,
    price_usd: f64,
}

/// One `GET /api/v1/billing/packs` item: the pack plus where the apps can buy
/// it (`store`, additive: older clients ignore it). The Cloud credits 402 body
/// keeps the plain [`CreditPackResponse`].
#[derive(Debug, Serialize)]
pub struct PackListing {
    #[serde(flatten)]
    pack: CreditPackResponse,
    store: crate::services::store_billing::StoreOffers,
}

#[derive(Debug, Serialize)]
pub struct PacksResponse {
    packs: Vec<PackListing>,
}

/// The body names only the pack and the ui mode. There is no customer field:
/// the Stripe customer is always resolved from the signed-in account, and
/// unknown fields (`customer`, `customer_email`, ...) are ignored.
#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pack_id: String,
    #[serde(default)]
    ui_mode: Option<UiMode>,
}

/// How the client shows Stripe Checkout: `hosted` (a redirect to
/// checkout.stripe.com, the default) or `embedded` (Stripe.js mounts the form
/// in the app from a client secret).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiMode {
    #[default]
    Hosted,
    Embedded,
}

impl UiMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Embedded => "embedded",
        }
    }
}

/// The only payment methods Allternit Cloud checkout offers (decision E4):
/// pay-now card and Link. No wallets, no buy-now-pay-later, no delayed methods,
/// since credits and plans are granted on payment. `GET /billing/config`
/// reports the same list the sessions send.
pub(crate) const PAYMENT_METHOD_TYPES: [&str; 2] = ["card", "link"];

/// `payment_method_types[i]` form fields for a Checkout Session.
pub(crate) fn payment_method_fields() -> Vec<(String, String)> {
    PAYMENT_METHOD_TYPES
        .iter()
        .enumerate()
        .map(|(index, method)| (format!("payment_method_types[{index}]"), method.to_string()))
        .collect()
}

/// Where Stripe sends the buyer after checkout, per ui mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckoutReturn {
    /// Hosted page: Stripe redirects to one of the two pages.
    Hosted { success_url: String, cancel_url: String },
    /// Embedded form: no cancel page (closing the sheet is the cancel). Stripe
    /// redirects to `return_url` only when a method needs a redirect.
    Embedded { return_url: String },
}

impl CheckoutReturn {
    /// The return pages for a ui mode: the client's validated page, else env,
    /// else the platform billing page (see [`checkout_return_urls`]).
    pub(crate) fn for_mode(ui_mode: UiMode, return_to: &ReturnTo) -> Result<Self, ApiError> {
        let (success_url, cancel_url) = checkout_return_urls(return_to)?;
        Ok(match ui_mode {
            UiMode::Hosted => Self::Hosted { success_url, cancel_url },
            UiMode::Embedded => Self::Embedded { return_url: embedded_return_url(&success_url) },
        })
    }

    pub(crate) fn ui_mode(&self) -> UiMode {
        match self {
            Self::Hosted { .. } => UiMode::Hosted,
            Self::Embedded { .. } => UiMode::Embedded,
        }
    }

    /// The Checkout Session form fields for this return. Stripe rejects
    /// `success_url` / `cancel_url` on an embedded session, so each mode sends
    /// only its own fields.
    pub(crate) fn form_fields(&self) -> Vec<(String, String)> {
        match self {
            Self::Hosted { success_url, cancel_url } => vec![
                ("success_url".to_string(), success_url.clone()),
                ("cancel_url".to_string(), cancel_url.clone()),
            ],
            Self::Embedded { return_url } => vec![
                ("ui_mode".to_string(), "embedded".to_string()),
                ("return_url".to_string(), return_url.clone()),
                // Card and Link finish in place (Stripe.js `onComplete`), so the
                // app refreshes the plan and credits without leaving the sheet.
                ("redirect_on_completion".to_string(), "if_required".to_string()),
            ],
        }
    }
}

/// The success page plus Stripe's `{CHECKOUT_SESSION_ID}` placeholder.
fn embedded_return_url(success_url: &str) -> String {
    if success_url.contains("{CHECKOUT_SESSION_ID}") {
        return success_url.to_string();
    }
    let separator = if success_url.contains('?') { '&' } else { '?' };
    format!("{success_url}{separator}session_id={{CHECKOUT_SESSION_ID}}")
}

/// A created Checkout Session, as the route returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckoutSession {
    Hosted { url: String },
    Embedded { client_secret: String, session_id: String },
}

/// `{ client_secret, session_id }`: the embedded reply Stripe.js mounts from.
#[derive(Debug, Serialize)]
struct EmbeddedCheckoutResponse {
    client_secret: String,
    session_id: String,
}

/// The JSON reply for a created session. Hosted keeps today's
/// `{ checkout_url }` exactly (packs and plans share it).
pub(crate) fn checkout_session_response(session: CheckoutSession) -> Response {
    match session {
        CheckoutSession::Hosted { url } => Json(CheckoutResponse { checkout_url: url }).into_response(),
        CheckoutSession::Embedded { client_secret, session_id } => {
            Json(EmbeddedCheckoutResponse { client_secret, session_id }).into_response()
        }
    }
}

/// Create the Checkout Session from a finished form: hosted returns the URL,
/// embedded returns the session's client secret and id.
pub(crate) async fn open_checkout_session(
    checkout: &dyn StripeCheckout,
    secret_key: &str,
    form: &[(String, String)],
    ui_mode: UiMode,
) -> Result<CheckoutSession, ApiError> {
    match ui_mode {
        UiMode::Hosted => checkout
            .create_checkout_session(secret_key, form)
            .await
            .map(|url| CheckoutSession::Hosted { url }),
        UiMode::Embedded => {
            let session = checkout.create_checkout_session_object(secret_key, form).await?;
            let client_secret = session["client_secret"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ApiError::Internal("Stripe returned no client secret".to_string()))?;
            let session_id = session["id"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ApiError::Internal("Stripe returned no session id".to_string()))?;
            Ok(CheckoutSession::Embedded {
                client_secret: client_secret.to_string(),
                session_id: session_id.to_string(),
            })
        }
    }
}

/// `GET /api/v1/billing/config`: what Stripe.js needs to mount checkout.
#[derive(Debug, Serialize, PartialEq)]
pub struct BillingConfigResponse {
    publishable_key: String,
    /// `test` or `live`, from the key prefix.
    mode: &'static str,
    payment_method_types: [&'static str; 2],
}

/// `test` / `live` from a Stripe key prefix (`pk_`, `sk_` or restricted `rk_`).
fn stripe_key_mode(key: &str, prefixes: &[&str]) -> Option<&'static str> {
    let key = key.trim();
    for prefix in prefixes {
        if key.starts_with(&format!("{prefix}_test_")) {
            return Some("test");
        }
        if key.starts_with(&format!("{prefix}_live_")) {
            return Some("live");
        }
    }
    None
}

/// The billing config from the two keys, or `None` when it is not usable:
/// a missing or malformed key, or a publishable key whose mode differs from
/// the secret key's (Stripe.js would fail to mount the session).
fn billing_config_from(publishable_key: Option<&str>, secret_key: Option<&str>) -> Option<BillingConfigResponse> {
    let publishable_key = publishable_key.map(str::trim).filter(|key| !key.is_empty())?;
    let mode = stripe_key_mode(publishable_key, &["pk"])?;
    let secret_mode = stripe_key_mode(secret_key?, &["sk", "rk"])?;
    if mode != secret_mode {
        tracing::warn!(publishable = mode, secret = secret_mode, "Stripe publishable and secret keys are in different modes");
        return None;
    }
    Some(BillingConfigResponse {
        publishable_key: publishable_key.to_string(),
        mode,
        payment_method_types: PAYMENT_METHOD_TYPES,
    })
}

async fn billing_config() -> Response {
    let publishable_key = std::env::var("STRIPE_PUBLISHABLE_KEY").ok();
    let secret_key = std::env::var("STRIPE_SECRET_KEY").ok();
    match billing_config_from(publishable_key.as_deref(), secret_key.as_deref()) {
        Some(config) => Json(config).into_response(),
        None => billing_not_configured_response(),
    }
}

/// Optional `?return_url=` on checkout, subscribe and portal: the page Stripe
/// sends the buyer back to. Several surfaces sell through these routes
/// (platform.allternit.com/billing, ai.allternit.com Settings → Billing), so
/// each names its own page; absent, the env/default URLs apply.
#[derive(Debug, Default, Deserialize)]
pub struct ReturnTo {
    #[serde(default)]
    pub return_url: Option<String>,
}

/// Hosts a client may name in `return_url`. `ALLTERNIT_BILLING_RETURN_HOSTS`
/// (comma-separated) replaces the list.
const DEFAULT_RETURN_HOSTS: &[&str] = &["platform.allternit.com", "ai.allternit.com", "bots.allternit.com", "m.allternit.com"];

/// Validate a client-supplied return page: https, an allowed host, no query
/// or fragment (the server adds `?checkout=`). `Ok(None)` when absent.
pub(crate) fn client_return_url(requested: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(raw) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let invalid = || ApiError::BadRequest("return_url must be an https page on an Allternit site.".to_string());
    let url = reqwest::Url::parse(raw).map_err(|_| invalid())?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = match std::env::var("ALLTERNIT_BILLING_RETURN_HOSTS") {
        Ok(list) if !list.trim().is_empty() => list
            .split(',')
            .map(|item| item.trim().to_ascii_lowercase())
            .any(|item| !item.is_empty() && item == host),
        _ => DEFAULT_RETURN_HOSTS.iter().any(|item| *item == host),
    };
    if url.scheme() != "https"
        || !allowed
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(Some(url.to_string()))
}

/// `base` plus `checkout=<outcome>` for the Checkout success/cancel URLs.
pub(crate) fn with_checkout_outcome(base: &str, outcome: &str) -> String {
    format!("{base}?checkout={outcome}")
}

/// Success and cancel URLs: the client's validated page, else env, else default.
pub(crate) fn checkout_return_urls(return_to: &ReturnTo) -> Result<(String, String), ApiError> {
    if let Some(base) = client_return_url(return_to.return_url.as_deref())? {
        return Ok((with_checkout_outcome(&base, "success"), with_checkout_outcome(&base, "cancelled")));
    }
    Ok((
        std::env::var("STRIPE_CHECKOUT_SUCCESS_URL").unwrap_or_else(|_| DEFAULT_SUCCESS_URL.to_string()),
        std::env::var("STRIPE_CHECKOUT_CANCEL_URL").unwrap_or_else(|_| DEFAULT_CANCEL_URL.to_string()),
    ))
}

#[derive(Debug, Serialize)]
pub struct CheckoutResponse {
    checkout_url: String,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/billing/packs", get(list_packs))
        .route("/api/v1/billing/checkout", post(create_checkout))
        .route("/api/v1/billing/config", get(billing_config))
}

/// The credit pack catalog as `GET /api/v1/billing/packs` returns it. The
/// Cloud credits 402 body (`inference_settlement::credits_required_body`)
/// carries the same list.
pub(crate) fn credit_pack_catalog() -> Vec<CreditPackResponse> {
    CREDIT_PACKS.iter().map(pack_response).collect()
}

fn pack_response(pack: &CreditPack) -> CreditPackResponse {
    CreditPackResponse {
        id: pack.id,
        label: pack.label,
        credits_usd: pack.credits_usd,
        price_usd: pack.credits_usd,
    }
}

async fn list_packs() -> Json<PacksResponse> {
    let config = crate::services::store_billing::StoreConfig::from_env();
    Json(packs_response(&config))
}

fn packs_response(config: &crate::services::store_billing::StoreConfig) -> PacksResponse {
    PacksResponse {
        packs: CREDIT_PACKS
            .iter()
            .map(|pack| PackListing {
                pack: pack_response(pack),
                store: config.offers(pack.apple_product_id, pack.google_product_id, pack.store_price_usd),
            })
            .collect(),
    }
}

async fn create_checkout(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(return_to): Query<ReturnTo>,
    Json(request): Json<CheckoutRequest>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let user_id = user.id;
    let organization_id = user.organization_id;
    let Some(pack) = find_pack(&request.pack_id) else {
        return ApiError::BadRequest(format!("Unknown credit pack: {:?}.", request.pack_id)).into_response();
    };
    // Chargeback hold: untrusted buyers are limited to small packs while their
    // first payment settles; larger packs unlock automatically afterwards.
    let trusted = match crate::routes::billing_subscriptions::is_trusted_purchaser(&state.db, &user_id).await {
        Ok(trusted) => trusted,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = crate::routes::billing_subscriptions::ensure_purchase_allowed(
        trusted,
        pack.credits_usd,
        "The $50 and $100 packs",
    ) {
        return error.into_response();
    }
    // When the fabric-ledger bridge is configured every credit grant must be
    // org-routable at webhook time (the webhook has no Clerk session of its
    // own to resolve an org from). Refuse to sell an unrouteable pack
    // instead of taking money the ledger can never deliver.
    if crate::services::fabric_ledger::fabric_bridge_configured() && organization_id.is_none() {
        return ApiError::BadRequest(
            "Credit purchases require an active Clerk organization: sign in again with an organization selected."
                .to_string(),
        )
        .into_response();
    }
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let ui_mode = request.ui_mode.unwrap_or_default();
    let checkout_return = match CheckoutReturn::for_mode(ui_mode, &return_to) {
        Ok(checkout_return) => checkout_return,
        Err(error) => return error.into_response(),
    };
    let checkout = ReqwestStripeCheckout::new();
    // One Stripe customer per person, so the pack saves its card for auto
    // top-up (setup_future_usage). A failure here must not stop the sale:
    // the pack then sells without a customer, as before.
    let customer_id = match crate::services::auto_topup::ensure_customer(
        &state.db,
        &checkout,
        &secret_key,
        &user_id,
        user.email.as_deref(),
    )
    .await
    {
        Ok(customer_id) => Some(customer_id),
        Err(error) => {
            tracing::warn!(%user_id, %error, "credit pack checkout without a Stripe customer");
            None
        }
    };

    match create_pack_checkout(
        &checkout,
        &secret_key,
        pack,
        &user_id,
        organization_id.as_deref(),
        customer_id.as_deref(),
        &checkout_return,
    )
    .await
    {
        Ok(session) => {
            crate::services::audit::write_audit_log(
                &state.db,
                crate::services::audit::AuditEvent {
                    action: "billing.checkout.created".to_string(),
                    resource_type: "billing_checkout".to_string(),
                    resource_id: Some(request.pack_id.clone()),
                    user_id: Some(user_id.clone()),
                    user_email: None,
                    details: Some(serde_json::json!({
                        "pack_id": request.pack_id,
                        "credits_usd": pack.credits_usd,
                        "ui_mode": ui_mode.as_str(),
                    })),
                    success: true,
                },
            )
            .await;
            checkout_session_response(session)
        }
        Err(error) => billing_upstream_error_response(&error),
    }
}

pub(crate) fn billing_not_configured_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": "billing_not_configured" })),
    )
        .into_response()
}

pub(crate) fn billing_upstream_error_response(error: &ApiError) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": "billing_upstream_error",
            "message": error.to_string(),
        })),
    )
        .into_response()
}

/// The pack sizes in USD; auto top-up amounts must be one of them.
pub(crate) fn pack_sizes_usd() -> Vec<f64> {
    CREDIT_PACKS.iter().map(|pack| pack.credits_usd).collect()
}

/// Look up a pack in the static catalog. Unknown ids are a 400: the client asked for a pack the
/// server does not sell, and charging a wrong amount silently is worse than failing fast.
pub(crate) fn find_pack(pack_id: &str) -> Option<&'static CreditPack> {
    CREDIT_PACKS.iter().find(|pack| pack.id == pack_id)
}

/// Every pack in the catalog (store product id lookups in `services::store_billing`).
pub(crate) fn credit_packs() -> &'static [CreditPack] {
    CREDIT_PACKS
}

/// The slice of the Stripe API this surface needs, abstracted so the orchestration is testable
/// without a reachable Stripe API. Implementations must scrub the secret key from error messages
/// (Stripe error payloads can echo request parameters back).
#[async_trait::async_trait]
pub trait StripeCheckout: Send + Sync {
    /// Create a Checkout Session from the pre-encoded form fields and return the hosted session URL.
    async fn create_checkout_session(
        &self,
        secret_key: &str,
        form: &[(String, String)],
    ) -> Result<String, ApiError>;

    /// Create a Checkout Session and return the whole session object. Embedded
    /// checkout needs its `client_secret` and `id` (a hosted session's `url` is
    /// null there). Defaults to [`StripeCheckout::post_object`] on
    /// `/v1/checkout/sessions`.
    async fn create_checkout_session_object(
        &self,
        secret_key: &str,
        form: &[(String, String)],
    ) -> Result<Value, ApiError> {
        self.post_object(secret_key, "/v1/checkout/sessions", None, form).await
    }

    /// Create a Billing Portal Session from the pre-encoded form fields and return the portal URL.
    async fn create_billing_portal_session(
        &self,
        secret_key: &str,
        form: &[(String, String)],
    ) -> Result<String, ApiError>;

    /// Create a Stripe invoice item (`POST /v1/invoiceitems`) from the pre-encoded form fields and
    /// return its id. `idempotency_key` makes a retry return the same item instead of a second
    /// charge. Used by the admin-approved voice overage billing; not available on every impl.
    async fn create_invoice_item(
        &self,
        _secret_key: &str,
        _idempotency_key: &str,
        _form: &[(String, String)],
    ) -> Result<String, ApiError> {
        Err(ApiError::Internal("invoice items are not supported by this Stripe client".to_string()))
    }

    /// `POST https://api.stripe.com{path}` with form fields; returns the created object.
    /// Used by embedded checkout sessions ([`StripeCheckout::create_checkout_session_object`])
    /// and by the Platform API billing plan and meter reporter, which run
    /// only when their off-by-default flags are on (`platform_v1::stripe_plan`).
    async fn post_object(
        &self,
        _secret_key: &str,
        _path: &str,
        _idempotency_key: Option<&str>,
        _form: &[(String, String)],
    ) -> Result<Value, ApiError> {
        Err(ApiError::Internal("this Stripe client can't create objects".to_string()))
    }

    /// `GET https://api.stripe.com{path}` with query fields; returns the object
    /// (Platform API billing: price lookup by `lookup_key`).
    async fn get_object(&self, _secret_key: &str, _path: &str, _query: &[(String, String)]) -> Result<Value, ApiError> {
        Err(ApiError::Internal("this Stripe client can't read objects".to_string()))
    }

    /// `DELETE https://api.stripe.com{path}` with form fields (Platform API
    /// billing: cancel a project's previous subscription on a plan change).
    async fn delete_object(&self, _secret_key: &str, _path: &str, _form: &[(String, String)]) -> Result<Value, ApiError> {
        Err(ApiError::Internal("this Stripe client can't delete objects".to_string()))
    }
}

/// Build the Checkout Session for a validated pack purchase: the hosted URL,
/// or the embedded client secret and session id.
async fn create_pack_checkout(
    checkout: &dyn StripeCheckout,
    secret_key: &str,
    pack: &CreditPack,
    clerk_user_id: &str,
    organization_id: Option<&str>,
    customer_id: Option<&str>,
    checkout_return: &CheckoutReturn,
) -> Result<CheckoutSession, ApiError> {
    let form = checkout_form_params(pack, clerk_user_id, organization_id, customer_id, checkout_return);
    open_checkout_session(checkout, secret_key, &form, checkout_return.ui_mode()).await
}

/// Form fields for POST /v1/checkout/sessions: one payment-mode line item at the pack price, with the
/// credit-grant metadata contract on the session object itself — routes::billing_webhooks reads
/// clerk_user_id, allternit_credits_usd, and allternit_org_id out of
/// event.data.object.metadata for completed payment-mode sessions, so those
/// names must stay stable.
///
/// With the person's Stripe customer, the session also saves the card for
/// off-session use (`payment_intent_data[setup_future_usage]=off_session`),
/// which auto top-up charges later (`services::auto_topup`).
fn checkout_form_params(
    pack: &CreditPack,
    clerk_user_id: &str,
    organization_id: Option<&str>,
    customer_id: Option<&str>,
    checkout_return: &CheckoutReturn,
) -> Vec<(String, String)> {
    let mut form = vec![("mode".to_string(), "payment".to_string())];
    // Pay-now methods only, as for plans: credits are granted on payment,
    // so no buy-now-pay-later or delayed methods.
    form.extend(payment_method_fields());
    form.extend(checkout_return.form_fields());
    form.extend([
        (
            "line_items[0][price_data][currency]".to_string(),
            "usd".to_string(),
        ),
        (
            "line_items[0][price_data][unit_amount]".to_string(),
            ((pack.credits_usd * 100.0).round() as u64).to_string(),
        ),
        (
            "line_items[0][price_data][product_data][name]".to_string(),
            format!("Allternit Cloud credits — {}", pack.label),
        ),
        ("line_items[0][quantity]".to_string(), "1".to_string()),
        (
            "metadata[clerk_user_id]".to_string(),
            clerk_user_id.to_string(),
        ),
        (
            "metadata[allternit_credits_usd]".to_string(),
            format!("{:.2}", pack.credits_usd),
        ),
    ]);
    // The same contract on the PaymentIntent behind the session, so a
    // chargeback (`charge.dispute.created`, which names the intent, not the
    // session) can be traced back to the person. `kind` tells packs apart
    // from auto top-ups (`kind=auto_topup`, which grant from the intent).
    form.push((
        "payment_intent_data[metadata][clerk_user_id]".to_string(),
        clerk_user_id.to_string(),
    ));
    form.push((
        "payment_intent_data[metadata][allternit_credits_usd]".to_string(),
        format!("{:.2}", pack.credits_usd),
    ));
    form.push((
        "payment_intent_data[metadata][kind]".to_string(),
        "credit_pack".to_string(),
    ));
    // Org routing for the webhook grant. Only written when the session has an
    // active Clerk org — absent means the bridge was off at purchase time and
    // the grant falls back to the cloud wallet.
    if let Some(organization_id) = organization_id {
        form.push((
            "metadata[allternit_org_id]".to_string(),
            organization_id.to_string(),
        ));
        form.push((
            "payment_intent_data[metadata][allternit_org_id]".to_string(),
            organization_id.to_string(),
        ));
    }
    if let Some(customer_id) = customer_id {
        form.push(("customer".to_string(), customer_id.to_string()));
        form.push((
            "payment_intent_data[setup_future_usage]".to_string(),
            "off_session".to_string(),
        ));
    }
    form
}

/// The production StripeCheckout: POSTs the form to the Stripe REST API with the secret key as the
/// basic-auth username (empty password, per Stripe's auth convention) and returns the hosted session
/// URL. The secret key never appears in error text.
pub struct ReqwestStripeCheckout;

impl ReqwestStripeCheckout {
    pub fn new() -> Self {
        Self
    }

    async fn post_form(&self, url: &str, secret_key: &str, form: &[(String, String)]) -> Result<String, ApiError> {
        let body = self.post_form_value(url, secret_key, None, form).await?;
        body["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ApiError::Internal("Stripe returned no session URL".to_string()))
    }

    async fn post_form_value(
        &self,
        url: &str,
        secret_key: &str,
        idempotency_key: Option<&str>,
        form: &[(String, String)],
    ) -> Result<Value, ApiError> {
        let mut request = crate::net_client::client()
            .post(url)
            .basic_auth(secret_key, None::<&str>)
            .form(form);
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        Self::send(request).await
    }

    async fn send(request: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let response = request
            .send()
            .await
            .map_err(|error| ApiError::Internal(format!("Failed to reach Stripe: {error}")))?;
        if !response.status().is_success() {
            let body = response.json::<Value>().await.ok();
            let message = body
                .and_then(|body| body["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| "Unknown Stripe error".to_string());
            return Err(ApiError::Internal(format!("Stripe rejected the session: {message}")));
        }
        response
            .json::<Value>()
            .await
            .map_err(|error| ApiError::Internal(format!("Failed to parse the Stripe response: {error}")))
    }
}

#[async_trait::async_trait]
impl StripeCheckout for ReqwestStripeCheckout {
    async fn create_checkout_session(
        &self,
        secret_key: &str,
        form: &[(String, String)],
    ) -> Result<String, ApiError> {
        self.post_form(STRIPE_CHECKOUT_SESSIONS_URL, secret_key, form).await
    }

    async fn create_billing_portal_session(
        &self,
        secret_key: &str,
        form: &[(String, String)],
    ) -> Result<String, ApiError> {
        self.post_form(STRIPE_PORTAL_SESSIONS_URL, secret_key, form).await
    }

    async fn post_object(
        &self,
        secret_key: &str,
        path: &str,
        idempotency_key: Option<&str>,
        form: &[(String, String)],
    ) -> Result<Value, ApiError> {
        self.post_form_value(&format!("https://api.stripe.com{path}"), secret_key, idempotency_key, form).await
    }

    async fn get_object(&self, secret_key: &str, path: &str, query: &[(String, String)]) -> Result<Value, ApiError> {
        let request = crate::net_client::client()
            .get(format!("https://api.stripe.com{path}"))
            .basic_auth(secret_key, None::<&str>)
            .query(query);
        Self::send(request).await
    }

    async fn delete_object(&self, secret_key: &str, path: &str, form: &[(String, String)]) -> Result<Value, ApiError> {
        let request = crate::net_client::client()
            .delete(format!("https://api.stripe.com{path}"))
            .basic_auth(secret_key, None::<&str>)
            .form(form);
        Self::send(request).await
    }

    async fn create_invoice_item(
        &self,
        secret_key: &str,
        idempotency_key: &str,
        form: &[(String, String)],
    ) -> Result<String, ApiError> {
        let body = self
            .post_form_value(STRIPE_INVOICE_ITEMS_URL, secret_key, Some(idempotency_key), form)
            .await?;
        body["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ApiError::Internal("Stripe returned no invoice item id".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosted(success_url: &str, cancel_url: &str) -> CheckoutReturn {
        CheckoutReturn::Hosted { success_url: success_url.to_string(), cancel_url: cancel_url.to_string() }
    }

    #[test]
    fn return_url_accepts_allternit_pages_and_rejects_everything_else() {
        assert_eq!(client_return_url(None).unwrap(), None);
        assert_eq!(client_return_url(Some("  ")).unwrap(), None);
        assert_eq!(
            client_return_url(Some("https://ai.allternit.com/settings/billing")).unwrap().as_deref(),
            Some("https://ai.allternit.com/settings/billing")
        );
        assert!(client_return_url(Some("https://platform.allternit.com/billing/")).unwrap().is_some());
        for bad in [
            "http://ai.allternit.com/settings/billing",
            "https://evil.example.com/settings/billing",
            "https://ai.allternit.com.evil.example/x",
            "https://ai.allternit.com:8443/settings/billing",
            "https://user:pw@ai.allternit.com/settings/billing",
            "https://ai.allternit.com/settings/billing?next=https://evil.example",
            "https://ai.allternit.com/settings/billing#x",
            "javascript:alert(1)",
            "not a url",
        ] {
            assert!(client_return_url(Some(bad)).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn checkout_return_urls_use_the_client_page_when_given() {
        let (success, cancel) = checkout_return_urls(&ReturnTo {
            return_url: Some("https://ai.allternit.com/settings/billing".to_string()),
        })
        .unwrap();
        assert_eq!(success, "https://ai.allternit.com/settings/billing?checkout=success");
        assert_eq!(cancel, "https://ai.allternit.com/settings/billing?checkout=cancelled");
        assert!(checkout_return_urls(&ReturnTo {
            return_url: Some("https://evil.example.com/".to_string()),
        })
        .is_err());
    }

    #[test]
    fn pack_catalog_is_the_four_static_packs_at_1_to_1_pricing() {
        let packs = credit_pack_catalog();
        assert_eq!(packs.len(), 4);
        assert_eq!(
            packs.iter().map(|pack| pack.id).collect::<Vec<_>>(),
            vec!["credits_10", "credits_25", "credits_50", "credits_100"]
        );
        for (pack, expected) in packs.iter().zip([10.0, 25.0, 50.0, 100.0]) {
            assert_eq!(pack.credits_usd, expected);
            assert_eq!(pack.price_usd, pack.credits_usd, "price is 1:1 with credits");
        }
    }

    #[test]
    fn packs_endpoint_serializes_the_public_shape() {
        let value = serde_json::to_value(packs_response(&crate::services::store_billing::StoreConfig::default())).unwrap();
        let first = &value["packs"][0];
        assert_eq!(first["id"], serde_json::json!("credits_10"));
        assert_eq!(first["credits_usd"], serde_json::json!(10.0));
        assert_eq!(first["price_usd"], serde_json::json!(10.0));
        assert!(first["label"].is_string());
    }

    /// An old client decodes packs into a struct with only the fields it knew
    /// before store billing; the added `store` block must not break it.
    #[test]
    fn packs_with_store_block_still_parse_for_old_clients() {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct OldPack {
            id: String,
            label: String,
            credits_usd: f64,
            price_usd: f64,
        }
        #[derive(serde::Deserialize)]
        struct OldPacks {
            packs: Vec<serde_json::Value>,
        }
        let config = crate::services::store_billing::StoreConfig { apple: true, google: false };
        let body = serde_json::to_string(&packs_response(&config)).unwrap();
        let parsed: OldPacks = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.packs.len(), 4);
        for mut pack in parsed.packs {
            // Only the store block is new; everything else is the old shape exactly.
            let store = pack.as_object_mut().unwrap().remove("store").unwrap();
            let old: OldPack = serde_json::from_value(pack).unwrap();
            assert_eq!(old.price_usd, old.credits_usd, "web price is unchanged");
            assert_eq!(store["apple"]["enabled"], serde_json::json!(true));
            assert_eq!(store["google"]["enabled"], serde_json::json!(false));
        }
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["packs"][1]["store"]["apple"]["product_id"], "com.allternit.mobile.credits.25");
        assert_eq!(value["packs"][1]["store"]["google"]["product_id"], "credits_25");
        assert_eq!(value["packs"][1]["store"]["reference_price_usd"], serde_json::json!(29.99));
        assert_eq!(value["packs"][1]["credits_usd"], serde_json::json!(25.0), "same credits as web");
    }

    #[test]
    fn pack_validation_accepts_catalog_ids() {
        for id in ["credits_10", "credits_25", "credits_50", "credits_100"] {
            assert!(find_pack(id).is_some(), "{id} must be a known pack");
        }
    }

    #[test]
    fn pack_validation_rejects_unknown_ids() {
        for id in ["credits_5", "CREDITS_10", "", "credits_10 ", "credits_10; DROP TABLE"] {
            assert!(find_pack(id).is_none(), "{id:?} must not be a known pack");
        }
    }

    #[test]
    fn checkout_form_metadata_carries_the_credit_contract() {
        let pack = find_pack("credits_25").unwrap();
        let form = checkout_form_params(pack, "user_123", Some("org_9"), None, &hosted("https://s.example", "https://c.example"));
        let field = |key: &str| -> String {
            form.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(field("metadata[clerk_user_id]"), "user_123");
        assert_eq!(field("metadata[allternit_credits_usd]"), "25.00");
        assert_eq!(field("metadata[allternit_org_id]"), "org_9");
        assert_eq!(field("line_items[0][price_data][unit_amount]"), "2500", "credits are priced 1:1 in cents");
        assert_eq!(field("line_items[0][price_data][currency]"), "usd");
        assert_eq!(field("mode"), "payment");
        assert_eq!(field("payment_method_types[0]"), "card", "pay-now methods only");
        assert_eq!(field("payment_method_types[1]"), "link");
        assert_eq!(field("success_url"), "https://s.example");
        assert_eq!(field("cancel_url"), "https://c.example");
        // The PaymentIntent carries the same contract so a dispute names the person.
        assert_eq!(field("payment_intent_data[metadata][clerk_user_id]"), "user_123");
        assert_eq!(field("payment_intent_data[metadata][allternit_credits_usd]"), "25.00");
        assert_eq!(field("payment_intent_data[metadata][allternit_org_id]"), "org_9");
        assert_eq!(field("payment_intent_data[metadata][kind]"), "credit_pack");
    }

    #[test]
    fn checkout_form_saves_the_card_when_the_person_has_a_customer() {
        let pack = find_pack("credits_25").unwrap();
        let form = checkout_form_params(pack, "user_1", None, Some("cus_9"), &hosted("s", "c"));
        assert!(form.contains(&("customer".to_string(), "cus_9".to_string())));
        assert!(form.contains(&(
            "payment_intent_data[setup_future_usage]".to_string(),
            "off_session".to_string()
        )));
        let without = checkout_form_params(pack, "user_1", None, None, &hosted("s", "c"));
        assert!(without
            .iter()
            .all(|(key, _)| key != "customer" && key != "payment_intent_data[setup_future_usage]"));
    }

    #[test]
    fn pack_sizes_are_the_auto_topup_amounts() {
        assert_eq!(pack_sizes_usd(), vec![10.0, 25.0, 50.0, 100.0]);
    }

    #[test]
    fn checkout_form_omits_org_metadata_without_an_organization() {
        let pack = find_pack("credits_10").unwrap();
        let form = checkout_form_params(pack, "user_123", None, None, &hosted("s", "c"));
        assert!(
            form.iter().all(|(key, _)| key != "metadata[allternit_org_id]"
                && key != "payment_intent_data[metadata][allternit_org_id]"),
            "no org metadata field may be sent when the session has no active org"
        );
        assert!(form.contains(&(
            "payment_intent_data[metadata][clerk_user_id]".to_string(),
            "user_123".to_string()
        )));
        assert!(form.contains(&("payment_intent_data[metadata][kind]".to_string(), "credit_pack".to_string())));
        // The rest of the contract is unaffected.
        assert!(form.contains(&("metadata[clerk_user_id]".to_string(), "user_123".to_string())));
        assert!(form.contains(&("metadata[allternit_credits_usd]".to_string(), "10.00".to_string())));
    }

    #[test]
    fn credits_are_formatted_to_two_decimals_in_metadata() {
        for (id, expected) in [
            ("credits_10", "10.00"),
            ("credits_25", "25.00"),
            ("credits_50", "50.00"),
            ("credits_100", "100.00"),
        ] {
            let pack = find_pack(id).unwrap();
            let form = checkout_form_params(pack, "user_1", None, None, &hosted("s", "c"));
            let credits = form
                .iter()
                .find(|(k, _)| k == "metadata[allternit_credits_usd]")
                .map(|(_, v)| v.clone())
                .unwrap();
            assert_eq!(credits, expected, "{id} metadata must be two-decimal USD");
        }
    }

    struct RecordingCheckout {
        last_form: std::sync::Mutex<Option<Vec<(String, String)>>>,
        last_secret: std::sync::Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl StripeCheckout for RecordingCheckout {
        async fn create_checkout_session(
            &self,
            secret_key: &str,
            form: &[(String, String)],
        ) -> Result<String, ApiError> {
            *self.last_form.lock().unwrap() = Some(form.to_vec());
            *self.last_secret.lock().unwrap() = Some(secret_key.to_string());
            Ok("https://checkout.stripe.com/c/pay/test_session".to_string())
        }

        async fn create_billing_portal_session(
            &self,
            _secret_key: &str,
            _form: &[(String, String)],
        ) -> Result<String, ApiError> {
            unimplemented!("portal sessions are stubbed in billing_subscriptions tests")
        }
    }

    #[tokio::test]
    async fn checkout_creation_builds_form_from_pack_and_user() {
        let checkout = RecordingCheckout {
            last_form: std::sync::Mutex::new(None),
            last_secret: std::sync::Mutex::new(None),
        };
        let pack = find_pack("credits_50").unwrap();
        let url = create_pack_checkout(&checkout, "sk_test_1", pack, "user_9", Some("org_9"), None, &hosted("https://s", "https://c"))
            .await
            .unwrap();
        assert_eq!(
            url,
            CheckoutSession::Hosted { url: "https://checkout.stripe.com/c/pay/test_session".to_string() }
        );
        let form = checkout.last_form.lock().unwrap().clone().unwrap();
        assert_eq!(
            *checkout.last_secret.lock().unwrap(),
            Some("sk_test_1".to_string())
        );
        assert!(form.contains(&("metadata[clerk_user_id]".to_string(), "user_9".to_string())));
        assert!(form.contains(&("metadata[allternit_credits_usd]".to_string(), "50.00".to_string())));
        assert!(form.contains(&("metadata[allternit_org_id]".to_string(), "org_9".to_string())));
    }

    struct FailingCheckout;

    #[async_trait::async_trait]
    impl StripeCheckout for FailingCheckout {
        async fn create_checkout_session(
            &self,
            _secret_key: &str,
            _form: &[(String, String)],
        ) -> Result<String, ApiError> {
            Err(ApiError::Internal("stripe declined".to_string()))
        }

        async fn create_billing_portal_session(
            &self,
            _secret_key: &str,
            _form: &[(String, String)],
        ) -> Result<String, ApiError> {
            unimplemented!("portal sessions are stubbed in billing_subscriptions tests")
        }
    }

    #[tokio::test]
    async fn checkout_creation_propagates_stripe_errors() {
        let pack = find_pack("credits_10").unwrap();
        let error = create_pack_checkout(&FailingCheckout, "sk_test_1", pack, "user_9", None, None, &hosted("s", "c"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("stripe declined"));
    }

    #[test]
    fn default_urls_point_at_the_platform_billing_page() {
        assert_eq!(DEFAULT_SUCCESS_URL, "https://platform.allternit.com/billing?checkout=success");
        assert_eq!(DEFAULT_CANCEL_URL, "https://platform.allternit.com/billing?checkout=cancelled");
    }

    /// Fake Stripe for embedded sessions: records the form sent to
    /// `/v1/checkout/sessions` and answers the session object Stripe returns
    /// for `ui_mode=embedded` (a client secret, no hosted URL).
    struct EmbeddedStripe {
        form: std::sync::Mutex<Option<Vec<(String, String)>>>,
        path: std::sync::Mutex<Option<String>>,
        session: Value,
    }

    impl EmbeddedStripe {
        fn new() -> Self {
            Self {
                form: std::sync::Mutex::new(None),
                path: std::sync::Mutex::new(None),
                session: serde_json::json!({
                    "id": "cs_test_embedded_1",
                    "object": "checkout.session",
                    "client_secret": "cs_test_embedded_1_secret_abc",
                    "url": null,
                    "ui_mode": "embedded",
                }),
            }
        }
    }

    #[async_trait::async_trait]
    impl StripeCheckout for EmbeddedStripe {
        async fn create_checkout_session(&self, _secret_key: &str, _form: &[(String, String)]) -> Result<String, ApiError> {
            Err(ApiError::Internal("an embedded session must not ask Stripe for a hosted URL".to_string()))
        }

        async fn create_billing_portal_session(&self, _secret_key: &str, _form: &[(String, String)]) -> Result<String, ApiError> {
            unimplemented!("portal sessions are stubbed in billing_subscriptions tests")
        }

        async fn post_object(
            &self,
            _secret_key: &str,
            path: &str,
            _idempotency_key: Option<&str>,
            form: &[(String, String)],
        ) -> Result<Value, ApiError> {
            *self.path.lock().unwrap() = Some(path.to_string());
            *self.form.lock().unwrap() = Some(form.to_vec());
            Ok(self.session.clone())
        }
    }

    async fn response_json(response: Response) -> (StatusCode, Value) {
        use http_body_util::BodyExt;
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn embedded() -> CheckoutReturn {
        CheckoutReturn::for_mode(
            UiMode::Embedded,
            &ReturnTo { return_url: Some("https://ai.allternit.com/settings/billing".to_string()) },
        )
        .unwrap()
    }

    #[test]
    fn request_ui_mode_defaults_to_hosted_and_rejects_unknown_modes() {
        let absent: CheckoutRequest = serde_json::from_str(r#"{"pack_id":"credits_10"}"#).unwrap();
        assert_eq!(absent.ui_mode.unwrap_or_default(), UiMode::Hosted);
        let null: CheckoutRequest = serde_json::from_str(r#"{"pack_id":"credits_10","ui_mode":null}"#).unwrap();
        assert_eq!(null.ui_mode.unwrap_or_default(), UiMode::Hosted);
        let hosted: CheckoutRequest = serde_json::from_str(r#"{"pack_id":"credits_10","ui_mode":"hosted"}"#).unwrap();
        assert_eq!(hosted.ui_mode, Some(UiMode::Hosted));
        let embedded: CheckoutRequest = serde_json::from_str(r#"{"pack_id":"credits_10","ui_mode":"embedded"}"#).unwrap();
        assert_eq!(embedded.ui_mode, Some(UiMode::Embedded));
        for bad in ["custom", "EMBEDDED", "elements", ""] {
            let body = format!(r#"{{"pack_id":"credits_10","ui_mode":"{bad}"}}"#);
            assert!(serde_json::from_str::<CheckoutRequest>(&body).is_err(), "{bad:?} must be rejected");
        }
    }

    #[tokio::test]
    async fn a_customer_in_the_request_body_never_reaches_stripe() {
        // A client tries to bill someone else's Stripe customer.
        let request: CheckoutRequest = serde_json::from_str(
            r#"{"pack_id":"credits_10","ui_mode":"embedded","customer":"cus_victim","customer_email":"x@example.com","customer_id":"cus_victim"}"#,
        )
        .unwrap();
        let pack = find_pack(&request.pack_id).unwrap();
        let checkout_return = CheckoutReturn::for_mode(request.ui_mode.unwrap_or_default(), &ReturnTo::default()).unwrap();
        let stripe = EmbeddedStripe::new();
        // No server-side customer: the form has none at all.
        create_pack_checkout(&stripe, "sk_test_1", pack, "user_1", None, None, &checkout_return).await.unwrap();
        let form = stripe.form.lock().unwrap().clone().unwrap();
        assert!(form.iter().all(|(key, value)| !key.starts_with("customer") && !value.contains("cus_victim")));
        // The signed-in account's customer is the only one sent.
        create_pack_checkout(&stripe, "sk_test_1", pack, "user_1", None, Some("cus_own"), &checkout_return).await.unwrap();
        let form = stripe.form.lock().unwrap().clone().unwrap();
        let customers: Vec<&str> = form.iter().filter(|(key, _)| key == "customer").map(|(_, value)| value.as_str()).collect();
        assert_eq!(customers, vec!["cus_own"]);
        assert!(form.iter().all(|(_, value)| !value.contains("cus_victim")));
    }

    #[tokio::test]
    async fn embedded_pack_checkout_returns_the_client_secret_and_session_id() {
        let stripe = EmbeddedStripe::new();
        let pack = find_pack("credits_25").unwrap();
        let session = create_pack_checkout(&stripe, "sk_test_1", pack, "user_7", Some("org_7"), Some("cus_7"), &embedded())
            .await
            .unwrap();
        assert_eq!(
            session,
            CheckoutSession::Embedded {
                client_secret: "cs_test_embedded_1_secret_abc".to_string(),
                session_id: "cs_test_embedded_1".to_string(),
            }
        );
        assert_eq!(stripe.path.lock().unwrap().as_deref(), Some("/v1/checkout/sessions"));
        let form = stripe.form.lock().unwrap().clone().unwrap();
        let field = |key: &str| form.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        assert_eq!(field("ui_mode").as_deref(), Some("embedded"));
        assert_eq!(
            field("return_url").as_deref(),
            Some("https://ai.allternit.com/settings/billing?checkout=success&session_id={CHECKOUT_SESSION_ID}")
        );
        assert_eq!(field("redirect_on_completion").as_deref(), Some("if_required"));
        assert_eq!(field("success_url"), None, "Stripe rejects success_url on embedded sessions");
        assert_eq!(field("cancel_url"), None, "Stripe rejects cancel_url on embedded sessions");
        // Card + Link only, and the same credit contract as hosted.
        assert_eq!(field("payment_method_types[0]").as_deref(), Some("card"));
        assert_eq!(field("payment_method_types[1]").as_deref(), Some("link"));
        assert_eq!(field("payment_method_types[2]"), None, "no wallets or other methods");
        assert_eq!(field("mode").as_deref(), Some("payment"));
        assert_eq!(field("metadata[clerk_user_id]").as_deref(), Some("user_7"));
        assert_eq!(field("metadata[allternit_credits_usd]").as_deref(), Some("25.00"));
        assert_eq!(field("metadata[allternit_org_id]").as_deref(), Some("org_7"));
        assert_eq!(field("customer").as_deref(), Some("cus_7"));
        assert_eq!(field("payment_intent_data[setup_future_usage]").as_deref(), Some("off_session"));

        let (status, body) = response_json(checkout_session_response(session)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            serde_json::json!({ "client_secret": "cs_test_embedded_1_secret_abc", "session_id": "cs_test_embedded_1" })
        );
    }

    #[tokio::test]
    async fn hosted_pack_checkout_reply_is_unchanged() {
        let checkout = RecordingCheckout {
            last_form: std::sync::Mutex::new(None),
            last_secret: std::sync::Mutex::new(None),
        };
        let pack = find_pack("credits_10").unwrap();
        let checkout_return = CheckoutReturn::for_mode(UiMode::Hosted, &ReturnTo::default()).unwrap();
        let session = create_pack_checkout(&checkout, "sk_test_1", pack, "user_1", None, None, &checkout_return)
            .await
            .unwrap();
        let form = checkout.last_form.lock().unwrap().clone().unwrap();
        assert!(form.iter().all(|(key, _)| key != "ui_mode" && key != "return_url" && key != "redirect_on_completion"));
        assert!(form.iter().any(|(key, _)| key == "success_url"));
        assert!(form.iter().any(|(key, _)| key == "cancel_url"));
        let (status, body) = response_json(checkout_session_response(session)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "checkout_url": "https://checkout.stripe.com/c/pay/test_session" }));
    }

    #[tokio::test]
    async fn embedded_checkout_without_a_client_secret_is_an_error() {
        let mut stripe = EmbeddedStripe::new();
        stripe.session = serde_json::json!({ "id": "cs_test_1", "url": "https://checkout.stripe.com/x" });
        let pack = find_pack("credits_10").unwrap();
        let error = create_pack_checkout(&stripe, "sk_test_1", pack, "user_1", None, None, &embedded())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no client secret"));
    }

    #[test]
    fn embedded_return_url_adds_the_session_placeholder_once() {
        assert_eq!(
            embedded_return_url("https://platform.allternit.com/billing?checkout=success"),
            "https://platform.allternit.com/billing?checkout=success&session_id={CHECKOUT_SESSION_ID}"
        );
        assert_eq!(
            embedded_return_url("https://platform.allternit.com/billing"),
            "https://platform.allternit.com/billing?session_id={CHECKOUT_SESSION_ID}"
        );
        assert_eq!(
            embedded_return_url("https://x.example/done?id={CHECKOUT_SESSION_ID}"),
            "https://x.example/done?id={CHECKOUT_SESSION_ID}"
        );
        // A client page outside the allowed hosts is still refused in embedded mode.
        assert!(CheckoutReturn::for_mode(
            UiMode::Embedded,
            &ReturnTo { return_url: Some("https://evil.example.com/".to_string()) }
        )
        .is_err());
    }

    #[test]
    fn billing_config_reports_the_key_mode_and_card_plus_link() {
        let test = billing_config_from(Some("pk_test_abc"), Some("sk_test_xyz")).unwrap();
        assert_eq!(
            serde_json::to_value(&test).unwrap(),
            serde_json::json!({ "publishable_key": "pk_test_abc", "mode": "test", "payment_method_types": ["card", "link"] })
        );
        assert_eq!(billing_config_from(Some(" pk_test_abc "), Some("rk_test_xyz")).unwrap().publishable_key, "pk_test_abc");
        assert_eq!(billing_config_from(Some("pk_live_abc"), Some("sk_live_xyz")).unwrap().mode, "live");
    }

    #[test]
    fn billing_config_is_unavailable_when_keys_are_missing_malformed_or_mismatched() {
        for (publishable, secret) in [
            (None, Some("sk_test_xyz")),
            (Some(""), Some("sk_test_xyz")),
            (Some("pk_test_abc"), None),
            (Some("sk_test_abc"), Some("sk_test_xyz")),
            (Some("pk_abc"), Some("sk_test_xyz")),
            (Some("pk_test_abc"), Some("pk_test_xyz")),
            (Some("pk_test_abc"), Some("sk_live_xyz")),
            (Some("pk_live_abc"), Some("sk_test_xyz")),
        ] {
            assert!(billing_config_from(publishable, secret).is_none(), "{publishable:?} / {secret:?}");
        }
    }
}
