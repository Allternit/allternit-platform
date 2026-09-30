use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::{get_user, AuthUser};
use crate::AppState;

pub fn mcp_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/oauth/callback", get(mcp_oauth_callback))
        .route(
            "/connectors",
            get(list_mcp_connectors).post(create_mcp_connector),
        )
        .route("/test", post(test_mcp_connection))
        .route(
            "/servers",
            get(list_mcp_servers).post(attach_mcp_server),
        )
        .route("/servers/:id", get(get_mcp_server).delete(detach_mcp_server))
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn mcp_oauth_callback(
    Query(params): Query<CallbackQuery>,
    state: axum::extract::State<Arc<AppState>>,
) -> Result<Html<String>, (StatusCode, Html<String>)> {
    // Never log the query itself: it carries the authorization code and state.
    info!(
        has_code = params.code.is_some(),
        has_error = params.error.is_some(),
        "MCP OAuth callback received"
    );

    // Handle OAuth error from provider
    if let Some(ref error) = params.error {
        let msg = params.error_description.as_deref().unwrap_or(error);
        return Err((
            StatusCode::BAD_REQUEST,
            render_html("Connector authorization failed", msg, false),
        ));
    }

    let code = params.code.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            render_html("Invalid MCP callback", "Missing OAuth code.", false),
        )
    })?;

    let state_val = params.state.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            render_html("Invalid MCP callback", "Missing OAuth state.", false),
        )
    })?;

    // Look up session by state
    let conn = state.db.connect().map_err(|e| {
        warn!("DB error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            render_html("Database error", "Failed to connect to database.", false),
        )
    })?;

    let session: Option<(String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, mcp_connector_id, code_verifier, metadata FROM mcp_oauth_sessions WHERE state = ?1",
            [&state_val],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();

    let (session_id, connector_id, code_verifier, _metadata_json) = session.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            render_html(
                "MCP session not found",
                "This authorization session is no longer available.",
                false,
            ),
        )
    })?;

    // Look up connector
    let connector: Option<(String, String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, name, url, oauth_client_id, oauth_client_secret FROM mcp_connectors WHERE id = ?1",
            [&connector_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .ok();

    let (_conn_id, conn_name, conn_url, client_id, client_secret) = connector.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            render_html(
                "MCP connector not found",
                "The connector for this authorization session could not be loaded.",
                false,
            ),
        )
    })?;

    // Store the authorization code in the session metadata
    let updated = conn.execute(
        "UPDATE mcp_oauth_sessions SET metadata = json_insert(COALESCE(metadata, '{}'), '$.authorizationCode', ?1, '$.callbackReceivedAt', ?2), updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
        [
            &code,
            &chrono::Utc::now().to_rfc3339(),
            &session_id,
        ],
    );

    if let Err(e) = updated {
        warn!("Failed to update MCP session: {}", e);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            render_html(
                "Connector authorization failed",
                &format!("Failed to persist authorization code: {}", e),
                false,
            ),
        ));
    }

    // Attempt token exchange
    let token_result = exchange_code_for_tokens(
        &conn_url,
        &code,
        &state_val,
        code_verifier.as_deref(),
        client_id.as_deref(),
        client_secret.as_deref(),
    )
    .await;

    match token_result {
        Ok(tokens) => {
            // Store tokens and mark as authenticated
            let tokens_json = serde_json::to_string(&tokens).unwrap_or_default();
            let _ = conn.execute(
                "UPDATE mcp_oauth_sessions SET tokens = ?1, is_authenticated = 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
                [&tokens_json, &session_id],
            );

            info!(
                connector_name = conn_name,
                session_id = session_id,
                "MCP OAuth token exchange succeeded"
            );

            Ok(render_html(
                "Connector connected",
                "Authorization completed successfully. You can close this window and return to Allternit.",
                true,
            ))
        }
        Err(e) => {
            warn!(
                connector_name = conn_name,
                error = %e,
                "MCP OAuth token exchange failed — code stored for retry"
            );

            // Code is already stored in metadata. Return a message that indicates
            // the auth code was received but token exchange needs manual completion.
            Ok(render_html(
                "Authorization code received",
                &format!(
                    "The authorization code was received, but automatic token exchange failed: {}. \
                     The code has been stored and can be completed manually.",
                    e
                ),
                true,
            ))
        }
    }
}

