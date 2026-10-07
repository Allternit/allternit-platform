//! Public MCP edge: `mcp.allternit.com` (and `api.allternit.com`) for the
//! vendor-bot connector and the agents server.
//!
//! Claude and ChatGPT can't reach a user's Mac or cloud computer, so the
//! connector's URL terminates here:
//!
//! * `POST /mcp/bots/:vendorBotId` — the vendor-bot connector (scope `bots:act`,
//!   `aud` = `<MCP_PUBLIC_URL>/bots/<id>`).
//! * `POST /mcp/server` and `POST /mcp` — the read-only agents server (scope
//!   `agents:read`, `aud` = `MCP_PUBLIC_URL`).
//! * `GET /.well-known/oauth-protected-resource[/mcp/bots/<id>]` — RFC 9728
//!   metadata, naming Clerk as the authorization server.
//!
//! The Clerk OAuth token is verified here (signature, issuer, expiry, `aud`,
//! scope). The owner's runtimes are then tried, connected ones first, and the
//! call is forwarded **synchronously** over the signed runtime relay
//! (`x-allternit-runtime-sig`, which also wakes a sleeping computer) to
//! allternit-api's `/webhooks/mcp-edge/*`, carrying the verified user in
//! `x-allternit-owner`. The OAuth token itself never leaves this process. A
//! runtime that doesn't hold the vendor bot answers 404 and the next one is
//! tried; if none does, the caller gets the same plain 404 (no oracle for
//! someone else's bot id). The whole attempt, wake included, gets 25 s; past
//! that, or with every computer offline, the caller gets a JSON-RPC error in
//! plain words.
//!
//! **MCP Events** (`events/list|subscribe|unsubscribe`) are answered here,
//! never relayed: the edge owns the subscriptions, knows the verified
//! principal and the owner's approvals, and delivery runs from the cloud
//! event backbone. See `routes::mcp_events`. `server/discover` (and a relayed
//! `initialize` answer) advertise `"events": {"listChanged": false}`.
//!
//! **Human page**: `GET /` on the MCP host, and `GET /mcp` from a browser
//! (`Accept: text/html`, no token), return a small self-contained "Allternit
//! MCP" page (server URLs, protocol versions, event catalog, how to connect).
//! JSON clients keep the behaviour above.
//!
//! Inert until `MCP_PUBLIC_URL` is set: every route answers 503
//! `{"error":"mcp_edge_not_configured"}`. `MCP_OAUTH_ISSUER` (default
//! `https://allternit.com/__clerk`) is advertised as the authorization server.
//! The host doesn't matter: `Host: mcp.allternit.com` uses the same paths.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use super::runtime_relay::{connected_runtime_ids, relay_signed_request_to_runtime_with, RelayRequest};
use crate::ApiState;

pub const BOT_SCOPE: &str = "bots:act";
pub const AGENTS_SCOPE: &str = "agents:read";
/// The one scope Clerk can issue that we accept (Clerk can't mint `bots:act` / `agents:read`).
/// A token carrying it is honoured only for a client its owner approved (`mcp_oauth_approvals`).
pub const CLERK_SCOPE: &str = "profile";
/// Everything, including waking a sleeping computer, must fit in this.
pub const BUDGET: Duration = Duration::from_secs(25);
const DEFAULT_ISSUER: &str = "https://allternit.com/__clerk";
const BOTS_RUNTIME_PATH: &str = "/webhooks/mcp-edge/bots";
const SERVER_RUNTIME_PATH: &str = "/webhooks/mcp-edge/server";
const CLIENT_HEADER: &str = "x-allternit-mcp-client";
/// How long a bot → runtime answer is trusted before the owner's runtimes are tried afresh.
const HOLDER_TTL: Duration = Duration::from_secs(600);
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

pub const OFFLINE_MESSAGE: &str = "Your Allternit computer is offline; it will be woken — try again in a minute.";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/", get(home_page))
        .route("/mcp/bots/:vendor_bot_id", get(bot_endpoint).post(bot_endpoint))
        .route("/mcp/server", get(agents_endpoint).post(agents_endpoint))
        .route("/mcp", get(agents_endpoint).post(agents_endpoint))
        .route("/.well-known/oauth-protected-resource", get(well_known_agents))
        .route("/.well-known/oauth-protected-resource/*rest", get(well_known_at))
}

// ─── configuration and URLs ───────────────────────────────────────────────────

/// The agents server's public URL (`MCP_PUBLIC_URL`, e.g.
/// `https://mcp.allternit.com/mcp`) — also the base the bot URLs hang off.
/// `None` = the edge is off.
fn public_mcp_url() -> Option<String> {
    std::env::var("MCP_PUBLIC_URL").ok().map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| !v.is_empty())
}

/// The edge's public base for the CLI-key routes (`vendor_bot_keys`); `None` = the edge is off.
pub fn public_mcp_url_for_keys() -> Option<String> {
    public_mcp_url()
}

fn oauth_issuer() -> String {
    std::env::var("MCP_OAUTH_ISSUER").ok().map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT_ISSUER.to_string())
}

/// RFC 9728 §3.1: the well-known path goes between the origin and the resource path.
fn resource_metadata_url(resource: &str) -> String {
    let (origin, path) = match resource.find("://") {
        Some(i) => match resource[i + 3..].find('/') {
            Some(j) => resource.split_at(i + 3 + j),
            None => (resource, ""),
        },
        None => (resource, ""),
    };
    format!("{origin}/.well-known/oauth-protected-resource{}", path.trim_end_matches('/'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Bot(String),
    Agents,
}

impl Target {
    fn resource(&self, base: &str) -> String {
        match self {
            Target::Bot(id) => format!("{base}/bots/{id}"),
            Target::Agents => base.to_string(),
        }
    }
    /// `mcp_oauth_approvals.target`.
    fn approval_target(&self) -> String {
        match self {
            Target::Bot(id) => format!("bot:{id}"),
            Target::Agents => "agents".to_string(),
        }
    }
    fn scope(&self) -> &'static str {
        match self {
            Target::Bot(_) => BOT_SCOPE,
            Target::Agents => AGENTS_SCOPE,
        }
    }
    fn runtime_path(&self) -> String {
        match self {
            Target::Bot(id) => format!("{BOTS_RUNTIME_PATH}/{id}"),
            Target::Agents => SERVER_RUNTIME_PATH.to_string(),
        }
    }
}

/// The bot id in a `.well-known` suffix like `mcp/bots/<id>`.
fn bot_id_from_resource_path(rest: &str) -> Option<&str> {
    let id = rest.trim_matches('/').rsplit_once("bots/").map(|(_, id)| id)?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

fn metadata(target: &Target, base: &str) -> Value {
    json!({
        "resource": target.resource(base),
        "authorization_servers": [oauth_issuer()],
        "scopes_supported": [CLERK_SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": match target { Target::Bot(_) => "Allternit vendor bot", Target::Agents => "Allternit Agents" }
    })
}

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "mcp_edge_not_configured" }))).into_response()
}

// ─── token checks ─────────────────────────────────────────────────────────────

fn claim_has_audience(claims: &Value, resource: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_string();
    match claims.get("aud") {
        Some(Value::String(s)) => norm(s) == norm(resource),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).any(|s| norm(s) == norm(resource)),
        _ => false,
    }
}

fn claim_scopes(claims: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["scope", "scp"] {
        match claims.get(key) {
            Some(Value::String(s)) => out.extend(s.split_whitespace().map(str::to_string)),
            Some(Value::Array(a)) => out.extend(a.iter().filter_map(Value::as_str).map(str::to_string)),
            _ => {}
        }
    }
    out
}

fn client_label(claims: &Value) -> String {
    ["azp", "client_id", "cid"].iter().find_map(|k| claims[k].as_str()).filter(|c| !c.is_empty()).unwrap_or("oauth-client").to_string()
}

fn challenge(resource: &str, error: Option<(&str, &str)>) -> HeaderValue {
    let meta = resource_metadata_url(resource);
    let v = match error {
        Some((code, desc)) => format!("Bearer error=\"{code}\", error_description=\"{desc}\", resource_metadata=\"{meta}\""),
        None => format!("Bearer resource_metadata=\"{meta}\""),
    };
    HeaderValue::from_str(&v).unwrap_or_else(|_| HeaderValue::from_static("Bearer"))
}

fn refusal(resource: &str, status: StatusCode, body: Value, error: Option<(&str, &str)>) -> Response {
    let mut resp = (status, Json(body)).into_response();
    resp.headers_mut().insert(header::WWW_AUTHENTICATE, challenge(resource, error));
    resp
}

struct Caller {
    user_id: String,
    client: String,
    /// A Clerk-scoped token: only valid once the owner approved `client` for this target.
    needs_approval: bool,
}

