//! Allternit-owned connector standard — catalog + connection engine.
//!
//! Reuses the existing in-process design catalog (`assets/open-design/connectors.json`)
//! and layers owned auth metadata (`assets/connectors.meta.json`) on top. The
//! curated 3 (github/notion/slack — the explicit meta entries) connect/execute
//! entirely in-process: local_cli (e.g. `gh`), api_key, oauth2 PKCE/loopback,
//! device_flow, with tokens sealed by `token_crypto.rs` in `connector_connections`
//! (V16, backend='rust_native'). Every other catalog id is served by the
//! vendored open-connector sidecar (`services/open-connector`) through
//! `crate::open_connector_proxy`: the sidecar holds the real credential in its
//! own encrypted SQLite and Rust keeps only an index row (backend='open_connector',
//! V17, tokens NULL). Connection state for both backends lives in
//! `connector_connections`, scoped by (connector_id, user_id). A third
//! backend, `allternit_native`, marks the Allternit Mail connector: its row is
//! index-only like the sidecar rows, but the credential is the per-agent
//! mailflare key sealed in `agent_identity_channels`, and its MCP tools
//! (`allternit_mail.*`) are served in-process on the internal MCP endpoint.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::auth::get_user;
use crate::AppState;

const CATALOG_JSON: &str = include_str!("../assets/open-design/connectors.json");
const META_JSON: &str = include_str!("../assets/connectors.meta.json");

fn catalog() -> &'static Value {
    static C: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON).unwrap_or_else(|_| json!({"connectors": []}))
    })
}

fn meta() -> &'static Value {
    static M: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    M.get_or_init(|| serde_json::from_str(META_JSON).unwrap_or_else(|_| json!({})))
}

/// Owned auth mapping for a connector: explicit `assets/connectors.meta.json`
/// entry if present, otherwise a synthesized Allternit-owned OAuth2 mapping.
/// Synthesis NEVER invents a provider base_url or OAuth endpoints — those only
/// exist when an explicit meta entry (or a future curated mapping) supplies them.
/// What synthesis guarantees for all 181: a concrete owned auth method + the exact
/// one-click-config knob (`ALLTERNIT_<ID>_CLIENT_ID`), so Connect never answers
/// "not supported"; it answers "register the Allternit OAuth app once (this env)".
fn synthesize_meta(id: &str) -> Value {
    let env = format!(
        "ALLTERNIT_{}_CLIENT_ID",
        id.to_uppercase().replace('-', "_")
    );
    json!({
        "auth_type": "oauth2",
        "tier": 2,
        "mcp_backed": false,
        "connectable": true,
        "synthesized": true,
        "oauth2": { "pkce": false },
        "setup_env": env
    })
}

fn meta_for(id: &str) -> Value {
    meta()
        .get(id)
        .cloned()
        .unwrap_or_else(|| synthesize_meta(id))
}

fn catalog_connectors() -> Vec<Value> {
    catalog()
        .get("connectors")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default()
}

fn find_catalog(id: &str) -> Option<Value> {
    catalog_connectors()
        .into_iter()
        .find(|c| c.get("id").and_then(|i| i.as_str()) == Some(id))
}

pub fn connector_router() -> Router<Arc<AppState>> {
    // NOTE: `/connectors/oauth/callback` is intentionally NOT mounted here.
    // An OAuth provider's browser redirect cannot carry a Clerk JWT, so the
    // callback lives in the public router (`connector_public_router`, mounted
    // unauthenticated in main.rs) alongside the sidecar's `/oauth/callback`.
    Router::new()
        .route("/connectors", get(list_connectors))
        .route("/connectors/setup-status", get(connectors_setup_status))
        .route("/connectors/mcp", post(mcp_proxy))
        .route("/connectors/:id", get(get_connector))
        .route("/connectors/:id/connect", post(connect_connector))
        .route("/connectors/:id/refresh", post(refresh_connector))
        .route("/connectors/:id/poll", post(poll_device))
        .route("/connectors/:id/execute", post(execute_connector))
        .route("/connectors/:id/disconnect", delete(disconnect_connector))
}

/// Unified MCP surface: one endpoint (`/api/v1/connectors/mcp`, Clerk-
/// authenticated, same as every other route in this router) exposing every
/// sidecar-backed connector to any MCP-capable agent host, instead of each
/// consumer having to know Allternit's bespoke REST shapes. Proxies straight
/// to the sidecar's own `/mcp` (stateless JSON-RPC — see
/// `open_connector_proxy::proxy_mcp`); the curated 3 (github/notion/slack)
/// are NOT reachable through this surface, only through the REST routes
/// above — they never went through the sidecar to begin with.
async fn mcp_proxy(headers: axum::http::HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
    proxy_mcp_response(caller(&headers), body).await
}

/// Prefix of cloud-issued runtime-device tokens, mirroring
/// `DEVICE_TOKEN_PREFIX` in allternit-cloud-api's runtime_pairing.rs and what
/// gizzi-code's pairing service sends (`Authorization: Bearer
/// allternit_runtime_…`).
pub(crate) const DEVICE_TOKEN_PREFIX: &str = "allternit_runtime_";

const DEVICE_TOKEN_CACHE_TTL: Duration = Duration::from_secs(300);

struct CachedDeviceUser {
    user_id: String,
    email: Option<String>,
    valid_until: Instant,
}

pub(crate) struct DeviceIdentity {
    pub user_id: String,
    pub email: Option<String>,
}

fn device_token_cache() -> &'static Mutex<HashMap<String, CachedDeviceUser>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedDeviceUser>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_device_identity(token: &str) -> Option<DeviceIdentity> {
    let cache = device_token_cache().lock().ok()?;
    let entry = cache.get(token)?;
    (Instant::now() < entry.valid_until).then(|| DeviceIdentity {
        user_id: entry.user_id.clone(),
        email: entry.email.clone(),
    })
}

fn remember_device_identity(token: &str, identity: DeviceIdentity) {
    if let Ok(mut cache) = device_token_cache().lock() {
        let now = Instant::now();
        cache.retain(|_, entry| now < entry.valid_until);
        cache.insert(
            token.to_string(),
            CachedDeviceUser {
                user_id: identity.user_id,
                email: identity.email,
                valid_until: now + DEVICE_TOKEN_CACHE_TTL,
            },
        );
    }
}

pub(crate) fn device_token_from_headers(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.starts_with(DEVICE_TOKEN_PREFIX))
}

/// Verify a cloud-issued runtime-device token by introspecting it against
/// allternit-cloud-api (`POST /api/v1/runtime-devices/verify-token`) — the
/// `runtime_devices` registry lives in that service's database, so a network
/// introspection call is the honest minimal path. Returns the device owner's
/// user_id; the token-derived identity always wins over any caller-asserted
/// `x-allternit-user-id` header. Fails closed: 401 on rejection, 502 when
/// the cloud-api itself is unreachable or answers garbage.
pub(crate) async fn verify_runtime_device_token(
    state: &AppState,
    token: &str,
) -> Result<DeviceIdentity, axum::response::Response> {
    // Fail closed: without a configured cloud-api URL there is no way to
    // verify a device token, so it cannot authenticate anything.
    if let Some(identity) = cached_device_identity(token) {
        return Ok(identity);
    }

    let Some(base) = state.config.cloud_api_url() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "unauthorized",
                "message": "runtime-device tokens are not verifiable: no cloud-api URL configured",
            })),
        )
            .into_response());
    };
    let url = format!(
        "{}/api/v1/runtime-devices/verify-token",
        base.trim_end_matches('/')
    );
    let resp = match reqwest::Client::new()
        .post(&url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            if let Some(identity) = cached_device_identity(token) {
                return Ok(identity);
            }
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "cloud_api_unavailable",
                    "message": format!("cloud-api token introspection unavailable: {}", e),
                })),
            )
                .into_response());
        }
    };

    if !resp.status().is_success() {
        // Rate-limit / cloud 5xx is not "this runtime is revoked". Reuse a
        // recent successful verify so chat does not 401-loop.
        if resp.status() == StatusCode::TOO_MANY_REQUESTS || resp.status().is_server_error() {
            if let Some(identity) = cached_device_identity(token) {
                return Ok(identity);
            }
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "cloud_api_unavailable",
                    "message": format!("cloud-api token introspection {}", resp.status()),
                })),
            )
                .into_response());
        }
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response());
    }
    let body = resp.json::<Value>().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "cloud_api_unavailable",
                "message": format!("cloud-api introspection decode failed: {}", e),
            })),
        )
            .into_response()
    })?;
    let user_id = body
        .get("userId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "cloud_api_unavailable",
                    "message": "cloud-api introspection response missing userId",
                })),
            )
                .into_response()
        })?;
    let email = body
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && s.contains('@'))
        .map(str::to_string);
    let identity = DeviceIdentity { user_id, email };
    remember_device_identity(token, DeviceIdentity {
        user_id: identity.user_id.clone(),
        email: identity.email.clone(),
    });
    Ok(identity)
}

/// Headless variant of the MCP proxy for peer services with no Clerk session —
/// the local gizzi daemon's MCP client during an agent run. Mounted on the
/// public router via `internal_routes` (`/internal/connectors/mcp`). Two
/// credentials are accepted, checked in this order:
///
/// 1. A cloud-issued runtime-device token (`Authorization: Bearer
///    allternit_runtime_…`), introspected against allternit-cloud-api. The
///    alias user is the device's registered owner — the token-derived
///    identity wins over any caller-asserted header.
/// 2. The internal service token (`internal_auth::require_internal_token`),
///    the same peer-service trust model the ACU gateway uses for
///    `/internal/*`; the user is named explicitly via `x-allternit-user-id`,
///    which is only meaningful because the internal token already
///    authenticated the *service*.
///
/// Anything else is 401 — there is no silent admin bypass.
///
/// The `allternit_mail.*` tools are answered in-process (they are not sidecar
/// tools): `tools/list` merges them into the sidecar's list, and
/// `tools/call` dispatches to `agent_email_routes::call_mail_mcp_tool` with
/// the user_id resolved above.
pub async fn mcp_proxy_internal(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let user_id = if let Some(token) = device_token_from_headers(&headers) {
        let token = token.to_string();
        match verify_runtime_device_token(&state, &token).await {
            Ok(identity) => identity.user_id,
            Err(resp) => return resp,
        }
    } else {
        if let Err(status) = crate::internal_auth::require_internal_token(&headers, &state) {
            return (status, Json(json!({"error": "unauthorized"}))).into_response();
        }
        let user_id = headers
            .get("x-allternit-user-id")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        match user_id {
            Some(user_id) => user_id,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "x-allternit-user-id header is required"})),
                )
                    .into_response()
            }
        }
    };

    // Allternit Mail is served in-process, not by the sidecar: intercept its
    // tools here (ownership is enforced against the authenticated user_id
    // inside the tool handlers, same as the REST surface).
    if let Some(resp) = handle_allternit_mail_mcp(&state, &user_id, &body).await {
        return resp;
    }
    proxy_mcp_response(user_id, body).await
}

