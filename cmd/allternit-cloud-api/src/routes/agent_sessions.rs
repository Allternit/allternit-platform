//! Control-plane exposure of the agent-sessions namespace (P1, tranche 1).
//!
//! The agent-session handlers live only on the user's data-plane node
//! (`agent_session_router` in allternit-api :8013, backed by the node's gizzi
//! runtime), so these handlers implement the universal P1 design from
//! docs/architecture/2026-09-04-p1-route-inventory.md §3:
//!
//! 1. **Auth** — `auth::resolve_user_scoped(.., "compute")`, the same
//!    Clerk-first resolver the runtime relay uses.
//! 2. **Node resolution** — the caller's default data-plane node via
//!    [`DataPlaneGateway::resolve_default_node`] (routes::data_plane seam);
//!    428 "pair a device" when the account has no healthy node.
//! 3. **Relay** — the request is forwarded through the EXISTING outbound-WS
//!    relay (`runtime_relay::relay_request_to_runtime`): same allow-list,
//!    same wake-on-demand, same `Body::from_stream` chunked responses. SSE
//!    (`/sync`) is not buffered anywhere — the relay streams head +
//!    chunks verbatim, and `text/event-stream` passes the response header
//!    filter.
//! 4. **Cache nothing, transform nothing** — v1 is a faithful proxy.
//!
//! Routes are self-authenticating (mounted in the public runtime router,
//! like the pairing/relay routes) because the legacy Tower auth middleware
//! on the protected router only accepts `allternit_*` API tokens, not Clerk
//! sessions.

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
        .route(
            "/api/v1/agent-sessions",
            get(list_sessions).post(create_session),
        )
        // Static segment; axum/matchit prefers it over `:id` below.
        .route("/api/v1/agent-sessions/sync", get(sync_sessions))
        .route(
            "/api/v1/agent-sessions/:id",
            get(get_session).patch(update_session).delete(delete_session),
        )
        .route(
            "/api/v1/agent-sessions/:id/messages",
            get(list_messages).post(send_message),
        )
        .route("/api/v1/agent-sessions/:id/abort", post(abort_session))
        // Replies the app sends back to the node's gizzi runtime: answers to
        // in-chat questions, and pane_browser / pane_artifact / pane_render results.
        .route("/api/v1/questions/:id/reply", post(reply_question))
        .route("/api/v1/questions/:id/reject", post(reject_question))
        .route("/api/v1/pane-browser/:id/reply", post(reply_pane_browser))
        .route("/api/v1/pane-artifact/:id/reply", post(reply_pane_artifact))
        .route("/api/v1/pane-render/:id/reply", post(reply_pane_render))
        .route("/api/v1/agent-sessions/:id/revert", post(revert_session))
        .route(
            "/api/v1/agent-sessions/:id/unrevert",
            post(unrevert_session),
        )
        .route(
            "/api/v1/agent-sessions/:id/compact",
            post(compact_session),
        )
        .route(
            "/api/v1/agent-sessions/:id/fetch-origin",
            post(fetch_native_origin),
        )
        .route(
            "/api/v1/agent-sessions/:id/origin",
            get(get_native_origin),
        )
        .route(
            "/api/v1/agent-sessions/:id/export-native",
            post(export_native_session),
        )
        .route(
            "/api/v1/native-sessions/harnesses",
            get(list_native_harnesses),
        )
        .route("/api/v1/native-sessions", get(list_native_sessions))
        .route(
            "/api/v1/native-sessions/pickup",
            post(pickup_native_session),
        )
        .route(
            "/api/v1/native-sessions/:harness/:id",
            get(show_native_session),
        )
}

/// Universal P1 handler core (shared seam in routes::data_plane): Clerk auth
/// → default-node resolution → relay. `path` is the exact :8013 path to
/// proxy (e.g. `/api/v1/agent-sessions/:id`), including any query string.
async fn relay_agent_sessions_request(
    state: &ApiState,
    headers: &HeaderMap,
    method: &str,
    path: String,
    body: &[u8],
) -> Result<Response, ApiError> {
    relay_data_plane_request(state, headers, method, path, body).await
}

