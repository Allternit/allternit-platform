//! allternit-api as an MCP *server* — `tools/list` / `tools/call` over the
//! MCP Streamable HTTP transport (single JSON-RPC endpoint, plain JSON
//! responses — no server-initiated push, so no SSE upgrade needed for this
//! tool set).
//!
//! Previously `mcp_routes.rs` only let allternit-api act as an MCP *client*
//! (consuming external MCP servers via the `/mcp/connectors` catalog). This
//! is the inverse direction: an external MCP client (Claude Code, Claude
//! Desktop, etc.) can point at `/mcp/server` with a Clerk bearer token and
//! call the same tool registry `tool_routes.rs` already exposes over REST.
//! Mounted inside the existing `/mcp` nest, so it inherits the same
//! `auth_middleware` Clerk gate as everything else there — no new attack
//! surface beyond what `/api/tools/execute` already grants an authenticated
//! caller.
//!
//! Scope limits, stated rather than silently assumed: single JSON-RPC
//! request objects only (no batch arrays); no `Mcp-Session-Id`
//! issuance/enforcement (stateless — fine for a tool set with no
//! server-initiated messages, but a strict client may expect one).

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::mcp_tunnel_auth::require_tunnel_auth;
use crate::tool_routes::{execute_tool_internal, ExecuteToolRequest};
use crate::AppState;

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Protocol versions, `initialize`, `server/discover` and modern result
/// decoration come from the shared `mcp-protocol` crate.
fn server_spec() -> mcp_protocol::ServerSpec {
    mcp_protocol::servers::agents(SERVER_VERSION)
}

pub fn mcp_server_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/server", post(handle_rpc))
        .route("/pins/check", post(pins_check_route))
        .route("/pins/approve", post(pins_approve_route))
}

// ─── Tool pinning ───────────────────────────────────────────────────────────
//
// Same rule as `mcp_directory_held.rs`: once a server's tools are pinned, a new
// tool or a changed name/description/inputSchema/annotations stays out of the
// bot's tool list until the owner approves it. The first sight of a server pins
// what it lists. A change opens a `tool_changed` attention item.

/// sha256 of `(name, description, inputSchema, annotations)`. `serde_json` keeps
/// object keys sorted, so the same tool always hashes the same.
pub fn tool_hash(tool: &Value) -> String {
    let canon = json!([tool["name"], tool["description"], tool["inputSchema"], tool["annotations"]]);
    hex::encode(Sha256::digest(canon.to_string().as_bytes()))
}

/// Split `tools` into the ones the owner may use and the ones held (`new` or `changed`).
/// A server with no pins yet is pinned as listed (nothing held). Each held tool opens
/// its own `tool_changed` item (`source_id` = `server/tool`) with an Approve retry.
pub fn check_pins(conn: &Connection, owner: &str, server_id: &str, tools: &[Value]) -> (Vec<String>, Vec<Value>) {
    let pinned: i64 = conn.query_row("SELECT COUNT(*) FROM mcp_tool_pins WHERE owner = ?1 AND server_id = ?2", params![owner, server_id], |r| r.get(0)).unwrap_or(0);
    let now = crate::agent_gateway_routes::now();
    let mut allowed = vec![];
    let mut held = vec![];
    for t in tools {
        let Some(name) = t["name"].as_str() else { continue };
        let hash = tool_hash(t);
        if pinned == 0 {
            let _ = conn.execute("INSERT OR REPLACE INTO mcp_tool_pins (owner, server_id, tool, hash, approved_at, description) VALUES (?1,?2,?3,?4,?5,?6)", params![owner, server_id, name, hash, now, t["description"].as_str()]);
            allowed.push(name.to_string());
            continue;
        }
        let stored: Option<(String, Option<String>)> = conn.query_row("SELECT hash, description FROM mcp_tool_pins WHERE owner = ?1 AND server_id = ?2 AND tool = ?3", params![owner, server_id, name], |r| Ok((r.get(0)?, r.get(1)?))).ok();
        match stored {
            Some((h, _)) if h == hash => allowed.push(name.to_string()),
            other => {
                let reason = if other.is_some() { "changed" } else { "new" };
                let old = other.and_then(|(_, d)| d);
                held.push(json!({ "tool": name, "reason": reason, "hash": hash, "oldDescription": old, "newDescription": t["description"] }));
            }
        }
    }
    for h in &held {
        let name = h["tool"].as_str().unwrap_or_default();
        crate::netops::alerts::open(
            conn,
            &crate::netops::alerts::Item {
                owner: owner.to_string(),
                kind: "tool_changed".into(),
                severity: "warn".into(),
                source: "mcp".into(),
                source_id: format!("{server_id}/{name}"),
                summary: format!("{server_id} changed its tool \"{name}\". It stays off until you approve it."),
                detail: json!({ "serverId": server_id, "tool": name, "reason": h["reason"], "hash": h["hash"], "oldDescription": h["oldDescription"], "newDescription": h["newDescription"] }),
                retry: Some(json!({ "pinApprove": { "serverId": server_id, "tool": name, "hash": h["hash"] } })),
                ..Default::default()
            },
        );
    }
    (allowed, held)
}