/// Serve the `allternit_mail.*` tools on the internal MCP endpoint. Returns
/// `None` (caller should proxy to the sidecar) for anything that isn't a
/// `tools/list` or an `allternit_mail.*` `tools/call`.
async fn handle_allternit_mail_mcp(
    state: &Arc<AppState>,
    user_id: &str,
    body: &Value,
) -> Option<axum::response::Response> {
    let method = body.get("method").and_then(|m| m.as_str())?;
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let rpc_ok = |result: Value| {
        (
            StatusCode::OK,
            Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
        )
            .into_response()
    };
    // MCP tool results carry payloads as text content; handler errors map to
    // isError:true (JSON-RPC 200), so MCP clients surface them as tool errors.
    let rpc_tool = |out: Result<Value, (StatusCode, Json<Value>)>| match out {
        Ok(payload) => rpc_ok(json!({
            "content": [{ "type": "text", "text": payload.to_string() }],
            "isError": false,
        })),
        Err((_, Json(e))) => rpc_ok(json!({
            "content": [{ "type": "text", "text": e.to_string() }],
            "isError": true,
        })),
    };

    match method {
        "tools/list" => {
            // Merge with the sidecar's own tools when reachable; the
            // allternit_mail tools must stay listed even when it is down.
            let mut tools = match crate::open_connector_proxy::proxy_mcp(user_id, body.clone()).await
            {
                Ok(resp) => resp
                    .get("result")
                    .and_then(|r| r.get("tools"))
                    .and_then(|t| t.as_array())
                    .cloned()
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            if let Some(own) = crate::agent_email_routes::mail_mcp_tools().as_array() {
                tools.extend(own.iter().cloned());
            }
            Some(rpc_ok(json!({ "tools": tools })))
        }
        "tools/call" => {
            let name = body
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            if !name.starts_with("allternit_mail.") {
                return None;
            }
            let args = body
                .get("params")
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            Some(rpc_tool(
                crate::agent_email_routes::call_mail_mcp_tool(state, user_id, name, args).await,
            ))
        }
        _ => None,
    }
}

/// Shared core of both MCP proxy entry points: forward the JSON-RPC body to
/// the sidecar with the per-user `x-oo-connector-alias` header.
async fn proxy_mcp_response(user_id: String, body: Value) -> axum::response::Response {
    match crate::open_connector_proxy::proxy_mcp(&user_id, body).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) if e.unreachable => (
            StatusCode::BAD_GATEWAY,
            Json(
                json!({ "error": "sidecar_unavailable", "message": "Connector sidecar unavailable — the open-connector process is down or still starting." }),
            ),
        )
            .into_response(),
        Err(e) => (
            StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(json!({ "error": "mcp_proxy_failed", "message": e.message })),
        )
            .into_response(),
    }
}

/// Public connector routes — mounted OUTSIDE the Clerk-protected router in
/// main.rs. Both are OAuth provider redirect targets: the browser arrives
/// straight from the provider's consent screen with no Allternit session.
/// `/api/v1/connectors/oauth/callback` is the rust-native (curated 3)
/// loopback; `/oauth/callback` proxies to the open-connector sidecar, whose
/// OAuth client configs use `{ALLTERNIT origin}/oauth/callback` as redirect
/// URI (OOMOL_CONNECT_ORIGIN is set to this API's origin by dev-stack-watch).
pub fn connector_public_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/connectors/oauth/callback", get(oauth_callback))
        .route("/oauth/callback", get(sidecar_oauth_callback))
}

/// Sidecar OAuth completion: the provider redirected to this API's origin;
/// forward the query string to the sidecar's own `/oauth/callback` (which
/// knows the pending state) and pipe its HTML completion page back verbatim.
/// On success the sidecar holds the credential; the next `list_connectors`
/// lazily upgrades this user's index row to 'connected' via the live
/// connection view, so no DB write is needed (or possible — this request is
/// unauthenticated) here.
async fn sidecar_oauth_callback(
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> impl IntoResponse {
    match crate::open_connector_proxy::proxy_oauth_callback(&query.unwrap_or_default()).await {
        Ok((status, html)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
            Html(html),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Html(format!(
                "<html><body><h2>Connector sidecar unavailable</h2><p>{}</p></body></html>",
                e.message
            )),
        )
            .into_response(),
    }
}

fn caller(headers: &axum::http::HeaderMap) -> String {
    get_user(headers)
        .map(|u| u.user_id)
        .unwrap_or_else(|| "local-dev".to_string())
}

/// True for the curated 3 (github/notion/slack) — ids with an explicit entry
/// in `assets/connectors.meta.json`. Those keep the rust-native path; every
/// other catalog id is served by the open-connector sidecar.
fn is_curated(id: &str) -> bool {
    meta().get(id).is_some()
}

/// Catalog id of Allternit's own agent-email connector. It lives in
/// `connectors.meta.json` (so `is_curated` is true and the catalog merge never
/// consults the sidecar for it), but connect/disconnect are special-cased:
/// the credential is a per-AGENT mailflare key sealed in
/// `agent_identity_channels`, and the `connector_connections` row is an
/// index-only marker (backend='allternit_native', tokens NULL).
pub(crate) const ALLTERNIT_MAIL_ID: &str = "allternit-mail";

/// Connect handler for the Allternit Mail connector. Connections are per-user
/// but mailboxes are per-agent, so:
/// - With `agent_id` in the body: verify ownership, provision (idempotently)
///   the agent's mailflare mailbox via the same logic as
///   `POST /agents/:id/identity/email`, and upsert the index-only
///   connector_connections row (no tokens — the key stays sealed in
///   agent_identity_channels).
/// - Without `agent_id`: report the rail status so the UI can render a setup
///   hint when the mailflare env is missing.
async fn connect_allternit_mail(
    state: &Arc<AppState>,
    user_id: &str,
    body: ConnectBody,
) -> (StatusCode, Json<Value>) {
    let Some(agent_id) = body.agent_id.filter(|s| !s.trim().is_empty()) else {
        let rail = crate::agent_email_routes::agent_email_status_value().await;
        let configured = rail
            .get("configured")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        return (
            StatusCode::OK,
            Json(json!({
                "status": if configured { "available" } else { "unconfigured" },
                "connector": ALLTERNIT_MAIL_ID,
                "backend": "allternit_native",
                "owned": true,
                "rail": rail,
                "setup_hint": if configured {
                    "Pass {\"agent_id\":\"...\"} to provision (or adopt) the agent's mailbox and mark this connector connected."
                } else {
                    "Allternit Mail is not configured on this deployment: set ALLTERNIT_MAILFLARE_URL, ALLTERNIT_MAILFLARE_ADMIN_KEY and ALLTERNIT_BOT_EMAIL_DOMAIN, then connect with {\"agent_id\":\"...\"}."
                },
            })),
        );
    };

    if let Err((status, Json(e))) =
        crate::agent_email_routes::require_agent_owner_id(state, user_id, &agent_id)
    {
        return (status, Json(e));
    }
    let Some(client) = crate::mailflare_client::MailflareClient::from_env() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": "mailflare_not_configured",
                "id": ALLTERNIT_MAIL_ID,
                "message": "ALLTERNIT_MAILFLARE_URL/ALLTERNIT_MAILFLARE_ADMIN_KEY are not configured.",
            })),
        );
    };
    let address = match crate::allternit_bus_routes::provision_email_mailflare(
        state, user_id, &agent_id, client,
    )
    .await
    {
        Ok(address) => address,
        Err((status, Json(e))) => return (status, Json(e)),
    };

    // Index-only row: backend='allternit_native', tokens NULL. account carries
    // the provisioned address; metadata ties the row to the agent so later
    // connects for OTHER agents of the same user remain visible.
    let metadata = json!({ "agent_id": agent_id }).to_string();
    let res = {
        let db = state.db.clone();
        let uid = user_id.to_string();
        let acc = address.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.connect().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account, metadata, backend)
                 VALUES (?1, ?2, ?3, 'allternit_native', 'connected', ?4, ?5, 'allternit_native')
                 ON CONFLICT(connector_id, user_id) DO UPDATE SET
                   auth_type='allternit_native', status='connected', account=excluded.account,
                   metadata=excluded.metadata, backend='allternit_native',
                   access_token=NULL, refresh_token=NULL, updated_at=CURRENT_TIMESTAMP",
                params![uuid::Uuid::new_v4().to_string(), ALLTERNIT_MAIL_ID, uid, acc, metadata],
            )
            .map_err(|e| e.to_string())?;
            Ok::<_, String>(())
        })
        .await
        .map_err(|e| format!("db task: {e}"))
    };
    match res {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({
                "status": "connected",
                "connector": ALLTERNIT_MAIL_ID,
                "backend": "allternit_native",
                "owned": true,
                "agent_id": agent_id,
                "address": address,
                "account": address,
            })),
        ),
        Ok(Err(e)) | Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

/// True when the sidecar rejected an OAuth start because no OAuth client is
/// registered for the provider (`oauth_client_config_required` — the sidecar
/// flattens to HTTP 400; the proxy embeds the code in its message string).
fn sidecar_oauth_app_missing(e: &crate::open_connector_proxy::ProxyError) -> bool {
    !e.unreachable && e.message.contains("oauth_client_config_required")
}

/// Structured answer for `sidecar_oauth_app_missing`, mirroring the
/// rust-native `oauth_registration_required` shape: tell the caller exactly
/// which one-time admin step unblocks Connect.
fn oauth_app_not_configured(id: &str) -> (StatusCode, Json<Value>) {
    let service = crate::open_connector_proxy::sidecar_id(id);
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "oauth_app_not_configured",
            "id": id,
            "message": format!("No OAuth client is registered for '{service}' on the connector sidecar, so an authorization URL cannot be issued yet."),
            "setup_hint": format!("One-time admin step: register the OAuth app for '{service}' via the sidecar admin endpoint PUT /api/oauth/configs/{service} (see services/open-connector docs; GET /api/oauth/configs lists what is already configured), then retry Connect."),
        })),
    )
}


/// Snapshot of sidecar state for one catalog render: the slim provider map
/// (`None` = sidecar unreachable — degrade gracefully) and THIS user's live
/// connections (service -> account label), alias-filtered inside the proxy.
struct SidecarView {
    providers: Option<
        std::sync::Arc<
            std::collections::HashMap<String, crate::open_connector_proxy::ProviderSummary>,
        >,
    >,
    connections: std::collections::HashMap<String, String>,
}

