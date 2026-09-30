//! MCP App commerce HTTP surface (P5, Stripe test mode only). All logic lives
//! in [`crate::commerce`]; these handlers only map requests/errors.
//!
//! Protected (merged into `/api/v1`):
//! - `GET  /commerce/config`                       publishable key for the host sheet
//! - `POST /commerce/connect/accounts`             Express account + onboarding link
//! - `POST /commerce/checkout/sessions`            validate + store an app's session
//! - `POST /commerce/checkout/sessions/:id/pay`    charge the STORED total (after approval)
//! - `POST /commerce/checkout/sessions/:id/complete` finish 3DS / retry fulfilment
//! - `GET  /commerce/orders/:id`                   buyer's order
//! - `POST /commerce/orders/:id/refund`            host-initiated full refund (operators)
//!
//! Public (Stripe signs it; no Clerk session): `POST /webhooks/stripe-commerce`.

use axum::{
    body::Bytes,
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::commerce::{
    CheckoutSessionInput, CommerceConfig, CommerceError, CommerceService, DispatcherAppCaller, StripeHttp,
};
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/commerce/config", get(get_config))
        .route("/commerce/connect/accounts", post(connect_account))
        .route("/commerce/checkout/sessions", post(open_session))
        .route("/commerce/checkout/sessions/:id/pay", post(pay))
        .route("/commerce/checkout/sessions/:id/complete", post(complete))
        .route("/commerce/orders/:id", get(get_order))
        .route("/commerce/orders/:id/refund", post(refund))
}

pub fn webhook_router() -> Router<Arc<AppState>> {
    Router::new().route("/webhooks/stripe-commerce", post(stripe_webhook))
}

/// Startup guard: `Err` (a live key is configured) means the API must not
/// start. `Ok(false)` = commerce is simply off.
pub fn check_startup_config() -> Result<bool, CommerceError> {
    CommerceConfig::from_env().map(|c| c.is_some())
}

fn service(state: &AppState) -> Result<CommerceService, CommerceError> {
    let cfg = CommerceConfig::from_env()?.ok_or(CommerceError::NotConfigured)?;
    let stripe = Arc::new(StripeHttp::new(&cfg));
    let apps = Arc::new(DispatcherAppCaller(state.mcp_dispatcher.clone()));
    Ok(CommerceService::new(cfg, state.db.clone(), stripe, apps))
}

pub fn status_for(e: &CommerceError) -> StatusCode {
    match e {
        CommerceError::NotConfigured => StatusCode::SERVICE_UNAVAILABLE,
        CommerceError::LiveKeyRefused(_) => StatusCode::SERVICE_UNAVAILABLE,
        CommerceError::Invalid(_) => StatusCode::BAD_REQUEST,
        CommerceError::Mismatch(_) => StatusCode::UNPROCESSABLE_ENTITY,
        CommerceError::NotFound(_) => StatusCode::NOT_FOUND,
        CommerceError::Forbidden(_) => StatusCode::FORBIDDEN,
        CommerceError::Conflict(_) => StatusCode::CONFLICT,
        CommerceError::Stripe(_) => StatusCode::BAD_GATEWAY,
        CommerceError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn err(e: CommerceError) -> Response {
    let status = status_for(&e);
    if status.is_server_error() {
        tracing::warn!(error = %e, "commerce request failed");
    }
    // Never echo a key: LiveKeyRefused text names the env var only.
    (status, Json(json!({ "error": format!("{e}"), "kind": kind(&e) }))).into_response()
}

fn kind(e: &CommerceError) -> &'static str {
    match e {
        CommerceError::LiveKeyRefused(_) => "live_key_refused",
        CommerceError::NotConfigured => "not_configured",
        CommerceError::Invalid(_) => "invalid",
        CommerceError::Mismatch(_) => "mismatch",
        CommerceError::NotFound(_) => "not_found",
        CommerceError::Forbidden(_) => "forbidden",
        CommerceError::Conflict(_) => "conflict",
        CommerceError::Stripe(_) => "stripe",
        CommerceError::Db(_) => "internal",
    }
}

async fn get_config() -> Response {
    match CommerceConfig::from_env() {
        Ok(Some(c)) => Json(json!({
            "enabled": true,
            "test_mode": true,
            "publishable_key": c.publishable_key,
        }))
        .into_response(),
        Ok(None) => Json(json!({ "enabled": false, "test_mode": true })).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct ConnectBody {
    app_id: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    country: Option<String>,
    refresh_url: String,
    return_url: String,
}

async fn connect_account(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(b): Json<ConnectBody>,
) -> Response {
    let svc = match service(&state) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    match svc
        .register_account(&user.user_id, &b.app_id, b.email.as_deref(), b.country.as_deref(), &b.refresh_url, &b.return_url)
        .await
    {
        Ok(r) => (StatusCode::CREATED, Json(r)).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct OpenSessionBody {
    app_id: String,
    session: CheckoutSessionInput,
}

async fn open_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(b): Json<OpenSessionBody>,
) -> Response {
    let svc = match service(&state) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    match svc.open_session(&user.user_id, &b.app_id, &b.session).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct PayBody {
    /// Stripe `pm_…` created by Elements in the host sheet. No amount field:
    /// the charge is whatever the host stored for this session.
    payment_method: String,
}

async fn pay(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(b): Json<PayBody>,
) -> Response {
    let svc = match service(&state) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    match svc.pay(&user.user_id, &id, &b.payment_method).await {
        Ok(o) => Json(o).into_response(),
        Err(e) => err(e),
    }
}

async fn complete(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let svc = match service(&state) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    match svc.complete(&user.user_id, &id).await {
        Ok(o) => Json(o).into_response(),
        Err(e) => err(e),
    }
}

async fn get_order(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let svc = match service(&state) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    match svc.get_order(&user.user_id, &id) {
        Ok(o) => Json(o).into_response(),
        Err(e) => err(e),
    }
}

async fn refund(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let svc = match service(&state) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    match svc.refund_order(&user.user_id, &id).await {
        Ok(o) => Json(o).into_response(),
        Err(e) => err(e),
    }
}

async fn stripe_webhook(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let svc = match service(&state) {
        Ok(s) => s,
        Err(e) => return err(e),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let sig = headers.get("stripe-signature").and_then(|v| v.to_str().ok());
    match svc.handle_webhook(sig, &body, now) {
        Ok(o) => Json(o).into_response(),
        // Bad signature must be a 4xx so Stripe does not treat it as delivered.
        Err(CommerceError::Forbidden(m)) => (StatusCode::UNAUTHORIZED, Json(json!({ "error": m }))).into_response(),
        Err(e) => err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_mapping() {
        assert_eq!(status_for(&CommerceError::Mismatch("x".into())), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(status_for(&CommerceError::NotConfigured), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(status_for(&CommerceError::Forbidden("x".into())), StatusCode::FORBIDDEN);
    }

    #[test]
    fn startup_refuses_live_key() {
        // from_lookup is what from_env delegates to; env is process-global, so
        // exercise the same code path without mutating it.
        let r = CommerceConfig::from_lookup(|k| {
            (k == "ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY").then(|| "sk_live_x".to_string())
        });
        assert!(matches!(r, Err(CommerceError::LiveKeyRefused(_))));
    }
}