/// Approve: re-pin `tools` (the held ones, as the server lists them now) and close their items.
pub fn approve_pins(conn: &Connection, owner: &str, server_id: &str, tools: &[Value]) -> usize {
    let mut n = 0;
    for t in tools {
        let Some(name) = t["name"].as_str() else { continue };
        n += pin_tool(conn, owner, server_id, name, &tool_hash(t), t["description"].as_str());
    }
    n
}

/// Pin one tool at `hash` and resolve its `tool_changed` item. Used by Approve in the inbox.
pub fn pin_tool(conn: &Connection, owner: &str, server_id: &str, tool: &str, hash: &str, description: Option<&str>) -> usize {
    let now = crate::agent_gateway_routes::now();
    let n = conn.execute("INSERT OR REPLACE INTO mcp_tool_pins (owner, server_id, tool, hash, approved_at, description) VALUES (?1,?2,?3,?4,?5,?6)", params![owner, server_id, tool, hash, now, description]).unwrap_or(0);
    crate::netops::alerts::resolve_source(conn, owner, "tool_changed", &format!("{server_id}/{tool}"), "approved");
    n
}

#[derive(Debug, Deserialize)]
struct PinsBody {
    #[serde(rename = "serverId")]
    server_id: String,
    #[serde(default)]
    tools: Vec<Value>,
}

async fn pins_run(state: Arc<AppState>, user: AuthUser, body: PinsBody, approve: bool) -> axum::response::Response {
    if body.server_id.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "serverId is required" }))).into_response();
    }
    let res = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(|e| e.to_string())?;
        Ok::<Value, String>(if approve {
            json!({ "pinned": approve_pins(&conn, &user.user_id, &body.server_id, &body.tools) })
        } else {
            let (allowed, held) = check_pins(&conn, &user.user_id, &body.server_id, &body.tools);
            json!({ "allowed": allowed, "held": held })
        })
    })
    .await;
    match res {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

/// `POST /mcp/pins/check` `{serverId, tools}` → `{allowed, held}`.
async fn pins_check_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<PinsBody>) -> axum::response::Response {
    pins_run(state, user, body, false).await
}

/// `POST /mcp/pins/approve` `{serverId, tools}` → `{pinned}`.
async fn pins_approve_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<PinsBody>) -> axum::response::Response {
    pins_run(state, user, body, true).await
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub(crate) id: Option<Value>,
    pub(crate) method: String,
    #[serde(default)]
    pub(crate) params: Value,
}