async fn sidecar_view(user_id: &str) -> SidecarView {
    use crate::open_connector_proxy as sidecar;
    match sidecar::provider_summaries().await {
        Ok(map) => {
            let conns = sidecar::list_user_connections(user_id)
                .await
                .unwrap_or_default();
            SidecarView {
                providers: Some(map),
                connections: conns.into_iter().collect(),
            }
        }
        Err(_) => SidecarView {
            providers: None,
            connections: std::collections::HashMap::new(),
        },
    }
}

/// Detect whether a `local_cli` connector's backing CLI is installed and
/// authenticated on THIS machine. Generic: driven entirely by
/// `meta.local_cli.{cmd,token_args,account_args}`, so gh/gcloud/git/... all
/// light up the same way. Never returns the token itself — only presence.
fn detect_local_cli(m: &Value) -> Value {
    let lc = m.get("local_cli").cloned().unwrap_or_else(|| json!({}));
    let cmd = lc
        .get("cmd")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let token_args: Vec<String> = lc
        .get("token_args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let account_args: Vec<String> = lc
        .get("account_args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if cmd.is_empty() {
        return json!({ "installed": false, "authed": false, "account": "", "cmd": "" });
    }
    match std::process::Command::new(&cmd).args(&token_args).output() {
        Err(_) => json!({ "installed": false, "authed": false, "account": "", "cmd": cmd }),
        Ok(o) if !o.status.success() => json!({
            "installed": true, "authed": false, "account": "", "cmd": cmd,
            "hint": format!("`{}` is installed but not authenticated; run its login once", cmd),
        }),
        Ok(o) => {
            let authed = !String::from_utf8_lossy(&o.stdout).trim().is_empty();
            let mut account = String::new();
            if authed && !account_args.is_empty() {
                if let Ok(a) = std::process::Command::new(&cmd)
                    .args(&account_args)
                    .output()
                {
                    if a.status.success() {
                        account = String::from_utf8_lossy(&a.stdout).trim().to_string();
                    }
                }
            }
            json!({ "installed": true, "authed": authed, "account": account, "cmd": cmd })
        }
    }
}

/// Merge catalog entry + owned meta + live connection status into one view.
/// Curated 3 (github/notion/slack): the rust-native meta path below, untouched.
/// Everything else: sidecar-sourced truth via `merge_sidecar` — never the old
/// synthesized always-`connectable:true` stub.
/// All of one user's `connector_connections` rows in a single query, keyed by
/// `connector_id`. `db.connect()` opens a fresh SQLite connection every call
/// (no pooling — see `db.rs`), so doing this once per request instead of once
/// per connector is the difference between ~1,100 connection opens and one,
/// once the catalog covers the full sidecar provider set (`sidecar_only_entries`)
/// rather than just the legacy 181.
fn fetch_connection_rows(
    db: &crate::db::DbHandle,
    user_id: &str,
) -> std::collections::HashMap<String, (String, String)> {
    db.connect()
        .ok()
        .and_then(|conn| {
            let mut stmt = conn
                .prepare("SELECT connector_id, status, COALESCE(account,'') FROM connector_connections WHERE user_id=?1")
                .ok()?;
            let rows = stmt
                .query_map(params![user_id], |row| {
                    Ok((row.get::<_, String>(0)?, (row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
                })
                .ok()?
                .filter_map(|r| r.ok())
                .collect();
            Some(rows)
        })
        .unwrap_or_default()
}

fn merge(
    c: &Value,
    user_id: &str,
    db: &crate::db::DbHandle,
    sv: &SidecarView,
    rows: &std::collections::HashMap<String, (String, String)>,
) -> Value {
    let id = c.get("id").and_then(|i| i.as_str()).unwrap_or("");
    if !is_curated(id) {
        return merge_sidecar(c, user_id, db, sv, rows);
    }
    let m = meta_for(id);
    let auth_type = m
        .get("auth_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let tier = m.get("tier").and_then(|v| v.as_i64()).unwrap_or(2);
    let mcp_backed = m
        .get("mcp_backed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let connectable = m
        .get("connectable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let base_url = m.get("base_url").and_then(|v| v.as_str());
    let synthesized = m
        .get("synthesized")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let setup_env = m
        .get("setup_env")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let tools_mapped = m
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    let executable = base_url.is_some() && tools_mapped;

    let (conn_status, account) = rows
        .get(id)
        .cloned()
        .unwrap_or_else(|| ("disconnected".to_string(), String::new()));

    let mut out = c.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("provider".to_string(), json!("allternit"));
        obj.insert(
            "auth".to_string(),
            json!({ "type": auth_type, "owned": true, "synthesized": synthesized }),
        );
        obj.insert("auth_type".to_string(), json!(auth_type));
        obj.insert("tier".to_string(), json!(tier));
        obj.insert("mcp_backed".to_string(), json!(mcp_backed));
        obj.insert("connectable".to_string(), json!(connectable));
        obj.insert("executable".to_string(), json!(executable));
        if let Some(b) = base_url {
            obj.insert("base_url".to_string(), json!(b));
        }
        if synthesized {
            if let Some(env) = setup_env {
                obj.insert(
                    "setup".to_string(),
                    json!({
                        "kind": "owned_oauth_app",
                        "set_env": env,
                        "one_click": true,
                        "message": "One-time Allternit OAuth app for this provider; set the env above (or use the Connect button) — no third-party key.",
                    }),
                );
            }
        }
        if auth_type == "local_cli" {
            obj.insert("availability".to_string(), detect_local_cli(&m));
        }
        obj.insert(
            "connection".to_string(),
            json!({ "status": conn_status, "account": account }),
        );
    }
    out
}

/// Sidecar-backed merge: real `connectable`/`executable`/`auth_type` from the
/// sidecar's own catalog, and live connection status from the sidecar's
/// connection list (falling back to the index row). Degrades to an honest
/// `connectable:false` with a clear message when the sidecar is down or lacks
/// the provider — no fake stub, and curated connectors are unaffected.
fn merge_sidecar(
    c: &Value,
    user_id: &str,
    db: &crate::db::DbHandle,
    sv: &SidecarView,
    rows: &std::collections::HashMap<String, (String, String)>,
) -> Value {
    let id = c.get("id").and_then(|i| i.as_str()).unwrap_or("");

    let (db_status, db_account) = rows
        .get(id)
        .cloned()
        .unwrap_or_else(|| ("disconnected".to_string(), String::new()));

    let mut out = c.clone();
    let mut provider_auth_type = String::from("unknown");
    if let Some(obj) = out.as_object_mut() {
        obj.insert("provider".to_string(), json!("allternit"));
        obj.insert("tier".to_string(), json!(3));
        obj.insert("mcp_backed".to_string(), json!(false));
        obj.insert("backend".to_string(), json!("open_connector"));
        match sv.providers.as_ref().and_then(|m| m.get(id)) {
            Some(p) => {
                provider_auth_type = p
                    .auth_types
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                obj.insert(
                    "auth".to_string(),
                    json!({ "type": provider_auth_type, "owned": true, "synthesized": false }),
                );
                obj.insert("auth_type".to_string(), json!(provider_auth_type));
                obj.insert("connectable".to_string(), json!(true));
                obj.insert("executable".to_string(), json!(p.executable_actions > 0));
                // Same field name the curated 3 already populate from
                // connectors.meta.json — the frontend derives a real logo
                // from this domain via a favicon service either way.
                if let Some(homepage) = &p.homepage_url {
                    obj.insert("base_url".to_string(), json!(homepage));
                }
            }
            None if sv.providers.is_none() => {
                obj.insert(
                    "auth".to_string(),
                    json!({ "type": "unknown", "owned": true, "synthesized": false }),
                );
                obj.insert("auth_type".to_string(), json!("unknown"));
                obj.insert("connectable".to_string(), json!(false));
                obj.insert("executable".to_string(), json!(false));
                obj.insert("setup".to_string(), json!({
                    "kind": "sidecar_unavailable",
                    "one_click": false,
                    "message": "Connector sidecar unavailable — the open-connector process is down or still starting. github/notion/slack are unaffected.",
                }));
            }
            None => {
                obj.insert(
                    "auth".to_string(),
                    json!({ "type": "unknown", "owned": true, "synthesized": false }),
                );
                obj.insert("auth_type".to_string(), json!("unknown"));
                obj.insert("connectable".to_string(), json!(false));
                obj.insert("executable".to_string(), json!(false));
                obj.insert("setup".to_string(), json!({
                    "kind": "not_in_sidecar_catalog",
                    "one_click": false,
                    "message": "This catalog entry has no matching provider in the vendored open-connector catalog yet.",
                }));
            }
        }
        // Live sidecar status wins over the index row; lazily sync the row so
        // `refresh`/`disconnect`/`execute` see it too.
        if let Some(account) = sv.connections.get(id) {
            if db_status != "connected" {
                mark_sidecar_connected(db, id, user_id, &provider_auth_type, account);
            }
            obj.insert(
                "connection".to_string(),
                json!({ "status": "connected", "account": account }),
            );
        } else {
            obj.insert(
                "connection".to_string(),
                json!({ "status": db_status, "account": db_account }),
            );
        }
    }
    out
}

/// Lazily sync the index row when the sidecar reports a live connection the DB
/// doesn't know about yet (e.g. OAuth completed in the popup while this row
/// was still 'pending'). Index-only: tokens stay NULL, backend='open_connector'.
fn mark_sidecar_connected(
    db: &crate::db::DbHandle,
    id: &str,
    user_id: &str,
    auth_type: &str,
    account: &str,
) {
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account, backend)
             VALUES (?1, ?2, ?3, ?4, 'connected', ?5, 'open_connector')
             ON CONFLICT(connector_id, user_id) DO UPDATE SET
               status='connected', account=excluded.account, backend='open_connector',
               access_token=NULL, refresh_token=NULL, updated_at=CURRENT_TIMESTAMP",
            params![uuid::Uuid::new_v4().to_string(), id, user_id, auth_type, account],
        );
    }
}

/// Build a minimal catalog-shaped entry for a sidecar provider that has no
/// entry at all in Allternit's legacy 181-connector catalog. Fed through
/// `merge_sidecar` exactly like a real catalog entry — that function only
/// ever reads `id` from its input and otherwise sources everything from the
/// sidecar, so a synthesized base is indistinguishable downstream.
fn synthesize_sidecar_catalog_entry(
    id: &str,
    p: &crate::open_connector_proxy::ProviderSummary,
) -> Value {
    json!({
        "id": id,
        "name": p.display_name,
        "category": "Open Connector",
        "description": format!("{} — via the vendored open-connector catalog (not in Allternit's original 181-entry list).", p.display_name),
        "status": "available",
    })
}

/// Every sidecar provider not already present in the legacy 181-entry catalog
/// (by id, after aliasing — `provider_summaries()` keys are already resolved
/// to the Allternit spelling when an alias exists). This is what closes the
/// gap between "Allternit's own catalog has 181 entries" and "open-connector
/// actually has 1,000+ providers": without this, only providers Allternit's
/// legacy catalog already happened to list could ever show up, no matter how
/// many the sidecar supports.
fn sidecar_only_entries(
    sv: &SidecarView,
    legacy_ids: &std::collections::HashSet<&str>,
) -> Vec<Value> {
    sv.providers
        .as_ref()
        .map(|providers| {
            providers
                .iter()
                .filter(|(id, _)| !legacy_ids.contains(id.as_str()))
                .map(|(id, p)| synthesize_sidecar_catalog_entry(id, p))
                .collect()
        })
        .unwrap_or_default()
}

async fn list_connectors(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let user_id = caller(&headers);
    let db = state.db.clone();
    let sv = sidecar_view(&user_id).await;
    let sidecar_ok = sv.providers.is_some();
    let rows = fetch_connection_rows(&db, &user_id);
    let legacy = catalog_connectors();
    let legacy_ids: std::collections::HashSet<&str> = legacy
        .iter()
        .filter_map(|c| c.get("id").and_then(|i| i.as_str()))
        .collect();
    let mut list: Vec<Value> = legacy
        .iter()
        .map(|c| merge(c, &user_id, &db, &sv, &rows))
        .collect();
    for extra in sidecar_only_entries(&sv, &legacy_ids) {
        list.push(merge_sidecar(&extra, &user_id, &db, &sv, &rows));
    }
    let total = list.len();
    Json(json!({
        "connectors": list,
        "total": total,
        "source": "allternit-owned",
        "standard": "mcp+owned-oauth+openapi",
        "sidecar": if sidecar_ok { "ok" } else { "unavailable" },
    }))
}

/// Aggregate setup status for the three first-party connectors. This is
/// deployment-level state (not per-user) so the UI can show a single
/// "Finish setup" banner when any connector is not yet provisioned.
async fn connectors_setup_status(
    State(_state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let _user_id = caller(&headers);

    let mail_status = crate::agent_email_routes::agent_email_status_value().await;
    let mail_configured = mail_status
        .get("configured")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mail_reachable = mail_status
        .get("reachable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mail_domain = mail_status
        .get("domain")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let sidecar_healthy = crate::open_connector_proxy::is_reachable().await;
    let sidecar_url = crate::open_connector_proxy::sidecar_url();

    let configured_oauth: std::collections::HashSet<String> =
        match crate::open_connector_proxy::configured_oauth_services().await {
            Ok(services) => services.into_iter().collect(),
            Err(_) => std::collections::HashSet::new(),
        };

    let oauth_check = |service: &str| -> Value {
        let configured = configured_oauth.contains(service);
        json!({
            "configured": configured,
            "setup_hint": if configured {
                Value::Null
            } else {
                json!(format!("Run ./scripts/install-connectors.sh to register the OAuth app for {service}."))
            }
        })
    };

    let all_ready = mail_configured && mail_reachable && sidecar_healthy
        && configured_oauth.contains("gmail")
        && configured_oauth.contains("googledrive");

    Json(json!({
        "ready": all_ready,
        "checks": {
            "allternit_mail": {
                "configured": mail_configured,
                "reachable": mail_reachable,
                "domain": mail_domain,
                "setup_hint": if mail_configured { Value::Null } else {
                    json!("Run ./scripts/install-connectors.sh to deploy Allternit Mail to your Cloudflare account.")
                }
            },
            "sidecar": {
                "healthy": sidecar_healthy,
                "url": sidecar_url,
                "setup_hint": if sidecar_healthy { Value::Null } else {
                    json!("Start the open-connector sidecar with ./dev/scripts/start-connector-sidecar.sh, or run ./scripts/install-connectors.sh.")
                }
            },
            "gmail": oauth_check("gmail"),
            "google_drive": oauth_check("googledrive"),
        }
    }))
}

async fn get_connector(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let user_id = caller(&headers);
    let rows = fetch_connection_rows(&state.db, &user_id);
    match find_catalog(&id) {
        Some(c) => {
            let sv = sidecar_view(&user_id).await;
            (
                StatusCode::OK,
                Json(merge(&c, &user_id, &state.db, &sv, &rows)),
            )
        }
        None => {
            // Not in the legacy 181-entry catalog — check whether it's one of
            // the ~950 sidecar-only providers before giving up.
            let sv = sidecar_view(&user_id).await;
            match sv.providers.as_ref().and_then(|m| m.get(&id)) {
                Some(p) => {
                    let base = synthesize_sidecar_catalog_entry(&id, p);
                    (
                        StatusCode::OK,
                        Json(merge_sidecar(&base, &user_id, &state.db, &sv, &rows)),
                    )
                }
                None => (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "connector_not_found", "id": id })),
                ),
            }
        }
    }
}

#[derive(Deserialize)]
struct ConnectBody {
    via: Option<String>,
    api_key: Option<String>,
    values: Option<Value>,
    /// `allternit-mail` only: the agent to provision a mailbox for.
    agent_id: Option<String>,
}

async fn connect_connector(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<ConnectBody>>,
) -> impl IntoResponse {
    // Legacy 181-entry catalog first; falls through to a bare `{id}` stub for
    // the ~950 sidecar-only providers (never in that catalog by design — see
    // `sidecar_only_entries`). `connect_sidecar` verifies real existence via
    // `sidecar::get_provider` and 404s honestly if the id is wrong.
    let c = find_catalog(&id).unwrap_or_else(|| json!({ "id": id }));
    let m = meta_for(&id);
    let user_id = caller(&headers);
    let body = body.map(|Json(b)| b).unwrap_or(ConnectBody {
        via: None,
        api_key: None,
        values: None,
        agent_id: None,
    });
    if id == ALLTERNIT_MAIL_ID {
        return connect_allternit_mail(&state, &user_id, body).await;
    }
    if !is_curated(&id) {
        return connect_sidecar(&state, &user_id, &id, &c, body).await;
    }
    let via = body.via.unwrap_or_else(|| {
        m.get("auth_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });

    match via.as_str() {
        "local_cli" => connect_local_cli(&state, &user_id, &id, &c, &m).await,
        "api_key" => {
            connect_api_key(
                &state,
                &user_id,
                &id,
                &c,
                body.api_key.as_deref(),
                body.values.as_ref(),
            )
            .await
        }
        "oauth2" => connect_oauth2(&state, &user_id, &id, &c, &m).await,
        "device_flow" => connect_device(&state, &user_id, &id, &c, &m).await,
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported_auth_type", "via": other })),
        ),
    }
}

/// Connect a non-curated connector via the open-connector sidecar. The secret
/// lands only in the sidecar's encrypted store; Rust keeps an index row
/// (backend='open_connector', tokens NULL). Response shapes match exactly what
/// `owned-connector.ts` already expects, so the frontend needs no changes.
async fn connect_sidecar(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
    c: &Value,
    body: ConnectBody,
) -> (StatusCode, Json<Value>) {
    use crate::open_connector_proxy as sidecar;
    let provider = match sidecar::get_provider(id).await {
        Ok(p) => p,
        Err(e) if e.unreachable => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(
                    json!({ "error": "sidecar_unavailable", "id": id, "message": "Connector sidecar unavailable — the open-connector process is down or still starting." }),
                ),
            );
        }
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "connector_not_found", "id": id, "message": e.message })),
            );
        }
    };
    let auth_types: Vec<String> = provider
        .get("authTypes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let via = body
        .via
        .clone()
        .unwrap_or_else(|| auth_types.first().cloned().unwrap_or_default());

    match via.as_str() {
        "oauth2" => {
            let auth = match sidecar::start_oauth(id, user_id).await {
                Ok(a) => a,
                Err(e) if e.unreachable => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(
                            json!({ "error": "sidecar_unavailable", "id": id, "message": "Connector sidecar unavailable — the open-connector process is down or still starting." }),
                        ),
                    );
                }
                Err(e) if sidecar_oauth_app_missing(&e) => {
                    return oauth_app_not_configured(id);
                }
                Err(e) => {
                    return (
                        StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY),
                        Json(
                            json!({ "error": "sidecar_oauth_failed", "id": id, "message": e.message }),
                        ),
                    );
                }
            };
            let authorize_url = auth
                .get("authorizationUrl")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if authorize_url.is_empty() {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(
                        json!({ "error": "sidecar_bad_response", "id": id, "message": "Sidecar returned no authorizationUrl — is an OAuth client configured for this provider?" }),
                    ),
                );
            }
            if let Err(e) =
                persist_sidecar_row(&state.db, user_id, id, "oauth2", "pending", "").await
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e })),
                );
            }
            (
                StatusCode::OK,
                Json(json!({
                    "status": "authorization_required",
                    "auth_type": "oauth2",
                    "owned": true,
                    "connector": c.get("id"),
                    "authorize_url": authorize_url,
                    "backend": "open_connector",
                    "instructions": "Open authorize_url, approve Allternit; the provider redirects back to the Allternit callback, which forwards to the connector sidecar to finish.",
                })),
            )
        }
        "api_key" => {
            let key = body
                .api_key
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if key.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "api_key_required", "id": id })),
                );
            }
            let resp = sidecar::upsert_credential(
                id,
                user_id,
                json!({ "authType": "api_key", "values": { "apiKey": key } }),
            )
            .await;
            finish_sidecar_connect(state, user_id, id, c, "api_key", resp).await
        }
        "no_auth" => {
            let resp =
                sidecar::upsert_credential(id, user_id, json!({ "authType": "no_auth" })).await;
            finish_sidecar_connect(state, user_id, id, c, "no_auth", resp).await
        }
        "custom_credential" => {
            let values = body
                .values
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            if values.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(
                        json!({ "error": "custom_credential_requires_values", "id": id, "message": "This provider needs structured credentials. Pass them in `values`." }),
                    ),
                );
            }
            let resp = sidecar::upsert_credential(
                id,
                user_id,
                json!({ "authType": "custom_credential", "values": values }),
            )
            .await;
            finish_sidecar_connect(state, user_id, id, c, "custom_credential", resp).await
        }
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported_auth_type", "via": other })),
        ),
    }
}

