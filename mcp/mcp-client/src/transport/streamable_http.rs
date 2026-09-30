//! Streamable HTTP transport (MCP spec 2025-06-18) for the MCP client
//!
//! A single MCP endpoint accepts every client message as an HTTP POST:
//! - `Accept: application/json, text/event-stream` on every POST
//! - the response is either one `application/json` JSON-RPC message or a
//!   `text/event-stream` carrying the response (plus any server messages)
//! - the server may assign a session in the `Mcp-Session-Id` response header
//!   of the `initialize` result; the client echoes it on every later request
//! - after `initialize` the client sends `MCP-Protocol-Version` with the
//!   negotiated version on every request
//! - notifications are POSTed and acknowledged with `202 Accepted`
//! - `DELETE` on the endpoint with the session id ends the session
//!
//! Not implemented: the optional standalone `GET` SSE stream for
//! server-initiated messages, and resumability (`Last-Event-ID`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, trace, warn};

use crate::error::{McpError, Result, TransportError};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION};
use crate::transport::{McpTransport, TransportType};

/// Header carrying the server-assigned session id
pub const SESSION_ID_HEADER: &str = "mcp-session-id";
/// Header carrying the negotiated protocol version
pub const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

/// Longest error body echoed back into an error message
const MAX_ERROR_BODY: usize = 512;

/// Configuration for the streamable HTTP transport
#[derive(Clone)]
pub struct StreamableHttpConfig {
    /// Full MCP endpoint URL (e.g. `https://example.com/mcp`)
    pub url: String,
    /// Bearer token sent as `Authorization: Bearer <token>` (never logged)
    pub auth_token: Option<String>,
    /// Extra request headers (may carry credentials; never logged)
    pub headers: HashMap<String, String>,
    /// Per-request timeout in seconds
    pub timeout_secs: u64,
    /// Connect `host` to exactly this address instead of resolving it again
    /// (closes the DNS-rebinding gap after a caller validated the address).
    pub pin: Option<(String, std::net::SocketAddr)>,
}

impl std::fmt::Debug for StreamableHttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamableHttpConfig")
            .field("url", &self.url)
            .field("auth_token", &self.auth_token.as_ref().map(|_| "<redacted>"))
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl StreamableHttpConfig {
    /// Config with a 60 second timeout and no credentials
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: None,
            headers: HashMap::new(),
            timeout_secs: 60,
            pin: None,
        }
    }
}

/// Streamable HTTP transport
pub struct StreamableHttpTransport {
    client: Client,
    url: String,
    auth_token: RwLock<Option<String>>,
    headers: HashMap<String, String>,
    session_id: RwLock<Option<String>>,
    protocol_version: RwLock<Option<String>>,
    request_counter: AtomicU64,
    closed: AtomicBool,
}

impl std::fmt::Debug for StreamableHttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamableHttpTransport")
            .field("url", &self.url)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish()
    }
}

impl StreamableHttpTransport {
    /// Create a new transport. No network I/O happens until the first request.
    pub fn new(config: StreamableHttpConfig) -> Result<Arc<Self>> {
        let mut builder = Client::builder().timeout(Duration::from_secs(config.timeout_secs));
        if let Some((host, addr)) = &config.pin {
            builder = builder.resolve(host, *addr);
        }
        let client = builder
            .build()
            .map_err(|e| TransportError::Http {
                status: 0,
                message: format!("Failed to create HTTP client: {e}"),
            })?;

        Ok(Arc::new(Self {
            client,
            url: config.url,
            auth_token: RwLock::new(config.auth_token),
            headers: config.headers,
            session_id: RwLock::new(None),
            protocol_version: RwLock::new(None),
            request_counter: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        }))
    }

    /// Replace the bearer token (e.g. after an OAuth refresh)
    pub async fn set_auth_token(&self, token: Option<String>) {
        *self.auth_token.write().await = token;
    }

    /// The session id the server assigned, if any
    pub async fn session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// The protocol version negotiated by `initialize`, if it has run
    pub async fn protocol_version(&self) -> Option<String> {
        self.protocol_version.read().await.clone()
    }