async fn tool_catalog(state: &AppState) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "shell.exec",
            "description": "Execute a shell command on the allternit-api host.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string" }
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "file.read",
            "description": "Read a file's contents.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }),
        json!({
            "name": "file.write",
            "description": "Write content to a file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "file.list",
            "description": "List a directory's entries.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }
        }),
        json!({
            "name": "file.exists",
            "description": "Check whether a path exists.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }),
        json!({
            "name": "file.remove",
            "description": "Delete a file.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }),
        json!({
            "name": "system.info",
            "description": "Get host platform/arch/hostname info.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "system.env",
            "description": "Read an environment variable (or list all if no key given).",
            "inputSchema": {
                "type": "object",
                "properties": { "key": { "type": "string" } }
            }
        }),
        json!({
            "name": "http.get",
            "description": "Make an HTTP GET request.",
            "inputSchema": {
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"]
            }
        }),
        json!({
            "name": "http.post",
            "description": "Make an HTTP POST request.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "body": {},
                    "headers": { "type": "object" }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "time.now",
            "description": "Get the current UTC time.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "inference.execute_routed_turn",
            "description": "Run a prompt through a local CLI inference provider (codex, claude-code, cursor, openrouter).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "provider": { "type": "string", "enum": ["codex", "claude-code", "cursor", "openrouter"] },
                    "prompt": { "type": "string" }
                },
                "required": ["provider", "prompt"]
            }
        }),
        json!({
            "name": "inference.get_routed_usage",
            "description": "Return recent routed CLI inference usage events.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "default": 50 } }
            }
        }),
    ];

    // Office-engine markdown conversion tools — descriptors shared with the
    // REST registry (`tool_routes::list_tools`), so both front doors advertise
    // the same contract.
    for tool in crate::tool_routes::markdown_builtin_tools() {
        tools.push(json!({
            "name": tool.get("id").cloned().unwrap_or(Value::Null),
            "description": tool.get("description").cloned().unwrap_or(Value::Null),
            "inputSchema": tool.get("parameters").cloned().unwrap_or(json!({})),
        }));
    }

    tools.extend(aai_tool_descriptors());

    // Attached MCP servers from the dispatcher registry.
    for remote in state.mcp_dispatcher.list_tools().await {
        tools.push(remote);
    }

    tools
}

/// A modern client's `Mcp-Method` / `Mcp-Name` headers must agree with the
/// body (MCP 2026-07-28); a mismatch is HTTP 400 with `-32020`.
/// A JSON-RPC reply as a Streamable HTTP response, with the status the
/// request's era calls for (`mcp_protocol::http_status`): 400 for version /
/// header errors, 404 for an unknown method on a modern request, else 200.
/// Every allternit-api MCP server answers through this.
pub(crate) fn rpc_response(method: &str, params: &Value, body: Value) -> axum::response::Response {
    let modern = mcp_protocol::Era::of(method, params, None).is_modern();
    let status = StatusCode::from_u16(mcp_protocol::http_status(modern, &body)).unwrap_or(StatusCode::OK);
    (status, Json(body)).into_response()
}

