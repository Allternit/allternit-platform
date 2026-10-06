//! Allternit integration types for the policy / tool-bridge / gateway layer.
//!
//! The JSON-RPC and MCP wire types (and the protocol version) come from the
//! `mcp-client` crate — the one client implementation — and are re-exported
//! here so `mcp::types::JsonRpcRequest` etc. keep working. Only the types
//! this crate shapes differently for the Allternit tool gateway live here:
//! a tool descriptor without `_meta`, a typed `ToolContent` enum and the
//! `CallToolRequest` the policy engine evaluates.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use mcp_client::protocol::{
    ClientCapabilities, Implementation, InitializeParams as InitializeRequest, InitializeResult, JsonRpcError,
    JsonRpcErrorResponse, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, ListResourcesResult,
    Resource, ResourceContent, ServerCapabilities, JSONRPC_VERSION, MCP_PROTOCOL_VERSION,
};

/// MCP Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// MCP Tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// Tool content (text or image)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
}

/// List tools result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Call tool request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolRequest {
    pub name: String,
    pub arguments: Option<Value>,
}

/// Read resource result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContent>,
}
