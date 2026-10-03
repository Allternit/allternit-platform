//! Control-plane exposure of the beta namespace (P1).
//!
//! Beta handlers live only on the user's data-plane node (`beta_session_router`
//! and `research_task_router` in allternit-api :8013, backed by the node's
//! SQLite: `beta_sessions`, `beta_session_events`, `research_tasks`, ...), so
//! these handlers follow the universal P1 four-step design (see
//! routes::data_plane): Clerk auth → resolve default node → relay verbatim →
//! cache/transform nothing.
//!
//! Coverage follows the P1 route inventory
//! (docs/architecture/2026-09-04-p1-route-inventory.md §3.4), in priority
//! order: research tasks, then the playground session surface
//! (CRUD + event polling + memory search + run).
//!
//! ## Session events WebSocket
//!
//! `GET /api/v1/beta/sessions/:id/events` (SSE) is relayed like any other
//! request, but `GET /api/v1/beta/sessions/:id/events/ws` is WebSocket-only,
//! so it is exposed as a ticket pair mirroring the runtime relay's
//! browser-socket flow — a browser WebSocket cannot set an `Authorization`
//! header, so authentication happens on a Clerk-authed POST that mints a
//! 30-second single-use socket ticket:
//!
//! 1. `POST /api/v1/beta/sessions/:id/events/ws-ticket` — Clerk auth
//!    (`compute` scope), resolves the caller's default node
//!    ([`DataPlaneGateway::resolve_default_node`]), then mints the ticket via
//!    [`runtime_relay::issue_socket_ticket`] (capability check +
//!    wake-on-demand shared with the generic socket-ticket route). Optional
//!    JSON body `{"after": <cursor>}` pins the replay cursor into the
//!    ticket's node path (`/api/v1/beta/sessions/:id/events/ws?after=N`).
//! 2. `GET /api/v1/beta/sessions/:id/events/ws?ticket=…` — redeems the ticket
//!    ([`runtime_relay::upgrade_with_socket_ticket`]) and pumps frames between
//!    the browser socket and the node over the node's outbound relay
//!    connection, using the `socket_open`/`socket_ready`/`socket_data`/
//!    `socket_close` tunnel contract documented in routes::runtime_relay.
//!
//! Deliberately NOT exposed here:
//! - resources/files/context/tool-context/interrupt — 8013-owned but not in
//!   the P1 flagged-surface list.
//!
//! Note: `POST /api/v1/beta/sessions/:id/run` is relayed even though the
//! current :8013 route table does not define it (the playground client calls
//! it; the node's 404 comes back verbatim until the data-plane handler
//! lands). §3.4 #18 lists it as part of the namespace.

use axum::{
    extract::{ws::WebSocketUpgrade, Path, Query, RawQuery, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use serde::Deserialize;
use std::sync::Arc;

use super::data_plane::{relay_data_plane_request, with_query};
use super::runtime_relay;
use crate::{ApiError, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        // Research tasks (SQLite `research_tasks` on the node).
        .route(
            "/api/v1/beta/research",
            post(create_research_task).get(list_research_tasks),
        )
        // :8013 serves update as POST, not PATCH (research_task_routes.rs).
        .route(
            "/api/v1/beta/research/:id",
            get(get_research_task)
                .post(update_research_task)
                .delete(delete_research_task),
        )
        // Beta playground sessions.
        .route(
            "/api/v1/beta/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/api/v1/beta/sessions/:id",
            get(get_session)
                .patch(update_session)
                .delete(archive_session),
        )
        .route(
            "/api/v1/beta/sessions/:id/events/list",
            get(list_events_json),
        )
        .route(
            "/api/v1/beta/sessions/:id/memory/search",
            get(search_session_memory),
        )
        .route("/api/v1/beta/sessions/:id/run", post(run_session))
        // Session events WebSocket (ticket pair — see module docs).
        .route(
            "/api/v1/beta/sessions/:id/events/ws-ticket",
            post(create_events_ws_ticket),
        )
        .route(
            "/api/v1/beta/sessions/:id/events/ws",
            get(connect_events_ws),
        )
}

async fn create_research_task(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "POST",
        "/api/v1/beta/research".to_string(),
        &body,
    )
    .await
}

async fn list_research_tasks(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        with_query("/api/v1/beta/research", &query),
        &[],
    )
    .await
}

