//! Integration with Allternit tools-gateway
//!
//! This module provides integration between MCP servers and the Allternit tools-gateway,
//! enabling MCP tools to be executed through the standard Allternit tool execution flow.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Tool Execution Request                    │
//! └──────────────────────┬──────────────────────────────────────┘
//!                        │
//!           ┌────────────▼────────────┐
//!           │   McpToolProvider       │
//!           │  (implements ToolProvider)│
//!           └────────────┬────────────┘
//!                        │
//!        ┌───────────────┼───────────────┐
//!        │               │               │
//! ┌──────▼─────┐  ┌──────▼──────┐  ┌─────▼──────┐
//! │  Registry  │  │   Bridge    │  │   Client   │
//! │  Lookup    │  │  Transform  │  │   Pool     │
//! └────────────┘  └─────────────┘  └────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use mcp::gateway_integration::{McpClientPool, McpToolProvider, ToolProvider};
//! use mcp::tools_registry::McpToolsRegistry;
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let registry = Arc::new(McpToolsRegistry::new());
//! let client_pool = Arc::new(McpClientPool::new());
//!
//! let provider = McpToolProvider::new(registry, client_pool);
//!
//! // Discover available tools
//! let tools = provider.discover_tools().await;
//! # Ok(())
//! # }
//! ```

use crate::{
    tool_bridge::McpToolBridge,
    tools_registry::McpToolsRegistry,
    transport::McpTransport,
    types::{CallToolRequest, ToolResult},
};
use allternit_sdk_core::{ExecuteResponse, ToolGatewayDefinition};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info};

/// Error type for MCP tool provider operations
#[derive(Debug, thiserror::Error)]
pub enum McpToolProviderError {
    /// Tool not found in registry
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    /// MCP client not available
    #[error("MCP client not available for server: {0}")]
    ClientNotAvailable(String),

    /// MCP transport error
    #[error("MCP transport error: {0}")]
    TransportError(String),

    /// Tool execution error
    #[error("tool execution error: {0}")]
    ExecutionError(String),

    /// Serialization error
    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// Timeout error
    #[error("tool execution timeout")]
    Timeout,

    /// Server disconnected
    #[error("MCP server disconnected: {0}")]
    ServerDisconnected(String),
}

/// Result type for MCP tool provider operations
pub type McpToolProviderResult<T> = Result<T, McpToolProviderError>;

/// Trait for tool providers that can be registered with the tools-gateway
///
/// This trait defines the interface that the Allternit tools-gateway uses
/// to discover and execute tools from different sources.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Discover all tools available from this provider
    async fn discover_tools(&self) -> Vec<ToolGatewayDefinition>;

    /// Execute a tool with the given parameters
    ///
    /// # Arguments
    ///
    /// * `tool_id` - The full tool identifier (with prefix)
    /// * `parameters` - Tool parameters as JSON
    ///
    /// # Returns
    ///
    /// The tool execution result
    async fn execute_tool(
        &self,
        tool_id: &str,
        parameters: Value,
    ) -> McpToolProviderResult<ExecuteResponse>;

    /// Check if this provider handles a specific tool
    async fn handles_tool(&self, tool_id: &str) -> bool;
}

/// MCP Client Pool for managing connections to multiple MCP servers
///
/// This pool maintains active connections to MCP servers and provides
/// a way to route tool execution requests to the correct server.
pub struct McpClientPool {
    clients: Arc<RwLock<HashMap<String, Arc<Mutex<dyn McpTransport>>>>>,
}

impl std::fmt::Debug for McpClientPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClientPool")
            .field("clients", &"<dyn McpTransport map>")
            .finish()
    }
}

impl McpClientPool {
    /// Create a new empty client pool
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an MCP client for a server
    ///
    /// # Arguments
    ///
    /// * `server_id` - Unique identifier for the MCP server
    /// * `client` - The MCP transport client
    pub async fn register_client(&self, server_id: String, client: Arc<Mutex<dyn McpTransport>>) {
        info!(server_id = %server_id, "Registering MCP client");
        self.clients.write().await.insert(server_id, client);
    }

    /// Unregister an MCP client
    pub async fn unregister_client(&self, server_id: &str) {
        info!(server_id = %server_id, "Unregistering MCP client");
        self.clients.write().await.remove(server_id);
    }

    /// Get a client for a specific server
    pub async fn get_client(&self, server_id: &str) -> Option<Arc<Mutex<dyn McpTransport>>> {
        self.clients.read().await.get(server_id).cloned()
    }

    /// Check if a client exists for a server
    pub async fn has_client(&self, server_id: &str) -> bool {
        self.clients.read().await.contains_key(server_id)
    }

