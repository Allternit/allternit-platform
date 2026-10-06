//! Server-side MCP dispatcher for `/mcp/server`.
//!
//! Maintains a registry of attached MCP servers and forwards `tools/call`
//! requests to them. Tool names are namespaced as `<server_id>.<tool_name>`
//! so they do not collide with the built-in registry in `tool_routes.rs`.
//!
//! Talks to the servers through the `mcp-client` crate (dual-era: MCP
//! 2026-07-28 stateless first, legacy `initialize` fallback, era cached per
//! origin), advertising the MCP Apps extension like every Allternit host.

use mcp_client::{McpClient, McpError, StreamableHttpConfig, StreamableHttpTransport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

const CLIENT_NAME: &str = "allternit-api";
/// Per-request timeout for attached servers.
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Descriptor for a tool advertised by an attached MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, alias = "inputSchema")]
    pub input_schema: Value,
}

/// An MCP server attached to the API. The API fetches and caches the remote
/// tool list at attach time so `tools/list` is served from memory.
#[derive(Debug, Clone)]
pub struct McpAttachedServer {
    pub id: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub tools: Vec<McpToolDescriptor>,
}

impl McpAttachedServer {
    /// Build the namespaced tool name used in the API catalog and in calls.
    pub fn namespaced_name(&self, tool_name: &str) -> String {
        format!("{}.{}", self.id, tool_name)
    }
}

/// In-memory registry of attached MCP servers.
#[derive(Debug, Default, Clone)]
pub struct McpDispatcher {
    servers: Arc<RwLock<HashMap<String, McpAttachedServer>>>,
}

impl McpDispatcher {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Attach a server and synchronise its tool catalog. The returned value
    /// contains the cached tool descriptors so callers can report what was
    /// registered.
    pub async fn attach_and_sync(
        &self,
        id: String,
        url: String,
        headers: HashMap<String, String>,
    ) -> Result<McpAttachedServer, String> {
        let tools = Self::list_tools_remote(&url, &headers).await?;
        let server = McpAttachedServer {
            id: id.clone(),
            url,
            headers,
            tools,
        };
        self.servers.write().await.insert(id, server.clone());
        Ok(server)
    }

    /// Attach a server whose tool catalog is already known (used in tests).
    pub async fn attach(&self, server: McpAttachedServer) {
        self.servers.write().await.insert(server.id.clone(), server);
    }

    /// Remove an attached server.
    pub async fn detach(&self, id: &str) -> Option<McpAttachedServer> {
        self.servers.write().await.remove(id)
    }

    /// List all attached servers.
    /// List all attached servers, ordered by id.
    pub async fn list_servers(&self) -> Vec<McpAttachedServer> {
        let mut servers: Vec<_> = self.servers.read().await.values().cloned().collect();
        servers.sort_by(|a, b| a.id.cmp(&b.id));
        servers
    }

    /// Return all remote tools as namespaced JSON-RPC tool descriptors, in
    /// the canonical (name) order so `tools/list` is deterministic.
    pub async fn list_tools(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for server in self.servers.read().await.values() {
            for tool in &server.tools {
                out.push(json!({
                    "name": server.namespaced_name(&tool.name),
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                }));
            }
        }
        mcp_protocol::ordering::sort_tools(&mut out);
        out
    }

    /// Dispatch a tool call to the attached server identified by the namespace
    /// prefix of `namespaced_name`.
    pub async fn dispatch_call(
        &self,
        namespaced_name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let (server_id, tool_name) = namespaced_name
            .split_once('.')
            .ok_or_else(|| format!("Invalid namespaced tool name: {}", namespaced_name))?;

        let server = self
            .servers
            .read()
            .await
            .get(server_id)
            .cloned()
            .ok_or_else(|| format!("No attached MCP server named '{}'", server_id))?;

        if !server.tools.iter().any(|t| t.name == tool_name) {
            return Err(format!(
                "Tool '{}' not found on attached MCP server '{}'",
                tool_name, server_id
            ));
        }

        Self::call_tool_remote(&server.url, &server.headers, tool_name, arguments).await
    }

    /// Open a connection: the crate client negotiates the era (modern
    /// `server/discover`, else legacy `initialize`).
    async fn connect(url: &str, headers: &HashMap<String, String>) -> Result<McpClient, String> {
        let mut config = StreamableHttpConfig::new(url.trim_end_matches('/'));
        config.headers = headers.clone();
        config.timeout_secs = REQUEST_TIMEOUT_SECS;
        let transport = StreamableHttpTransport::new(config).map_err(describe)?;
        let mut client = McpClient::new(transport).with_client_info(CLIENT_NAME, env!("CARGO_PKG_VERSION"));
        client.initialize().await.map_err(describe)?;
        Ok(client)
    }

    /// Run `f` on a fresh connection and always close it afterwards.
    async fn with_client<T, F, Fut>(url: &str, headers: &HashMap<String, String>, f: F) -> Result<T, String>
    where
        F: FnOnce(McpClient) -> Fut,
        Fut: std::future::Future<Output = (McpClient, Result<T, String>)>,
    {
        let client = Self::connect(url, headers).await?;
        let (mut client, out) = f(client).await;
        let _ = client.shutdown().await;
        out
    }