/// Attempt to exchange the authorization code for access/refresh tokens.
/// This performs OAuth 2.0 token endpoint discovery and the code exchange.
async fn exchange_code_for_tokens(
    server_url: &str,
    code: &str,
    state: &str,
    code_verifier: Option<&str>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();

    // 1. Discover token endpoint via .well-known/oauth-authorization-server
    let base_url = server_url.trim_end_matches('/');
    let well_known_url = format!("{}/.well-known/oauth-authorization-server", base_url);

    let mut token_endpoint: Option<String> = None;

    match client.get(&well_known_url).send().await {
        Ok(res) if res.status().is_success() => {
            if let Ok(meta) = res.json::<serde_json::Value>().await {
                if let Some(url) = meta.get("token_endpoint").and_then(|u| u.as_str()) {
                    token_endpoint = Some(url.to_string());
                }
            }
        }
        _ => {}
    }

    // Fallback: try common token endpoint paths
    if token_endpoint.is_none() {
        let fallback_urls = [
            format!("{}/oauth/token", base_url),
            format!("{}/token", base_url),
            format!("{}/v1/token", base_url),
        ];
        for url in &fallback_urls {
            match client.head(url).send().await {
                Ok(res) if res.status().is_success() || res.status().as_u16() == 405 => {
                    token_endpoint = Some(url.clone());
                    break;
                }
                _ => {}
            }
        }
    }

    let token_url = token_endpoint.ok_or_else(|| {
        "Could not discover token endpoint. Tried .well-known/oauth-authorization-server and common paths.".to_string()
    })?;

    // 2. Build token request
    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("state", state),
    ];

    if let Some(verifier) = code_verifier {
        params.push(("code_verifier", verifier));
    }

    // Add client credentials to params
    if let (Some(id), Some(secret)) = (client_id, client_secret) {
        params.push(("client_id", id));
        params.push(("client_secret", secret));
    } else if let Some(id) = client_id {
        params.push(("client_id", id));
    }

    // Build request with all params
    let mut req = client.post(&token_url).form(&params);
    if let (Some(id), Some(secret)) = (client_id, client_secret) {
        req = req.basic_auth(id, Some(secret));
    }

    // 3. Execute token request
    let res = req
        .send()
        .await
        .map_err(|e| format!("Token request failed: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Token endpoint returned {}: {}", status, body));
    }

    let tokens: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    Ok(tokens)
}

fn render_html(title: &str, message: &str, close_window: bool) -> Html<String> {
    let script = if close_window {
        "<script>window.close();</script>"
    } else {
        ""
    };

    Html(format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{title}</title>
{script}
</head>
<body style="font-family: 'Allternit Sans', Inter, ui-sans-serif, system-ui, sans-serif; padding: 24px; color: #111;">
<h1 style="font-size: 18px; margin: 0 0 12px;">{title}</h1>
<p style="margin: 0; line-height: 1.5;">{message}</p>
</body>
</html>"#
    ))
}

#[derive(Serialize)]
struct McpConnectorRow {
    id: String,
    name: String,
    name_id: String,
    url: String,
    #[serde(rename = "type")]
    connector_type: String,
    oauth_client_id: Option<String>,
    enabled: bool,
    created_at: String,
    updated_at: String,
}

// ─── MCP connectors list ─────────────────────────────────────────────────────

async fn list_mcp_connectors(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Unauthorized"})),
            )
        }
    };

    let db = state.db.clone();
    let user_id = user.user_id;

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, name_id, url, type, oauth_client_id, enabled, created_at, updated_at
             FROM mcp_connectors WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![user_id], |row| {
                Ok(McpConnectorRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    name_id: row.get(2)?,
                    url: row.get(3)?,
                    connector_type: row.get(4)?,
                    oauth_client_id: row.get(5)?,
                    enabled: row.get::<_, i64>(6)? != 0,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(data)) => (
            StatusCode::OK,
            Json(serde_json::json!({"connectors": data, "total": data.len()})),
        ),
        Ok(Err(e)) => {
            warn!("DB error listing MCP connectors: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
        }
    }
}

