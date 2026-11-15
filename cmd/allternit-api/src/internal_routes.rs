//! Internal-only routes the ACU (computer-use) Python gateway calls — never
//! exposed to end users or the Clerk-authenticated frontend. Gated by
//! `internal_auth::require_internal_token`, not `auth_middleware`, since the
//! calling service has no Clerk session to present. See
//! `internal_auth.rs` for why this exists and its fail-closed posture.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;

use crate::internal_auth::require_internal_token;
use crate::pricing::compute_cost_cents;
use crate::token_crypto;
use crate::AppState;

pub fn internal_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/internal/cloud-credentials/:id/resolve",
            post(resolve_credential),
        )
        .route("/internal/usage-events", post(ingest_usage_event))
        .route("/internal/push/notify/:runtime_id", post(notify_push))
        // Headless MCP path for the local gizzi daemon's MCP client — see
        // connector_routes::mcp_proxy_internal for the trust model.
        .route(
            "/internal/connectors/mcp",
            post(crate::connector_routes::mcp_proxy_internal),
        )
        // The gateway's own tool registry (shell/file/http + office-engine
        // markdown tools) over the same internal-token trust model, so the
        // gizzi runtime's bundled `allternit-tools` MCP server can call them.
        .route(
            "/internal/tools/mcp",
            post(crate::mcp_server_routes::mcp_tools_internal),
        )
}

#[derive(Debug, Deserialize)]
struct ResolveCredentialRequest {
    organization_id: String,
}

/// Resolves a sealed cloud credential for the ACU gateway to actually
/// provision against. Verifies the caller-asserted `organization_id` matches
/// the row's real owner before unsealing anything — the minimal
/// cross-org-misuse guard: a caller that doesn't already know which org owns
/// a credential id can't fish for it by guessing ids, and a mismatch is
/// logged, not just silently rejected.
async fn resolve_credential(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    Path(id): Path<String>,
    Json(body): Json<ResolveCredentialRequest>,
) -> impl IntoResponse {
    if let Err(status) = require_internal_token(&headers, peer, &state) {
        return (status, Json(json!({"error": "unauthorized"}))).into_response();
    }

    let db = state.db.clone();
    let requested_org = body.organization_id;

    let result = tokio::task::spawn_blocking(move || -> Result<Value, (StatusCode, Value)> {
        let conn = db.connect().map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()}))
        })?;

        let row = conn
            .query_row(
                "SELECT organization_id, provider, region, external_id, secret_sealed, status
                 FROM cloud_credentials WHERE id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .map_err(|_| {
                (StatusCode::NOT_FOUND, json!({"error": "not_found", "message": "No such cloud credential."}))
            })?;

        let (owner_org, provider, region, external_id, secret_sealed, status) = row;

        if owner_org != requested_org {
            warn!(
                credential_id = %id,
                requested_org = %requested_org,
                actual_org = %owner_org,
                "cloud credential resolve rejected: organization mismatch"
            );
            return Err((
                StatusCode::FORBIDDEN,
                json!({"error": "organization_mismatch", "message": "This credential does not belong to the requested organization."}),
            ));
        }
        if status != "active" {
            return Err((
                StatusCode::CONFLICT,
                json!({"error": "credential_revoked", "message": format!("Credential status is '{status}', not active.")}),
            ));
        }

        let secret_json = token_crypto::open(&secret_sealed);
        let secret: Value = serde_json::from_str(&secret_json).unwrap_or(Value::Null);

        Ok(json!({
            "provider": provider,
            "region": region,
            "external_id": external_id,
            "secret": secret,
        }))
    })
    .await;

    // Never log the response body -- it carries the unsealed secret.
    match result {
        Ok(Ok(value)) => (StatusCode::OK, Json(value)).into_response(),
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "task_join_error"}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct UsageEventRequest {
    organization_id: String,
    environment_id: String,
    resource_type: String,
    quantity: f64,
    unit: String,
    provider: Option<String>,
    started_at: String,
    ended_at: String,
    idempotency_key: Option<String>,
    metadata: Option<Value>,
}

/// Ingests one usage-metering event. Cost is always computed server-side
/// (pricing.rs) — never trust a client-supplied cost. Idempotent on
/// `idempotency_key`: the Python gateway's `stop()` does a best-effort POST
/// with no local durability, so a retried delivery after a network blip must
/// be a no-op, not a double-charge.
async fn ingest_usage_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    Json(body): Json<UsageEventRequest>,
) -> impl IntoResponse {
    if let Err(status) = require_internal_token(&headers, peer, &state) {
        return (status, Json(json!({"error": "unauthorized"}))).into_response();
    }

    let cost_cents = compute_cost_cents(&body.resource_type, &body.unit, body.quantity);
    let id = uuid::Uuid::new_v4().to_string();
    let db = state.db.clone();
    let metadata_str = body.metadata.map(|v| v.to_string());

    let result = tokio::task::spawn_blocking(move || -> Result<(), (StatusCode, Value)> {
        let conn = db.connect().map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()}))
        })?;

        conn.execute(
            "INSERT OR IGNORE INTO usage_events
                (id, organization_id, environment_id, resource_type, quantity, unit,
                 provider, started_at, ended_at, computed_cost_cents, idempotency_key, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id, body.organization_id, body.environment_id, body.resource_type,
                body.quantity, body.unit, body.provider, body.started_at, body.ended_at,
                cost_cents, body.idempotency_key, metadata_str,
            ],
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()})))?;

        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => (StatusCode::CREATED, Json(json!({"computed_cost_cents": cost_cents}))).into_response(),
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "task_join_error"}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct PushNotifyRequest {
    title: Option<String>,
    body: Option<String>,
    tag: Option<String>,
    data: Option<Value>,
}

/// Proxies a push-notification request from a paired runtime to the Cloudflare
/// Remote Control push worker. Runtimes present the internal service token
/// instead of a Clerk JWT, so the request stays within the platform trust
/// boundary rather than calling the worker directly.
async fn notify_push(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    Path(runtime_id): Path<String>,
    Json(body): Json<PushNotifyRequest>,
) -> impl IntoResponse {
    if let Err(status) = require_internal_token(&headers, peer, &state) {
        return (status, Json(json!({"error": "unauthorized"}))).into_response();
    }

    let Some(worker_url) = state.config.push_worker_url() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "push_worker_not_configured"})),
        )
            .into_response();
    };

    let url = format!("{}/push/notify/{}", worker_url.trim_end_matches('/'), runtime_id);
    let payload = json!({
        "title": body.title,
        "body": body.body,
        "tag": body.tag,
        "data": body.data,
    });

    match crate::net_client::client().post(url).json(&payload).send().await {
        Ok(response) => {
            let status = StatusCode::from_u16(response.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let body = response.text().await.unwrap_or_else(|_| "{}".to_string());
            let body_json: Value = serde_json::from_str(&body).unwrap_or(json!({"raw": body}));
            (status, Json(body_json)).into_response()
        }
        Err(error) => {
            warn!("push notify proxy failed: {}", error);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("push worker request failed: {}", error)})),
            )
                .into_response()
        }
    }
}
