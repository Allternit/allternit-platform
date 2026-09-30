//! MCP Apps host (SEP-1865), server side.
//!
//! * `POST /api/mcp/apps` — the app bridge. A rendered MCP App asks its host to
//!   talk to the connector that produced it; this proxies exactly five
//!   read/call actions to that connector over streamable HTTP, with the
//!   caller's own stored credentials, and never for a connector the caller
//!   does not own.
//! * `GET|POST /api/mcp/sandbox` — the sandbox proxy document the web client
//!   loads a `ui://` resource into.
//! * [`app_frame_for_tool_part`] — the `mcp_app` stream frame the chat bridge
//!   emits after a tool with `_meta.ui.resourceUri` completes.
//!
//! Connector records and OAuth tokens are those written by `mcp_routes.rs`
//! (`mcp_connectors`, `mcp_oauth_sessions`). Tokens are only ever placed in an
//! `Authorization` header; they are never logged or returned.

use axum::{
    extract::{Extension, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use mcp_client::{
    McpClient, McpError, ReconnectConfig, SseConfig, SseTransport, StreamableHttpConfig, StreamableHttpTransport,
    TransportError,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::auth::AuthUser;
use crate::AppState;

const EXTENSION_ID: &str = "io.modelcontextprotocol/ui";
const APP_MIME: &str = "text/html;profile=mcp-app";
/// Set to `1`/`true` to let connectors resolve to loopback/private addresses
/// (local development against a server on this machine).
const ALLOW_PRIVATE_ENV: &str = "ALLTERNIT_MCP_ALLOW_PRIVATE_CONNECTORS";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const EMIT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_LIST_PAGES: usize = 20;
const MAX_APP_HTML_BYTES: usize = 2 * 1024 * 1024;

pub fn mcp_apps_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/mcp/apps", post(app_bridge))
        .route("/mcp/sandbox", post(sandbox_page).get(sandbox_status))
}

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct AppsError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl AppsError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }
    fn not_found_connector() -> Self {
        // Same answer for "does not exist" and "belongs to someone else".
        Self::new(StatusCode::NOT_FOUND, "connector_not_found", "connector not found")
    }
    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for AppsError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": self.message, "code": self.code })),
        )
            .into_response()
    }
}

/// Map a failure talking to the connector onto a response. Messages are
/// generic where the underlying text could carry connector-side detail.
fn unauthorized() -> AppsError {
    AppsError::new(
        StatusCode::BAD_GATEWAY,
        "connector_unauthorized",
        "the connector rejected its credentials; reconnect it and try again",
    )
}

pub(crate) fn upstream_error(err: McpError) -> AppsError {
    match err {
        McpError::Transport(TransportError::Http { status, .. }) if status == 401 || status == 403 => unauthorized(),
        // The SSE transport reports a 401 as an expired OAuth token.
        McpError::OAuth(_) => unauthorized(),
        McpError::Transport(TransportError::Http { status, message }) => AppsError::new(
            StatusCode::BAD_GATEWAY,
            "connector_http_error",
            format!("connector returned HTTP {status}: {message}"),
        ),
        McpError::JsonRpc { code, message, .. } => AppsError::new(
            StatusCode::BAD_GATEWAY,
            "connector_error",
            format!("connector error ({code}): {message}"),
        ),
        McpError::Timeout(_) => AppsError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "connector_timeout",
            "the connector did not respond in time",
        ),
        other => AppsError::new(
            StatusCode::BAD_GATEWAY,
            "connector_unreachable",
            format!("could not reach the connector: {other}"),
        ),
    }
}

// ─── Connectors ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub name_id: String,
    pub url: String,
    token: Option<String>,
    /// OAuth material needed to renew `token`; never leaves this module.
    oauth: OAuthMaterial,
    /// Set when the stored token has expired and could not be renewed; every
    /// call then answers `connector_unauthorized` without contacting the connector.
    auth_failed: bool,
}

#[derive(Clone, Default)]
struct OAuthMaterial {
    /// `mcp_oauth_sessions.id` the tokens were read from (and are written back to).
    session_row: Option<String>,
    refresh_token: Option<String>,
    /// Unix seconds; `None` when the token response carried no `expires_in`.
    expires_at: Option<i64>,
    client_id: Option<String>,
    client_secret: Option<String>,
    /// Token-endpoint auth method when `client_id` was issued by dynamic registration.
    client_auth_method: Option<String>,
    /// The whole stored token-exchange response, kept so a refresh can merge into it.
    tokens: Option<Value>,
    /// Recorded by the OAuth start from the auth server's metadata.
    token_endpoint: Option<String>,
    resource: Option<String>,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector")
            .field("id", &self.id)
            .field("name_id", &self.name_id)
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Connector {
    #[cfg(test)]
    pub fn for_test(id: &str, name_id: &str, url: &str, token: Option<&str>) -> Self {
        Self {
            id: id.into(),
            name: name_id.into(),
            name_id: name_id.into(),
            url: url.into(),
            token: token.map(String::from),
            oauth: OAuthMaterial::default(),
            auth_failed: false,
        }
    }
}

/// `access_token` of a stored token-exchange response.
fn access_token_of(tokens_json: &str) -> Option<String> {
    serde_json::from_str::<Value>(tokens_json)
        .ok()?
        .get("access_token")?
        .as_str()
        .filter(|t| !t.is_empty())
        .map(String::from)
}

/// Unix-seconds deadline of a stored token set: `obtained_at + expires_in`,
/// falling back to the row's `updated_at` (older rows carry no `obtained_at`).
pub(crate) fn token_expiry(tokens: &Value, updated_at: Option<&str>) -> Option<i64> {
    let expires_in = tokens.get("expires_in").and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))?;
    let obtained = tokens.get("obtained_at").and_then(|v| v.as_i64()).or_else(|| {
        chrono::NaiveDateTime::parse_from_str(updated_at?, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|t| t.and_utc().timestamp())
    })?;
    Some(obtained + expires_in)
}

/// Columns every connector query selects, in the order `ConnectorRow` reads them.
const CONNECTOR_COLUMNS: &str = "id, name, name_id, url, enabled, oauth_client_id, oauth_client_secret";

struct ConnectorRow {
    id: String,
    name: String,
    name_id: String,
    url: String,
    enabled: i64,
    client_id: Option<String>,
    client_secret: Option<String>,
}

impl ConnectorRow {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            name: r.get(1)?,
            name_id: r.get(2)?,
            url: r.get(3)?,
            enabled: r.get(4)?,
            client_id: r.get(5)?,
            client_secret: r.get(6)?,
        })
    }
}

