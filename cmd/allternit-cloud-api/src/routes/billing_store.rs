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
//! - `POST /api/v1/billing/store/apple/transactions` (Clerk session or API
//!   token with the `billing` scope). Body `{transaction_id}`. The server
//!   reads the transaction from the App Store Server API (D3), checks the
//!   bundle id and that its `appAccountToken` is the caller's account token
//!   (D4), applies the sandbox rule (D8), then records the plan and grants the
//!   credits once (`services::store_apple`). Answers
//!   `{granted, reason?, message?, subscription?, balance_usd?, idempotent_replay}`;
//!   `granted: false` with `reason` `sandbox_not_granted` (and the message to
//!   show) or `refunded` is still 200, so the app finishes the transaction.
//!   Refusals: 400 `invalid_transaction_id`, `wrong_app`, `unknown_product`,
//!   `account_token_missing`; 404 `transaction_not_found`; 409
//!   `owned_by_another_account`; 502 `store_unavailable`; 503
//!   `store_billing_not_configured`.
//! - `POST /api/v1/billing/store/apple/notifications` and
//!   `…/notifications/sandbox` (unauthenticated: App Store Server
//!   Notifications v2, `{signedPayload}`). The payload is decoded for ids
//!   only and the transaction re-read from Apple (D3). Answers 200 for every
//!   type, handled or not; 503 or 500 only for a transient failure (Apple
//!   down, store not configured, database error) so Apple retries.
//!
//! The Google routes (`…/google/purchases`, `…/google/rtdn`) come with the
//! Google store client (plan step 3).

use axum::{
    body::Bytes,
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
    services::{
        store_apple::{
            self, AppleApiError, AppleStoreClient, AppleSync, AppleSyncError, Suspension,
        },
        store_billing::{self, StoreConfig, StoreEnvironment, StoreItem, StoreProvider},
    },
    ApiState,
};

/// Serializes tests that set the store secrets in the process env.
#[cfg(test)]
pub(crate) static STORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/billing/store/preflight", post(preflight))
        .route(
            "/api/v1/billing/store/apple/transactions",
            post(apple_transaction),
        )
        .route(
            "/api/v1/billing/store/apple/notifications",
            post(apple_notification),
        )
        .route(
            "/api/v1/billing/store/apple/notifications/sandbox",
            post(apple_notification),
        )
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

#[derive(Debug, Deserialize)]
pub struct AppleTransactionRequest {
    #[serde(alias = "transactionId")]
    transaction_id: String,
}

/// `POST /api/v1/billing/store/apple/transactions`.
async fn apple_transaction(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<AppleTransactionRequest>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let transaction_id = request.transaction_id.trim();
    if !store_apple::valid_transaction_id(transaction_id) {
        return refusal(
            StatusCode::BAD_REQUEST,
            "invalid_transaction_id",
            "transaction_id must be an App Store transaction id.",
        );
    }
    let client = match apple_client() {
        Ok(client) => client,
        Err(response) => return response,
    };
    match AppleSync::from_env(&state.db, &client)
        .verify_transaction(&user.id, transaction_id)
        .await
    {
        Ok(outcome) => {
            run_suspensions(&state, &outcome.suspend);
            Json(outcome).into_response()
        }
        Err(error) => {
            tracing::warn!(user_id = %user.id, %transaction_id, %error, "apple transaction refused");
            apple_refusal(error)
        }
    }
}

#[derive(Debug, Deserialize)]
struct AppleNotificationBody {
    #[serde(rename = "signedPayload")]
    signed_payload: String,
}

