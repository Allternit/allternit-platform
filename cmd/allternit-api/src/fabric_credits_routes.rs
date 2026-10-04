//! Fabric credits routes — balance/history, org credits grants, and the
//! purchase endpoint (honesty-gated: it never self-credits; with the checkout
//! flag on it delegates to the platform billing checkout, otherwise it
//! answers 409 "not enabled").
//!
//! Merged into the `/api/v1` chain in `main.rs`, so public paths land at
//! `/api/v1/credits/*` and admin paths at `/api/v1/admin/credits/*`.

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{
    auth::AuthUser,
    fabric::credits::{CreditLedgerEntry, CreditsError, CreditsLedger, TransactionType},
    AppState,
};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/credits/balance", get(get_balance))
        .route("/credits/transactions", get(list_transactions))
        .route("/credits/purchase", post(purchase_credits))
        .route("/credits/transfer_from_wallet", post(transfer_from_wallet))
        .route("/admin/credits/grant", post(admin_grant_credits))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: Json<Value>,
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: Json(json!({"error": code, "message": message.into()})),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, self.body).into_response()
    }
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(status, code, message)
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "fabric credits operation failed");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        err.to_string(),
    )
}

fn credits_error(err: CreditsError) -> ApiError {
    match err {
        CreditsError::InsufficientCredits { .. } => {
            error(StatusCode::PAYMENT_REQUIRED, "insufficient_credits", err.to_string())
        }
        CreditsError::InvalidAmount(_) => {
            error(StatusCode::BAD_REQUEST, "invalid_amount", err.to_string())
        }
        CreditsError::HoldNotFound(_) | CreditsError::HoldAlreadyFinalized(_) => {
            error(StatusCode::CONFLICT, "hold_error", err.to_string())
        }
        CreditsError::Db(e) => internal(e),
    }
}

fn require_org(user: &AuthUser) -> Result<String, ApiError> {
    user.organization_id.clone().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required.",
        )
    })
}

fn require_org_admin(conn: &rusqlite::Connection, user: &AuthUser) -> Result<String, ApiError> {
    let org = require_org(user)?;
    if !crate::rbac::is_org_admin(conn, &org, &user.user_id).map_err(internal)? {
        return Err(error(
            StatusCode::FORBIDDEN,
            "insufficient_role",
            "Only organization owners/admins can manage credits.",
        ));
    }
    Ok(org)
}

fn entry_json(entry: &CreditLedgerEntry) -> Value {
    json!({
        "id": entry.id,
        "organization_id": entry.organization_id,
        "transaction_type": entry.transaction_type.as_str(),
        "amount_cents": entry.amount_cents,
        "balance_cents_after": entry.balance_cents_after,
        "description": entry.description,
        "reference_type": entry.reference_type,
        "reference_id": entry.reference_id,
        "expires_at": entry.expires_at.map(|d| d.to_rfc3339()),
        "created_at": entry.created_at.to_rfc3339(),
    })
}

async fn get_balance(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let org = require_org(&user)?;
    let db = state.db.clone();
    let org_for_balance = org.clone();
    let (balance, available, held) = tokio::task::spawn_blocking(move || {
        let ledger = CreditsLedger::new(db);
        let balance = ledger.balance_cents(&org_for_balance)?;
        let available = ledger.available_cents(&org_for_balance)?;
        let held = ledger.held_cents(&org_for_balance)?;
        Ok::<_, CreditsError>((balance, available, held))
    })
    .await
    .map_err(internal)?
    .map_err(credits_error)?;

    Ok(Json(json!({
        "organization_id": org,
        "balance_cents": balance,
        "available_cents": available,
        "held_cents": held,
        "currency": "USD",
    })))
}

#[derive(Debug, Deserialize)]
struct ListTransactionsQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

