//! Transport layer for MCP client

use async_trait::async_trait;
use serde_json::Value;

use crate::protocol::JsonRpcMessage;

pub mod sse;
pub mod stdio;
pub mod streamable_http;

pub use sse::{ReconnectConfig, SseConfig, SseTransport};
pub use stdio::StdioTransport;
pub use streamable_http::{StreamableHttpConfig, StreamableHttpTransport};

/// Type of transport
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportType {
    /// Standard input/output transport (local process)
    Stdio,
    /// Server-Sent Events transport (HTTP)
    Sse,
    /// Streamable HTTP transport (legacy 2025-03-26..2025-11-25 sessions, and
    /// stateless 2026-07-28)
    StreamableHttp,
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportType::Stdio => write!(f, "stdio"),
            TransportType::Sse => write!(f, "sse"),
            TransportType::StreamableHttp => write!(f, "streamable_http"),
        }
    }
}

/// Transport trait for MCP communication
///
/// Implementations handle the low-level communication with MCP servers,
/// whether through stdio, HTTP/SSE, or other protocols.
#[async_trait]
pub trait McpTransport: Send + Sync + std::fmt::Debug {
    /// Send a JSON-RPC request and wait for a response
    ///
    /// # Arguments
    /// * `method` - The JSON-RPC method name
    /// * `params` - Optional parameters for the method
    ///
    /// # Returns
    /// The JSON-RPC result value or an error
    async fn request(&self, method: &str, params: Option<Value>) -> crate::error::Result<Value>;

    /// Send a JSON-RPC notification (no response expected)
    ///
    /// # Arguments
    /// * `method` - The JSON-RPC method name
    /// * `params` - Optional parameters for the method
    async fn notify(&self, method: &str, params: Option<Value>) -> crate::error::Result<()>;

    /// Send a raw JSON-RPC message. Only the stdio transport (and the POST
    /// half of the HTTP transports) support this; the default refuses.
    async fn send(&self, message: JsonRpcMessage) -> crate::error::Result<()> {
        let _ = message;
        Err(crate::error::McpError::Protocol(format!(
            "{} transport does not support raw send; use request()",
            self.transport_type()
        )))
    }

    /// Receive the next raw JSON-RPC message the server sends (stdio only;
    /// subscribe before sending — earlier messages are not replayed). The
    /// default refuses.
    async fn receive(&self) -> crate::error::Result<Option<JsonRpcMessage>> {
        Err(crate::error::McpError::Protocol(format!(
            "{} transport does not support raw receive; use request()",
            self.transport_type()
        )))
    }

    /// Check if the transport is healthy and connected
    async fn is_healthy(&self) -> bool;

    /// Close the transport connection
    async fn close(&self) -> crate::error::Result<()>;

    /// Get the transport type
    fn transport_type(&self) -> TransportType;

    /// Key under which the server's protocol era is cached process-wide
    /// (the HTTP origin). `None` (the default) disables caching, e.g. for a
    /// stdio child whose era lives only as long as the process.
    fn era_cache_key(&self) -> Option<String> {
        None
    }
}

// Any shared transport is a transport.
#[async_trait]
impl<T> McpTransport for std::sync::Arc<T>
where
    T: McpTransport + ?Sized,
{
    async fn request(&self, method: &str, params: Option<Value>) -> crate::error::Result<Value> {
        (**self).request(method, params).await
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> crate::error::Result<()> {
        (**self).notify(method, params).await
    }

    async fn send(&self, message: JsonRpcMessage) -> crate::error::Result<()> {
        (**self).send(message).await
    }

    async fn receive(&self) -> crate::error::Result<Option<JsonRpcMessage>> {
        (**self).receive().await
    }

    async fn is_healthy(&self) -> bool {
        (**self).is_healthy().await
    }

    async fn close(&self) -> crate::error::Result<()> {
        (**self).close().await
    }

    fn transport_type(&self) -> TransportType {
        (**self).transport_type()
    }

    fn era_cache_key(&self) -> Option<String> {
        (**self).era_cache_key()
    }
}

/// Transport configuration
#[derive(Debug, Clone)]
pub enum TransportConfig {
    /// Stdio transport configuration
    Stdio {
        /// Command to spawn
        command: String,
        /// Arguments for the command
        args: Vec<String>,
        /// Environment variables
        env: std::collections::HashMap<String, String>,
        /// Working directory
        cwd: Option<std::path::PathBuf>,
    },
    /// SSE transport configuration
    Sse {
        /// Server URL
        url: String,
        /// Authentication token
        auth_token: Option<String>,
        /// Request timeout
        timeout_secs: u64,
    },
    /// Streamable HTTP transport configuration
    StreamableHttp(StreamableHttpConfig),
}

impl TransportConfig {
    /// Create a new stdio transport configuration
    pub fn stdio(command: impl Into<String>, args: Vec<String>) -> Self {
        TransportConfig::Stdio {
            command: command.into(),
            args,
            env: std::collections::HashMap::new(),
            cwd: None,
        }
    }

    /// Create a new stdio transport with environment variables
    pub fn stdio_with_env(
        command: impl Into<String>,
        args: Vec<String>,
        env: std::collections::HashMap<String, String>,
    ) -> Self {
        TransportConfig::Stdio {
            command: command.into(),
            args,
            env,
            cwd: None,
        }
    }

    /// Create a new SSE transport configuration
    pub fn sse(url: impl Into<String>) -> Self {
        TransportConfig::Sse {
            url: url.into(),
            auth_token: None,
            timeout_secs: 60,
        }
    }

    /// Create a new streamable HTTP transport configuration
    pub fn streamable_http(url: impl Into<String>) -> Self {
        TransportConfig::StreamableHttp(StreamableHttpConfig::new(url))
    }

    /// Get the transport type
    pub fn transport_type(&self) -> TransportType {
        match self {
            TransportConfig::Stdio { .. } => TransportType::Stdio,
            TransportConfig::Sse { .. } => TransportType::Sse,
            TransportConfig::StreamableHttp(_) => TransportType::StreamableHttp,
        }
    }
}
