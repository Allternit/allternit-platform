//! MCP (Model Context Protocol) Client for Allternit
//!
//! This crate provides a Rust implementation of the Model Context Protocol client,
//! supporting stdio, HTTP/SSE and streamable HTTP transports with OAuth 2.1 + PKCE authentication.
//!
//! Dual-era per MCP 2026-07-28: [`McpClient::initialize`] tries the stateless
//! modern protocol first and falls back to the legacy `initialize` handshake
//! (and callers to the deprecated HTTP+SSE transport) only when the server
//! does not speak it. Versions and the era rules come from `mcp-protocol`.
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

pub mod bridge;
pub mod error;
pub mod health;
pub mod oauth;
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

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tracing::{debug, info};

use mcp_protocol::client::{self as dual, ProbeVerdict};

/// How long a stdio era probe (`server/discover`) may go unanswered before
/// the server is taken to be legacy (spec: "the probe returns a non-modern
/// error or times out, and the client falls back to `initialize`").
const STDIO_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The era a connected server speaks, as negotiated by [`McpClient::initialize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolEra {
    /// Not connected yet.
    Unknown,
    /// Stateless 2026-07-28+: every request carries `_meta` and headers.
    Modern { version: String },
    /// Handshake era: `initialize` negotiated this version.
    Legacy { version: String },
}

impl ProtocolEra {
    pub fn is_modern(&self) -> bool {
        matches!(self, ProtocolEra::Modern { .. })
    }

    /// The negotiated protocol version, once connected.
    pub fn version(&self) -> Option<&str> {
        match self {
            ProtocolEra::Unknown => None,
            ProtocolEra::Modern { version } | ProtocolEra::Legacy { version } => Some(version),
        }
    }
}

/// Era per server origin (spec: clients SHOULD cache the era per origin for
/// HTTP), shared by every client in the process. A cached guess is only an
/// ordering hint: if it turns out wrong the other era is tried and the cache
/// corrected.
fn era_cache() -> &'static Mutex<HashMap<String, ProtocolEra>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ProtocolEra>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_era(key: &Option<String>) -> Option<ProtocolEra> {
    let key = key.as_ref()?;
    era_cache().lock().ok()?.get(key).cloned()
}

fn remember_era(key: &Option<String>, era: &ProtocolEra) {
    if let (Some(key), Ok(mut cache)) = (key, era_cache().lock()) {
        cache.insert(key.clone(), era.clone());
    }
}

/// Forget every cached era (tests, or after a server is known to have been
/// replaced).
pub fn clear_era_cache() {
    if let Ok(mut cache) = era_cache().lock() {
        cache.clear();
    }
}

/// The JSON-RPC `error` object an HTTP error body carried, if any.
fn rpc_error_in(err: &McpError) -> Option<Value> {
    match err {
        McpError::JsonRpc { code, message, data } => {
            Some(json!({ "code": code, "message": message, "data": data }))
        }
        McpError::Transport(TransportError::Http { message, .. }) => serde_json::from_str::<Value>(message)
            .ok()
            .and_then(|body| body.get("error").cloned())
            .filter(|e| e.get("code").is_some()),
        _ => None,
    }
}

fn http_status_of(err: &McpError) -> Option<u16> {
    match err {
        McpError::Transport(TransportError::Http { status, .. }) => Some(*status),
        // A JSON-RPC error in a 2xx body.
        McpError::JsonRpc { .. } => Some(200),
        _ => None,
    }
}

/// MCP Client implementation
///
/// The main entry point for talking to an MCP server. Dual-era (spec
/// 2026-07-28): [`McpClient::initialize`] first tries the stateless modern
/// protocol (`server/discover` carrying per-request `_meta`) and falls back
/// to the legacy `initialize` handshake when the server doesn't speak it;
/// every later call is shaped for the era that won.
#[derive(Debug)]
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    capabilities: Option<ServerCapabilities>,
    initialized: bool,
    era: ProtocolEra,
    client_capabilities: Value,
    client_info: Value,
}

impl McpClient {
    /// Create a new MCP client with the given transport. It advertises the
    /// MCP Apps extension (`io.modelcontextprotocol/ui`) and identifies as
    /// `allternit-mcp-client`.
    pub fn new(transport: Arc<dyn McpTransport>) -> Self {
        Self {
            transport,
            capabilities: None,
            initialized: false,
            era: ProtocolEra::Unknown,
            client_capabilities: serde_json::to_value(ClientCapabilities::with_mcp_apps())
                .unwrap_or_else(|_| json!({})),
            client_info: json!({ "name": "allternit-mcp-client", "version": env!("CARGO_PKG_VERSION") }),
        }
    }

    /// Identify as `name`/`version` (`clientInfo`) instead of the default.
    pub fn with_client_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client_info = json!({ "name": name.into(), "version": version.into() });
        self
    }

    /// Advertise these client capabilities instead of the default (MCP Apps).
    pub fn with_client_capabilities(mut self, capabilities: ClientCapabilities) -> Self {
        self.client_capabilities = serde_json::to_value(capabilities).unwrap_or_else(|_| json!({}));
        self
    }

    /// Connect: negotiate the era and version.
    ///
    /// * HTTP / stdio: a modern `server/discover` probe first (or the legacy
    ///   handshake first when this origin is cached as legacy); a failure
    ///   that identifies a legacy server falls back to `initialize`.
    /// * The deprecated HTTP+SSE transport is legacy by definition.
    ///
    /// Must be called before any other operation.
    pub async fn initialize(&mut self) -> Result<InitializeResult> {
        info!("Initializing MCP connection");
        let key = self.transport.era_cache_key();
        let result = if self.transport.transport_type() == TransportType::Sse {
            self.legacy_initialize(dual::LEGACY_OFFER).await?
        } else if matches!(cached_era(&key), Some(ProtocolEra::Legacy { .. })) {
            match self.legacy_initialize(dual::LEGACY_OFFER).await {
                Ok(r) => r,
                // The cached assumption failed: the server may have moved to
                // the modern era. Re-probe; keep the original error if that
                // fails too.
                Err(e) if matches!(dual::classify_probe_failure(http_status_of(&e), None, false), ProbeVerdict::Legacy(_)) => {
                    match self.modern_connect(mcp_protocol::LATEST).await {
                        Ok(r) => r,
                        Err(_) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        } else {
            self.dual_era_connect().await?
        };
        remember_era(&key, &self.era);
        info!(
            era = ?self.era,
            "MCP connection initialized with server: {} {}",
            result.server_info.name, result.server_info.version
        );
        Ok(result)
    }

    async fn dual_era_connect(&mut self) -> Result<InitializeResult> {
        let mut version = mcp_protocol::LATEST;
        // At most one version retry: the server told us which it speaks.
        for _ in 0..2 {
            let err = match self.modern_connect(version).await {
                Ok(r) => return Ok(r),
                Err(e) => e,
            };
            let timed_out = matches!(err, McpError::Timeout(_));
            let rpc = rpc_error_in(&err);
            let status = if self.transport.transport_type() == TransportType::Stdio {
                None
            } else {
                http_status_of(&err)
            };
            match dual::classify_probe_failure(status, rpc.as_ref(), timed_out) {
                ProbeVerdict::RetryModern(v) if v != version => version = v,
                ProbeVerdict::Legacy(v) => {
                    debug!(offer = v, "MCP server is legacy; falling back to initialize");
                    return self.legacy_initialize(v).await;
                }
                _ => return Err(err),
            }
        }
        Err(McpError::Protocol("no mutually supported MCP protocol version".into()))
    }

    /// Modern connect: `server/discover` with per-request `_meta`.
    async fn modern_connect(&mut self, version: &str) -> Result<InitializeResult> {
        let params = dual::with_modern_meta(None, version, &self.client_capabilities, &self.client_info);
        let request = self.transport.request("server/discover", Some(params));
        let discovered = if self.transport.transport_type() == TransportType::Stdio {
            tokio::time::timeout(STDIO_PROBE_TIMEOUT, request)
                .await
                .map_err(|_| McpError::Timeout(STDIO_PROBE_TIMEOUT))??
        } else {
            request.await?
        };
        dual::check_result_type(&discovered).map_err(McpError::Protocol)?;
        // A real DiscoverResult always lists `supportedVersions`. A legacy
        // server that answers unknown methods with an empty success is not
        // modern: fall back to the handshake.
        let Some(list) = discovered.get("supportedVersions").filter(|l| l.is_array()) else {
            debug!("server/discover answered without supportedVersions; treating server as legacy");
            return self.legacy_initialize(dual::LEGACY_OFFER).await;
        };
        // A server that lists versions but not ours answers with the newest
        // modern one it does list.
        let ours_listed = list.as_array().map_or(false, |a| a.iter().any(|v| v.as_str() == Some(version)));
        let version = match ours_listed {
            false => {
                match dual::pick_modern_version(list) {
                    Some(v) => v.to_string(),
                    None => {
                        let offer = dual::pick_legacy_version(Some(list));
                        return self.legacy_initialize(offer).await;
                    }
                }
            }
            _ => version.to_string(),
        };
        let capabilities: ServerCapabilities = discovered
            .get("capabilities")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let server_info = discovered
            .pointer(&format!("/_meta/{}", mcp_protocol::META_SERVER_INFO.replace('/', "~1")))
            .cloned()
            .and_then(|v| serde_json::from_value::<protocol::Implementation>(v).ok())
            .unwrap_or_else(|| protocol::Implementation { name: "unknown".into(), version: "unknown".into() });
        self.capabilities = Some(capabilities.clone());
        self.initialized = true;
        self.era = ProtocolEra::Modern { version: version.clone() };
        Ok(InitializeResult { protocol_version: version, capabilities, server_info })
    }

    /// Legacy connect: the `initialize` handshake offering `version`.
    async fn legacy_initialize(&mut self, version: &str) -> Result<InitializeResult> {
        let params = InitializeParams {
            protocol_version: version.to_string(),
            capabilities: serde_json::from_value(self.client_capabilities.clone()).unwrap_or_default(),
            client_info: serde_json::from_value(self.client_info.clone()).unwrap_or(protocol::Implementation {
                name: "allternit-mcp-client".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            }),
        };
        let result = self
            .transport
            .request("initialize", Some(serde_json::to_value(params)?))
            .await?;
        // Lenient: servers in the wild omit `serverInfo` or `capabilities`.
        let init_result = InitializeResult {
            protocol_version: result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(version)
                .to_string(),
            capabilities: result
                .get("capabilities")
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default(),
            server_info: result
                .get("serverInfo")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_else(|| protocol::Implementation::new("unknown", "unknown")),
        };
        self.capabilities = Some(init_result.capabilities.clone());
        self.initialized = true;
        self.era = ProtocolEra::Legacy { version: init_result.protocol_version.clone() };
        self.transport.notify("notifications/initialized", None).await?;
        Ok(init_result)
    }

    /// Send one request shaped for the negotiated era and return its result.
    async fn send(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.ensure_initialized()?;
        match &self.era {
            ProtocolEra::Modern { version } => {
                let params = dual::with_modern_meta(params, version, &self.client_capabilities, &self.client_info);
                let result = self.transport.request(method, Some(params)).await.map_err(|e| {
                    // A modern server reports JSON-RPC errors with HTTP 400/404;
                    // surface them as JSON-RPC errors, not transport failures.
                    match (&e, rpc_error_in(&e)) {
                        (McpError::Transport(TransportError::Http { status: 400 | 404, .. }), Some(rpc)) => McpError::JsonRpc {
                            code: rpc.get("code").and_then(Value::as_i64).unwrap_or(-32603) as i32,
                            message: rpc.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
                            data: rpc.get("data").cloned().filter(|d| !d.is_null()),
                        },
                        _ => e,
                    }
                })?;
                dual::check_result_type(&result).map_err(McpError::Protocol)?;
                Ok(result)
            }
            _ => self.transport.request(method, params).await,
        }
    }

    /// List available tools from the MCP server
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        debug!("Listing tools");
        let result = self.send("tools/list", None).await?;
        let list_result: ListToolsResult = serde_json::from_value(result)?;
        Ok(list_result.tools)
    }

    /// Call a tool on the MCP server
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolResult> {
        debug!("Calling tool: {name}");
        let params = json!({ "name": name, "arguments": arguments });
        let result = self.send("tools/call", Some(params)).await?;
        let tool_result: ToolResult = serde_json::from_value(result)?;
        Ok(tool_result)
    }

    /// List available resources from the MCP server
    pub async fn list_resources(&self) -> Result<Vec<Resource>> {
        debug!("Listing resources");
        let result = self.send("resources/list", None).await?;
        let list_result: ListResourcesResult = serde_json::from_value(result)?;
        Ok(list_result.resources)
    }

    /// Read a resource from the MCP server
    pub async fn read_resource(&self, uri: &str) -> Result<ResourceContent> {
        debug!("Reading resource: {uri}");
        let result = self.send("resources/read", Some(json!({ "uri": uri }))).await?;
        let content: ResourceContent = serde_json::from_value(result)?;
        Ok(content)
    }

    /// Send a raw JSON-RPC request and return the untouched `result`.
    ///
    /// Use for methods whose results carry fields the typed helpers drop
    /// (`_meta`, `structuredContent`, resource `contents`). In the modern
    /// era the request still gets the protocol `_meta` and headers.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        self.send(method, params).await
    }

    /// Check if the client is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// The negotiated era and version.
    pub fn era(&self) -> &ProtocolEra {
        &self.era
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
            era: self.era.clone(),
            client_capabilities: self.client_capabilities.clone(),
            client_info: self.client_info.clone(),
        }
    }
}

#[cfg(test)]
pub(crate) fn seed_era_cache_for_tests(key: &str, era: ProtocolEra) {
    remember_era(&Some(key.to_string()), &era);
}