/// `POST /api/v1/billing/store/apple/notifications` (and `/sandbox`).
async fn apple_notification(State(state): State<Arc<ApiState>>, body: Bytes) -> Response {
    let Ok(notification) = serde_json::from_slice::<AppleNotificationBody>(&body) else {
        return refusal(
            StatusCode::BAD_REQUEST,
            "invalid_notification",
            "Expected {\"signedPayload\": \"...\"}.",
        );
    };
    let client = match apple_client() {
        Ok(client) => client,
        Err(response) => return response,
    };
    match AppleSync::from_env(&state.db, &client)
        .handle_notification(&notification.signed_payload)
        .await
    {
        Ok(outcome) => {
            if let Some(sync) = &outcome.sync {
                run_suspensions(&state, &sync.suspend);
            }
            Json(serde_json::json!({
                "received": true,
                "notification_type": outcome.notification_type,
                "handled": outcome.handled,
            }))
            .into_response()
        }
        // Apple retries a non-2xx answer: only for what may pass.
        Err(AppleSyncError::Store(AppleApiError::Unavailable(detail))) => {
            tracing::error!(%detail, "apple notification: App Store unreachable, Apple will retry");
            refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "store_unavailable",
                "The App Store could not be reached.",
            )
        }
        Err(AppleSyncError::Api(error)) => {
            tracing::error!(%error, "apple notification failed, Apple will retry");
            error.into_response()
        }
        Err(error) => {
            tracing::error!(%error, "apple notification not applied");
            Json(serde_json::json!({ "received": true, "handled": false })).into_response()
        }
    }
}

/// The App Store client, or the 503 `store_billing_not_configured` answer
/// while the Apple secrets are unset or the key does not parse (D10).
fn apple_client() -> Result<AppleStoreClient, Response> {
    let not_configured = || PreflightRefusal::NotConfigured(StoreProvider::Apple).into_response();
    if !StoreConfig::from_env().apple {
        return Err(not_configured());
    }
    AppleStoreClient::from_env().map_err(|error| {
        tracing::error!(%error, "App Store client not configured");
        not_configured()
    })
}

/// Stop the cloud computer of each plan that went unpaid or canceled, as the
/// Stripe webhook does (fire and forget; a no-op without a computer).
pub(crate) fn run_suspensions(state: &ApiState, suspend: &[Suspension]) {
    for suspension in suspend {
        state.provisioning_service.spawn_suspend_for_subscription(
            &suspension.user_id,
            &suspension.subscription_key,
            suspension.reason,
        );
    }
}

fn refusal(status: StatusCode, error: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": error, "message": message })),
    )
        .into_response()
}