pub(crate) fn header_mismatch(headers: &axum::http::HeaderMap, req: &JsonRpcRequest) -> Option<axum::response::Response> {
    let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let id = req.id.clone().unwrap_or_default();
    mcp_protocol::check_headers(&id, &req.method, &req.params, h("mcp-method"), h("mcp-name"))
        .map(|err| (StatusCode::BAD_REQUEST, Json(err)).into_response())
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i32, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

const TUNNEL_ID_HEADER: &str = "x-allternit-tunnel-id";
const TUNNEL_CERT_THUMBPRINT_HEADER: &str = "x-allternit-client-cert-thumbprint";
const TUNNEL_OAUTH_ISSUER_HEADER: &str = "x-allternit-oauth-issuer";
const TUNNEL_OAUTH_AUDIENCE_HEADER: &str = "x-allternit-oauth-audience";

#[tracing::instrument(skip_all, name = "mcp_server.handle_rpc", fields(method = %req.method))]
async fn handle_rpc(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: axum::http::HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    if let Some(resp) = header_mismatch(&headers, &req) {
        return resp;
    }
    let tunnel_id = headers
        .get(TUNNEL_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let thumbprint = headers
        .get(TUNNEL_CERT_THUMBPRINT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let issuer = headers
        .get(TUNNEL_OAUTH_ISSUER_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let audience = headers
        .get(TUNNEL_OAUTH_AUDIENCE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match require_tunnel_auth(
        &state.db,
        tunnel_id,
        thumbprint.as_deref(),
        issuer.as_deref(),
        audience.as_deref(),
    ) {
        Ok(_) => {
            let org_id = user.organization_id.as_deref().or(user.tenant_id.as_deref());
            // OAuth access tokens (ChatGPT / Claude connectors) are verified
            // for audience + scope and limited to the read-only agent tools.
            let agents_only = match crate::mcp_agents::authorize_bearer(&state, &headers).await {
                Ok(v) => v,
                Err(resp) => return resp,
            };
            handle_rpc_inner(&state, &user.user_id, org_id, req, agents_only).await
        }
        Err(result) => (
            StatusCode::UNAUTHORIZED,
            Json(rpc_error(
                serde_json::Value::Null,
                -32001,
                format!("MCP tunnel auth failed: {}", result.reason()),
            )),
        )
            .into_response(),
    }
}

/// Internal sibling of `/mcp/server` for the local gizzi runtime's MCP client
/// (the `allternit-tools` bundled server in cmd/gizzi-code): same tool
/// registry, but gated by `internal_auth::require_internal_token` (shared
/// secret) instead of Clerk, with the user named explicitly via
/// `x-allternit-user-id` — the same trust model as
/// `connector_routes::mcp_proxy_internal`. Mounted by `internal_routes.rs` at
/// `/internal/tools/mcp`, outside the Clerk-protected router.
pub async fn mcp_tools_internal(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    if let Err(status) = crate::internal_auth::require_internal_token(&headers, &state) {
        return (status, Json(json!({"error": "unauthorized"}))).into_response();
    }
    let user_id = headers
        .get("x-allternit-user-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let org_id = headers
        .get("x-allternit-organization-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    match user_id {
        Some(user_id) => handle_rpc_inner(&state, &user_id, org_id.as_deref(), req, false).await,
        None => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "x-allternit-user-id header is required"})),
        )
            .into_response(),
    }
}

/// Stdio entry point used by `allternit-mcp-server`. Parses one JSON-RPC
/// request line, dispatches it, and returns the serialized response (or None
/// for notifications that require no response body).
pub async fn mcp_tools_internal_stdio(state: &Arc<AppState>, user_id: &str, line: &str) -> Option<String> {
    let req: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(req) => req,
        Err(e) => {
            return Some(serde_json::to_string(&rpc_error(
                serde_json::Value::Null,
                -32700,
                format!("Parse error: {e}"),
            )).unwrap_or_default());
        }
    };

    if req.id.is_none() {
        // Notification: no response body required.
        return None;
    }

    let response = handle_rpc_inner_value(state, user_id, None, req, false).await;
    Some(serde_json::to_string(&response).unwrap_or_default())
}

/// Shared JSON-RPC core behind both the Clerk-gated `/mcp/server` route and
/// the internal-token-gated `/internal/tools/mcp` route.
async fn handle_rpc_inner(
    state: &Arc<AppState>,
    user_id: &str,
    org_id: Option<&str>,
    req: JsonRpcRequest,
    agents_only: bool,
) -> axum::response::Response {
    // JSON-RPC notifications (no `id`) get no response body — the caller
    // isn't waiting on one. `notifications/initialized` is the only one this
    // server expects.
    if req.id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }

    let (method, params) = (req.method.clone(), req.params.clone());
    rpc_response(&method, &params, handle_rpc_inner_value(state, user_id, org_id, req, agents_only).await)
}