async fn list_transactions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<ListTransactionsQuery>,
) -> Result<Json<Value>, ApiError> {
    let org = require_org(&user)?;
    let limit = query.limit;
    let db = state.db.clone();
    let org_for_list = org.clone();
    let entries = tokio::task::spawn_blocking(move || {
        let ledger = CreditsLedger::new(db);
        ledger.list(&org_for_list, limit)
    })
    .await
    .map_err(internal)?
    .map_err(credits_error)?;

    Ok(Json(json!({
        "organization_id": org,
        "transactions": entries.iter().map(entry_json).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
struct PurchaseRequest {
    /// Accepted for wire compatibility; the gated endpoint never actiones a
    /// purchase itself, so the amount is not credited from this request.
    amount_cents: i64,
    /// Accepted for wire compatibility; not actioned (see `amount_cents`).
    method: String,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    reference_id: Option<String>,
}

enum PurchaseMode {
    /// Cloud billing is configured: purchases go through the Stripe-backed
    /// platform checkout, and this route only ever delegates to it (returns
    /// the checkout URL) — it must never mint unbacked balance.
    Hosted { platform_url: String },
    /// Self-hosted: no payment provider is wired, so the purchase endpoint
    /// has no backend to delegate to and stays behind the 409 honesty gate.
    /// Operators settle manually with an org admin grant instead.
    SelfHosted,
}

fn purchase_mode(config: &crate::config::AppConfig) -> PurchaseMode {
    if config.cloud_api_url().is_some() {
        let platform_url = std::env::var("ALLTERNIT_PLATFORM_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "https://platform.allternit.com".to_string());
        PurchaseMode::Hosted { platform_url }
    } else {
        PurchaseMode::SelfHosted
    }
}

/// The 409 every not-fully-wired purchase attempt gets. Money rule: an
/// endpoint that cannot complete a real payment must say so, never simulate
/// success. The message names the supported path so a misconfigured
/// deployment is fixable from the error alone.
const PURCHASE_NOT_ENABLED: &str =
    "credits purchase is not enabled in this deployment; use the billing checkout";

async fn purchase_credits(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<PurchaseRequest>,
) -> Result<Json<Value>, ApiError> {
    // Honesty gate (G15): until the Stripe-backed checkout is explicitly
    // enabled for this deployment, refuse rather than self-credit. There is
    // no payment proof attached to this request — storing a "stripe" or
    // "crypto" purchase row from it would be minting unbacked balance.
    if !state.config.credits_checkout_enabled() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            PURCHASE_NOT_ENABLED,
            "Set ALLTERNIT_CREDITS_CHECKOUT_ENABLED=true only where the platform billing checkout is deployed.",
        ));
    }

    let PurchaseMode::Hosted { platform_url } = purchase_mode(&state.config) else {
        // Flag set but no checkout backend configured — still not wired.
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            PURCHASE_NOT_ENABLED,
            "No cloud API / billing checkout backend is configured on this deployment.",
        ));
    };

    tracing::debug!(
        user_id = %user.user_id,
        amount_cents = req.amount_cents,
        method = %req.method,
        "credits purchase delegated to the platform billing checkout"
    );

    // Delegation, never self-crediting: the client completes the purchase in
    // the hosted checkout; credits land in the org ledger through the
    // cloud-api payment webhook → internal grant. No balance moves here.
    Ok(Json(json!({
        "checkout_url": format!("{platform_url}/billing"),
        "message": "Complete the credit pack purchase at the billing checkout. Credits are granted by the payment webhook after checkout completes.",
    })))
}

#[derive(Debug, Deserialize)]
struct GrantRequest {
    amount_cents: i64,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
    /// Internal (service-token) calls only: the organization to credit. The
    /// synthesized internal AuthUser has no organization of its own, so the
    /// cloud-api Stripe webhook names the target org explicitly.
    #[serde(default)]
    organization_id: Option<String>,
    /// Internal (service-token) calls only: upstream reference (the Stripe
    /// event id) recorded as the ledger reference_id.
    #[serde(default)]
    reference_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TransferFromWalletRequest {
    amount_cents: i64,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Move credits from the caller's Stripe-backed user wallet (cloud-api) into
/// this organization's fabric credits ledger.
///
/// Orchestration: debit the wallet, credit the local ledger with the same
/// idempotency key; if the local credit fails after a successful debit,
/// refund the wallet (same key) so money is neither lost nor duplicated.
async fn transfer_from_wallet(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<TransferFromWalletRequest>,
) -> Result<Json<Value>, ApiError> {
    let org = require_org(&user)?;
    let client = crate::wallet::ReqwestWalletClient::from_env(&state.config).ok_or_else(|| {
        error(
            StatusCode::CONFLICT,
            "wallet_transfers_unavailable",
            "Wallet transfers require hosted billing (no cloud API configured). Use an admin grant instead.",
        )
    })?;

    let idempotency_key = req
        .idempotency_key
        .clone()
        .filter(|key| !key.is_empty())
        .unwrap_or_else(|| format!("wallet-transfer-{}", uuid::Uuid::new_v4()));

    let result = transfer_from_wallet_inner(
        state.db.clone(),
        client,
        user.user_id.clone(),
        org,
        req.amount_cents,
        idempotency_key,
    )
    .await?;

    Ok(Json(result))
}

async fn transfer_from_wallet_inner(
    db: crate::db::DbHandle,
    client: impl crate::wallet::WalletClient,
    user_id: String,
    org: String,
    amount_cents: i64,
    idempotency_key: String,
) -> Result<Value, ApiError> {
    use crate::wallet::{TransferDirection, WalletError};

    if amount_cents <= 0 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_amount",
            "amount_cents must be positive.",
        ));
    }

    let wallet_balance = match client
        .transfer(
            &user_id,
            amount_cents,
            &idempotency_key,
            TransferDirection::Debit,
        )
        .await
    {
        Ok(result) => result.balance_usd,
        Err(WalletError::Insufficient(message)) => {
            return Err(error(
                StatusCode::PAYMENT_REQUIRED,
                "insufficient_wallet_balance",
                message,
            ));
        }
        Err(WalletError::Unauthorized(message)) => {
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "wallet_credentials_rejected",
                message,
            ));
        }
        Err(e) => {
            return Err(error(
                StatusCode::BAD_GATEWAY,
                "wallet_transfer_failed",
                e.to_string(),
            ));
        }
    };

    let key_for_credit = idempotency_key.clone();
    let org_for_credit = org.clone();
    let user_id_for_credit = user_id.clone();
    let credit = tokio::task::spawn_blocking(move || {
        let ledger = CreditsLedger::new(db);
        ledger.credit_with_idempotency(
            &org_for_credit,
            amount_cents,
            TransactionType::Grant,
            Some("transfer from user wallet"),
            Some("wallet_transfer"),
            Some(&user_id_for_credit),
            None,
            Some(&key_for_credit),
        )
    })
    .await
    .map_err(internal)?;

    match credit {
        Ok(entry) => Ok(json!({
            "organization_id": org,
            "transaction": entry_json(&entry),
            "balance_cents": entry.balance_cents_after,
            "available_cents": entry.balance_cents_after,
            "wallet_balance_usd": wallet_balance,
        })),
        Err(e) => {
            // The wallet was already debited — refund it with the same key so
            // the transfer is all-or-nothing across the two ledgers.
            let refund = client
                .transfer(
                    &user_id,
                    amount_cents,
                    &idempotency_key,
                    TransferDirection::Credit,
                )
                .await;
            match refund {
                Ok(_) => Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ledger_credit_failed_refunded",
                    format!("Wallet debit was refunded: {e}"),
                )),
                Err(refund_err) => Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ledger_credit_failed_refund_failed",
                    format!("Local credit failed ({e}) AND wallet refund failed ({refund_err}) — reconcile manually."),
                )),
            }
        }
    }
}

