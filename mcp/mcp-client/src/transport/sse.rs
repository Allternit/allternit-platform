//! SSE (Server-Sent Events) transport implementation for MCP client
//!
//! This transport communicates with MCP servers over HTTP:
//! - HTTP POST for JSON-RPC requests
//! - SSE stream for server→client messages (notifications)
//!
//! Features:
//! - Automatic reconnection with exponential backoff
//! - OAuth Bearer token injection
//! - Connection health monitoring

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

use crate::error::{McpError, Result, TransportError};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::transport::{McpTransport, TransportType};

/// SSE transport for MCP communication
///
/// Uses HTTP POST for requests and SSE stream for server messages.
#[derive(Debug)]
pub struct SseTransport {
    /// HTTP client
    client: Client,
    /// Base URL for the MCP server
    base_url: String,
    /// SSE endpoint path
    sse_path: String,
    /// POST endpoint path
    post_path: String,
    /// Authentication token
    auth_token: Arc<RwLock<Option<String>>>,
    /// Request timeout
    timeout: Duration,
    /// Request ID counter
    request_counter: AtomicU64,
    /// Whether the transport is closed
    closed: AtomicBool,
    /// Reconnection configuration
    reconnect_config: ReconnectConfig,
    /// Current reconnection attempts
    reconnect_attempts: AtomicU64,
}

/// Configuration for SSE transport
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SseConfig {
    /// Server base URL
    pub url: String,
    /// SSE endpoint path (default: "/sse")
    pub sse_path: Option<String>,
    /// POST endpoint path (default: "/message")
    pub post_path: Option<String>,
    /// Authentication token (optional)
    pub auth_token: Option<String>,
    /// Request timeout in seconds (default: 60)
    pub timeout_secs: u64,
    /// Reconnection configuration
    pub reconnect: ReconnectConfig,
    /// Connect `host` to exactly this address instead of resolving it again
    /// (closes the DNS-rebinding gap after a caller validated the address).
    #[serde(skip)]
    pub pin: Option<(String, std::net::SocketAddr)>,
}

/// Reconnection configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReconnectConfig {
    /// Enable automatic reconnection
    pub enabled: bool,
    /// Base delay for exponential backoff
    pub base_delay: Duration,
    /// Maximum delay between reconnection attempts
    pub max_delay: Duration,
    /// Maximum number of reconnection attempts (0 = unlimited)
    pub max_attempts: u32,
    /// Jitter factor (0.0 - 1.0)
    pub jitter: f64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            max_attempts: 10,
            jitter: 0.1,
        }
    }
}

impl SseTransport {
    /// Create a new SSE transport
    ///
    /// # Arguments
    /// * `config` - Configuration for the SSE transport
    pub fn new(config: SseConfig) -> Result<Arc<Self>> {
        info!("Creating SSE transport for: {}", config.url);

        let mut builder = Client::builder().timeout(Duration::from_secs(config.timeout_secs));
        if let Some((host, addr)) = &config.pin {
            builder = builder.resolve(host, *addr);
        }
        // No redirects: the host SSRF-checks and pins the connector address.
        let client = builder
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| TransportError::Http {
                status: 0,
                message: format!("Failed to create HTTP client: {e}"),
            })?;

        let transport = Arc::new(Self {
            client,
            base_url: config.url.trim_end_matches('/').to_string(),
            sse_path: config.sse_path.unwrap_or_else(|| "/sse".to_string()),
            post_path: config.post_path.unwrap_or_else(|| "/message".to_string()),
            auth_token: Arc::new(RwLock::new(config.auth_token)),
            timeout: Duration::from_secs(config.timeout_secs),
            request_counter: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            reconnect_config: config.reconnect,
            reconnect_attempts: AtomicU64::new(0),
        });

        // Start the SSE listener task
        let transport_clone = transport.clone();
        tokio::spawn(async move {
            transport_clone.sse_listener_loop().await;
        });

