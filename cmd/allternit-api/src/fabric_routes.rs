//! Clerk-protected mirror for Gizzi Fabric session-worker endpoints.
//!
//! The iOS app (and any other authenticated client) calls
//! `/api/v1/fabric/*` and `/api/v1/session-worker/*` with a Clerk bearer token.
//! This module proxies those requests to the active runtime backend at
//! `/v1/fabric/*` and `/v1/session-worker/*`, translating auth and SSE streams
//! transparently.
//!
//! This is the gateway transport layer for capability-native harness access:
//! clients do not reach the runtime directly; they use the platform gateway and
//! let it forward to the active runtime backend. For local/desktop mode the
//! active backend falls back to the local terminal server URL (`gizzi_base()`).

use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::agent_session_routes::{gizzi_base, gizzi_client};
use crate::AppState;

pub fn fabric_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/fabric/peers", get(list_peers))
        .route("/fabric/peers/local", get(get_local_peer))
        .route("/fabric/directory", get(get_directory))
        .route("/fabric/workers/self", get(get_worker_manifest))
        .route("/fabric/leases", post(issue_lease))
        .route("/session-worker/invoke", post(invoke_capability))
}

/// Resolve the base URL of the runtime backend for this user.
///
/// Reads the active remote backend target from `user_backend_preferences` /
/// `remote_backend_targets`. If a remote backend is selected and has a
/// `backend_url`, that URL is used. Otherwise the local terminal server URL
/// (`gizzi_base()`) is returned.
///
/// NOTE: remote backends currently store their runtime URL in
/// `remote_backend_targets.backend_url`. The local default is the gizzi terminal
/// server, not the gateway itself, to avoid a proxy loop.
async fn resolve_runtime_base(state: &AppState, user_id: &str) -> String {
    let db = state.db.clone();
    let user_id = user_id.to_string();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        let target_id: Option<String> = conn
            .query_row(
                "SELECT active_remote_backend_target_id
                 FROM user_backend_preferences WHERE user_id = ?1",
                params![user_id],
                |row| row.get(0),
            )
            .unwrap_or(None);

        let backend_url: Option<String> = if let Some(id) = target_id {
            conn.query_row(
                "SELECT backend_url FROM remote_backend_targets
                 WHERE user_id = ?1 AND id = ?2 AND status = 'ready'
                 ORDER BY updated_at DESC LIMIT 1",
                params![user_id, id],
                |row| row.get(0),
            )
            .unwrap_or(None)
        } else {
            None
        };

        Ok::<_, rusqlite::Error>(backend_url)
    })
    .await;

    match result {
        Ok(Ok(Some(url))) if !url.is_empty() => url.trim_end_matches('/').to_string(),
        _ => gizzi_base(),
    }
}

fn upstream_url(base: &str, path: &str) -> String {
    format!("{}{}", base, path)
}

fn user_id_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-allternit-user-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous")
        .to_string()
}

async fn proxy_json(
    state: &AppState,
    headers: &HeaderMap,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
    lease_header: Option<&str>,
) -> impl IntoResponse {
    let user_id = user_id_from_headers(headers);
    let base = resolve_runtime_base(state, &user_id).await;
    let client = gizzi_client(headers);
    let mut request = client.request(method, upstream_url(&base, path));
    if let Some(payload) = body {
        request = request.json(&payload);
    }
    if let Some(lease) = lease_header {
        request = request.header("X-Allternit-Lease", lease);
    }

    let upstream = match request.send().await {
        Ok(r) => r,
        Err(error) => {
            warn!("fabric upstream request failed: {}", error);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Upstream request failed: {}", error) })),
            )
                .into_response();
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = match upstream.text().await {
        Ok(text) => text,
        Err(error) => {
            warn!("fabric upstream body read failed: {}", error);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Upstream body read failed: {}", error) })),
            )
                .into_response();
        }
    };

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn local_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOST").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "Allternit Desktop".to_string())
}

fn local_capabilities() -> Value {
    json!([
        "harness.session",
        "harness.session.get",
        "harness.session.create",
        "harness.session.message",
        "harness.session.abort",
        "harness.session.permissions.list",
        "harness.session.permissions.reply",
        "harness.session.questions.list",
        "harness.session.questions.reply",
        "harness.session.questions.reject",
        "runtime:connect",
        "runtime:remote_control"
    ])
}

