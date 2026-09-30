//! Per-user MCP proxy: `POST /mcp/user-proxy`.
//!
//! Chat tools execute in gizzi-code, which knows nothing about a user's
//! `mcp_connectors` — and must not: their OAuth tokens stay in allternit-api.
//! For each chat turn the bridge mints a short-lived proxy token bound to
//! `{user_id, session_id}` and registers this endpoint as the turn's single MCP
//! server. The proxy is a streamable-HTTP MCP server whose tools are the union
//! of the user's enabled connectors' tools, namespaced `<connector>__<tool>`;
//! `tools/call` is routed to the owning connector with its stored credentials.
//!
//! The proxy token is an HMAC-signed `{uid, sid, exp}` — never the user's Clerk
//! token — and is honoured only together with a matching `X-Allternit-Session`
//! header. Connector credentials, the proxy token and tool arguments are never
//! logged.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use mcp_client::McpClient;
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::mcp_apps::{
    allow_private_hosts, close_session, list_all_tools, load_user_connectors, normalize_name, open_session,
    tool_visible_to_model, ui_resource_uri, upstream_error, AppsError, Connector,
};
use crate::AppState;

/// The key gizzi registers the proxy under; it is also what `mcp_app` emission
/// recognises as "this tool name is namespaced".
pub const PROXY_SERVER_NAME: &str = "allternit-connectors";
/// Header gizzi sends with the session the token was minted for.
/// `_meta` key carrying `{id, name}` of the connector a proxied tool belongs to.
pub const CONNECTOR_META_KEY: &str = "allternit/connector";
pub const SESSION_HEADER: &str = "x-allternit-session";
/// Upper bound on a proxy token's life (the task's ≤ 15 minutes).
pub const TOKEN_TTL_SECS: i64 = 15 * 60;
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const PER_CONNECTOR_TIMEOUT: Duration = Duration::from_secs(25);

pub fn mcp_user_proxy_router() -> Router<Arc<AppState>> {
    Router::new().route("/mcp/user-proxy", post(proxy_post).delete(proxy_delete))
}

// ─── Proxy token ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyClaims {
    pub user_id: String,
    pub session_id: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    Malformed,
    BadSignature,
    Expired,
    WrongSession,
}

fn proxy_secret() -> &'static [u8; 32] {
    static SECRET: OnceLock<[u8; 32]> = OnceLock::new();
    SECRET.get_or_init(|| {
        use sha2::Digest;
        // Explicit secret for multi-instance deployments; otherwise a per-process
        // random one — tokens live minutes and are minted and checked by the
        // same process.
        let seed = std::env::var("ALLTERNIT_MCP_PROXY_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()));
        let mut out = [0u8; 32];
        out.copy_from_slice(&Sha256::digest(seed.as_bytes()));
        out
    })
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn sign(payload: &str) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(proxy_secret()).expect("hmac accepts any key length");
    mac.update(payload.as_bytes());
    mac
}

fn mint_at(user_id: &str, session_id: &str, now: i64) -> String {
    let claims = json!({ "uid": user_id, "sid": session_id, "exp": now + TOKEN_TTL_SECS });
    let payload = b64().encode(claims.to_string());
    let tag = b64().encode(sign(&payload).finalize().into_bytes());
    format!("v1.{payload}.{tag}")
}

/// A proxy token for `user_id` in chat session `session_id`, valid for 15 minutes.
pub fn mint_proxy_token(user_id: &str, session_id: &str) -> String {
    mint_at(user_id, session_id, chrono::Utc::now().timestamp())
}

fn verify_at(token: &str, session_id: &str, now: i64) -> Result<ProxyClaims, TokenError> {
    let mut parts = token.split('.');
    let (Some("v1"), Some(payload), Some(tag), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(TokenError::Malformed);
    };
    let tag = b64().decode(tag).map_err(|_| TokenError::Malformed)?;
    // `verify_slice` compares in constant time.
    sign(payload).verify_slice(&tag).map_err(|_| TokenError::BadSignature)?;
    let claims: Value = serde_json::from_slice(&b64().decode(payload).map_err(|_| TokenError::Malformed)?)
        .map_err(|_| TokenError::Malformed)?;
    let (Some(uid), Some(sid), Some(exp)) = (
        claims["uid"].as_str(),
        claims["sid"].as_str(),
        claims["exp"].as_i64(),
    ) else {
        return Err(TokenError::Malformed);
    };
    if now >= exp || exp - now > TOKEN_TTL_SECS {
        return Err(TokenError::Expired);
    }
    if sid != session_id {
        return Err(TokenError::WrongSession);
    }
    Ok(ProxyClaims { user_id: uid.into(), session_id: sid.into(), expires_at: exp })
}

/// Check `token` for use in `session_id`.
pub fn verify_proxy_token(token: &str, session_id: &str) -> Result<ProxyClaims, TokenError> {
    verify_at(token, session_id, chrono::Utc::now().timestamp())
}

// ─── Namespacing ─────────────────────────────────────────────────────────────

