//! Saved payment methods for the in-app billing page (Claude polish C3, E4).
//!
//! Card and Link only. One saved method is the **default** (what a Stripe
//! plan and auto top-up charge); every other one is a **backup**: saved, can
//! be made the default in one tap, never charged on its own.
//!
//! All four verify the Clerk session (or a `billing`-scoped API token) per
//! request and resolve the Stripe customer from that session
//! (`user_billing_accounts`), never from the request:
//!
//! - `POST /api/v1/billing/payment-methods/setup-intent` — a SetupIntent on
//!   the caller's customer (`usage=off_session`, card + link); returns
//!   `{ client_secret, setup_intent_id }` for Stripe Elements. Creates the
//!   customer when the person has none yet.
//! - `GET /api/v1/billing/payment-methods` — `{ payment_methods: [{ id,
//!   kind: card|link, brand, last4, exp_month, exp_year, email,
//!   is_default }], default_id }`, the default first.
//! - `POST /api/v1/billing/payment-methods/:id/default` — make a saved
//!   method the default: the customer's invoice default, every open Stripe
//!   plan's default, and auto top-up's stored card.
//! - `DELETE /api/v1/billing/payment-methods/:id` — detach a saved method.
//!   409 `default_payment_method_in_use` when it is the default and a Stripe
//!   plan is open. Removing the default otherwise promotes the newest backup.
//!
//! A payment method id that is not on the caller's own customer answers 404
//! `payment_method_not_found`, the same as one that does not exist, so one
//! account can never read, default or detach another account's card.
//!
//! 503 `billing_not_configured` without `STRIPE_SECRET_KEY`. Stripe failures
//! answer 502 `billing_upstream_error`.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use super::billing_checkout::{
    billing_not_configured_response, billing_upstream_error_response, ReqwestStripeCheckout, StripeCheckout,
};
use crate::error::ApiError;
use crate::services::auto_topup::{self, SavedCard};
use crate::ApiState;

/// Stripe subscription statuses that still bill the customer (the same set
/// `routes::billing_subscriptions` treats as an open plan).
const OPEN_SUBSCRIPTION_STATUSES: &[&str] = &["active", "trialing", "past_due", "unpaid"];

/// The most saved methods read per customer. Stripe's list maximum.
const LIST_LIMIT: &str = "100";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/billing/payment-methods", get(list_payment_methods))
        .route("/api/v1/billing/payment-methods/setup-intent", post(create_setup_intent))
        .route("/api/v1/billing/payment-methods/:id/default", post(set_default_payment_method))
        .route("/api/v1/billing/payment-methods/:id", delete(remove_payment_method))
}

/// One saved method as the client shows it (`components/billing/PaymentMethods.tsx`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaymentMethodView {
    pub id: String,
    /// `card` or `link`.
    pub kind: &'static str,
    /// Card network from Stripe (`visa`, `mastercard`, ...), `link` for Link.
    pub brand: String,
    pub last4: Option<String>,
    pub exp_month: Option<i64>,
    pub exp_year: Option<i64>,
    /// The Link account email (Link only).
    pub email: Option<String>,
    pub is_default: bool,
}

impl PaymentMethodView {
    fn saved_card(&self) -> SavedCard {
        SavedCard { payment_method_id: self.id.clone(), brand: self.brand.clone(), last4: self.last4.clone() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaymentMethodList {
    pub payment_methods: Vec<PaymentMethodView>,
    pub default_id: Option<String>,
}

impl PaymentMethodList {
    fn empty() -> Self {
        Self { payment_methods: Vec::new(), default_id: None }
    }

    fn find(&self, payment_method_id: &str) -> Option<&PaymentMethodView> {
        self.payment_methods.iter().find(|method| method.id == payment_method_id)
    }

    fn default_method(&self) -> Option<&PaymentMethodView> {
        self.payment_methods.iter().find(|method| method.is_default)
    }
}

/// Why a payment-method call did not go through. Ownership failures are
/// their own variant so they can never surface as a 502 that hints the id exists.
#[derive(Debug)]
pub enum MethodError {
    NotFound,
    DefaultInUse,
    Upstream(ApiError),
}

impl From<ApiError> for MethodError {
    fn from(error: ApiError) -> Self {
        MethodError::Upstream(error)
    }
}

impl IntoResponse for MethodError {
    fn into_response(self) -> Response {
        match self {
            MethodError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "payment_method_not_found",
                    "message": "That card isn't saved on your account.",
                })),
            )
                .into_response(),
            MethodError::DefaultInUse => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "default_payment_method_in_use",
                    "message": "Your plan is billed to this card. Make another card the default first, then remove this one.",
                })),
            )
                .into_response(),
            MethodError::Upstream(ApiError::DatabaseError(error)) => ApiError::DatabaseError(error).into_response(),
            MethodError::Upstream(error) => billing_upstream_error_response(&error),
        }
    }
}