/// Shared tail for the non-OAuth sidecar connect paths: on success, upsert the
/// index row (status='connected', tokens NULL, backend='open_connector') and
/// return the exact `{status:"connected", ...}` shape the frontend expects.
async fn finish_sidecar_connect(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
    c: &Value,
    auth_type: &str,
    resp: Result<Value, crate::open_connector_proxy::ProxyError>,
) -> (StatusCode, Json<Value>) {
    let r = match resp {
        Ok(r) => r,
        Err(e) if e.unreachable => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(
                    json!({ "error": "sidecar_unavailable", "id": id, "message": "Connector sidecar unavailable — the open-connector process is down or still starting." }),
                ),
            );
        }
        Err(e) => {
            return (
                StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY),
                Json(json!({ "error": "sidecar_connect_failed", "id": id, "message": e.message })),
            );
        }
    };
    let account = r
        .get("profile")
        .and_then(|p| p.get("displayName"))
        .and_then(|v| v.as_str())
        .or_else(|| r.get("alias").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    match persist_sidecar_row(&state.db, user_id, id, auth_type, "connected", &account).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "connected",
                "connector": c.get("id"),
                "auth_type": auth_type,
                "owned": true,
                "account": account,
                "backend": "open_connector",
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}

/// Upsert the sidecar-backed index row. Index-only: access_token/refresh_token
/// are always NULL — the sidecar's own encrypted SQLite is the token vault.
async fn persist_sidecar_row(
    db: &crate::db::DbHandle,
    user_id: &str,
    id: &str,
    auth_type: &str,
    status: &str,
    account: &str,
) -> Result<(), String> {
    let db = db.clone();
    let uid = user_id.to_string();
    let id_s = id.to_string();
    let at = auth_type.to_string();
    let st = status.to_string();
    let acc = account.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account, backend)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open_connector')
             ON CONFLICT(connector_id, user_id) DO UPDATE SET
               auth_type=excluded.auth_type, status=excluded.status, account=excluded.account,
               backend='open_connector', access_token=NULL, refresh_token=NULL, metadata=NULL,
               updated_at=CURRENT_TIMESTAMP",
            params![uuid::Uuid::new_v4().to_string(), id_s, uid, at, st, acc],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| format!("db task: {}", e))?
}

