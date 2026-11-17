//! Store purchases (Apple In-App Purchase, Google Play Billing): routes under
//! the unique prefix `/api/v1/billing/store/*` (plan D12), so they can never
//! overlap the Stripe billing routes.
//!
//! - `POST /api/v1/billing/store/preflight` (Clerk session or API token with
//!   the `billing` scope). Body `{provider, kind, id, environment?}`:
//!   `provider` is `apple` or `google`, `kind` is `plan` or `pack`, `id` is the
//!   server catalog id (`plus`, `super`, `ultra`, `credits_25`, …) or that
//!   store's product id, `environment` is `production` (default) or `sandbox`.
//!   Answers `{account_token, product_id}`: the per-user UUID the app passes
//!   to the store (Apple `appAccountToken`, D4) and the product to buy.
//!   Refusals:
//!   - 400 for an unknown provider, kind, id or environment;
//!   - 503 `store_billing_not_configured` while that store's secrets are unset (D10);
//!   - 409 `already_subscribed` with `billing_provider` and `manage_url` when a
//!     plan is already open and this purchase would bill twice (D5);
//!   - 403 `first_purchase_limit` when the credit amount is over the
//!     first-purchase cap (D9; not for sandbox).
//!
//! The verify and notification routes (`…/apple/transactions`,
//! `…/apple/notifications`, `…/google/purchases`, `…/google/rtdn`) come with
//! the Apple and Google store clients (plan steps 2 and 3).

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;

use crate::{
    error::ApiError,
    routes::billing_subscriptions::{self, OpenSubscription},
    services::store_billing::{self, StoreConfig, StoreEnvironment, StoreItem, StoreProvider},
    ApiState,
};

/// Serializes tests that set the store secrets in the process env.
#[cfg(test)]
pub(crate) static STORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/billing/store/preflight", post(preflight))
}

#[derive(Debug, Deserialize)]
pub struct PreflightRequest {
    provider: String,
    kind: String,
    id: String,
    #[serde(default)]
    environment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PreflightResponse {
    /// The account's store token (UUID), the same on every call.
    account_token: String,
    /// The store product to buy.
    product_id: &'static str,
}

/// Why preflight refused.
#[derive(Debug)]
enum PreflightRefusal {
    NotConfigured(StoreProvider),
    AlreadySubscribed(OpenSubscription),
    /// The first-purchase cap's message.
    FirstPurchaseLimit(String),
    Api(ApiError),
}

impl From<ApiError> for PreflightRefusal {
    fn from(error: ApiError) -> Self {
        PreflightRefusal::Api(error)
    }
}

impl IntoResponse for PreflightRefusal {
    fn into_response(self) -> Response {
        match self {
            PreflightRefusal::NotConfigured(provider) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "store_billing_not_configured",
                    "provider": provider.as_str(),
                    "message": "Buying in the app isn't available yet. You can buy on the web.",
                })),
            )
                .into_response(),
            PreflightRefusal::AlreadySubscribed(current) => {
                billing_subscriptions::already_subscribed_response(&current)
            }
            PreflightRefusal::FirstPurchaseLimit(message) => (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": "first_purchase_limit",
                    "message": message,
                })),
            )
                .into_response(),
            PreflightRefusal::Api(error) => error.into_response(),
        }
    }
}