/// The id inside a Stripe field that is either an id string or an expanded object.
fn object_id(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| value["id"].as_str()).filter(|id| !id.is_empty())
}

/// The display fields of one Stripe PaymentMethod, or `None` for anything
/// that is not a card or Link, or that names a different customer.
pub(crate) fn method_view(payment_method: &Value, customer_id: &str, default_id: Option<&str>) -> Option<PaymentMethodView> {
    let id = object_id(&payment_method["id"])?;
    // Defence in depth: the list is already scoped to the customer.
    if let Some(owner) = object_id(&payment_method["customer"]) {
        if owner != customer_id {
            return None;
        }
    }
    let is_default = default_id == Some(id);
    match payment_method["type"].as_str()? {
        "card" => Some(PaymentMethodView {
            id: id.to_string(),
            kind: "card",
            brand: payment_method["card"]["brand"].as_str().unwrap_or("card").to_string(),
            last4: payment_method["card"]["last4"].as_str().map(str::to_string),
            exp_month: payment_method["card"]["exp_month"].as_i64(),
            exp_year: payment_method["card"]["exp_year"].as_i64(),
            email: None,
            is_default,
        }),
        "link" => Some(PaymentMethodView {
            id: id.to_string(),
            kind: "link",
            brand: "link".to_string(),
            last4: None,
            exp_month: None,
            exp_year: None,
            email: payment_method["link"]["email"].as_str().map(str::to_string),
            is_default,
        }),
        _ => None,
    }
}