async fn admin_grant_credits(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    Json(req): Json<GrantRequest>,
) -> Result<Json<Value>, ApiError> {
    // Server-to-server path: cloud-api's Stripe webhook authenticates with
    // the internal service token; auth_middleware verified it and injected
    // the synthetic internal identity. That identity only ever enters
    // request extensions after the token check; re-verifying the header here
    // keeps the handler safe even if it is ever mounted without the
    // middleware in front of it.
    if user.user_id == crate::auth::INTERNAL_SERVICE_USER_ID
        && crate::internal_auth::require_internal_token(&headers, &state).is_ok()
    {
        return internal_grant_credits(state, req).await;
    }

    let db = state.db.clone();
    let user_for_admin = user.clone();
    let amount_cents = req.amount_cents;
    let description = req.description;
    let idempotency_key = req.idempotency_key;

    let (org, entry) = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(internal)?;
        let org = require_org_admin(&conn, &user_for_admin)?;
        let ledger = CreditsLedger::new(db);
        let entry = ledger
            .credit_with_idempotency(
                &org,
                amount_cents,
                TransactionType::Grant,
                description.as_deref().or(Some("admin grant")),
                Some("admin_grant"),
                None,
                None,
                idempotency_key.as_deref(),
            )
            .map_err(credits_error)?;
        Ok::<_, ApiError>((org, entry))
    })
    .await
    .map_err(internal)??;

    Ok(Json(json!({
        "organization_id": org,
        "transaction": entry_json(&entry),
        "balance_cents": entry.balance_cents_after,
        "available_cents": entry.balance_cents_after,
    })))
}

