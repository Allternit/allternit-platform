//! MCP stdio bridge to the officecli MCP server.
//!
//! One long-lived `officecli mcp` child per user, driven by the `mcp-client`
//! crate's stdio transport (dual-era: a `server/discover` probe, falling back
//! to the legacy `initialize` handshake officecli speaks). Docs are passed
//! per tool call as file paths, so a single server per user suffices.
//!
//! The add-in reaches it through `POST /office/cli/mcp`, which is itself an
//! MCP endpoint built on `mcp-protocol`: `initialize`, `server/discover`,
//! `ping` and notifications are answered here (both eras), every other
//! method is forwarded to the child — giving the add-in `tools/list`
//! (dynamic discovery of officecli's full tool surface, in canonical order)
//! and `tools/call` with no per-tool server code to maintain.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use mcp_client::{ClientCapabilities, McpClient, McpError, StdioConfig, StdioTransport};
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

use crate::config::AppConfig;
use crate::office_cli_routes::caller_id;
use crate::AppState;

/// Timeout for a single forwarded JSON-RPC request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Timeout for the connect (era probe + handshake) on spawn.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

const SERVER_NAME: &str = "allternit-office-cli";

fn server_spec() -> mcp_protocol::ServerSpec {
    mcp_protocol::ServerSpec {
        name: SERVER_NAME,
        version: env!("CARGO_PKG_VERSION"),
        capabilities: json!({ "tools": { "listChanged": false } }),
        instructions: None,
    }
}

pub struct McpSession {
    client: McpClient,
    pub last_active: Instant,
}

impl McpSession {
    /// Spawn the officecli MCP stdio server and connect to it.
    pub async fn spawn(config: &AppConfig) -> Result<McpSession, String> {
        let stdio = StdioConfig {
            command: config.officecli_bin().to_string_lossy().into_owned(),
            args: config.officecli_mcp_args(),
            env: Default::default(),
            cwd: None,
            timeout_secs: REQUEST_TIMEOUT.as_secs(),
        };
        let transport = StdioTransport::spawn(stdio)
            .await
            .map_err(|e| format!("Failed to spawn officecli MCP server: {}", e))?;
        // officecli renders nothing, so no MCP Apps extension here.
        let mut client = McpClient::new(transport)
            .with_client_info("allternit-gateway", env!("CARGO_PKG_VERSION"))
            .with_client_capabilities(ClientCapabilities::default());
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, client.initialize()).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                let _ = client.shutdown().await;
                return Err(format!("officecli MCP handshake failed: {e}"));
            }
            Err(_) => {
                let _ = client.shutdown().await;
                return Err("officecli MCP handshake timed out".to_string());
            }
        }
        Ok(McpSession { client, last_active: Instant::now() })
    }

    /// Forward one request; the result, or the server's JSON-RPC error.
    pub async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        self.last_active = Instant::now();
        self.client.request(method, params).await
    }

    pub async fn shutdown(&mut self) {
        let _ = self.client.shutdown().await;
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn reply(modern: bool, body: Value) -> Response {
    let status = StatusCode::from_u16(mcp_protocol::http_status(modern, &body)).unwrap_or(StatusCode::OK);
    (status, Json(body)).into_response()
}

/// `POST /office/cli/mcp` — the add-in's MCP endpoint. Lazily spawns the
/// user's session; on a transport failure (child died) the session is
/// dropped, respawned once and the request retried once. JSON-RPC errors
/// from officecli are returned as-is; transport failures map to `-32603`.
///
/// NOTE: the session map stays write-locked across the forwarded request, so
/// MCP calls are serialized process-wide — acceptable for v1 (one user per
/// gateway in practice); revisit if multi-user concurrency matters.
pub async fn mcp_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let user_id = caller_id(&headers);
    let Some(method) = body.get("method").and_then(|m| m.as_str()).map(str::to_string) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Bad request", "message": "Missing JSON-RPC 'method'" })),
        )
            .into_response();
    };
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let mut params = body.get("params").cloned().unwrap_or_else(|| json!({}));

    let spec = server_spec();
    if let Some(err) = mcp_protocol::check_headers(&id, &method, &params, header(&headers, "mcp-method"), header(&headers, "mcp-name")) {
        return (StatusCode::BAD_REQUEST, Json(err)).into_response();
    }
    let era = mcp_protocol::Era::of(&method, &params, header(&headers, "mcp-protocol-version"));
    let modern = era.is_modern();
    if let Some(done) = mcp_protocol::preflight(&spec, &era, &id, &method) {
        if done.is_null() {
            return StatusCode::ACCEPTED.into_response();
        }
        return reply(modern, done);
    }
    if body.get("id").is_none() {
        // Any other notification: nothing to forward to a per-request bridge.
        return StatusCode::ACCEPTED.into_response();
    }

    // `doc_id` is a gateway extension: resolve it to an absolute path (with
    // ownership check), rewrite "@doc" placeholders inside params. It never
    // reaches the officecli server.
    if let Some(doc_id) = body.get("doc_id").and_then(|v| v.as_str()) {
        let Ok(uuid) = doc_id.parse::<Uuid>() else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Bad request", "message": "Invalid doc_id" })),
            )
                .into_response();
        };
        let path = {
            let docs = state.office_cli_docs.read().await;
            docs.get(&uuid).filter(|doc| doc.user_id == user_id).map(|doc| doc.path.clone())
        };
        let Some(path) = path else {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Office CLI document not found" }))).into_response();
        };
        rewrite_doc_placeholders(&mut params, &path);
    }
    // The add-in's protocol `_meta` is for this endpoint; the session client
    // adds its own for the child's era.
    mcp_protocol::client::strip_modern_meta(&mut params);

    let mut sessions = state.office_cli_mcp_sessions.write().await;
    let mut last_error: Option<String> = None;
    for _attempt in 0..2 {
        if !sessions.contains_key(&user_id) {
            match McpSession::spawn(&state.config).await {
                Ok(session) => {
                    sessions.insert(user_id.clone(), session);
                }
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            }
        }
        let session = sessions.get_mut(&user_id).expect("session inserted above");
        match session.request(&method, Some(params.clone())).await {
            Ok(result) => {
                let response = mcp_protocol::rpc_ok(&id, result);
                return reply(modern, mcp_protocol::finish(&spec, &era, &method, response));
            }
            Err(McpError::JsonRpc { code, message, data }) => {
                let response = match data {
                    Some(data) => mcp_protocol::rpc_err_data(&id, code as i64, message, data),
                    None => mcp_protocol::rpc_err(&id, code as i64, message),
                };
                return reply(modern, mcp_protocol::finish(&spec, &era, &method, response));
            }
            Err(e) => {
                last_error = Some(e.to_string());
                // Drop the (possibly dead) session; the next loop iteration
                // respawns it and retries the request once.
                if let Some(mut dead) = sessions.remove(&user_id) {
                    dead.shutdown().await;
                }
            }
        }
    }

    reply(
        modern,
        mcp_protocol::rpc_err(&id, -32603, last_error.unwrap_or_else(|| "officecli MCP request failed".to_string())),
    )
}