fn read_connector(conn: &rusqlite::Connection, row: ConnectorRow) -> rusqlite::Result<Option<Connector>> {
    if row.enabled == 0 {
        return Ok(None);
    }
    let session: Option<(String, String, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, tokens, updated_at, metadata, client_info FROM mcp_oauth_sessions
             WHERE mcp_connector_id = ?1 AND is_authenticated = 1 AND tokens IS NOT NULL
             ORDER BY updated_at DESC LIMIT 1",
            params![row.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .ok();
    let mut oauth = OAuthMaterial {
        client_id: row.client_id.filter(|c| !c.is_empty()),
        client_secret: row
            .client_secret
            .as_deref()
            .map(crate::token_crypto::open)
            .filter(|c| !c.is_empty()),
        ..OAuthMaterial::default()
    };
    let mut token = None;
    if let Some((session_row, stored, updated_at, metadata, client_info)) = session {
        oauth.client_auth_method = crate::mcp_directory_routes::dcr_auth_method(client_info.as_deref());
        oauth.token_endpoint = crate::mcp_routes::session_meta_str(metadata.as_deref(), "tokenEndpoint");
        oauth.resource = crate::mcp_routes::session_meta_str(metadata.as_deref(), "resource");
        // Sealed rows open; legacy plaintext rows pass through unchanged.
        let tokens_json = crate::token_crypto::open(&stored);
        token = access_token_of(&tokens_json);
        if let Ok(tokens) = serde_json::from_str::<Value>(&tokens_json) {
            oauth.refresh_token = tokens
                .get("refresh_token")
                .and_then(|t| t.as_str())
                .filter(|t| !t.is_empty())
                .map(String::from);
            oauth.expires_at = token_expiry(&tokens, updated_at.as_deref());
            oauth.tokens = Some(tokens);
        }
        oauth.session_row = Some(session_row);
    }
    Ok(Some(Connector {
        token,
        id: row.id,
        name: row.name,
        name_id: row.name_id,
        url: row.url,
        oauth,
        auth_failed: false,
    }))
}

/// Seconds before the stated expiry at which a token is renewed.
const REFRESH_SKEW_SECS: i64 = 60;

/// Renew the connector's access token when it has expired (or is about to) and a
/// refresh token is on file. A failed renewal marks the connector
/// unauthorized rather than sending a token known to be dead.
async fn refresh_if_needed(state: &Arc<AppState>, connector: &mut Connector, allow_private: bool) {
    let Some(expires_at) = connector.oauth.expires_at else { return };
    if chrono::Utc::now().timestamp() + REFRESH_SKEW_SECS < expires_at {
        return;
    }
    let Some(refresh_token) = connector.oauth.refresh_token.clone() else {
        // Expired with nothing to renew it: the connector will say 401; do not guess.
        return;
    };
    match request_refresh(connector, &refresh_token, allow_private).await {
        Ok(fresh) => {
            let mut merged = connector.oauth.tokens.clone().unwrap_or_else(|| json!({}));
            if let (Some(base), Some(new)) = (merged.as_object_mut(), fresh.as_object()) {
                for (k, v) in new {
                    base.insert(k.clone(), v.clone());
                }
                // Servers may rotate the refresh token or keep the old one.
                base.entry("refresh_token").or_insert(json!(refresh_token));
            }
            merged["obtained_at"] = json!(chrono::Utc::now().timestamp());
            connector.token = access_token_of(&merged.to_string());
            connector.oauth.refresh_token = merged.get("refresh_token").and_then(|t| t.as_str()).map(String::from);
            connector.oauth.expires_at = token_expiry(&merged, None);
            let sealed = crate::token_crypto::seal(&merged.to_string());
            connector.oauth.tokens = Some(merged);
            if let Some(row) = connector.oauth.session_row.clone() {
                let db = state.db.clone();
                let saved = tokio::task::spawn_blocking(move || {
                    db.connect()?.execute(
                        "UPDATE mcp_oauth_sessions SET tokens = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
                        params![sealed, row],
                    )
                })
                .await;
                if !matches!(saved, Ok(Ok(_))) {
                    warn!(connector_id = %connector.id, "mcp connector: refreshed token could not be stored");
                }
            }
            info!(connector_id = %connector.id, "mcp connector: access token refreshed");
        }
        Err(reason) => {
            warn!(connector_id = %connector.id, reason, "mcp connector: token refresh failed");
            connector.auth_failed = true;
            if reason == INVALID_CLIENT {
                // A dynamically registered client the server dropped: forget it so the next
                // OAuth start registers a new one. User-configured clients are never cleared.
                if let Some(client_id) = connector.oauth.client_id.clone() {
                    let (db, id) = (state.db.clone(), connector.id.clone());
                    let _ = tokio::task::spawn_blocking(move || {
                        db.connect().map(|c| crate::mcp_directory_routes::clear_dcr_client(&c, &id, &client_id))
                    })
                    .await;
                }
            }
        }
    }
}

/// `request_refresh` error for an `invalid_client` answer (the auth server forgot our client).
const INVALID_CLIENT: &str = "token endpoint rejected the client (invalid_client)";

/// RFC 6749 §6 refresh-token grant. Returns the new token-endpoint response.
async fn request_refresh(connector: &Connector, refresh_token: &str, allow_private: bool) -> Result<Value, &'static str> {
    // Every URL here derives from the user-supplied connector URL and ends up receiving the refresh
    // token and client secret: each is validated and pinned like a connector call.
    let token_url = match connector.oauth.token_endpoint.clone() {
        Some(url) => url,
        None => {
            let probe = guarded_client(&connector.url, allow_private).await.map_err(|_| "connector URL not allowed")?;
            crate::mcp_routes::discover_token_endpoint(&probe, &connector.url)
                .await
                .ok_or("token endpoint not found")?
        }
    };
    let client = guarded_client(&token_url, allow_private).await.map_err(|_| "token endpoint not allowed")?;

    let mut form = vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token)];
    let method = connector.oauth.client_auth_method.as_deref();
    if let Some(resource) = connector.oauth.resource.as_deref() {
        form.push(("resource", resource));
    }
    if let Some(id) = connector.oauth.client_id.as_deref() {
        form.push(("client_id", id));
    }
    if let (Some(secret), Some("client_secret_post")) = (connector.oauth.client_secret.as_deref(), method) {
        form.push(("client_secret", secret));
    }
    let mut req = client.post(&token_url).form(&form);
    if let (Some(id), Some(secret)) = (connector.oauth.client_id.as_deref(), connector.oauth.client_secret.as_deref()) {
        if method != Some("client_secret_post") {
            req = req.basic_auth(id, Some(secret));
        }
    }
    let res = req.send().await.map_err(|_| "token endpoint unreachable")?;
    if !res.status().is_success() {
        let invalid = crate::mcp_directory_routes::is_invalid_client(&res.text().await.unwrap_or_default());
        return Err(if invalid { INVALID_CLIENT } else { "token endpoint refused the refresh token" });
    }
    let body: Value = res.json().await.map_err(|_| "token endpoint returned invalid JSON")?;
    if access_token_of(&body.to_string()).is_none() {
        return Err("token endpoint returned no access token");
    }
    Ok(body)
}

/// The caller's enabled connector `connector_id`, or `None` when it does not
/// exist, is disabled, or belongs to another user.
async fn load_connector(
    state: &Arc<AppState>,
    user_id: &str,
    connector_id: &str,
    allow_private: bool,
) -> Result<Option<Connector>, AppsError> {
    let db = state.db.clone();
    let user_id = user_id.to_string();
    let connector_id = connector_id.to_string();
    let loaded = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let row = conn
            .query_row(
                &format!("SELECT {CONNECTOR_COLUMNS} FROM mcp_connectors WHERE id = ?1 AND user_id = ?2"),
                params![connector_id, user_id],
                ConnectorRow::read,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        match row {
            Some(row) => read_connector(&conn, row),
            None => Ok(None),
        }
    })
    .await
    .map_err(|e| AppsError::internal(format!("connector lookup failed: {e}")))?
    .map_err(|e: rusqlite::Error| AppsError::internal(format!("connector lookup failed: {e}")))?;
    match loaded {
        Some(mut connector) => {
            refresh_if_needed(state, &mut connector, allow_private).await;
            Ok(Some(connector))
        }
        None => Ok(None),
    }
}

/// Every enabled connector of `user_id` (newest first, ties by id).
pub(crate) async fn load_user_connectors(state: &Arc<AppState>, user_id: &str, allow_private: bool) -> Vec<Connector> {
    let db = state.db.clone();
    let user_id = user_id.to_string();
    let loaded = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<Connector>> {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {CONNECTOR_COLUMNS} FROM mcp_connectors
             WHERE user_id = ?1 ORDER BY created_at DESC, id ASC"
        ))?;
        let rows = stmt
            .query_map(params![user_id], ConnectorRow::read)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(c) = read_connector(&conn, row)? {
                out.push(c);
            }
        }
        Ok(out)
    })
    .await;
    let mut connectors = match loaded {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            warn!(error = %e, "mcp apps: could not load connectors");
            Vec::new()
        }
        Err(e) => {
            warn!(error = %e, "mcp apps: connector load task failed");
            Vec::new()
        }
    };
    for connector in connectors.iter_mut() {
        refresh_if_needed(state, connector, allow_private).await;
    }
    connectors
}

// ─── SSRF guard ──────────────────────────────────────────────────────────────

fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 0x40)
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_forbidden_ip(IpAddr::V4(mapped));
            }
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
        }
    }
}

