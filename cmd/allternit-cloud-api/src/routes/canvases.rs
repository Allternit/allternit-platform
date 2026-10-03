//! Control-plane exposure of the canvas namespace (P1, tranche 2).
//!
//! Canvas handlers live only on the user's data-plane node
//! (`canvas_router` in allternit-api :8013, `canvas_routes.rs`, backed by
//! the node's local SQLite `agent_canvases` table). Like agent-sessions and
//! office, these handlers follow the universal P1 four-step design from
//! docs/architecture/2026-09-04-p1-route-inventory.md §3 via the shared seam
//! in routes::data_plane:
//!
//! 1. **Auth** — `auth::resolve_user_scoped(.., "compute")`.
//! 2. **Node resolution** — the caller's default data-plane node via
//!    [`DataPlaneGateway::resolve_default_node`]; 428 "pair a device" when
//!    the account has no healthy node.
//! 3. **Relay** — `runtime_relay::relay_request_to_runtime`, verbatim.
//! 4. **Cache nothing, transform nothing** — v1 is a faithful proxy.
//!
//! The data-plane canvas router owns exactly three paths (no SSE/streaming
//! variant exists — canvases are plain JSON CRUD):
//!
//! - `GET    /api/v1/canvases`                          (user-wide list)
//! - `GET/POST /api/v1/agent-sessions/:session_id/canvases` (session list/create)
//! - `GET/PATCH/DELETE /api/v1/canvases/:canvas_id`
//!
//! The web client's list/create calls already go through the agent-sessions
//! namespace, so this module exposes the `/api/v1/canvases*` equivalents the
//! client calls directly (user-wide list + per-canvas get/patch/delete).
//! Canvases are user-scoped, not node-affine like office bindings, but v1
//! still routes them to the account's default node — a canvas row lives in
//! exactly one node's SQLite and the relay is the only path to it.

use axum::{
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::Response,
    routing::get,
    Router,
};
use bytes::Bytes;
use std::sync::Arc;

use super::data_plane::{relay_data_plane_request, with_query};
use crate::{ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/canvases", get(list_canvases))
        .route(
            "/api/v1/canvases/:canvas_id",
            get(get_canvas).patch(update_canvas).delete(delete_canvas),
        )
}

async fn list_canvases(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        with_query("/api/v1/canvases", &query),
        &[],
    )
    .await
}

async fn get_canvas(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(canvas_id): Path<String>,
) -> Result<Response, ApiError> {
    relay_data_plane_request(&state, &headers, "GET", format!("/api/v1/canvases/{canvas_id}"), &[])
        .await
}

async fn update_canvas(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(canvas_id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "PATCH",
        format!("/api/v1/canvases/{canvas_id}"),
        &body,
    )
    .await
}

async fn delete_canvas(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(canvas_id): Path<String>,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "DELETE",
        format!("/api/v1/canvases/{canvas_id}"),
        &[],
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
            ("GET", "/api/v1/canvases"),
            ("GET", "/api/v1/canvases/cv_1"),
            ("PATCH", "/api/v1/canvases/cv_1"),
            ("DELETE", "/api/v1/canvases/cv_1"),
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
    async fn unknown_bearer_token_is_rejected_without_touching_the_gateway() {
        let gateway = Arc::new(MockGateway::new(Some(MockGateway::healthy_node()), vec![]));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        let request = Request::builder()
            .method("GET")
            .uri("/api/v1/canvases")
            .header("authorization", "Bearer allternit_not_a_real_token_0123456789")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            gateway.recorded().is_empty(),
            "unauthenticated requests never reach the relay"
        );
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
            .oneshot(authed_request("GET", "/api/v1/canvases", ""))
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
    async fn relays_to_the_resolved_default_node_with_exact_method_and_path() {
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
            ("GET", "/api/v1/canvases", "/api/v1/canvases", ""),
            ("GET", "/api/v1/canvases/cv_1", "/api/v1/canvases/cv_1", ""),
            (
                "PATCH",
                "/api/v1/canvases/cv_1",
                "/api/v1/canvases/cv_1",
                r#"{"title":"x"}"#,
            ),
            ("DELETE", "/api/v1/canvases/cv_1", "/api/v1/canvases/cv_1", ""),
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

    #[tokio::test]
    async fn list_passes_the_query_string_through_verbatim() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![MockGateway::json(StatusCode::OK, "[]")],
        ));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        let response = router
            .oneshot(authed_request("GET", "/api/v1/canvases?limit=5", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let recorded = gateway.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].method, "GET");
        assert_eq!(recorded[0].path, "/api/v1/canvases?limit=5");

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