async fn get_research_task(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        format!("/api/v1/beta/research/{id}"),
        &[],
    )
    .await
}

async fn update_research_task(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "POST",
        format!("/api/v1/beta/research/{id}"),
        &body,
    )
    .await
}

async fn delete_research_task(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "DELETE",
        format!("/api/v1/beta/research/{id}"),
        &[],
    )
    .await
}

async fn list_sessions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        with_query("/api/v1/beta/sessions", &query),
        &[],
    )
    .await
}

async fn create_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "POST",
        "/api/v1/beta/sessions".to_string(),
        &body,
    )
    .await
}

async fn get_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        format!("/api/v1/beta/sessions/{id}"),
        &[],
    )
    .await
}

async fn update_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "PATCH",
        format!("/api/v1/beta/sessions/{id}"),
        &body,
    )
    .await
}

async fn archive_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "DELETE",
        format!("/api/v1/beta/sessions/{id}"),
        &[],
    )
    .await
}

async fn list_events_json(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        with_query(&format!("/api/v1/beta/sessions/{id}/events/list"), &query),
        &[],
    )
    .await
}

async fn search_session_memory(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query: RawQuery,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "GET",
        with_query(&format!("/api/v1/beta/sessions/{id}/memory/search"), &query),
        &[],
    )
    .await
}

