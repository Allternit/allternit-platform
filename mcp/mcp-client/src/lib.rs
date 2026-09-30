//! MCP (Model Context Protocol) Client for Allternit
//!
//! This crate provides a Rust implementation of the Model Context Protocol client,
//! supporting stdio, HTTP/SSE and streamable HTTP transports with OAuth 2.1 + PKCE authentication.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │         McpClientManager                │
//! └─────────────┬───────────────────────────┘
//!               │
//! ┌─────────────▼───────────────────────────┐
//! │           McpClient                     │
//! │  ┌─────────────┐  ┌─────────────────┐  │
//! │  │   Transport │  │  OAuth Provider │  │
//! │  │(Stdio/SSE/H)│  │  (Token Mgmt)   │  │
//! │  └─────────────┘  └─────────────────┘  │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use mcp_client::McpClient;
//! use mcp_client::StdioConfig;
//! use mcp_client::StdioTransport;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = StdioConfig {
//!         command: "mcp-server".to_string(),
//!         args: vec!["--stdio".to_string()],
//!         env: std::collections::HashMap::new(),
//!         cwd: None,
//!         timeout_secs: 30,
//!     };
//!
//!     let transport = StdioTransport::spawn(config).await?;
//!     let mut client = McpClient::new(transport);
//!     
//!     // Initialize the connection
//!     client.initialize().await?;
//!     
//!     // List available tools
//!     let tools = client.list_tools().await?;
//!     println!("Available tools: {:?}", tools);
//!     
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod health;
pub mod protocol;
pub mod registry;
pub mod transport;

// Re-export main types
pub use error::{McpError, OAuthError, Result, TransportError};
pub use protocol::{
    ClientCapabilities, InitializeParams, InitializeResult, ListResourcesResult, ListToolsResult,
    Resource, ResourceContent, ServerCapabilities, Tool, ToolResult,
};
pub use transport::sse::{ReconnectConfig, SseConfig};
pub use transport::stdio::StdioConfig;
pub use transport::{
    McpTransport, SseTransport, StdioTransport, StreamableHttpConfig, StreamableHttpTransport,
    TransportConfig, TransportType,
};

// Re-export registry types
pub use registry::{
    ConnectionState, McpRegistry, McpServerRecord, McpServerStatus, OAuthTokenRecord,
};

// Re-export health monitoring types
pub use health::{
    CircuitBreakerState, HealthMetrics, HealthMonitorConfig, McpHealthMonitor, ServerHealth,
};

use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info};

/// MCP Client implementation
///
/// This is the main entry point for interacting with MCP servers.
/// It handles the protocol initialization, tool discovery, and tool execution.
#[derive(Debug)]
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    capabilities: Option<ServerCapabilities>,
    initialized: bool,
}

impl McpClient {
    /// Create a new MCP client with the given transport
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        Self {
            transport,
            capabilities: None,
            initialized: false,
        }
    }

    /// Initialize the MCP connection
    ///
    /// This must be called before any other operations.
    /// It performs the MCP initialize handshake.
    pub async fn initialize(&mut self) -> Result<InitializeResult> {
        info!("Initializing MCP connection");

        let params = InitializeParams {
            protocol_version: protocol::MCP_PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities::with_mcp_apps(),
            client_info: protocol::Implementation {
                name: "allternit-mcp-client".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let result = self
            .transport
            .request("initialize", Some(serde_json::to_value(params)?))
            .await?;

        let init_result: InitializeResult = serde_json::from_value(result)?;
        self.capabilities = Some(init_result.capabilities.clone());
        self.initialized = true;

        // Send initialized notification
        self.transport
            .notify("notifications/initialized", None)
            .await?;

        info!(
            "MCP connection initialized with server: {} {}",
            init_result.server_info.name, init_result.server_info.version
        );

        Ok(init_result)
    }

    /// List available tools from the MCP server
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        self.ensure_initialized()?;

        debug!("Listing tools");
        let result = self.transport.request("tools/list", None).await?;

        let list_result: ListToolsResult = serde_json::from_value(result)?;
        Ok(list_result.tools)
    }

    /// Call a tool on the MCP server
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolResult> {
        self.ensure_initialized()?;

        debug!("Calling tool: {name}");
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let result = self.transport.request("tools/call", Some(params)).await?;
        let tool_result: ToolResult = serde_json::from_value(result)?;
        Ok(tool_result)
    }

    /// List available resources from the MCP server
    pub async fn list_resources(&self) -> Result<Vec<Resource>> {
        self.ensure_initialized()?;

        debug!("Listing resources");
        let result = self.transport.request("resources/list", None).await?;

        let list_result: ListResourcesResult = serde_json::from_value(result)?;
        Ok(list_result.resources)
    }

    /// Read a resource from the MCP server
    pub async fn read_resource(&self, uri: &str) -> Result<ResourceContent> {
        self.ensure_initialized()?;

        debug!("Reading resource: {uri}");
        let params = serde_json::json!({
            "uri": uri,
        });

        let result = self
            .transport
            .request("resources/read", Some(params))
            .await?;
        let content: ResourceContent = serde_json::from_value(result)?;
        Ok(content)
    }

    /// Send a raw JSON-RPC request and return the untouched `result`.
    ///
    /// Use for methods whose results carry fields the typed helpers drop
    /// (`_meta`, `structuredContent`, resource `contents`).
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.ensure_initialized()?;
        self.transport.request(method, params).await
    }

    /// Check if the client is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get the server capabilities (if initialized)
    pub fn capabilities(&self) -> Option<&ServerCapabilities> {
        self.capabilities.as_ref()
    }

    /// Check if the transport is healthy
    pub async fn is_healthy(&self) -> bool {
        self.transport.is_healthy().await
    }

    /// Shutdown the client connection
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("Shutting down MCP client");
        self.initialized = false;
        self.transport.close().await
    }

    fn ensure_initialized(&self) -> Result<()> {
        if !self.initialized {
            return Err(McpError::NotReady);
        }
        Ok(())
    }
}

/// MCP Client Manager for handling multiple MCP server connections
#[derive(Debug, Default)]
pub struct McpClientManager {
    clients: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<String, McpClient>>>,
}

impl McpClientManager {
    /// Create a new client manager
    pub fn new() -> Self {
        Self {
            clients: std::sync::Arc::new(
                tokio::sync::RwLock::new(std::collections::HashMap::new()),
            ),
        }
    }

    /// Add a client to the manager
    pub async fn add_client(&self, name: String, client: McpClient) {
        let mut clients = self.clients.write().await;
        clients.insert(name, client);
    }

    /// Get a client by name
    pub async fn get_client(&self, name: &str) -> Option<McpClient> {
        let clients = self.clients.read().await;
        clients.get(name).cloned()
    }

    /// Remove a client
    pub async fn remove_client(&self, name: &str) -> Option<McpClient> {
        let mut clients = self.clients.write().await;
        clients.remove(name)
    }

    /// List all client names
    pub async fn list_clients(&self) -> Vec<String> {
        let clients = self.clients.read().await;
        clients.keys().cloned().collect()
    }

    /// Shutdown all clients
    pub async fn shutdown_all(&self) {
        let mut clients = self.clients.write().await;
        for (name, client) in clients.iter_mut() {
            info!("Shutting down client: {name}");
            let _ = client.shutdown().await;
        }
        clients.clear();
    }
}

impl Clone for McpClient {
    fn clone(&self) -> Self {
        Self {
            transport: Arc::clone(&self.transport),
            capabilities: self.capabilities.clone(),
            initialized: self.initialized,
        }
    }
}