/// The agents server for a call the cloud edge already verified (OAuth token
/// checked for `aud` and `agents:read`): always the read-only agent tools, as for
/// any OAuth caller. See [`crate::mcp_edge_relay`].
pub async fn relayed_agents_rpc(state: &Arc<AppState>, user_id: &str, body: &[u8]) -> axum::response::Response {
    match serde_json::from_slice::<JsonRpcRequest>(body) {
        Ok(req) => handle_rpc_inner(state, user_id, None, req, true).await,
        Err(e) => Json(rpc_error(serde_json::Value::Null, -32700, format!("Parse error: {e}"))).into_response(),
    }
}

/// Value-returning core so the stdio binary can reuse the same dispatch logic
/// without constructing Axum responses.
async fn handle_rpc_inner_value(
    state: &Arc<AppState>,
    user_id: &str,
    org_id: Option<&str>,
    req: JsonRpcRequest,
    agents_only: bool,
) -> Value {
    let id = req.id.clone().unwrap_or_default();
    let spec = server_spec();
    let era = mcp_protocol::Era::of(&req.method, &req.params, None);
    if let Some(done) = mcp_protocol::preflight(&spec, &era, &id, &req.method) {
        return done;
    }
    let method = req.method.clone();
    let response = dispatch_method(state, user_id, org_id, req, agents_only, id).await;
    mcp_protocol::finish(&spec, &era, &method, response)
}

/// The server's own methods (everything `mcp_protocol::preflight` doesn't
/// answer).
async fn dispatch_method(
    state: &Arc<AppState>,
    user_id: &str,
    org_id: Option<&str>,
    req: JsonRpcRequest,
    agents_only: bool,
    id: Value,
) -> Value {
    match req.method.as_str() {
        "tools/list" => {
            let mut tools = if agents_only { Vec::new() } else { tool_catalog(state).await };
            tools.extend(crate::mcp_agents::tool_descriptors());
            success(id, json!({ "tools": tools }))
        }

        "resources/list" => success(id, json!({ "resources": crate::mcp_agents::resource_descriptors() })),

        "resources/templates/list" => success(id, json!({ "resourceTemplates": [] })),

        "resources/read" => {
            let uri = req.params.get("uri").and_then(|v| v.as_str()).unwrap_or_default();
            match crate::mcp_agents::read_resource(uri) {
                Some(result) => success(id, result),
                None => rpc_error(id, -32002, format!("Resource not found: {uri}")),
            }
        }

        "tools/call" => {
            let name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let arguments = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            if let Some(result) =
                crate::mcp_agents::call_tool(state, user_id, &name, arguments.clone()).await
            {
                return success(id, result);
            }
            if agents_only {
                return rpc_error(id, -32602, format!("Unknown tool: {name}"));
            }

            let result = if is_aai_tool(&name) {
                call_aai_tool(state, user_id, &name, arguments).await
            } else if name.starts_with("inference.") || name == "time.now" {
                handle_builtin_dotted_tool(state, user_id, org_id, &name, arguments).await
            } else if name.contains('.') {
                state
                    .mcp_dispatcher
                    .dispatch_call(&name, arguments.clone())
                    .await
                    .map_err(|e| e.to_string())
            } else {
                let exec_request = ExecuteToolRequest {
                    tool: name,
                    args: arguments,
                    timeout: None,
                    workspace_id: None,
                    ..Default::default()
                };
                execute_tool_internal(state, &exec_request, user_id, org_id).await
            };

            match result {
                Ok(result) => success(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": result.to_string() }],
                        "isError": false
                    }),
                ),
                Err(err) => success(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": err }],
                        "isError": true
                    }),
                ),
            }
        }

        other => rpc_error(id, -32601, format!("Method not found: {other}")),
    }
}