pub(crate) fn allow_private_hosts() -> bool {
    std::env::var(ALLOW_PRIVATE_ENV)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// An address `host` was validated to resolve to; connections must go there.
pub(crate) type Pin = Option<(String, std::net::SocketAddr)>;

/// A connector URL is user-supplied and this host attaches the user's token to
/// requests sent to it, so it must be http(s) and must not resolve to a local
/// or private address (unless explicitly allowed for local development).
///
/// For a DNS name the first validated address is returned as a [`Pin`]: the
/// caller connects to exactly that address, so a second lookup (DNS rebinding)
/// cannot redirect the request somewhere that would have failed the check.
pub(crate) async fn validate_connector_url(raw: &str, allow_private: bool) -> Result<Pin, AppsError> {
    let url = url::Url::parse(raw)
        .map_err(|_| AppsError::bad_request("connector URL is not a valid URL"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(AppsError::bad_request("connector URL must be http or https"));
    }
    if allow_private {
        return Ok(None);
    }
    let forbidden = || {
        AppsError::new(
            StatusCode::FORBIDDEN,
            "connector_url_forbidden",
            "connector URL points to a local or private address",
        )
    };
    let host = url
        .host_str()
        .ok_or_else(|| AppsError::bad_request("connector URL has no host"))?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return if is_forbidden_ip(ip) { Err(forbidden()) } else { Ok(None) };
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<_> = tokio::net::lookup_host((bare, port))
        .await
        .map_err(|_| {
            AppsError::new(StatusCode::BAD_GATEWAY, "connector_unreachable", "could not resolve the connector host")
        })?
        .collect();
    choose_pin(bare, &addrs).ok_or_else(forbidden).map(Some)
}

/// The address to pin `host` to, or `None` when it resolved to nothing or to
/// anything forbidden (one bad record among several is enough to refuse).
fn choose_pin(host: &str, addrs: &[std::net::SocketAddr]) -> Option<(String, std::net::SocketAddr)> {
    if addrs.is_empty() || addrs.iter().any(|a| is_forbidden_ip(a.ip())) {
        return None;
    }
    Some((host.to_string(), addrs[0]))
}

/// An HTTP client for a user-supplied URL (OAuth discovery, token endpoints): validated like a
/// connector URL and pinned to the address that passed the check.
pub(crate) async fn guarded_client(raw: &str, allow_private: bool) -> Result<reqwest::Client, AppsError> {
    let pin = validate_connector_url(raw, allow_private).await?;
    // No redirects: a redirect would leave the address that passed the check.
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none());
    if let Some((host, addr)) = pin {
        builder = builder.resolve(&host, addr);
    }
    builder
        .build()
        .map_err(|e| AppsError::internal(format!("could not build an HTTP client: {e}")))
}

// ─── MCP session ─────────────────────────────────────────────────────────────

pub(crate) async fn open_session(connector: &Connector, allow_private: bool) -> Result<McpClient, AppsError> {
    if connector.auth_failed {
        return Err(unauthorized());
    }
    let pin = validate_connector_url(&connector.url, allow_private).await?;
    let mut config = StreamableHttpConfig::new(connector.url.trim_end_matches('/'));
    config.auth_token = connector.token.clone();
    config.timeout_secs = REQUEST_TIMEOUT_SECS;
    config.pin = pin.clone();
    let transport = StreamableHttpTransport::new(config).map_err(upstream_error)?;
    let mut client = McpClient::new(transport);
    match client.initialize().await {
        Ok(_) => Ok(client),
        // A server that predates streamable HTTP has no POST endpoint at this URL.
        Err(McpError::Transport(TransportError::Http { status, .. })) if status == 404 || status == 405 => {
            debug!(connector_id = %connector.id, status, "mcp connector: streamable HTTP refused, trying legacy SSE");
            open_legacy_sse_session(connector, pin).await
        }
        Err(e) => Err(upstream_error(e)),
    }
}

/// The pre-2025 HTTP+SSE transport: GET `<base>/sse` for the stream, POST
/// `<base>/message` for requests. A connector URL ending in `/sse` is that
/// stream endpoint; otherwise its path is the base.
async fn open_legacy_sse_session(connector: &Connector, pin: Pin) -> Result<McpClient, AppsError> {
    let url = url::Url::parse(&connector.url).map_err(|_| AppsError::bad_request("connector URL is not a valid URL"))?;
    let path = url.path().trim_end_matches('/');
    let base_path = path.strip_suffix("/sse").unwrap_or(path);
    let mut base = url.clone();
    base.set_path(base_path);
    base.set_query(None);
    base.set_fragment(None);
    let config = SseConfig {
        url: base.as_str().trim_end_matches('/').to_string(),
        sse_path: Some("/sse".into()),
        post_path: Some("/message".into()),
        auth_token: connector.token.clone(),
        timeout_secs: REQUEST_TIMEOUT_SECS,
        // One short-lived session per call: a dropped stream is not worth retrying.
        reconnect: ReconnectConfig { enabled: false, ..ReconnectConfig::default() },
        pin,
    };
    let transport = SseTransport::new(config).map_err(upstream_error)?;
    let mut client = McpClient::new(transport);
    client.initialize().await.map_err(upstream_error)?;
    Ok(client)
}

pub(crate) async fn close_session(mut client: McpClient) {
    let _ = client.shutdown().await;
}

fn cursor_params(cursor: Option<&str>) -> Option<Value> {
    cursor.map(|c| json!({ "cursor": c }))
}

/// Every tool the connector lists, following pagination.
pub(crate) async fn list_all_tools(client: &McpClient) -> Result<Vec<Value>, AppsError> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_LIST_PAGES {
        let page = client
            .request("tools/list", cursor_params(cursor.as_deref()))
            .await
            .map_err(upstream_error)?;
        if let Some(items) = page.get("tools").and_then(|t| t.as_array()) {
            tools.extend(items.iter().cloned());
        }
        cursor = page.get("nextCursor").and_then(|c| c.as_str()).map(String::from);
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

// ─── Tool metadata (`_meta.ui`) ──────────────────────────────────────────────

/// `_meta.ui.resourceUri`, else the legacy flat keys.
pub fn ui_resource_uri(tool: &Value) -> Option<String> {
    let meta = tool.get("_meta")?;
    meta.pointer("/ui/resourceUri")
        .or_else(|| meta.get("ui/resourceUri"))
        .or_else(|| meta.get("openai/outputTemplate"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// `_meta.ui.visibility`; absent (or nothing recognisable) means both.
fn ui_visibility(tool: &Value) -> Vec<&'static str> {
    let listed: Vec<&'static str> = tool
        .pointer("/_meta/ui/visibility")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| match v.as_str() {
                    Some("model") => Some("model"),
                    Some("app") => Some("app"),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if listed.is_empty() {
        vec!["model", "app"]
    } else {
        listed
    }
}

pub fn tool_visible_to_app(tool: &Value) -> bool {
    ui_visibility(tool).contains(&"app")
}

pub fn tool_visible_to_model(tool: &Value) -> bool {
    ui_visibility(tool).contains(&"model")
}

// ─── POST /api/mcp/apps ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BridgeBody {
    action: String,
    #[serde(rename = "connectorId")]
    connector_id: String,
    #[serde(default)]
    params: Option<Value>,
}

const ALLOWED_ACTIONS: [&str; 5] = [
    "tools/list",
    "tools/call",
    "resources/list",
    "resources/read",
    "resources/templates/list",
];

/// The request forwarded to the connector, built only from validated fields —
/// nothing else the app sent is passed through.
#[derive(Debug, PartialEq)]
enum Forward {
    List { method: &'static str, cursor: Option<String> },
    ToolCall { name: String, arguments: Value },
    ReadResource { uri: String },
}

fn parse_forward(action: &str, params: Option<&Value>) -> Result<Forward, AppsError> {
    let cursor = || {
        params
            .and_then(|p| p.get("cursor"))
            .and_then(|c| c.as_str())
            .map(String::from)
    };
    match action {
        "tools/list" => Ok(Forward::List { method: "tools/list", cursor: cursor() }),
        "resources/list" => Ok(Forward::List { method: "resources/list", cursor: cursor() }),
        "resources/templates/list" => Ok(Forward::List {
            method: "resources/templates/list",
            cursor: cursor(),
        }),
        "tools/call" => {
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .ok_or_else(|| AppsError::bad_request("tools/call requires params.name"))?;
            let arguments = match params.and_then(|p| p.get("arguments")) {
                None | Some(Value::Null) => json!({}),
                Some(v @ Value::Object(_)) => v.clone(),
                Some(_) => return Err(AppsError::bad_request("params.arguments must be an object")),
            };
            Ok(Forward::ToolCall { name: name.to_string(), arguments })
        }
        "resources/read" => {
            let uri = params
                .and_then(|p| p.get("uri"))
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
                .ok_or_else(|| AppsError::bad_request("resources/read requires params.uri"))?;
            Ok(Forward::ReadResource { uri: uri.to_string() })
        }
        other => Err(AppsError::new(
            StatusCode::FORBIDDEN,
            "action_not_allowed",
            format!("action '{other}' is not allowed"),
        )),
    }
}

async fn app_bridge(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<BridgeBody>,
) -> Response {
    let started = Instant::now();
    let tool_name = body
        .params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(String::from);

    let outcome = run_bridge(&state, &user.user_id, &body, allow_private_hosts()).await;

    let (status, code) = match &outcome {
        Ok(_) => (StatusCode::OK, "ok"),
        Err(e) => (e.status, e.code),
    };
    info!(
        user_id = %user.user_id,
        connector_id = %body.connector_id,
        action = %body.action,
        tool = tool_name.as_deref().unwrap_or(""),
        status = status.as_u16(),
        outcome = code,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "mcp app bridge call"
    );

    match outcome {
        Ok(result) => Json(result).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn run_bridge(
    state: &Arc<AppState>,
    user_id: &str,
    body: &BridgeBody,
    allow_private: bool,
) -> Result<Value, AppsError> {
    if !ALLOWED_ACTIONS.contains(&body.action.as_str()) {
        return Err(AppsError::new(
            StatusCode::FORBIDDEN,
            "action_not_allowed",
            format!("action '{}' is not allowed", body.action),
        ));
    }
    let forward = parse_forward(&body.action, body.params.as_ref())?;
    let connector = load_connector(state, user_id, &body.connector_id, allow_private)
        .await?
        .ok_or_else(AppsError::not_found_connector)?;
    bridge_to_connector(&connector, forward, allow_private).await
}

async fn bridge_to_connector(
    connector: &Connector,
    forward: Forward,
    allow_private: bool,
) -> Result<Value, AppsError> {
    let client = open_session(connector, allow_private).await?;
    let result = forward_request(&client, forward).await;
    close_session(client).await;
    result
}

async fn forward_request(client: &McpClient, forward: Forward) -> Result<Value, AppsError> {
    match forward {
        Forward::List { method, cursor } => client
            .request(method, cursor_params(cursor.as_deref()))
            .await
            .map_err(upstream_error),
        Forward::ReadResource { uri } => client
            .request("resources/read", Some(json!({ "uri": uri })))
            .await
            .map_err(upstream_error),
        Forward::ToolCall { name, arguments } => {
            // Visibility is only knowable from the connector's own tool list.
            let tools = list_all_tools(client).await?;
            let tool = tools
                .iter()
                .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name.as_str()))
                .ok_or_else(|| {
                    AppsError::new(StatusCode::NOT_FOUND, "tool_not_found", format!("unknown tool '{name}'"))
                })?;
            if !tool_visible_to_app(tool) {
                return Err(AppsError::new(
                    StatusCode::FORBIDDEN,
                    "tool_not_visible_to_app",
                    format!("tool '{name}' is not callable from an app"),
                ));
            }
            client
                .request("tools/call", Some(json!({ "name": name, "arguments": arguments })))
                .await
                .map_err(upstream_error)
        }
    }
}

// ─── Sandbox proxy ───────────────────────────────────────────────────────────

async fn sandbox_status() -> Json<Value> {
    Json(json!({ "available": true }))
}

#[derive(Debug, Default, Deserialize)]
struct CspInput {
    #[serde(default, alias = "connect_domains", rename = "connectDomains")]
    connect_domains: Vec<String>,
    #[serde(default, alias = "resource_domains", rename = "resourceDomains")]
    resource_domains: Vec<String>,
    #[serde(default, alias = "frame_domains", rename = "frameDomains")]
    frame_domains: Vec<String>,
    #[serde(default, alias = "base_uri_domains", rename = "baseUriDomains")]
    base_uri_domains: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SandboxRequest {
    html: String,
    #[serde(default)]
    csp: Option<Value>,
    #[serde(default)]
    permissions: Option<Value>,
    #[serde(default, rename = "toolCallId")]
    tool_call_id: String,
}

/// One CSP source: `[scheme://]host[:port][/path]` with an optional leading
/// `*.` wildcard. Rejects keywords, bare schemes (`data:`, `blob:`), lone `*`,
/// and anything that could add directives (`;`, `,`, quotes, whitespace).
fn is_plain_origin(entry: &str) -> bool {
    if entry.is_empty() || entry.len() > 255 {
        return false;
    }
    let rest = ["https://", "http://", "wss://", "ws://"]
        .iter()
        .find_map(|scheme| entry.strip_prefix(scheme))
        .unwrap_or(entry);
    let (host_port, _path) = rest.split_once('/').unwrap_or((rest, ""));
    if !rest
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/' | '*'))
    {
        return false;
    }
    let (host, port) = match host_port.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host_port, None),
    };
    if let Some(port) = port {
        if port != "*" && (port.is_empty() || !port.chars().all(|c| c.is_ascii_digit())) {
            return false;
        }
    }
    let host = host.strip_prefix("*.").unwrap_or(host);
    !host.is_empty()
        && !host.contains('*')
        && !host.starts_with(['.', '-'])
        && host.chars().any(|c| c.is_ascii_alphanumeric())
}

/// CSP source expressions come from an untrusted server; keep only plain origins.
fn sanitize_domains(domains: &[String]) -> Vec<String> {
    domains
        .iter()
        .map(|d| d.trim())
        .filter(|d| is_plain_origin(d))
        .take(50)
        .map(String::from)
        .collect()
}

/// The Content-Security-Policy a sandboxed app runs under: nothing beyond what
/// the resource declared in `_meta.ui.csp`.
pub fn build_csp(csp: &Value) -> String {
    let input: CspInput = serde_json::from_value(csp.clone()).unwrap_or_default();
    let connect = sanitize_domains(&input.connect_domains);
    let resources = sanitize_domains(&input.resource_domains);
    let frames = sanitize_domains(&input.frame_domains);
    let bases = sanitize_domains(&input.base_uri_domains);

    let with = |base: &str, extra: &[String]| {
        if extra.is_empty() {
            base.to_string()
        } else {
            format!("{base} {}", extra.join(" "))
        }
    };
    let or_none = |list: &[String]| {
        if list.is_empty() {
            "'none'".to_string()
        } else {
            list.join(" ")
        }
    };

    [
        "default-src 'none'".to_string(),
        format!("script-src {}", with("'unsafe-inline'", &resources)),
        format!("style-src {}", with("'unsafe-inline'", &resources)),
        format!("img-src {}", with("data:", &resources)),
        format!("font-src {}", or_none(&resources)),
        format!("media-src {}", with("data:", &resources)),
        format!("connect-src {}", or_none(&connect)),
        format!("frame-src {}", or_none(&frames)),
        format!("base-uri {}", or_none(&bases)),
        "object-src 'none'".to_string(),
        "form-action 'none'".to_string(),
    ]
    .join("; ")
}

/// Iframe `allow` attribute for the declared permissions.
pub fn allow_attribute(permissions: Option<&Value>) -> String {
    let Some(p) = permissions.and_then(|p| p.as_object()) else {
        return String::new();
    };
    [
        ("camera", "camera"),
        ("microphone", "microphone"),
        ("geolocation", "geolocation"),
        ("clipboardWrite", "clipboard-write"),
    ]
    .iter()
    .filter(|(key, _)| p.get(*key).is_some_and(|v| v.is_object()))
    .map(|(_, directive)| *directive)
    .collect::<Vec<_>>()
    .join("; ")
}

/// Embed a value in an inline `<script>` without letting it close the tag or
/// break out via line separators.
fn json_for_script(value: &Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// The app document with the CSP meta as its very first element, so no app
/// markup can run before the policy is in force.
fn app_document(html: &str, csp: &str) -> String {
    format!(
        "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">{html}"
    )
}

const SANDBOX_TEMPLATE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>MCP App sandbox</title>
<style>html,body{margin:0;height:100%;background:transparent}iframe{border:0;width:100%;height:100%;display:block;background:transparent}</style>
</head><body><script>
(function () {
  var CFG = __CONFIG__;
  var parentOrigin = location.origin && location.origin !== "null" ? location.origin : "*";
  var frame = document.createElement("iframe");
  // The app gets an opaque origin (the same-origin flag is deliberately left out),
  // isolating it from the host's storage, cookies and DOM.
  frame.setAttribute("sandbox", "allow-scripts allow-forms allow-popups");
  if (CFG.allow) frame.setAttribute("allow", CFG.allow);
  frame.addEventListener("load", function () {
    window.parent.postMessage({ type: "mcp-sandbox-ready", toolCallId: CFG.toolCallId }, parentOrigin);
  });
  document.body.appendChild(frame);
  frame.srcdoc = CFG.document;
  window.addEventListener("message", function (event) {
    var data = event.data;
    if (event.source === window.parent) {
      if (data && typeof data.method === "string" && data.method.indexOf("ui/notifications/sandbox-") === 0) return;
      if (frame.contentWindow) frame.contentWindow.postMessage(data, "*");
    } else if (frame.contentWindow && event.source === frame.contentWindow) {
      window.parent.postMessage(data, parentOrigin);
    }
  });
  window.parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/sandbox-proxy-ready", params: {} }, parentOrigin);
})();
</script></body></html>
"#;

/// The sandbox proxy page for one app.
pub fn render_sandbox(html: &str, csp: &Value, permissions: Option<&Value>, tool_call_id: &str) -> String {
    let config = json!({
        "toolCallId": tool_call_id,
        "allow": allow_attribute(permissions),
        "document": app_document(html, &build_csp(csp)),
    });
    SANDBOX_TEMPLATE.replace("__CONFIG__", &json_for_script(&config))
}

async fn sandbox_page(Json(req): Json<SandboxRequest>) -> Response {
    if req.html.trim().is_empty() {
        return AppsError::bad_request("html is required").into_response();
    }
    if req.html.len() > MAX_APP_HTML_BYTES {
        return AppsError::new(StatusCode::PAYLOAD_TOO_LARGE, "html_too_large", "app html is too large")
            .into_response();
    }
    let csp = req.csp.unwrap_or(Value::Null);
    let page = render_sandbox(&req.html, &csp, req.permissions.as_ref(), &req.tool_call_id);
    let mut response = (StatusCode::OK, page).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

// ─── `mcp_app` emission ──────────────────────────────────────────────────────

/// Same normalisation gizzi-code applies to MCP server names
/// (`normalizeNameForMCP`), so a gizzi server key and a connector `name_id`
/// compare equal.
pub(crate) fn normalize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

/// ChatGPT Apps SDK's original View MIME type; accepted as an MCP App View.
const SKYBRIDGE_MIME: &str = "text/html+skybridge";
/// Internal marker on the returned `ui_meta`: the View was written for
/// ChatGPT's `window.openai`, so the host injects its compat shim.
const OPENAI_COMPAT_FIELD: &str = "openaiCompat";

/// True when a `_meta` object carries any `openai/*` key or the explicit opt-in.
fn has_openai_meta(meta: Option<&Value>) -> bool {
    meta.and_then(|m| m.as_object())
        .map(|m| m.keys().any(|k| k.starts_with("openai/") || k == "allternit/openaiCompat"))
        .unwrap_or(false)
}

/// `_meta.ui`-shaped metadata from ChatGPT's resource keys (`openai/widgetCSP`,
/// `openai/widgetDomain`, `openai/widgetPrefersBorder`); used only when the
/// resource has no `_meta.ui`.
fn ui_meta_from_openai(meta: &Value) -> Value {
    let mut out = Map::new();
    if let Some(csp) = meta.get("openai/widgetCSP").filter(|c| c.is_object()) {
        out.insert("csp".into(), csp.clone());
    }
    if let Some(d) = meta.get("openai/widgetDomain").filter(|d| d.is_string()) {
        out.insert("domain".into(), d.clone());
    }
    if let Some(b) = meta.get("openai/widgetPrefersBorder").filter(|b| b.is_boolean()) {
        out.insert("prefersBorder".into(), b.clone());
    }
    Value::Object(out)
}

/// The HTML UI content of a `resources/read` result, plus the resource
/// `_meta.ui` (content-level first, then result-level). A View written for
/// ChatGPT (skybridge MIME or `openai/*` metadata) is accepted too and marked
/// with [`OPENAI_COMPAT_FIELD`].
fn html_of_resource(result: &Value) -> Option<(String, Value)> {
    let contents = result.get("contents")?.as_array()?;
    let item = contents.iter().find(|c| {
        matches!(
            c.get("mimeType").and_then(|m| m.as_str()),
            Some(APP_MIME) | Some("text/html") | Some(SKYBRIDGE_MIME)
        )
    })?;
    let html = match item.get("text").and_then(|t| t.as_str()) {
        Some(text) => text.to_string(),
        None => {
            use base64::Engine;
            let blob = item.get("blob").and_then(|b| b.as_str())?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(blob).ok()?;
            String::from_utf8(bytes).ok()?
        }
    };
    let metas = [item.get("_meta"), result.get("_meta")];
    let mut ui_meta = metas
        .into_iter()
        .flatten()
        .find_map(|meta| meta.get("ui").or_else(|| meta.get(EXTENSION_ID)))
        .cloned()
        .unwrap_or(Value::Null);
    let openai_meta = metas.into_iter().flatten().find(|m| has_openai_meta(Some(m)));
    let compat = item.get("mimeType").and_then(|m| m.as_str()) == Some(SKYBRIDGE_MIME) || openai_meta.is_some();
    if ui_meta.is_null() {
        if let Some(meta) = openai_meta {
            ui_meta = ui_meta_from_openai(meta);
        }
    }
    if compat {
        if !ui_meta.is_object() {
            ui_meta = json!({});
        }
        ui_meta[OPENAI_COMPAT_FIELD] = json!(true);
    }
    Some((html, ui_meta))
}

fn camel_csp(ui_meta: &Value) -> Option<Value> {
    let csp = ui_meta.get("csp")?;
    let pick = |camel: &str, snake: &str| -> Value {
        csp.get(camel)
            .or_else(|| csp.get(snake))
            .filter(|v| v.is_array())
            .cloned()
            .unwrap_or(Value::Null)
    };
    let mut out = Map::new();
    for (camel, snake) in [
        ("connectDomains", "connect_domains"),
        ("resourceDomains", "resource_domains"),
        ("frameDomains", "frame_domains"),
        ("baseUriDomains", "base_uri_domains"),
    ] {
        let v = pick(camel, snake);
        if !v.is_null() {
            out.insert(camel.to_string(), v);
        }
    }
    Some(Value::Object(out))
}

/// The tool-call result an app receives (`ui/notifications/tool-result`):
/// the connector's own result when gizzi preserved it, else the model-visible
/// text output.
fn tool_result_of(mcp_meta: &Value, output: Option<&Value>) -> Value {
    if let Some(raw) = mcp_meta.get("result").filter(|r| r.is_object()) {
        return raw.clone();
    }
    let text = match output {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    json!({ "content": [{ "type": "text", "text": text }] })
}

/// Build the `mcp_app` frame for a resolved tool + resource. Pure so it can be
/// tested without a connector.
#[allow(clippy::too_many_arguments)]
fn build_app_frame(
    message_id: &str,
    call_id: &str,
    connector: &Connector,
    tool: &Value,
    resource_uri: &str,
    html: String,
    ui_meta: &Value,
    tool_input: Value,
    tool_result: Value,
) -> Value {
    let tool_name = tool.get("name").and_then(|n| n.as_str()).unwrap_or_default();
    let str_field = |key: &str| tool.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let title = str_field("title")
        .or_else(|| str_field("description"))
        .unwrap_or(&connector.name);

    let permissions = ui_meta.get("permissions").filter(|p| p.is_object()).cloned();
    let mut frame = json!({
        "type": "mcp_app",
        "messageId": message_id,
        "toolCallId": call_id,
        "toolName": tool_name,
        "connectorId": connector.id,
        "connectorName": connector.name,
        "title": title,
        "resourceUri": resource_uri,
        "html": html,
        "allow": allow_attribute(permissions.as_ref()),
        "prefersBorder": ui_meta.get("prefersBorder").and_then(|b| b.as_bool()).unwrap_or(true),
        "tool": {
            "name": tool_name,
            "title": tool.get("title"),
            "description": tool.get("description"),
            "inputSchema": tool.get("inputSchema"),
            "annotations": tool.get("annotations"),
            "_meta": tool.get("_meta"),
        },
        "toolInput": tool_input,
        "toolResult": tool_result,
    });
    let obj = frame.as_object_mut().expect("frame is an object");
    if let Some(d) = str_field("description") {
        obj.insert("description".into(), json!(d));
    }
    if let Some(csp) = camel_csp(ui_meta) {
        obj.insert("csp".into(), csp);
    }
    // A View written for ChatGPT's `window.openai`: the host injects its shim.
    if ui_meta.get(OPENAI_COMPAT_FIELD) == Some(&json!(true)) || has_openai_meta(tool.get("_meta")) {
        obj.insert(OPENAI_COMPAT_FIELD.into(), json!(true));
    }
    if let Some(p) = permissions {
        obj.insert("permissions".into(), p);
    }
    if let Some(domain) = ui_meta.get("domain").and_then(|d| d.as_str()) {
        obj.insert("domain".into(), json!(domain));
    }
    frame
}

/// Everything `app_frame_for_tool_part` needs from a gizzi tool part.
#[derive(Debug)]
struct McpToolCall<'a> {
    server: &'a str,
    tool: &'a str,
    meta: &'a Value,
    input: Value,
    output: Option<&'a Value>,
}

fn mcp_tool_call(part: &Value) -> Option<McpToolCall<'_>> {
    let meta = part.pointer("/state/metadata/mcp")?;
    Some(McpToolCall {
        server: meta.get("server")?.as_str()?,
        tool: meta.get("tool")?.as_str()?,
        meta,
        input: part
            .pointer("/state/input")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| json!({})),
        output: part.pointer("/state/output"),
    })
}

/// The `mcp_app` frame for a completed gizzi `tool` part, when that part is an
/// MCP tool of one of the user's connectors that declares a `ui://` resource.
/// Best effort: any failure means "no app", never a broken chat turn.
pub async fn app_frame_for_tool_part(
    state: &Arc<AppState>,
    user_id: &str,
    message_id: &str,
    part: &Value,
) -> Option<Value> {
    app_frame_for_tool_part_with(state, user_id, message_id, part, allow_private_hosts()).await
}

pub(crate) async fn app_frame_for_tool_part_with(
    state: &Arc<AppState>,
    user_id: &str,
    message_id: &str,
    part: &Value,
    allow_private: bool,
) -> Option<Value> {
    let call = mcp_tool_call(part)?;
    let call_id = part.get("callID").and_then(|c| c.as_str())?;
    let server = normalize_name(call.server);

    let connectors = load_user_connectors(state, user_id, allow_private).await;
    // Through the per-user proxy the tool name is `<connector>__<tool>`; resolve it back.
    let (connector, tool_name) = if server == normalize_name(crate::mcp_user_proxy::PROXY_SERVER_NAME) {
        let (connector, tool) = crate::mcp_user_proxy::resolve_namespaced(&connectors, call.tool)?;
        (connector, tool)
    } else {
        // A gizzi server configured directly: matched to a connector by name.
        let Some(connector) = connectors.iter().find(|c| normalize_name(&c.name_id) == server) else {
            debug!(server = call.server, "mcp apps: no connector for MCP server");
            return None;
        };
        (connector, call.tool)
    };

    match tokio::time::timeout(
        EMIT_TIMEOUT,
        emit_for_connector(connector, &call, tool_name, message_id, call_id, allow_private),
    )
    .await
    {
        Ok(Ok(frame)) => frame,
        Ok(Err(e)) => {
            warn!(
                connector_id = %connector.id,
                tool = tool_name,
                error = %e.message,
                "mcp apps: could not build app frame"
            );
            None
        }
        Err(_) => {
            warn!(connector_id = %connector.id, tool = tool_name, "mcp apps: app frame timed out");
            None
        }
    }
}

async fn emit_for_connector(
    connector: &Connector,
    call: &McpToolCall<'_>,
    tool_name: &str,
    message_id: &str,
    call_id: &str,
    allow_private: bool,
) -> Result<Option<Value>, AppsError> {
    let client = open_session(connector, allow_private).await?;
    let built = async {
        let tools = list_all_tools(&client).await?;
        let Some(tool) = tools
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(tool_name))
        else {
            return Ok(None);
        };
        let Some(uri) = ui_resource_uri(tool) else {
            return Ok(None);
        };
        if !uri.starts_with("ui://") {
            return Ok(None);
        }
        let resource = client
            .request("resources/read", Some(json!({ "uri": uri })))
            .await
            .map_err(upstream_error)?;
        let Some((html, ui_meta)) = html_of_resource(&resource) else {
            return Ok(None);
        };
        if html.len() > MAX_APP_HTML_BYTES {
            warn!(connector_id = %connector.id, "mcp apps: app html over size limit; not emitted");
            return Ok(None);
        }
        Ok(Some(build_app_frame(
            message_id,
            call_id,
            connector,
            tool,
            &uri,
            html,
            &ui_meta,
            call.input.clone(),
            tool_result_of(call.meta, call.output),
        )))
    }
    .await;
    close_session(client).await;
    built
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn resource_uri_and_visibility_follow_the_spec_defaults() {
        let plain = json!({ "name": "t" });
        assert_eq!(ui_resource_uri(&plain), None);
        assert!(tool_visible_to_app(&plain) && tool_visible_to_model(&plain));

        let nested = json!({ "_meta": { "ui": { "resourceUri": "ui://a/b" } } });
        assert_eq!(ui_resource_uri(&nested).as_deref(), Some("ui://a/b"));
        let legacy = json!({ "_meta": { "openai/outputTemplate": "ui://w" } });
        assert_eq!(ui_resource_uri(&legacy).as_deref(), Some("ui://w"));

        let app_only = json!({ "_meta": { "ui": { "visibility": ["app"] } } });
        assert!(tool_visible_to_app(&app_only));
        assert!(!tool_visible_to_model(&app_only));
        let model_only = json!({ "_meta": { "ui": { "visibility": ["model"] } } });
        assert!(!tool_visible_to_app(&model_only));
        assert!(tool_visible_to_model(&model_only));
        // nothing recognisable falls back to both
        let junk = json!({ "_meta": { "ui": { "visibility": ["x"] } } });
        assert!(tool_visible_to_app(&junk) && tool_visible_to_model(&junk));
    }

    #[test]
    fn only_the_five_actions_are_forwardable() {
        for action in ALLOWED_ACTIONS {
            let params = match action {
                "tools/call" => json!({ "name": "t" }),
                "resources/read" => json!({ "uri": "ui://x" }),
                _ => json!({}),
            };
            assert!(parse_forward(action, Some(&params)).is_ok(), "{action}");
        }
        for action in ["prompts/list", "prompts/get", "initialize", "sampling/createMessage", "tools/call2", ""] {
            let err = parse_forward(action, None).unwrap_err();
            assert_eq!(err.status, StatusCode::FORBIDDEN, "{action}");
        }
    }

    #[test]
    fn forwarded_params_are_rebuilt_not_passed_through() {
        let p = json!({ "name": "t", "arguments": { "a": 1 }, "_meta": { "evil": true }, "extra": 1 });
        assert_eq!(
            parse_forward("tools/call", Some(&p)).unwrap(),
            Forward::ToolCall { name: "t".into(), arguments: json!({ "a": 1 }) }
        );
        assert_eq!(
            parse_forward("tools/call", Some(&json!({ "name": "t" }))).unwrap(),
            Forward::ToolCall { name: "t".into(), arguments: json!({}) }
        );
        assert!(parse_forward("tools/call", Some(&json!({ "name": "t", "arguments": [1] }))).is_err());
        assert!(parse_forward("tools/call", Some(&json!({}))).is_err());
        assert!(parse_forward("resources/read", None).is_err());
        assert_eq!(
            parse_forward("tools/list", Some(&json!({ "cursor": "c", "x": 1 }))).unwrap(),
            Forward::List { method: "tools/list", cursor: Some("c".into()) }
        );
    }

    #[test]
    fn access_token_is_read_from_the_stored_exchange() {
        assert_eq!(
            access_token_of(r#"{"access_token":"abc","token_type":"Bearer"}"#).as_deref(),
            Some("abc")
        );
        assert_eq!(access_token_of(r#"{"access_token":""}"#), None);
        assert_eq!(access_token_of("not json"), None);
    }

    #[test]
    fn connector_debug_never_shows_the_token() {
        let c = Connector::for_test("c1", "demo", "https://x.test/mcp", Some("super-secret"));
        assert!(!format!("{c:?}").contains("super-secret"));
    }

    #[tokio::test]
    async fn ssrf_guard_rejects_local_and_private_targets() {
        for url in [
            "http://127.0.0.1/mcp",
            "http://localhost:8080/mcp",
            "http://10.0.0.5/mcp",
            "http://192.168.1.1/mcp",
            "http://169.254.169.254/latest",
            "http://[::1]/mcp",
            "http://[::ffff:127.0.0.1]/mcp",
            "http://100.64.0.1/mcp",
        ] {
            let err = validate_connector_url(url, false).await.unwrap_err();
            assert!(
                err.status == StatusCode::FORBIDDEN || err.status == StatusCode::BAD_GATEWAY,
                "{url}: {err:?}"
            );
        }
        assert_eq!(
            validate_connector_url("ftp://example.com/x", false).await.unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        assert!(validate_connector_url("http://93.184.216.34/mcp", false).await.is_ok());
        assert!(validate_connector_url("http://127.0.0.1/mcp", true).await.is_ok());
    }

    #[test]
    fn dns_answers_are_pinned_only_when_every_record_is_public() {
        let addr = |s: &str| -> std::net::SocketAddr { s.parse().unwrap() };
        let public = addr("93.184.216.34:443");
        assert_eq!(choose_pin("example.com", &[public]), Some(("example.com".into(), public)));
        // several public records: the first one is what the connection is pinned to
        assert_eq!(
            choose_pin("example.com", &[public, addr("93.184.216.35:443")]),
            Some(("example.com".into(), public))
        );
        // a rebinding-style answer mixing in one private/loopback/link-local record is refused outright
        for bad in ["127.0.0.1:443", "10.1.2.3:443", "169.254.169.254:443", "[::1]:443", "100.64.0.9:443"] {
            assert_eq!(choose_pin("rebind.example", &[public, addr(bad)]), None, "{bad}");
            assert_eq!(choose_pin("rebind.example", &[addr(bad)]), None, "{bad}");
        }
        assert_eq!(choose_pin("nothing.example", &[]), None);
    }

    #[test]
    fn csp_defaults_to_nothing_and_only_adds_declared_origins() {
        let none = build_csp(&Value::Null);
        assert!(none.contains("default-src 'none'"));
        assert!(none.contains("connect-src 'none'"));
        assert!(none.contains("frame-src 'none'"));
        assert!(none.contains("base-uri 'none'"));

        let csp = build_csp(&json!({
            "connectDomains": ["https://api.example.com", "wss://live.example.com"],
            "resourceDomains": ["https://cdn.example.com", "*.static.example.com"],
            "frameDomains": ["https://embed.example.com"],
        }));
        assert!(csp.contains("connect-src https://api.example.com wss://live.example.com;"));
        assert!(csp.contains("script-src 'unsafe-inline' https://cdn.example.com *.static.example.com;"));
        assert!(csp.contains("frame-src https://embed.example.com;"));
        assert!(csp.contains("img-src data: https://cdn.example.com"));
    }

    #[test]
    fn csp_drops_entries_that_could_inject_directives() {
        let csp = build_csp(&json!({
            "connectDomains": [
                "*",
                "https://ok.example.com; script-src *",
                "'unsafe-eval'",
                "data:",
                "blob:",
                "https://a.example.com, https://b.example.com",
                "https://good.example.com",
                "javascript:alert(1)",
                ""
            ],
            "base_uri_domains": ["https://base.example.com"],
        }));
        assert!(csp.contains("connect-src https://good.example.com;"), "{csp}");
        assert!(!csp.contains("unsafe-eval"));
        assert!(!csp.contains("script-src *"));
        assert!(!csp.contains("blob:"));
        assert!(!csp.contains("javascript:"));
        // snake_case keys from a raw resource `_meta` are honoured too
        assert!(csp.contains("base-uri https://base.example.com"));
    }

    #[test]
    fn allow_attribute_lists_declared_permissions() {
        assert_eq!(allow_attribute(None), "");
        assert_eq!(
            allow_attribute(Some(&json!({ "camera": {}, "clipboardWrite": {}, "bogus": {} }))),
            "camera; clipboard-write"
        );
    }

    #[test]
    fn sandbox_page_embeds_the_app_under_a_leading_csp_and_escapes_script_breakouts() {
        let html = "</script><script>alert(1)</script><p>\u{2028}</p>";
        let page = render_sandbox(html, &json!({ "connectDomains": ["https://api.example.com"] }), None, "call-1");
        // the app html can never terminate the proxy's own script element
        assert_eq!(page.matches("</script>").count(), 1, "{page}");
        assert!(!page.contains('\u{2028}'));
        assert!(page.contains("call-1"));
        assert!(page.contains("mcp-sandbox-ready"));
        assert!(page.contains("ui/notifications/sandbox-proxy-ready"));
        // the inner frame is isolated: scripts yes, same-origin no
        assert!(page.contains("allow-scripts allow-forms allow-popups"));
        assert!(!page.contains("allow-same-origin"));
        // CSP meta is the first thing in the app document
        assert!(page.contains("\\u003c!doctype html\\u003e\\u003cmeta http-equiv=\\\"Content-Security-Policy\\\""));
    }

    fn sample_tool() -> Value {
        json!({
            "name": "show_chart",
            "title": "Chart",
            "description": "Draw a chart",
            "inputSchema": { "type": "object" },
            "_meta": { "ui": { "resourceUri": "ui://chart/app" } }
        })
    }

    #[test]
    fn app_frame_has_every_field_the_stream_adapter_requires() {
        let connector = Connector::for_test("conn-1", "chart-server", "https://x.test/mcp", None);
        let resource = json!({ "contents": [{
            "uri": "ui://chart/app",
            "mimeType": APP_MIME,
            "text": "<html>app</html>",
            "_meta": { "ui": {
                "csp": { "connectDomains": ["https://api.example.com"] },
                "permissions": { "camera": {} },
                "domain": "chart.example.com",
                "prefersBorder": false
            }}
        }]});
        let (html, ui_meta) = html_of_resource(&resource).unwrap();
        let mcp_meta = json!({
            "server": "chart-server",
            "tool": "show_chart",
            "result": { "content": [{ "type": "text", "text": "ok" }], "structuredContent": { "n": 3 }, "_meta": { "k": 1 } }
        });
        let frame = build_app_frame(
            "msg_1",
            "call_1",
            &connector,
            &sample_tool(),
            "ui://chart/app",
            html,
            &ui_meta,
            json!({ "q": "sales" }),
            tool_result_of(&mcp_meta, None),
        );

        // rust-stream-adapter.ts buildMcpAppPart() requires all of these
        for key in ["toolCallId", "toolName", "connectorId", "connectorName", "resourceUri", "html", "title"] {
            assert!(frame[key].as_str().is_some_and(|s| !s.is_empty()), "{key}: {frame}");
        }
        assert_eq!(frame["type"], "mcp_app");
        assert_eq!(frame["toolCallId"], "call_1");
        assert_eq!(frame["connectorId"], "conn-1");
        assert_eq!(frame["toolName"], "show_chart");
        assert_eq!(frame["title"], "Chart");
        assert_eq!(frame["resourceUri"], "ui://chart/app");
        assert_eq!(frame["prefersBorder"], false);
        assert_eq!(frame["allow"], "camera");
        assert_eq!(frame["domain"], "chart.example.com");
        assert_eq!(frame["csp"]["connectDomains"][0], "https://api.example.com");
        assert_eq!(frame["permissions"]["camera"], json!({}));
        assert_eq!(frame["toolInput"]["q"], "sales");
        assert_eq!(frame["toolResult"]["structuredContent"]["n"], 3);
        assert_eq!(frame["toolResult"]["_meta"]["k"], 1);
        assert_eq!(frame["tool"]["_meta"]["ui"]["resourceUri"], "ui://chart/app");
    }

    #[test]
    fn resource_meta_is_found_at_content_level_result_level_or_extension_key() {
        let content_level = json!({ "contents": [{ "mimeType": APP_MIME, "text": "h", "_meta": { "ui": { "domain": "a" } } }] });
        assert_eq!(html_of_resource(&content_level).unwrap().1["domain"], "a");
        let result_level = json!({ "_meta": { "ui": { "domain": "b" } }, "contents": [{ "mimeType": "text/html", "text": "h" }] });
        assert_eq!(html_of_resource(&result_level).unwrap().1["domain"], "b");
        let ext_key = json!({ "_meta": { EXTENSION_ID: { "domain": "c" } }, "contents": [{ "mimeType": APP_MIME, "text": "h" }] });
        assert_eq!(html_of_resource(&ext_key).unwrap().1["domain"], "c");
        assert!(html_of_resource(&json!({ "contents": [{ "mimeType": "text/plain", "text": "h" }] })).is_none());
        // ChatGPT-style Views: skybridge MIME and openai/* metadata are accepted and marked.
        let sky = json!({ "contents": [{ "mimeType": "text/html+skybridge", "text": "h",
            "_meta": { "openai/widgetCSP": { "connect_domains": ["https://a.test"] }, "openai/widgetPrefersBorder": false } }] });
        let (_, meta) = html_of_resource(&sky).unwrap();
        assert_eq!(meta["openaiCompat"], true);
        assert_eq!(camel_csp(&meta).unwrap()["connectDomains"][0], "https://a.test");
        assert_eq!(meta["prefersBorder"], false);
        // A standard View is not marked, and _meta.ui wins over openai/* keys.
        assert!(html_of_resource(&content_level).unwrap().1.get("openaiCompat").is_none());
        let both = json!({ "contents": [{ "mimeType": APP_MIME, "text": "h",
            "_meta": { "ui": { "domain": "std" }, "openai/widgetDomain": "legacy" } }] });
        let (_, meta) = html_of_resource(&both).unwrap();
        assert_eq!((meta["domain"].as_str(), meta["openaiCompat"].as_bool()), (Some("std"), Some(true)));
    }

    #[test]
    fn tool_part_is_only_an_app_candidate_when_gizzi_tagged_it() {
        let plain = json!({ "type": "tool", "callID": "c", "tool": "read", "state": { "status": "completed" } });
        assert!(mcp_tool_call(&plain).is_none());
        let tagged = json!({
            "type": "tool", "callID": "c", "tool": "mcp__s__t",
            "state": { "status": "completed", "input": { "a": 1 }, "output": "text",
                       "metadata": { "mcp": { "server": "s", "tool": "t" } } }
        });
        let call = mcp_tool_call(&tagged).unwrap();
        assert_eq!((call.server, call.tool), ("s", "t"));
        assert_eq!(call.input["a"], 1);
        // no preserved raw result: fall back to the model-visible output
        assert_eq!(tool_result_of(call.meta, call.output)["content"][0]["text"], "text");
    }

    #[test]
    fn server_names_normalise_like_gizzi_code() {
        assert_eq!(normalize_name("my server.v2"), "my_server_v2");
        assert_eq!(normalize_name("ok-name_1"), "ok-name_1");
    }

    // ── integration: a real MCP Apps server over streamable HTTP ─────────────

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::sync::Mutex;
    use tower::ServiceExt;

    pub(crate) const APP_HTML: &str = "<html><body><h1>dash</h1></body></html>";
    pub(crate) const TOKEN: &str = "tok-abc-123";

    #[derive(Debug, Clone)]
    pub(crate) struct Seen {
        pub(crate) method: String,
        pub(crate) auth: Option<String>,
        session: Option<String>,
        version: Option<String>,
        accept: Option<String>,
        params: Value,
    }

    #[derive(Clone, Default)]
    pub(crate) struct TestServer {
        pub(crate) log: Arc<Mutex<Vec<Seen>>>,
        deleted: Arc<Mutex<bool>>,
    }

    impl TestServer {
        pub(crate) fn methods(&self) -> Vec<String> {
            self.log.lock().unwrap().iter().map(|s| s.method.clone()).collect()
        }
        pub(crate) fn count(&self) -> usize {
            self.log.lock().unwrap().len()
        }
        pub(crate) fn called_tools(&self) -> Vec<String> {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.method == "tools/call")
                .filter_map(|s| s.params["name"].as_str().map(String::from))
                .collect()
        }
    }

    fn hdr(h: &axum::http::HeaderMap, name: &str) -> Option<String> {
        h.get(name).and_then(|v| v.to_str().ok()).map(String::from)
    }

    async fn test_mcp(
        State(server): State<TestServer>,
        headers: axum::http::HeaderMap,
        Json(req): Json<Value>,
    ) -> Response {
        let method = req["method"].as_str().unwrap_or("").to_string();
        server.log.lock().unwrap().push(Seen {
            method: method.clone(),
            auth: hdr(&headers, "authorization"),
            session: hdr(&headers, "mcp-session-id"),
            version: hdr(&headers, "mcp-protocol-version"),
            accept: hdr(&headers, "accept"),
            params: req["params"].clone(),
        });

        if hdr(&headers, "authorization").as_deref() != Some(&format!("Bearer {TOKEN}")) {
            return (StatusCode::UNAUTHORIZED, "no").into_response();
        }
        let Some(id) = req.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });

        if method == "initialize" {
            let ext = &req["params"]["capabilities"]["extensions"][EXTENSION_ID];
            let payload = if ext["mimeTypes"][0] == APP_MIME {
                ok(json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {}, "resources": {} },
                    "serverInfo": { "name": "dash", "version": "1" }
                }))
            } else {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "host must advertise MCP Apps" } })
            };
            return Response::builder()
                .header("content-type", "application/json")
                .header("mcp-session-id", "sess-dash")
                .body(Body::from(payload.to_string()))
                .unwrap();
        }
        // everything after initialize must carry the session and negotiated version
        if hdr(&headers, "mcp-session-id").as_deref() != Some("sess-dash")
            || hdr(&headers, "mcp-protocol-version").as_deref() != Some("2025-06-18")
        {
            return (StatusCode::BAD_REQUEST, "missing session headers").into_response();
        }

        let payload = match method.as_str() {
            "tools/list" => ok(json!({ "tools": [
                {
                    "name": "show_dashboard", "title": "Dashboard", "description": "Show the dashboard",
                    "inputSchema": { "type": "object", "properties": { "range": { "type": "string" } } },
                    "_meta": { "ui": { "resourceUri": "ui://dash/app" } }
                },
                {
                    "name": "refresh_data", "description": "App-only refresh",
                    "inputSchema": { "type": "object" },
                    "_meta": { "ui": { "resourceUri": "ui://dash/app", "visibility": ["app"] } }
                },
                {
                    "name": "model_only", "description": "Model-only",
                    "inputSchema": { "type": "object" },
                    "annotations": { "readOnlyHint": true },
                    "_meta": { "ui": { "visibility": ["model"] } }
                },
                { "name": "plain", "inputSchema": { "type": "object" }, "annotations": { "destructiveHint": true } }
            ]})),
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                let result = json!({
                    "content": [{ "type": "text", "text": format!("ran {name}") }],
                    "structuredContent": { "tool": name, "rows": [1, 2, 3] },
                    "_meta": { "trace": "t-1" }
                });
                if name == "show_dashboard" {
                    // answered as an event stream, with a notification ahead of the response
                    let note = json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": { "progress": 1 } });
                    let text = format!("data: {note}\n\nevent: message\ndata: {}\n\n", ok(result));
                    return Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from(text))
                        .unwrap();
                }
                ok(result)
            }
            "resources/list" => ok(json!({ "resources": [
                { "uri": "ui://dash/app", "name": "Dashboard app", "mimeType": APP_MIME }
            ]})),
            "resources/templates/list" => ok(json!({ "resourceTemplates": [] })),
            "resources/read" => ok(json!({ "contents": [{
                "uri": req["params"]["uri"],
                "mimeType": APP_MIME,
                "text": APP_HTML,
                "_meta": { "ui": {
                    "csp": { "connectDomains": ["https://api.dash.example"], "resourceDomains": ["https://cdn.dash.example"] },
                    "permissions": { "clipboardWrite": {} },
                    "prefersBorder": false
                }}
            }]})),
            other => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("no {other}") } }),
        };
        (StatusCode::OK, Json(payload)).into_response()
    }

    async fn spawn_mcp_apps_server() -> (String, TestServer) {
        spawn_mcp_apps_server_with(Router::new()).await
    }

    /// The MCP Apps test server plus `extra` routes (e.g. an OAuth token endpoint on the same origin).
    pub(crate) async fn spawn_mcp_apps_server_with(extra: Router) -> (String, TestServer) {
        let server = TestServer::default();
        let deleted = server.deleted.clone();
        let app = Router::new()
            .route(
                "/mcp",
                post(test_mcp).delete(move || {
                    let deleted = deleted.clone();
                    async move {
                        *deleted.lock().unwrap() = true;
                        StatusCode::OK
                    }
                }),
            )
            .with_state(server.clone())
            .merge(extra);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/mcp"), server)
    }

    pub(crate) async fn fixture() -> (Arc<AppState>, TestServer, String) {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::beta_session_routes::tests::test_app_state(&temp).await;
        let (url, server) = spawn_mcp_apps_server().await;
        let conn = state.db.connect().unwrap();
        for (id, user, name_id, enabled) in [
            ("conn-1", "user-1", "dash-server", 1),
            ("conn-off", "user-1", "off-server", 0),
        ] {
            conn.execute(
                "INSERT INTO mcp_connectors (id, user_id, name, name_id, url, enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, user, format!("Name {id}"), name_id, url, enabled],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, tokens, is_authenticated)
             VALUES ('s1', 'conn-1', 'st-1', ?1, 1)",
            params![json!({ "access_token": TOKEN, "token_type": "Bearer" }).to_string()],
        )
        .unwrap();
        (state, server, url)
    }

    fn bridge(connector: &str, action: &str, params: Value) -> BridgeBody {
        BridgeBody { action: action.into(), connector_id: connector.into(), params: Some(params) }
    }

    #[tokio::test]
    async fn bridge_lists_calls_and_reads_over_streamable_http() {
        let (state, server, _) = fixture().await;
        let run = |b: BridgeBody| {
            let state = state.clone();
            async move { run_bridge(&state, "user-1", &b, true).await }
        };

        // tools/list: `_meta.ui` arrives intact
        let tools = run(bridge("conn-1", "tools/list", json!({}))).await.unwrap();
        assert_eq!(tools["tools"].as_array().unwrap().len(), 4);
        assert_eq!(tools["tools"][0]["_meta"]["ui"]["resourceUri"], "ui://dash/app");
        assert_eq!(tools["tools"][1]["_meta"]["ui"]["visibility"], json!(["app"]));

        // tools/call (answered as SSE, notification skipped) keeps structuredContent + _meta
        let called = run(bridge("conn-1", "tools/call", json!({ "name": "show_dashboard", "arguments": { "range": "7d" } })))
            .await
            .unwrap();
        assert_eq!(called["structuredContent"]["rows"], json!([1, 2, 3]));
        assert_eq!(called["_meta"]["trace"], "t-1");

        // an app-only tool is callable from the app
        let refreshed = run(bridge("conn-1", "tools/call", json!({ "name": "refresh_data" }))).await.unwrap();
        assert_eq!(refreshed["structuredContent"]["tool"], "refresh_data");

        // resources/read returns the html with `_meta.ui` (csp, permissions) intact
        let read = run(bridge("conn-1", "resources/read", json!({ "uri": "ui://dash/app" }))).await.unwrap();
        assert_eq!(read["contents"][0]["text"], APP_HTML);
        assert_eq!(read["contents"][0]["mimeType"], APP_MIME);
        assert_eq!(read["contents"][0]["_meta"]["ui"]["csp"]["connectDomains"][0], "https://api.dash.example");
        assert_eq!(read["contents"][0]["_meta"]["ui"]["permissions"]["clipboardWrite"], json!({}));

        assert!(run(bridge("conn-1", "resources/list", json!({}))).await.unwrap()["resources"].is_array());
        assert!(run(bridge("conn-1", "resources/templates/list", json!({}))).await.unwrap()["resourceTemplates"].is_array());

        // wire-level: the host advertised the extension, sent its own token, echoed session + version
        let log = server.log.lock().unwrap().clone();
        assert!(log.iter().all(|s| s.auth.as_deref() == Some("Bearer tok-abc-123")));
        assert!(log.iter().all(|s| s.accept.as_deref() == Some("application/json, text/event-stream")));
        let init = log.iter().find(|s| s.method == "initialize").unwrap();
        assert_eq!(init.params["capabilities"]["extensions"][EXTENSION_ID]["mimeTypes"][0], APP_MIME);
        assert_eq!(init.session, None);
        for s in log.iter().filter(|s| s.method != "initialize") {
            assert_eq!(s.session.as_deref(), Some("sess-dash"), "{s:?}");
            assert_eq!(s.version.as_deref(), Some("2025-06-18"), "{s:?}");
        }
        assert!(*server.deleted.lock().unwrap(), "sessions are closed after each bridge call");
    }

    #[tokio::test]
    async fn bridge_enforces_visibility_ownership_and_the_action_allowlist() {
        let (state, server, _) = fixture().await;

        // model-only tool: not callable from an app; never reaches the server
        let err = run_bridge(&state, "user-1", &bridge("conn-1", "tools/call", json!({ "name": "model_only" })), true)
            .await
            .unwrap_err();
        assert_eq!((err.status, err.code), (StatusCode::FORBIDDEN, "tool_not_visible_to_app"));
        assert!(!server.called_tools().contains(&"model_only".to_string()));

        // unknown tool: cannot verify visibility, so it is refused
        let err = run_bridge(&state, "user-1", &bridge("conn-1", "tools/call", json!({ "name": "nope" })), true)
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert!(server.called_tools().is_empty());

        // prompts (and anything else) never leave the host
        let before = server.count();
        for action in ["prompts/list", "prompts/get", "sampling/createMessage"] {
            let err = run_bridge(&state, "user-1", &bridge("conn-1", action, json!({})), true).await.unwrap_err();
            assert_eq!((err.status, err.code), (StatusCode::FORBIDDEN, "action_not_allowed"), "{action}");
        }

        // another user's connector, a disabled connector, and a missing one all look the same
        for (user, connector) in [("user-2", "conn-1"), ("user-1", "conn-off"), ("user-1", "does-not-exist")] {
            let err = run_bridge(&state, user, &bridge(connector, "tools/list", json!({})), true).await.unwrap_err();
            assert_eq!((err.status, err.code), (StatusCode::NOT_FOUND, "connector_not_found"), "{user}/{connector}");
        }
        assert_eq!(server.count(), before, "none of those reached the connector");
    }

    #[tokio::test]
    async fn bridge_refuses_private_connector_hosts_by_default() {
        let (state, server, _) = fixture().await;
        let err = run_bridge(&state, "user-1", &bridge("conn-1", "tools/list", json!({})), false).await.unwrap_err();
        assert_eq!((err.status, err.code), (StatusCode::FORBIDDEN, "connector_url_forbidden"));
        assert_eq!(server.count(), 0);
    }

    fn tool_part(server: &str, tool: &str, with_meta: bool) -> Value {
        let mut state = json!({
            "status": "completed",
            "input": { "range": "7d" },
            "output": "ran it"
        });
        if with_meta {
            state["metadata"] = json!({ "mcp": {
                "server": server, "tool": tool,
                "result": {
                    "content": [{ "type": "text", "text": "ran it" }],
                    "structuredContent": { "rows": [1, 2, 3] },
                    "_meta": { "trace": "t-1" }
                }
            }});
        }
        json!({ "type": "tool", "callID": "call-9", "sessionID": "s", "tool": format!("mcp__{server}__{tool}"), "state": state })
    }

    #[tokio::test]
    async fn completed_ui_tool_emits_an_mcp_app_frame() {
        let (state, server, _) = fixture().await;
        let frame = app_frame_for_tool_part_with(&state, "user-1", "msg_1", &tool_part("dash-server", "show_dashboard", true), true)
            .await
            .expect("frame");

        assert_eq!(frame["type"], "mcp_app");
        assert_eq!(frame["messageId"], "msg_1");
        assert_eq!(frame["toolCallId"], "call-9");
        assert_eq!(frame["toolName"], "show_dashboard");
        assert_eq!(frame["connectorId"], "conn-1");
        assert_eq!(frame["connectorName"], "Name conn-1");
        assert_eq!(frame["title"], "Dashboard");
        assert_eq!(frame["resourceUri"], "ui://dash/app");
        assert_eq!(frame["html"], APP_HTML);
        assert_eq!(frame["allow"], "clipboard-write");
        assert_eq!(frame["prefersBorder"], false);
        assert_eq!(frame["csp"]["connectDomains"][0], "https://api.dash.example");
        assert_eq!(frame["csp"]["resourceDomains"][0], "https://cdn.dash.example");
        assert_eq!(frame["permissions"]["clipboardWrite"], json!({}));
        assert_eq!(frame["toolInput"]["range"], "7d");
        assert_eq!(frame["toolResult"]["structuredContent"]["rows"], json!([1, 2, 3]));
        assert_eq!(frame["toolResult"]["_meta"]["trace"], "t-1");
        assert_eq!(frame["tool"]["name"], "show_dashboard");
        assert_eq!(frame["tool"]["_meta"]["ui"]["resourceUri"], "ui://dash/app");
        // the emission path reads the tool list and the resource, but never re-runs the tool
        assert_eq!(server.called_tools(), Vec::<String>::new());
        let methods = server.methods();
        assert!(methods.contains(&"tools/list".to_string()) && methods.contains(&"resources/read".to_string()));
        // and it advertises the extension like every other host path
        let init = server.log.lock().unwrap().iter().find(|s| s.method == "initialize").cloned().unwrap();
        assert_eq!(init.params["capabilities"]["extensions"][EXTENSION_ID]["mimeTypes"][0], APP_MIME);
    }

    #[tokio::test]
    async fn emission_is_skipped_unless_a_users_ui_tool_completed() {
        let (state, server, _) = fixture().await;
        let emit = |user: &'static str, part: Value| {
            let state = state.clone();
            async move { app_frame_for_tool_part_with(&state, user, "m", &part, true).await }
        };

        // a tool gizzi did not tag as an MCP tool
        let untagged = json!({ "type": "tool", "callID": "c", "tool": "bash", "state": { "status": "completed" } });
        assert!(emit("user-1", untagged).await.is_none());
        assert_eq!(server.count(), 0, "no connector traffic for non-MCP tools");

        // an MCP tool with no ui resource, and one that is not on the connector
        assert!(emit("user-1", tool_part("dash-server", "plain", true)).await.is_none());
        assert!(emit("user-1", tool_part("dash-server", "ghost", true)).await.is_none());
        // a server name that is not one of the user's connectors
        assert!(emit("user-1", tool_part("other-server", "show_dashboard", true)).await.is_none());
        // someone else's turn never resolves this user's connector
        let before = server.count();
        assert!(emit("user-2", tool_part("dash-server", "show_dashboard", true)).await.is_none());
        assert_eq!(server.count(), before);
        // a disabled connector
        assert!(emit("user-1", tool_part("off-server", "show_dashboard", true)).await.is_none());
    }

    #[tokio::test]
    async fn emission_falls_back_to_the_model_visible_output() {
        let (state, _server, _) = fixture().await;
        let frame = app_frame_for_tool_part_with(&state, "user-1", "m", &tool_part("dash-server", "show_dashboard", false), true).await;
        // untagged parts are not app candidates at all
        assert!(frame.is_none());

        let mut part = tool_part("dash-server", "show_dashboard", true);
        part["state"]["metadata"]["mcp"].as_object_mut().unwrap().remove("result");
        let frame = app_frame_for_tool_part_with(&state, "user-1", "m", &part, true).await.unwrap();
        assert_eq!(frame["toolResult"]["content"][0]["text"], "ran it");
    }

    async fn call(app: Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        (status, headers, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    fn post_json(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn routes_answer_in_the_shapes_the_web_client_expects() {
        let (state, _server, _) = fixture().await;
        let app = mcp_apps_router().with_state(state).layer(Extension(user("user-2")));

        // another user's connector: the web client throws with `error`
        let (status, _, body) = call(
            app.clone(),
            post_json("/mcp/apps", json!({ "action": "tools/list", "connectorId": "conn-1" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(serde_json::from_slice::<Value>(&body).unwrap()["error"].is_string());

        // prompts/list is refused even though the web type includes it
        let (status, _, body) = call(
            app.clone(),
            post_json("/mcp/apps", json!({ "action": "prompts/list", "connectorId": "conn-1" })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(serde_json::from_slice::<Value>(&body).unwrap()["error"].is_string());

        // sandbox availability probe
        let (status, _, _) = call(
            app.clone(),
            Request::builder().uri("/mcp/sandbox").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // sandbox document for the SandboxConfig the client posts
        let (status, headers, body) = call(
            app.clone(),
            post_json(
                "/mcp/sandbox",
                json!({
                    "html": APP_HTML,
                    "csp": { "connectDomains": ["https://api.dash.example"] },
                    "permissions": { "camera": {} },
                    "allow": "camera; microphone",
                    "toolCallId": "call-9",
                    "connectorId": "conn-1"
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(headers["content-type"].to_str().unwrap().starts_with("text/html"));
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        let page = String::from_utf8(body).unwrap();
        assert!(page.contains("call-9") && page.contains("mcp-sandbox-ready"));
        assert!(page.contains("connect-src https://api.dash.example"));
        // the browser-supplied `allow` string is not trusted; permissions decide
        assert!(page.contains("\"allow\":\"camera\""), "{page}");
        assert!(!page.contains("microphone"));

        let (status, _, _) = call(app, post_json("/mcp/sandbox", json!({ "html": "  " }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn bridge_route_proxies_for_the_owner() {
        let (state, _server, _) = fixture().await;
        // route handlers use the env-driven guard; the fixture server is on loopback
        std::env::set_var(ALLOW_PRIVATE_ENV, "1");
        let app = mcp_apps_router().with_state(state).layer(Extension(user("user-1")));
        let (status, _, body) = call(
            app,
            post_json("/mcp/apps", json!({ "action": "resources/read", "connectorId": "conn-1", "params": { "uri": "ui://dash/app" } })),
        )
        .await;
        std::env::remove_var(ALLOW_PRIVATE_ENV);
        assert_eq!(status, StatusCode::OK);
        let result: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(result["contents"][0]["text"], APP_HTML);
    }
}