/// Refresh/reconnect an existing connection using how it was originally authed:
/// local_cli re-derives + re-seals the token (e.g. re-run `gh auth token`);
/// oauth2 / device_flow re-open the owned authorization; api_key must re-POST
/// /connect with the key. Never stores plaintext.
async fn refresh_connector(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // See `connect_connector` for why this falls back to a bare stub instead
    // of 404ing for sidecar-only ids.
    let c = find_catalog(&id).unwrap_or_else(|| json!({ "id": id }));
    let m = meta_for(&id);
    let user_id = caller(&headers);

    let db = state.db.clone();
    let id_s = id.to_string();
    let uid = user_id.clone();
    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT auth_type, status FROM connector_connections WHERE connector_id=?1 AND user_id=?2",
            params![id_s, uid],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
    })
    .await;
    let (stored_auth, _stored_status) = match row {
        Ok(Ok(v)) => v,
        _ => {
            return (
                StatusCode::CONFLICT,
                Json(
                    json!({ "error": "not_connected", "id": id, "message": "Nothing to refresh — connect this connector first." }),
                ),
            )
        }
    };

    if !is_curated(&id) {
        // Sidecar-backed: oauth2 re-opens the sidecar authorization; api_key
        // rotation is a fresh /connect, same as the rust-native rule.
        return match stored_auth.as_str() {
            "api_key" => (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({ "error": "api_key_refresh_via_connect", "id": id, "message": "Re-POST /connect with {\"api_key\":\"...\"} to rotate an api_key connector." }),
                ),
            ),
            other => {
                connect_sidecar(
                    &state,
                    &user_id,
                    &id,
                    &c,
                    ConnectBody {
                        via: Some(other.to_string()),
                        api_key: None,
                        values: None,
                        agent_id: None,
                    },
                )
                .await
            }
        };
    }

    match stored_auth.as_str() {
        "local_cli" => connect_local_cli(&state, &user_id, &id, &c, &m).await,
        "oauth2" => connect_oauth2(&state, &user_id, &id, &c, &m).await,
        "device_flow" => connect_device(&state, &user_id, &id, &c, &m).await,
        "api_key" => (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "api_key_refresh_via_connect", "id": id, "message": "Re-POST /connect with {\"api_key\":\"...\"} to rotate an api_key connector." }),
            ),
        ),
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unsupported_auth_type", "via": other })),
        ),
    }
}

async fn connect_local_cli(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
    c: &Value,
    m: &Value,
) -> (StatusCode, Json<Value>) {
    let lc = m.get("local_cli").cloned().unwrap_or_else(|| json!({}));
    let cmd = lc
        .get("cmd")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let token_args: Vec<String> = lc
        .get("token_args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let account_args: Vec<String> = lc
        .get("account_args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    if cmd.is_empty() || token_args.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "local_cli_not_configured", "id": id })),
        );
    }

    // Run `<cmd> <token_args>` (e.g. `gh auth token`).
    let token_out = tokio::process::Command::new(&cmd)
        .args(&token_args)
        .output()
        .await;
    let token = match token_out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "local_cli_not_authenticated",
                    "id": id,
                    "message": format!("`{}` is not authenticated. Run the login flow once (e.g. `gh auth login`).", cmd),
                    "stderr": stderr,
                })),
            );
        }
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "local_cli_missing",
                    "id": id,
                    "message": format!("`{}` not found on this machine: {}. Install it and authenticate once.", cmd, e),
                })),
            );
        }
    };

    if token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "local_cli_empty_token", "id": id })),
        );
    }

    // Best-effort account label (e.g. `gh api user --jq .login`).
    let mut account = String::new();
    if !account_args.is_empty() {
        if let Ok(o) = tokio::process::Command::new(&cmd)
            .args(&account_args)
            .output()
            .await
        {
            if o.status.success() {
                account = String::from_utf8_lossy(&o.stdout).trim().to_string();
            }
        }
    }

    let db = state.db.clone();
    let cid = uuid::Uuid::new_v4().to_string();
    let id_s = id.to_string();
    let uid = user_id.to_string();
    let tok = crate::token_crypto::seal(&token);
    let acc = account.clone();
    let scopes = m
        .get("scopes")
        .cloned()
        .unwrap_or_else(|| json!([]))
        .to_string();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, access_token, account, scopes)
             VALUES (?1, ?2, ?3, 'local_cli', 'connected', ?4, ?5, ?6)
             ON CONFLICT(connector_id, user_id) DO UPDATE SET
               auth_type='local_cli', status='connected', access_token=excluded.access_token,
               account=excluded.account, scopes=excluded.scopes, updated_at=CURRENT_TIMESTAMP",
            params![cid, id_s, uid, tok, acc, scopes],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match res {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({
                "status": "connected",
                "connector": c.get("id"),
                "auth_type": "local_cli",
                "owned": true,
                "account": account,
            })),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("db task: {}", e) })),
        ),
    }
}

async fn connect_api_key(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
    c: &Value,
    api_key: Option<&str>,
    values: Option<&Value>,
) -> (StatusCode, Json<Value>) {
    let key = if let Some(k) = api_key.map(str::trim).filter(|s| !s.is_empty()) {
        k.to_string()
    } else if let Some(v) = values {
        if let Some(s) = v.as_str() {
            s.trim().to_string()
        } else if v.is_object() && !v.as_object().unwrap().is_empty() {
            if let Some(ak) = v.get("apiKey").or_else(|| v.get("api_key")).and_then(|s| s.as_str()) {
                ak.trim().to_string()
            } else {
                v.to_string()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    if key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "api_key_required", "id": id })),
        );
    }
    let db = state.db.clone();
    let cid = uuid::Uuid::new_v4().to_string();
    let id_s = id.to_string();
    let uid = user_id.to_string();
    let key = crate::token_crypto::seal(&key);
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, access_token)
             VALUES (?1, ?2, ?3, 'api_key', 'connected', ?4)
             ON CONFLICT(connector_id, user_id) DO UPDATE SET
               auth_type='api_key', status='connected', access_token=excluded.access_token, updated_at=CURRENT_TIMESTAMP",
            params![cid, id_s, uid, key],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;
    match res {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(
                json!({ "status": "connected", "connector": c.get("id"), "auth_type": "api_key", "owned": true }),
            ),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("db task: {}", e) })),
        ),
    }
}

#[derive(Deserialize)]
struct ExecuteBody {
    tool: Option<String>,
    input: Option<Value>,
    path: Option<String>,
    method: Option<String>,
}

/// Synthesized connector-MCP: execute any tool declared in `meta.tools[]` against
/// `meta.base_url` — no per-connector Rust code. Supports `{path}` templating from
/// input, GET/DELETE query pass-through, and JSON body for POST/PUT/PATCH. Optional
/// `query` / `body` arrays in a tool entry restrict which input keys are forwarded;
/// otherwise GET forwards all leftover scalar inputs as query and writes send all
/// leftover input as the JSON body.
fn generic_dispatch(
    meta: &Value,
    tool: &str,
    input: &Value,
    base_url: &str,
) -> Option<(String, String, Option<Value>)> {
    if tool.is_empty() {
        return None;
    }
    let tools = meta.get("tools").and_then(|v| v.as_array())?;
    let t = tools
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(tool))?;
    let method = t
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_uppercase();
    let mut path = t
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if path.is_empty() {
        return None;
    }

    // Collect {path} template tokens.
    let mut tokens: Vec<String> = Vec::new();
    let mut rest = path.as_str();
    while let Some(start) = rest.find('{') {
        if let Some(end) = rest[start..].find('}') {
            tokens.push(rest[start + 1..start + end].to_string());
            rest = &rest[start + end + 1..];
        } else {
            break;
        }
    }

    let obj = input.as_object();
    let mut used: Vec<String> = Vec::new();
    for name in &tokens {
        if let Some(val) = obj.and_then(|o| o.get(name)).and_then(|v| v.as_str()) {
            path = path.replace(&format!("{{{}}}", name), &urlencoding(val));
            used.push(name.clone());
        }
    }

    let query_keys: Option<Vec<String>> = t.get("query").and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect()
    });
    let body_keys: Option<Vec<String>> = t.get("body").and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect()
    });

    let mut url = format!("{}{}", base_url, path);
    let mut req_body: Option<Value> = None;

    if method == "GET" || method == "DELETE" {
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let Some(o) = obj {
            for (k, v) in o.iter() {
                if used.contains(k) {
                    continue;
                }
                if let Some(ref qk) = query_keys {
                    if !qk.contains(k) {
                        continue;
                    }
                }
                if let Some(s) = v.as_str() {
                    pairs.push((k.clone(), s.to_string()));
                } else if let Some(n) = v.as_i64() {
                    pairs.push((k.clone(), n.to_string()));
                } else if let Some(b) = v.as_bool() {
                    pairs.push((k.clone(), b.to_string()));
                }
            }
        }
        if !pairs.is_empty() {
            let qs: String = pairs
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding(k), urlencoding(v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{}?{}", url, qs);
        }
    } else {
        let mut bmap = serde_json::Map::new();
        if let Some(o) = obj {
            for (k, v) in o.iter() {
                if used.contains(k) {
                    continue;
                }
                if let Some(ref bk) = body_keys {
                    if !bk.contains(k) {
                        continue;
                    }
                }
                bmap.insert(k.clone(), v.clone());
            }
        }
        req_body = Some(Value::Object(bmap));
    }
    Some((method, url, req_body))
}