async fn run_session(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    relay_data_plane_request(
        &state,
        &headers,
        "POST",
        format!("/api/v1/beta/sessions/{id}/run"),
        &body,
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateEventsWsTicketRequest {
    after: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct EventsWsQuery {
    ticket: Option<String>,
}

/// `POST /api/v1/beta/sessions/:id/events/ws-ticket` — Clerk-authed mint of a
/// 30-second single-use socket ticket for the node's
/// `GET /api/v1/beta/sessions/:id/events/ws` endpoint on the caller's default
/// node. See the module docs for the full flow.
async fn create_events_ws_ticket(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<CreateEventsWsTicketRequest>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let node = state
        .data_plane_gateway
        .resolve_default_node(&user.id)
        .await?;
    let after = body
        .and_then(|Json(request)| request.after)
        .filter(|after| *after >= 0);
    let path = match after {
        Some(after) => format!("/api/v1/beta/sessions/{id}/events/ws?after={after}"),
        None => format!("/api/v1/beta/sessions/{id}/events/ws"),
    };
    let ticket =
        runtime_relay::issue_socket_ticket(&state, &user.id, &node.device_id, path).await?;
    Ok(Json(ticket))
}

/// `GET /api/v1/beta/sessions/:id/events/ws?ticket=…` — redeem a socket
/// ticket and tunnel the browser WebSocket to the node's events endpoint over
/// the node's outbound relay connection.
async fn connect_events_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Query(query): Query<EventsWsQuery>,
) -> Result<Response, ApiError> {
    let Some(ticket_id) = query.ticket else {
        return Err(ApiError::Unauthorized(
            "A socket ticket is required; mint one via POST …/events/ws-ticket".to_string(),
        ));
    };
    // The ticket binds this exact session's events path (optionally with an
    // `after` query): a ticket minted for session A cannot be redeemed
    // against session B's upgrade URL.
    let expected = format!("/api/v1/beta/sessions/{id}/events/ws");
    let ticket = runtime_relay::take_socket_ticket(&ticket_id)
        .await
        .filter(|ticket| {
            ticket.path == expected || ticket.path.starts_with(&format!("{expected}?"))
        })
        .ok_or_else(|| {
            ApiError::Unauthorized("Invalid or expired session socket ticket".to_string())
        })?;
    runtime_relay::upgrade_with_socket_ticket(ws, &state, ticket).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::runtime_relay::{self, CloudMessage, RuntimeSocketFrame};
    use crate::routes::test_support::{
        authed_request, seed_runtime_device, test_state, MockGateway, DEV_USER,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use futures_util::{SinkExt, StreamExt};
    use http_body_util::BodyExt;
    use std::time::Duration;
    use tower::ServiceExt;

    fn node_with_id(device_id: &str) -> crate::services::ResolvedNode {
        crate::services::ResolvedNode {
            device_id: device_id.to_string(),
            name: "test node".to_string(),
            kind: crate::services::NodeKind("local".to_string()),
            last_seen_at: Some(chrono::Utc::now()),
        }
    }

    /// The upgrade path runs ensure_quota → check_relay_socket_allowed →
    /// open/close_relay_socket, so the schema-per-test pool needs a generous
    /// quota row for DEV_USER.
    async fn seed_quota(db: &sqlx::PgPool) {
        sqlx::query(
            r#"
            INSERT INTO user_runtime_quotas (
                user_id, plan_tier_id, max_active_devices, max_pairings_per_day,
                max_relay_sockets, max_relay_mb_per_day, max_hosted_runtime_hours_monthly,
                can_create_hosted_runtime, max_hosted_runtimes,
                max_hosted_runtime_memory_mb, hard_spend_cap_usd
            ) VALUES ($1, 'free', 100, 100, 100, 100000, 0, FALSE, 0, 0, NULL)
            "#,
        )
        .bind(DEV_USER)
        .execute(db)
        .await
        .unwrap();
    }

    /// Mint a ticket through the real HTTP route and return it.
    async fn mint_ticket(app: &axum::Router, session_id: &str, body: &str) -> String {
        let response = app
            .clone()
            .oneshot(authed_request(
                "POST",
                &format!("/api/v1/beta/sessions/{session_id}/events/ws-ticket"),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["ticket"].as_str().unwrap().to_string()
    }

    /// HTTP status of a rejected WebSocket upgrade attempt.
    async fn upgrade_status(addr: std::net::SocketAddr, path_and_query: &str) -> u16 {
        let url = format!("ws://{addr}{path_and_query}");
        let error = tokio_tungstenite::connect_async(url.as_str())
            .await
            .expect_err("upgrade must be rejected");
        match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => response.status().as_u16(),
            other => panic!("expected an HTTP error response, got {other}"),
        }
    }

    #[tokio::test]
    async fn every_endpoint_requires_authentication() {
        let state = test_state(Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            vec![],
        )))
        .await;
        let router = routes().with_state(state);

        for (method, path) in [
            ("POST", "/api/v1/beta/research"),
            ("GET", "/api/v1/beta/research"),
            ("GET", "/api/v1/beta/research/rt_1"),
            ("POST", "/api/v1/beta/research/rt_1"),
            ("DELETE", "/api/v1/beta/research/rt_1"),
            ("GET", "/api/v1/beta/sessions"),
            ("POST", "/api/v1/beta/sessions"),
            ("GET", "/api/v1/beta/sessions/sess_1"),
            ("PATCH", "/api/v1/beta/sessions/sess_1"),
            ("DELETE", "/api/v1/beta/sessions/sess_1"),
            ("GET", "/api/v1/beta/sessions/sess_1/events/list"),
            ("GET", "/api/v1/beta/sessions/sess_1/memory/search?q=hi"),
            ("POST", "/api/v1/beta/sessions/sess_1/run"),
            ("POST", "/api/v1/beta/sessions/sess_1/events/ws-ticket"),
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
        let message =
            "No data-plane node registered for this account — pair a device (or start a hosted runtime) and try again";

        let gateway = Arc::new(MockGateway::failing(message));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        let response = router
            .oneshot(authed_request(
                "POST",
                "/api/v1/beta/research",
                r#"{"query":"x"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["message"].as_str().unwrap().contains("pair a device"));
        assert!(gateway.recorded().is_empty());

        // The WS ticket endpoint resolves the default node too, so it must
        // fail the same way before any ticket is minted.
        let router = routes().with_state(test_state(Arc::new(MockGateway::failing(message))).await);
        let response = router
            .oneshot(authed_request(
                "POST",
                "/api/v1/beta/sessions/sess_1/events/ws-ticket",
                r#"{"after":2}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn relays_to_the_resolved_default_node_with_exact_method_path_and_body() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            (0..6)
                .map(|_| MockGateway::json(StatusCode::OK, "{}"))
                .collect(),
        ));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        let cases: Vec<(&str, &str, &str, &str)> = vec![
            (
                "POST",
                "/api/v1/beta/research",
                "/api/v1/beta/research",
                r#"{"query":"q"}"#,
            ),
            (
                "GET",
                "/api/v1/beta/research?status=running",
                "/api/v1/beta/research?status=running",
                "",
            ),
            (
                "POST",
                "/api/v1/beta/research/rt_1",
                "/api/v1/beta/research/rt_1",
                r#"{"status":"done"}"#,
            ),
            (
                "DELETE",
                "/api/v1/beta/research/rt_1",
                "/api/v1/beta/research/rt_1",
                "",
            ),
            (
                "POST",
                "/api/v1/beta/sessions",
                "/api/v1/beta/sessions",
                r#"{"title":"t"}"#,
            ),
            (
                "GET",
                "/api/v1/beta/sessions/sess_1/events/list?limit=10",
                "/api/v1/beta/sessions/sess_1/events/list?limit=10",
                "",
            ),
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
        assert_eq!(recorded.len(), 6);
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
    async fn session_lifecycle_routes_wire_to_the_matching_8013_paths() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let gateway = Arc::new(MockGateway::new(
            Some(MockGateway::healthy_node()),
            (0..7)
                .map(|_| MockGateway::json(StatusCode::OK, "{}"))
                .collect(),
        ));
        let state = test_state(gateway.clone()).await;
        let router = routes().with_state(state);

        for (method, uri, expected_path) in [
            (
                "GET",
                "/api/v1/beta/sessions/sess_9",
                "/api/v1/beta/sessions/sess_9",
            ),
            (
                "PATCH",
                "/api/v1/beta/sessions/sess_9",
                "/api/v1/beta/sessions/sess_9",
            ),
            (
                "DELETE",
                "/api/v1/beta/sessions/sess_9",
                "/api/v1/beta/sessions/sess_9",
            ),
            (
                "GET",
                "/api/v1/beta/sessions/sess_9/memory/search?q=foo",
                "/api/v1/beta/sessions/sess_9/memory/search?q=foo",
            ),
            (
                "GET",
                "/api/v1/beta/sessions/sess_9/events/list",
                "/api/v1/beta/sessions/sess_9/events/list",
            ),
            (
                "POST",
                "/api/v1/beta/sessions/sess_9/run",
                "/api/v1/beta/sessions/sess_9/run",
            ),
            (
                "GET",
                "/api/v1/beta/research/rt_9",
                "/api/v1/beta/research/rt_9",
            ),
        ] {
            let response = router
                .clone()
                .oneshot(authed_request(method, uri, r#"{"prompt":"hi"}"#))
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

    #[tokio::test]
    async fn events_ws_ticket_binds_the_node_beta_events_path() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let device_id = format!("rt_{}", uuid::Uuid::new_v4().simple());
        let gateway = Arc::new(MockGateway::new(Some(node_with_id(&device_id)), vec![]));
        let state = test_state(gateway).await;
        seed_runtime_device(&state.db, &device_id, DEV_USER).await;
        // A live (fake) relay connection keeps ticket issuance off the
        // wake-on-demand path.
        let _connection = runtime_relay::register_test_connection(&device_id).await;
        let router = routes().with_state(state);

        // No cursor → bare node path.
        let ticket = mint_ticket(&router, "sess_1", "{}").await;
        let redeemed = runtime_relay::take_socket_ticket(&ticket)
            .await
            .expect("ticket must be redeemable");
        assert_eq!(redeemed.runtime_id, device_id);
        assert_eq!(redeemed.path, "/api/v1/beta/sessions/sess_1/events/ws");

        // Cursor → the `after` query is baked into the node path.
        let ticket = mint_ticket(&router, "sess_1", r#"{"after":7}"#).await;
        let redeemed = runtime_relay::take_socket_ticket(&ticket)
            .await
            .expect("ticket must be redeemable");
        assert_eq!(
            redeemed.path,
            "/api/v1/beta/sessions/sess_1/events/ws?after=7"
        );

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn events_ws_tunnel_relays_frames_to_and_from_the_node() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");

        let device_id = format!("rt_{}", uuid::Uuid::new_v4().simple());
        let gateway = Arc::new(MockGateway::new(Some(node_with_id(&device_id)), vec![]));
        let state = test_state(gateway).await;
        seed_runtime_device(&state.db, &device_id, DEV_USER).await;
        seed_quota(&state.db).await;
        let (connection, mut node_outbound) =
            runtime_relay::register_test_connection(&device_id).await;

        let app = routes().with_state(state);
        let serve_app = app.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, serve_app).await.unwrap();
        });

        // Missing or bogus tickets are rejected before any upgrade.
        assert_eq!(
            upgrade_status(addr, "/api/v1/beta/sessions/sess_1/events/ws").await,
            StatusCode::UNAUTHORIZED.as_u16()
        );
        assert_eq!(
            upgrade_status(
                addr,
                "/api/v1/beta/sessions/sess_1/events/ws?ticket=not-a-ticket",
            )
            .await,
            StatusCode::UNAUTHORIZED.as_u16()
        );

        // A ticket minted for one session cannot be redeemed on another
        // session's upgrade URL.
        let foreign_ticket = mint_ticket(&app, "sess_2", "{}").await;
        assert_eq!(
            upgrade_status(
                addr,
                &format!("/api/v1/beta/sessions/sess_1/events/ws?ticket={foreign_ticket}"),
            )
            .await,
            StatusCode::UNAUTHORIZED.as_u16()
        );

        // Happy path: mint for sess_1 with a cursor and open the tunnel.
        let ticket = mint_ticket(&app, "sess_1", r#"{"after":2}"#).await;
        let url = format!("ws://{addr}/api/v1/beta/sessions/sess_1/events/ws?ticket={ticket}");
        let (mut client, _) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .expect("valid ticket must upgrade");

        // The node receives socket_open for the exact :8013 path (query
        // included) on its outbound relay envelope stream.
        let open = tokio::time::timeout(Duration::from_secs(5), node_outbound.recv())
            .await
            .expect("node must receive socket_open")
            .expect("envelope stream open");
        let socket_id = match open {
            CloudMessage::SocketOpen {
                socket_id, path, ..
            } => {
                assert_eq!(path, "/api/v1/beta/sessions/sess_1/events/ws?after=2");
                socket_id
            }
            other => panic!("expected socket_open, got {other:?}"),
        };
        let socket_tx = connection
            .sockets
            .lock()
            .await
            .get(&socket_id)
            .cloned()
            .expect("pump must register the tunnel");

        // Node ready → browser handshake sentinel.
        socket_tx.send(RuntimeSocketFrame::Ready).unwrap();
        let message = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("browser must receive the ready frame")
            .expect("client stream open")
            .expect("client frame ok");
        assert_eq!(
            message.to_text().unwrap(),
            r#"{"type":"allternit_socket_ready"}"#
        );

        // Node event → browser (UTF-8 event JSON passes through as text).
        socket_tx
            .send(RuntimeSocketFrame::Data(
                Bytes::from_static(br#"{"seq":1,"type":"token"}"#),
                false,
            ))
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("browser must receive the event")
            .expect("client stream open")
            .expect("client frame ok");
        assert_eq!(message.to_text().unwrap(), r#"{"seq":1,"type":"token"}"#);

        // Browser text → node socket_data (utf8).
        client
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "ack".to_string().into(),
            ))
            .await
            .unwrap();
        let inbound = tokio::time::timeout(Duration::from_secs(5), node_outbound.recv())
            .await
            .expect("node must receive browser frame")
            .expect("envelope stream open");
        match inbound {
            CloudMessage::SocketData {
                socket_id: id,
                body,
                body_encoding,
            } => {
                assert_eq!(id, socket_id);
                assert_eq!(body, "ack");
                assert_eq!(body_encoding, "utf8");
            }
            other => panic!("expected socket_data, got {other:?}"),
        }

        // Browser binary → node socket_data (base64).
        client
            .send(tokio_tungstenite::tungstenite::Message::Binary(
                vec![1, 2, 3].into(),
            ))
            .await
            .unwrap();
        let inbound = tokio::time::timeout(Duration::from_secs(5), node_outbound.recv())
            .await
            .expect("node must receive browser binary")
            .expect("envelope stream open");
        match inbound {
            CloudMessage::SocketData {
                body,
                body_encoding,
                ..
            } => {
                assert_eq!(body, STANDARD.encode([1, 2, 3]));
                assert_eq!(body_encoding, "base64");
            }
            other => panic!("expected socket_data, got {other:?}"),
        }

        // Browser close → node socket_close.
        client.close(None).await.unwrap();
        let inbound = tokio::time::timeout(Duration::from_secs(5), node_outbound.recv())
            .await
            .expect("node must receive socket_close")
            .expect("envelope stream open");
        match inbound {
            CloudMessage::SocketClose { code, .. } => assert_eq!(code, 1000),
            other => panic!("expected socket_close, got {other:?}"),
        }

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