    async fn build_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        for (name, value) in &self.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| TransportError::ConnectionFailed(format!("invalid header name: {name}")))?;
            let mut value = HeaderValue::from_str(value).map_err(|_| {
                TransportError::ConnectionFailed(format!("invalid value for header {name}"))
            })?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }

        if let Some(token) = self.auth_token.read().await.as_deref() {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| TransportError::ConnectionFailed("invalid auth token".to_string()))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }

        if let Some(session) = self.session_id.read().await.as_deref() {
            let value = HeaderValue::from_str(session)
                .map_err(|_| TransportError::ConnectionFailed("invalid session id".to_string()))?;
            headers.insert(HeaderName::from_static(SESSION_ID_HEADER), value);
        }
        if let Some(version) = self.protocol_version.read().await.as_deref() {
            let value = HeaderValue::from_str(version).map_err(|_| {
                TransportError::ConnectionFailed("invalid protocol version".to_string())
            })?;
            headers.insert(HeaderName::from_static(PROTOCOL_VERSION_HEADER), value);
        }

        Ok(headers)
    }

    async fn post(&self, body: &JsonRpcRequest) -> Result<reqwest::Response> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed);
        }

        let headers = self.build_headers().await?;
        let response = self
            .client
            .post(&self.url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    McpError::Timeout(Duration::ZERO)
                } else {
                    McpError::Transport(TransportError::ConnectionFailed(e.without_url().to_string()))
                }
            })?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        // Spec: a 404 for a request carrying a session id means the session
        // ended; the client must start a new one by re-initializing.
        if status == StatusCode::NOT_FOUND && self.session_id.read().await.is_some() {
            *self.session_id.write().await = None;
            *self.protocol_version.write().await = None;
            return Err(TransportError::Http {
                status: status.as_u16(),
                message: "MCP session expired; re-initialize".to_string(),
            }
            .into());
        }

        let text = response.text().await.unwrap_or_default();
        Err(TransportError::Http {
            status: status.as_u16(),
            message: truncate(&text, MAX_ERROR_BODY),
        }
        .into())
    }

    fn next_id(&self) -> u64 {
        self.request_counter.fetch_add(1, Ordering::SeqCst)
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id();
        let is_initialize = method == "initialize";
        let body = JsonRpcRequest::new(id, method, params);
        debug!(method, id, "streamable-http request");

        let response = self.post(&body).await?;

        if is_initialize {
            if let Some(session) = response
                .headers()
                .get(SESSION_ID_HEADER)
                .and_then(|v| v.to_str().ok())
            {
                *self.session_id.write().await = Some(session.to_string());
            }
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        let message = if content_type.starts_with("text/event-stream") {
            read_sse_response(response, id).await?
        } else {
            let bytes = response.bytes().await.map_err(|e| {
                McpError::Transport(TransportError::ConnectionFailed(e.without_url().to_string()))
            })?;
            let value: Value = serde_json::from_slice(&bytes)?;
            pick_response(value, id).ok_or_else(|| {
                McpError::Protocol(format!("no JSON-RPC response for request {id}"))
            })?
        };

        let result = into_result(message)?;

        if is_initialize {
            let negotiated = result
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or(MCP_PROTOCOL_VERSION);
            *self.protocol_version.write().await = Some(negotiated.to_string());
        }

        Ok(result)
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let body = JsonRpcRequest::notification(method, params);
        trace!(method, "streamable-http notification");
        let response = self.post(&body).await?;
        // 202 Accepted with no body is the specified answer; drain anything else.
        let _ = response.bytes().await;
        Ok(())
    }

    async fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let session = self.session_id.write().await.take();
        if let Some(session) = session {
            let mut headers = HeaderMap::new();
            if let Ok(value) = HeaderValue::from_str(&session) {
                headers.insert(HeaderName::from_static(SESSION_ID_HEADER), value);
            }
            if let Some(version) = self.protocol_version.read().await.as_deref() {
                if let Ok(value) = HeaderValue::from_str(version) {
                    headers.insert(HeaderName::from_static(PROTOCOL_VERSION_HEADER), value);
                }
            }
            if let Some(token) = self.auth_token.read().await.as_deref() {
                if let Ok(mut value) = HeaderValue::from_str(&format!("Bearer {token}")) {
                    value.set_sensitive(true);
                    headers.insert(AUTHORIZATION, value);
                }
            }
            // A 405 means the server does not allow client-initiated
            // termination; that is not an error.
            if let Err(e) = self.client.delete(&self.url).headers(headers).send().await {
                warn!("failed to terminate MCP session: {}", e.without_url());
            }
        }
        Ok(())
    }

    fn transport_type(&self) -> TransportType {
        TransportType::StreamableHttp
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Select the response for request `id` from a JSON body (a single message,
/// or a legacy batch array).
fn pick_response(value: Value, id: u64) -> Option<JsonRpcResponse> {
    match value {
        Value::Array(items) => items.into_iter().find_map(|item| pick_response(item, id)),
        Value::Object(_) => serde_json::from_value::<JsonRpcResponse>(value)
            .ok()
            .filter(|r| r.id == id),
        _ => None,
    }
}

fn into_result(message: JsonRpcResponse) -> Result<Value> {
    if let Some(error) = message.error {
        return Err(McpError::JsonRpc {
            code: error.code,
            message: error.message,
            data: error.data,
        });
    }
    message
        .result
        .ok_or_else(|| McpError::Protocol("JSON-RPC response has neither result nor error".into()))
}

async fn read_sse_response(response: reqwest::Response, id: u64) -> Result<JsonRpcResponse> {
    let mut parser = SseParser::default();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            McpError::Transport(TransportError::Sse(e.without_url().to_string()))
        })?;
        for event in parser.push(&String::from_utf8_lossy(&chunk)) {
            if let Some(found) = event_response(&event, id) {
                return Ok(found);
            }
        }
    }
    if let Some(event) = parser.finish() {
        if let Some(found) = event_response(&event, id) {
            return Ok(found);
        }
    }
    Err(McpError::Protocol(format!(
        "event stream ended without a response for request {id}"
    )))
}