async fn list_sessions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "GET", with_query("/api/v1/agent-sessions", &query), &[]).await
}

async fn create_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", with_query("/api/v1/agent-sessions", &query), &body).await
}

async fn get_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "GET", format!("/api/v1/agent-sessions/{id}"), &[]).await
}

async fn update_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "PATCH", format!("/api/v1/agent-sessions/{id}"), &body).await
}

async fn delete_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "DELETE", format!("/api/v1/agent-sessions/{id}"), &[]).await
}

async fn list_messages(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(
        &state,
        &headers,
        "GET",
        with_query(&format!("/api/v1/agent-sessions/{id}/messages"), &query),
        &[],
    )
    .await
}

async fn reply_question(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/questions/{id}/reply"), &body).await
}

async fn reject_question(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/questions/{id}/reject"), &body).await
}

async fn reply_pane_browser(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/pane-browser/{id}/reply"), &body).await
}

async fn reply_pane_artifact(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/pane-artifact/{id}/reply"), &body).await
}

async fn reply_pane_render(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/pane-render/{id}/reply"), &body).await
}

async fn send_message(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(
        &state,
        &headers,
        "POST",
        format!("/api/v1/agent-sessions/{id}/messages"),
        &body,
    )
    .await
}

/// SSE sync channel (permission/question events from the node's gizzi
/// runtime). The relay answers from the response head immediately and then
/// streams chunks via `Body::from_stream`, so the event channel is never
/// buffered; `text/event-stream` survives `filtered_response_headers`.
/// The 90s `RELAY_TIMEOUT` bounds only the head wait, not the stream.
async fn sync_sessions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(
        &state,
        &headers,
        "GET",
        with_query("/api/v1/agent-sessions/sync", &query),
        &[],
    )
    .await
}

async fn abort_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/agent-sessions/{id}/abort"), &[]).await
}

async fn revert_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/agent-sessions/{id}/revert"), &[]).await
}

async fn unrevert_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/agent-sessions/{id}/unrevert"), &[]).await
}

async fn compact_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", format!("/api/v1/agent-sessions/{id}/compact"), &[]).await
}

async fn fetch_native_origin(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(
        &state,
        &headers,
        "POST",
        format!("/api/v1/agent-sessions/{id}/fetch-origin"),
        &[],
    )
    .await
}

async fn export_native_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(
        &state,
        &headers,
        "POST",
        format!("/api/v1/agent-sessions/{id}/export-native"),
        &body,
    )
    .await
}

async fn get_native_origin(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(
        &state,
        &headers,
        "GET",
        format!("/api/v1/agent-sessions/{id}/origin"),
        &[],
    )
    .await
}

async fn list_native_harnesses(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "GET", "/api/v1/native-sessions/harnesses".into(), &[]).await
}

async fn list_native_sessions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "GET", with_query("/api/v1/native-sessions", &query), &[]).await
}

async fn pickup_native_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(&state, &headers, "POST", "/api/v1/native-sessions/pickup".into(), &body).await
}