async fn preflight(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<PreflightRequest>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    match preflight_for(
        &state.db,
        &user.id,
        user.organization_id.as_deref(),
        &request,
        &StoreConfig::from_env(),
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// The preflight decision, in order: a valid request, the store switched on,
/// no second plan, the first-purchase cap, an org when the fabric ledger needs
/// one, then the account token.
async fn preflight_for(
    db: &PgPool,
    user_id: &str,
    organization_id: Option<&str>,
    request: &PreflightRequest,
    config: &StoreConfig,
) -> Result<PreflightResponse, PreflightRefusal> {
    let provider = StoreProvider::parse(&request.provider).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "Unknown store: {:?}. Use \"apple\" or \"google\".",
            request.provider
        ))
    })?;
    let environment = match request.environment.as_deref() {
        None => StoreEnvironment::Production,
        Some(value) => StoreEnvironment::parse(value).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Unknown environment: {value:?}. Use \"production\" or \"sandbox\"."
            ))
        })?,
    };
    let item = store_billing::find_item(&request.kind, &request.id, provider).ok_or_else(|| {
        ApiError::BadRequest(format!("Unknown {} {:?}.", request.kind, request.id))
    })?;
    if !config.enabled(provider) {
        return Err(PreflightRefusal::NotConfigured(provider));
    }

    if let StoreItem::Plan(plan) = item {
        let open = billing_subscriptions::open_subscriptions(db, user_id).await?;
        if let Some(blocking) = blocking_plan(&open, provider, plan.id) {
            return Err(PreflightRefusal::AlreadySubscribed(blocking.clone()));
        }
    }

    // D9: the cap is on the credits granted, so the $29.99 pack ($25 of
    // credits) passes a $25 cap. Sandbox purchases are free and skip it.
    if environment == StoreEnvironment::Production {
        let trusted = billing_subscriptions::is_trusted_purchaser(db, user_id).await?;
        let unlock_hint = match item {
            StoreItem::Plan(_) => "The Max and Ultra plans",
            StoreItem::Pack(_) => "The $50 and $100 packs",
        };
        billing_subscriptions::ensure_purchase_allowed(trusted, item.credits_usd(), unlock_hint)
            .map_err(|error| match error {
                ApiError::Forbidden(message) => PreflightRefusal::FirstPurchaseLimit(message),
                other => PreflightRefusal::Api(other),
            })?;
    }

    // Same rule as Stripe packs: with the fabric ledger bridge on, pack
    // credits go to an organization, so one must be selected.
    if matches!(item, StoreItem::Pack(_))
        && crate::services::fabric_ledger::fabric_bridge_configured()
        && organization_id.is_none()
    {
        return Err(ApiError::BadRequest(
            "Credit purchases require an active Clerk organization: sign in again with an organization selected."
                .to_string(),
        )
        .into());
    }

    let token = store_billing::issue_account_token(db, user_id, organization_id).await?;
    Ok(PreflightResponse {
        account_token: token.to_string(),
        product_id: item.product_id(provider),
    })
}

