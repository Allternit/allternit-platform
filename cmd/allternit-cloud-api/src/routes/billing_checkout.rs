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
}

const CREDIT_PACKS: &[CreditPack] = &[
    CreditPack { id: "credits_10", label: "$10", credits_usd: 10.00 },
    CreditPack { id: "credits_25", label: "$25", credits_usd: 25.00 },
    CreditPack { id: "credits_50", label: "$50", credits_usd: 50.00 },
    CreditPack { id: "credits_100", label: "$100", credits_usd: 100.00 },
];

#[derive(Debug, Serialize)]
pub struct CreditPackResponse {
    id: &'static str,
    label: &'static str,
    credits_usd: f64,
    price_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct PacksResponse {
    packs: Vec<CreditPackResponse>,
}

#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pack_id: String,
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
const DEFAULT_RETURN_HOSTS: &[&str] = &["platform.allternit.com", "ai.allternit.com", "m.allternit.com"];

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
}

async fn list_packs() -> Json<PacksResponse> {
    Json(PacksResponse {
        packs: CREDIT_PACKS
            .iter()
            .map(|pack| CreditPackResponse {
                id: pack.id,
                label: pack.label,
                credits_usd: pack.credits_usd,
                price_usd: pack.credits_usd,
            })
            .collect(),
    })
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
    let (success_url, cancel_url) = match checkout_return_urls(&return_to) {
        Ok(urls) => urls,
        Err(error) => return error.into_response(),
    };
    let checkout = ReqwestStripeCheckout::new();

    match create_checkout_url(
        &checkout,
        &secret_key,
        pack,
        &user_id,
        organization_id.as_deref(),
        &success_url,
        &cancel_url,
    )
    .await
    {
        Ok(url) => {
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
                    })),
                    success: true,
                },
            )
            .await;
            Json(CheckoutResponse { checkout_url: url }).into_response()
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

/// Look up a pack in the static catalog. Unknown ids are a 400: the client asked for a pack the
/// server does not sell, and charging a wrong amount silently is worse than failing fast.
fn find_pack(pack_id: &str) -> Option<&'static CreditPack> {
    CREDIT_PACKS.iter().find(|pack| pack.id == pack_id)
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
    /// Used only by the Platform API billing plan and meter reporter, which run
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
}

/// Build the Checkout Session for a validated pack purchase and return its hosted URL.
async fn create_checkout_url(
    checkout: &dyn StripeCheckout,
    secret_key: &str,
    pack: &CreditPack,
    clerk_user_id: &str,
    organization_id: Option<&str>,
    success_url: &str,
    cancel_url: &str,
) -> Result<String, ApiError> {
    let form = checkout_form_params(pack, clerk_user_id, organization_id, success_url, cancel_url);
    checkout.create_checkout_session(secret_key, &form).await
}

/// Form fields for POST /v1/checkout/sessions: one payment-mode line item at the pack price, with the
/// credit-grant metadata contract on the session object itself — routes::billing_webhooks reads
/// clerk_user_id, allternit_credits_usd, and allternit_org_id out of
/// event.data.object.metadata for completed payment-mode sessions, so those
/// names must stay stable.
fn checkout_form_params(
    pack: &CreditPack,
    clerk_user_id: &str,
    organization_id: Option<&str>,
    success_url: &str,
    cancel_url: &str,
) -> Vec<(String, String)> {
    let mut form = vec![
        ("mode".to_string(), "payment".to_string()),
        // Pay-now methods only, as for plans: credits are granted on payment,
        // so no buy-now-pay-later or delayed methods.
        ("payment_method_types[0]".to_string(), "card".to_string()),
        ("payment_method_types[1]".to_string(), "link".to_string()),
        ("success_url".to_string(), success_url.to_string()),
        ("cancel_url".to_string(), cancel_url.to_string()),
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
    ];
    // Org routing for the webhook grant. Only written when the session has an
    // active Clerk org — absent means the bridge was off at purchase time and
    // the grant falls back to the cloud wallet.
    if let Some(organization_id) = organization_id {
        form.push((
            "metadata[allternit_org_id]".to_string(),
            organization_id.to_string(),
        ));
    }
    form
}

/// The production StripeCheckout: POSTs the form to the Stripe REST API with the secret key as the
/// basic-auth username (empty password, per Stripe's auth convention) and returns the hosted session
/// URL. The secret key never appears in error text.
pub(crate) struct ReqwestStripeCheckout;

impl ReqwestStripeCheckout {
    pub(crate) fn new() -> Self {
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
        let mut request = reqwest::Client::new()
            .post(url)
            .basic_auth(secret_key, None::<&str>)
            .form(form);
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
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

    #[tokio::test]
    async fn pack_catalog_is_the_four_static_packs_at_1_to_1_pricing() {
        let Json(packs) = list_packs().await;
        assert_eq!(packs.packs.len(), 4);
        assert_eq!(
            packs.packs.iter().map(|pack| pack.id).collect::<Vec<_>>(),
            vec!["credits_10", "credits_25", "credits_50", "credits_100"]
        );
        for (pack, expected) in packs.packs.iter().zip([10.0, 25.0, 50.0, 100.0]) {
            assert_eq!(pack.credits_usd, expected);
            assert_eq!(pack.price_usd, pack.credits_usd, "price is 1:1 with credits");
        }
    }

    #[tokio::test]
    async fn packs_endpoint_serializes_the_public_shape() {
        let Json(packs) = list_packs().await;
        let value = serde_json::to_value(&packs).unwrap();
        let first = &value["packs"][0];
        assert_eq!(first["id"], serde_json::json!("credits_10"));
        assert_eq!(first["credits_usd"], serde_json::json!(10.0));
        assert_eq!(first["price_usd"], serde_json::json!(10.0));
        assert!(first["label"].is_string());
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
        let form = checkout_form_params(pack, "user_123", Some("org_9"), "https://s.example", "https://c.example");
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
    }

    #[test]
    fn checkout_form_omits_org_metadata_without_an_organization() {
        let pack = find_pack("credits_10").unwrap();
        let form = checkout_form_params(pack, "user_123", None, "s", "c");
        assert!(
            form.iter().all(|(key, _)| key != "metadata[allternit_org_id]"),
            "no org metadata field may be sent when the session has no active org"
        );
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
            let form = checkout_form_params(pack, "user_1", None, "s", "c");
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
        let url = create_checkout_url(&checkout, "sk_test_1", pack, "user_9", Some("org_9"), "https://s", "https://c")
            .await
            .unwrap();
        assert_eq!(url, "https://checkout.stripe.com/c/pay/test_session");
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
        let error = create_checkout_url(&FailingCheckout, "sk_test_1", pack, "user_9", None, "s", "c")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("stripe declined"));
    }

    #[test]
    fn default_urls_point_at_the_platform_billing_page() {
        assert_eq!(DEFAULT_SUCCESS_URL, "https://platform.allternit.com/billing?checkout=success");
        assert_eq!(DEFAULT_CANCEL_URL, "https://platform.allternit.com/billing?checkout=cancelled");
    }
}
