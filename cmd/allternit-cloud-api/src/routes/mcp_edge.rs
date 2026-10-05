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
    let caller = if token.starts_with(crate::routes::vendor_bot_keys::KEY_PREFIX) {
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
    // `server/discover` is static: answer it here so a modern client learns
    // versions, capabilities and instructions without waking the computer.
    if rpc_method == "server/discover" {
        let spec = match target {
            Target::Bot(_) => mcp_protocol::servers::vendor_bot(env!("CARGO_PKG_VERSION")),
            Target::Agents => mcp_protocol::servers::agents(env!("CARGO_PKG_VERSION")),
        };
        let era = mcp_protocol::Era::of(rpc_method, &request["params"], h("mcp-protocol-version"));
        if let Some(reply) = mcp_protocol::preflight(&spec, &era, &id, rpc_method) {
            return Json(reply).into_response();
        }
    }

    let outcome = match tokio::time::timeout(BUDGET, deliver(backend, &target, &caller, &body)).await {
        Ok(o) => o,
        Err(_) => Outcome::Unreachable,
    };
    match outcome {
        Outcome::Unreachable => rpc_error(id, OFFLINE_MESSAGE),
        Outcome::NotFound => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Outcome::Answer(status, reply) => {
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

// ─── handlers ─────────────────────────────────────────────────────────────────

async fn bot_endpoint(State(state): State<Arc<ApiState>>, Path(vendor_bot_id): Path<String>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let Some(base) = public_mcp_url() else { return not_configured() };
    serve(&ProdBackend { state: &state }, &base, Target::Bot(vendor_bot_id), &method, &headers, body).await
}

async fn agents_endpoint(State(state): State<Arc<ApiState>>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let Some(base) = public_mcp_url() else { return not_configured() };
    serve(&ProdBackend { state: &state }, &base, Target::Agents, &method, &headers, body).await
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
}

#[async_trait::async_trait]
impl EdgeBackend for ProdBackend<'_> {
    async fn verify_cli_key(&self, token: &str, vendor_bot_id: &str) -> Result<Option<(String, String)>, String> {
        crate::routes::vendor_bot_keys::verify_key(&self.state.db, token, vendor_bot_id).await.map_err(|e| e.to_string())
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
    }

    impl Fake {
        fn new(runtimes: &[&str]) -> Self {
            Self { tokens: HashMap::new(), runtimes: runtimes.iter().map(|r| r.to_string()).collect(), replies: Mutex::new(HashMap::new()), calls: Mutex::new(vec![]), verified: AtomicUsize::new(0), cli_keys: HashMap::new(), approvals: vec![] }
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
        let backend = ProdBackend { state: &state };

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
