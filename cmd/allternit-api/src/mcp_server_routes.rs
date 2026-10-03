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
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::mcp_tunnel_auth::require_tunnel_auth;
use crate::tool_routes::{execute_tool_internal, ExecuteToolRequest};
use crate::AppState;

const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2025-03-26"];
const SERVER_NAME: &str = "allternit-api";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn mcp_server_router() -> Router<Arc<AppState>> {
    Router::new().route("/server", post(handle_rpc))
}

#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
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

    let id = req.id.clone().unwrap_or_default();
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
    let Some(id) = req.id.clone() else {
        return StatusCode::ACCEPTED.into_response();
    };

    Json(handle_rpc_inner_value(state, user_id, org_id, req, agents_only).await).into_response()
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

    match req.method.as_str() {
        "initialize" => {
            let requested = req.params.get("protocolVersion").and_then(|v| v.as_str());
            let version = requested
                .filter(|v| SUPPORTED_PROTOCOL_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSION);
            success(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {
                        "tools": { "listChanged": false },
                        "resources": { "subscribe": false, "listChanged": false }
                    },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                    "instructions": crate::mcp_agents::SERVER_INSTRUCTIONS
                }),
            )
        }

        "ping" => success(id, json!({})),

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
}