/// The 403 for an unapproved client, with where the owner approves it.
fn approval_required(resource: &str, target: &Target, client: &str) -> Response {
    let url = approve_url(target, client);
    let msg = format!("The owner has not approved this app ({client}) for this connector. Approve it here, then retry: {url}");
    refusal(
        resource,
        StatusCode::FORBIDDEN,
        json!({ "error": "approval_required", "client": client, "target": target.approval_target(), "approve_url": url, "message": msg }),
        Some(("insufficient_scope", &msg.replace('"', "'"))),
    )
}

/// Where the owner approves a client: `MCP_APPROVE_URL` (default the ai app's approve page).
fn approve_url(target: &Target, client: &str) -> String {
    let base = std::env::var("MCP_APPROVE_URL").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| "https://ai.allternit.com/mcp/approve".to_string());
    let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
    format!("{base}?client={}&target={}", enc(client), enc(&target.approval_target()))
}

/// The caller behind a bearer token, or a ready 401/403 with the OAuth challenge.
fn check_claims(claims: &Value, target: &Target, base: &str) -> Result<Caller, Response> {
    let resource = target.resource(base);
    let invalid = |msg: &str| refusal(&resource, StatusCode::UNAUTHORIZED, json!({ "error": "invalid_token", "message": msg }), Some(("invalid_token", "The access token is invalid")));
    let scopes = claim_scopes(claims);
    let own_scope = claim_has_audience(claims, &resource) && scopes.iter().any(|s| s == target.scope());
    if !own_scope && scopes.iter().any(|s| s == CLERK_SCOPE) {
        // A Clerk token: no `aud` for us, and the client must be named so the owner's approval can bind to it.
        let (Some(user_id), Some(client)) = (claims.get("sub").and_then(Value::as_str).filter(|s| !s.is_empty()), ["azp", "client_id", "cid"].iter().find_map(|k| claims[k].as_str()).filter(|c| !c.is_empty())) else {
            return Err(invalid("Token has no subject or client"));
        };
        return Ok(Caller { user_id: user_id.to_string(), client: client.to_string(), needs_approval: true });
    }
    if !claim_has_audience(claims, &resource) {
        return Err(invalid("Invalid audience"));
    }
    if !scopes.iter().any(|s| s == target.scope()) {
        return Err(refusal(&resource, StatusCode::FORBIDDEN, json!({ "error": "insufficient_scope", "scope": target.scope() }), Some(("insufficient_scope", &format!("{} is required", target.scope())))));
    }
    let Some(user_id) = claims.get("sub").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return Err(invalid("Token has no subject"));
    };
    Ok(Caller { user_id: user_id.to_string(), client: client_label(claims), needs_approval: false })
}

// ─── backend seam ─────────────────────────────────────────────────────────────

/// Why a forward didn't produce an answer.
#[derive(Debug, PartialEq, Eq)]
pub enum Unreached {
    /// Not connected and can't be woken.
    Offline,
    /// A wake was issued but the computer isn't back yet.
    Warming,
    Timeout,
    Other(String),
}

#[async_trait::async_trait]
pub trait EdgeBackend: Send + Sync {
    /// Verified claims of an OAuth token (signature, issuer, expiry).
    async fn verify(&self, token: &str) -> Result<Value, String>;
    /// The user's runtimes, best candidate first (connected ones, then most recently seen).
    async fn runtimes(&self, user_id: &str) -> Result<Vec<String>, String>;
    /// One signed, synchronous call to `path` on the runtime, as `user_id`.
    async fn forward(&self, user_id: &str, runtime_id: &str, path: &str, client: &str, body: &[u8]) -> Result<(u16, Vec<u8>), Unreached>;
    /// `true` = `user_id` approved OAuth `client` for `target` (`bot:<id>` / `agents`).
    async fn is_approved(&self, user_id: &str, client: &str, target: &str) -> Result<bool, String>;
    /// `(owner, key id)` when `token` is a live `allternit-bot` CLI key (`abk_…`) for exactly this vendor bot.
    async fn verify_cli_key(&self, _token: &str, _vendor_bot_id: &str) -> Result<Option<(String, String)>, String> {
        Ok(None)
    }
    /// Where `events/*` subscriptions live; `None` = events unavailable.
    fn events_store(&self) -> Option<&dyn crate::routes::mcp_events::EventsStore> {
        None
    }
    /// A live Platform API project key (`alt_live_…` / `alt_test_…`) on a project
    /// the Platform API is switched on for; `None` = invalid, revoked or switched off.
    async fn verify_project_key(&self, _token: &str) -> Result<Option<crate::routes::platform_v1::PlatformCaller>, String> {
        Ok(None)
    }
}

/// A Platform API project key on the agents server (spec §4 MCP): the project's
/// hosted runtime answers, as the project's runtime owner. Needs the `agents`
/// scope. A key bound to one account is refused (fail closed): the agents server
/// lists every agent of the project, so it would reach other accounts' agents.
fn project_key_caller(target: &Target, resource: &str, key: &crate::routes::platform_v1::PlatformCaller) -> Result<Caller, Response> {
    let refuse = |status: StatusCode, code: &str, msg: &str| refusal(resource, status, json!({ "error": code, "message": msg }), None);
    if *target != Target::Agents {
        return Err(refuse(StatusCode::FORBIDDEN, "insufficient_scope", "A project API key opens the agents server (/mcp) only."));
    }
    if !key.has_scope("agents") {
        return Err(refuse(StatusCode::FORBIDDEN, "insufficient_scope", "This API key lacks the 'agents' scope."));
    }
    if key.account_id.is_some() {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "account_bound_key",
            "This API key is bound to one account; the MCP agents server needs a project-wide key with the 'agents' scope.",
        ));
    }
    Ok(Caller { user_id: crate::routes::platform_v1::hosting::runtime_owner(&key.project_id), client: format!("platform-key:{}", key.key_id), needs_approval: false })
}

/// Which of the owner's runtimes answered for a bot last time.
fn holders() -> &'static Mutex<HashMap<(String, String), (String, Instant)>> {
    static HOLDERS: OnceLock<Mutex<HashMap<(String, String), (String, Instant)>>> = OnceLock::new();
    HOLDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remembered_holder(user_id: &str, bot_id: &str) -> Option<String> {
    let map = holders().lock().unwrap_or_else(|e| e.into_inner());
    map.get(&(user_id.to_string(), bot_id.to_string())).filter(|(_, at)| at.elapsed() < HOLDER_TTL).map(|(r, _)| r.clone())
}

fn remember_holder(user_id: &str, bot_id: &str, runtime_id: &str) {
    holders().lock().unwrap_or_else(|e| e.into_inner()).insert((user_id.to_string(), bot_id.to_string()), (runtime_id.to_string(), Instant::now()));
}

fn forget_holder(user_id: &str, bot_id: &str) {
    holders().lock().unwrap_or_else(|e| e.into_inner()).remove(&(user_id.to_string(), bot_id.to_string()));
}

// ─── core ─────────────────────────────────────────────────────────────────────

/// A reply the edge answers itself, with the Streamable HTTP status its era
/// calls for (`mcp_protocol::http_status`: 400 version/header errors, 404 an
/// unknown method on a modern request, else 200) — the same rule the
/// runtime servers apply.
fn rpc_reply(era: &mcp_protocol::Era, reply: Value) -> Response {
    let status = StatusCode::from_u16(mcp_protocol::http_status(era.is_modern(), &reply)).unwrap_or(StatusCode::OK);
    (status, Json(reply)).into_response()
}

fn rpc_error(id: Value, message: &str) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })).into_response()
}

fn is_bot_not_found(status: u16, body: &[u8]) -> bool {
    status == 404 && serde_json::from_slice::<Value>(body).ok().and_then(|v| v["error"].as_str().map(|e| e == "not_found")).unwrap_or(false)
}

enum Outcome {
    Answer(u16, Vec<u8>),
    /// Every reachable runtime said it doesn't hold the bot.
    NotFound,
    Unreachable,
}

async fn deliver(backend: &dyn EdgeBackend, target: &Target, caller: &Caller, body: &[u8]) -> Outcome {
    let runtimes = match backend.runtimes(&caller.user_id).await {
        Ok(r) => r,
        Err(error) => {
            tracing::warn!("mcp edge: listing runtimes failed: {error}");
            return Outcome::Unreachable;
        }
    };
    let mut order = runtimes;
    if let Target::Bot(bot) = target {
        if let Some(known) = remembered_holder(&caller.user_id, bot) {
            order.retain(|r| *r != known);
            order.insert(0, known);
        }
    }
    // The agents server reads whichever computer is best; only a bot has to be found.
    if matches!(target, Target::Agents) {
        order.truncate(1);
    }
    if order.is_empty() {
        return Outcome::NotFound;
    }
    let path = target.runtime_path();
    let mut missed_some = false;
    for runtime_id in &order {
        match backend.forward(&caller.user_id, runtime_id, &path, &caller.client, body).await {
            Ok((status, reply)) => {
                if let Target::Bot(bot) = target {
                    if is_bot_not_found(status, &reply) {
                        forget_holder(&caller.user_id, bot);
                        continue;
                    }
                    if (200..300).contains(&status) {
                        remember_holder(&caller.user_id, bot, runtime_id);
                    }
                }
                return Outcome::Answer(status, reply);
            }
            Err(why) => {
                tracing::info!(runtime = %runtime_id, "mcp edge: runtime not reached: {why:?}");
                missed_some = true;
            }
        }
    }
    if missed_some {
        Outcome::Unreachable
    } else {
        Outcome::NotFound
    }
}