    /// Execute a tool call on a specific MCP server
    ///
    /// # Arguments
    ///
    /// * `server_id` - The MCP server ID
    /// * `request` - The tool call request
    ///
    /// # Returns
    ///
    /// The tool execution result
    pub async fn call_tool(
        &self,
        server_id: &str,
        request: CallToolRequest,
    ) -> McpToolProviderResult<ToolResult> {
        let client = self
            .get_client(server_id)
            .await
            .ok_or_else(|| McpToolProviderError::ClientNotAvailable(server_id.to_string()))?;

        let params = serde_json::to_value(&request)?;

        // One request/response exchange through the transport (which matches
        // the reply by id), instead of a separate send + receive that could
        // miss a reply arriving before the receive subscribed.
        let result = {
            let client_guard = client.lock().await;
            client_guard.request("tools/call", Some(params)).await
        };
        match result {
            Ok(value) => Ok(serde_json::from_value(value)?),
            Err(crate::McpError::JsonRpc { message, .. }) => Err(McpToolProviderError::ExecutionError(message)),
            Err(crate::McpError::ConnectionClosed) => {
                Err(McpToolProviderError::ServerDisconnected(server_id.to_string()))
            }
            Err(e) => Err(McpToolProviderError::TransportError(e.to_string())),
        }
    }

    /// Get all registered server IDs
    pub async fn get_server_ids(&self) -> Vec<String> {
        self.clients.read().await.keys().cloned().collect()
    }

    /// Get the number of registered clients
    pub async fn client_count(&self) -> usize {
        self.clients.read().await.len()
    }
}

impl Default for McpClientPool {
    fn default() -> Self {
        Self::new()
    }
}

/// MCP Tool Provider implementation
///
/// This struct implements the `ToolProvider` trait, allowing MCP tools
/// to be registered with and executed through the Allternit tools-gateway.
#[derive(Debug)]
pub struct McpToolProvider {
    registry: Arc<McpToolsRegistry>,
    client_pool: Arc<McpClientPool>,
    bridges: Arc<RwLock<HashMap<String, McpToolBridge>>>,
}

impl McpToolProvider {
    /// Create a new MCP tool provider
    ///
    /// # Arguments
    ///
    /// * `registry` - The tools registry for looking up tool ownership
    /// * `client_pool` - The client pool for executing tool calls
    pub fn new(registry: Arc<McpToolsRegistry>, client_pool: Arc<McpClientPool>) -> Self {
        Self {
            registry,
            client_pool,
            bridges: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a tool bridge for a server
    ///
    /// This associates an `McpToolBridge` with a server ID for proper
    /// tool name transformation.
    pub async fn register_bridge(&self, server_id: String, bridge: McpToolBridge) {
        self.bridges.write().await.insert(server_id, bridge);
    }

    /// Get or create a bridge for a server
    async fn get_bridge(&self, server_id: &str, server_name: &str) -> McpToolBridge {
        let bridges = self.bridges.read().await;
        if let Some(bridge) = bridges.get(server_id) {
            bridge.clone()
        } else {
            drop(bridges);
            let bridge = McpToolBridge::new(server_name.to_string());
            self.bridges
                .write()
                .await
                .insert(server_id.to_string(), bridge.clone());
            bridge
        }
    }
}

#[async_trait]
impl ToolProvider for McpToolProvider {
    async fn discover_tools(&self) -> Vec<ToolGatewayDefinition> {
        self.registry.get_all_definitions().await
    }

    async fn execute_tool(
        &self,
        tool_id: &str,
        parameters: Value,
    ) -> McpToolProviderResult<ExecuteResponse> {
        let start = std::time::Instant::now();

        // Find which MCP server owns this tool
        let registered = self
            .registry
            .lookup(tool_id)
            .await
            .ok_or_else(|| McpToolProviderError::ToolNotFound(tool_id.to_string()))?;

        debug!(
            tool_id = %tool_id,
            server_id = %registered.server_id,
            server_name = %registered.server_name,
            "Executing MCP tool"
        );

        // Get the bridge for this server
        let bridge = self
            .get_bridge(&registered.server_id, &registered.server_name)
            .await;

        // Convert Allternit request to MCP request
        let mcp_request = bridge.to_mcp_request(tool_id, parameters);

        // Execute via the client pool
        let mcp_result = self
            .client_pool
            .call_tool(&registered.server_id, mcp_request)
            .await?;

        // Convert MCP result to Allternit response
        let (output, error, execution_time_ms) =
            bridge.to_allternit_result(mcp_result, start.elapsed().as_millis() as u64);

        let success = error.is_none();

        Ok(ExecuteResponse {
            success,
            result: if success { Some(output) } else { None },
            error,
            execution_time_ms,
            ui_card: None,
        })
    }

    async fn handles_tool(&self, tool_id: &str) -> bool {
        self.registry.is_registered(tool_id).await
    }
}

/// Tool router for directing tool calls to the appropriate provider
///
/// This router manages multiple tool providers and routes execution
/// requests to the correct one based on tool ID.
pub struct ToolRouter {
    providers: Arc<RwLock<Vec<Arc<dyn ToolProvider>>>>,
    fallback_provider: Arc<RwLock<Option<Arc<dyn ToolProvider>>>>,
}

impl std::fmt::Debug for ToolRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRouter")
            .field("providers", &"<dyn ToolProvider list>")
            .field("fallback_provider", &"<dyn ToolProvider option>")
            .finish()
    }
}