/// A connector's namespace: its `name_id` normalised, with no `__` inside and
/// no leading/trailing `_`, so `split_namespaced` always cuts at the right `__`.
fn base_prefix(name_id: &str) -> String {
    let mut out = String::new();
    for c in normalize_name(name_id).chars() {
        if c == '_' && out.ends_with('_') {
            continue;
        }
        out.push(c);
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() { "connector".into() } else { trimmed.into() }
}

/// One unique prefix per connector, in the connectors' own order (colliding
/// names get `_2`, `_3`, …).
pub(crate) fn connector_prefixes(connectors: &[Connector]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    connectors
        .iter()
        .map(|c| {
            let base = base_prefix(&c.name_id);
            let mut candidate = base.clone();
            let mut n = 2;
            while !seen.insert(candidate.clone()) {
                candidate = format!("{base}_{n}");
                n += 1;
            }
            candidate
        })
        .collect()
}

pub(crate) fn namespaced(prefix: &str, tool: &str) -> String {
    format!("{prefix}__{tool}")
}

/// `<prefix>__<tool>` → `(prefix, tool)`.
pub(crate) fn split_namespaced(name: &str) -> Option<(&str, &str)> {
    let (prefix, tool) = name.split_once("__")?;
    (!prefix.is_empty() && !tool.is_empty()).then_some((prefix, tool))
}

/// The connector and original tool name a namespaced tool refers to.
pub(crate) fn resolve_namespaced<'a>(connectors: &'a [Connector], name: &'a str) -> Option<(&'a Connector, &'a str)> {
    let (prefix, tool) = split_namespaced(name)?;
    let prefixes = connector_prefixes(connectors);
    let index = prefixes.iter().position(|p| p == prefix)?;
    Some((&connectors[index], tool))
}

// ─── Registration with gizzi ─────────────────────────────────────────────────

/// Where gizzi reaches the proxy. `ALLTERNIT_MCP_PROXY_URL` for deployments where
/// gizzi is not on the same host as this API.
fn proxy_url() -> String {
    std::env::var("ALLTERNIT_MCP_PROXY_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| {
            let port = crate::APP_CONFIG.get().map(|c| c.api_port()).unwrap_or(8013);
            format!("http://127.0.0.1:{port}/mcp/user-proxy")
        })
}

/// The `mcpProxy` prompt field for one gizzi turn, or `None` when the user has
/// no enabled connector (nothing to proxy, so no token is minted). Gizzi holds
/// it in memory for that turn only.
pub async fn proxy_registration(state: &Arc<AppState>, user_id: &str, session_id: &str) -> Option<Value> {
    let db = state.db.clone();
    let uid = user_id.to_string();
    let has_connectors = tokio::task::spawn_blocking(move || -> rusqlite::Result<bool> {
        db.connect()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM mcp_connectors WHERE user_id = ?1 AND enabled = 1)",
            rusqlite::params![uid],
            |r| r.get(0),
        )
    })
    .await
    .ok()?
    .unwrap_or(false);
    has_connectors.then(|| {
        json!({
            "server": PROXY_SERVER_NAME,
            "url": proxy_url(),
            "sessionId": session_id,
            "token": mint_proxy_token(user_id, session_id),
        })
    })
}

// ─── JSON-RPC plumbing ───────────────────────────────────────────────────────

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn rpc_from_apps_error(id: &Value, e: &AppsError) -> Value {
    rpc_error(id, -32001, e.code, Some(json!({ "code": e.code, "message": e.message })))
}

fn unauthorized_response(reason: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
        Json(json!({ "error": reason })),
    )
        .into_response()
}

fn bearer_of(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|t| !t.is_empty())
}

fn claims_from(headers: &HeaderMap) -> Result<ProxyClaims, Response> {
    let token = bearer_of(headers).ok_or_else(|| unauthorized_response("missing proxy token"))?;
    let session = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| unauthorized_response("missing session"))?;
    verify_proxy_token(token, session).map_err(|e| {
        warn!(error = ?e, "mcp user proxy: token rejected");
        unauthorized_response("invalid proxy token")
    })
}

async fn proxy_delete(headers: HeaderMap) -> Response {
    // Stateless server: there is no session to end, but a valid caller is told so.
    match claims_from(&headers) {
        Ok(_) => StatusCode::OK.into_response(),
        Err(r) => r,
    }
}

async fn proxy_post(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let claims = match claims_from(&headers) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match handle_rpc(&state, &claims, &body, allow_private_hosts()).await {
        Some(payload) => Json(payload).into_response(),
        // Notification (e.g. notifications/initialized): acknowledged, nothing to do.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// One JSON-RPC message from an authenticated caller. `None` for notifications.
async fn handle_rpc(state: &Arc<AppState>, claims: &ProxyClaims, body: &Value, allow_private: bool) -> Option<Value> {
    let id = body.get("id").cloned()?;
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = body.get("params").cloned().unwrap_or(Value::Null);
    let started = Instant::now();

    let (payload, tool, outcome) = match method {
        "initialize" => (rpc_result(&id, initialize_result(&params)), None, "ok"),
        "ping" => (rpc_result(&id, json!({})), None, "ok"),
        "tools/list" => {
            let connectors = load_user_connectors(state, &claims.user_id, allow_private).await;
            (rpc_result(&id, list_tools(&connectors, allow_private).await), None, "ok")
        }
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let arguments = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(v @ Value::Object(_)) => v.clone(),
                Some(_) => return Some(rpc_error(&id, -32602, "arguments must be an object", None)),
            };
            let connectors = load_user_connectors(state, &claims.user_id, allow_private).await;
            let result = call_tool(&connectors, &name, arguments, allow_private).await;
            let outcome = if result.is_ok() { "ok" } else { "error" };
            let payload = match result {
                Ok(v) => rpc_result(&id, v),
                Err(e) => rpc_from_apps_error(&id, &e),
            };
            (payload, Some(name), outcome)
        }
        "resources/read" => {
            let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("").to_string();
            let connectors = load_user_connectors(state, &claims.user_id, allow_private).await;
            let result = read_ui_resource(&connectors, &uri, allow_private).await;
            let outcome = if result.is_ok() { "ok" } else { "error" };
            let payload = match result {
                Ok(v) => rpc_result(&id, v),
                Err(e) => rpc_from_apps_error(&id, &e),
            };
            (payload, None, outcome)
        }
        other => (rpc_error(&id, -32601, &format!("method '{other}' not found"), None), None, "unsupported"),
    };

    info!(
        user_id = %claims.user_id,
        session_id = %claims.session_id,
        method,
        tool = tool.as_deref().unwrap_or(""),
        outcome,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "mcp user proxy call"
    );
    Some(payload)
}

