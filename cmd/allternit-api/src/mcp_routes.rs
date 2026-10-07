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

/// The OAuth redirect target. The browser arrives from the auth server's
/// consent screen with no Clerk JWT, so this is mounted outside the protected
/// router; the single-use `state` identifies the pending session.
pub fn mcp_oauth_public_router() -> Router<Arc<AppState>> {
    Router::new().route("/mcp/oauth/callback", get(mcp_oauth_callback))
}

pub fn mcp_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/connectors",
            get(list_mcp_connectors).post(create_mcp_connector),
        )
        .route("/connectors/:id", delete(delete_mcp_connector))
        .route(
            "/connectors/:id/oauth/start",
            post(crate::mcp_directory_routes::start_connector_oauth),
        )
        .route("/test", post(test_mcp_connection))
        .route(
            "/servers",
            get(list_mcp_servers).post(attach_mcp_server),
        )
        .route("/servers/:id", get(get_mcp_server).delete(detach_mcp_server))
        // MCP Events from the connector: list, subscribe a bot, unsubscribe.
        .merge(crate::mcp_events_client::connector_events_router())
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
    complete_oauth_callback(params, state, crate::mcp_apps::allow_private_hosts()).await
}

async fn complete_oauth_callback(
    params: CallbackQuery,
    state: axum::extract::State<Arc<AppState>>,
    allow_private: bool,
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
            render_html("Sign-in was cancelled", msg, false),
        ));
    }

    let code = params.code.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            render_html("Couldn’t finish signing in", "The app didn’t send back a sign-in code.", false),
        )
    })?;

    let state_val = params.state.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            render_html("Couldn’t finish signing in", "This sign-in link is incomplete.", false),
        )
    })?;

    // Look up session by state
    let conn = state.db.connect().map_err(|e| {
        warn!("DB error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            render_html("Couldn’t finish signing in", "Allternit couldn’t save the sign-in on this computer.", false),
        )
    })?;

    let session: Option<(String, String, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, mcp_connector_id, code_verifier, metadata, client_info FROM mcp_oauth_sessions WHERE state = ?1",
            [&state_val],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .ok();

    let (session_id, connector_id, code_verifier, metadata_json, client_info) = session.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            render_html(
                "This sign-in has expired",
                "Start connecting the app again from Allternit.",
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

    let (_conn_id, conn_name, conn_url, client_id, stored_client_secret) = connector.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            render_html(
                "App not found",
                "The app you were connecting was removed. Add it again from Allternit.",
                false,
            ),
        )
    })?;

    // Connector credentials are sealed at rest; older rows are plaintext and `open` passes them through.
    let client_secret = stored_client_secret
        .as_deref()
        .map(crate::token_crypto::open)
        .filter(|secret| !secret.is_empty());
    let redirect_uri = redirect_uri_for_session(metadata_json.as_deref());

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
                "Couldn’t finish signing in",
                &format!("Allternit couldn’t save the sign-in: {}", e),
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
        crate::mcp_directory_routes::dcr_auth_method(client_info.as_deref()).as_deref(),
        &redirect_uri,
        session_meta_str(metadata_json.as_deref(), "tokenEndpoint").as_deref(),
        session_meta_str(metadata_json.as_deref(), "resource").as_deref(),
        allow_private,
    )
    .await;

    match token_result {
        Ok(tokens) => {
            // Store tokens (sealed) and mark as authenticated
            let tokens_json = seal_token_set(&tokens);
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
                &format!("{conn_name} is connected"),
                "Allternit can now use it in your chats and bots.",
                true,
            ))
        }
        Err(e) => {
            warn!(
                connector_name = conn_name,
                error = %e,
                "MCP OAuth token exchange failed — code stored for retry"
            );
            // The auth server no longer knows the client Allternit registered for this connector:
            // forget it (DCR-issued clients only) so the next start registers a fresh one.
            if crate::mcp_directory_routes::is_invalid_client(&e) {
                if let Some(id) = client_id.as_deref() {
                    if crate::mcp_directory_routes::clear_dcr_client(&conn, &connector_id, id) {
                        warn!(connector_name = conn_name, "MCP OAuth: dynamically registered client rejected; cleared");
                    }
                }
            }

            // Code is already stored in metadata. Return a message that indicates
            // the auth code was received but token exchange needs manual completion.
            Ok(render_html(
                &format!("Couldn’t finish connecting {conn_name}"),
                &format!("The app accepted the sign-in, but Allternit couldn’t complete it: {e}"),
                false,
            ))
        }
    }
}