fn is_aai_tool(name: &str) -> bool {
    matches!(name, "agents_list" | "agent_send" | "thread_events" | "approvals_list" | "approval_respond")
}

/// AAI as MCP tools, scoped to the authenticated user. MCP callers are not
/// human, so `approval_respond` always refuses.
pub(crate) fn aai_tool_descriptors() -> Vec<Value> {
    vec![
        json!({ "name": "agents_list", "description": "List the user's bots, including vendor-backed ones, with provenance (vendor, lane, guarantee).",
            "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "agent_send", "description": "Send a turn to a bot on a thread (a new task thread when thread_id is omitted). Vendor bots run through the Agent Gateway. If an approval is required the call fails with the approval id; approve it in Allternit.",
            "inputSchema": { "type": "object", "properties": { "bot_id": { "type": "string" }, "text": { "type": "string" }, "thread_id": { "type": "string" },
                "correlation_id": { "type": "string" }, "consequential": { "type": "boolean" }, "allternit_approval_id": { "type": "string" } }, "required": ["bot_id", "text"] } }),
        json!({ "name": "thread_events", "description": "Thread activity events, ascending after a sequence cursor (newest first when after is omitted).",
            "inputSchema": { "type": "object", "properties": { "thread_id": { "type": "string" }, "after": { "type": "integer" }, "limit": { "type": "integer" } }, "required": ["thread_id"] } }),
        json!({ "name": "approvals_list", "description": "Approvals on a thread (optionally filtered by state, e.g. pending).",
            "inputSchema": { "type": "object", "properties": { "thread_id": { "type": "string" }, "state": { "type": "string" } }, "required": ["thread_id"] } }),
        json!({ "name": "approval_respond", "description": "Not available over MCP: approvals are answered by a person in Allternit.",
            "inputSchema": { "type": "object", "properties": { "approval_id": { "type": "string" }, "decision": { "type": "string" } } } }),
    ]
}

pub(crate) async fn call_aai_tool(state: &Arc<AppState>, user_id: &str, name: &str, a: Value) -> Result<Value, String> {
    use crate::aai_facade as f;
    let req = |k: &str| a[k].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("{k} is required"));
    let fail = |e: f::FacadeErr| e.to_json().to_string();
    match name {
        "agents_list" => f::agents_list(state, user_id, None).map(|v| json!({ "agents": v })).map_err(fail),
        "agent_send" => f::send(
            state,
            user_id,
            f::SendIn {
                bot_id: req("bot_id")?,
                thread_id: a["thread_id"].as_str().filter(|s| !s.is_empty()),
                text: req("text")?,
                correlation_id: a["correlation_id"].as_str().map(str::to_string),
                consequential: a["consequential"].as_bool().unwrap_or(false),
                allternit_approval_id: a["allternit_approval_id"].as_str().map(str::to_string),
                via: "mcp",
                traceparent: a["traceparent"].as_str().map(str::to_string),
                idempotency_key: a["correlation_id"].as_str().map(|k| crate::netops::hops::derive_key(&["mcp", user_id, k])),
                from_kind: "vendor_bot",
                from_id: "mcp",
            },
        )
        .await
        .map_err(fail),
        "thread_events" => f::thread_events(state, user_id, req("thread_id")?, a["after"].as_i64(), a["limit"].as_i64()).map_err(fail),
        "approvals_list" => f::approvals_list(state, user_id, req("thread_id")?, a["state"].as_str()).map(|v| json!({ "approvals": v })).map_err(fail),
        _ => Err(json!({
            "error": "MCP callers are not a human. Approve or deny this in Allternit.",
            "code": "HUMAN_REQUIRED", "status": 403,
        })
        .to_string()),
    }
}