fn event_response(event: &SseEvent, id: u64) -> Option<JsonRpcResponse> {
    if event.data.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(&event.data) {
        // Anything that is not our response (server notifications and
        // requests share the stream) is skipped.
        Ok(value) => pick_response(value, id),
        Err(e) => {
            trace!("skipping non-JSON SSE event: {e}");
            None
        }
    }
}

/// One dispatched server-sent event
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    /// `event:` field, if any
    pub event: Option<String>,
    /// `data:` lines joined with `\n`
    pub data: String,
}

/// Incremental parser for the `text/event-stream` wire format
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    /// Feed more text; returns every event completed by it
    pub fn push(&mut self, text: &str) -> Vec<SseEvent> {
        self.buffer.push_str(text);
        let mut events = Vec::new();

        while let Some(pos) = self.buffer.find(['\n', '\r']) {
            // A lone trailing "\r" may be the first half of "\r\n"; wait for more.
            if self.buffer.as_bytes()[pos] == b'\r' && pos + 1 == self.buffer.len() {
                break;
            }
            let mut line: String = self.buffer.drain(..=pos).collect();
            if line.ends_with('\r') && self.buffer.starts_with('\n') {
                self.buffer.remove(0);
            }
            line.truncate(line.trim_end_matches(['\r', '\n']).len());

            if let Some(event) = self.process_line(&line) {
                events.push(event);
            }
        }
        events
    }

    /// Flush a final event the stream closed without terminating
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.process_line(line.trim_end_matches(['\r', '\n'])) {
                return Some(event);
            }
        }
        self.dispatch()
    }

    fn process_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        Some(SseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{HeaderMap as AxumHeaders, StatusCode as AxumStatus};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use axum::Router;
    use serde_json::json;
    use std::sync::Mutex;

    // ── SSE parser ───────────────────────────────────────────────────────────

    #[test]
    fn sse_parser_splits_events_and_joins_data_lines() {
        let mut p = SseParser::default();
        let events = p.push("event: message\ndata: {\"a\":\ndata: 1}\n\n: comment\ndata: x\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data, "{\"a\":\n1}");
        assert_eq!(events[1].data, "x");
    }

    #[test]
    fn sse_parser_handles_crlf_and_chunk_boundaries() {
        let mut p = SseParser::default();
        let mut events = Vec::new();
        for chunk in ["data: he", "llo\r", "\n\r", "\ndata: two\r\n\r\n"] {
            events.extend(p.push(chunk));
        }
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "hello");
        assert_eq!(events[1].data, "two");
    }

    #[test]
    fn sse_parser_finish_flushes_unterminated_event() {
        let mut p = SseParser::default();
        assert!(p.push("data: tail").is_empty());
        assert_eq!(p.finish().unwrap().data, "tail");
    }

    // ── transport against a local streamable HTTP server ─────────────────────

    #[derive(Default)]
    struct Seen {
        /// (method, mcp-session-id, mcp-protocol-version, authorization, accept)
        requests: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>)>,
        deleted: bool,
    }

    #[derive(Clone)]
    struct Server {
        seen: Arc<Mutex<Seen>>,
        sse: bool,
    }

    fn header(h: &AxumHeaders, name: &str) -> Option<String> {
        h.get(name).and_then(|v| v.to_str().ok()).map(String::from)
    }

    async fn handle(
        State(server): State<Server>,
        headers: AxumHeaders,
        body: axum::Json<Value>,
    ) -> Response {
        let method = body["method"].as_str().unwrap_or("").to_string();
        server.seen.lock().unwrap().requests.push((
            method.clone(),
            header(&headers, "mcp-session-id"),
            header(&headers, "mcp-protocol-version"),
            header(&headers, "authorization"),
            header(&headers, "accept"),
        ));

        if body.get("id").is_none() {
            return AxumStatus::ACCEPTED.into_response();
        }
        let id = body["id"].clone();
        let result = match method.as_str() {
            "initialize" => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
                "serverInfo": { "name": "test", "version": "0" }
            }),
            "tools/list" => json!({ "tools": [{
                "name": "echo",
                "inputSchema": { "type": "object" },
                "_meta": { "ui": { "resourceUri": "ui://echo/app" } }
            }]}),
            _ => {
                let payload = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "nope" } });
                return (AxumStatus::OK, axum::Json(payload)).into_response();
            }
        };
        let payload = json!({ "jsonrpc": "2.0", "id": id, "result": result });

        let mut builder = Response::builder().status(AxumStatus::OK);
        if method == "initialize" {
            builder = builder.header("mcp-session-id", "sess-1");
        }
        if server.sse {
            // A server notification first, then the response, split mid-event.
            let note = json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} });
            let text = format!("data: {note}\n\nevent: message\ndata: {payload}\n\n");
            builder
                .header("content-type", "text/event-stream")
                .body(Body::from(text))
                .unwrap()
        } else {
            builder
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap()
        }
    }

    async fn spawn(sse: bool) -> (String, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let server = Server { seen: seen.clone(), sse };
        let del_seen = seen.clone();
        let app = Router::new()
            .route(
                "/mcp",
                post(handle).delete(move || {
                    let del_seen = del_seen.clone();
                    async move {
                        del_seen.lock().unwrap().deleted = true;
                        AxumStatus::OK
                    }
                }),
            )
            .with_state(server);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/mcp"), seen)
    }

    async fn exercise(sse: bool) {
        let (url, seen) = spawn(sse).await;
        let mut config = StreamableHttpConfig::new(url);
        config.auth_token = Some("tok-123".to_string());
        let transport = StreamableHttpTransport::new(config).unwrap();
        assert_eq!(transport.transport_type(), TransportType::StreamableHttp);

        let init = transport
            .request("initialize", Some(json!({ "protocolVersion": "2025-06-18" })))
            .await
            .unwrap();
        assert_eq!(init["serverInfo"]["name"], "test");
        assert_eq!(transport.session_id().await.as_deref(), Some("sess-1"));
        assert_eq!(transport.protocol_version().await.as_deref(), Some("2025-06-18"));

        transport.notify("notifications/initialized", None).await.unwrap();
        let tools = transport.request("tools/list", None).await.unwrap();
        assert_eq!(tools["tools"][0]["_meta"]["ui"]["resourceUri"], "ui://echo/app");

        let err = transport.request("nope/nope", None).await.unwrap_err();
        assert!(matches!(err, McpError::JsonRpc { code: -32601, .. }), "{err:?}");

        transport.close().await.unwrap();
        assert!(!transport.is_healthy().await);
        assert!(matches!(
            transport.request("tools/list", None).await,
            Err(McpError::ConnectionClosed)
        ));

        let seen = seen.lock().unwrap();
        // initialize carries neither session nor version; later calls carry both.
        let init = &seen.requests[0];
        assert_eq!((init.0.as_str(), &init.1, &init.2), ("initialize", &None, &None));
        for r in &seen.requests[1..] {
            assert_eq!(r.1.as_deref(), Some("sess-1"), "{r:?}");
            assert_eq!(r.2.as_deref(), Some("2025-06-18"), "{r:?}");
        }
        for r in seen.requests.iter() {
            assert_eq!(r.3.as_deref(), Some("Bearer tok-123"));
            assert_eq!(r.4.as_deref(), Some("application/json, text/event-stream"));
        }
        assert!(seen.requests.iter().any(|r| r.0 == "notifications/initialized"));
        assert!(seen.deleted, "close() should DELETE the session");
    }

    #[tokio::test]
    async fn client_initializes_when_servers_declare_empty_capability_objects() {
        for sse in [false, true] {
            let (url, _seen) = spawn(sse).await;
            let transport = StreamableHttpTransport::new(StreamableHttpConfig::new(url)).unwrap();
            let mut client = crate::McpClient::new(transport);
            let init = client.initialize().await.unwrap();
            assert_eq!(init.server_info.name, "test");
            assert!(client.capabilities().unwrap().resources.is_some());
            let raw = client.request("tools/list", None).await.unwrap();
            assert_eq!(raw["tools"][0]["_meta"]["ui"]["resourceUri"], "ui://echo/app");
            client.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn json_responses_session_and_version_headers() {
        exercise(false).await;
    }

    #[tokio::test]
    async fn sse_responses_skip_server_notifications() {
        exercise(true).await;
    }

    #[tokio::test]
    async fn http_error_status_is_reported_without_credentials() {
        let app = Router::new().route(
            "/mcp",
            post(|| async { (AxumStatus::UNAUTHORIZED, "bad token") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut config = StreamableHttpConfig::new(format!("http://{addr}/mcp"));
        config.auth_token = Some("secret-value".to_string());
        assert!(!format!("{config:?}").contains("secret-value"));
        let transport = StreamableHttpTransport::new(config).unwrap();
        let err = transport.request("initialize", None).await.unwrap_err();
        match err {
            McpError::Transport(TransportError::Http { status, message }) => {
                assert_eq!(status, 401);
                assert_eq!(message, "bad token");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pinned_address_is_used_instead_of_resolving_the_host_again() {
        let (url, seen) = spawn(false).await;
        let port: u16 = url.trim_end_matches("/mcp").rsplit(':').next().unwrap().parse().unwrap();
        // `.invalid` never resolves: the request can only succeed by going to the pinned address.
        let pinned_url = format!("http://pinned.invalid:{port}/mcp");

        let mut unpinned = StreamableHttpConfig::new(pinned_url.clone());
        unpinned.timeout_secs = 5;
        let transport = StreamableHttpTransport::new(unpinned).unwrap();
        assert!(transport.request("initialize", Some(json!({}))).await.is_err());
        assert!(seen.lock().unwrap().requests.is_empty());

        let mut config = StreamableHttpConfig::new(pinned_url);
        config.timeout_secs = 5;
        config.pin = Some(("pinned.invalid".into(), std::net::SocketAddr::from(([127, 0, 0, 1], port))));
        let transport = StreamableHttpTransport::new(config).unwrap();
        transport.request("initialize", Some(json!({ "protocolVersion": "2025-06-18" }))).await.unwrap();
        assert_eq!(seen.lock().unwrap().requests.len(), 1);
    }
}