/// The caller's saved card and Link methods with the default marked, the
/// default first, then newest first (Stripe's list order).
pub(crate) async fn load_methods(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
) -> Result<PaymentMethodList, ApiError> {
    let customer = stripe.get_object(secret_key, &format!("/v1/customers/{customer_id}"), &[]).await?;
    let default_id = object_id(&customer["invoice_settings"]["default_payment_method"]).map(str::to_string);
    let list = stripe
        .get_object(
            secret_key,
            &format!("/v1/customers/{customer_id}/payment_methods"),
            &[("limit".to_string(), LIST_LIMIT.to_string())],
        )
        .await?;
    let mut methods: Vec<PaymentMethodView> = list["data"]
        .as_array()
        .map(|data| {
            data.iter().filter_map(|method| method_view(method, customer_id, default_id.as_deref())).collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // Stable: keeps Stripe's newest-first order among the backups.
    methods.sort_by_key(|method| !method.is_default);
    // A default that is no longer attached (or not card/Link) is no default.
    let default_id = default_id.filter(|id| methods.iter().any(|method| &method.id == id));
    Ok(PaymentMethodList { payment_methods: methods, default_id })
}

/// Make `payment_method_id` the customer's default and the default of every
/// open Stripe plan on it. The caller checked ownership.
async fn write_default(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
    payment_method_id: &str,
) -> Result<(), ApiError> {
    stripe
        .post_object(
            secret_key,
            &format!("/v1/customers/{customer_id}"),
            None,
            &[("invoice_settings[default_payment_method]".to_string(), payment_method_id.to_string())],
        )
        .await?;
    // A plan bought through Checkout carries its own default_payment_method,
    // which wins over the customer's; point it at the new default too.
    let subscriptions = stripe
        .get_object(
            secret_key,
            "/v1/subscriptions",
            &[
                ("customer".to_string(), customer_id.to_string()),
                ("status".to_string(), "all".to_string()),
                ("limit".to_string(), "20".to_string()),
            ],
        )
        .await?;
    for subscription in subscriptions["data"].as_array().into_iter().flatten() {
        let open = subscription["status"].as_str().map_or(false, |status| OPEN_SUBSCRIPTION_STATUSES.contains(&status));
        let ours = object_id(&subscription["customer"]).map_or(true, |owner| owner == customer_id);
        let Some(subscription_id) = object_id(&subscription["id"]) else { continue };
        if !open || !ours {
            continue;
        }
        stripe
            .post_object(
                secret_key,
                &format!("/v1/subscriptions/{subscription_id}"),
                None,
                &[("default_payment_method".to_string(), payment_method_id.to_string())],
            )
            .await?;
    }
    Ok(())
}

/// Set a saved method as the default. 404 unless it is on `customer_id`.
/// Returns the new list.
pub(crate) async fn set_default(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
    payment_method_id: &str,
) -> Result<PaymentMethodList, MethodError> {
    let methods = load_methods(stripe, secret_key, customer_id).await?;
    if methods.find(payment_method_id).is_none() {
        return Err(MethodError::NotFound);
    }
    write_default(stripe, secret_key, customer_id, payment_method_id).await?;
    Ok(load_methods(stripe, secret_key, customer_id).await?)
}

/// Detach a saved method. 404 unless it is on `customer_id`; refused while
/// it is the default and a Stripe plan is open. Removing the default with no
/// plan open makes the newest backup the default, so there is always one
/// default while any method is saved. Returns the new list.
pub(crate) async fn remove(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
    payment_method_id: &str,
    has_open_stripe_plan: bool,
) -> Result<PaymentMethodList, MethodError> {
    let methods = load_methods(stripe, secret_key, customer_id).await?;
    let Some(method) = methods.find(payment_method_id) else {
        return Err(MethodError::NotFound);
    };
    let was_default = method.is_default;
    if was_default && has_open_stripe_plan {
        return Err(MethodError::DefaultInUse);
    }
    stripe
        .post_object(secret_key, &format!("/v1/payment_methods/{payment_method_id}/detach"), None, &[])
        .await?;
    let mut after = load_methods(stripe, secret_key, customer_id).await?;
    if was_default && after.default_id.is_none() {
        if let Some(next) = after.payment_methods.first().map(|method| method.id.clone()) {
            write_default(stripe, secret_key, customer_id, &next).await?;
            after = load_methods(stripe, secret_key, customer_id).await?;
        }
    }
    Ok(after)
}

/// Form for `POST /v1/setup_intents`: save a card or Link for later
/// off-session charges on the caller's customer.
pub(crate) fn setup_intent_form(customer_id: &str, clerk_user_id: &str, kind: &str) -> Vec<(String, String)> {
    vec![
        ("customer".to_string(), customer_id.to_string()),
        ("usage".to_string(), "off_session".to_string()),
        ("payment_method_types[0]".to_string(), "card".to_string()),
        ("payment_method_types[1]".to_string(), "link".to_string()),
        ("metadata[kind]".to_string(), kind.to_string()),
        ("metadata[clerk_user_id]".to_string(), clerk_user_id.to_string()),
    ]
}

/// Create a SetupIntent and return `{ client_secret, setup_intent_id }`.
pub(crate) async fn create_setup_intent_secret(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
    clerk_user_id: &str,
    kind: &str,
) -> Result<Value, ApiError> {
    let intent = stripe
        .post_object(secret_key, "/v1/setup_intents", None, &setup_intent_form(customer_id, clerk_user_id, kind))
        .await?;
    let client_secret = intent["client_secret"]
        .as_str()
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| ApiError::Internal("Stripe returned no setup intent secret".to_string()))?;
    Ok(json!({
        "client_secret": client_secret,
        "setup_intent_id": intent["id"].as_str(),
    }))
}

/// True when a Stripe-billed plan is open on the account: that plan charges
/// the default card, so the default cannot be removed. Store-billed plans
/// (Apple, Google) never charge it.
async fn has_open_stripe_plan(db: &PgPool, user_id: &str) -> Result<bool, ApiError> {
    let open = super::billing_subscriptions::open_subscriptions(db, user_id).await?;
    Ok(open.iter().any(|plan| plan.provider == "stripe"))
}

/// Keep auto top-up on the default: the stored card follows the new
/// default; with nothing saved any more it is cleared and auto top-up turns
/// off. Only touches a row that already holds a card. Best effort: the
/// Stripe change already happened.
async fn sync_auto_topup_card(db: &PgPool, user_id: &str, customer_id: &str, methods: &PaymentMethodList) {
    let row = match auto_topup::load(db, user_id).await {
        Ok(Some(row)) if row.stripe_payment_method_id.is_some() => row,
        Ok(_) => return,
        Err(error) => {
            tracing::warn!(%user_id, %error, "payment methods: could not read auto top-up");
            return;
        }
    };
    let result = match methods.default_method() {
        Some(default) if row.stripe_payment_method_id.as_deref() == Some(default.id.as_str()) => return,
        Some(default) => auto_topup::save_card(db, user_id, customer_id, &default.saved_card()).await,
        None => sqlx::query(
            r#"
            UPDATE billing_auto_topup
            SET stripe_payment_method_id = NULL, card_brand = NULL, card_last4 = NULL,
                enabled = false, updated_at = now()
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .execute(db)
        .await
        .map(|_| ())
        .map_err(ApiError::DatabaseError),
    };
    if let Err(error) = result {
        tracing::warn!(%user_id, %error, "payment methods: could not move auto top-up to the default card");
    }
}

async fn audit(db: &PgPool, user_id: &str, action: &str, payment_method_id: &str) {
    crate::services::audit::write_audit_log(
        db,
        crate::services::audit::AuditEvent {
            action: action.to_string(),
            resource_type: "billing_payment_method".to_string(),
            resource_id: Some(payment_method_id.to_string()),
            user_id: Some(user_id.to_string()),
            user_email: None,
            details: None,
            success: true,
        },
    )
    .await;
}

async fn list_payment_methods(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let customer_id = match auto_topup::linked_customer(&state.db, &user.id).await {
        Ok(Some(customer_id)) => customer_id,
        Ok(None) => return Json(PaymentMethodList::empty()).into_response(),
        Err(error) => return error.into_response(),
    };
    match load_methods(&ReqwestStripeCheckout::new(), &secret_key, &customer_id).await {
        Ok(methods) => Json(methods).into_response(),
        Err(error) => billing_upstream_error_response(&error),
    }
}

async fn create_setup_intent(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let stripe = ReqwestStripeCheckout::new();
    let customer_id =
        match auto_topup::ensure_customer(&state.db, &stripe, &secret_key, &user.id, user.email.as_deref()).await {
            Ok(customer_id) => customer_id,
            Err(error) => return billing_upstream_error_response(&error),
        };
    match create_setup_intent_secret(&stripe, &secret_key, &customer_id, &user.id, "payment_method_setup").await {
        Ok(body) => Json(body).into_response(),
        Err(error) => billing_upstream_error_response(&error),
    }
}

async fn set_default_payment_method(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(payment_method_id): Path<String>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let customer_id = match auto_topup::linked_customer(&state.db, &user.id).await {
        Ok(Some(customer_id)) => customer_id,
        Ok(None) => return MethodError::NotFound.into_response(),
        Err(error) => return error.into_response(),
    };
    match set_default(&ReqwestStripeCheckout::new(), &secret_key, &customer_id, &payment_method_id).await {
        Ok(methods) => {
            sync_auto_topup_card(&state.db, &user.id, &customer_id, &methods).await;
            audit(&state.db, &user.id, "billing.payment_method.default_set", &payment_method_id).await;
            Json(methods).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn remove_payment_method(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(payment_method_id): Path<String>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let customer_id = match auto_topup::linked_customer(&state.db, &user.id).await {
        Ok(Some(customer_id)) => customer_id,
        Ok(None) => return MethodError::NotFound.into_response(),
        Err(error) => return error.into_response(),
    };
    let plan_open = match has_open_stripe_plan(&state.db, &user.id).await {
        Ok(open) => open,
        Err(error) => return error.into_response(),
    };
    match remove(&ReqwestStripeCheckout::new(), &secret_key, &customer_id, &payment_method_id, plan_open).await {
        Ok(methods) => {
            sync_auto_topup_card(&state.db, &user.id, &customer_id, &methods).await;
            audit(&state.db, &user.id, "billing.payment_method.removed", &payment_method_id).await;
            Json(methods).into_response()
        }
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! An in-memory Stripe for the payment-method and invoice tests: two
    //! customers, each with its own cards and invoices. Detach and default
    //! writes change the state, so a test sees what a second call would.

    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeStripe {
        /// payment method id -> (owning customer, Stripe PaymentMethod JSON).
        pub methods: Mutex<Vec<(String, Value)>>,
        /// customer id -> default payment method id.
        pub defaults: Mutex<HashMap<String, String>>,
        /// customer id -> Stripe subscriptions.
        pub subscriptions: Mutex<HashMap<String, Vec<Value>>>,
        /// Stripe invoices (each carries its `customer`).
        pub invoices: Mutex<Vec<Value>>,
        /// Every write: (path, form).
        pub posts: Mutex<Vec<(String, Vec<(String, String)>)>>,
        /// Every read: (path, query).
        pub gets: Mutex<Vec<(String, Vec<(String, String)>)>>,
    }

    pub fn card(id: &str, customer: &str, brand: &str, last4: &str) -> Value {
        json!({
            "id": id, "type": "card", "customer": customer,
            "card": { "brand": brand, "last4": last4, "exp_month": 4, "exp_year": 2030 },
        })
    }

    impl FakeStripe {
        /// `cus_a` has `pm_a1` (default) and `pm_a2`; `cus_b` has `pm_b1` (default).
        pub fn two_customers() -> Self {
            let stripe = FakeStripe::default();
            stripe.add("cus_a", card("pm_a2", "cus_a", "mastercard", "4444"));
            stripe.add("cus_a", card("pm_a1", "cus_a", "visa", "4242"));
            stripe.add("cus_b", card("pm_b1", "cus_b", "amex", "0005"));
            stripe.defaults.lock().unwrap().insert("cus_a".into(), "pm_a1".into());
            stripe.defaults.lock().unwrap().insert("cus_b".into(), "pm_b1".into());
            stripe
        }

        pub fn add(&self, customer: &str, method: Value) {
            self.methods.lock().unwrap().push((customer.to_string(), method));
        }

        pub fn method_ids(&self, customer: &str) -> Vec<String> {
            self.methods
                .lock()
                .unwrap()
                .iter()
                .filter(|(owner, _)| owner == customer)
                .map(|(_, method)| method["id"].as_str().unwrap().to_string())
                .collect()
        }

        pub fn wrote_to(&self, needle: &str) -> bool {
            self.posts.lock().unwrap().iter().any(|(path, _)| path.contains(needle))
        }
    }

    fn query_value<'a>(query: &'a [(String, String)], key: &str) -> Option<&'a str> {
        query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[async_trait::async_trait]
    impl StripeCheckout for FakeStripe {
        async fn create_checkout_session(&self, _secret_key: &str, _form: &[(String, String)]) -> Result<String, ApiError> {
            Err(ApiError::Internal("not used".into()))
        }

        async fn create_billing_portal_session(&self, _secret_key: &str, _form: &[(String, String)]) -> Result<String, ApiError> {
            Err(ApiError::Internal("not used".into()))
        }

        async fn get_object(&self, _secret_key: &str, path: &str, query: &[(String, String)]) -> Result<Value, ApiError> {
            self.gets.lock().unwrap().push((path.to_string(), query.to_vec()));
            let parts: Vec<&str> = path.trim_start_matches("/v1/").split('/').collect();
            match parts.as_slice() {
                ["customers", customer] => {
                    let default = self.defaults.lock().unwrap().get(*customer).cloned();
                    Ok(json!({ "id": customer, "invoice_settings": { "default_payment_method": default } }))
                }
                ["customers", customer, "payment_methods"] => {
                    let data: Vec<Value> = self
                        .methods
                        .lock()
                        .unwrap()
                        .iter()
                        .rev()
                        .filter(|(owner, _)| owner == customer)
                        .map(|(_, method)| method.clone())
                        .collect();
                    Ok(json!({ "data": data, "has_more": false }))
                }
                ["subscriptions"] => {
                    let customer = query_value(query, "customer").unwrap_or_default();
                    let data = self.subscriptions.lock().unwrap().get(customer).cloned().unwrap_or_default();
                    Ok(json!({ "data": data, "has_more": false }))
                }
                ["invoices"] => {
                    // Stripe filters by `customer`; with none it lists the whole account.
                    let customer = query_value(query, "customer");
                    let data: Vec<Value> = self
                        .invoices
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|invoice| customer.map_or(true, |c| invoice["customer"].as_str() == Some(c)))
                        .cloned()
                        .collect();
                    Ok(json!({ "data": data, "has_more": false }))
                }
                _ => Err(ApiError::Internal(format!("fake stripe: no GET {path}"))),
            }
        }

        async fn post_object(
            &self,
            _secret_key: &str,
            path: &str,
            _idempotency_key: Option<&str>,
            form: &[(String, String)],
        ) -> Result<Value, ApiError> {
            self.posts.lock().unwrap().push((path.to_string(), form.to_vec()));
            let parts: Vec<&str> = path.trim_start_matches("/v1/").split('/').collect();
            match parts.as_slice() {
                ["customers", customer] => {
                    if let Some(id) = query_value(form, "invoice_settings[default_payment_method]") {
                        self.defaults.lock().unwrap().insert(customer.to_string(), id.to_string());
                    }
                    Ok(json!({ "id": customer }))
                }
                ["payment_methods", id, "detach"] => {
                    let mut methods = self.methods.lock().unwrap();
                    let before = methods.len();
                    methods.retain(|(_, method)| method["id"].as_str() != Some(*id));
                    if methods.len() == before {
                        return Err(ApiError::Internal("No such PaymentMethod".into()));
                    }
                    // Stripe clears an invoice default that was detached.
                    self.defaults.lock().unwrap().retain(|_, default| default.as_str() != *id);
                    Ok(json!({ "id": id, "customer": null }))
                }
                ["subscriptions", id] => Ok(json!({ "id": id })),
                ["setup_intents"] => Ok(json!({ "id": "seti_1", "client_secret": "seti_1_secret_test" })),
                _ => Err(ApiError::Internal(format!("fake stripe: no POST {path}"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    const KEY: &str = "sk_test_fake";

    #[tokio::test]
    async fn list_marks_the_default_first_and_only_the_callers_methods() {
        let stripe = FakeStripe::two_customers();
        let list = load_methods(&stripe, KEY, "cus_a").await.unwrap();
        let ids: Vec<&str> = list.payment_methods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["pm_a1", "pm_a2"]);
        assert_eq!(list.default_id.as_deref(), Some("pm_a1"));
        assert!(list.payment_methods[0].is_default);
        assert!(!list.payment_methods[1].is_default, "the second card is a backup");
        assert_eq!(list.payment_methods[0].brand, "visa");
        assert_eq!(list.payment_methods[0].last4.as_deref(), Some("4242"));
        assert_eq!(list.payment_methods[0].exp_year, Some(2030));
        assert_eq!(list.payment_methods[0].kind, "card");
    }

    #[test]
    fn a_method_naming_another_customer_is_dropped() {
        let other = card("pm_b1", "cus_b", "amex", "0005");
        assert!(method_view(&other, "cus_a", None).is_none());
        let link = json!({ "id": "pm_l", "type": "link", "customer": "cus_a", "link": { "email": "a@example.com" } });
        let view = method_view(&link, "cus_a", Some("pm_l")).unwrap();
        assert_eq!((view.kind, view.brand.as_str(), view.is_default), ("link", "link", true));
        let wallet = json!({ "id": "pm_x", "type": "us_bank_account", "customer": "cus_a" });
        assert!(method_view(&wallet, "cus_a", None).is_none(), "card and Link only");
    }

    #[tokio::test]
    async fn set_default_on_another_customers_method_is_404_and_writes_nothing() {
        let stripe = FakeStripe::two_customers();
        let result = set_default(&stripe, KEY, "cus_a", "pm_b1").await;
        assert!(matches!(result, Err(MethodError::NotFound)));
        assert!(stripe.posts.lock().unwrap().is_empty(), "no Stripe write for a foreign id");
        assert_eq!(stripe.defaults.lock().unwrap().get("cus_b").map(String::as_str), Some("pm_b1"));
        let response = MethodError::NotFound.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn remove_another_customers_method_is_404_and_detaches_nothing() {
        let stripe = FakeStripe::two_customers();
        let result = remove(&stripe, KEY, "cus_a", "pm_b1", false).await;
        assert!(matches!(result, Err(MethodError::NotFound)));
        assert!(!stripe.wrote_to("detach"));
        assert_eq!(stripe.method_ids("cus_b"), vec!["pm_b1".to_string()]);
    }

    #[tokio::test]
    async fn an_unknown_id_is_404_too() {
        let stripe = FakeStripe::two_customers();
        assert!(matches!(set_default(&stripe, KEY, "cus_a", "pm_nope").await, Err(MethodError::NotFound)));
        assert!(matches!(remove(&stripe, KEY, "cus_a", "pm_nope", false).await, Err(MethodError::NotFound)));
    }

    #[tokio::test]
    async fn backup_becomes_default_and_open_plans_follow() {
        let stripe = FakeStripe::two_customers();
        stripe.subscriptions.lock().unwrap().insert(
            "cus_a".into(),
            vec![
                json!({ "id": "sub_open", "status": "active", "customer": "cus_a" }),
                json!({ "id": "sub_old", "status": "canceled", "customer": "cus_a" }),
            ],
        );
        let list = set_default(&stripe, KEY, "cus_a", "pm_a2").await.unwrap();
        assert_eq!(list.default_id.as_deref(), Some("pm_a2"));
        assert_eq!(list.payment_methods[0].id, "pm_a2");
        assert!(stripe.wrote_to("/v1/subscriptions/sub_open"));
        assert!(!stripe.wrote_to("/v1/subscriptions/sub_old"), "a canceled plan is left alone");
    }

    #[tokio::test]
    async fn removing_the_default_with_an_open_plan_is_refused_with_a_plain_message() {
        let stripe = FakeStripe::two_customers();
        let result = remove(&stripe, KEY, "cus_a", "pm_a1", true).await;
        assert!(matches!(result, Err(MethodError::DefaultInUse)));
        assert!(!stripe.wrote_to("detach"));
        let response = MethodError::DefaultInUse.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], json!("default_payment_method_in_use"));
        assert!(body["message"].as_str().unwrap().contains("Make another card the default first"));
    }

    #[tokio::test]
    async fn a_backup_can_be_removed_while_a_plan_is_open() {
        let stripe = FakeStripe::two_customers();
        let list = remove(&stripe, KEY, "cus_a", "pm_a2", true).await.unwrap();
        assert_eq!(stripe.method_ids("cus_a"), vec!["pm_a1".to_string()]);
        assert_eq!(list.default_id.as_deref(), Some("pm_a1"));
    }

    #[tokio::test]
    async fn removing_the_default_without_a_plan_promotes_the_backup() {
        let stripe = FakeStripe::two_customers();
        let list = remove(&stripe, KEY, "cus_a", "pm_a1", false).await.unwrap();
        assert_eq!(list.default_id.as_deref(), Some("pm_a2"));
        assert_eq!(list.payment_methods.len(), 1);
        assert!(list.payment_methods[0].is_default);
    }

    #[tokio::test]
    async fn setup_intent_is_off_session_card_and_link_on_the_callers_customer() {
        let stripe = FakeStripe::two_customers();
        let body = create_setup_intent_secret(&stripe, KEY, "cus_a", "user_a", "payment_method_setup").await.unwrap();
        assert_eq!(body["client_secret"], json!("seti_1_secret_test"));
        let posts = stripe.posts.lock().unwrap();
        let (path, form) = posts.last().unwrap();
        assert_eq!(path, "/v1/setup_intents");
        for field in [
            ("customer", "cus_a"),
            ("usage", "off_session"),
            ("payment_method_types[0]", "card"),
            ("payment_method_types[1]", "link"),
            ("metadata[clerk_user_id]", "user_a"),
        ] {
            assert!(form.contains(&(field.0.to_string(), field.1.to_string())), "missing {field:?}");
        }
    }
}