async fn serve(backend: &dyn EdgeBackend, base: &str, target: Target, method: &Method, headers: &HeaderMap, body: Bytes) -> Response {
    let resource = target.resource(base);
    let Some(token) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim).filter(|t| !t.is_empty()) else {
        return refusal(&resource, StatusCode::UNAUTHORIZED, json!({ "error": "unauthorized" }), None);
    };
    let caller = if crate::routes::platform_v1::caller::is_project_key_token(token) {
        match backend.verify_project_key(token).await {
            Ok(Some(key)) => match project_key_caller(&target, &resource, &key) {
                Ok(c) => c,
                Err(resp) => return resp,
            },
            Ok(None) => return refusal(&resource, StatusCode::UNAUTHORIZED, json!({ "error": "invalid_token", "message": "Invalid or revoked API key" }), Some(("invalid_token", "The key is invalid"))),
            Err(error) => {
                tracing::warn!("mcp edge: project key lookup failed: {error}");
                return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "unavailable" }))).into_response();
            }
        }
    } else if token.starts_with(crate::routes::vendor_bot_keys::KEY_PREFIX) {
        // An `allternit-bot` CLI key opens one vendor bot and nothing else.
        let invalid = |msg: &str| refusal(&resource, StatusCode::UNAUTHORIZED, json!({ "error": "invalid_token", "message": msg }), Some(("invalid_token", "The key is invalid")));
        let Target::Bot(bot) = &target else { return invalid("This key opens one vendor bot only") };
        match backend.verify_cli_key(token, bot).await {
            Ok(Some((user_id, key_id))) => Caller { user_id, client: format!("cli-key:{key_id}"), needs_approval: false },
            Ok(None) => return invalid("Invalid or revoked key"),
            Err(error) => {
                tracing::warn!("mcp edge: cli key lookup failed: {error}");
                return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "unavailable" }))).into_response();
            }
        }
    } else {
        let claims = match backend.verify(token).await {
            Ok(c) => c,
            Err(msg) => return refusal(&resource, StatusCode::UNAUTHORIZED, json!({ "error": "invalid_token", "message": msg }), Some(("invalid_token", "The access token is invalid"))),
        };
        match check_claims(&claims, &target, base) {
            Ok(c) => c,
            Err(resp) => return resp,
        }
    };
    if caller.needs_approval {
        match backend.is_approved(&caller.user_id, &caller.client, &target.approval_target()).await {
            Ok(true) => {}
            Ok(false) => return approval_required(&resource, &target, &caller.client),
            Err(error) => {
                tracing::warn!("mcp edge: approval lookup failed: {error}");
                return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "unavailable" }))).into_response();
            }
        }
    }
    if *method != Method::POST {
        let mut resp = (StatusCode::METHOD_NOT_ALLOWED, Json(json!({ "error": "method_not_allowed" }))).into_response();
        resp.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
        return resp;
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response(),
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let rpc_method = request["method"].as_str().unwrap_or_default();
    let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(err) = mcp_protocol::check_headers(&id, rpc_method, &request["params"], h("mcp-method"), h("mcp-name")) {
        return (StatusCode::BAD_REQUEST, Json(err)).into_response();
    }
    let spec = edge_spec(&target);
    let era = mcp_protocol::Era::of(rpc_method, &request["params"], h("mcp-protocol-version"));
    // `server/discover` is static: answer it here so a modern client learns
    // versions, capabilities and instructions without waking the computer.
    if rpc_method == "server/discover" {
        if let Some(reply) = mcp_protocol::preflight(&spec, &era, &id, rpc_method) {
            return rpc_reply(&era, reply);
        }
    }
    // MCP Events: the edge owns subscriptions; nothing goes to the runtime.
    if rpc_method.starts_with("events/") {
        if let Some(reply) = mcp_protocol::preflight(&spec, &era, &id, rpc_method) {
            return rpc_reply(&era, reply);
        }
        let Some(store) = backend.events_store() else {
            return rpc_reply(&era, mcp_protocol::rpc_err(&id, mcp_protocol::codes::METHOD_NOT_FOUND, "Events are not available"));
        };
        let principal = crate::routes::mcp_events::Principal { user_id: caller.user_id.clone(), client: caller.client.clone(), target: target.approval_target() };
        let reply = crate::routes::mcp_events::handle(store, &principal, &id, rpc_method, &request["params"], chrono::Utc::now()).await;
        return rpc_reply(&era, mcp_protocol::finish(&spec, &era, rpc_method, reply));
    }

    let outcome = match tokio::time::timeout(BUDGET, deliver(backend, &target, &caller, &body)).await {
        Ok(o) => o,
        Err(_) => Outcome::Unreachable,
    };
    match outcome {
        Outcome::Unreachable => rpc_error(id, OFFLINE_MESSAGE),
        Outcome::NotFound => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Outcome::Answer(status, reply) => {
            let reply = if rpc_method == "initialize" && status == 200 { advertise_events(reply) } else { reply };
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut resp = Response::new(axum::body::Body::from(reply));
            *resp.status_mut() = status;
            resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            if status == StatusCode::UNAUTHORIZED {
                // The runtime refused (a revoked connection); the relay drops its headers.
                resp.headers_mut().insert(header::WWW_AUTHENTICATE, challenge(&resource, Some(("invalid_token", "This connection was revoked"))));
            }
            resp
        }
    }
}

/// The spec the edge answers `server/discover` (and `events/*` decoration) with.
fn edge_spec(target: &Target) -> mcp_protocol::ServerSpec {
    let spec = match target {
        Target::Bot(_) => mcp_protocol::servers::vendor_bot(env!("CARGO_PKG_VERSION")),
        Target::Agents => mcp_protocol::servers::agents(env!("CARGO_PKG_VERSION")),
    };
    mcp_protocol::servers::with_events(spec)
}

/// A relayed `initialize` answer comes from the runtime, which doesn't know
/// the edge serves events: add the capability so legacy-era clients see it too.
fn advertise_events(reply: Vec<u8>) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<Value>(&reply) else { return reply };
    match v.pointer_mut("/result/capabilities").and_then(Value::as_object_mut) {
        Some(caps) => {
            caps.insert("events".into(), json!({ "listChanged": false }));
            serde_json::to_vec(&v).unwrap_or(reply)
        }
        None => reply,
    }
}

// ─── human page ───────────────────────────────────────────────────────────────

fn wants_html(headers: &HeaderMap) -> bool {
    headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|a| a.contains("text/html"))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const DOCS_BASE: &str = "https://docs.allternit.com";