async fn execute_connector(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ExecuteBody>,
) -> impl IntoResponse {
    // See `connect_connector` for why this falls back to a bare stub instead
    // of 404ing for sidecar-only ids.
    let c = find_catalog(&id).unwrap_or_else(|| json!({ "id": id }));
    let m = meta_for(&id);
    let base_url = m
        .get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let user_id = caller(&headers);

    if !is_curated(&id) {
        return execute_sidecar(&user_id, &id, &c, body).await;
    }

    // Load active connection + token.
    let db = state.db.clone();
    let id_s = id.to_string();
    let uid = user_id.clone();
    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT status, COALESCE(access_token,'') FROM connector_connections WHERE connector_id=?1 AND user_id=?2",
            params![id_s, uid],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
    })
    .await;

    let (status, token) = match row {
        Ok(Ok(v)) => v,
        _ => {
            return (
                StatusCode::CONFLICT,
                Json(
                    json!({ "error": "not_connected", "id": id, "message": "Connect this connector first, then execute." }),
                ),
            )
        }
    };
    let token = crate::token_crypto::open(&token);
    if status != "connected" || token.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "not_connected", "id": id })),
        );
    }

    let client = reqwest::Client::new();
    let tool = body.tool.clone().unwrap_or_default();
    let input = body.input.clone().unwrap_or_else(|| json!({}));

    // Dispatch order: hand-written (github) -> meta tools[] (generic, long-tail) -> raw path passthrough.
    // Triple = (method, url, optional_json_body).
    let built: Option<(String, String, Option<Value>)> = if id == "github" {
        match tool.as_str() {
            "github_search_repositories" => {
                let q = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
                Some((
                    "GET".to_string(),
                    format!(
                        "{}/search/repositories?q={}&per_page=5",
                        base_url,
                        urlencoding(q)
                    ),
                    None,
                ))
            }
            "github_get_issue" => {
                let owner = input.get("owner").and_then(|v| v.as_str()).unwrap_or("");
                let repo = input.get("repo").and_then(|v| v.as_str()).unwrap_or("");
                let number = input
                    .get("issue_number")
                    .and_then(|v| v.as_i64())
                    .map(|n| n.to_string())
                    .or_else(|| {
                        input
                            .get("issue_number")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                Some((
                    "GET".to_string(),
                    format!("{}/repos/{}/{}/issues/{}", base_url, owner, repo, number),
                    None,
                ))
            }
            _ => generic_dispatch(&m, &tool, &input, &base_url),
        }
    } else {
        generic_dispatch(&m, &tool, &input, &base_url)
    };
    let built = built.or_else(|| {
        body.path.clone().map(|p| {
            (
                body.method.clone().unwrap_or_else(|| "GET".to_string()),
                format!("{}{}", base_url, p),
                None,
            )
        })
    });

    let (method, url, req_body) = match built {
        Some(u) => u,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({ "error": "execute_needs_tool_or_path", "id": id, "message": "Provide a known 'tool' (hand-written or meta tools[]) or a generic 'path'." }),
                ),
            )
        }
    };

    let mut req = match method.to_uppercase().as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "PATCH" => client.patch(&url),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    };
    req = req
        .header("Authorization", format!("Bearer {}", token))
        .header("User-Agent", "Allternit-Connectors/1.0")
        .header("Accept", "application/vnd.github+json, application/json");
    if let Some(b) = req_body {
        req = req.json(&b);
    }

    let resp = req.send().await;
    match resp {
        Ok(r) => {
            let code = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body_json: Value = r.json().await.unwrap_or_else(|_| json!({}));
            (
                code,
                Json(
                    json!({ "status": "ok", "connector": c.get("id"), "tool": tool, "provider_status": code.as_u16(), "result": body_json }),
                ),
            )
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "provider_request_failed", "id": id, "message": e.to_string() })),
        ),
    }
}

/// Execute a non-curated connector through the sidecar: resolve the tool name
/// to a sidecar action id (`{service}.{name}`), then proxy `POST
/// /v1/actions/:actionId` with the per-user alias. Response is normalized to
/// the same `{status:"ok", result:...}` shape the curated path returns.
async fn execute_sidecar(
    user_id: &str,
    id: &str,
    c: &Value,
    body: ExecuteBody,
) -> (StatusCode, Json<Value>) {
    use crate::open_connector_proxy as sidecar;
    let tool = body.tool.clone().unwrap_or_default();
    if tool.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "error": "execute_needs_tool", "id": id, "message": "Sidecar-backed connectors execute by action id — pass a 'tool' from this connector's action list (GET /api/v1/connectors/:id)." }),
            ),
        );
    }
    let provider = match sidecar::get_provider(id).await {
        Ok(p) => p,
        Err(e) if e.unreachable => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(
                    json!({ "error": "sidecar_unavailable", "id": id, "message": "Connector sidecar unavailable — the open-connector process is down or still starting." }),
                ),
            );
        }
        Err(e) => {
            return (
                StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY),
                Json(
                    json!({ "error": "sidecar_provider_unavailable", "id": id, "message": e.message }),
                ),
            );
        }
    };
    let sidecar_service = sidecar::sidecar_id(id);
    let action_id = match resolve_action_id(id, &sidecar_service, &tool, &provider) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let input = body.input.clone().unwrap_or_else(|| json!({}));
    match sidecar::execute(&action_id, user_id, input).await {
        Ok(env) => {
            let data = env.get("data").cloned().unwrap_or_else(|| json!({}));
            let result = data.get("result").cloned().unwrap_or(data);
            (
                StatusCode::OK,
                Json(
                    json!({ "status": "ok", "connector": c.get("id"), "tool": tool, "provider_status": 200, "result": result, "backend": "open_connector" }),
                ),
            )
        }
        Err(e) if e.unreachable => (
            StatusCode::BAD_GATEWAY,
            Json(
                json!({ "error": "sidecar_unavailable", "id": id, "message": "Connector sidecar unavailable — the open-connector process is down or still starting." }),
            ),
        ),
        Err(e) => (
            StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(
                json!({ "error": "sidecar_action_failed", "id": id, "tool": tool, "message": e.message }),
            ),
        ),
    }
}

/// Map a caller-supplied tool name to a sidecar action id: exact action-id
/// match first, then action display name, then accept a qualified
/// `{service}.{name}` string using either the Allternit catalog id or the
/// sidecar provider id as prefix (rewriting to the sidecar id before calling
/// /v1/actions).
fn resolve_action_id(
    allternit_id: &str,
    sidecar_id: &str,
    tool: &str,
    provider: &Value,
) -> Result<String, (StatusCode, Json<Value>)> {
    let actions = provider.get("actions").and_then(|v| v.as_array());
    if let Some(actions) = actions {
        if actions
            .iter()
            .any(|a| a.get("id").and_then(|v| v.as_str()) == Some(tool))
        {
            return Ok(tool.to_string());
        }
        if let Some(a) = actions
            .iter()
            .find(|a| a.get("name").and_then(|v| v.as_str()) == Some(tool))
        {
            if let Some(aid) = a.get("id").and_then(|v| v.as_str()) {
                return Ok(aid.to_string());
            }
        }
    }
    // Accept prefixes in either spelling and normalize to the sidecar id.
    for prefix in [allternit_id, sidecar_id] {
        if let Some(rest) = tool.strip_prefix(&format!("{}.", prefix)) {
            return Ok(format!("{}.{}", sidecar_id, rest));
        }
    }
    let available: Vec<&str> = actions
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("id").and_then(|v| v.as_str()))
                .take(10)
                .collect()
        })
        .unwrap_or_default();
    Err((
        StatusCode::BAD_REQUEST,
        Json(
            json!({ "error": "unknown_action", "id": allternit_id, "tool": tool, "message": "Unknown tool for this provider. Pass one of available_actions (or its short name).", "available_actions": available }),
        ),
    ))
}

async fn disconnect_connector(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let user_id = caller(&headers);
    if id == ALLTERNIT_MAIL_ID {
        // Remove only the index row — never the agent's mailbox or its sealed
        // key (mailbox teardown is tied to agent deletion, not connector
        // toggles).
        let db = state.db.clone();
        let res = tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "DELETE FROM connector_connections WHERE connector_id=?1 AND user_id=?2",
                params![ALLTERNIT_MAIL_ID, user_id],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await;
        return match res {
            Ok(Ok(())) => (
                StatusCode::OK,
                Json(json!({ "status": "disconnected", "connector": id })),
            ),
            Ok(Err(e)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            ),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("db task: {}", e) })),
            ),
        };
    }
    if !is_curated(&id) {
        // Proxy the real disconnect to the sidecar first. If it is down, bail
        // so the user can retry — otherwise the sidecar would keep the secret
        // while our index row says disconnected. Other errors (e.g. the
        // connection is already gone) fall through to index cleanup.
        if let Err(e) = crate::open_connector_proxy::disconnect(&id, &user_id).await {
            if e.unreachable {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(
                        json!({ "error": "sidecar_unavailable", "id": id, "message": "Connector sidecar unavailable — credential NOT removed. Retry when it is back." }),
                    ),
                );
            }
        }
    }
    let db = state.db.clone();
    let id_s = id.clone();
    let res = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let n = conn.execute(
            "UPDATE connector_connections SET status='disconnected', access_token=NULL, refresh_token=NULL, updated_at=CURRENT_TIMESTAMP WHERE connector_id=?1 AND user_id=?2",
            params![id_s, user_id],
        )?;
        Ok::<_, rusqlite::Error>(n)
    })
    .await;
    match res {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(json!({ "status": "disconnected", "connector": id })),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("db task: {}", e) })),
        ),
    }
}

// ─── S2: owned OAuth2 (loopback) + device-flow engine ─────────────────────────

const CALLBACK_PATH: &str = "/api/v1/connectors/oauth/callback";