fn initialize_result(params: &Value) -> Value {
    // Echo a version the client asked for when it is one this proxy is written to.
    let version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .filter(|v| ["2024-11-05", "2025-03-26", "2025-06-18"].contains(v))
        .unwrap_or(MCP_PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false }, "resources": {} },
        "serverInfo": { "name": PROXY_SERVER_NAME, "version": "1" },
    })
}

// ─── tools ───────────────────────────────────────────────────────────────────

async fn with_session<T, F, Fut>(connector: &Connector, allow_private: bool, f: F) -> Result<T, AppsError>
where
    F: FnOnce(McpClient) -> Fut,
    Fut: std::future::Future<Output = (McpClient, Result<T, AppsError>)>,
{
    let client = open_session(connector, allow_private).await?;
    let (client, result) = f(client).await;
    close_session(client).await;
    result
}

/// Union of the connectors' model-visible tools, namespaced. A connector that
/// cannot be reached or whose credentials are dead contributes nothing.
async fn list_tools(connectors: &[Connector], allow_private: bool) -> Value {
    let prefixes = connector_prefixes(connectors);
    let listed = futures::future::join_all(connectors.iter().map(|connector| async move {
        let work = with_session(connector, allow_private, |client| async move {
            let tools = list_all_tools(&client).await;
            (client, tools)
        });
        tokio::time::timeout(PER_CONNECTOR_TIMEOUT, work).await
    }))
    .await;

    let mut tools = Vec::new();
    for ((connector, prefix), listing) in connectors.iter().zip(&prefixes).zip(listed) {
        match listing {
            Ok(Ok(items)) => {
                for mut tool in items.into_iter().filter(tool_visible_to_model) {
                    let Some(name) = tool.get("name").and_then(|n| n.as_str()).map(String::from) else { continue };
                    tool["name"] = json!(namespaced(prefix, &name));
                    // Additive: which connector this tool came from, for hosts that render its
                    // app without a database of their own (gizzi's own agent-chat route).
                    if !tool["_meta"].is_object() {
                        tool["_meta"] = json!({});
                    }
                    tool["_meta"][CONNECTOR_META_KEY] = json!({ "id": connector.id, "name": connector.name });
                    tools.push(tool);
                }
            }
            Ok(Err(e)) => warn!(connector_id = %connector.id, code = e.code, "mcp user proxy: connector tools unavailable"),
            Err(_) => warn!(connector_id = %connector.id, "mcp user proxy: connector tools timed out"),
        }
    }
    json!({ "tools": tools })
}

async fn call_tool(
    connectors: &[Connector],
    namespaced_name: &str,
    arguments: Value,
    allow_private: bool,
) -> Result<Value, AppsError> {
    let (connector, tool_name) = resolve_namespaced(connectors, namespaced_name).ok_or_else(|| {
        AppsError::new(StatusCode::NOT_FOUND, "tool_not_found", format!("unknown tool '{namespaced_name}'"))
    })?;
    let tool_name = tool_name.to_string();
    with_session(connector, allow_private, |client| async move {
        let result = async {
            // Visibility is only knowable from the connector's own list: app-only
            // tools are not callable by the model, whatever name it asks for.
            let tools = list_all_tools(&client).await?;
            let tool = tools
                .iter()
                .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(tool_name.as_str()))
                .ok_or_else(|| {
                    AppsError::new(StatusCode::NOT_FOUND, "tool_not_found", format!("unknown tool '{tool_name}'"))
                })?;
            if !tool_visible_to_model(tool) {
                return Err(AppsError::new(
                    StatusCode::FORBIDDEN,
                    "tool_not_visible_to_model",
                    format!("tool '{tool_name}' is not callable by the model"),
                ));
            }
            client
                .request("tools/call", Some(json!({ "name": tool_name, "arguments": arguments })))
                .await
                .map_err(upstream_error)
        }
        .await;
        (client, result)
    })
    .await
}