fn local_peer_doc() -> Value {
    let hostname = local_hostname();
    json!({
        "id": "local-desktop",
        "nodeId": "local-desktop",
        "name": hostname,
        "hostname": hostname,
        "status": "online",
        "runtimeType": "desktop",
        "platform": std::env::consts::OS,
        "version": env!("CARGO_PKG_VERSION"),
        "endpoints": [{
            "transport": "loopback",
            "url": gizzi_base(),
            "priority": 0
        }],
        "capabilities": local_capabilities(),
        "resources": []
    })
}

fn local_worker_manifest() -> Value {
    json!({
        "name": "desktop-session-worker",
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": local_capabilities()
    })
}

fn local_directory() -> Value {
    let peer = local_peer_doc();
    json!({
        "local": peer,
        "peers": [local_peer_doc()]
    })
}

fn local_lease(body: &Value) -> Value {
    json!({
        "id": "lease-desktop-local",
        "capabilityId": body.get("capabilityId").and_then(|v| v.as_str()).unwrap_or("harness.session"),
        "grantee": body.get("grantee").and_then(|v| v.as_str()).unwrap_or("web-client"),
        "ttlSeconds": body.get("ttlSeconds").and_then(|v| v.as_u64()).unwrap_or(300),
        "status": "active",
        "signature": "desktop-local",
        "issuedAt": chrono::Utc::now().to_rfc3339(),
    })
}

fn is_missing_upstream(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE
    )
}

async fn list_peers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let mut path = "/v1/fabric/peers".to_string();
    if !query.is_empty() {
        let params: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        path = format!("{}?{}", path, params.join("&"));
    }
    let resp = proxy_json(&state, &headers, reqwest::Method::GET, &path, None, None)
        .await
        .into_response();
    if is_missing_upstream(resp.status()) {
        return (StatusCode::OK, Json(json!([local_peer_doc()]))).into_response();
    }
    resp
}

async fn get_local_peer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let mut path = "/v1/fabric/peers/local".to_string();
    if let Some(format) = query.get("format") {
        path = format!("{}?format={}", path, urlencoding::encode(format));
    }
    let resp = proxy_json(&state, &headers, reqwest::Method::GET, &path, None, None)
        .await
        .into_response();
    if is_missing_upstream(resp.status()) {
        return (StatusCode::OK, Json(local_peer_doc())).into_response();
    }
    resp
}

async fn get_directory(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let resp = proxy_json(
        &state,
        &headers,
        reqwest::Method::GET,
        "/v1/fabric/directory",
        None,
        None,
    )
    .await
    .into_response();
    if is_missing_upstream(resp.status()) {
        return (StatusCode::OK, Json(local_directory())).into_response();
    }
    resp
}

async fn get_worker_manifest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let resp = proxy_json(
        &state,
        &headers,
        reqwest::Method::GET,
        "/v1/fabric/workers/self",
        None,
        None,
    )
    .await
    .into_response();
    if is_missing_upstream(resp.status()) {
        return (StatusCode::OK, Json(local_worker_manifest())).into_response();
    }
    resp
}

async fn issue_lease(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let config = crate::config::AppConfig::load();
    if let Some(authority_url) = config.allternitos_lease_authority_url() {
        return proxy_to_canonical_lease_authority(&state, &headers, authority_url, body).await;
    }
    // Production default: this node's gizzi issues short-lived leases for its
    // own capabilities until a canonical AllternitOS authority is configured.
    // gizzi-code does not currently expose /v1/fabric/leases, so synthesize a
    // local lease when that upstream is missing.
    let resp = proxy_json(
        &state,
        &headers,
        reqwest::Method::POST,
        "/v1/fabric/leases",
        Some(body.clone()),
        None,
    )
    .await
    .into_response();
    if is_missing_upstream(resp.status()) {
        return (StatusCode::OK, Json(local_lease(&body))).into_response();
    }
    resp
}

