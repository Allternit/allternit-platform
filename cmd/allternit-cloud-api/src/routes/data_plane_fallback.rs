//! Catch-all data-plane relay (ADR 2026-09-03 D1/D2): any `/api/v1/*` route
//! the control plane doesn't serve itself goes to the caller's default
//! data-plane node (their Desktop app, paired box or provisioned instance)
//! through the same path every P1 namespace uses: Clerk auth → default-node
//! resolution → runtime relay, faithful proxy, nothing cached.
//!
//! Eoj 2026-09-30: ai.allternit.com must work like the Desktop app. Before
//! this, routes that live only on the runtime (subscriptions, the agent
//! gateway, computers, …) 404'd on the web because each namespace had to be
//! wired by hand. Named P1 handlers still take precedence (they're routes;
//! this is the router's fallback), and control-plane names never fall
//! through: a mistyped billing or auth route stays a 404 here instead of
//! reaching a user's machine.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::data_plane::relay_data_plane_request;
use crate::ApiState;

/// Namespaces the control plane owns. A path under one of these that no
/// route matched is a 404, never a relay.
const CONTROL_PLANE_PREFIXES: &[&str] = &[
    "/api/v1/auth",
    "/api/v1/admin",
    "/api/v1/billing",
    "/api/v1/api-keys",
    "/api/v1/inference-keys",
    "/api/v1/me",
    "/api/v1/health",
    "/api/v1/metrics",
    "/api/v1/runtime-",
    "/api/v1/hosted",
    "/api/v1/contabo",
    "/api/v1/provisioned-instances",
    "/api/v1/mesh",
    "/api/v1/wizard",
    "/api/v1/webhooks",
    "/api/v1/dispatch",
    "/api/v1/gizzi-instances",
];

pub(crate) fn relays(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/v1/") else { return false };
    if rest.is_empty() || path.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    !CONTROL_PLANE_PREFIXES.iter().any(|p| {
        path == *p || path.strip_prefix(p).is_some_and(|s| s.is_empty() || s.starts_with('/') || p.ends_with('-'))
    })
}

pub async fn fallback(
    State(state): State<Arc<ApiState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !relays(uri.path()) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "NOT_FOUND", "message": "No such route" }))).into_response();
    }
    let path = uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| uri.path().to_string());
    match relay_data_plane_request(&state, &headers, method.as_str(), path, &body).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};
    use axum::body::Body;
    use axum::http::Request;
    use axum::Router;
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use tower::ServiceExt;

    fn router(state: Arc<ApiState>) -> Router {
        Router::new().fallback(fallback).with_state(state)
    }

    #[test]
    fn only_runtime_namespaces_relay() {
        for p in ["/api/v1/subscriptions/status", "/api/v1/gateway/provider-accounts", "/api/v1/computers", "/api/v1/subscriptions/gateway/v1/accounts"] {
            assert!(relays(p), "{p} should relay");
        }
        for p in ["/api/v1/billing/x", "/api/v1/auth/me2", "/api/v1/runtime-devices/x", "/api/v1/admin", "/api/v1/", "/api/v2/x", "/health", "/api/v1/computers/../billing"] {
            assert!(!relays(p), "{p} must not relay");
        }
    }

    #[tokio::test]
    async fn unknown_runtime_routes_relay_with_method_path_query_and_body() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![MockGateway::json(StatusCode::OK, "[]"), MockGateway::json(StatusCode::CREATED, "{}")],
        ));
        let app = router(test_state(gateway.clone()).await);
        let r = app.clone().oneshot(authed_request("GET", "/api/v1/subscriptions/gateway/v1/accounts?x=1", "")).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let r = app.oneshot(authed_request("PATCH", "/api/v1/gateway/provider-accounts/a1", r#"{"state":"CONNECTED"}"#)).await.unwrap();
        assert_eq!(r.status(), StatusCode::CREATED);
        let calls = gateway.recorded();
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].user_id.as_str(), calls[0].device_id.as_str()), (DEV_USER, "rt_default"));
        assert_eq!((calls[0].method.as_str(), calls[0].path.as_str()), ("GET", "/api/v1/subscriptions/gateway/v1/accounts?x=1"));
        assert_eq!((calls[1].method.as_str(), calls[1].path.as_str()), ("PATCH", "/api/v1/gateway/provider-accounts/a1"));
        assert_eq!(String::from_utf8(STANDARD.decode(&calls[1].body).unwrap()).unwrap(), r#"{"state":"CONNECTED"}"#);
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn control_plane_names_and_unauthenticated_calls_never_reach_a_runtime() {
        let gateway = Arc::new(MockGateway::new(Some(MockGateway::healthy_node()), vec![]));
        let app = router(test_state(gateway.clone()).await);
        let r = app.clone().oneshot(Request::builder().uri("/api/v1/billing/nope").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        let r = app.oneshot(Request::builder().uri("/api/v1/computers").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert!(gateway.recorded().is_empty());
    }

    #[tokio::test]
    async fn no_runtime_online_is_428_pair_a_device() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let gateway = Arc::new(MockGateway::failing("No data-plane node registered for this account — pair a device (or start a hosted runtime) and try again"));
        let app = router(test_state(gateway.clone()).await);
        let r = app.oneshot(authed_request("GET", "/api/v1/subscriptions/status", "")).await.unwrap();
        assert_eq!(r.status(), StatusCode::PRECONDITION_REQUIRED);
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
