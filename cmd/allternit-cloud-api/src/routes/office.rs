//! Control-plane exposure of the office namespace (P1).
//!
//! Office handlers live only on the user's data-plane node
//! (`office_router` in allternit-api :8013, `office_routes.rs`). Per the P1
//! route inventory (docs/architecture/2026-09-04-p1-route-inventory.md §3.3)
//! these handlers are **node-affine by design**: office bindings live in
//! memory in the :8013 process (`AppState.bindings`, TTL-reaped), so every
//! request must land on the resolved default node for the caller — exactly
//! what the shared four-step core in routes::data_plane does. The client's
//! fail-closed "binding absent" semantic makes a wrong node
//! indistinguishable from "no binding", which is why office does not get a
//! `?node=` override: it always follows the account's default node.
//!
//! Cache nothing, transform nothing — v1 is a faithful proxy.

use axum::{
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use std::sync::Arc;

use super::data_plane::{relay_data_plane_request, with_query};
use crate::{ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/office/bindings", get(list_bindings))
        .route("/api/v1/office/bindings/:binding_id", get(get_binding))
        .route("/api/v1/office/bootstrap", post(office_bootstrap))
        .route("/api/v1/office/runtime/state", post(office_runtime_state))
}

async fn list_bindings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        with_query("/api/v1/office/bindings", &query),
        &[],
    )
    .await
}

async fn get_binding(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(binding_id): Path<String>,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        format!("/api/v1/office/bindings/{binding_id}"),
        &[],
    )
    .await
}

async fn office_bootstrap(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "POST",
        with_query("/api/v1/office/bootstrap", &query),
        &body,
    )
    .await
}

async fn office_runtime_state(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "POST",
        with_query("/api/v1/office/runtime/state", &query),
        &body,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn every_endpoint_requires_authentication() {
        let state = test_state(Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![],
        )))
        .await;
        let router = routes().with_state(state);

        for (method, path) in [
            ("GET", "/api/v1/office/bindings"),
            ("GET", "/api/v1/office/bindings/bind_1"),
            ("POST", "/api/v1/office/bootstrap"),
            ("POST", "/api/v1/office/runtime/state"),
        ] {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must require auth"
            );
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"], "UNAUTHORIZED", "{method} {path}");
        }
    }

    #[tokio::test]
    async fn no_healthy_node_is_a_428_pair_a_device_error() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let gateway = Arc::new(MockGateway::failing(
            "No data-plane node registered for this account — pair a device (or start a hosted runtime) and try again",
        ));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        let response = router
            .oneshot(authed_request("GET", "/api/v1/office/bindings", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["message"].as_str().unwrap().contains("pair a device"));
        assert!(gateway.recorded().is_empty());

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn relays_to_the_resolved_default_node_with_exact_method_path_and_body() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![
                MockGateway::json(StatusCode::OK, "[]"),
                MockGateway::json(StatusCode::OK, "{}"),
                MockGateway::json(StatusCode::OK, "{}"),
                MockGateway::json(StatusCode::OK, "{}"),
            ],
        ));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        let cases: Vec<(&str, &str, &str, &str)> = vec![
            ("GET", "/api/v1/office/bindings?surface=word", "/api/v1/office/bindings?surface=word", ""),
            ("GET", "/api/v1/office/bindings/bind_1", "/api/v1/office/bindings/bind_1", ""),
            ("POST", "/api/v1/office/bootstrap", "/api/v1/office/bootstrap", r#"{"docId":"d1"}"#),
            ("POST", "/api/v1/office/runtime/state", "/api/v1/office/runtime/state", r#"{"cursor":3}"#),
        ];
        for (method, uri, _, body) in cases.iter().copied() {
            let response = router
                .clone()
                .oneshot(authed_request(method, uri, body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
        }

        let recorded = gateway.recorded();
        assert_eq!(recorded.len(), 4);
        for (call, (method, _, expected_path, body)) in recorded.iter().zip(cases.iter()) {
            assert_eq!(call.user_id, DEV_USER);
            assert_eq!(call.device_id, "rt_default");
            assert_eq!(call.method, *method);
            assert_eq!(call.path, *expected_path);
            let decoded = String::from_utf8(STANDARD.decode(&call.body).unwrap()).unwrap();
            assert_eq!(decoded, *body);
        }

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