// ─── MCP connector create ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateMcpConnectorBody {
    name: String,
    name_id: String,
    url: String,
    #[serde(rename = "type")]
    connector_type: Option<String>,
    oauth_client_id: Option<String>,
    oauth_client_secret: Option<String>,
}

async fn create_mcp_connector(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    Json(body): Json<CreateMcpConnectorBody>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Unauthorized"})),
            )
        }
    };

    let db = state.db.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let id2 = id.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO mcp_connectors (id, user_id, name, name_id, url, type, oauth_client_id, oauth_client_secret)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id2,
                user_id,
                body.name,
                body.name_id,
                body.url,
                body.connector_type.unwrap_or_else(|| "http".to_string()),
                body.oauth_client_id,
                body.oauth_client_secret,
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    }).await;

    match result {
        Ok(Ok(())) => (
            StatusCode::CREATED,
            Json(serde_json::json!({"id": id, "status": "created"})),
        ),
        Ok(Err(e)) => {
            warn!("DB error creating MCP connector: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "internal error"})),
            )
        }
    }
}

// ─── MCP test connection ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TestMcpBody {
    url: String,
}

async fn test_mcp_connection(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<TestMcpBody>,
) -> Json<serde_json::Value> {
    let client = reqwest::Client::new();
    let url = body.url.trim_end_matches('/');

    // Try to fetch the MCP server info endpoint
    match client
        .get(format!("{}/mcp/info", url))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => {
            if let Ok(json) = res.json::<serde_json::Value>().await {
                Json(serde_json::json!({
                    "success": true,
                    "message": "MCP server responded successfully",
                    "info": json,
                }))
            } else {
                Json(serde_json::json!({
                    "success": true,
                    "message": "MCP server responded (non-JSON)",
                }))
            }
        }
        Ok(res) => Json(serde_json::json!({
            "success": false,
            "message": format!("MCP server returned status {}", res.status()),
        })),
        Err(e) => Json(serde_json::json!({
            "success": false,
            "message": format!("Connection failed: {}", e),
        })),
    }
}

// ─── Attached MCP server directory ───────────────────────────────────────────

#[derive(Deserialize)]
struct AttachMcpServerBody {
    id: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

async fn list_mcp_servers(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let servers = state.mcp_dispatcher.list_servers().await;
    Json(serde_json::json!({
        "servers": servers.iter().map(|s| serde_json::json!({
            "id": s.id,
            "url": s.url,
            "tools": s.tools.len(),
        })).collect::<Vec<_>>(),
        "total": servers.len(),
    }))
}

async fn get_mcp_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl axum::response::IntoResponse {
    let servers = state.mcp_dispatcher.list_servers().await;
    match servers.into_iter().find(|s| s.id == id) {
        Some(s) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": s.id,
                "url": s.url,
                "tools": s.tools,
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "server_not_found"})),
        ),
    }
}

async fn attach_mcp_server(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AttachMcpServerBody>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "Unauthorized"})),
            )
                .into_response()
        }
    };

    match state
        .mcp_dispatcher
        .attach_and_sync(body.id.clone(), body.url, body.headers)
        .await
    {
        Ok(server) => {
            info!(
                user_id = %user.user_id,
                server_id = %server.id,
                tools = server.tools.len(),
                "MCP server attached"
            );
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": server.id,
                    "url": server.url,
                    "tools": server.tools.iter().map(|t| server.namespaced_name(&t.name)).collect::<Vec<_>>(),
                })),
            )
                .into_response()
        }
        Err(e) => {
            warn!("Failed to attach MCP server '{}': {}", body.id, e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": "attach_failed", "message": e})),
            )
                .into_response()
        }
    }
}

async fn detach_mcp_server(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl axum::response::IntoResponse {
    match state.mcp_dispatcher.detach(&id).await {
        Some(_) => (
            StatusCode::OK,
            Json(serde_json::json!({"id": id, "status": "detached"})),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "server_not_found"})),
        ),
    }
}