/// Stripe-webhook grant from cloud-api (G15). The org and the idempotency
/// key both come from the request body: the synthetic internal user has no
/// org, and the key (derived from the Stripe event id upstream) is what
/// makes webhook retries unable to double-credit.
async fn internal_grant_credits(
    state: Arc<AppState>,
    req: GrantRequest,
) -> Result<Json<Value>, ApiError> {
    let org = req
        .organization_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "organization_required",
                "organization_id is required for internal credit grants.",
            )
        })?;
    let idempotency_key = req
        .idempotency_key
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "idempotency_key_required",
                "idempotency_key is required for internal credit grants so Stripe retries cannot double-grant.",
            )
        })?;

    let db = state.db.clone();
    let amount_cents = req.amount_cents;
    let description = req.description;
    let reference_id = req.reference_id;
    let org_for_check = org.clone();
    let org_for_ledger = org.clone();
    let idem_for_ledger = idempotency_key.clone();

    let entry = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(internal)?;
        let org_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM organizations WHERE id = ?1)",
                rusqlite::params![&org_for_check],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if !org_exists {
            return Err(error(
                StatusCode::NOT_FOUND,
                "organization_not_found",
                format!("No organization with id {org_for_check}."),
            ));
        }
        let ledger = CreditsLedger::new(db);
        ledger
            .credit_with_idempotency(
                &org_for_ledger,
                amount_cents,
                TransactionType::Purchase,
                description
                    .as_deref()
                    .or(Some("stripe credit pack purchase (webhook)")),
                Some("stripe_checkout"),
                reference_id.as_deref(),
                None,
                Some(&idem_for_ledger),
            )
            .map_err(credits_error)
    })
    .await
    .map_err(internal)??;

    // Parity with the old self-hosted purchase flow: orgs subscribed to
    // billing events get notified when Stripe-backed credits land.
    crate::webhook_subscription_routes::deliver_registered_event(
        state.clone(),
        Some(&org),
        crate::webhook_subscription_routes::events::BILLING_CREDIT_PURCHASE,
        json!({
            "organization_id": org,
            "amount_cents": entry.amount_cents,
            "balance_cents_after": entry.balance_cents_after,
            "method": "stripe",
            "transaction_id": entry.id,
        }),
    )
    .await;

    Ok(Json(json!({
        "organization_id": org,
        "transaction": entry_json(&entry),
        "balance_cents": entry.balance_cents_after,
        "available_cents": entry.balance_cents_after,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    fn seed_org_user(conn: &rusqlite::Connection, org_id: &str, user_id: &str, role: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
            rusqlite::params![org_id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            rusqlite::params![user_id, format!("{}@test.local", user_id)],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![format!("{}:{}", org_id, user_id), org_id, user_id, role],
        )
        .unwrap();
    }

    fn auth_user(org_id: Option<&str>, user_id: &str) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: Some(format!("{}@test.local", user_id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::Null)
    }

    fn build_request(
        method: &str,
        uri: &str,
        user: AuthUser,
        body: Option<Value>,
    ) -> Request<Body> {
        build_request_with_headers(method, uri, user, body, &[])
    }

    fn build_request_with_headers(
        method: &str,
        uri: &str,
        user: AuthUser,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Request<Body> {
        let body = body
            .map(|b| Body::from(serde_json::to_string(&b).unwrap()))
            .unwrap_or_else(Body::empty);
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body)
            .unwrap();
        for (name, value) in headers {
            req.headers_mut().append(
                axum::http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
        }
        req.extensions_mut().insert(user);
        req
    }

    const INTERNAL_TOKEN: &str = "test-internal-token";

    fn internal_service_config() -> crate::config::AppConfig {
        crate::config::AppConfig {
            company: crate::config::CompanyConfig {
                internal_service_token: Some(INTERNAL_TOKEN.to_string()),
                ..Default::default()
            },
            user: crate::config::UserConfig::default(),
        }
    }

    fn internal_headers() -> Vec<(&'static str, &'static str)> {
        vec![("x-allternit-internal-token", INTERNAL_TOKEN)]
    }

    #[tokio::test]
    async fn purchase_refuses_409_when_checkout_flag_unset() {
        let _cloud_url = crate::test_helpers::cloud_url_env(None);
        let temp = tempfile::tempdir().unwrap().keep();
        // Default config: no ALLTERNIT_CREDITS_CHECKOUT_ENABLED → the gate is
        // closed and the endpoint must not self-credit.
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/credits/purchase",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({
                    "amount_cents": 5000,
                    "method": "stripe",
                    "reference_id": "pi_test_123",
                    "idempotency_key": "key-1"
                })),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp.into_body()).await;
        assert_eq!(
            body["error"],
            "credits purchase is not enabled in this deployment; use the billing checkout"
        );
        let ledger = CreditsLedger::new(state.db.clone());
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 0, "no self-crediting");
    }

    #[tokio::test]
    async fn purchase_delegates_to_checkout_url_when_flag_set_and_hosted() {
        let _cloud_url = crate::test_helpers::cloud_url_env(None);
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_config(
            &temp,
            crate::config::AppConfig {
                company: crate::config::CompanyConfig {
                    cloud_api_url: Some("https://api.allternit.com".to_string()),
                    credits_checkout_enabled: Some(true),
                    ..Default::default()
                },
                user: crate::config::UserConfig::default(),
            },
        )
        .await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/credits/purchase",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"amount_cents": 5000, "method": "stripe"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert!(
            body["checkout_url"].as_str().unwrap().ends_with("/billing"),
            "delegation returns the billing checkout URL, got {:?}",
            body["checkout_url"]
        );
        let ledger = CreditsLedger::new(state.db.clone());
        assert_eq!(
            ledger.balance_cents("org-1").unwrap(),
            0,
            "delegation must never credit the ledger"
        );
    }

    #[tokio::test]
    async fn purchase_refuses_409_when_flag_set_but_no_checkout_backend() {
        let _cloud_url = crate::test_helpers::cloud_url_env(None);
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_config(
            &temp,
            crate::config::AppConfig {
                company: crate::config::CompanyConfig {
                    credits_checkout_enabled: Some(true),
                    ..Default::default()
                },
                user: crate::config::UserConfig::default(),
            },
        )
        .await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/credits/purchase",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"amount_cents": 5000, "method": "stripe"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp.into_body()).await;
        assert_eq!(
            body["error"],
            "credits purchase is not enabled in this deployment; use the billing checkout"
        );
    }

    #[test]
    fn purchase_mode_self_hosted_by_default() {
        let _cloud_url = crate::test_helpers::cloud_url_env(None);
        let config = crate::config::AppConfig {
            company: crate::config::CompanyConfig::default(),
            user: crate::config::UserConfig::default(),
        };
        assert!(matches!(purchase_mode(&config), PurchaseMode::SelfHosted));
    }

    #[test]
    fn purchase_mode_hosted_when_cloud_billing_configured() {
        let _cloud_url = crate::test_helpers::cloud_url_env(None);
        let config = crate::config::AppConfig {
            company: crate::config::CompanyConfig {
                cloud_api_url: Some("https://api.allternit.com".to_string()),
                ..Default::default()
            },
            user: crate::config::UserConfig::default(),
        };
        match purchase_mode(&config) {
            PurchaseMode::Hosted { platform_url } => {
                assert!(platform_url.ends_with("allternit.com"));
            }
            PurchaseMode::SelfHosted => panic!("expected hosted mode"),
        }
    }

    #[tokio::test]
    async fn internal_grant_credits_org_with_reference_metadata() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state =
            crate::test_helpers::app_state_with_config(&temp, internal_service_config()).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request_with_headers(
                "POST",
                "/admin/credits/grant",
                auth_user(None, crate::auth::INTERNAL_SERVICE_USER_ID),
                Some(json!({
                    "amount_cents": 2500,
                    "organization_id": "org-1",
                    "idempotency_key": "stripe-evt_1",
                    "reference_id": "evt_1"
                })),
                &internal_headers(),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["organization_id"], "org-1");
        assert_eq!(body["balance_cents"], 2500);
        assert_eq!(body["transaction"]["transaction_type"], "purchase");
        assert_eq!(body["transaction"]["reference_type"], "stripe_checkout");
        assert_eq!(body["transaction"]["reference_id"], "evt_1");

        let ledger = CreditsLedger::new(state.db.clone());
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 2500);
    }

    #[tokio::test]
    async fn internal_grant_replay_same_idempotency_key_does_not_double_credit() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state =
            crate::test_helpers::app_state_with_config(&temp, internal_service_config()).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let body1 = json!({
            "amount_cents": 2500,
            "organization_id": "org-1",
            "idempotency_key": "stripe-evt_1",
            "reference_id": "evt_1"
        });
        let resp1 = app
            .clone()
            .oneshot(build_request_with_headers(
                "POST",
                "/admin/credits/grant",
                auth_user(None, crate::auth::INTERNAL_SERVICE_USER_ID),
                Some(body1),
                &internal_headers(),
            ))
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        let first = body_json(resp1.into_body()).await;

        // Stripe retries the same webhook delivery: same idempotency key, a
        // different amount must be ignored — the first result is replayed.
        let resp2 = app
            .oneshot(build_request_with_headers(
                "POST",
                "/admin/credits/grant",
                auth_user(None, crate::auth::INTERNAL_SERVICE_USER_ID),
                Some(json!({
                    "amount_cents": 9999,
                    "organization_id": "org-1",
                    "idempotency_key": "stripe-evt_1",
                    "reference_id": "evt_1"
                })),
                &internal_headers(),
            ))
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let second = body_json(resp2.into_body()).await;

        assert_eq!(first["transaction"]["id"], second["transaction"]["id"]);
        assert_eq!(second["balance_cents"], 2500);

        let ledger = CreditsLedger::new(state.db.clone());
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 2500);
        assert_eq!(ledger.list("org-1", 10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn internal_grant_requires_org_and_idempotency_key_and_known_org() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state =
            crate::test_helpers::app_state_with_config(&temp, internal_service_config()).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());

        // Missing organization_id.
        let resp = app
            .clone()
            .oneshot(build_request_with_headers(
                "POST",
                "/admin/credits/grant",
                auth_user(None, crate::auth::INTERNAL_SERVICE_USER_ID),
                Some(json!({"amount_cents": 1000, "idempotency_key": "stripe-evt_2"})),
                &internal_headers(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Missing idempotency key (a webhook retry could then double-grant).
        let resp = app
            .clone()
            .oneshot(build_request_with_headers(
                "POST",
                "/admin/credits/grant",
                auth_user(None, crate::auth::INTERNAL_SERVICE_USER_ID),
                Some(json!({"amount_cents": 1000, "organization_id": "org-1"})),
                &internal_headers(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Unknown organization.
        let resp = app
            .oneshot(build_request_with_headers(
                "POST",
                "/admin/credits/grant",
                auth_user(None, crate::auth::INTERNAL_SERVICE_USER_ID),
                Some(json!({
                    "amount_cents": 1000,
                    "organization_id": "org-missing",
                    "idempotency_key": "stripe-evt_3"
                })),
                &internal_headers(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let ledger = CreditsLedger::new(state.db.clone());
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 0);
    }

    #[tokio::test]
    async fn internal_grant_path_is_not_reachable_without_the_token() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state =
            crate::test_helpers::app_state_with_config(&temp, internal_service_config()).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        // The synthetic internal identity alone (hand-crafted extension, as a
        // spoofed request would present) must not pass: the handler
        // re-verifies the shared-secret header.
        let resp = app
            .oneshot(build_request(
                "POST",
                "/admin/credits/grant",
                auth_user(None, crate::auth::INTERNAL_SERVICE_USER_ID),
                Some(json!({
                    "amount_cents": 1000,
                    "organization_id": "org-1",
                    "idempotency_key": "stripe-evt_4"
                })),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let ledger = CreditsLedger::new(state.db.clone());
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 0);
    }

    #[tokio::test]
    async fn admin_grant_requires_admin_role() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        seed_org_user(&conn, "org-1", "member-1", "member");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/admin/credits/grant",
                auth_user(Some("org-1"), "member-1"),
                Some(json!({"amount_cents": 1000})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_grant_increases_balance() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/admin/credits/grant",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({
                    "amount_cents": 2500,
                    "description": "beta credit grant",
                    "idempotency_key": "grant-1"
                })),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["balance_cents"], 2500);
        assert_eq!(body["transaction"]["transaction_type"], "grant");

        let ledger = CreditsLedger::new(state.db.clone());
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 2500);
    }

    #[tokio::test]
    async fn balance_and_transactions_are_returned() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 3000, TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .clone()
            .oneshot(build_request(
                "GET",
                "/credits/balance",
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["balance_cents"], 3000);
        assert_eq!(body["available_cents"], 3000);
        assert_eq!(body["currency"], "USD");

        let resp = app
            .oneshot(build_request(
                "GET",
                "/credits/transactions?limit=10",
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let txs = body["transactions"].as_array().unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0]["amount_cents"], 3000);
    }

    mod transfer_from_wallet_tests {
        use super::*;
        use crate::db::DbHandle;
        use crate::wallet::{TransferDirection, WalletClient, WalletError, WalletTransferResult};
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct FakeWallet {
            balance_usd: Arc<Mutex<f64>>,
            insufficient: bool,
            calls: Arc<Mutex<Vec<(TransferDirection, i64, String)>>>,
        }

        impl WalletClient for FakeWallet {
            async fn transfer(
                &self,
                user_id: &str,
                amount_cents: i64,
                idempotency_key: &str,
                direction: TransferDirection,
            ) -> Result<WalletTransferResult, WalletError> {
                self.calls
                    .lock()
                    .unwrap()
                    .push((direction, amount_cents, idempotency_key.to_string()));
                if direction == TransferDirection::Debit && self.insufficient {
                    return Err(WalletError::Insufficient(
                        "$0.00 available".to_string(),
                    ));
                }
                let mut balance = self.balance_usd.lock().unwrap();
                let amount = amount_cents as f64 / 100.0;
                *balance += if direction == TransferDirection::Debit {
                    -amount
                } else {
                    amount
                };
                Ok(WalletTransferResult {
                    balance_usd: *balance,
                    idempotent_replay: false,
                })
            }
        }

        fn test_db() -> DbHandle {
            let db = DbHandle::new_memory().expect("memory db");
            let conn = db.connect().unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO organizations (id, name) VALUES ('org-1', 'Test Org')",
                [],
            )
            .unwrap();
            db
        }

        #[tokio::test]
        async fn transfer_debits_wallet_and_credits_org_ledger() {
            let db = test_db();
            let wallet = FakeWallet {
                balance_usd: Arc::new(Mutex::new(20.0)),
                ..Default::default()
            };
            let calls = wallet.calls.clone();

            let result = super::transfer_from_wallet_inner(
                db.clone(),
                wallet,
                "user-1".to_string(),
                "org-1".to_string(),
                1500,
                "key-1".to_string(),
            )
            .await
            .unwrap();

            assert_eq!(result["balance_cents"], 1500);
            assert_eq!(result["wallet_balance_usd"], 5.0);
            let ledger = CreditsLedger::new(db);
            assert_eq!(ledger.balance_cents("org-1").unwrap(), 1500);
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, TransferDirection::Debit);
            assert_eq!(calls[0].1, 1500);
        }

        #[tokio::test]
        async fn transfer_insufficient_wallet_credits_nothing() {
            let db = test_db();
            let wallet = FakeWallet {
                balance_usd: Arc::new(Mutex::new(5.0)),
                insufficient: true,
                ..Default::default()
            };
            let calls = wallet.calls.clone();

            let err = super::transfer_from_wallet_inner(
                db.clone(),
                wallet,
                "user-1".to_string(),
                "org-1".to_string(),
                1500,
                "key-1".to_string(),
            )
            .await
            .unwrap_err();

            assert_eq!(err.status, StatusCode::PAYMENT_REQUIRED);
            let ledger = CreditsLedger::new(db);
            assert_eq!(ledger.balance_cents("org-1").unwrap(), 0);
            assert_eq!(calls.lock().unwrap().len(), 1, "no refund without a debit");
        }

        #[tokio::test]
        async fn transfer_ledger_failure_refunds_wallet() {
            let db = test_db();
            let wallet = FakeWallet {
                balance_usd: Arc::new(Mutex::new(20.0)),
                ..Default::default()
            };
            let calls = wallet.calls.clone();

            // The org row does not exist, so the ledger credit fails on its
            // foreign key AFTER the wallet debit succeeds — the compensation
            // path must refund the wallet.
            let err = super::transfer_from_wallet_inner(
                db,
                wallet,
                "user-1".to_string(),
                "org-missing".to_string(),
                1500,
                "key-1".to_string(),
            )
            .await
            .unwrap_err();

            assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "debit then refund");
            assert_eq!(calls[0].0, TransferDirection::Debit);
            assert_eq!(calls[1].0, TransferDirection::Credit);
            assert_eq!(calls[1].2, "key-1", "refund reuses the idempotency key");
        }
    }
}
