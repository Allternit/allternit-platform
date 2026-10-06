//! MCP Tool Bridge for tools-gateway (N12)
//!
//! Integrates MCP (Model Context Protocol) tools with the Allternit tool registry.
//! Enables policy enforcement, result sanitization, and audit logging for MCP calls.

use crate::{
    ResourceUsage, SafetyTier, ToolExecutionRequest, ToolExecutionResult, ToolGatewayError,
};
// async_trait not needed currently
use mcp_client::{
    McpClient, McpClientManager, SseConfig, SseTransport, StdioConfig, StdioTransport,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use utoipa::ToSchema;

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpServerConfig {
    /// Unique server identifier
    pub server_id: String,
    /// Server display name
    pub name: String,
    /// Server description
    pub description: String,
    /// Transport type (stdio or sse)
    pub transport_type: McpTransportType,
    /// Stdio configuration (if applicable)
    pub stdio_config: Option<McpStdioConfig>,
    /// SSE configuration (if applicable)
    pub sse_config: Option<McpSseConfig>,
    /// Safety tier for this server's tools
    pub safety_tier: SafetyTier,
    /// Whether the server is enabled
    pub enabled: bool,
    /// Auto-connect on startup
    pub auto_connect: bool,
    /// Tool name prefix (defaults to server_id)
    pub tool_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum McpTransportType {
    #[serde(rename = "stdio")]
    Stdio,
    #[serde(rename = "sse")]
    Sse,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpStdioConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpSseConfig {
    pub url: String,
    pub auth_token: Option<String>,
    pub timeout_secs: u64,
}

/// MCP tool bridge - manages MCP connections and tool execution
#[derive(Debug)]
pub struct McpToolBridge {
    /// MCP client manager for handling connections
    client_manager: Arc<McpClientManager>,
    /// Server configurations
    server_configs: Arc<RwLock<HashMap<String, McpServerConfig>>>,
    /// Connected clients (server_id -> client)
    connected_clients: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<McpClient>>>>>,
    /// Tool mappings (prefixed_tool_name -> (server_id, original_tool_name))
    tool_mappings: Arc<RwLock<HashMap<String, (String, String)>>>,
}

impl McpToolBridge {
    /// Create a new MCP tool bridge
    pub fn new() -> Self {
        Self {
            client_manager: Arc::new(McpClientManager::new()),
            server_configs: Arc::new(RwLock::new(HashMap::new())),
            connected_clients: Arc::new(RwLock::new(HashMap::new())),
            tool_mappings: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an MCP server configuration
    pub async fn register_server(&self, config: McpServerConfig) -> Result<(), McpBridgeError> {
        let server_id = config.server_id.clone();
        info!("Registering MCP server: {}", server_id);

        // Store configuration
        let mut configs = self.server_configs.write().await;
        configs.insert(server_id.clone(), config.clone());
        drop(configs);

        // Auto-connect if enabled
        if config.auto_connect && config.enabled {
            self.connect_server(&server_id).await?;
        }

        Ok(())
    }

    /// Connect to an MCP server
    pub async fn connect_server(&self, server_id: &str) -> Result<(), McpBridgeError> {
        let config = {
            let configs = self.server_configs.read().await;
            configs
                .get(server_id)
                .cloned()
                .ok_or_else(|| McpBridgeError::ServerNotFound(server_id.to_string()))?
        };

        if !config.enabled {
            return Err(McpBridgeError::ServerDisabled(server_id.to_string()));
        }

        info!("Connecting to MCP server: {}", server_id);

        // Create transport based on type
        let client = match config.transport_type {
            McpTransportType::Stdio => {
                let stdio_config = config.stdio_config.ok_or_else(|| {
                    McpBridgeError::InvalidConfig("Missing stdio_config".to_string())
                })?;

                let transport_config = StdioConfig {
                    command: stdio_config.command,
                    args: stdio_config.args,
                    env: stdio_config.env,
                    cwd: stdio_config.cwd.map(std::path::PathBuf::from),
                    timeout_secs: stdio_config.timeout_secs,
                };

                let transport = StdioTransport::spawn(transport_config)
                    .await
                    .map_err(|e| McpBridgeError::ConnectionFailed(e.to_string()))?;

                let mut client = McpClient::new(transport);
                client
                    .initialize()
                    .await
                    .map_err(|e| McpBridgeError::InitializationFailed(e.to_string()))?;

                client
            }
            McpTransportType::Sse => {
                let sse_config = config.sse_config.ok_or_else(|| {
                    McpBridgeError::InvalidConfig("Missing sse_config".to_string())
                })?;

                let transport_config = SseConfig {
                    url: sse_config.url,
                    sse_path: None,
                    post_path: None,
                    auth_token: sse_config.auth_token,
                    timeout_secs: sse_config.timeout_secs,
                    reconnect: mcp_client::transport::sse::ReconnectConfig::default(),
                    // No address pinning here: this bridge connects to
                    // operator-configured servers, not user-supplied URLs.
                    pin: None,
                };

                let transport = SseTransport::new(transport_config)
                    .map_err(|e| McpBridgeError::ConnectionFailed(e.to_string()))?;

                let mut client = McpClient::new(transport);
                client
                    .initialize()
                    .await
                    .map_err(|e| McpBridgeError::InitializationFailed(e.to_string()))?;

                client
            }
        };

        // Store connected client
        let mut clients = self.connected_clients.write().await;
        clients.insert(
            server_id.to_string(),
            Arc::new(tokio::sync::Mutex::new(client)),
        );
        drop(clients);

        // Register tools from this server
        self.register_server_tools(server_id).await?;

        info!("Successfully connected to MCP server: {}", server_id);
        Ok(())
    }

    /// Disconnect from an MCP server
    pub async fn disconnect_server(&self, server_id: &str) -> Result<(), McpBridgeError> {
        info!("Disconnecting from MCP server: {}", server_id);

        // Remove client
        let mut clients = self.connected_clients.write().await;
        if let Some(client) = clients.remove(server_id) {
            let mut client = client.lock().await;
            let _ = client.shutdown().await;
        }
        drop(clients);

        // Remove tool mappings for this server
        let mut mappings = self.tool_mappings.write().await;
        mappings.retain(|_, (sid, _)| sid != server_id);
        drop(mappings);

        info!("Disconnected from MCP server: {}", server_id);
        Ok(())
    }

    /// Register tools from a connected MCP server
    async fn register_server_tools(&self, server_id: &str) -> Result<(), McpBridgeError> {
        let client = {
            let clients = self.connected_clients.read().await;
            clients
                .get(server_id)
                .cloned()
                .ok_or_else(|| McpBridgeError::NotConnected(server_id.to_string()))?
        };

        let config = {
            let configs = self.server_configs.read().await;
            configs
                .get(server_id)
                .cloned()
                .ok_or_else(|| McpBridgeError::ServerNotFound(server_id.to_string()))?
        };

        // List tools from MCP server
        let tools = {
            let client = client.lock().await;
            client
                .list_tools()
                .await
                .map_err(|e| McpBridgeError::ToolDiscoveryFailed(e.to_string()))?
        };

        let prefix = config.tool_prefix.unwrap_or_else(|| server_id.to_string());

        let mut mappings = self.tool_mappings.write().await;
        for tool in &tools {
            let prefixed_name = format_tool_name(&prefix, &tool.name);
            mappings.insert(
                prefixed_name.clone(),
                (server_id.to_string(), tool.name.clone()),
            );
            debug!(
                "Registered MCP tool mapping: {} -> {}:{}",
                prefixed_name, server_id, tool.name
            );
        }
        drop(mappings);

        info!(
            "Registered {} tools from MCP server: {}",
            tools.len(),
            server_id
        );
        Ok(())
    }

    /// Execute an MCP tool
    pub async fn execute_mcp_tool(
        &self,
        tool_id: &str, // This is the prefixed tool name (e.g., "mcp__filesystem__read_file")
        request: &ToolExecutionRequest,
    ) -> Result<ToolExecutionResult, McpBridgeError> {
        let start_time = std::time::Instant::now();

        // Parse the tool name to get server and original tool name
        let (server_id, original_tool_name) = {
            let mappings = self.tool_mappings.read().await;
            mappings
                .get(tool_id)
                .cloned()
                .ok_or_else(|| McpBridgeError::ToolNotFound(tool_id.to_string()))?
        };

        debug!(
            "Executing MCP tool: {} (server: {}, tool: {})",
            tool_id, server_id, original_tool_name
        );

        // Get the client
        let client = {
            let clients = self.connected_clients.read().await;
            clients
                .get(&server_id)
                .cloned()
                .ok_or_else(|| McpBridgeError::NotConnected(server_id.clone()))?
        };

        // Execute the tool
        let result = {
            let client = client.lock().await;
            client
                .call_tool(&original_tool_name, request.input.clone())
                .await
                .map_err(|e| McpBridgeError::ExecutionFailed(e.to_string()))?
        };

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        // Convert MCP result to Allternit result
        let output = if result.is_error.unwrap_or(false) {
            serde_json::json!({
                "error": true,
                "content": result.content
            })
        } else {
            serde_json::json!({
                "result": result.content
            })
        };

        let tool_result = ToolExecutionResult {
            execution_id: uuid::Uuid::new_v4().to_string(),
            tool_id: tool_id.to_string(),
            input: request.input.clone(),
            output: Some(output),
            error: if result.is_error.unwrap_or(false) {
                Some(format!("MCP tool error: {:?}", result.content))
            } else {
                None
            },
            stdout: String::new(),
            stderr: String::new(),
            exit_code: if result.is_error.unwrap_or(false) {
                Some(1)
            } else {
                Some(0)
            },
            execution_time_ms,
            resources_used: ResourceUsage {
                cpu_time_ms: 0,
                memory_peak_kb: 0,
                network_bytes: 0,
                filesystem_ops: 0,
            },
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        Ok(tool_result)
    }

    /// Get all registered MCP tool definitions
    pub async fn get_mcp_tool_definitions(&self) -> Result<Vec<McpToolDefinition>, McpBridgeError> {
        let mut definitions = Vec::new();
        let mappings = self.tool_mappings.read().await;

        for (prefixed_name, (server_id, original_name)) in mappings.iter() {
            if let Some(config) = self.server_configs.read().await.get(server_id).cloned() {
                definitions.push(McpToolDefinition {
                    prefixed_name: prefixed_name.clone(),
                    server_id: server_id.clone(),
                    original_name: original_name.clone(),
                    server_name: config.name,
                    safety_tier: config.safety_tier,
                });
            }
        }

        Ok(definitions)
    }

    /// Check if a tool ID is an MCP tool
    pub async fn is_mcp_tool(&self, tool_id: &str) -> bool {
        let mappings = self.tool_mappings.read().await;
        mappings.contains_key(tool_id)
    }

    /// List all connected servers
    pub async fn list_connected_servers(&self) -> Vec<String> {
        let clients = self.connected_clients.read().await;
        clients.keys().cloned().collect()
    }

    /// Get server status
    pub async fn get_server_status(
        &self,
        server_id: &str,
    ) -> Result<McpServerStatus, McpBridgeError> {
        let config = {
            let configs = self.server_configs.read().await;
            configs
                .get(server_id)
                .cloned()
                .ok_or_else(|| McpBridgeError::ServerNotFound(server_id.to_string()))?
        };

        let connected = {
            let clients = self.connected_clients.read().await;
            clients.contains_key(server_id)
        };

        let healthy = if connected {
            if let Some(client) = self.connected_clients.read().await.get(server_id).cloned() {
                let client = client.lock().await;
                client.is_healthy().await
            } else {
                false
            }
        } else {
            false
        };

        let tool_count = {
            let mappings = self.tool_mappings.read().await;
            mappings
                .values()
                .filter(|(sid, _)| sid == server_id)
                .count()
        };

        Ok(McpServerStatus {
            server_id: server_id.to_string(),
            name: config.name,
            enabled: config.enabled,
            connected,
            healthy,
            tool_count,
        })
    }
}

impl Default for McpToolBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// MCP tool definition (simplified for registry)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub prefixed_name: String,
    pub server_id: String,
    pub original_name: String,
    pub server_name: String,
    pub safety_tier: SafetyTier,
}

/// MCP server status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub server_id: String,
    pub name: String,
    pub enabled: bool,
    pub connected: bool,
    pub healthy: bool,
    pub tool_count: usize,
}

/// MCP bridge errors
#[derive(Debug, thiserror::Error)]
pub enum McpBridgeError {
    #[error("Server not found: {0}")]
    ServerNotFound(String),
    #[error("Server disabled: {0}")]
    ServerDisabled(String),
    #[error("Not connected to server: {0}")]
    NotConnected(String),
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    #[error("Tool discovery failed: {0}")]
    ToolDiscoveryFailed(String),
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("MCP client error: {0}")]
    McpClientError(String),
}

impl From<McpBridgeError> for ToolGatewayError {
    fn from(err: McpBridgeError) -> Self {
        match err {
            McpBridgeError::ToolNotFound(_) => ToolGatewayError::ToolNotFound(err.to_string()),
            McpBridgeError::ExecutionFailed(msg) => ToolGatewayError::ExecutionFailed(msg),
            _ => ToolGatewayError::ExecutionFailed(format!("MCP bridge error: {}", err)),
        }
    }
}

/// Format a tool name with prefix
/// Format: "mcp__{prefix}__{tool_name}"
pub fn format_tool_name(prefix: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_name(prefix),
        sanitize_name(tool_name)
    )
}

/// Parse a prefixed tool name
pub fn parse_tool_name(prefixed: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = prefixed.splitn(3, "__").collect();
    if parts.len() == 3 && parts[0] == "mcp" {
        Some((parts[1].to_string(), parts[2].to_string()))
    } else {
        None
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tool_name() {
        assert_eq!(
            format_tool_name("filesystem", "read_file"),
            "mcp__filesystem__read_file"
        );
        // Note: sanitize_name preserves '-' and '_'
        assert_eq!(
            format_tool_name("my-server", "my-tool"),
            "mcp__my-server__my-tool"
        );
    }

    #[test]
    fn test_parse_tool_name() {
        let result = parse_tool_name("mcp__filesystem__read_file");
        assert_eq!(
            result,
            Some(("filesystem".to_string(), "read_file".to_string()))
        );

        let result = parse_tool_name("mcp__my_server__my_tool");
        assert_eq!(
            result,
            Some(("my_server".to_string(), "my_tool".to_string()))
        );
    }

    #[test]
    fn test_parse_invalid_tool_name() {
        assert_eq!(parse_tool_name("invalid"), None);
        assert_eq!(parse_tool_name("not__mcp__format"), None);
        assert_eq!(parse_tool_name("local_tool"), None);
    }

    #[tokio::test]
    async fn test_mcp_bridge_creation() {
        let bridge = McpToolBridge::new();
        let servers = bridge.list_connected_servers().await;
        assert!(servers.is_empty());
    }

    #[test]
    fn test_sanitize_name() {
        assert_eq!(sanitize_name("hello-world"), "hello-world");
        assert_eq!(sanitize_name("hello world"), "hello_world");
        assert_eq!(sanitize_name("hello/world"), "hello_world");
        assert_eq!(sanitize_name("tool@123"), "tool_123");
    }
}