    async fn list_tools_remote(url: &str, headers: &HashMap<String, String>) -> Result<Vec<McpToolDescriptor>, String> {
        let pages = Self::with_client(url, headers, |client| async move {
            let mut tools = Vec::new();
            let mut cursor: Option<String> = None;
            // Follow pagination (bounded).
            for _ in 0..20 {
                let params = cursor.as_ref().map(|c| json!({ "cursor": c }));
                match client.request("tools/list", params).await {
                    Ok(page) => {
                        tools.extend(page.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default());
                        cursor = page.get("nextCursor").and_then(|c| c.as_str()).map(String::from);
                        if cursor.is_none() {
                            break;
                        }
                    }
                    Err(e) => return (client, Err(describe(e))),
                }
            }
            (client, Ok(tools))
        })
        .await?;
        pages
            .into_iter()
            .map(|v| serde_json::from_value(v).map_err(|e| format!("Invalid tool descriptor: {}", e)))
            .collect()
    }

    async fn call_tool_remote(
        url: &str,
        headers: &HashMap<String, String>,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let params = json!({ "name": name, "arguments": arguments });
        Self::with_client(url, headers, |client| async move {
            let out = client.request("tools/call", Some(params)).await.map_err(describe);
            (client, out)
        })
        .await
    }
}

/// Human-readable error, same shape callers already showed.
fn describe(e: McpError) -> String {
    match e {
        McpError::JsonRpc { code, message, .. } => format!("MCP error ({code}): {message}"),
        other => format!("MCP request failed: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Json, routing::post, Router};
    use serde_json::json;
    use std::net::SocketAddr;

    async fn mock_mcp_server() -> SocketAddr {
        let app = Router::new().route("/mcp", post(|Json(req): Json<Value>| async move {
            let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            match method {
                "initialize" if req["params"]["capabilities"]["extensions"]["io.modelcontextprotocol/ui"]["mimeTypes"][0]
                    != "text/html;profile=mcp-app" => Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32602, "message": "host did not advertise io.modelcontextprotocol/ui" }
                })),
                "initialize" => Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "serverInfo": { "name": "mock", "version": "1.0.0" }
                    }
                })),
                "notifications/initialized" => Json(json!({ "jsonrpc": "2.0", "id": Value::Null, "result": {} })),
                "tools/list" => Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [
                            {
                                "name": "echo",
                                "description": "Echo input",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": { "message": { "type": "string" } },
                                    "required": ["message"]
                                }
                            }
                        ]
                    }
                })),
                "tools/call" => {
                    let name = req["params"]["name"].as_str().unwrap_or("");
                    let args = &req["params"]["arguments"];
                    Json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": format!("{} {}", name, args["message"].as_str().unwrap_or("")) }]
                        }
                    }))
                }
                _ => Json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("Method not found: {}", method) }
                })),
            }
        }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn attach_and_sync_populates_tools() {
        let addr = mock_mcp_server().await;
        let dispatcher = McpDispatcher::new();
        let server = dispatcher
            .attach_and_sync(
                "mock".to_string(),
                format!("http://{}/mcp", addr),
                HashMap::new(),
            )
            .await
            .unwrap();

        assert_eq!(server.id, "mock");
        assert_eq!(server.tools.len(), 1);
        assert_eq!(server.tools[0].name, "echo");

        let tools = dispatcher.list_tools().await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "mock.echo");
    }

    #[tokio::test]
    async fn dispatch_call_forwards_to_remote_server() {
        let addr = mock_mcp_server().await;
        let dispatcher = McpDispatcher::new();
        dispatcher
            .attach_and_sync(
                "mock".to_string(),
                format!("http://{}/mcp", addr),
                HashMap::new(),
            )
            .await
            .unwrap();

        let result = dispatcher
            .dispatch_call("mock.echo", json!({ "message": "hello" }))
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "echo hello");
    }

    #[tokio::test]
    async fn list_tools_is_in_canonical_order_across_servers() {
        let dispatcher = McpDispatcher::new();
        for (id, tools) in [("zeta", vec!["b", "a"]), ("alpha", vec!["c"])] {
            dispatcher
                .attach(McpAttachedServer {
                    id: id.into(),
                    url: "http://unused".into(),
                    headers: HashMap::new(),
                    tools: tools
                        .into_iter()
                        .map(|n| McpToolDescriptor { name: n.into(), description: String::new(), input_schema: json!({}) })
                        .collect(),
                })
                .await;
        }
        let names: Vec<_> = dispatcher.list_tools().await.iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["alpha.c", "zeta.a", "zeta.b"]);
        let ids: Vec<_> = dispatcher.list_servers().await.into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["alpha", "zeta"]);
    }

    #[tokio::test]
    async fn dispatch_call_rejects_unknown_server() {
        let dispatcher = McpDispatcher::new();
        let err = dispatcher
            .dispatch_call("unknown.tool", json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("No attached MCP server named 'unknown'"));
    }

    #[tokio::test]
    async fn dispatch_call_rejects_unknown_tool_on_known_server() {
        let addr = mock_mcp_server().await;
        let dispatcher = McpDispatcher::new();
        dispatcher
            .attach_and_sync(
                "mock".to_string(),
                format!("http://{}/mcp", addr),
                HashMap::new(),
            )
            .await
            .unwrap();

        let err = dispatcher
            .dispatch_call("mock.nope", json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("Tool 'nope' not found"));
    }
}
