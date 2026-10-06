//! MCP (Model Context Protocol) Client for Allternit
//!
//! This crate provides a Rust implementation of the Model Context Protocol client,
//! supporting both stdio and HTTP/SSE transports with OAuth 2.1 + PKCE authentication,
//! health monitoring, and full Allternit tool gateway integration.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                        MCP Client Architecture                               │
//! ├─────────────────────────────────────────────────────────────────────────────┤
//! │                                                                              │
//! │   Core MCP Client                                                            │
//! │   ┌─────────────────────────────────────────────────────────────────────┐   │
//! │   │  McpClient                                                          │   │
//! │   │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────────┐ │   │
//! │   │  │  Transport  │  │    OAuth    │  │         Registry            │ │   │
//! │   │  │ (Stdio/SSE) │  │   (PKCE)    │  │   (SQLite + Health Monitor) │ │   │
//! │   │  └─────────────┘  └─────────────┘  └─────────────────────────────┘ │   │
//! │   └─────────────────────────────────────────────────────────────────────┘   │
//! │                               ↓                                              │
//! │   Allternit Integration                                                            │
//! │   ┌─────────────────────────────────────────────────────────────────────┐   │
//! │   │  McpToolBridge ←→ McpToolsRegistry ←→ ToolProvider                │   │
//! │   │       ↓                    ↓                                         │   │
//! │   │  McpGatewayIntegration    Policy Enforcement                        │   │
//! │   │       ↓                                                              │   │
//! │   │   tools-gateway                                                      │   │
//! │   └─────────────────────────────────────────────────────────────────────┘   │
//! │                                                                              │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # One client implementation
//!
//! The protocol, transports (stdio, Streamable HTTP — dual-era per MCP
//! 2026-07-28 — and legacy HTTP+SSE), OAuth, registry and health monitoring
//! are the `mcp-client` crate's, re-exported here unchanged; this crate adds
//! only the Allternit integration layer (policy enforcement, the tool bridge
//! and registry, and the tools-gateway provider).
//!
//! # Example: Basic MCP Client
//!
//! ```rust,no_run
//! use mcp::McpClient;
//! use mcp::StdioConfig;
//! use mcp::StdioTransport;
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

// The client itself: one implementation, owned by `mcp-client`.
pub use mcp_client::{bridge, error, health, oauth, protocol, registry, transport};

pub mod policy;
pub mod types_allternit;

// Re-export types_allternit as types for backwards compatibility
pub use types_allternit as types;

// Allternit-specific integration modules
pub mod gateway_integration;
pub mod tool_bridge;
pub mod tools_registry;

// Re-export main types
pub use mcp_client::{clear_era_cache, McpClient, McpClientManager, ProtocolEra};
pub use error::{McpError, McpResult, OAuthError, Result, TransportError};
pub use protocol::{
    ClientCapabilities, InitializeParams, InitializeResult, ListResourcesResult, ListToolsResult,
    Resource, ResourceContent, ServerCapabilities, Tool, ToolResult as ProtocolToolResult,
};
pub use transport::sse::{ReconnectConfig, SseConfig};
pub use transport::stdio::StdioConfig;
pub use transport::{
    McpTransport, SseTransport, StdioTransport, StreamableHttpConfig, StreamableHttpTransport,
    TransportConfig, TransportType,
};
pub use types_allternit::{CallToolRequest, ToolContent, ToolResult};

// Re-export registry types
pub use registry::{
    ConnectionState, McpRegistry, McpServerRecord, McpServerStatus, OAuthTokenRecord,
};

// Re-export health monitoring types
pub use health::{
    CircuitBreakerState, HealthMetrics, HealthMonitorConfig, McpHealthMonitor, ServerHealth,
};
