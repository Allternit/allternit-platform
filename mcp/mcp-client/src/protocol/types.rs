//! JSON-RPC 2.0 and MCP protocol types

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC version string
pub const JSONRPC_VERSION: &str = "2.0";

/// Any JSON-RPC message (raw transport send/receive)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// Request message
    Request(JsonRpcRequest),
    /// Response message
    Response(JsonRpcResponse),
    /// Notification message (no id, no response expected)
    Notification(JsonRpcNotification),
    /// Error response without a usable id
    Error(JsonRpcErrorResponse),
}

/// JSON-RPC error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub error: JsonRpcError,
}

/// JSON-RPC 2.0 request object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new JSON-RPC request
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: method.into(),
            params,
        }
    }

    /// Create a new notification (no id)
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Create a successful response
    pub fn success(id: u64, result: Value) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id, result: Some(result), error: None }
    }

    /// Create an error response
    pub fn error(id: u64, error: JsonRpcError) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id, result: None, error: Some(error) }
    }
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    /// Standard JSON-RPC error codes
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    pub const SERVER_ERROR_START: i32 = -32099;
    pub const SERVER_ERROR_END: i32 = -32000;

    /// Create a new JSON-RPC error
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    /// Create a parse error
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(Self::PARSE_ERROR, message)
    }

    /// Create an invalid request error
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(Self::INVALID_REQUEST, message)
    }

    /// Create a method not found error
    pub fn method_not_found(method: impl AsRef<str>) -> Self {
        Self::new(Self::METHOD_NOT_FOUND, format!("Method not found: {}", method.as_ref()))
    }

    /// Create an invalid params error
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(Self::INVALID_PARAMS, message)
    }

    /// Create an internal error
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL_ERROR, message)
    }
}

/// JSON-RPC 2.0 notification (server -> client)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Create a new JSON-RPC notification
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), method: method.into(), params }
    }
}

/// Newest MCP protocol revision this client speaks. The version list lives
/// in `mcp-protocol` (one place for every Allternit client and server); this
/// is a re-export, not a second constant.
pub use mcp_protocol::LATEST as MCP_PROTOCOL_VERSION;

/// MCP Apps extension id (SEP-1865), advertised under `capabilities.extensions`
pub const MCP_APPS_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";

/// MIME type of an MCP App UI resource
pub const MCP_APP_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// MCP client capabilities
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<Value>,
    /// Protocol extensions, keyed by extension id (e.g. MCP Apps)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

impl ClientCapabilities {
    /// Capabilities that declare this client an MCP Apps host:
    /// `extensions["io.modelcontextprotocol/ui"] = {"mimeTypes":["text/html;profile=mcp-app"]}`.
    pub fn with_mcp_apps() -> Self {
        Self {
            extensions: Some(serde_json::json!({
                MCP_APPS_EXTENSION_ID: { "mimeTypes": [MCP_APP_MIME_TYPE] }
            })),
            ..Self::default()
        }
    }
}

/// Roots capability
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RootsCapability {
    pub list_changed: bool,
}

/// MCP server capabilities
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    /// MCP Events extension (draft experimental-ext-triggers-events):
    /// present when the server answers `events/list|subscribe|unsubscribe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Value>,
    /// 2026-07-28 extension capabilities, keyed by extension id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

/// Extension id the events draft may be advertised under.
pub const EVENTS_EXTENSION_ID: &str = "io.modelcontextprotocol/events";

impl ServerCapabilities {
    /// Whether the server advertises MCP Events: `capabilities.events`, the
    /// extension id under `extensions`, or `experimental.events`.
    pub fn supports_events(&self) -> bool {
        let present = |v: Option<&Value>| v.is_some_and(|v| !v.is_null() && v != &Value::Bool(false));
        present(self.events.as_ref())
            || present(self.extensions.as_ref().and_then(|e| e.get(EVENTS_EXTENSION_ID)))
            || present(self.experimental.as_ref().and_then(|e| e.get("events")))
    }
}

/// Prompts capability
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptsCapability {
    pub list_changed: bool,
}

/// Resources capability
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ResourcesCapability {
    pub subscribe: bool,
    pub list_changed: bool,
}

/// Tools capability
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolsCapability {
    pub list_changed: bool,
}

/// Initialize request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: Implementation,
}

/// Initialize response result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: Implementation,
}

/// Implementation info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

impl Implementation {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self { name: name.into(), version: version.into() }
    }
}

/// MCP Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// Tool metadata (`_meta`), e.g. `_meta.ui.resourceUri` for MCP Apps
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Tool call result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ToolContent>>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(
        rename = "structuredContent",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub structured_content: Option<Value>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// Tool content item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// List tools result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Resource content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

/// List resources result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourcesResult {
    pub resources: Vec<Resource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Prompt definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

/// Prompt argument
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// List prompts result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPromptsResult {
    pub prompts: Vec<Prompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod events_capability_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_capability_is_detected_in_every_place_it_may_be_advertised() {
        let caps = |v: Value| serde_json::from_value::<ServerCapabilities>(v).unwrap();
        assert!(caps(json!({ "events": { "listChanged": false } })).supports_events());
        assert!(caps(json!({ "events": {} })).supports_events());
        assert!(caps(json!({ "extensions": { "io.modelcontextprotocol/events": {} } })).supports_events());
        assert!(caps(json!({ "experimental": { "events": {} } })).supports_events());
        assert!(!caps(json!({ "tools": {} })).supports_events());
        assert!(!caps(json!({ "events": null })).supports_events());
        assert!(!caps(json!({ "events": false })).supports_events());
    }
}