async fn show_native_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((harness, id)): Path<(String, String)>,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_agent_sessions_request(
        &state,
        &headers,
        "GET",
        with_query(&format!("/api/v1/native-sessions/{harness}/{id}"), &query),
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
            ("GET", "/api/v1/agent-sessions"),
            ("POST", "/api/v1/agent-sessions"),
            ("GET", "/api/v1/agent-sessions/sess_1"),
            ("PATCH", "/api/v1/agent-sessions/sess_1"),
            ("DELETE", "/api/v1/agent-sessions/sess_1"),
            ("GET", "/api/v1/agent-sessions/sess_1/messages"),
            ("POST", "/api/v1/agent-sessions/sess_1/messages"),
            ("GET", "/api/v1/agent-sessions/sync"),
            ("POST", "/api/v1/agent-sessions/sess_1/abort"),
            ("POST", "/api/v1/agent-sessions/sess_1/revert"),
            ("POST", "/api/v1/agent-sessions/sess_1/unrevert"),
            ("POST", "/api/v1/agent-sessions/sess_1/compact"),
            ("POST", "/api/v1/agent-sessions/sess_1/fetch-origin"),
            ("GET", "/api/v1/agent-sessions/sess_1/origin"),
            ("GET", "/api/v1/native-sessions"),
            ("GET", "/api/v1/native-sessions/harnesses"),
            ("POST", "/api/v1/native-sessions/pickup"),
            ("GET", "/api/v1/native-sessions/claude/abc"),
            ("POST", "/api/v1/agent-sessions/sess_1/export-native"),
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
            .uri("/api/v1/agent-sessions")
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
            .oneshot(authed_request("GET", "/api/v1/agent-sessions", ""))
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

        // One shared gateway across calls so `recorded` accumulates.
        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![
                MockGateway::json(StatusCode::OK, "[]"),
                MockGateway::json(StatusCode::OK, "{}"),
                MockGateway::json(StatusCode::OK, "{}"),
            ],
        ));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        let cases: Vec<(&str, &str, &str, &str)> = vec![
            ("GET", "/api/v1/agent-sessions?originSurface=chat", "/api/v1/agent-sessions?originSurface=chat", ""),
            ("PATCH", "/api/v1/agent-sessions/sess_1", "/api/v1/agent-sessions/sess_1", r#"{"name":"x"}"#),
            ("DELETE", "/api/v1/agent-sessions/sess_1", "/api/v1/agent-sessions/sess_1", ""),
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
        assert_eq!(recorded.len(), 3);
        for (call, (method, _, expected_path, body)) in recorded.iter().zip(cases.iter()) {
            assert_eq!(call.user_id, DEV_USER);
            assert_eq!(call.device_id, "rt_default");
            assert_eq!(call.method, *method);
            assert_eq!(call.path, *expected_path);
            let decoded = String::from_utf8(STANDARD.decode(&call.body).unwrap()).unwrap();
            assert_eq!(decoded, *body);
        }

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }    #[tokio::test]
    async fn sync_streams_the_relayed_sse_channel_verbatim() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let frames = "data: {\"type\":\"snapshot\"}\n\ndata: {\"type\":\"permission\"}\n\n";
        let sse_response = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from_stream(tokio_stream::iter(
                frames
                    .as_bytes()
                    .chunks(7)
                    .map(|chunk| Ok::<_, std::convert::Infallible>(Bytes::copy_from_slice(chunk)))
                    .collect::<Vec<_>>(),
            )))
            .unwrap();
        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![sse_response],
        ));
        let state = test_state(gateway).await;
        let router = routes().with_state(state);

        let response = router
            .oneshot(authed_request("GET", "/api/v1/agent-sessions/sync?since=42", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"], "text/event-stream",
            "SSE content type must pass through so EventSource works"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            String::from_utf8(bytes.to_vec()).unwrap(),
            frames,
            "every chunk must stream through unbuffered"
        );

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn lifecycle_and_message_routes_wire_to_the_matching_8013_paths() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            (0..6).map(|_| MockGateway::json(StatusCode::OK, "{}")).collect(),
        ));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        for (uri, expected_path) in [
            ("/api/v1/agent-sessions/sess_9/messages?limit=5", "/api/v1/agent-sessions/sess_9/messages?limit=5"),
            ("/api/v1/agent-sessions/sess_9/messages", "/api/v1/agent-sessions/sess_9/messages"),
            ("/api/v1/agent-sessions/sess_9/abort", "/api/v1/agent-sessions/sess_9/abort"),
            ("/api/v1/agent-sessions/sess_9/revert", "/api/v1/agent-sessions/sess_9/revert"),
            ("/api/v1/agent-sessions/sess_9/unrevert", "/api/v1/agent-sessions/sess_9/unrevert"),
            ("/api/v1/agent-sessions/sess_9/compact", "/api/v1/agent-sessions/sess_9/compact"),
        ] {
            let method = if uri.contains("messages?") { "GET" } else { "POST" };
            let response = router
                .clone()
                .oneshot(authed_request(method, uri, r#"{"text":"hi"}"#))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
            let recorded = gateway.recorded();
            let call = recorded.last().unwrap();
            assert_eq!(call.method, method);
            assert_eq!(call.path, expected_path, "{uri}");
        }

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