/// `resources/read` for a `ui://` URI: served by the connector that declares it
/// on one of its tools.
async fn read_ui_resource(connectors: &[Connector], uri: &str, allow_private: bool) -> Result<Value, AppsError> {
    if !uri.starts_with("ui://") {
        return Err(AppsError::new(StatusCode::FORBIDDEN, "resource_not_allowed", "only ui:// resources are served"));
    }
    for connector in connectors {
        let read = with_session(connector, allow_private, |client| async move {
            let result = async {
                let tools = list_all_tools(&client).await?;
                if !tools.iter().any(|t| ui_resource_uri(t).as_deref() == Some(uri)) {
                    return Ok(None);
                }
                client
                    .request("resources/read", Some(json!({ "uri": uri })))
                    .await
                    .map(Some)
                    .map_err(upstream_error)
            }
            .await;
            (client, result)
        })
        .await;
        match read {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => {}
            Err(e) => warn!(connector_id = %connector.id, code = e.code, "mcp user proxy: resource read failed"),
        }
    }
    Err(AppsError::new(StatusCode::NOT_FOUND, "resource_not_found", "no connector serves that resource"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trips_and_carries_its_binding() {
        let token = mint_proxy_token("user-1", "ses_1");
        let claims = verify_proxy_token(&token, "ses_1").unwrap();
        assert_eq!((claims.user_id.as_str(), claims.session_id.as_str()), ("user-1", "ses_1"));
        assert!(claims.expires_at - chrono::Utc::now().timestamp() <= TOKEN_TTL_SECS);
    }

    #[test]
    fn token_expires() {
        let token = mint_at("u", "s", 1_000);
        assert!(verify_at(&token, "s", 1_000 + TOKEN_TTL_SECS - 1).is_ok());
        assert_eq!(verify_at(&token, "s", 1_000 + TOKEN_TTL_SECS), Err(TokenError::Expired));
        assert_eq!(verify_at(&token, "s", 1_000 + 10 * TOKEN_TTL_SECS), Err(TokenError::Expired));
    }

    #[test]
    fn token_is_single_session() {
        let token = mint_proxy_token("u", "ses_a");
        assert_eq!(verify_proxy_token(&token, "ses_b"), Err(TokenError::WrongSession));
    }

    #[test]
    fn tampered_tokens_are_rejected() {
        let token = mint_at("victim", "s", chrono::Utc::now().timestamp());
        let parts: Vec<&str> = token.split('.').collect();
        // a forged payload (another user) under the original signature
        let forged = b64().encode(json!({ "uid": "attacker", "sid": "s", "exp": i64::MAX / 2 }).to_string());
        assert_eq!(
            verify_proxy_token(&format!("v1.{forged}.{}", parts[2]), "s"),
            Err(TokenError::BadSignature)
        );
        // a flipped signature byte
        let mut tag = b64().decode(parts[2]).unwrap();
        tag[0] ^= 1;
        assert_eq!(
            verify_proxy_token(&format!("v1.{}.{}", parts[1], b64().encode(tag)), "s"),
            Err(TokenError::BadSignature)
        );
        // structurally wrong
        for bad in ["", "v1", "v1.a", "v2.a.b", "v1.a.b.c", "v1.!!.!!"] {
            assert!(verify_proxy_token(bad, "s").is_err(), "{bad}");
        }
        // unsigned / a user's bearer token of another kind is never a proxy token
        assert!(verify_proxy_token("eyJhbGciOiJSUzI1NiJ9.e30.sig", "s").is_err());
    }

    #[test]
    fn token_does_not_contain_anything_secret() {
        let token = mint_proxy_token("user-1", "ses_1");
        let payload = token.split('.').nth(1).unwrap();
        let text = String::from_utf8(b64().decode(payload).unwrap()).unwrap();
        assert!(text.contains("user-1") && text.contains("ses_1"));
    }

    fn connectors(names: &[&str]) -> Vec<Connector> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| Connector::for_test(&format!("c{i}"), n, "https://x.test/mcp", None))
            .collect()
    }

    #[test]
    fn namespacing_round_trips_for_awkward_names() {
        let cs = connectors(&["github", "my server.v2", "a__b", "_x_", "a_", "", "github"]);
        let prefixes = connector_prefixes(&cs);
        assert_eq!(prefixes[0], "github");
        assert_eq!(prefixes[1], "my_server_v2");
        assert_eq!(prefixes[2], "a_b");
        assert_eq!(prefixes[3], "x");
        assert_eq!(prefixes[4], "a");
        assert_eq!(prefixes[5], "connector");
        assert_eq!(prefixes[6], "github_2", "collisions are disambiguated");
        for p in &prefixes {
            assert!(!p.contains("__") && !p.starts_with('_') && !p.ends_with('_'), "{p}");
        }
        // every (connector, tool) pair resolves back to itself, including tools whose own
        // names contain `__` or start with `_`
        for (i, p) in prefixes.iter().enumerate() {
            for tool in ["search", "do__thing", "_private", "a-b.c"] {
                let name = namespaced(p, tool);
                let (c, t) = resolve_namespaced(&cs, &name).unwrap();
                assert_eq!((c.id.as_str(), t), (cs[i].id.as_str(), tool), "{name}");
            }
        }
        assert!(resolve_namespaced(&cs, "nosuch__tool").is_none());
        assert!(resolve_namespaced(&cs, "github").is_none());
        assert!(resolve_namespaced(&cs, "github__").is_none());
    }

    #[test]
    fn initialize_reports_tools_and_resources() {
        let r = initialize_result(&json!({ "protocolVersion": "2025-03-26" }));
        assert_eq!(r["protocolVersion"], "2025-03-26");
        assert_eq!(initialize_result(&json!({ "protocolVersion": "1999" }))["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(r["capabilities"]["tools"].is_object() && r["capabilities"]["resources"].is_object());
    }
}

#[cfg(test)]
mod e2e {
    use super::*;
    use crate::mcp_apps::tests::{fixture, spawn_mcp_apps_server_with, APP_HTML, TOKEN};
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use http_body_util::BodyExt;
    use rusqlite::params;
    use std::sync::Mutex;
    use tower::ServiceExt;

    fn claims(user: &str) -> ProxyClaims {
        ProxyClaims { user_id: user.into(), session_id: "ses_1".into(), expires_at: i64::MAX }
    }

    async fn rpc(state: &Arc<AppState>, user: &str, method: &str, params: Value) -> Value {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        handle_rpc(state, &claims(user), &body, true).await.expect("response")
    }

    fn tool_names(list: &Value) -> Vec<String> {
        list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn tools_list_is_namespaced_keeps_meta_and_hides_app_only_tools() {
        let (state, _server, _) = fixture().await;
        let list = rpc(&state, "user-1", "tools/list", json!({})).await;
        let mut names = tool_names(&list);
        names.sort();
        // `refresh_data` is visibility ["app"]: never offered to the model. The disabled
        // connector contributes nothing.
        assert_eq!(names, ["dash-server__model_only", "dash-server__plain", "dash-server__show_dashboard"]);
        let show = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "dash-server__show_dashboard")
            .unwrap();
        assert_eq!(show["_meta"]["ui"]["resourceUri"], "ui://dash/app");
        assert_eq!(show["_meta"][CONNECTOR_META_KEY]["id"], "conn-1");
        assert_eq!(show["title"], "Dashboard");
        assert_eq!(show["inputSchema"]["properties"]["range"]["type"], "string");
    }

    #[tokio::test]
    async fn end_to_end_call_through_the_proxy_yields_an_mcp_app_frame() {
        let (state, server, _) = fixture().await;
        let called = rpc(
            &state,
            "user-1",
            "tools/call",
            json!({ "name": "dash-server__show_dashboard", "arguments": { "range": "7d" } }),
        )
        .await;
        assert_eq!(called["result"]["structuredContent"]["rows"], json!([1, 2, 3]));
        assert_eq!(called["result"]["_meta"]["trace"], "t-1");
        // the connector saw its own tool name and its own stored credentials
        assert_eq!(server.called_tools(), ["show_dashboard"]);
        assert!(server.log.lock().unwrap().iter().all(|s| s.auth.as_deref() == Some(&*format!("Bearer {TOKEN}"))));

        // gizzi tags the completed tool part with the proxy server and the namespaced tool
        let part = json!({
            "type": "tool", "callID": "call-7", "tool": "mcp__allternit-connectors__dash-server__show_dashboard",
            "state": { "status": "completed", "input": { "range": "7d" }, "output": "ran show_dashboard",
                "metadata": { "mcp": {
                    "server": PROXY_SERVER_NAME, "tool": "dash-server__show_dashboard",
                    "result": called["result"].clone()
                }}}
        });
        let frame = crate::mcp_apps::app_frame_for_tool_part_with(&state, "user-1", "msg_1", &part, true)
            .await
            .expect("mcp_app frame");
        assert_eq!(frame["type"], "mcp_app");
        assert_eq!(frame["connectorId"], "conn-1");
        assert_eq!(frame["toolName"], "show_dashboard", "namespace resolved back to the connector's tool");
        assert_eq!(frame["toolCallId"], "call-7");
        assert_eq!(frame["html"], APP_HTML);
        assert_eq!(frame["toolInput"]["range"], "7d");
        assert_eq!(frame["toolResult"]["structuredContent"]["rows"], json!([1, 2, 3]));

        // someone else's turn never resolves this user's connector
        assert!(crate::mcp_apps::app_frame_for_tool_part_with(&state, "user-2", "m", &part, true).await.is_none());
        // an unknown namespace yields no app
        let mut bad = part.clone();
        bad["state"]["metadata"]["mcp"]["tool"] = json!("ghost__show_dashboard");
        assert!(crate::mcp_apps::app_frame_for_tool_part_with(&state, "user-1", "m", &bad, true).await.is_none());
    }

    #[tokio::test]
    async fn app_only_and_unknown_tools_are_not_callable_by_the_model() {
        let (state, server, _) = fixture().await;
        let app_only = rpc(&state, "user-1", "tools/call", json!({ "name": "dash-server__refresh_data" })).await;
        assert_eq!(app_only["error"]["data"]["code"], "tool_not_visible_to_model");
        let unknown = rpc(&state, "user-1", "tools/call", json!({ "name": "dash-server__ghost" })).await;
        assert_eq!(unknown["error"]["data"]["code"], "tool_not_found");
        let no_namespace = rpc(&state, "user-1", "tools/call", json!({ "name": "show_dashboard" })).await;
        assert_eq!(no_namespace["error"]["data"]["code"], "tool_not_found");
        assert_eq!(server.called_tools(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn users_only_see_and_reach_their_own_connectors() {
        let (state, server, url) = fixture().await;
        // user-2 has a connector of their own with a different name (same fake server, no token)
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES ('conn-2', 'user-2', 'Theirs', 'theirs', ?1)",
                params![url],
            )
            .unwrap();
        let theirs = rpc(&state, "user-2", "tools/list", json!({})).await;
        // conn-2 has no token, so the fake server rejects it and it contributes nothing — and
        // user-1's tools never appear in user-2's list
        assert_eq!(tool_names(&theirs), Vec::<String>::new());
        let before = server.called_tools().len();
        let crossed = rpc(&state, "user-2", "tools/call", json!({ "name": "dash-server__show_dashboard" })).await;
        assert_eq!(crossed["error"]["data"]["code"], "tool_not_found");
        assert_eq!(server.called_tools().len(), before, "user-1's connector was never contacted for user-2");
        // a user with no connectors at all
        let nobody = rpc(&state, "user-9", "tools/list", json!({})).await;
        assert_eq!(tool_names(&nobody), Vec::<String>::new());
    }

    #[tokio::test]
    async fn resources_read_serves_only_ui_resources_declared_by_a_tool() {
        let (state, _server, _) = fixture().await;
        let read = rpc(&state, "user-1", "resources/read", json!({ "uri": "ui://dash/app" })).await;
        assert_eq!(read["result"]["contents"][0]["text"], APP_HTML);
        assert_eq!(read["result"]["contents"][0]["_meta"]["ui"]["csp"]["connectDomains"][0], "https://api.dash.example");
        for uri in ["ui://other/app", "file:///etc/passwd", "https://x.test"] {
            let r = rpc(&state, "user-1", "resources/read", json!({ "uri": uri })).await;
            assert!(r["error"].is_object(), "{uri}");
        }
        let other_user = rpc(&state, "user-2", "resources/read", json!({ "uri": "ui://dash/app" })).await;
        assert!(other_user["error"].is_object());
    }

    #[tokio::test]
    async fn initialize_ping_unknown_methods_and_notifications() {
        let (state, _server, _) = fixture().await;
        assert_eq!(rpc(&state, "user-1", "initialize", json!({})).await["result"]["serverInfo"]["name"], PROXY_SERVER_NAME);
        assert_eq!(rpc(&state, "user-1", "ping", json!({})).await["result"], json!({}));
        assert_eq!(rpc(&state, "user-1", "prompts/list", json!({})).await["error"]["code"], -32601);
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle_rpc(&state, &claims("user-1"), &note, true).await.is_none());
    }

    async fn http(state: Arc<AppState>, headers: &[(&str, String)], body: Value) -> StatusCode {
        let mut req = Request::builder().method("POST").uri("/mcp/user-proxy").header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        let resp = mcp_user_proxy_router()
            .with_state(state)
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let _ = resp.into_body().collect().await;
        status
    }

    #[tokio::test]
    async fn the_route_demands_a_valid_token_for_this_session() {
        let (state, _server, _) = fixture().await;
        let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
        let token = mint_proxy_token("user-1", "ses_1");
        let auth = |t: &str| ("authorization", format!("Bearer {t}"));
        let sid = |s: &str| (SESSION_HEADER, s.to_string());

        assert_eq!(http(state.clone(), &[auth(&token), sid("ses_1")], ping.clone()).await, StatusCode::OK);
        // no token, no session header, wrong session, garbage, a user's ordinary bearer token
        assert_eq!(http(state.clone(), &[sid("ses_1")], ping.clone()).await, StatusCode::UNAUTHORIZED);
        assert_eq!(http(state.clone(), &[auth(&token)], ping.clone()).await, StatusCode::UNAUTHORIZED);
        assert_eq!(http(state.clone(), &[auth(&token), sid("ses_2")], ping.clone()).await, StatusCode::UNAUTHORIZED);
        assert_eq!(http(state.clone(), &[auth("garbage"), sid("ses_1")], ping.clone()).await, StatusCode::UNAUTHORIZED);
        assert_eq!(http(state.clone(), &[auth("a.b.c"), sid("ses_1")], ping.clone()).await, StatusCode::UNAUTHORIZED);
        // notifications are acknowledged
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(http(state, &[auth(&token), sid("ses_1")], note).await, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn registration_is_minted_only_for_users_with_connectors() {
        let (state, _server, _) = fixture().await;
        let reg = proxy_registration(&state, "user-1", "ses_1").await.expect("registration");
        assert_eq!(reg["server"], PROXY_SERVER_NAME);
        assert_eq!(reg["sessionId"], "ses_1");
        assert!(reg["url"].as_str().unwrap().ends_with("/mcp/user-proxy"));
        let claims = verify_proxy_token(reg["token"].as_str().unwrap(), "ses_1").unwrap();
        assert_eq!(claims.user_id, "user-1");
        assert!(proxy_registration(&state, "user-9", "ses_1").await.is_none());
    }

    // ── OAuth refresh ────────────────────────────────────────────────────────

    #[derive(Clone, Default)]
    struct OAuthLog {
        forms: Arc<Mutex<Vec<String>>>,
        refuse: Arc<std::sync::atomic::AtomicBool>,
    }

    fn oauth_routes(log: OAuthLog) -> axum::Router {
        use axum::extract::State;
        axum::Router::new()
            .route(
                "/mcp/.well-known/oauth-authorization-server",
                get(|headers: HeaderMap| async move {
                    let host = headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
                    Json(json!({ "token_endpoint": format!("http://{host}/oauth-token") }))
                }),
            )
            .route(
                "/oauth-token",
                post(|State(log): State<OAuthLog>, body: String| async move {
                    log.forms.lock().unwrap().push(body);
                    if log.refuse.load(std::sync::atomic::Ordering::SeqCst) {
                        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_grant" }))).into_response();
                    }
                    Json(json!({ "access_token": TOKEN, "token_type": "Bearer", "expires_in": 3600 })).into_response()
                }),
            )
            // A separate auth server's token endpoint, recorded at OAuth start.
            .route(
                "/as/token",
                post(|State(log): State<OAuthLog>, body: String| async move {
                    log.forms.lock().unwrap().push(format!("AS {body}"));
                    Json(json!({ "access_token": TOKEN, "token_type": "Bearer", "expires_in": 3600 })).into_response()
                }),
            )
            .with_state(log)
    }

    /// A connector whose stored access token expired an hour ago.
    async fn expired_fixture(refuse: bool) -> (Arc<AppState>, crate::mcp_apps::tests::TestServer, OAuthLog) {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::beta_session_routes::tests::test_app_state(&temp).await;
        let log = OAuthLog::default();
        log.refuse.store(refuse, std::sync::atomic::Ordering::SeqCst);
        let (url, server) = spawn_mcp_apps_server_with(oauth_routes(log.clone())).await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO mcp_connectors (id, user_id, name, name_id, url, oauth_client_id, oauth_client_secret)
             VALUES ('conn-r', 'user-1', 'Refreshable', 'refreshable', ?1, 'client-1', ?2)",
            params![url, crate::token_crypto::seal("client-secret-1")],
        )
        .unwrap();
        let tokens = json!({
            "access_token": "stale-token", "refresh_token": "refresh-1", "expires_in": 3600,
            "obtained_at": chrono::Utc::now().timestamp() - 7200
        });
        conn.execute(
            "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, tokens, is_authenticated)
             VALUES ('s-r', 'conn-r', 'st-r', ?1, 1)",
            params![crate::token_crypto::seal(&tokens.to_string())],
        )
        .unwrap();
        (state, server, log)
    }

    #[test]
    fn refresh_fixture_shape_is_sane() {
        // token_expiry: obtained_at + expires_in, else the row timestamp
        let tokens = json!({ "expires_in": 100, "obtained_at": 1000 });
        assert_eq!(crate::mcp_apps::token_expiry(&tokens, None), Some(1100));
        let legacy = json!({ "expires_in": "60" });
        assert_eq!(crate::mcp_apps::token_expiry(&legacy, Some("1970-01-01 00:10:00")), Some(660));
        assert_eq!(crate::mcp_apps::token_expiry(&json!({}), Some("1970-01-01 00:10:00")), None);
    }

    #[tokio::test]
    async fn expired_token_is_refreshed_before_the_call_and_stored_sealed() {
        let (state, server, log) = expired_fixture(false).await;
        let list = rpc(&state, "user-1", "tools/list", json!({})).await;
        assert!(tool_names(&list).contains(&"refreshable__show_dashboard".to_string()), "{list}");
        // the connector only ever saw the refreshed token
        assert!(server.log.lock().unwrap().iter().all(|s| s.auth.as_deref() == Some(&*format!("Bearer {TOKEN}"))));
        // a refresh-token grant with the stored client credentials
        let forms = log.forms.lock().unwrap().clone();
        assert_eq!(forms.len(), 1, "one refresh for one expiry");
        assert!(forms[0].contains("grant_type=refresh_token") && forms[0].contains("refresh_token=refresh-1"), "{}", forms[0]);
        assert!(forms[0].contains("client_id=client-1"));
        // the new tokens were written back sealed, keeping the refresh token the server did not rotate
        let stored: String = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT tokens FROM mcp_oauth_sessions WHERE id = 's-r'", [], |r| r.get(0))
            .unwrap();
        assert!(stored.starts_with("enc:v1:"), "{stored}");
        assert!(!stored.contains(TOKEN));
        let opened: Value = serde_json::from_str(&crate::token_crypto::open(&stored)).unwrap();
        assert_eq!(opened["access_token"], TOKEN);
        assert_eq!(opened["refresh_token"], "refresh-1");
        // a second call within the new lifetime does not refresh again
        rpc(&state, "user-1", "tools/list", json!({})).await;
        assert_eq!(log.forms.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn refresh_uses_the_token_endpoint_recorded_at_oauth_start() {
        let (state, _server, log) = expired_fixture(false).await;
        let conn = state.db.connect().unwrap();
        let url: String = conn.query_row("SELECT url FROM mcp_connectors WHERE id = 'conn-r'", [], |r| r.get(0)).unwrap();
        let origin = url.trim_end_matches("/mcp").to_string();
        conn.execute(
            "UPDATE mcp_oauth_sessions SET metadata = ?1 WHERE id = 's-r'",
            params![json!({ "tokenEndpoint": format!("{origin}/as/token"), "resource": url }).to_string()],
        )
        .unwrap();
        rpc(&state, "user-1", "tools/list", json!({})).await;
        let forms = log.forms.lock().unwrap().clone();
        assert_eq!(forms.len(), 1, "{forms:?}");
        assert!(forms[0].starts_with("AS "), "went to the discovered endpoint instead: {}", forms[0]);
        assert!(forms[0].contains("resource="), "{}", forms[0]);
    }

    #[tokio::test]
    async fn failed_refresh_reports_connector_unauthorized_without_calling_the_connector() {
        let (state, server, log) = expired_fixture(true).await;
        let called = rpc(&state, "user-1", "tools/call", json!({ "name": "refreshable__show_dashboard" })).await;
        // the listing is what resolves the connector, so the failed one surfaces on call as
        // unauthorized (its tools are simply absent from tools/list)
        let code = called["error"]["data"]["code"].as_str().unwrap_or("").to_string();
        assert_eq!(code, "connector_unauthorized", "{called}");
        assert_eq!(log.forms.lock().unwrap().len(), 1);
        assert_eq!(server.count(), 0, "a token known to be dead is never sent");
        let list = rpc(&state, "user-1", "tools/list", json!({})).await;
        assert!(tool_names(&list).is_empty());
    }

    // ── legacy SSE-only connectors ───────────────────────────────────────────

    /// A pre-2025 HTTP+SSE server: no POST endpoint on `/sse` (405), a stream on GET `/sse`,
    /// JSON-RPC over POST `/message`.
    async fn spawn_legacy_sse_server() -> (String, Arc<Mutex<Vec<(String, Option<String>)>>>) {
        use axum::extract::State;
        let log: Arc<Mutex<Vec<(String, Option<String>)>>> = Arc::default();
        let app = axum::Router::new()
            .route(
                "/sse",
                get(|| async {
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(futures::stream::pending::<Result<bytes::Bytes, std::io::Error>>()))
                        .unwrap()
                })
                .post(|| async { StatusCode::METHOD_NOT_ALLOWED }),
            )
            .route(
                "/message",
                post(|State(log): State<Arc<Mutex<Vec<(String, Option<String>)>>>>, headers: HeaderMap, Json(req): Json<Value>| async move {
                    let method = req["method"].as_str().unwrap_or("").to_string();
                    log.lock().unwrap().push((
                        method.clone(),
                        headers.get("authorization").and_then(|v| v.to_str().ok()).map(String::from),
                    ));
                    let Some(id) = req.get("id").cloned() else { return StatusCode::ACCEPTED.into_response() };
                    let result = match method.as_str() {
                        "initialize" => json!({
                            "protocolVersion": "2024-11-05",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "legacy", "version": "1" }
                        }),
                        "tools/list" => json!({ "tools": [
                            { "name": "echo", "description": "Echo", "inputSchema": { "type": "object" } }
                        ]}),
                        "tools/call" => json!({ "content": [{ "type": "text", "text": "legacy echo" }] }),
                        _ => json!({}),
                    };
                    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
                }),
            )
            .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/sse"), log)
    }

    #[tokio::test]
    async fn sse_only_connectors_fall_back_after_streamable_http_answers_405() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::beta_session_routes::tests::test_app_state(&temp).await;
        let (url, log) = spawn_legacy_sse_server().await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES ('conn-l', 'user-1', 'Legacy', 'legacy', ?1)",
            params![url],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, tokens, is_authenticated) VALUES ('s-l', 'conn-l', 'st-l', ?1, 1)",
            params![json!({ "access_token": "legacy-token" }).to_string()],
        )
        .unwrap();

        let direct = crate::mcp_apps::open_session(
            &Connector::for_test("x", "legacy", &url, Some("legacy-token")),
            true,
        )
        .await;
        if let Err(e) = &direct {
            panic!("fallback failed: {} {}", e.code, e.message);
        }
        let list = rpc(&state, "user-1", "tools/list", json!({})).await;
        assert_eq!(tool_names(&list), ["legacy__echo"], "{list}");
        let called = rpc(&state, "user-1", "tools/call", json!({ "name": "legacy__echo" })).await;
        assert_eq!(called["result"]["content"][0]["text"], "legacy echo", "{called}");
        // the fallback carried the connector's own token
        let seen = log.lock().unwrap().clone();
        assert!(seen.iter().any(|(m, _)| m == "initialize"));
        assert!(seen.iter().all(|(_, auth)| auth.as_deref() == Some("Bearer legacy-token")), "{seen:?}");
    }

    // ── tokens at rest ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn sealed_and_legacy_plaintext_tokens_both_authenticate() {
        let (state, server, _) = fixture().await;
        // the fixture's row is legacy plaintext JSON: it works as before
        assert!(tool_names(&rpc(&state, "user-1", "tools/list", json!({})).await).contains(&"dash-server__plain".to_string()));

        // re-store the same tokens sealed: same behaviour, and the row no longer shows the token
        let sealed = crate::token_crypto::seal(&json!({ "access_token": TOKEN, "token_type": "Bearer" }).to_string());
        assert!(sealed.starts_with("enc:v1:"), "test key is configured");
        assert!(!sealed.contains(TOKEN));
        state
            .db
            .connect()
            .unwrap()
            .execute("UPDATE mcp_oauth_sessions SET tokens = ?1 WHERE id = 's1'", params![sealed])
            .unwrap();
        let before = server.count();
        assert!(tool_names(&rpc(&state, "user-1", "tools/list", json!({})).await).contains(&"dash-server__plain".to_string()));
        assert!(server.count() > before);

        // a sealed row that cannot be opened is unusable, not a plaintext fallback
        state
            .db
            .connect()
            .unwrap()
            .execute("UPDATE mcp_oauth_sessions SET tokens = 'enc:v1:AAAA:AAAA' WHERE id = 's1'", [])
            .unwrap();
        assert!(tool_names(&rpc(&state, "user-1", "tools/list", json!({})).await).is_empty());
    }

    #[tokio::test]
    async fn legacy_plaintext_secrets_can_be_sealed_in_place_idempotently() {
        let (state, _server, _) = fixture().await;
        let conn = state.db.connect().unwrap();
        conn.execute("UPDATE mcp_connectors SET oauth_client_secret = 'plain-secret' WHERE id = 'conn-1'", []).unwrap();
        let sealed = crate::mcp_routes::seal_legacy_mcp_secrets(&conn).unwrap();
        assert_eq!(sealed, 2, "one token row and one client secret");
        let (tokens, secret): (String, String) = conn
            .query_row(
                "SELECT (SELECT tokens FROM mcp_oauth_sessions WHERE id = 's1'), oauth_client_secret FROM mcp_connectors WHERE id = 'conn-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(tokens.starts_with("enc:v1:") && secret.starts_with("enc:v1:"));
        assert_eq!(crate::token_crypto::open(&secret), "plain-secret");
        assert_eq!(crate::mcp_routes::seal_legacy_mcp_secrets(&conn).unwrap(), 0, "second run finds nothing");
        // values marked `plain:` while no key was configured are sealed too
        conn.execute("UPDATE mcp_connectors SET oauth_client_secret = 'plain:later-secret' WHERE id = 'conn-1'", []).unwrap();
        assert_eq!(crate::mcp_routes::seal_legacy_mcp_secrets(&conn).unwrap(), 1);
        let secret: String =
            conn.query_row("SELECT oauth_client_secret FROM mcp_connectors WHERE id = 'conn-1'", [], |r| r.get(0)).unwrap();
        assert!(secret.starts_with("enc:v1:"));
        assert_eq!(crate::token_crypto::open(&secret), "later-secret");
        // and the connector still works
        assert!(tool_names(&rpc(&state, "user-1", "tools/list", json!({})).await).contains(&"dash-server__plain".to_string()));
    }
}