        info!("SSE transport created successfully");
        Ok(transport)
    }

    /// Create a new SSE transport with OAuth token
    pub fn with_auth(url: impl Into<String>, token: String) -> Result<Arc<Self>> {
        Self::new(SseConfig {
            url: url.into(),
            sse_path: None,
            post_path: None,
            auth_token: Some(token),
            timeout_secs: 60,
            reconnect: ReconnectConfig::default(),
            pin: None,
        })
    }

    /// Set or update the authentication token
    pub async fn set_auth_token(&self, token: Option<String>) {
        let mut auth = self.auth_token.write().await;
        *auth = token;
    }

    /// Get the full URL for a path
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Build request headers with optional auth
    async fn build_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        headers.insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());

        // Add Bearer token if available
        if let Some(token) = self.auth_token.read().await.as_ref() {
            if let Ok(auth_value) = format!("Bearer {token}").parse() {
                headers.insert(reqwest::header::AUTHORIZATION, auth_value);
            }
        }

        headers
    }

    /// Calculate backoff delay with jitter
    fn backoff_delay(&self, attempt: u64) -> Duration {
        if !self.reconnect_config.enabled {
            return Duration::MAX;
        }

        let exponent = attempt.min(10) as u32;
        let delay_secs = self
            .reconnect_config
            .base_delay
            .as_secs()
            .saturating_mul(2_u64.saturating_pow(exponent));
        let delay = Duration::from_secs(delay_secs.min(self.reconnect_config.max_delay.as_secs()));

        // Add jitter
        if self.reconnect_config.jitter > 0.0 {
            let jitter_factor =
                1.0 + (rand::random::<f64>() - 0.5) * 2.0 * self.reconnect_config.jitter;
            let jittered_millis = (delay.as_millis() as f64 * jitter_factor) as u64;
            Duration::from_millis(jittered_millis)
        } else {
            delay
        }
    }

    /// Check if we should attempt reconnection
    fn should_reconnect(&self) -> bool {
        if !self.reconnect_config.enabled {
            return false;
        }

        if self.closed.load(Ordering::SeqCst) {
            return false;
        }

        let attempts = self.reconnect_attempts.load(Ordering::SeqCst);
        if self.reconnect_config.max_attempts > 0 {
            attempts < self.reconnect_config.max_attempts as u64
        } else {
            true
        }
    }

    /// SSE listener loop with automatic reconnection
    async fn sse_listener_loop(&self) {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                debug!("SSE listener loop ending (transport closed)");
                break;
            }

            match self.connect_sse().await {
                Ok(()) => {
                    // Reset reconnect attempts on successful connection
                    self.reconnect_attempts.store(0, Ordering::SeqCst);
                }
                Err(e) => {
                    warn!("SSE connection error: {e}");

                    if !self.should_reconnect() {
                        error!("Max reconnection attempts reached or reconnection disabled");
                        break;
                    }

                    let attempt = self.reconnect_attempts.fetch_add(1, Ordering::SeqCst);
                    let delay = self.backoff_delay(attempt);
                    warn!("Reconnecting in {:?} (attempt {})", delay, attempt + 1);
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// Connect to SSE stream and process events
    async fn connect_sse(&self) -> Result<()> {
        let url = self.url(&self.sse_path);
        debug!("Connecting to SSE stream: {url}");

        let headers = self.build_headers().await;
        let response = self
            .client
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| TransportError::Http {
                status: 0,
                message: format!("SSE connection failed: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(TransportError::Http {
                status: status.as_u16(),
                message: format!("SSE connection failed with status: {status}"),
            }
            .into());
        }

        info!("SSE connection established");

        // Process SSE events
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            if self.closed.load(Ordering::SeqCst) {
                break;
            }

            match chunk {
                Ok(bytes) => {
                    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                        self.process_sse_event(&text).await;
                    }
                }
                Err(e) => {
                    warn!("SSE stream error: {e}");
                    return Err(TransportError::Sse(format!("Stream error: {e}")).into());
                }
            }
        }

        warn!("SSE stream ended");
        Err(TransportError::Sse("Stream ended unexpectedly".to_string()).into())
    }

    /// Process SSE event data
    async fn process_sse_event(&self, data: &str) {
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            trace!("SSE event: {line}");

            // SSE format: "data: {...}"
            if let Some(json_str) = line.strip_prefix("data:") {
                let json_str = json_str.trim();

                // Try to parse as JSON-RPC notification
                if let Ok(notification) = serde_json::from_str::<Value>(json_str) {
                    debug!("Received SSE notification: {notification}");
                    // TODO: Handle notifications (e.g., tool list changes)
                }
            }
        }
    }

    /// Send a JSON-RPC request via HTTP POST
    async fn send_post_request(&self, request: &JsonRpcRequest) -> Result<Value> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed);
        }

        let url = self.url(&self.post_path);
        let headers = self.build_headers().await;
        let body = serde_json::to_vec(request)?;

        trace!("POST {url}: {}", String::from_utf8_lossy(&body));

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| TransportError::Http {
                status: 0,
                message: format!("Request failed: {e}"),
            })?;

        let status = response.status();

        // Handle authentication errors
        if status == StatusCode::UNAUTHORIZED {
            return Err(McpError::OAuth(crate::error::OAuthError::TokenExpired));
        }

        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(TransportError::Http {
                status: status.as_u16(),
                message: error_text,
            }
            .into());
        }

        let response_body = response.bytes().await.map_err(|e| TransportError::Http {
            status: 0,
            message: format!("Failed to read response: {e}"),
        })?;

        let response_str = String::from_utf8_lossy(&response_body);
        trace!("Response: {response_str}");

        // Notifications are acknowledged with an empty 202/204 body.
        if response_body.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(Value::Null);
        }
        let json_response: JsonRpcResponse = serde_json::from_slice(&response_body)?;

        if let Some(error) = json_response.error {
            Err(McpError::JsonRpc {
                code: error.code,
                message: error.message,
                data: error.data,
            })
        } else {
            Ok(json_response.result.unwrap_or(Value::Null))
        }
    }

    /// Perform a health check
    pub async fn health_check(&self) -> bool {
        let url = self.url(&self.post_path);
        let headers = self.build_headers().await;

        match self.client.head(&url).headers(headers).send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);
        self.send_post_request(&request).await
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        // Notifications are the same as requests but without waiting for response
        // In SSE transport, we can just send without ID
        let request = JsonRpcRequest::notification(method, params);
        let _ = self.send_post_request(&request).await?;
        Ok(())
    }

    async fn send(&self, message: crate::protocol::JsonRpcMessage) -> Result<()> {
        // POST the message; the reply (if any) is not surfaced here.
        use crate::protocol::JsonRpcMessage;
        let body = match message {
            JsonRpcMessage::Request(r) => r,
            JsonRpcMessage::Notification(n) => JsonRpcRequest::notification(n.method, n.params),
            _ => return Err(McpError::Protocol("clients may only send requests and notifications".into())),
        };
        let _ = self.send_post_request(&body).await?;
        Ok(())
    }

    async fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.health_check().await
    }

    async fn close(&self) -> Result<()> {
        info!("Closing SSE transport");
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn transport_type(&self) -> TransportType {
        TransportType::Sse
    }
}