impl ToolRouter {
    /// Create a new tool router
    pub fn new() -> Self {
        Self {
            providers: Arc::new(RwLock::new(Vec::new())),
            fallback_provider: Arc::new(RwLock::new(None)),
        }
    }

    /// Register a tool provider
    pub async fn register_provider(&self, provider: Arc<dyn ToolProvider>) {
        self.providers.write().await.push(provider);
    }

    /// Set a fallback provider for tools not handled by other providers
    pub async fn set_fallback_provider(&self, provider: Arc<dyn ToolProvider>) {
        *self.fallback_provider.write().await = Some(provider);
    }

    /// Route a tool execution to the appropriate provider
    pub async fn execute_tool(
        &self,
        tool_id: &str,
        parameters: Value,
    ) -> McpToolProviderResult<ExecuteResponse> {
        // Find a provider that handles this tool
        let providers = self.providers.read().await;

        for provider in providers.iter() {
            if provider.handles_tool(tool_id).await {
                return provider.execute_tool(tool_id, parameters).await;
            }
        }
        drop(providers);

        // Try fallback provider
        if let Some(fallback) = self.fallback_provider.read().await.as_ref() {
            return fallback.execute_tool(tool_id, parameters).await;
        }

        Err(McpToolProviderError::ToolNotFound(tool_id.to_string()))
    }

    /// Discover all tools from all providers
    pub async fn discover_all_tools(&self) -> Vec<ToolGatewayDefinition> {
        let mut all_tools = Vec::new();

        for provider in self.providers.read().await.iter() {
            let tools = provider.discover_tools().await;
            all_tools.extend(tools);
        }

        all_tools
    }
}

impl Default for ToolRouter {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating MCP tool providers
pub struct McpToolProviderBuilder {
    registry: Option<Arc<McpToolsRegistry>>,
    client_pool: Option<Arc<McpClientPool>>,
}

impl McpToolProviderBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            registry: None,
            client_pool: None,
        }
    }

    /// Set the tools registry
    pub fn with_registry(mut self, registry: Arc<McpToolsRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Set the client pool
    pub fn with_client_pool(mut self, client_pool: Arc<McpClientPool>) -> Self {
        self.client_pool = Some(client_pool);
        self
    }

    /// Build the MCP tool provider
    pub fn build(self) -> Result<McpToolProvider, McpToolProviderError> {
        let registry = self
            .registry
            .ok_or_else(|| McpToolProviderError::ExecutionError("Registry required".to_string()))?;

        let client_pool = self.client_pool.ok_or_else(|| {
            McpToolProviderError::ExecutionError("Client pool required".to_string())
        })?;

        Ok(McpToolProvider::new(registry, client_pool))
    }
}

impl Default for McpToolProviderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools_registry::McpToolsRegistry;

    // Note: These tests require mocking the transport layer
    // For now, we just test the basic structure

    #[tokio::test]
    async fn test_client_pool_new() {
        let pool = McpClientPool::new();
        assert_eq!(pool.client_count().await, 0);
    }

    #[tokio::test]
    async fn test_tool_provider_handles_tool() {
        let registry = Arc::new(McpToolsRegistry::new());
        let client_pool = Arc::new(McpClientPool::new());
        let provider = McpToolProvider::new(registry.clone(), client_pool);

        // Initially no tools registered
        assert!(!provider.handles_tool("filesystem.read_file").await);
    }

    #[tokio::test]
    async fn test_tool_router_new() {
        let router = ToolRouter::new();
        let tools = router.discover_all_tools().await;
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_builder() {
        let registry = Arc::new(McpToolsRegistry::new());
        let client_pool = Arc::new(McpClientPool::new());

        let result = McpToolProviderBuilder::new()
            .with_registry(registry)
            .with_client_pool(client_pool)
            .build();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_builder_missing_registry() {
        let client_pool = Arc::new(McpClientPool::new());

        let result = McpToolProviderBuilder::new()
            .with_client_pool(client_pool)
            .build();

        assert!(result.is_err());
    }
}
