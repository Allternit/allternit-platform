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
pub mod events_poll;
pub mod health;
pub mod oauth;
pub mod protocol;
pub mod registry;
pub mod server_requests;
pub mod skills;
pub mod transport;

#[cfg(test)]
mod tasks_tests;

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

// Server-to-client request hook (elicitation) and MCP Tasks (SEP-2663)
pub use mcp_protocol::tasks::{
    CreateTaskResult, DetailedTask, ElicitAction, ElicitRequest, ElicitResult, InputRequest, InputRequests,
    InputResponse, InputResponses, Task, TaskStatus, TASKS_EXTENSION_ID,
};
pub use events_poll::PollBatch;
pub use skills::SkillInfo;
pub use server_requests::{DeclineAll, ElicitContext, HandlerError, ServerRequestHandler};

/// What `tools/call` produced: the tool's result, or (only when the client
/// declared the tasks extension, see [`McpClient::with_tasks`]) a task
/// handle to follow with [`McpClient::tasks_get`] / [`McpClient::tasks_update`]
/// / [`McpClient::tasks_cancel`].
#[derive(Debug, Clone)]
pub enum ToolOutcome<T = ToolResult> {
    Complete(T),
    Task(CreateTaskResult),
}

impl<T> ToolOutcome<T> {
    /// The complete result, or [`McpError::TaskCreated`] for a task (for
    /// callers that do not follow tasks).
    pub fn into_complete(self) -> Result<T> {
        match self {
            ToolOutcome::Complete(r) => Ok(r),
            ToolOutcome::Task(t) => Err(McpError::TaskCreated(Box::new(t))),
        }
    }
}

/// Bounds for honoring a task's `pollIntervalMs` in [`McpClient::wait_for_task`].
const MIN_POLL: Duration = Duration::from_millis(10);
const MAX_POLL: Duration = Duration::from_secs(30);
const DEFAULT_POLL: Duration = Duration::from_secs(1);

// Re-export registry types
pub use registry::{
    ConnectionState, McpRegistry, McpServerRecord, McpServerStatus, OAuthTokenRecord,
};

// Re-export health monitoring types
pub use health::{
    CircuitBreakerState, HealthMetrics, HealthMonitorConfig, McpHealthMonitor, ServerHealth,
};

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tracing::{debug, info};