/// A string field the OAuth start recorded on the session (`tokenEndpoint`,
/// `resource`), if any.
pub(crate) fn session_meta_str(metadata_json: Option<&str>, key: &str) -> Option<String> {
    metadata_json
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .and_then(|m| m.get(key).and_then(|v| v.as_str()).map(String::from))
        .filter(|v| !v.is_empty())
}

/// The `redirect_uri` the authorization request used: the session's own record
/// of it when present, else this API's callback route.
fn redirect_uri_for_session(metadata_json: Option<&str>) -> String {
    let recorded = metadata_json
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .and_then(|m| {
            m.get("redirect_uri")
                .or_else(|| m.get("redirectUri"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .filter(|u| !u.is_empty());
    recorded.unwrap_or_else(default_redirect_uri)
}

/// Same URI the connector OAuth start and our CIMD document register.
fn default_redirect_uri() -> String {
    crate::mcp_directory_routes::oauth_redirect_uri(&crate::mcp_directory_routes::public_base())
}

/// Stamp a token-endpoint response with the moment it was obtained (so its
/// `expires_in` can be turned into a deadline later) and seal it for storage.
pub(crate) fn seal_token_set(tokens: &serde_json::Value) -> String {
    let mut stamped = tokens.clone();
    if let Some(obj) = stamped.as_object_mut() {
        obj.insert("obtained_at".into(), serde_json::json!(chrono::Utc::now().timestamp()));
    }
    crate::token_crypto::seal(&serde_json::to_string(&stamped).unwrap_or_default())
}

/// Seal pre-existing plaintext connector secrets (legacy bare values and
/// `plain:` values written while no key was configured). Idempotent; run at
/// startup only when an encryption key is configured, since sealing without a
/// key just re-marks values `plain:`. Returns the number of values sealed.
pub fn seal_legacy_mcp_secrets(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    if !crate::token_crypto::encryption_enabled() {
        return Ok(0);
    }
    let is_plain = |v: &str| !v.is_empty() && !v.starts_with("enc:v1:");
    let mut sealed = 0;
    for (table, column) in [("mcp_oauth_sessions", "tokens"), ("mcp_connectors", "oauth_client_secret")] {
        let rows: Vec<(String, String)> = {
            let mut stmt = conn.prepare(&format!("SELECT id, {column} FROM {table} WHERE {column} IS NOT NULL"))?;
            let collected = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            collected
        };
        for (id, value) in rows.into_iter().filter(|(_, v)| is_plain(v)) {
            conn.execute(
                &format!("UPDATE {table} SET {column} = ?1 WHERE id = ?2"),
                params![crate::token_crypto::seal(value.strip_prefix("plain:").unwrap_or(&value)), id],
            )?;
            sealed += 1;
        }
    }
    Ok(sealed)
}

/// Find the OAuth token endpoint of `server_url`: authorization-server
/// metadata first, then the common paths.
pub(crate) async fn discover_token_endpoint(client: &reqwest::Client, server_url: &str) -> Option<String> {
    let base_url = server_url.trim_end_matches('/');
    let well_known_url = format!("{}/.well-known/oauth-authorization-server", base_url);

    if let Ok(res) = client.get(&well_known_url).send().await {
        if res.status().is_success() {
            if let Ok(meta) = res.json::<serde_json::Value>().await {
                if let Some(url) = meta.get("token_endpoint").and_then(|u| u.as_str()) {
                    return Some(url.to_string());
                }
            }
        }
    }

    // Fallback: try common token endpoint paths
    for url in [
        format!("{}/oauth/token", base_url),
        format!("{}/token", base_url),
        format!("{}/v1/token", base_url),
    ] {
        if let Ok(res) = client.head(&url).send().await {
            if res.status().is_success() || res.status().as_u16() == 405 {
                return Some(url);
            }
        }
    }
    None
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
    // Token-endpoint auth method of a dynamically registered client; `None` keeps the legacy behavior.
    client_auth_method: Option<&str>,
    redirect_uri: &str,
    recorded_token_endpoint: Option<&str>,
    resource: Option<&str>,
    allow_private: bool,
) -> Result<serde_json::Value, String> {
    // The connector URL is user-supplied and so is whatever token endpoint it advertises; the code,
    // verifier and client secret go there. Both hops are validated and pinned like a connector call.
    // 1. Token endpoint: the one the OAuth start found in the auth server's
    // metadata, else discovery under the connector URL (older sessions).
    let token_url = match recorded_token_endpoint {
        Some(url) => url.to_string(),
        None => {
            let discovery = crate::mcp_apps::guarded_client(server_url, allow_private)
                .await
                .map_err(|e| e.message)?;
            discover_token_endpoint(&discovery, server_url).await.ok_or_else(|| {
                "Could not discover token endpoint. Tried .well-known/oauth-authorization-server and common paths.".to_string()
            })?
        }
    };
    let client = crate::mcp_apps::guarded_client(&token_url, allow_private)
        .await
        .map_err(|e| e.message)?;

    // 2. Build token request
    // RFC 6749 §4.1.3: redirect_uri must repeat the authorization request's.
    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("state", state),
    ];

    if let Some(verifier) = code_verifier {
        params.push(("code_verifier", verifier));
    }
    // RFC 8707 / MCP authorization: name the resource the token is for.
    if let Some(resource) = resource {
        params.push(("resource", resource));
    }

    // Client credentials. A DCR client declared one method at registration and RFC 6749 §2.3
    // allows only one per request; other clients keep sending both.
    let (in_body, in_header) = match client_auth_method {
        Some("client_secret_post") => (true, false),
        Some("client_secret_basic") => (false, true),
        _ => (true, true),
    };
    if let Some(id) = client_id {
        params.push(("client_id", id));
    }
    if let (Some(secret), true) = (client_secret, in_body) {
        params.push(("client_secret", secret));
    }

    // Build request with all params
    let mut req = client.post(&token_url).form(&params);
    if let (Some(id), Some(secret), true) = (client_id, client_secret, in_header) {
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

/// The branded sign-in result page (`oauth_result_page`); `ok` = connected (closes itself).
fn render_html(title: &str, message: &str, ok: bool) -> Html<String> {
    use crate::oauth_result_page::{render, Outcome};
    render(if ok { Outcome::Connected } else { Outcome::Failed }, title, message)
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

/// `DELETE /mcp/connectors/:id`: remove a connector the caller owns and everything it left on this computer:
/// its MCP Events subscriptions (ended at the app and the cloud first), its stored OAuth tokens and sessions, its id
/// in any agent's `mcp_connector_ids`, and the row. `204`, or `404` when it isn't the caller's.
async fn delete_mcp_connector(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl axum::response::IntoResponse {
    match delete_connector_with(&state, &crate::mcp_events_client::Ctx::production(), &user.user_id, &id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "connector not found", "code": "connector_not_found"}))).into_response(),
        Err(e) => {
            warn!(connector_id = %id, "MCP connector delete failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "could not delete the connector"}))).into_response()
        }
    }
}

/// The delete behind `DELETE /mcp/connectors/:id`. `Ok(false)` = not `user`'s connector.
pub(crate) async fn delete_connector_with(
    state: &Arc<AppState>,
    ctx: &crate::mcp_events_client::Ctx,
    user: &str,
    id: &str,
) -> Result<bool, String> {
    let (db, uid, cid) = (state.db.clone(), user.to_string(), id.to_string());
    let owned = tokio::task::spawn_blocking(move || -> rusqlite::Result<bool> {
        let conn = db.connect()?;
        match conn.query_row("SELECT 1 FROM mcp_connectors WHERE id = ?1 AND user_id = ?2", params![cid, uid], |_| Ok(())) {
            Ok(()) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    if !owned {
        return Ok(false);
    }

    // First, while the connector (and its credential) still exist: stop its event subscriptions at the app and the cloud.
    let ended = crate::mcp_events_client::end_connector_subscriptions(state, ctx, user, id).await;

    let (db, uid, cid) = (state.db.clone(), user.to_string(), id.to_string());
    tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = db.connect()?;
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM mcp_oauth_sessions WHERE mcp_connector_id = ?1", params![cid])?;
        // Unbind it from this user's agents (`mcp_connector_ids` is a JSON array of ids).
        let bound: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT id, mcp_connector_ids FROM agents WHERE user_id = ?1 AND mcp_connector_ids LIKE '%' || ?2 || '%'")?;
            let rows = stmt.query_map(params![uid, cid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for (agent_id, ids) in bound {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(&ids) {
                let kept: Vec<String> = list.into_iter().filter(|x| x != &cid).collect();
                tx.execute("UPDATE agents SET mcp_connector_ids = ?1 WHERE id = ?2", params![serde_json::to_string(&kept).unwrap_or_else(|_| "[]".into()), agent_id])?;
            }
        }
        // Ended rows whose cloud removal failed are kept for the lifecycle loop; anything else of this connector goes.
        tx.execute("DELETE FROM mcp_event_subscriptions WHERE connector_id = ?1 AND user_id = ?2 AND status <> 'ended'", params![cid, uid])?;
        tx.execute("DELETE FROM mcp_connectors WHERE id = ?1 AND user_id = ?2", params![cid, uid])?;
        tx.commit()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    info!(connector_id = %id, subscriptions_ended = ended, "MCP connector deleted");
    Ok(true)
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
    let sealed_client_secret = body
        .oauth_client_secret
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(crate::token_crypto::seal);

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
                sealed_client_secret,
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
    // The URL is caller-supplied and the response is echoed back: same guard as a connector call.
    let client = match crate::mcp_apps::guarded_client(&body.url, crate::mcp_apps::allow_private_hosts()).await {
        Ok(client) => client,
        Err(e) => {
            return Json(serde_json::json!({ "success": false, "message": e.message }));
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use std::sync::Mutex;

    #[test]
    fn redirect_uri_comes_from_the_session_else_the_callback_route() {
        assert_eq!(
            redirect_uri_for_session(Some(r#"{"redirect_uri":"https://app.test/cb"}"#)),
            "https://app.test/cb"
        );
        assert_eq!(
            redirect_uri_for_session(Some(r#"{"redirectUri":"https://app.test/cb2"}"#)),
            "https://app.test/cb2"
        );
        for none in [None, Some("not json"), Some("{}"), Some(r#"{"redirect_uri":""}"#)] {
            assert!(redirect_uri_for_session(none).ends_with("/mcp/oauth/callback"), "{none:?}");
        }
    }

    /// Token endpoint that records every form body it receives.
    async fn spawn_token_server() -> (String, Arc<Mutex<Vec<String>>>) {
        let forms: Arc<Mutex<Vec<String>>> = Arc::default();
        let captured = forms.clone();
        let app = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(|headers: axum::http::HeaderMap| async move {
                    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
                    Json(serde_json::json!({ "token_endpoint": format!("http://{host}/token") }))
                }),
            )
            .route(
                "/token",
                post(move |body: String| {
                    let captured = captured.clone();
                    async move {
                        captured.lock().unwrap().push(body);
                        Json(serde_json::json!({ "access_token": "at-secret-1", "refresh_token": "rt-secret-1", "expires_in": 3600 }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), forms)
    }

    #[tokio::test]
    async fn token_exchange_sends_redirect_uri_and_the_callback_stores_tokens_sealed() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::beta_session_routes::tests::test_app_state(&temp).await;
        let (origin, forms) = spawn_token_server().await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO mcp_connectors (id, user_id, name, name_id, url, oauth_client_id, oauth_client_secret)
             VALUES ('c1', 'u1', 'C', 'c', ?1, 'client-1', ?2)",
            params![origin, crate::token_crypto::seal("client-secret-1")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, code_verifier, metadata)
             VALUES ('s1', 'c1', 'state-1', 'verifier-1', ?1)",
            params![serde_json::json!({ "redirect_uri": "https://app.test/cb" }).to_string()],
        )
        .unwrap();

        let page = complete_oauth_callback(
            CallbackQuery { code: Some("code-1".into()), state: Some("state-1".into()), error: None, error_description: None },
            axum::extract::State(state.clone()),
            true,
        )
        .await
        .unwrap();
        assert!(page.0.contains("is connected") && page.0.contains(r#"aria-label="Allternit""#), "branded success page");

        let forms = forms.lock().unwrap().clone();
        assert_eq!(forms.len(), 1);
        let form = &forms[0];
        assert!(form.contains("grant_type=authorization_code"), "{form}");
        assert!(form.contains("redirect_uri=https%3A%2F%2Fapp.test%2Fcb"), "RFC 6749 §4.1.3: {form}");
        assert!(form.contains("code_verifier=verifier-1") && form.contains("client_id=client-1"));

        let stored: String = conn
            .query_row("SELECT tokens FROM mcp_oauth_sessions WHERE id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert!(stored.starts_with("enc:v1:"), "{stored}");
        assert!(!stored.contains("at-secret-1") && !stored.contains("rt-secret-1"));
        let opened: serde_json::Value = serde_json::from_str(&crate::token_crypto::open(&stored)).unwrap();
        assert_eq!(opened["access_token"], "at-secret-1");
        assert_eq!(opened["refresh_token"], "rt-secret-1");
        assert!(opened["obtained_at"].as_i64().is_some(), "the deadline can be computed later");
        let authenticated: i64 = conn
            .query_row("SELECT is_authenticated FROM mcp_oauth_sessions WHERE id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(authenticated, 1);
    }

    #[tokio::test]
    async fn created_connectors_store_their_client_secret_sealed() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::beta_session_routes::tests::test_app_state(&temp).await;
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-allternit-user-id", "u1".parse().unwrap());
        let user = crate::auth::AuthUser {
            user_id: "u1".into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        };
        let resp = create_mcp_connector(
            State(state.clone()),
            Extension(user),
            headers,
            Json(CreateMcpConnectorBody {
                name: "N".into(),
                name_id: "n".into(),
                url: "https://x.test/mcp".into(),
                connector_type: None,
                oauth_client_id: Some("client-1".into()),
                oauth_client_secret: Some("s3cret".into()),
            }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let stored: String = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT oauth_client_secret FROM mcp_connectors WHERE user_id = 'u1'", [], |r| r.get(0))
            .unwrap();
        assert!(stored.starts_with("enc:v1:") && !stored.contains("s3cret"), "{stored}");
        assert_eq!(crate::token_crypto::open(&stored), "s3cret");
    }

    #[tokio::test]
    async fn token_exchange_refuses_private_token_endpoints_and_never_sends_the_secret() {
        let (origin, forms) = spawn_token_server().await;
        // the same loopback server, without the development override: refused before any request
        let err = exchange_code_for_tokens(&origin, "code", "st", Some("v"), Some("id"), Some("secret"), None, "https://app.test/cb", None, None, false)
            .await
            .unwrap_err();
        assert!(err.contains("local or private"), "{err}");
        assert!(forms.lock().unwrap().is_empty());

        // the connection test endpoint is guarded the same way
        let state = crate::beta_session_routes::tests::test_app_state(&tempfile::tempdir().unwrap().keep()).await;
        let Json(result) = test_mcp_connection(
            State(state),
            Json(TestMcpBody { url: "http://169.254.169.254/latest".into() }),
        )
        .await;
        assert_eq!(result["success"], false);
    }
}