fn public_base() -> String {
    std::env::var("ALLTERNIT_PUBLIC_BASE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8013".to_string())
}
fn redirect_uri() -> String {
    format!("{}{}", public_base(), CALLBACK_PATH)
}
fn env_name(id: &str, suffix: &str) -> String {
    format!(
        "ALLTERNIT_{}_{}",
        id.to_uppercase().replace('-', "_"),
        suffix
    )
}
fn read_client(id: &str, m: &Value) -> (Option<String>, Option<String>) {
    let cid = std::env::var(env_name(id, "CLIENT_ID"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            m.get("oauth2")
                .and_then(|o| o.get("client_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            m.get("device")
                .and_then(|o| o.get("client_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
    let sec = std::env::var(env_name(id, "CLIENT_SECRET"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            m.get("oauth2")
                .and_then(|o| o.get("client_secret"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
    (cid, sec)
}
fn oauth_registration_required(id: &str) -> (StatusCode, Json<Value>) {
    let env = env_name(id, "CLIENT_ID");
    let sec_env = env_name(id, "CLIENT_SECRET");
    (
        StatusCode::OK,
        Json(json!({
            "status": "oauth_app_registration_required",
            "connector": id,
            "owned": true,
            "set_env": env,
            "message": format!("Owned OAuth needs a one-time Allternit OAuth app client id for '{}'. Set {} (and optionally {}). No third-party key. GitHub works today via local_cli (`gh`).", id, env, sec_env),
        })),
    )
}

fn scopes_of(m: &Value) -> Vec<String> {
    m.get("scopes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

async fn persist_pending(
    db: &crate::db::DbHandle,
    user_id: &str,
    id: &str,
    auth_type: &str,
    metadata: String,
) -> Result<(), String> {
    let db = db.clone();
    let uid = user_id.to_string();
    let id_s = id.to_string();
    let at = auth_type.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, metadata)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)
             ON CONFLICT(connector_id, user_id) DO UPDATE SET
               status='pending', auth_type=excluded.auth_type, metadata=excluded.metadata,
               access_token=NULL, refresh_token=NULL, updated_at=CURRENT_TIMESTAMP",
            params![uuid::Uuid::new_v4().to_string(), id_s, uid, at, metadata],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| format!("db task: {}", e))?
}

async fn connect_oauth2(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
    c: &Value,
    m: &Value,
) -> (StatusCode, Json<Value>) {
    let (client_id, client_secret) = read_client(id, m);
    let client_id = match client_id {
        Some(v) => v,
        None => return oauth_registration_required(id),
    };
    let oa = m.get("oauth2").cloned().unwrap_or_else(|| json!({}));
    let authorize = oa
        .get("authorize_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let token_url = oa
        .get("token_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if authorize.is_empty() || token_url.is_empty() {
        // Client id is present, but Allternit has not mapped this provider's OAuth
        // endpoints yet. Honest status — never fabricate an authorize URL.
        return (
            StatusCode::OK,
            Json(json!({
                "status": "owned_oauth_endpoint_mapping_needed",
                "connector": id,
                "owned": true,
                "message": format!("Allternit client id is set for '{}', but its OAuth authorize/token endpoints are not mapped in connectors.meta.json yet. Add oauth2.authorize_url + token_url to finish; execution stays gated until then.", id),
            })),
        );
    }
    let redirect = redirect_uri();
    let oauth_state = uuid::Uuid::new_v4().to_string();
    let meta_json = json!({ "state": oauth_state, "token_url": token_url, "redirect_uri": redirect, "client_id": client_id, "client_secret": client_secret }).to_string();
    if let Err(e) = persist_pending(&state.db, user_id, id, "oauth2", meta_json).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        );
    }
    let scopes = scopes_of(m);
    let mut url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&state={}",
        authorize,
        urlencoding(&client_id),
        urlencoding(&redirect),
        urlencoding(&oauth_state)
    );
    if !scopes.is_empty() {
        url.push_str(&format!("&scope={}", urlencoding(&scopes.join(" "))));
    }
    (
        StatusCode::OK,
        Json(json!({
            "status": "authorization_required",
            "auth_type": "oauth2",
            "owned": true,
            "connector": c.get("id"),
            "authorize_url": url,
            "redirect_uri": redirect,
            "state": oauth_state,
            "instructions": "Open authorize_url, approve Allternit; the provider redirects back to the Allternit loopback callback to finish. No third-party key.",
        })),
    )
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CallbackQuery>,
) -> Html<String> {
    if let Some(err) = q.error.clone() {
        let desc = q.error_description.clone().unwrap_or(err);
        return Html(format!(
            "<html><body><h2>Connector authorization failed</h2><p>{}</p></body></html>",
            desc
        ));
    }
    let code = match q.code.clone() {
        Some(c) => c,
        None => return Html("<html><body>Missing code.</body></html>".to_string()),
    };
    let st = match q.state.clone() {
        Some(s) => s,
        None => return Html("<html><body>Missing state.</body></html>".to_string()),
    };

    let db = state.db.clone();
    let st2 = st.clone();
    let found = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT connector_id, user_id, COALESCE(metadata,'{}') FROM connector_connections WHERE status='pending' AND json_extract(metadata,'$.state')=?1",
        )?;
        let row = stmt
            .query_row(params![st2], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?))
            })
            .ok();
        Ok::<_, rusqlite::Error>(row)
    })
    .await;

    let (connector_id, _user_id, metadata_s) = match found {
        Ok(Ok(Some(r))) => r,
        _ => {
            return Html(
                "<html><body>Pending authorization not found or expired.</body></html>".to_string(),
            )
        }
    };
    let md: Value = serde_json::from_str(&metadata_s).unwrap_or_else(|_| json!({}));
    let token_url = md
        .get("token_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let redirect = md
        .get("redirect_uri")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let client_id = md
        .get("client_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let client_secret = md
        .get("client_secret")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let client = reqwest::Client::new();
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", redirect),
        ("client_id", client_id.clone()),
    ];
    if let Some(ref sec) = client_secret {
        form.push(("client_secret", sec.clone()));
    }
    let resp = client
        .post(&token_url)
        .form(&form)
        .header("Accept", "application/json")
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            let tok: Value = r.json().await.unwrap_or_else(|_| json!({}));
            let access = tok
                .get("access_token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let refresh = tok
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let expires_at = tok
                .get("expires_in")
                .and_then(|v| v.as_i64())
                .map(|s| (chrono::Utc::now() + chrono::Duration::seconds(s)).to_rfc3339());
            if access.is_empty() {
                return Html(format!(
                    "<html><body>Token response missing access_token: {}</body></html>",
                    tok
                ));
            }
            let db2 = state.db.clone();
            let cid = connector_id.clone();
            let sealed_access = crate::token_crypto::seal(&access);
            let refr = refresh.as_deref().map(crate::token_crypto::seal);
            let exp = expires_at.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = db2.connect()?;
                conn.execute(
                    "UPDATE connector_connections SET status='connected', access_token=?1, refresh_token=?2, expires_at=?3, metadata=NULL, updated_at=CURRENT_TIMESTAMP WHERE connector_id=?4 AND status='pending'",
                    params![sealed_access, refr, exp, cid],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await;
            Html("<html><body><h2>Connector connected</h2><p>You can close this window and return to Allternit.</p><script>window.close();</script></body></html>".to_string())
        }
        Ok(r) => {
            let body = r.text().await.unwrap_or_default();
            Html(format!(
                "<html><body>Token exchange failed: {}</body></html>",
                body
            ))
        }
        Err(e) => Html(format!(
            "<html><body>Token request error: {}</body></html>",
            e
        )),
    }
}

async fn connect_device(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
    c: &Value,
    m: &Value,
) -> (StatusCode, Json<Value>) {
    let (client_id, _) = read_client(id, m);
    let client_id = match client_id {
        Some(v) => v,
        None => return oauth_registration_required(id),
    };
    let device_url = m
        .get("device")
        .and_then(|d| d.get("device_url"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            if id == "github" {
                Some("https://github.com/login/device/code")
            } else {
                None
            }
        })
        .unwrap_or("")
        .to_string();
    let token_url = m
        .get("device")
        .and_then(|d| d.get("token_url"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            if id == "github" {
                Some("https://github.com/login/oauth/access_token")
            } else {
                None
            }
        })
        .unwrap_or("")
        .to_string();
    let scopes = scopes_of(m);
    let client = reqwest::Client::new();
    let mut form: Vec<(&str, String)> = vec![("client_id", client_id.clone())];
    if !scopes.is_empty() {
        form.push(("scope", scopes.join(" ")));
    }
    let resp = client
        .post(&device_url)
        .form(&form)
        .header("Accept", "application/json")
        .header("User-Agent", "Allternit-Connectors/1.0")
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            let d: Value = r.json().await.unwrap_or_else(|_| json!({}));
            let device_code = d
                .get("device_code")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let user_code = d
                .get("user_code")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let verification_uri = d
                .get("verification_uri")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let expires_in = d.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(900);
            let interval = d.get("interval").and_then(|v| v.as_i64()).unwrap_or(5);
            if device_code.is_empty() {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": "device_flow_no_code", "provider": d })),
                );
            }
            let meta_json = json!({ "device_code": device_code, "token_url": token_url, "client_id": client_id, "interval": interval }).to_string();
            if let Err(e) = persist_pending(&state.db, user_id, id, "device_flow", meta_json).await
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e })),
                );
            }
            (
                StatusCode::OK,
                Json(json!({
                    "status": "device_authorization_required",
                    "auth_type": "device_flow",
                    "owned": true,
                    "connector": c.get("id"),
                    "user_code": user_code,
                    "verification_uri": verification_uri,
                    "expires_in": expires_in,
                    "interval": interval,
                    "instructions": "Enter user_code at verification_uri to approve Allternit, then call POST /api/v1/connectors/:id/poll to complete. No third-party key.",
                })),
            )
        }
        Ok(r) => {
            let status = r.status().as_u16();
            let body: Value = r.json().await.unwrap_or_else(|_| json!({}));
            (
                StatusCode::OK,
                Json(json!({
                    "status": "device_provider_reached",
                    "auth_type": "device_flow",
                    "owned": true,
                    "connector": c.get("id"),
                    "provider_status": status,
                    "provider": body,
                    "message": "Owned device flow reached the provider endpoint; it rejected the (unregistered/test) client id, which proves the wiring is live. Register the Allternit OAuth app and set its client id to complete.",
                })),
            )
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "device_request_failed", "message": e.to_string() })),
        ),
    }
}