use mcp_protocol::client::{self as dual, ProbeVerdict, ResultKind};
use mcp_protocol::tasks as tasks_ext;

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
    server_requests: Arc<dyn ServerRequestHandler>,
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
            server_requests: Arc::new(DeclineAll) as Arc<dyn ServerRequestHandler>,
        }
    }

    /// Declare the MCP Tasks extension (`io.modelcontextprotocol/tasks`,
    /// SEP-2663) on every request. Servers may then answer `tools/call` with
    /// a task: [`McpClient::call_tool`] returns [`ToolOutcome::Task`], and
    /// [`McpClient::request`] (which cannot return one) fails with
    /// [`McpError::TaskCreated`]. Without this a server MUST NOT return a task.
    pub fn with_tasks(mut self) -> Self {
        if !self.client_capabilities.is_object() {
            self.client_capabilities = json!({});
        }
        let caps = self.client_capabilities.as_object_mut().expect("object ensured above");
        let ext = caps.entry("extensions").or_insert_with(|| json!({}));
        if !ext.is_object() {
            *ext = json!({});
        }
        ext[tasks_ext::TASKS_EXTENSION_ID] = tasks_ext::extension_entry();
        self
    }

    /// Install the hook that answers server-to-client requests: legacy
    /// `elicitation/create` on the transport's streams, and the
    /// `inputRequests` of tasks ([`McpClient::answer_task_inputs`]). Without
    /// one, every elicitation is declined ([`DeclineAll`]). A handler whose
    /// [`ServerRequestHandler::supports_elicitation`] is true makes the
    /// client advertise `elicitation` (form + url).
    pub fn with_server_request_handler(mut self, handler: Arc<dyn ServerRequestHandler>) -> Self {
        if handler.supports_elicitation() {
            if !self.client_capabilities.is_object() {
                self.client_capabilities = json!({});
            }
            self.client_capabilities["elicitation"] = json!({ "form": {}, "url": {} });
        }
        self.transport.set_server_request_handler(handler.clone());
        self.server_requests = handler;
        self
    }

    /// Whether requests declare the tasks extension.
    pub fn tasks_declared(&self) -> bool {
        tasks_ext::client_declares_tasks(&self.client_capabilities)
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

    /// Send one request shaped for the negotiated era and return its result
    /// with its shape. `task` is only accepted for `tools/call` when the
    /// tasks extension is declared ([`dual::check_result_type_for`]).
    async fn send_checked(&self, method: &str, params: Option<Value>) -> Result<(Value, ResultKind)> {
        self.ensure_initialized()?;
        let tasks = self.tasks_declared();
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
                let kind = dual::check_result_type_for(method, &result, tasks).map_err(McpError::Protocol)?;
                Ok((result, kind))
            }
            _ => {
                let result = self.transport.request(method, params).await?;
                // Legacy results are taken as-is, except a task we asked for.
                let kind = match result.get("resultType").and_then(Value::as_str) {
                    Some(tasks_ext::RESULT_TYPE_TASK) => {
                        dual::check_result_type_for(method, &result, tasks).map_err(McpError::Protocol)?
                    }
                    _ => ResultKind::Complete,
                };
                Ok((result, kind))
            }
        }
    }

    /// [`Self::send_checked`] for callers that take only complete results.
    async fn send(&self, method: &str, params: Option<Value>) -> Result<Value> {
        match self.send_checked(method, params).await? {
            (result, ResultKind::Complete) => Ok(result),
            (result, ResultKind::Task) => Err(McpError::TaskCreated(Box::new(parse_task(result)?))),
        }
    }

    /// List available tools from the MCP server
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        debug!("Listing tools");
        let result = self.send("tools/list", None).await?;
        let list_result: ListToolsResult = serde_json::from_value(result)?;
        Ok(list_result.tools)
    }

    /// Call a tool on the MCP server. A task comes back only when the
    /// client declared the tasks extension ([`McpClient::with_tasks`]).
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolOutcome> {
        debug!("Calling tool: {name}");
        let params = json!({ "name": name, "arguments": arguments });
        match self.call_tool_raw(params).await? {
            ToolOutcome::Complete(result) => Ok(ToolOutcome::Complete(serde_json::from_value(result)?)),
            ToolOutcome::Task(task) => Ok(ToolOutcome::Task(task)),
        }
    }

    /// `tools/call` with caller-built `params` (`name`, `arguments`, any
    /// `_meta`), returning the untouched `CallToolResult` JSON (so
    /// `structuredContent` and `_meta` survive) or the task handle.
    pub async fn call_tool_raw(&self, params: Value) -> Result<ToolOutcome<Value>> {
        match self.send_checked("tools/call", Some(params)).await? {
            (result, ResultKind::Complete) => Ok(ToolOutcome::Complete(result)),
            (result, ResultKind::Task) => Ok(ToolOutcome::Task(parse_task(result)?)),
        }
    }

    /// `tasks/get`: the task's current state, with `inputRequests` when
    /// `input_required`, `result` when `completed`, `error` when `failed`.
    pub async fn tasks_get(&self, task_id: &str) -> Result<DetailedTask> {
        let result = self.send(tasks_ext::METHOD_TASKS_GET, Some(json!({ "taskId": task_id }))).await?;
        Ok(serde_json::from_value(result)?)
    }

    /// `tasks/update`: answer outstanding `inputRequests` (keys as given).
    /// The ack is eventually consistent; keep polling [`Self::tasks_get`].
    pub async fn tasks_update(&self, task_id: &str, input_responses: InputResponses) -> Result<()> {
        let params = json!({ "taskId": task_id, "inputResponses": input_responses });
        self.send(tasks_ext::METHOD_TASKS_UPDATE, Some(params)).await.map(|_| ())
    }

    /// `tasks/cancel`: cooperative and ack-only; the task may still finish.
    pub async fn tasks_cancel(&self, task_id: &str) -> Result<()> {
        self.send(tasks_ext::METHOD_TASKS_CANCEL, Some(json!({ "taskId": task_id }))).await.map(|_| ())
    }

    /// `tasks/result`, which exists only in the 2025-11-25 core-spec tasks
    /// (SEP-2663 removed it: a completed task carries `result` inline in
    /// [`Self::tasks_get`]). Refused unless that legacy version was negotiated.
    pub async fn tasks_result(&self, task_id: &str) -> Result<Value> {
        match &self.era {
            ProtocolEra::Legacy { version } if tasks_ext::legacy_has_tasks_result(version) => {
                self.send(tasks_ext::METHOD_TASKS_RESULT_LEGACY, Some(json!({ "taskId": task_id }))).await
            }
            era => Err(McpError::Protocol(format!(
                "tasks/result does not exist in protocol {:?}; read `result` from tasks/get",
                era.version().unwrap_or("unknown")
            ))),
        }
    }

    /// Answer a task's outstanding `inputRequests` through the installed
    /// [`ServerRequestHandler`] and send them with `tasks/update`. Keys in
    /// `answered` are skipped (polls repeat them) and new ones are added.
    /// Returns whether anything was sent.
    pub async fn answer_task_inputs(&self, task: &DetailedTask, answered: &mut HashSet<String>) -> Result<bool> {
        let Some(requests) = task.input_requests.as_ref().filter(|_| task.task.status == TaskStatus::InputRequired) else {
            return Ok(false);
        };
        let responses =
            server_requests::answer_input_requests(self.server_requests.as_ref(), &task.task.task_id, requests, answered).await;
        if responses.is_empty() {
            return Ok(false);
        }
        self.tasks_update(&task.task.task_id, responses).await?;
        Ok(true)
    }

    /// Poll a task until it is terminal, answering `input_required` through
    /// the installed handler and honoring `pollIntervalMs`. Gives up with
    /// [`McpError::Timeout`] after `max_wait` (the task keeps running at the
    /// server; cancel it if it should stop).
    pub async fn wait_for_task(&self, task_id: &str, max_wait: Duration) -> Result<DetailedTask> {
        let deadline = tokio::time::Instant::now() + max_wait;
        let mut answered = HashSet::new();
        loop {
            let task = self.tasks_get(task_id).await?;
            if task.task.status.is_terminal() {
                return Ok(task);
            }
            self.answer_task_inputs(&task, &mut answered).await?;
            let interval = task
                .task
                .poll_interval_ms
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_POLL)
                .clamp(MIN_POLL, MAX_POLL);
            if tokio::time::Instant::now() + interval > deadline {
                return Err(McpError::Timeout(max_wait));
            }
            tokio::time::sleep(interval).await;
        }
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

    /// Whether the connected server advertises MCP Events.
    pub fn supports_events(&self) -> bool {
        self.capabilities.as_ref().is_some_and(ServerCapabilities::supports_events)
    }

    /// `events/list`, following pagination (at most `max_pages`). Returns the
    /// raw event definitions (`name`, `description`, `delivery`,
    /// `inputSchema`, `payloadSchema`).
    pub async fn list_events(&self, max_pages: usize) -> Result<Vec<Value>> {
        let mut events = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..max_pages.max(1) {
            let page = self.send("events/list", cursor.as_ref().map(|c| json!({ "cursor": c }))).await?;
            if let Some(items) = page.get("events").and_then(Value::as_array) {
                events.extend(items.iter().cloned());
            }
            cursor = page.get("nextCursor").and_then(Value::as_str).map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        Ok(events)
    }

    /// `events/subscribe` in webhook mode. `ttl_ms` `None` = server default.
    /// Returns the raw result (`id`, `refreshBefore`, `cursor`).
    pub async fn subscribe_event(
        &self,
        name: &str,
        arguments: &Value,
        url: &str,
        secret: &str,
        ttl_ms: Option<u64>,
    ) -> Result<Value> {
        self.subscribe_event_with_cursor(name, arguments, url, secret, ttl_ms, None).await
    }

    /// Resume an event stream after the last durably accepted cursor.
    pub async fn subscribe_event_with_cursor(&self, name: &str, arguments: &Value, url: &str, secret: &str, ttl_ms: Option<u64>, cursor: Option<&str>) -> Result<Value> {
        let mut params = json!({
            "name": name,
            "arguments": arguments,
            "delivery": { "mode": "webhook", "url": url, "secret": secret },
        });
        if let Some(ttl) = ttl_ms {
            params["ttlMs"] = json!(ttl);
        }
        if let Some(cursor) = cursor { params["cursor"] = json!(cursor); }
        self.send("events/subscribe", Some(params)).await
    }

    /// `events/unsubscribe` for the webhook identity `(name, arguments, url)`.
    pub async fn unsubscribe_event(&self, name: &str, arguments: &Value, url: &str) -> Result<Value> {
        let params = json!({ "name": name, "arguments": arguments, "delivery": { "mode": "webhook", "url": url } });
        self.send("events/unsubscribe", Some(params)).await
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
            server_requests: Arc::clone(&self.server_requests),
        }
    }
}

/// Decode a `resultType: "task"` result.
fn parse_task(result: Value) -> Result<CreateTaskResult> {
    serde_json::from_value(result).map_err(|e| McpError::Protocol(format!("invalid CreateTaskResult: {e}")))
}

#[cfg(test)]
pub(crate) fn seed_era_cache_for_tests(key: &str, era: ProtocolEra) {
    remember_era(&Some(key.to_string()), &era);
}