async fn handle_builtin_dotted_tool(
    state: &Arc<AppState>,
    user_id: &str,
    org_id: Option<&str>,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    match name {
        "inference.execute_routed_turn" => {
            let provider = arguments
                .get("provider")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "provider is required".to_string())?;
            let prompt = arguments
                .get("prompt")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "prompt is required".to_string())?;

            let result = crate::inference_router_executor::execute_routed_turn(
                provider,
                prompt,
                None,
                None,
            )
            .await;
            Ok(serde_json::to_value(result).map_err(|e| e.to_string())?)
        }
        "inference.get_routed_usage" => {
            let limit = arguments
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .clamp(1, 1000) as i64;
            let events = crate::inference_router_routes::query_routed_usage(&state.db, user_id, limit)
                .map_err(|e| e.to_string())?;
            Ok(json!({ "events": events }))
        }
        _ => {
            let exec_request = ExecuteToolRequest {
                tool: name.to_string(),
                args: arguments,
                timeout: None,
                workspace_id: None,
                ..Default::default()
            };
            execute_tool_internal(state, &exec_request, user_id, org_id).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn aai_tools_are_listed_scoped_and_never_approve() {
        let names: Vec<_> = aai_tool_descriptors().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["agents_list", "agent_send", "thread_events", "approvals_list", "approval_respond"]);
        let st = crate::aai_facade::test_util::setup("mcp", "READY").await;
        let a = call_aai_tool(&st, "user-a", "agents_list", json!({})).await.unwrap();
        assert_eq!(a["agents"].as_array().unwrap().len(), 2);
        assert!(call_aai_tool(&st, "user-b", "agents_list", json!({})).await.unwrap()["agents"].as_array().unwrap().is_empty());
        // 428 comes back as a tool error carrying the approval id.
        let e = call_aai_tool(&st, "user-a", "agent_send", json!({ "bot_id": "bot-vendor", "thread_id": "th-vendor", "text": "wire $5k", "consequential": true })).await.unwrap_err();
        let e: Value = serde_json::from_str(&e).unwrap();
        assert_eq!(e["status"], 428);
        let aid = e["approvalId"].as_str().unwrap().to_string();
        let l = call_aai_tool(&st, "user-a", "approvals_list", json!({ "thread_id": "th-vendor", "state": "pending" })).await.unwrap();
        assert_eq!(l["approvals"][0]["id"], aid.as_str());
        // MCP can never answer it, and it stays pending.
        let r = call_aai_tool(&st, "user-a", "approval_respond", json!({ "approval_id": aid, "decision": "approve" })).await.unwrap_err();
        assert!(r.contains("HUMAN_REQUIRED"));
        let l = call_aai_tool(&st, "user-a", "approvals_list", json!({ "thread_id": "th-vendor", "state": "pending" })).await.unwrap();
        assert_eq!(l["approvals"].as_array().unwrap().len(), 1);
        // The tool call also flows through the JSON-RPC dispatcher.
        let req: JsonRpcRequest = serde_json::from_value(json!({ "id": 1, "method": "tools/call", "params": { "name": "thread_events", "arguments": { "thread_id": "th-vendor" } } })).unwrap();
        let out = handle_rpc_inner_value(&st, "user-a", None, req, false).await;
        assert_eq!(out["result"]["isError"], false);
    }

    #[test]
    fn success_response_has_jsonrpc_shape() {
        let id = json!("req-1");
        let result = json!({"tools": []});
        let resp = success(id.clone(), result.clone());
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], id);
        assert_eq!(resp["result"], result);
        assert!(resp.get("error").is_none());
    }

    #[test]
    fn rpc_error_response_has_jsonrpc_shape() {
        let id = json!(42);
        let resp = rpc_error(id.clone(), -32601, "Method not found".into());
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], id);
        assert_eq!(resp["error"]["code"], -32601);
        assert_eq!(resp["error"]["message"], "Method not found");
    }

    #[test]
    fn tunnel_auth_header_constants_are_set() {
        // These headers are read by handle_rpc and passed to mcp_tunnel_auth.
        assert!(!TUNNEL_ID_HEADER.is_empty());
        assert!(!TUNNEL_CERT_THUMBPRINT_HEADER.is_empty());
        assert!(!TUNNEL_OAUTH_ISSUER_HEADER.is_empty());
        assert!(!TUNNEL_OAUTH_AUDIENCE_HEADER.is_empty());
        assert!(TUNNEL_ID_HEADER.starts_with("x-allternit-"));
    }

    fn pin_conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../migrations/V258__attention_items.sql").split("-- Why a run").next().unwrap()).unwrap();
        c.execute_batch(include_str!("../migrations/V264__mcp_tool_pins.sql")).unwrap();
        c.execute_batch(include_str!("../migrations/V265__mcp_tool_pin_description.sql")).unwrap();
        c.execute_batch(
            "CREATE TABLE runtime_user_events (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT, user_id TEXT, event_type TEXT, thread_id TEXT, payload TEXT, idempotency_key TEXT, occurred_at TEXT, UNIQUE(user_id, idempotency_key));
             CREATE TABLE agents (id TEXT PRIMARY KEY, name TEXT);",
        )
        .unwrap();
        c
    }

    #[test]
    fn changed_and_new_tools_are_held_until_approved() {
        let c = pin_conn();
        let t1 = json!({"name": "read", "description": "Read a file", "inputSchema": {"type": "object"}});
        let (allowed, held) = check_pins(&c, "u", "srv", &[t1.clone()]);
        assert_eq!((allowed, held.len()), (vec!["read".to_string()], 0)); // first sight pins

        let t1b = json!({"name": "read", "description": "Read a file, then email it", "inputSchema": {"type": "object"}});
        let t2 = json!({"name": "send", "description": "Send"});
        let (allowed, held) = check_pins(&c, "u", "srv", &[t1b.clone(), t2.clone()]);
        assert!(allowed.is_empty());
        assert_eq!(held.iter().map(|h| h["reason"].as_str().unwrap()).collect::<Vec<_>>(), vec!["changed", "new"]);
        let open: i64 = c.query_row("SELECT COUNT(*) FROM attention_items WHERE kind = 'tool_changed' AND resolved_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(open, 2, "one item per held tool, each with its own Approve");

        approve_pins(&c, "u", "srv", &[t1b.clone(), t2.clone()]);
        let (allowed, held) = check_pins(&c, "u", "srv", &[t1b, t2]);
        assert_eq!((allowed.len(), held.len()), (2, 0));
        let open: i64 = c.query_row("SELECT COUNT(*) FROM attention_items WHERE kind = 'tool_changed' AND resolved_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(open, 0);
    }

    #[test]
    fn inbox_approve_repins_one_tool_and_it_comes_back() {
        let c = pin_conn();
        let t1 = json!({"name": "read", "description": "Read a file"});
        check_pins(&c, "u", "srv", &[t1]);
        let t1b = json!({"name": "read", "description": "Read a file, then email it"});
        let (allowed, held) = check_pins(&c, "u", "srv", &[t1b.clone()]);
        assert!(allowed.is_empty());
        assert_eq!(held[0]["oldDescription"], "Read a file");
        let (id, retry): (String, String) = c.query_row("SELECT id, retry_json FROM attention_items WHERE kind = 'tool_changed' AND resolved_at IS NULL", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(id, "tool_changed:srv/read");
        let spec: Value = serde_json::from_str(&retry).unwrap();
        let p = &spec["pinApprove"];
        pin_tool(&c, "u", p["serverId"].as_str().unwrap(), p["tool"].as_str().unwrap(), p["hash"].as_str().unwrap(), t1b["description"].as_str());
        let (allowed, held) = check_pins(&c, "u", "srv", &[t1b]);
        assert_eq!((allowed.len(), held.len()), (1, 0));
        let open: i64 = c.query_row("SELECT COUNT(*) FROM attention_items WHERE kind = 'tool_changed' AND resolved_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(open, 0);
    }
}