/// The open plan that stops buying `plan_id` from `provider`, if any (D5).
/// A plan billed by the same store can be changed there (an upgrade inside
/// the Apple subscription group, a Play replacement), which bills once, so
/// only buying the plan the account is already on is refused. A plan billed
/// by anyone else would bill twice.
fn blocking_plan<'a>(
    open: &'a [OpenSubscription],
    provider: StoreProvider,
    plan_id: &str,
) -> Option<&'a OpenSubscription> {
    open.iter()
        .find(|sub| sub.provider != provider.as_str() || sub.plan_id == plan_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt;

    /// The billing tables these routes read, as production has them:
    /// `billing_subscriptions` (005 + 116) and the trust table (008), then the
    /// real migration 148 on top.
    async fn billing_tables(db: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE billing_subscriptions (
                stripe_subscription_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                plan_id TEXT NOT NULL,
                plan_tier TEXT NOT NULL,
                status TEXT NOT NULL,
                stripe_customer_id TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                current_period_end TIMESTAMPTZ
            );
            CREATE TABLE user_billing_accounts (
                user_id TEXT PRIMARY KEY,
                stripe_customer_id TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            CREATE TABLE billing_purchase_trust (
                user_id TEXT PRIMARY KEY,
                first_paid_at TIMESTAMPTZ,
                paid_purchase_count INT NOT NULL DEFAULT 0
            );
            "#,
        )
        .execute(db)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations_pg/148_store_billing.sql"))
            .execute(db)
            .await
            .expect("148 applies");
    }

    fn apple_on() {
        std::env::set_var("APPLE_IAP_ISSUER_ID", "issuer");
        std::env::set_var("APPLE_IAP_KEY_ID", "key");
        std::env::set_var("APPLE_IAP_PRIVATE_KEY", "-----BEGIN PRIVATE KEY-----");
    }

    fn apple_off() {
        for name in store_billing::APPLE_ENV {
            std::env::remove_var(name);
        }
    }

    fn post(path: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", "Bearer dev-api-token")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 16).await.unwrap()).unwrap()
    }

    /// Through the real router: a Stripe subscriber asking to buy a plan in
    /// the App Store gets 409 `already_subscribed` naming Stripe and where to
    /// manage it, and no account token. A pack still preflights.
    #[tokio::test]
    #[serial_test::serial]
    async fn preflight_refuses_a_plan_for_a_stripe_subscriber() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        billing_tables(&state.db).await;
        sqlx::query(
            "INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, plan_id, plan_tier, status) \
             VALUES ('sub_1', 'dev-user', 'plus', 'pro', 'active')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let app = crate::create_router(state.clone());

        let _dev = crate::auth::dev_token::DEV_TOKEN_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _store = STORE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(crate::auth::dev_token::ALLOW_DEV_TOKEN_ENV, "1");
        apple_on();

        let res = app
            .clone()
            .oneshot(post(
                "/api/v1/billing/store/preflight",
                serde_json::json!({ "provider": "apple", "kind": "plan", "id": "super", "environment": "sandbox" }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body = json(res).await;
        assert_eq!(body["error"], "already_subscribed");
        assert_eq!(body["current_plan_id"], "plus");
        assert_eq!(body["billing_provider"], "stripe");
        assert_eq!(body["manage_url"], "https://platform.allternit.com/billing");
        assert!(body.get("account_token").is_none());

        // Packs are not plans: a Stripe subscriber can still buy credits.
        let res = app
            .clone()
            .oneshot(post(
                "/api/v1/billing/store/preflight",
                serde_json::json!({ "provider": "apple", "kind": "pack", "id": "credits_25" }),
            ))
            .await
            .unwrap();
        apple_off();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json(res).await;
        assert_eq!(body["product_id"], "com.allternit.mobile.credits.25");
        assert!(uuid::Uuid::parse_str(body["account_token"].as_str().unwrap()).is_ok());
    }

    /// Through the real router: the Stripe portal answers 409 `store_billed`
    /// for a plan Apple bills, before any Stripe configuration is consulted.
    #[tokio::test]
    #[serial_test::serial]
    async fn portal_refuses_a_store_billed_plan() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        billing_tables(&state.db).await;
        let plus = billing_subscriptions::find_plan("plus").unwrap();
        store_billing::upsert_store_subscription(
            &state.db,
            &store_billing::StoreSubscriptionUpdate {
                provider: StoreProvider::Apple,
                store_id: "2000000111",
                user_id: "dev-user",
                plan: plus,
                status: "active",
                environment: StoreEnvironment::Production,
                period_end: Some(1_800_000_000),
            },
        )
        .await
        .unwrap();
        let app = crate::create_router(state.clone());

        let _dev = crate::auth::dev_token::DEV_TOKEN_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(crate::auth::dev_token::ALLOW_DEV_TOKEN_ENV, "1");

        let res = app
            .clone()
            .oneshot(post("/api/v1/billing/portal", serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CONFLICT);
        let body = json(res).await;
        assert_eq!(body["error"], "store_billed");
        assert_eq!(body["billing_provider"], "apple");
        assert_eq!(
            body["manage_url"],
            "https://apps.apple.com/account/subscriptions"
        );

        // The subscription read names the store and finds no conflict.
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/billing/subscription")
                    .header("authorization", "Bearer dev-api-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = json(res).await;
        assert_eq!(body["plan_id"], "plus");
        assert_eq!(body["status"], "active");
        assert_eq!(body["billing_provider"], "apple");
        assert_eq!(body["conflict"], false);
    }

    /// D4: one token per user, stable across calls, different per user; a
    /// later preflight with an org records it without changing the token.
    #[tokio::test]
    async fn account_token_is_stable_per_user_and_unique() {
        let db = crate::routes::test_support::test_pool().await;
        billing_tables(&db).await;
        let first = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        let again = store_billing::issue_account_token(&db, "user_a", Some("org_1"))
            .await
            .unwrap();
        let other = store_billing::issue_account_token(&db, "user_b", None)
            .await
            .unwrap();
        assert_eq!(first, again, "the token is stable per user");
        assert_ne!(first, other, "each user gets their own token");
        let account = store_billing::account_for_token(&db, first)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(account.user_id, "user_a");
        assert_eq!(account.organization_id.as_deref(), Some("org_1"));
        // A preflight without an org keeps the one captured before.
        store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        assert_eq!(
            store_billing::organization_for_user(&db, "user_a")
                .await
                .unwrap()
                .as_deref(),
            Some("org_1")
        );
    }

    /// D1/D5: a store plan never moves to another account, and a second open
    /// plan is reported as a conflict on the subscription read.
    #[tokio::test]
    async fn store_plan_rows_keep_their_owner_and_flag_conflicts() {
        let db = crate::routes::test_support::test_pool().await;
        billing_tables(&db).await;
        let plus = billing_subscriptions::find_plan("plus").unwrap();
        let update = |user_id: &'static str| store_billing::StoreSubscriptionUpdate {
            provider: StoreProvider::Apple,
            store_id: "2000000222",
            user_id,
            plan: plus,
            status: "active",
            environment: StoreEnvironment::Production,
            period_end: None,
        };
        let outcome = store_billing::upsert_store_subscription(&db, &update("user_a"))
            .await
            .unwrap();
        assert_eq!(outcome.key, "apple:2000000222");
        assert!(!outcome.conflict);
        assert!(matches!(
            store_billing::upsert_store_subscription(&db, &update("user_b")).await,
            Err(store_billing::StoreError::OwnedByAnotherAccount)
        ));

        sqlx::query(
            "INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, plan_id, plan_tier, status) \
             VALUES ('sub_9', 'user_a', 'super', 'team', 'active')",
        )
        .execute(&db)
        .await
        .unwrap();
        let outcome = store_billing::upsert_store_subscription(&db, &update("user_a"))
            .await
            .unwrap();
        assert!(
            outcome.conflict,
            "a Stripe plan and an App Store plan are both open"
        );
        let open = billing_subscriptions::open_subscriptions(&db, "user_a")
            .await
            .unwrap();
        assert_eq!(open.len(), 2);
        assert!(
            billing_subscriptions::store_billed_plan(&open).is_none(),
            "with a Stripe plan open the portal stays reachable to cancel it"
        );
    }

    /// D5 with D14: the store that bills a plan can change it (one bill);
    /// buying the same plan again, or a plan from another biller, is refused.
    #[test]
    fn only_a_plan_change_in_the_same_store_passes_preflight() {
        let apple_plus = [OpenSubscription {
            plan_id: "plus".to_string(),
            provider: "apple".to_string(),
        }];
        assert!(
            blocking_plan(&apple_plus, StoreProvider::Apple, "ultra").is_none(),
            "upgrade inside the Apple group"
        );
        assert!(
            blocking_plan(&apple_plus, StoreProvider::Apple, "plus").is_some(),
            "already on Plus"
        );
        assert!(
            blocking_plan(&apple_plus, StoreProvider::Google, "ultra").is_some(),
            "Google would bill twice"
        );
        let stripe_plus = [OpenSubscription {
            plan_id: "plus".to_string(),
            provider: "stripe".to_string(),
        }];
        assert!(
            blocking_plan(&stripe_plus, StoreProvider::Apple, "ultra").is_some(),
            "Stripe would bill twice"
        );
        assert!(blocking_plan(&[], StoreProvider::Apple, "plus").is_none());
    }
}