/// The "Allternit MCP" page: plain, self-contained (no external assets),
/// light/dark, readable without JavaScript.
pub fn home_html(base: &str) -> String {
    use crate::routes::allternit_events::{visible, Audience};
    let base = esc(base.trim_end_matches('/'));
    let versions = mcp_protocol::SUPPORTED.iter().map(|v| format!("<code>{v}</code>")).collect::<Vec<_>>().join(", ");
    let rows = |aud: Audience| {
        visible(aud)
            .map(|e| format!("<tr><td><code>{}</code></td><td>{}</td></tr>", esc(e.name), esc(e.description)))
            .collect::<String>()
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Allternit MCP</title>
<style>
:root{{--bg:#ffffff;--fg:#1a1a1a;--muted:#5c5c5c;--line:#e3e3e3;--code:#f4f4f4;--link:#0b57d0}}
@media (prefers-color-scheme: dark){{:root{{--bg:#141414;--fg:#ececec;--muted:#a8a8a8;--line:#2e2e2e;--code:#1f1f1f;--link:#8ab4f8}}}}
*{{box-sizing:border-box}}body{{margin:0;background:var(--bg);color:var(--fg);font:16px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif}}
main{{max-width:760px;margin:0 auto;padding:32px 16px 64px}}h1{{font-size:1.7rem;margin:0 0 4px}}h2{{font-size:1.15rem;margin:32px 0 8px}}
p,li{{color:var(--fg)}}.muted{{color:var(--muted)}}a{{color:var(--link)}}code{{background:var(--code);padding:1px 5px;border-radius:4px;font-size:.92em;word-break:break-all}}
table{{width:100%;border-collapse:collapse;font-size:.95rem}}th,td{{text-align:left;vertical-align:top;padding:8px 6px;border-bottom:1px solid var(--line)}}
th{{color:var(--muted);font-weight:600}}ol{{padding-left:20px}}
</style></head>
<body><main>
<h1>Allternit MCP</h1>
<p class="muted">Model Context Protocol servers for your Allternit agents. Connect them from ChatGPT, Claude, or any MCP client.</p>

<h2>Servers</h2>
<table><thead><tr><th scope="col">URL</th><th scope="col">What it is</th></tr></thead><tbody>
<tr><td><code>{base}</code></td><td>Agents server: read your agents and their runs. OAuth scope <code>agents:read</code>.</td></tr>
<tr><td><code>{base}/bots/&lt;bot id&gt;</code></td><td>Vendor bot connector: act through one bot. OAuth scope <code>bots:act</code>.</td></tr>
</tbody></table>
<p>Protocol versions: {versions}. Streamable HTTP, <code>POST</code> JSON-RPC. Modern clients can call <code>server/discover</code>.</p>

<h2>How to connect</h2>
<ol>
<li><strong>ChatGPT</strong>: Settings, Apps &amp; Connectors, add a custom connector with the server URL above, then sign in with your Allternit account.</li>
<li><strong>Claude</strong>: Settings, Connectors, add custom connector with the server URL, then sign in.</li>
<li>The first time an app connects, approve it in Allternit when asked. You can remove that approval at any time, which also ends its event subscriptions.</li>
</ol>

<h2>Events</h2>
<p>Both servers support MCP Events (<code>events/list</code>, <code>events/subscribe</code>, <code>events/unsubscribe</code>) with signed webhook delivery (Standard Webhooks). Subscriptions last 24 hours by default and up to 7 days; refresh them before <code>refreshBefore</code>.</p>
<h3 class="muted" style="font-size:1rem">Agents server</h3>
<table><thead><tr><th scope="col">Event</th><th scope="col">When</th></tr></thead><tbody>{agents}</tbody></table>
<h3 class="muted" style="font-size:1rem">Vendor bot connector</h3>
<table><thead><tr><th scope="col">Event</th><th scope="col">When</th></tr></thead><tbody>{bot}</tbody></table>

<h2>Docs</h2>
<ul>
<li><a href="{docs}/api/mcp-servers">MCP servers reference</a></li>
<li><a href="{docs}/guides/mcp-events">MCP Events guide</a></li>
</ul>
</main></body></html>
"#,
        agents = rows(Audience::Agents),
        bot = rows(Audience::Bot),
        docs = DOCS_BASE,
    )
}

fn html_response(body: String) -> Response {
    let mut resp = Response::new(axum::body::Body::from(body));
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"));
    resp
}

/// The MCP host's base (`mcp.allternit.com`), from `MCP_PUBLIC_URL`.
fn public_host(base: &str) -> Option<String> {
    reqwest::Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase))
}

/// `GET /`: the human page, only on the MCP host (api.allternit.com's root is untouched).
async fn home_page(headers: HeaderMap) -> Response {
    let Some(base) = public_mcp_url() else { return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response() };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).map(|h| h.split(':').next().unwrap_or(h).to_ascii_lowercase());
    if host.is_none() || host != public_host(&base) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    }
    html_response(home_html(&base))
}

// ─── handlers ─────────────────────────────────────────────────────────────────

async fn bot_endpoint(State(state): State<Arc<ApiState>>, Path(vendor_bot_id): Path<String>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let Some(base) = public_mcp_url() else { return not_configured() };
    serve(&ProdBackend::new(&state), &base, Target::Bot(vendor_bot_id), &method, &headers, body).await
}

async fn agents_endpoint(State(state): State<Arc<ApiState>>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let Some(base) = public_mcp_url() else { return not_configured() };
    // A browser opening the server URL gets the human page, not a 401.
    if method == Method::GET && wants_html(&headers) && !headers.contains_key(header::AUTHORIZATION) {
        return html_response(home_html(&base));
    }
    serve(&ProdBackend::new(&state), &base, Target::Agents, &method, &headers, body).await
}

async fn well_known_agents() -> Response {
    match public_mcp_url() {
        Some(base) => Json(metadata(&Target::Agents, &base)).into_response(),
        None => not_configured(),
    }
}

/// `…/oauth-protected-resource/mcp/bots/<id>` describes that connector; every
/// other suffix describes the agents server. Nothing says whether the bot exists.
async fn well_known_at(Path(rest): Path<String>) -> Response {
    let Some(base) = public_mcp_url() else { return not_configured() };
    let target = bot_id_from_resource_path(&rest).map(|id| Target::Bot(id.to_string())).unwrap_or(Target::Agents);
    Json(metadata(&target, &base)).into_response()
}

// ─── production backend ───────────────────────────────────────────────────────

struct ProdBackend<'a> {
    state: &'a ApiState,
    events: crate::routes::mcp_events::PgEventsStore<'a>,
}

impl<'a> ProdBackend<'a> {
    fn new(state: &'a ApiState) -> Self {
        Self { state, events: crate::routes::mcp_events::PgEventsStore { db: &state.db } }
    }
}

#[async_trait::async_trait]
impl EdgeBackend for ProdBackend<'_> {
    fn events_store(&self) -> Option<&dyn crate::routes::mcp_events::EventsStore> {
        Some(&self.events)
    }

    async fn verify_cli_key(&self, token: &str, vendor_bot_id: &str) -> Result<Option<(String, String)>, String> {
        crate::routes::vendor_bot_keys::verify_key(&self.state.db, token, vendor_bot_id).await.map_err(|e| e.to_string())
    }

    async fn verify_project_key(&self, token: &str) -> Result<Option<crate::routes::platform_v1::PlatformCaller>, String> {
        use crate::routes::platform_v1 as p;
        match p::caller::authenticate(&self.state.db, Some(token)).await {
            // The same switch as `/v1`: on for everyone, or for the beta owners only.
            Ok(c) if p::platform_api_enabled() || p::beta_owners_from_env().contains(&c.owner_user_id) => Ok(Some(c)),
            Ok(_) => Ok(None),
            Err(e) if e.status == StatusCode::UNAUTHORIZED => Ok(None),
            Err(e) => Err(e.message),
        }
    }

    async fn is_approved(&self, user_id: &str, client: &str, target: &str) -> Result<bool, String> {
        crate::routes::mcp_oauth_approvals::is_approved(&self.state.db, user_id, client, target).await.map_err(|e| e.to_string())
    }

    async fn verify(&self, token: &str) -> Result<Value, String> {
        crate::auth::clerk::verified_claims(token).await.map_err(|e| e.to_string())
    }

    async fn runtimes(&self, user_id: &str) -> Result<Vec<String>, String> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL AND credential_expires_at > CURRENT_TIMESTAMP ORDER BY last_seen_at DESC NULLS LAST, created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| e.to_string())?;
        let connected = connected_runtime_ids().await;
        let (mut live, asleep): (Vec<String>, Vec<String>) = ids.into_iter().partition(|id| connected.contains(id));
        live.extend(asleep);
        Ok(live)
    }

    async fn forward(&self, user_id: &str, runtime_id: &str, path: &str, client: &str, body: &[u8]) -> Result<(u16, Vec<u8>), Unreached> {
        // Only a content type goes down: the caller's OAuth token stays here.
        let response = relay_signed_request_to_runtime_with(
            &self.state.db,
            &self.state.contabo_runtime_service,
            &self.state.quota_service,
            &self.state.provisioning_service,
            user_id,
            runtime_id,
            RelayRequest {
                method: "POST".to_string(),
                path: path.to_string(),
                headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
                body: STANDARD.encode(body),
                body_encoding: "base64".to_string(),
            },
            &["content-type"],
            HashMap::from([(CLIENT_HEADER.to_string(), client.to_string())]),
        )
        .await
        .map_err(|e| Unreached::Other(e.to_string()))?;
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), MAX_REPLY_BYTES).await.map_err(|e| Unreached::Other(e.to_string()))?.to_vec();
        // The relay's own refusals (as opposed to the runtime's answers).
        if status == 503 || status == 504 {
            let error = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["error"].as_str().map(str::to_string));
            match error.as_deref() {
                Some("runtime_offline") => return Err(Unreached::Offline),
                Some("runtime_warming") => return Err(Unreached::Warming),
                Some("runtime_timeout") => return Err(Unreached::Timeout),
                _ => {}
            }
        }
        Ok((status, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const BASE: &str = "https://mcp.allternit.com/mcp";

    fn claims(aud: &str, scope: &str, sub: &str) -> Value {
        json!({ "iss": DEFAULT_ISSUER, "sub": sub, "aud": aud, "scope": scope, "azp": "claude-connector" })
    }

    /// Tokens are `tok:<n>` → a canned claims value; everything else is invalid.
    struct Fake {
        tokens: HashMap<String, Value>,
        runtimes: Vec<String>,
        /// runtime id → what forwarding to it yields.
        replies: Mutex<HashMap<String, Result<(u16, String), Unreached>>>,
        calls: Mutex<Vec<(String, String, String, String, Vec<u8>)>>,
        verified: AtomicUsize,
        /// CLI key → (vendor bot, owner, key id).
        cli_keys: HashMap<String, (String, String, String)>,
        /// (owner, client, target) the owner approved.
        approvals: Vec<(String, String, String)>,
        events: crate::routes::mcp_events::testing::MemStore,
        project_keys: HashMap<String, crate::routes::platform_v1::PlatformCaller>,
    }

    impl Fake {
        fn new(runtimes: &[&str]) -> Self {
            Self { tokens: HashMap::new(), runtimes: runtimes.iter().map(|r| r.to_string()).collect(), replies: Mutex::new(HashMap::new()), calls: Mutex::new(vec![]), verified: AtomicUsize::new(0), cli_keys: HashMap::new(), approvals: vec![], events: Default::default(), project_keys: HashMap::new() }
        }
        fn approved(mut self, user: &str, client: &str, target: &str) -> Self {
            self.approvals.push((user.into(), client.into(), target.into()));
            self
        }
        fn token(mut self, t: &str, c: Value) -> Self {
            self.tokens.insert(t.to_string(), c);
            self
        }
        fn cli_key(mut self, key: &str, bot: &str, owner: &str, id: &str) -> Self {
            self.cli_keys.insert(key.to_string(), (bot.to_string(), owner.to_string(), id.to_string()));
            self
        }
        fn project_key(mut self, key: &str, project: &str, account: Option<&str>, scopes: &[&str]) -> Self {
            let c = crate::routes::platform_v1::PlatformCaller {
                project_id: project.into(),
                project_env: crate::routes::platform_v1::ProjectEnv::Sandbox,
                account_id: account.map(str::to_string),
                key_id: format!("ak_{project}"),
                scopes: scopes.iter().map(|s| s.to_string()).collect(),
                owner_user_id: "dev".into(),
                org_id: None,
                plan: crate::routes::platform_v1::caller::Plan::Sandbox,
                rpm_override: None,
                call_cap_override: None,
            };
            self.project_keys.insert(key.to_string(), c);
            self
        }
        fn reply(self, runtime: &str, r: Result<(u16, &str), Unreached>) -> Self {
            self.replies.lock().unwrap().insert(runtime.to_string(), r.map(|(s, b)| (s, b.to_string())));
            self
        }
        fn paths(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().iter().map(|c| (c.1.clone(), c.2.clone())).collect()
        }
    }

    #[async_trait::async_trait]
    impl EdgeBackend for Fake {
        fn events_store(&self) -> Option<&dyn crate::routes::mcp_events::EventsStore> {
            Some(&self.events)
        }
        async fn verify(&self, token: &str) -> Result<Value, String> {
            self.verified.fetch_add(1, Ordering::SeqCst);
            self.tokens.get(token).cloned().ok_or_else(|| "Invalid Clerk signature".to_string())
        }
        async fn is_approved(&self, user_id: &str, client: &str, target: &str) -> Result<bool, String> {
            Ok(self.approvals.contains(&(user_id.to_string(), client.to_string(), target.to_string())))
        }
        async fn verify_cli_key(&self, token: &str, vendor_bot_id: &str) -> Result<Option<(String, String)>, String> {
            Ok(self.cli_keys.get(token).filter(|k| k.0 == vendor_bot_id).map(|k| (k.1.clone(), k.2.clone())))
        }
        async fn verify_project_key(&self, token: &str) -> Result<Option<crate::routes::platform_v1::PlatformCaller>, String> {
            Ok(self.project_keys.get(token).cloned())
        }
        async fn runtimes(&self, _user: &str) -> Result<Vec<String>, String> {
            Ok(self.runtimes.clone())
        }
        async fn forward(&self, user_id: &str, runtime_id: &str, path: &str, client: &str, body: &[u8]) -> Result<(u16, Vec<u8>), Unreached> {
            self.calls.lock().unwrap().push((user_id.into(), runtime_id.into(), path.into(), client.into(), body.to_vec()));
            match self.replies.lock().unwrap().remove(runtime_id) {
                Some(Ok((s, b))) => Ok((s, b.into_bytes())),
                Some(Err(e)) => Err(e),
                None => Ok((200, r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_string().into_bytes())),
            }
        }
    }

    fn bearer(t: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, format!("Bearer {t}").parse().unwrap());
        h
    }

    const LIST: &str = r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#;

    async fn post(f: &Fake, target: Target, headers: &HeaderMap, body: &str) -> (StatusCode, HeaderMap, Value) {
        let resp = serve(f, BASE, target, &Method::POST, headers, Bytes::from(body.to_string())).await;
        let (parts, body) = resp.into_parts();
        let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        (parts.status, parts.headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn bot(id: &str) -> Target {
        Target::Bot(id.to_string())
    }

    // ── Clerk tokens (scope `profile`) + owner approval ──────────────────────

    fn clerk_claims(sub: &str, client: Option<&str>) -> Value {
        let mut c = json!({ "iss": DEFAULT_ISSUER, "sub": sub, "scope": "openid profile" });
        if let Some(client) = client {
            c["client_id"] = json!(client);
        }
        c
    }

    #[tokio::test]
    async fn a_clerk_token_is_refused_with_an_approve_link_until_the_owner_approves_that_client_for_that_bot() {
        let f = Fake::new(&["rt1"]).token("tok", clerk_claims("user_c", Some("client-abc")));
        let (status, headers, body) = post(&f, bot("b1"), &bearer("tok"), LIST).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"], "approval_required");
        assert!(body["message"].as_str().unwrap().contains("client=client-abc&target=bot:b1"), "{body}");
        assert!(headers.contains_key(header::WWW_AUTHENTICATE));
        assert!(f.calls.lock().unwrap().is_empty(), "nothing is forwarded before approval");

        let f = Fake::new(&["rt1"]).token("tok", clerk_claims("user_c", Some("client-abc"))).approved("user_c", "client-abc", "bot:b1");
        let (status, _, _) = post(&f, bot("b1"), &bearer("tok"), LIST).await;
        assert_eq!(status, StatusCode::OK);
        let calls = f.calls.lock().unwrap();
        assert_eq!((calls[0].0.as_str(), calls[0].3.as_str()), ("user_c", "client-abc"));
    }

    #[tokio::test]
    async fn approval_does_not_carry_to_another_bot_client_owner_or_the_agents_server() {
        let approved = |f: Fake| f.approved("user_c", "client-abc", "bot:b1");
        for (target, sub, client) in [(bot("b2"), "user_c", "client-abc"), (bot("b1"), "user_c", "client-xyz"), (bot("b1"), "user_d", "client-abc"), (Target::Agents, "user_c", "client-abc")] {
            let f = approved(Fake::new(&["rt1"]).token("tok", clerk_claims(sub, Some(client))));
            let (status, _, body) = post(&f, target, &bearer("tok"), LIST).await;
            assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("approval_required")));
            assert!(f.calls.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn a_clerk_token_without_a_client_or_subject_is_invalid_and_agents_need_their_own_approval() {
        let f = Fake::new(&["rt1"]).token("noclient", clerk_claims("user_c", None)).token("agents", clerk_claims("user_c", Some("c1"))).approved("user_c", "c1", "agents");
        assert_eq!(post(&f, bot("b1"), &bearer("noclient"), LIST).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(post(&f, Target::Agents, &bearer("agents"), LIST).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_token_with_neither_our_scope_nor_profile_is_still_refused() {
        let f = Fake::new(&["rt1"]).token("t", json!({ "sub": "u", "scope": "openid email", "client_id": "c", "aud": "https://mcp.allternit.com/mcp/bots/b1" }));
        assert_eq!(post(&f, bot("b1"), &bearer("t"), LIST).await.0, StatusCode::FORBIDDEN);
    }

    // ── CLI keys ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_cli_key_for_this_bot_is_relayed_as_its_owner_with_a_key_client_label() {
        let f = Fake::new(&["rt1"]).cli_key("abk_good", "b-key-1", "user_k", "vbk_1");
        let (status, _, _) = post(&f, bot("b-key-1"), &bearer("abk_good"), LIST).await;
        assert_eq!(status, StatusCode::OK);
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].0.as_str(), calls[0].3.as_str()), ("user_k", "cli-key:vbk_1"));
        assert_eq!(f.verified.load(Ordering::SeqCst), 0, "a CLI key is never treated as a Clerk token");
    }

    #[tokio::test]
    async fn a_cli_key_for_another_bot_or_the_agents_server_or_an_unknown_key_is_refused_and_nothing_is_forwarded() {
        let f = Fake::new(&["rt1"]).cli_key("abk_good", "b-key-2", "user_k", "vbk_1");
        for (target, token) in [(bot("b-other"), "abk_good"), (Target::Agents, "abk_good"), (bot("b-key-2"), "abk_forged")] {
            let (status, headers, _) = post(&f, target, &bearer(token), LIST).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(headers.contains_key(header::WWW_AUTHENTICATE));
        }
        assert!(f.calls.lock().unwrap().is_empty());
    }

    // ── Platform API project keys (spec §4 MCP) ──────────────────────────────

    const PKEY: &str = "alt_test_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PKEY_BOUND: &str = "alt_test_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const PKEY_NOSCOPE: &str = "alt_test_cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    #[tokio::test]
    async fn a_project_key_with_the_agents_scope_reaches_the_projects_runtime_as_its_owner() {
        let f = Fake::new(&["rt1"]).project_key(PKEY, "proj_9", None, &["agents"]);
        let (status, _, _) = post(&f, Target::Agents, &bearer(PKEY), LIST).await;
        assert_eq!(status, StatusCode::OK);
        let calls = f.calls.lock().unwrap();
        assert_eq!((calls[0].0.as_str(), calls[0].3.as_str()), ("platform:proj_9", "platform-key:ak_proj_9"));
        assert_eq!(f.verified.load(Ordering::SeqCst), 0, "a project key is never treated as a Clerk token");
    }

    #[tokio::test]
    async fn project_keys_fail_closed_bound_unscoped_unknown_or_on_a_vendor_bot() {
        let f = Fake::new(&["rt1"])
            .project_key(PKEY, "proj_9", None, &["agents"])
            .project_key(PKEY_BOUND, "proj_9", Some("acct_a"), &["agents"])
            .project_key(PKEY_NOSCOPE, "proj_9", None, &["twin"]);
        let (s, _, b) = post(&f, Target::Agents, &bearer(PKEY_BOUND), LIST).await;
        assert_eq!((s, b["error"].as_str()), (StatusCode::FORBIDDEN, Some("account_bound_key")));
        let (s, _, b) = post(&f, Target::Agents, &bearer(PKEY_NOSCOPE), LIST).await;
        assert_eq!((s, b["error"].as_str()), (StatusCode::FORBIDDEN, Some("insufficient_scope")));
        let (s, _, _) = post(&f, bot("b-1"), &bearer(PKEY), LIST).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let unknown = format!("alt_test_{}", "d".repeat(64));
        let (s, _, b) = post(&f, Target::Agents, &bearer(&unknown), LIST).await;
        assert_eq!((s, b["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("invalid_token")));
        assert!(f.calls.lock().unwrap().is_empty(), "nothing reaches a runtime");
    }

    // ── auth at the edge ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn no_token_gets_the_oauth_challenge_and_nothing_is_forwarded() {
        let f = Fake::new(&["rt1"]);
        let (status, headers, _) = post(&f, bot("b-auth-1"), &HeaderMap::new(), LIST).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let challenge = headers[header::WWW_AUTHENTICATE].to_str().unwrap();
        assert!(challenge.contains("resource_metadata=\"https://mcp.allternit.com/.well-known/oauth-protected-resource/mcp/bots/b-auth-1\""), "{challenge}");
        assert!(f.paths().is_empty());
    }

    #[tokio::test]
    async fn a_forged_or_expired_token_is_refused() {
        let f = Fake::new(&["rt1"]);
        let (status, headers, body) = post(&f, bot("b-auth-2"), &bearer("not-a-real-token"), LIST).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::UNAUTHORIZED, Some("invalid_token")));
        assert!(headers.contains_key(header::WWW_AUTHENTICATE));
        assert!(f.paths().is_empty());
    }

    #[tokio::test]
    async fn the_token_must_be_for_this_bot_and_this_scope() {
        let f = Fake::new(&["rt1"])
            .token("other-bot", claims("https://mcp.allternit.com/mcp/bots/b-other", BOT_SCOPE, "user-a"))
            .token("agents-token", claims(BASE, AGENTS_SCOPE, "user-a"))
            .token("no-scope", claims("https://mcp.allternit.com/mcp/bots/b-auth-3", "agents:read", "user-a"))
            .token("array-aud", json!({ "iss": DEFAULT_ISSUER, "sub": "user-a", "aud": ["x", "https://mcp.allternit.com/mcp/bots/b-auth-3/"], "scp": ["bots:act"] }));
        for t in ["other-bot", "agents-token"] {
            let (status, headers, _) = post(&f, bot("b-auth-3"), &bearer(t), LIST).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{t}: wrong audience");
            assert!(headers.contains_key(header::WWW_AUTHENTICATE));
        }
        let (status, headers, body) = post(&f, bot("b-auth-3"), &bearer("no-scope"), LIST).await;
        assert_eq!((status, body["scope"].as_str()), (StatusCode::FORBIDDEN, Some("bots:act")));
        assert!(headers[header::WWW_AUTHENTICATE].to_str().unwrap().contains("insufficient_scope"));
        assert!(f.paths().is_empty(), "nothing reaches a runtime until the token is right");
        // aud as an array, scope as `scp`, trailing slash: accepted.
        assert_eq!(post(&f, bot("b-auth-3"), &bearer("array-aud"), LIST).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn edge_answered_errors_get_the_spec_http_status() {
        let f = Fake::new(&["rt1"]).token("agents", claims(BASE, AGENTS_SCOPE, "user-a"));
        let bad_version = r#"{"jsonrpc":"2.0","id":"d","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2099-01-01"}}}"#;
        let (status, body) = { let (s, _, b) = post(&f, Target::Agents, &bearer("agents"), bad_version).await; (s, b) };
        assert_eq!((status, body["error"]["code"].as_i64()), (StatusCode::BAD_REQUEST, Some(-32022)));
        let ok = r#"{"jsonrpc":"2.0","id":"d","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
        assert_eq!(post(&f, Target::Agents, &bearer("agents"), ok).await.0, StatusCode::OK);
        assert!(f.paths().is_empty());
    }

    #[tokio::test]
    async fn server_discover_is_answered_at_the_edge_without_a_runtime() {
        let f = Fake::new(&["rt1"]).token("agents", claims(BASE, AGENTS_SCOPE, "user-a")).token("bot", claims("https://mcp.allternit.com/mcp/bots/b-d", BOT_SCOPE, "user-a"));
        let discover = r#"{"jsonrpc":"2.0","id":"d","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
        let (status, _, body) = post(&f, Target::Agents, &bearer("agents"), discover).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["supportedVersions"][0], "2026-07-28");
        assert_eq!(body["result"]["instructions"], mcp_protocol::servers::AGENTS_INSTRUCTIONS);
        let (_, _, body) = post(&f, bot("b-d"), &bearer("bot"), discover).await;
        assert_eq!(body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "allternit-vendor-bot");
        assert!(f.paths().is_empty(), "discover never wakes a computer");
        // Still behind auth.
        assert_eq!(post(&f, Target::Agents, &HeaderMap::new(), discover).await.0, StatusCode::UNAUTHORIZED);
    }

    // ── MCP Events ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn both_servers_advertise_events_in_discover_and_a_relayed_initialize() {
        let f = Fake::new(&["rt1"])
            .token("agents", claims(BASE, AGENTS_SCOPE, "user-a"))
            .token("bot", claims("https://mcp.allternit.com/mcp/bots/b-d", BOT_SCOPE, "user-a"))
            .reply("rt1", Ok((200, r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"x"}}}"#)));
        let discover = r#"{"jsonrpc":"2.0","id":"d","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
        for (target, token) in [(Target::Agents, "agents"), (bot("b-d"), "bot")] {
            let (_, _, body) = post(&f, target, &bearer(token), discover).await;
            assert_eq!(body["result"]["capabilities"]["events"], json!({ "listChanged": false }));
        }
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        let (_, _, body) = post(&f, Target::Agents, &bearer("agents"), init).await;
        assert_eq!(body["result"]["capabilities"]["events"], json!({ "listChanged": false }));
        assert_eq!(body["result"]["capabilities"]["tools"], json!({}));
    }

    #[tokio::test]
    async fn events_are_answered_at_the_edge_scoped_to_the_server_and_never_relayed() {
        let f = Fake::new(&["rt1"]).token("agents", claims(BASE, AGENTS_SCOPE, "user-a")).token("bot", claims("https://mcp.allternit.com/mcp/bots/b-d", BOT_SCOPE, "user-a"));
        let list = r#"{"jsonrpc":"2.0","id":3,"method":"events/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#;
        let (status, _, body) = post(&f, Target::Agents, &bearer("agents"), list).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["resultType"], "complete", "modern results are decorated");
        let names: Vec<_> = body["result"]["events"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"approval.requested".to_string()));
        let (_, _, body) = post(&f, bot("b-d"), &bearer("bot"), list).await;
        assert!(body["result"]["events"].as_array().unwrap().iter().any(|e| e["name"] == "vendor.ticket.created"));

        let sub = json!({ "jsonrpc": "2.0", "id": 4, "method": "events/subscribe", "params": {
            "name": "vendor.ticket.created", "arguments": {},
            "delivery": { "mode": "webhook", "url": "https://cb.example.com/x", "secret": crate::routes::mcp_events::testing::secret(1) } } });
        let (_, _, body) = post(&f, bot("b-d"), &bearer("bot"), &sub.to_string()).await;
        assert!(body["result"]["id"].as_str().unwrap().starts_with("sub_"), "{body}");
        let stored = f.events.subs.lock().unwrap().values().next().cloned().unwrap();
        assert_eq!(stored.principal, crate::routes::mcp_events::Principal { user_id: "user-a".into(), client: "claude-connector".into(), target: "bot:b-d".into() });
        assert_eq!(stored.arguments, json!({ "bot_id": "b-d" }));
        assert!(f.paths().is_empty(), "events/* never reach a runtime");
    }

    #[tokio::test]
    async fn events_sit_behind_the_same_token_and_approval_checks() {
        let f = Fake::new(&["rt1"]).token("tok", clerk_claims("user_c", Some("client-abc")));
        let list = r#"{"jsonrpc":"2.0","id":3,"method":"events/list"}"#;
        let (status, _, body) = post(&f, Target::Agents, &bearer("tok"), list).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("approval_required")));
        assert_eq!(post(&f, Target::Agents, &HeaderMap::new(), list).await.0, StatusCode::UNAUTHORIZED);
        let f = f.approved("user_c", "client-abc", "agents");
        assert_eq!(post(&f, Target::Agents, &bearer("tok"), list).await.0, StatusCode::OK);
    }

    #[test]
    fn the_human_page_lists_servers_versions_and_the_event_catalog_without_external_assets() {
        let html = home_html("https://mcp.allternit.com/mcp");
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>Allternit MCP</title>"));
        assert!(html.contains("https://mcp.allternit.com/mcp/bots/&lt;bot id&gt;"));
        for v in mcp_protocol::SUPPORTED {
            assert!(html.contains(v));
        }
        assert!(html.contains("approval.requested") && html.contains("vendor.ticket.created"));
        assert!(html.contains("prefers-color-scheme: dark"));
        assert!(html.contains("https://docs.allternit.com/guides/mcp-events"));
        assert!(!html.contains("<script") && !html.contains("<link") && !html.contains("src="));
        assert!(wants_html(&{ let mut h = HeaderMap::new(); h.insert(header::ACCEPT, "text/html,application/xhtml+xml".parse().unwrap()); h }));
        assert!(!wants_html(&{ let mut h = HeaderMap::new(); h.insert(header::ACCEPT, "application/json, text/event-stream".parse().unwrap()); h }));
        assert_eq!(public_host("https://MCP.allternit.com/mcp").as_deref(), Some("mcp.allternit.com"));
    }

    #[tokio::test]
    async fn a_mismatched_mcp_method_header_is_a_400() {
        let f = Fake::new(&["rt1"]).token("agents", claims(BASE, AGENTS_SCOPE, "user-a"));
        let mut h = bearer("agents");
        h.insert("mcp-method", "tools/call".parse().unwrap());
        let (status, _, body) = post(&f, Target::Agents, &h, LIST).await;
        assert_eq!((status, body["error"]["code"].as_i64()), (StatusCode::BAD_REQUEST, Some(-32020)));
        assert!(f.paths().is_empty());
        h.insert("mcp-method", "tools/list".parse().unwrap());
        assert_eq!(post(&f, Target::Agents, &h, LIST).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_bot_token_cannot_open_the_agents_server_and_the_reverse() {
        let f = Fake::new(&["rt1"]).token("bot", claims("https://mcp.allternit.com/mcp/bots/b-x", BOT_SCOPE, "user-a")).token("agents", claims(BASE, AGENTS_SCOPE, "user-a"));
        assert_eq!(post(&f, Target::Agents, &bearer("bot"), LIST).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(post(&f, bot("b-x"), &bearer("agents"), LIST).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(post(&f, Target::Agents, &bearer("agents"), LIST).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn get_is_405_after_auth_and_a_bad_body_is_400() {
        let f = Fake::new(&["rt1"]).token("t", claims("https://mcp.allternit.com/mcp/bots/b-m", BOT_SCOPE, "user-a"));
        let resp = serve(&f, BASE, bot("b-m"), &Method::GET, &bearer("t"), Bytes::new()).await;
        assert_eq!((resp.status(), resp.headers()[header::ALLOW].to_str().unwrap()), (StatusCode::METHOD_NOT_ALLOWED, "POST"));
        let resp = serve(&f, BASE, bot("b-m"), &Method::GET, &HeaderMap::new(), Bytes::new()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "an unauthenticated GET still gets the challenge");
        assert_eq!(post(&f, bot("b-m"), &bearer("t"), "{nope").await.0, StatusCode::BAD_REQUEST);
    }

    // ── forwarding ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_good_call_goes_to_the_owners_runtime_as_the_verified_user_and_the_answer_comes_back() {
        let f = Fake::new(&["rt1"]).token("t", claims("https://mcp.allternit.com/mcp/bots/b-fwd", BOT_SCOPE, "user-a")).reply("rt1", Ok((200, r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#)));
        let (status, _, body) = post(&f, bot("b-fwd"), &bearer("t"), LIST).await;
        assert_eq!((status, body["result"]["tools"].is_array()), (StatusCode::OK, true));
        let calls = f.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].0.as_str(), calls[0].1.as_str(), calls[0].2.as_str(), calls[0].3.as_str()), ("user-a", "rt1", "/webhooks/mcp-edge/bots/b-fwd", "claude-connector"));
        assert_eq!(calls[0].4, LIST.as_bytes(), "the JSON-RPC body goes through untouched");
    }

    #[tokio::test]
    async fn the_agents_server_forwards_to_the_best_runtime_only() {
        let f = Fake::new(&["rt1", "rt2"]).token("t", claims(BASE, AGENTS_SCOPE, "user-a"));
        assert_eq!(post(&f, Target::Agents, &bearer("t"), LIST).await.0, StatusCode::OK);
        assert_eq!(f.paths(), vec![("rt1".to_string(), "/webhooks/mcp-edge/server".to_string())]);
    }

    #[tokio::test]
    async fn the_runtime_holding_the_bot_is_found_and_remembered() {
        let f = Fake::new(&["rt1", "rt2"])
            .token("t", claims("https://mcp.allternit.com/mcp/bots/b-find", BOT_SCOPE, "user-find"))
            .reply("rt1", Ok((404, r#"{"error":"not_found"}"#)));
        assert_eq!(post(&f, bot("b-find"), &bearer("t"), LIST).await.0, StatusCode::OK);
        let tried: Vec<String> = f.paths().into_iter().map(|p| p.0).collect();
        assert_eq!(tried, ["rt1", "rt2"], "a 404 moves on to the owner's next runtime");
        // Next time rt2 goes first.
        assert_eq!(post(&f, bot("b-find"), &bearer("t"), LIST).await.0, StatusCode::OK);
        let tried: Vec<String> = f.paths().into_iter().map(|p| p.0).collect();
        assert_eq!(tried, ["rt1", "rt2", "rt2"]);
    }

    #[tokio::test]
    async fn a_bot_no_runtime_of_the_owner_holds_is_a_plain_404_not_an_offline_error() {
        // Someone else's bot id: every one of this user's runtimes says not_found.
        let f = Fake::new(&["rt1", "rt2"])
            .token("t", claims("https://mcp.allternit.com/mcp/bots/b-theirs", BOT_SCOPE, "user-b"))
            .reply("rt1", Ok((404, r#"{"error":"not_found"}"#)))
            .reply("rt2", Ok((404, r#"{"error":"not_found"}"#)));
        let (status, _, body) = post(&f, bot("b-theirs"), &bearer("t"), LIST).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
        // A user with no computer at all reads the same.
        let none = Fake::new(&[]).token("t", claims("https://mcp.allternit.com/mcp/bots/b-theirs", BOT_SCOPE, "user-b"));
        assert_eq!(post(&none, bot("b-theirs"), &bearer("t"), LIST).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_offline_or_warming_computer_gets_a_plain_jsonrpc_error_with_the_callers_id() {
        for why in [Unreached::Offline, Unreached::Warming, Unreached::Timeout, Unreached::Other("boom".into())] {
            let f = Fake::new(&["rt1"]).token("t", claims("https://mcp.allternit.com/mcp/bots/b-off", BOT_SCOPE, "user-a")).reply("rt1", Err(why));
            let (status, _, body) = post(&f, bot("b-off"), &bearer("t"), LIST).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["id"], 7);
            assert_eq!(body["error"]["message"], "Your Allternit computer is offline; it will be woken — try again in a minute.");
        }
    }

    #[tokio::test]
    async fn an_unreachable_runtime_next_to_a_404_is_offline_because_the_bot_may_live_there() {
        let f = Fake::new(&["rt1", "rt2"])
            .token("t", claims("https://mcp.allternit.com/mcp/bots/b-mix", BOT_SCOPE, "user-a"))
            .reply("rt1", Err(Unreached::Warming))
            .reply("rt2", Ok((404, r#"{"error":"not_found"}"#)));
        let (status, _, body) = post(&f, bot("b-mix"), &bearer("t"), LIST).await;
        assert_eq!((status, body["error"]["code"].as_i64()), (StatusCode::OK, Some(-32000)));
    }

    #[tokio::test(start_paused = true)]
    async fn a_runtime_that_never_answers_is_cut_off_at_the_25_second_budget() {
        struct Stuck;
        #[async_trait::async_trait]
        impl EdgeBackend for Stuck {
            async fn is_approved(&self, _: &str, _: &str, _: &str) -> Result<bool, String> {
                Ok(true)
            }
            async fn verify(&self, _: &str) -> Result<Value, String> {
                Ok(claims("https://mcp.allternit.com/mcp/bots/b-stuck", BOT_SCOPE, "user-a"))
            }
            async fn runtimes(&self, _: &str) -> Result<Vec<String>, String> {
                Ok(vec!["rt1".into()])
            }
            async fn forward(&self, _: &str, _: &str, _: &str, _: &str, _: &[u8]) -> Result<(u16, Vec<u8>), Unreached> {
                tokio::time::sleep(Duration::from_secs(600)).await;
                Ok((200, vec![]))
            }
        }
        let started = tokio::time::Instant::now();
        let resp = serve(&Stuck, BASE, bot("b-stuck"), &Method::POST, &bearer("t"), Bytes::from(LIST)).await;
        assert_eq!(started.elapsed(), BUDGET);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["message"], OFFLINE_MESSAGE);
    }

    #[tokio::test]
    async fn a_revoked_connection_comes_back_401_with_the_challenge_the_relay_dropped() {
        let f = Fake::new(&["rt1"]).token("t", claims("https://mcp.allternit.com/mcp/bots/b-rev", BOT_SCOPE, "user-a")).reply("rt1", Ok((401, r#"{"error":"invalid_token","message":"This connection was revoked"}"#)));
        let (status, headers, _) = post(&f, bot("b-rev"), &bearer("t"), LIST).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(headers[header::WWW_AUTHENTICATE].to_str().unwrap().contains("revoked"));
    }

    #[tokio::test]
    async fn a_notification_passes_through_as_202() {
        let f = Fake::new(&["rt1"]).token("t", claims("https://mcp.allternit.com/mcp/bots/b-note", BOT_SCOPE, "user-a")).reply("rt1", Ok((202, "")));
        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(post(&f, bot("b-note"), &bearer("t"), note).await.0, StatusCode::ACCEPTED);
    }

    // ── well-known and routing ────────────────────────────────────────────────

    #[test]
    fn metadata_is_per_bot_with_its_own_scope_and_the_agents_server_keeps_its_own() {
        let m = metadata(&bot("b1"), BASE);
        assert_eq!((m["resource"].as_str(), m["scopes_supported"][0].as_str()), (Some("https://mcp.allternit.com/mcp/bots/b1"), Some("profile")));
        assert_eq!(m["authorization_servers"][0], oauth_issuer());
        let a = metadata(&Target::Agents, BASE);
        assert_eq!((a["resource"].as_str(), a["scopes_supported"][0].as_str()), (Some(BASE), Some("profile")));
        assert_eq!(bot_id_from_resource_path("mcp/bots/b1"), Some("b1"));
        assert_eq!(bot_id_from_resource_path("/mcp/bots/b1/"), Some("b1"));
        assert_eq!(bot_id_from_resource_path("mcp/server"), None);
        assert_eq!(bot_id_from_resource_path("mcp/bots/"), None);
        assert_eq!(resource_metadata_url("https://mcp.allternit.com/mcp/bots/b1"), "https://mcp.allternit.com/.well-known/oauth-protected-resource/mcp/bots/b1");
    }

    #[test]
    fn the_runtime_signature_matches_allternit_apis_pinned_vector() {
        // Same vector allternit-api's relay_auth tests pin: this is what lets a runtime accept the edge's call.
        let key = "c8963414bf6c4c869eeac5f8a057c3dc574d422f1b108397b66f67bab3d2f981";
        assert_eq!(
            super::super::runtime_relay::sign_runtime_request(key, 1_700_000_000, "POST", "/api/v1/voice/calls", b"{}"),
            "v1=34edb38cb1c7839397d5993a055972d2f352b42164bcdfd935264cdcae1d1576"
        );
    }

    // ── production backend and routes (Postgres) ──────────────────────────────

    use crate::routes::runtime_relay::{answer_test_request, register_test_connection, CloudMessage};
    use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway};
    use tower::ServiceExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn the_production_forward_signs_for_the_runtime_names_the_owner_and_keeps_the_oauth_token_here() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        seed_runtime_device(&state.db, "mcp-edge-rt-1", "user-a").await;
        seed_runtime_device(&state.db, "mcp-edge-rt-2", "user-a").await;
        seed_runtime_device(&state.db, "mcp-edge-rt-other", "user-b").await;
        let (connection, mut outgoing) = register_test_connection("mcp-edge-rt-2").await;
        let backend = ProdBackend::new(&state);

        let order = backend.runtimes("user-a").await.unwrap();
        assert_eq!(order, ["mcp-edge-rt-2", "mcp-edge-rt-1"], "the connected computer is tried first, and only the user's own");

        let forward = backend.forward("user-a", "mcp-edge-rt-2", "/webhooks/mcp-edge/bots/b1", "claude-connector", LIST.as_bytes());
        let answer = async {
            let CloudMessage::Request { request_id, method, path, headers, body, body_encoding } = tokio::time::timeout(Duration::from_secs(10), outgoing.recv()).await.unwrap().unwrap() else {
                panic!("a relay request")
            };
            assert_eq!((method.as_str(), path.as_str(), body_encoding.as_str()), ("POST", "/webhooks/mcp-edge/bots/b1", "base64"));
            assert_eq!(STANDARD.decode(body).unwrap(), LIST.as_bytes());
            assert_eq!(headers.get("x-allternit-owner").map(String::as_str), Some("user-a"));
            assert_eq!(headers.get(CLIENT_HEADER).map(String::as_str), Some("claude-connector"));
            assert!(headers["x-allternit-runtime-sig"].starts_with("v1="), "{headers:?}");
            assert!(headers.contains_key("x-allternit-runtime-ts"));
            assert!(!headers.contains_key("authorization"), "the OAuth token never goes to the runtime");
            answer_test_request(&connection, &request_id, 200, r#"{"jsonrpc":"2.0","id":7,"result":{}}"#).await;
        };
        let (reply, _) = tokio::join!(forward, answer);
        assert_eq!(reply.unwrap().0, 200);
        // A runtime that isn't the user's can't be signed for.
        assert!(matches!(backend.forward("user-a", "mcp-edge-rt-other", "/webhooks/mcp-edge/server", "c", b"{}").await, Err(Unreached::Other(_))));
    }

    #[tokio::test]
    async fn without_mcp_public_url_every_route_is_a_503_and_with_it_the_well_known_docs_answer() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let app = routes().with_state(state);
        let send = |method: &str, uri: &str| {
            let app = app.clone();
            let req = axum::http::Request::builder().method(method).uri(uri).header("host", "mcp.allternit.com").body(axum::body::Body::from(LIST)).unwrap();
            async move {
                let resp = app.oneshot(req).await.unwrap();
                let status = resp.status();
                let headers = resp.headers().clone();
                let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
                (status, headers, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
            }
        };

        std::env::remove_var("MCP_PUBLIC_URL");
        for (m, u) in [("POST", "/mcp/bots/b1"), ("POST", "/mcp/server"), ("POST", "/mcp"), ("GET", "/.well-known/oauth-protected-resource"), ("GET", "/.well-known/oauth-protected-resource/mcp/bots/b1")] {
            let (status, _, body) = send(m, u).await;
            assert_eq!((status, body["error"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("mcp_edge_not_configured")), "{u}");
        }

        std::env::set_var("MCP_PUBLIC_URL", BASE);
        std::env::remove_var("MCP_OAUTH_ISSUER");
        let (status, _, doc) = send("GET", "/.well-known/oauth-protected-resource/mcp/bots/b1").await;
        assert_eq!((status, doc["resource"].as_str(), doc["scopes_supported"][0].as_str()), (StatusCode::OK, Some("https://mcp.allternit.com/mcp/bots/b1"), Some("profile")));
        assert_eq!(doc["authorization_servers"][0], "https://allternit.com/__clerk");
        let (_, _, doc) = send("GET", "/.well-known/oauth-protected-resource").await;
        assert_eq!((doc["resource"].as_str(), doc["scopes_supported"][0].as_str()), (Some(BASE), Some("profile")));
        let (_, _, doc) = send("GET", "/.well-known/oauth-protected-resource/mcp/server").await;
        assert_eq!(doc["resource"].as_str(), Some(BASE));
        // A real route with no token: the OAuth challenge, via the same paths on the mcp host.
        for u in ["/mcp/bots/b1", "/mcp/server", "/mcp"] {
            let (status, headers, _) = send("POST", u).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{u}");
            assert!(headers[header::WWW_AUTHENTICATE].to_str().unwrap().contains("oauth-protected-resource"), "{u}");
        }
        std::env::remove_var("MCP_PUBLIC_URL");
    }
}