async fn proxy_to_canonical_lease_authority(
    _state: &AppState,
    headers: &HeaderMap,
    authority_url: String,
    body: Value,
) -> Response {
    let client = gizzi_client(headers);
    let url = format!("{}/v1/leases", authority_url.trim_end_matches('/'));
    let upstream = match client.post(url).json(&body).send().await {
        Ok(r) => r,
        Err(error) => {
            warn!("canonical lease authority request failed: {}", error);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Lease authority request failed: {}", error) })),
            )
                .into_response();
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = match upstream.text().await {
        Ok(text) => text,
        Err(error) => {
            warn!("canonical lease authority body read failed: {}", error);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Lease authority body read failed: {}", error) })),
            )
                .into_response();
        }
    };

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

async fn invoke_capability(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    // The lease rides in the JSON body as `lease` from some clients, or as the
    // X-Allternit-Lease header from others. Forward whichever is present.
    let lease_from_body = body.get("lease").and_then(|v| v.as_str()).map(String::from);
    let lease_header = headers
        .get("X-Allternit-Lease")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .or(lease_from_body);

    // Strip the lease wrapper from the body if it was embedded; the runtime
    // expects `{ capability, inputs }` plus the lease header.
    let upstream_body = if body.get("lease").is_some() {
        let mut clone = body.clone();
        clone.as_object_mut().map(|m| m.remove("lease"));
        clone
    } else {
        body
    };

    let resp = proxy_json(
        &state,
        &headers,
        reqwest::Method::POST,
        "/v1/session-worker/invoke",
        Some(upstream_body.clone()),
        lease_header.as_deref(),
    )
    .await
    .into_response();
    if is_missing_upstream(resp.status()) {
        let capability = upstream_body
            .get("capability")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let inputs = upstream_body
            .get("inputs")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match invoke_via_gizzi_sessions(&headers, capability, &inputs).await {
            Ok(result) => return (StatusCode::OK, Json(json!({ "result": result }))).into_response(),
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": error })),
                )
                    .into_response();
            }
        }
    }
    resp
}

async fn gizzi_json(
    headers: &HeaderMap,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let client = gizzi_client(headers);
    let mut req = client.request(method, format!("{}{path}", gizzi_base()));
    if let Some(payload) = body {
        req = req.json(&payload);
    }
    let res = req.send().await.map_err(|e| e.to_string())?;
    let status = res.status();
    let text = res.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "{status} {path}: {}",
            text.chars().take(240).collect::<String>()
        ));
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

async fn invoke_via_gizzi_sessions(
    headers: &HeaderMap,
    capability: &str,
    inputs: &Value,
) -> Result<Value, String> {
    let session_id = inputs
        .get("sessionID")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match capability {
        "harness.session" => {
            gizzi_json(headers, reqwest::Method::GET, "/v1/remote-control/sessions", None).await
        }
        "harness.session.get" => {
            gizzi_json(
                headers,
                reqwest::Method::GET,
                &format!("/v1/remote-control/sessions/{session_id}"),
                None,
            )
            .await
        }
        "harness.session.message" => {
            gizzi_json(
                headers,
                reqwest::Method::POST,
                &format!("/v1/remote-control/sessions/{session_id}/messages"),
                Some(json!({
                    "text": inputs.get("text"),
                    "attachments": inputs.get("attachments"),
                    "agent": inputs.get("agent"),
                    "model": inputs.get("model"),
                })),
            )
            .await
        }
        "harness.session.abort" => {
            gizzi_json(
                headers,
                reqwest::Method::POST,
                &format!("/v1/remote-control/sessions/{session_id}/abort"),
                Some(json!({})),
            )
            .await
        }
        "harness.session.create" => {
            gizzi_json(headers, reqwest::Method::POST, "/v1/session", Some(inputs.clone())).await
        }
        "harness.session.permissions.list" => {
            gizzi_json(headers, reqwest::Method::GET, "/v1/permission", None).await
        }
        "harness.session.questions.list" => {
            gizzi_json(headers, reqwest::Method::GET, "/v1/question", None).await
        }
        "harness.session.permissions.reply" => {
            let id = inputs.get("requestID").and_then(|v| v.as_str()).unwrap_or("");
            gizzi_json(
                headers,
                reqwest::Method::POST,
                &format!("/v1/permission/{id}/reply"),
                Some(json!({ "reply": inputs.get("reply"), "message": inputs.get("message") })),
            )
            .await
        }
        "harness.session.questions.reply" => {
            let id = inputs.get("requestID").and_then(|v| v.as_str()).unwrap_or("");
            gizzi_json(
                headers,
                reqwest::Method::POST,
                &format!("/v1/question/{id}/reply"),
                Some(json!({ "answers": inputs.get("answers") })),
            )
            .await
        }
        "harness.session.questions.reject" => {
            let id = inputs.get("requestID").and_then(|v| v.as_str()).unwrap_or("");
            gizzi_json(
                headers,
                reqwest::Method::POST,
                &format!("/v1/question/{id}/reject"),
                Some(json!({})),
            )
            .await
        }
        other => Err(format!("unsupported harness capability {other}")),
    }
}