impl Drop for SseTransport {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        debug!("SseTransport dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_backoff_delay() {
        let config = SseConfig {
            url: "http://localhost:3000".to_string(),
            sse_path: None,
            post_path: None,
            auth_token: None,
            timeout_secs: 60,
            reconnect: ReconnectConfig {
                enabled: true,
                base_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(60),
                max_attempts: 10,
                jitter: 0.0,
            },
            pin: None,
        };

        let transport = SseTransport::new(config).unwrap();

        // Test exponential backoff
        assert_eq!(transport.backoff_delay(0).as_secs(), 1);
        assert_eq!(transport.backoff_delay(1).as_secs(), 2);
        assert_eq!(transport.backoff_delay(2).as_secs(), 4);
        assert_eq!(transport.backoff_delay(3).as_secs(), 8);
    }

    #[tokio::test]
    async fn test_should_reconnect() {
        let config = SseConfig {
            url: "http://localhost:3000".to_string(),
            sse_path: None,
            post_path: None,
            auth_token: None,
            timeout_secs: 60,
            reconnect: ReconnectConfig {
                enabled: true,
                base_delay: Duration::from_secs(1),
                max_delay: Duration::from_secs(60),
                max_attempts: 3,
                jitter: 0.0,
            },
            pin: None,
        };

        let transport = SseTransport::new(config).unwrap();
        assert!(transport.should_reconnect());

        transport.reconnect_attempts.store(3, Ordering::SeqCst);
        assert!(!transport.should_reconnect());
    }
}
