//! Server-side MCP dispatcher for `/mcp/server`.
//!
//! Maintains a registry of attached MCP servers and forwards `tools/call`
//! requests to them. Tool names are namespaced as `<server_id>.<tool_name>`
//! so they do not collide with the built-in registry in `tool_routes.rs`.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const CLIENT_NAME: &str = "allternit-api";

/// Client capabilities sent on `initialize`: this host renders MCP Apps
/// (SEP-1865), so servers may attach `_meta.ui` to tools and serve `ui://`
/// resources.
fn client_capabilities() -> Value {
    json!({
        "extensions": {
            "io.modelcontextprotocol/ui": { "mimeTypes": ["text/html;profile=mcp-app"] }
        }
    })
}

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
        let client = Client::new();
        let _ = Self::initialize(&client, &url, &headers).await?;
        let tools = Self::list_tools_remote(&client, &url, &headers).await?;
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
    pub async fn list_servers(&self) -> Vec<McpAttachedServer> {
        self.servers.read().await.values().cloned().collect()
    }

    /// Return all remote tools as namespaced JSON-RPC tool descriptors.
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

        let client = Client::new();
        Self::call_tool_remote(&client, &server.url, &server.headers, tool_name, arguments).await
    }

    async fn initialize(
        client: &Client,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<Value, String> {
        let result = Self::request(
            client,
            url,
            headers,
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": client_capabilities(),
                "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .await?;
        // Fire-and-forget initialized notification.
        let _ = Self::request(
            client,
            url,
            headers,
            "notifications/initialized",
            Value::Null,
        )
        .await;
        Ok(result)
    }

    async fn list_tools_remote(
        client: &Client,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<Vec<McpToolDescriptor>, String> {
        let result = Self::request(client, url, headers, "tools/list", Value::Object(Default::default())).await?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        tools
            .into_iter()
            .map(|v| serde_json::from_value(v).map_err(|e| format!("Invalid tool descriptor: {}", e)))
            .collect()
    }

    async fn call_tool_remote(
        client: &Client,
        url: &str,
        headers: &HashMap<String, String>,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        Self::request(
            client,
            url,
            headers,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
        .await
    }

    async fn request(
        client: &Client,
        url: &str,
        headers: &HashMap<String, String>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut req = client
            .post(url.trim_end_matches('/'))
            .json(&body)
            .timeout(std::time::Duration::from_secs(30));
        for (k, v) in headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.map_err(|e| format!("MCP request failed: {}", e))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| format!("MCP response decode failed: {}", e))?;

        if let Some(error) = body.get("error") {
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown MCP error");
            return Err(format!("MCP error ({}): {}", status, message));
        }

        body.get("result")
            .cloned()
            .ok_or_else(|| "MCP response missing result".to_string())
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