fn apple_refusal(error: AppleSyncError) -> Response {
    match error {
        AppleSyncError::OwnedByAnotherAccount => {
            store_billing::StoreError::OwnedByAnotherAccount.into_response()
        }
        AppleSyncError::AccountTokenMissing => refusal(
            StatusCode::BAD_REQUEST,
            "account_token_missing",
            "This purchase is not linked to an Allternit account. Sign in to the account that bought it and use Restore Purchases.",
        ),
        AppleSyncError::WrongApp => refusal(
            StatusCode::BAD_REQUEST,
            "wrong_app",
            "This purchase was made in a different app.",
        ),
        AppleSyncError::UnknownProduct(_) => refusal(
            StatusCode::BAD_REQUEST,
            "unknown_product",
            "This purchase is not an Allternit plan or credit pack.",
        ),
        AppleSyncError::NotFound => refusal(
            StatusCode::NOT_FOUND,
            "transaction_not_found",
            "The App Store has no record of this purchase.",
        ),
        AppleSyncError::Store(AppleApiError::NotConfigured(_)) => {
            PreflightRefusal::NotConfigured(StoreProvider::Apple).into_response()
        }
        AppleSyncError::Store(_) => refusal(
            StatusCode::BAD_GATEWAY,
            "store_unavailable",
            "Couldn't reach the App Store. Try again in a moment.",
        ),
        AppleSyncError::Api(error) => error.into_response(),
    }
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

    // ---- Apple purchases (plan step 2), against a fake App Store Server API ----

    use crate::services::store_apple::{
        AppleRenewalInfo, AppleStoreApi, AppleSubscriptionStatus, AppleTransaction,
        REFUND_REVERSED_SOURCE, REFUND_SOURCE, SANDBOX_MESSAGE,
    };
    use crate::services::SuspendReason;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use uuid::Uuid;

    const BUNDLE: &str = "com.allternit.mobile";
    const PLUS: &str = "com.allternit.mobile.plan.plus.monthly";
    const ULTRA: &str = "com.allternit.mobile.plan.ultra.monthly";
    const PACK_25: &str = "com.allternit.mobile.credits.25";

    /// The App Store Server API as Apple would answer it now.
    #[derive(Default)]
    struct FakeApple {
        transactions: Mutex<HashMap<String, AppleTransaction>>,
        subscriptions: Mutex<HashMap<String, AppleSubscriptionStatus>>,
    }

    impl FakeApple {
        fn put(&self, tx: AppleTransaction) {
            self.transactions
                .lock()
                .unwrap()
                .insert(tx.transaction_id.clone(), tx);
        }

        /// A plan's status, with `latest` as its newest transaction.
        fn plan(&self, status: i64, latest: AppleTransaction) {
            self.put(latest.clone());
            self.subscriptions.lock().unwrap().insert(
                latest.original_transaction_id.clone(),
                AppleSubscriptionStatus {
                    status,
                    transaction: latest,
                    renewal: Some(AppleRenewalInfo::default()),
                },
            );
        }
    }

    #[async_trait::async_trait]
    impl AppleStoreApi for FakeApple {
        async fn transaction(&self, id: &str) -> Result<Option<AppleTransaction>, AppleApiError> {
            Ok(self.transactions.lock().unwrap().get(id).cloned())
        }

        async fn subscription(
            &self,
            id: &str,
        ) -> Result<Option<AppleSubscriptionStatus>, AppleApiError> {
            Ok(self.subscriptions.lock().unwrap().get(id).cloned())
        }
    }

    fn apple_tx(
        id: &str,
        original: &str,
        product: &str,
        token: Uuid,
        environment: &str,
    ) -> AppleTransaction {
        AppleTransaction {
            transaction_id: id.to_string(),
            original_transaction_id: original.to_string(),
            bundle_id: BUNDLE.to_string(),
            product_id: product.to_string(),
            app_account_token: Some(token.to_string()),
            environment: environment.to_string(),
            expires_date: Some((chrono::Utc::now().timestamp() + 30 * 86_400) * 1000),
            revocation_date: None,
        }
    }

    fn apple_sync<'a>(db: &'a PgPool, api: &'a FakeApple, sandbox: &[&str]) -> AppleSync<'a> {
        AppleSync {
            db,
            api,
            bundle_id: BUNDLE.to_string(),
            sandbox_grant_user_ids: sandbox.iter().map(|id| id.to_string()).collect(),
        }
    }

    /// A v2 `signedPayload` naming `tx` (signatures are not read, D3).
    fn notification(kind: &str, tx: &AppleTransaction) -> String {
        let jws = |payload: Value| {
            format!(
                "{}.{}.sig",
                URL_SAFE_NO_PAD.encode(r#"{"alg":"ES256"}"#),
                URL_SAFE_NO_PAD.encode(payload.to_string())
            )
        };
        let signed_tx = jws(serde_json::json!({
            "transactionId": tx.transaction_id,
            "originalTransactionId": tx.original_transaction_id,
        }));
        jws(serde_json::json!({
            "notificationType": kind,
            "data": { "bundleId": BUNDLE, "environment": tx.environment, "signedTransactionInfo": signed_tx },
        }))
    }

    /// The tables a grant, a refund and a plan tier change write, as
    /// production has them (users, plan tiers, entitlement events, wallet,
    /// ledger, processed-event dedupe). `user_runtime_quotas` comes from
    /// `test_pool`.
    async fn ledger_tables(db: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                last_login_at TIMESTAMPTZ
            );
            CREATE TABLE plan_tiers (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                max_active_devices BIGINT NOT NULL DEFAULT 1,
                max_pairings_per_day BIGINT NOT NULL DEFAULT 5,
                max_relay_sockets BIGINT NOT NULL DEFAULT 5,
                max_relay_mb_per_day BIGINT NOT NULL DEFAULT 100,
                max_hosted_runtime_hours_monthly BIGINT NOT NULL DEFAULT 0,
                can_create_hosted_runtime BOOLEAN NOT NULL DEFAULT FALSE,
                max_hosted_runtimes BIGINT NOT NULL DEFAULT 0,
                max_hosted_runtime_memory_mb BIGINT NOT NULL DEFAULT 0,
                hard_spend_cap_usd DOUBLE PRECISION
            );
            INSERT INTO plan_tiers (id, display_name) VALUES ('free', 'Free'), ('pro', 'Pro'), ('team', 'Team');
            CREATE TABLE billing_entitlement_events (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                previous_plan_tier_id TEXT,
                plan_tier_id TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'billing',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            CREATE TABLE user_credits (
                user_id TEXT PRIMARY KEY,
                balance_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            CREATE TABLE credit_transactions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                amount_usd DOUBLE PRECISION NOT NULL,
                transaction_id TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            CREATE TABLE webhook_events (
                id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                target TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            INSERT INTO users (id, email) VALUES
                ('user_a', 'a@example.com'), ('user_b', 'b@example.com'), ('review_user', 'r@example.com');
            "#,
        )
        .execute(db)
        .await
        .unwrap();
    }

    async fn apple_db() -> PgPool {
        let db = crate::routes::test_support::test_pool().await;
        billing_tables(&db).await;
        ledger_tables(&db).await;
        db
    }

    async fn balance(db: &PgPool, user_id: &str) -> f64 {
        sqlx::query_scalar(
            "SELECT COALESCE((SELECT balance_usd FROM user_credits WHERE user_id = $1), 0)",
        )
        .bind(user_id)
        .fetch_one(db)
        .await
        .unwrap()
    }

    async fn count(db: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
    }

    async fn tier(db: &PgPool, user_id: &str) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT plan_tier_id FROM user_runtime_quotas WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(db)
        .await
        .unwrap()
        .flatten()
    }

    /// D6: the app posting the same purchase twice grants it once.
    #[tokio::test]
    async fn apple_replayed_transaction_grants_once() {
        let db = apple_db().await;
        let token = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        let apple = FakeApple::default();
        apple.put(apple_tx("300", "300", PACK_25, token, "Production"));
        let sync = apple_sync(&db, &apple, &[]);

        let first = sync.verify_transaction("user_a", "300").await.unwrap();
        assert!(first.granted && !first.idempotent_replay);
        assert_eq!(first.balance_usd, Some(25.0));
        let again = sync.verify_transaction("user_a", "300").await.unwrap();
        assert!(
            again.granted && again.idempotent_replay,
            "a replay is reported as one"
        );
        assert_eq!(balance(&db, "user_a").await, 25.0, "credits granted once");
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM credit_transactions WHERE transaction_id = 'apple-300'"
            )
            .await,
            1
        );
    }

    /// D4: a purchase whose appAccountToken belongs to another account is
    /// refused with 409 and grants nothing; its owner can still claim it.
    #[tokio::test]
    async fn apple_wrong_account_token_is_409() {
        let db = apple_db().await;
        let token_a = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        store_billing::issue_account_token(&db, "user_b", None)
            .await
            .unwrap();
        let apple = FakeApple::default();
        apple.put(apple_tx("301", "301", PACK_25, token_a, "Production"));
        let sync = apple_sync(&db, &apple, &[]);

        let error = sync.verify_transaction("user_b", "301").await.unwrap_err();
        assert!(matches!(error, AppleSyncError::OwnedByAnotherAccount));
        let response = apple_refusal(error);
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(json(response).await["error"], "owned_by_another_account");
        assert_eq!(balance(&db, "user_b").await, 0.0);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM billing_store_transactions").await,
            0
        );

        assert!(
            sync.verify_transaction("user_a", "301")
                .await
                .unwrap()
                .granted
        );
        assert_eq!(balance(&db, "user_a").await, 25.0);
    }

    /// D7: REFUND takes the credits back once (also when Apple retries and
    /// when the app re-posts the refunded purchase); REFUND_REVERSED gives
    /// them back once.
    #[tokio::test]
    async fn apple_refund_takes_credits_once_and_reversal_gives_back_once() {
        let db = apple_db().await;
        let token = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        let apple = FakeApple::default();
        let paid = apple_tx("302", "302", PACK_25, token, "Production");
        apple.put(paid.clone());
        let sync = apple_sync(&db, &apple, &[]);
        sync.verify_transaction("user_a", "302").await.unwrap();
        assert_eq!(balance(&db, "user_a").await, 25.0);

        let mut refunded = paid.clone();
        refunded.revocation_date = Some(chrono::Utc::now().timestamp_millis());
        apple.put(refunded.clone());
        for _ in 0..2 {
            let outcome = sync
                .handle_notification(&notification("REFUND", &refunded))
                .await
                .unwrap();
            assert!(outcome.handled);
        }
        let repost = sync.verify_transaction("user_a", "302").await.unwrap();
        assert!(!repost.granted);
        assert_eq!(repost.reason, Some("refunded"));
        assert_eq!(balance(&db, "user_a").await, 0.0, "taken back once");
        assert_eq!(
            count(
                &db,
                &format!(
                    "SELECT COUNT(*) FROM credit_transactions WHERE source = '{REFUND_SOURCE}'"
                )
            )
            .await,
            1
        );

        apple.put(paid.clone());
        for _ in 0..2 {
            sync.handle_notification(&notification("REFUND_REVERSED", &paid))
                .await
                .unwrap();
        }
        assert_eq!(balance(&db, "user_a").await, 25.0, "given back once");
        assert_eq!(
            count(&db, &format!("SELECT COUNT(*) FROM credit_transactions WHERE source = '{REFUND_REVERSED_SOURCE}'")).await,
            1
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM billing_store_transactions WHERE transaction_id = '302' AND state = 'granted'").await,
            1
        );
    }

    /// D8: a sandbox purchase (TestFlight, App Review) by someone outside
    /// STORE_SANDBOX_GRANT_USER_IDS grants no credits, no plan row and no plan
    /// tier; an allow-listed account gets the plan.
    #[tokio::test]
    async fn apple_sandbox_for_an_unlisted_user_grants_nothing() {
        let db = apple_db().await;
        let token = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        let review = store_billing::issue_account_token(&db, "review_user", None)
            .await
            .unwrap();
        let apple = FakeApple::default();
        apple.plan(1, apple_tx("400", "400", PLUS, token, "Sandbox"));
        let sync = apple_sync(&db, &apple, &["review_user"]);

        let outcome = sync.verify_transaction("user_a", "400").await.unwrap();
        assert!(!outcome.granted);
        assert_eq!(outcome.reason, Some("sandbox_not_granted"));
        assert_eq!(outcome.message, Some(SANDBOX_MESSAGE));
        assert_eq!(balance(&db, "user_a").await, 0.0);
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM billing_subscriptions WHERE user_id = 'user_a'"
            )
            .await,
            0
        );
        assert_eq!(tier(&db, "user_a").await, None);
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM billing_store_transactions WHERE transaction_id = '400' AND state = 'sandbox_not_granted'").await,
            1
        );

        apple.plan(1, apple_tx("401", "401", PLUS, review, "Sandbox"));
        let outcome = sync.verify_transaction("review_user", "401").await.unwrap();
        assert!(outcome.granted);
        assert_eq!(outcome.subscription.unwrap().status, "active");
        assert_eq!(balance(&db, "review_user").await, 22.0);
        assert_eq!(tier(&db, "review_user").await.as_deref(), Some("pro"));
    }

    /// D6/D14: each renewal grants the month's credits once (Apple retries
    /// DID_RENEW), on the same plan row; an upgrade inside the group updates
    /// that row too.
    #[tokio::test]
    async fn apple_renewal_grants_the_month_once() {
        let db = apple_db().await;
        let token = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        let apple = FakeApple::default();
        apple.plan(1, apple_tx("500", "500", PLUS, token, "Production"));
        let sync = apple_sync(&db, &apple, &[]);

        let bought = sync.verify_transaction("user_a", "500").await.unwrap();
        assert!(bought.granted);
        let plan = bought.subscription.unwrap();
        assert_eq!(
            (plan.plan_id, plan.status.as_str(), plan.billing_provider),
            ("plus", "active", "apple")
        );
        assert_eq!(balance(&db, "user_a").await, 22.0);
        assert_eq!(tier(&db, "user_a").await.as_deref(), Some("pro"));

        let renewal = apple_tx("501", "500", PLUS, token, "Production");
        apple.plan(1, renewal.clone());
        for _ in 0..2 {
            assert!(
                sync.handle_notification(&notification("DID_RENEW", &renewal))
                    .await
                    .unwrap()
                    .handled
            );
        }
        // Rollover: the $10 cap of the $22 left, plus the new month.
        assert_eq!(balance(&db, "user_a").await, 32.0);
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM credit_transactions WHERE source = 'apple_subscription'"
            )
            .await,
            2
        );

        let upgrade = apple_tx("502", "500", ULTRA, token, "Production");
        apple.plan(1, upgrade.clone());
        sync.handle_notification(&notification("DID_CHANGE_RENEWAL_PREF", &upgrade))
            .await
            .unwrap();
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) FROM billing_subscriptions WHERE user_id = 'user_a'"
            )
            .await,
            1
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM billing_subscriptions WHERE stripe_subscription_id = 'apple:500' AND plan_id = 'ultra'").await,
            1
        );
        assert_eq!(tier(&db, "user_a").await.as_deref(), Some("team"));
    }

    /// D13/D14: once the period end passed, the subscription read re-asks
    /// Apple (not again within 10 minutes); expired becomes canceled, the
    /// tier drops to the default and the computer is stopped.
    #[tokio::test]
    async fn apple_expired_plan_is_canceled_on_refresh() {
        let db = apple_db().await;
        let token = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        let apple = FakeApple::default();
        let mut lapsed = apple_tx("600", "600", PLUS, token, "Production");
        lapsed.expires_date = Some((chrono::Utc::now().timestamp() - 3_600) * 1000);
        apple.plan(1, lapsed.clone());
        let sync = apple_sync(&db, &apple, &[]);
        sync.verify_transaction("user_a", "600").await.unwrap();

        apple.plan(2, lapsed);
        assert!(sync.refresh_due_plans("user_a").await.unwrap().is_empty());
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM billing_subscriptions WHERE user_id = 'user_a' AND status = 'active'").await,
            1,
            "checked less than 10 minutes ago: Apple is not asked again"
        );

        sqlx::query(
            "UPDATE billing_subscriptions SET store_checked_at = NOW() - INTERVAL '11 minutes'",
        )
        .execute(&db)
        .await
        .unwrap();
        let suspend = sync.refresh_due_plans("user_a").await.unwrap();
        assert_eq!(suspend.len(), 1);
        assert_eq!(suspend[0].reason, SuspendReason::Cancelled);
        assert_eq!(suspend[0].subscription_key, "apple:600");
        assert_eq!(
            count(&db, "SELECT COUNT(*) FROM billing_subscriptions WHERE user_id = 'user_a' AND status = 'canceled'").await,
            1
        );
        assert_eq!(tier(&db, "user_a").await.as_deref(), Some("free"));
        assert!(
            sync.refresh_due_plans("user_a").await.unwrap().is_empty(),
            "a canceled plan is not re-read"
        );
    }

    /// D7: a refunded plan takes its month's credits back and drops to the
    /// default tier.
    #[tokio::test]
    async fn apple_refunded_plan_drops_to_the_default_tier() {
        let db = apple_db().await;
        let token = store_billing::issue_account_token(&db, "user_a", None)
            .await
            .unwrap();
        let apple = FakeApple::default();
        let paid = apple_tx("700", "700", PLUS, token, "Production");
        apple.plan(1, paid.clone());
        let sync = apple_sync(&db, &apple, &[]);
        sync.verify_transaction("user_a", "700").await.unwrap();
        assert_eq!(tier(&db, "user_a").await.as_deref(), Some("pro"));

        let mut refunded = paid;
        refunded.revocation_date = Some(chrono::Utc::now().timestamp_millis());
        apple.plan(5, refunded.clone());
        let outcome = sync
            .handle_notification(&notification("REFUND", &refunded))
            .await
            .unwrap();
        let synced = outcome.sync.unwrap();
        assert_eq!(synced.subscription.unwrap().status, "canceled");
        assert_eq!(synced.suspend.len(), 1);
        assert_eq!(balance(&db, "user_a").await, 0.0);
        assert_eq!(tier(&db, "user_a").await.as_deref(), Some("free"));
    }
}
