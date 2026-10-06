//! Error types for MCP client

use std::time::Duration;
use thiserror::Error;

/// Main error type for MCP client operations
#[derive(Debug, Error)]
pub enum McpError {
    /// Transport-level errors
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),

    /// JSON-RPC protocol errors
    #[error("JSON-RPC error: {message} (code: {code})")]
    JsonRpc {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },

    /// Protocol-level errors (MCP specific)
    #[error("protocol error: {0}")]
    Protocol(String),

    /// OAuth authentication errors
    #[error("OAuth error: {0}")]
    OAuth(#[from] OAuthError),

    /// Operation timeout
    #[error("timeout after {0:?}")]
    Timeout(Duration),

    /// Connection closed
    #[error("connection closed")]
    ConnectionClosed,

    /// Server not ready
    #[error("server not ready")]
    NotReady,

    /// Unknown tool
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    /// Tool execution failed
    #[error("tool execution failed: {0}")]
    ToolExecution(String),

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Storage/database error
    #[error("storage error: {0}")]
    Storage(String),
}

/// Transport-specific errors
#[derive(Debug, Clone, Error)]
pub enum TransportError {
    /// Stdio transport errors
    #[error("stdio error: {0}")]
    Stdio(String),

    /// HTTP transport errors
    #[error("HTTP error: {status} - {message}")]
    Http { status: u16, message: String },

    /// SSE transport errors
    #[error("SSE error: {0}")]
    Sse(String),

    /// Process management errors
    #[error("process error: {0}")]
    Process(String),

    /// Connection failed
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
}

/// OAuth-specific errors
#[derive(Debug, Error)]
pub enum OAuthError {
    /// Authorization required
    #[error("authorization required")]
    AuthorizationRequired { authorization_url: reqwest::Url },

    /// Invalid state parameter
    #[error("invalid state")]
    InvalidState,

    /// Token expired
    #[error("token expired")]
    TokenExpired,

    /// Token refresh failed
    #[error("refresh failed: {0}")]
    RefreshFailed(String),

    /// PKCE error
    #[error("PKCE error: {0}")]
    Pkce(String),

    /// HTTP error during OAuth flow
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Invalid client information
    #[error("invalid client information: {0}")]
    InvalidClient(String),

    /// Token storage error
    #[error("token storage error: {0}")]
    Storage(String),
}

/// Result type alias for MCP operations
pub type Result<T> = std::result::Result<T, McpError>;

/// Result type alias for MCP operations (alternative name)
pub type McpResult<T> = std::result::Result<T, McpError>;