/// Recursively rewrite "@doc" inside any string value in a JSON-RPC params
/// object to the resolved absolute document path. Substring replacement is
/// required: the officecli MCP server exposes a single tool whose `command`
/// string embeds the filename (e.g. "view @doc outline").
fn rewrite_doc_placeholders(value: &mut Value, path: &std::path::Path) {
    match value {
        Value::String(s) if s.contains("@doc") => {
            *s = s.replace("@doc", &path.to_string_lossy().to_string());
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_doc_placeholders(item, path)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| rewrite_doc_placeholders(item, path)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_doc_placeholder_recursively() {
        let mut params = json!({
            "name": "docx_get",
            "arguments": {
                "file": "@doc",
                "nested": ["@doc", { "deep": "@doc" }, "untouched"]
            }
        });
        rewrite_doc_placeholders(&mut params, std::path::Path::new("/abs/path/report.docx"));
        assert_eq!(
            params["arguments"]["file"],
            json!("/abs/path/report.docx")
        );
        assert_eq!(
            params["arguments"]["nested"][0],
            json!("/abs/path/report.docx")
        );
        assert_eq!(
            params["arguments"]["nested"][1]["deep"],
            json!("/abs/path/report.docx")
        );
        assert_eq!(params["arguments"]["nested"][2], json!("untouched"));
        // "@doc" embedded inside a command string is rewritten too (the real
        // MCP tool takes a single `command` param containing the filename).
        let mut other = json!({"command": "view @doc outline"});
        rewrite_doc_placeholders(&mut other, std::path::Path::new("/abs/report.docx"));
        assert_eq!(other["command"], json!("view /abs/report.docx outline"));
    }
}