async fn poll_device(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let user_id = caller(&headers);
    let db = state.db.clone();
    let id_s = id.clone();
    let uid = user_id.clone();
    let found = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let v = conn
            .query_row(
                "SELECT COALESCE(metadata,'{}') FROM connector_connections WHERE connector_id=?1 AND user_id=?2 AND status='pending'",
                params![id_s, uid],
                |r| r.get::<_, String>(0),
            )
            .ok();
        Ok::<_, rusqlite::Error>(v)
    })
    .await;

    let metadata_s = match found {
        Ok(Ok(Some(s))) => s,
        _ => {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "error": "no_pending_device_flow", "id": id })),
            )
        }
    };
    let md: Value = serde_json::from_str(&metadata_s).unwrap_or_else(|_| json!({}));
    let device_code = md
        .get("device_code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let token_url = md
        .get("token_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let client_id = md
        .get("client_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let client = reqwest::Client::new();
    let form: Vec<(&str, String)> = vec![
        ("client_id", client_id),
        ("device_code", device_code),
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        ),
    ];
    let resp = client
        .post(&token_url)
        .form(&form)
        .header("Accept", "application/json")
        .header("User-Agent", "Allternit-Connectors/1.0")
        .send()
        .await;
    match resp {
        Ok(r) => {
            let d: Value = r.json().await.unwrap_or_else(|_| json!({}));
            if let Some(err) = d.get("error").and_then(|v| v.as_str()) {
                let status = if err == "authorization_pending" || err == "slow_down" {
                    "pending"
                } else {
                    "error"
                };
                return (
                    StatusCode::OK,
                    Json(json!({ "status": status, "provider_error": err, "id": id })),
                );
            }
            let access = d
                .get("access_token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if access.is_empty() {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": "no_access_token", "provider": d })),
                );
            }
            let db2 = state.db.clone();
            let cid = id.clone();
            let acc = crate::token_crypto::seal(&access);
            let _ = tokio::task::spawn_blocking(move || {
                let conn = db2.connect()?;
                conn.execute(
                    "UPDATE connector_connections SET status='connected', access_token=?1, metadata=NULL, updated_at=CURRENT_TIMESTAMP WHERE connector_id=?2 AND status='pending'",
                    params![acc, cid],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await;
            (
                StatusCode::OK,
                Json(
                    json!({ "status": "connected", "connector": id, "auth_type": "device_flow", "owned": true }),
                ),
            )
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "device_poll_failed", "message": e.to_string() })),
        ),
    }
}

fn urlencoding(s: &str) -> String {
    // Minimal percent-encoding for query parameters (no extra crate).
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::path::Path;
    use tower::ServiceExt;

    #[test]
    fn catalog_contains_allternit_mail() {
        let entry = find_catalog(ALLTERNIT_MAIL_ID).expect("allternit-mail in catalog");
        assert_eq!(
            entry.get("name").and_then(|v| v.as_str()),
            Some("Allternit Mail")
        );
        assert_eq!(
            entry.get("category").and_then(|v| v.as_str()),
            Some("Email")
        );
        let tools: Vec<&str> = entry
            .get("allowedToolNames")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|t| t.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(tools, vec!["allternit_mail.send", "allternit_mail.status"]);

        // Meta entry makes it curated (catalog merge never consults the
        // sidecar for it) and marks the native backend.
        assert!(is_curated(ALLTERNIT_MAIL_ID));
        let m = meta_for(ALLTERNIT_MAIL_ID);
        assert_eq!(
            m.get("auth_type").and_then(|v| v.as_str()),
            Some("allternit_native")
        );
        assert_eq!(
            m.get("connectable").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn oauth_app_missing_detection() {
        let missing = crate::open_connector_proxy::ProxyError {
            status: 400,
            message: "open-connector sidecar returned 400: {\"error\":{\"code\":\"oauth_client_config_required\",\"message\":\"Configure an OAuth client for gmail first.\"}}"
                .to_string(),
            unreachable: false,
        };
        assert!(sidecar_oauth_app_missing(&missing));

        let other = crate::open_connector_proxy::ProxyError {
            status: 400,
            message: "open-connector sidecar returned 400: {\"error\":{\"code\":\"invalid_input\"}}"
                .to_string(),
            unreachable: false,
        };
        assert!(!sidecar_oauth_app_missing(&other));

        let down = crate::open_connector_proxy::ProxyError {
            status: 502,
            message: "open-connector sidecar unreachable: connection refused".to_string(),
            unreachable: true,
        };
        assert!(!sidecar_oauth_app_missing(&down));

        let (status, Json(body)) = oauth_app_not_configured("gmail");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body.get("error").and_then(|v| v.as_str()),
            Some("oauth_app_not_configured")
        );
        let hint = body
            .get("setup_hint")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        assert!(hint.contains("PUT /api/oauth/configs/gmail"), "hint: {hint}");
    }

    async fn test_app_state(temp: &Path) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(tokio::sync::RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            office_cli_docs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_watches: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
                "__test__".to_string(),
            ),
            fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn connect_allternit_mail_without_agent_returns_rail_status() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = connector_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/connectors/allternit-mail/connect")
                    .header("content-type", "application/json")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        // No mailflare env in tests → the rail reports unconfigured with a
        // setup hint instead of attempting a connect.
        assert_eq!(body["status"], "unconfigured");
        assert_eq!(body["backend"], "allternit_native");
        assert_eq!(body["rail"]["configured"], false);
        assert!(
            body["setup_hint"]
                .as_str()
                .unwrap()
                .contains("ALLTERNIT_MAILFLARE_URL"),
            "hint: {}",
            body["setup_hint"]
        );
    }

    #[tokio::test]
    async fn disconnect_allternit_mail_removes_row_only() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account, backend)
                 VALUES ('c1', 'allternit-mail', 'user-1', 'allternit_native', 'connected', 'a@b.c', 'allternit_native')",
                [],
            )
            .unwrap();
        }
        let app = connector_router().with_state(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/connectors/allternit-mail/disconnect")
                    .header("x-allternit-user-id", "user-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], "disconnected");
        let conn = state.db.connect().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM connector_connections WHERE connector_id='allternit-mail' AND user_id='user-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "disconnect must delete the index row");
    }

    #[tokio::test]
    async fn mcp_tools_list_and_status_are_served_in_process() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;

        // tools/list: sidecar is down in tests, but the allternit_mail tools
        // must still be listed.
        let resp = handle_allternit_mail_mcp(
            &state,
            "user-1",
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await
        .expect("tools/list intercepted");
        let body = body_json(resp.into_body()).await;
        let tools = body["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"allternit_mail.send"), "tools: {names:?}");
        assert!(names.contains(&"allternit_mail.status"), "tools: {names:?}");
        assert_eq!(body["id"], 1);

        // allternit_mail.status without agent_id: rail status only (no
        // mailflare env in tests → configured:false).
        let resp = handle_allternit_mail_mcp(
            &state,
            "user-1",
            &json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "allternit_mail.status", "arguments": {} }
            }),
        )
        .await
        .expect("status tool intercepted");
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["result"]["isError"], false);
        let payload: Value =
            serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload["configured"], false);

        // Unknown allternit_mail tool → MCP tool error, not a proxy fallback.
        let resp = handle_allternit_mail_mcp(
            &state,
            "user-1",
            &json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "allternit_mail.nope", "arguments": {} }
            }),
        )
        .await
        .expect("unknown mail tool intercepted");
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["result"]["isError"], true);

        // Non-mail tools/call and other methods fall through to the sidecar.
        assert!(handle_allternit_mail_mcp(
            &state,
            "user-1",
            &json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": { "name": "list_apps", "arguments": {} }
            }),
        )
        .await
        .is_none());
    }

    #[tokio::test]
    async fn mcp_send_enforces_agent_ownership() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO agents (id, user_id, name, model, provider) VALUES ('agent-1', 'owner-1', 'A', 'm', 'p')",
                [],
            )
            .unwrap();
        }
        // A different user cannot send through someone else's agent.
        let resp = handle_allternit_mail_mcp(
            &state,
            "intruder",
            &json!({
                "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                "params": { "name": "allternit_mail.send", "arguments": {
                    "agent_id": "agent-1", "to": "x@y.z", "subject": "hi", "text": "body"
                } }
            }),
        )
        .await
        .expect("send intercepted");
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["result"]["isError"], true);
        let payload: Value =
            serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload["error"], "forbidden");
    }

    #[tokio::test]
    async fn resolver_marks_sidecar_and_native_connections() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO agents (id, user_id, name, model, provider) VALUES ('agent-1', 'user-1', 'A', 'm', 'p')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, backend)
                 VALUES ('c1', 'gmail', 'user-1', 'oauth2', 'connected', 'open_connector')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account, backend)
                 VALUES ('c2', 'allternit-mail', 'user-1', 'allternit_native', 'connected', 'agent-1@agents.test', 'allternit_native')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_identity_channels (id, agent_id, user_id, email_address, email_provider, email_send_enabled, email_receive_enabled)
                 VALUES ('ch1', 'agent-1', 'user-1', 'agent-1@agents.test', 'mailflare', 1, 1)",
                [],
            )
            .unwrap();
        }
        let app = crate::allternit_bus_routes::allternit_bus_router().with_state(state);
        let user = crate::auth::AuthUser {
            user_id: "user-1".to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        };
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/agents/agent-1/connectors/resolve")
                    .header("content-type", "application/json")
                    .extension(user)
                    .body(Body::from(
                        json!({ "bindings": [
                            { "connector_id": "gmail", "provider": "gmail", "label": "Gmail" },
                            { "connector_id": "allternit-mail", "provider": "allternit-mail", "label": "Allternit Mail" }
                        ] })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["missing"].as_array().unwrap().len(), 0);
        assert_eq!(body["credentials"].as_array().unwrap().len(), 0);
        let conns = body["connections"].as_array().unwrap();
        assert_eq!(conns.len(), 2, "connections: {conns:?}");
        let gmail = conns
            .iter()
            .find(|c| c["connector_id"] == "gmail")
            .expect("gmail marker");
        assert_eq!(gmail["backend"], "open_connector");
        assert_eq!(gmail["via"], "mcp");
        assert_eq!(gmail["connected"], true);
        let mail = conns
            .iter()
            .find(|c| c["connector_id"] == "allternit-mail")
            .expect("mail marker");
        assert_eq!(mail["backend"], "allternit_native");
        assert_eq!(mail["via"], "agent_email");
        assert_eq!(mail["address"], "agent-1@agents.test");
    }

    #[tokio::test]
    async fn connectors_setup_status_returns_shape_when_unconfigured() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = connector_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/connectors/setup-status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["ready"], false);
        let checks = body.get("checks").expect("checks object");
        assert!(checks.get("allternit_mail").is_some());
        assert!(checks.get("sidecar").is_some());
        assert!(checks.get("gmail").is_some());
        assert!(checks.get("google_drive").is_some());
        // Allternit Mail is definitely unconfigured in this test (no env vars).
        assert_eq!(checks["allternit_mail"]["configured"], false);
        // Sidecar / OAuth state depends on the host environment; only assert shape.
        assert!(checks["sidecar"]["healthy"].is_boolean());
        assert!(checks["gmail"]["configured"].is_boolean());
        assert!(checks["google_drive"]["configured"].is_boolean());
        assert!(checks["gmail"]["setup_hint"].is_string());
    }
}
