//! Stdio transport implementation for MCP client
//!
//! This transport spawns an MCP server as a subprocess and communicates
//! via JSON-RPC over stdin/stdout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tracing::{debug, error, info, trace, warn};

use crate::error::{McpError, Result, TransportError};
use crate::protocol::{JsonRpcMessage, JsonRpcRequest, JsonRpcResponse};
use crate::transport::{McpTransport, TransportType};

/// Stdio transport for MCP communication
///
/// Spawns an MCP server as a subprocess and communicates via JSON-RPC
/// over stdin (to server) and stdout (from server).
#[derive(Debug)]
pub struct StdioTransport {
    /// The spawned child process
    process: Arc<RwLock<Child>>,
    /// Writer to child's stdin
    stdin_writer: Arc<tokio::sync::Mutex<BufWriter<ChildStdin>>>,
    /// Counter for generating request IDs
    request_counter: AtomicU64,
    /// Channel for sending responses back to requesters
    response_tx: mpsc::UnboundedSender<(u64, Result<Value>)>,
    /// Every message the server sends, for raw `receive()` subscribers
    raw_message_tx: broadcast::Sender<std::result::Result<JsonRpcMessage, String>>,
    /// Pending requests awaiting responses
    pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    /// Whether the transport is closed
    closed: AtomicBool,
    /// Server configuration
    config: StdioConfig,
}

/// Configuration for stdio transport
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StdioConfig {
    /// Command to spawn
    pub command: String,
    /// Arguments for the command
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Working directory
    pub cwd: Option<PathBuf>,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl StdioConfig {
    /// Config for `command args` with environment variables and the default
    /// 30 second timeout. Pass it to `StdioTransport::spawn`.
    pub fn with_env(command: impl Into<String>, args: Vec<String>, env: HashMap<String, String>) -> StdioConfig {
        StdioConfig { command: command.into(), args, env, cwd: None, timeout_secs: 30 }
    }
}

impl Default for StdioConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            timeout_secs: 30,
        }
    }
}

impl StdioTransport {
    /// Spawn a new MCP server process and create a stdio transport
    ///
    /// # Arguments
    /// * `config` - Configuration for the stdio transport
    ///
    /// # Returns
    /// A new StdioTransport connected to the spawned process
    pub async fn spawn(config: StdioConfig) -> Result<Arc<Self>> {
        info!(
            "Spawning MCP server: {} {}",
            config.command,
            config.args.join(" ")
        );

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }

        // Kill the process when the transport is dropped
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| TransportError::Process(format!("Failed to spawn process: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| TransportError::Stdio("Failed to open stdin".to_string()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TransportError::Stdio("Failed to open stdout".to_string()))?;

        let stderr = child.stderr.take();

        let (response_tx, mut response_rx) = mpsc::unbounded_channel::<(u64, Result<Value>)>();

        let pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<_>>>> =
            Arc::new(StdMutex::new(HashMap::new()));

        let (raw_message_tx, _) = broadcast::channel::<std::result::Result<JsonRpcMessage, String>>(100);

        let transport = Arc::new(Self {
            raw_message_tx,
            process: Arc::new(RwLock::new(child)),
            stdin_writer: Arc::new(tokio::sync::Mutex::new(BufWriter::new(stdin))),
            request_counter: AtomicU64::new(1),
            response_tx,
            pending_requests: pending_requests.clone(),
            closed: AtomicBool::new(false),
            config: config.clone(),
        });

        // Start the stdout reader task
        let transport_clone = transport.clone();
        tokio::spawn(async move {
            if let Err(e) = transport_clone.read_stdout(stdout).await {
                error!("Stdout reader error: {e}");
            }
        });

        // Start the stderr reader task
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                Self::read_stderr(stderr).await;
            });
        }

        // Start the response dispatcher task
        tokio::spawn(async move {
            while let Some((id, result)) = response_rx.recv().await {
                let mut pending = pending_requests.lock().unwrap();
                if let Some(sender) = pending.remove(&id) {
                    let _ = sender.send(result);
                }
            }
        });

        info!("MCP server spawned successfully");
        Ok(transport)
    }

    /// Read from stdout and dispatch responses
    async fn read_stdout(&self, stdout: ChildStdout) -> Result<()> {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            trace!("Received: {line}");

            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                warn!("Failed to parse JSON-RPC message from server stdout");
                continue;
            };
            // Answers to `request()` go to their waiter.
            if value.get("method").is_none() {
                if let Ok(response) = serde_json::from_value::<JsonRpcResponse>(value.clone()) {
                    let result = if let Some(error) = response.error {
                        Err(McpError::JsonRpc {
                            code: error.code,
                            message: error.message,
                            data: error.data,
                        })
                    } else {
                        Ok(response.result.unwrap_or(Value::Null))
                    };
                    let _ = self.response_tx.send((response.id, result));
                }
            }
            // Every message also goes to raw `receive()` subscribers (none
            // is the normal case; a send error just means no listener).
            match serde_json::from_value::<JsonRpcMessage>(value) {
                Ok(message) => {
                    let _ = self.raw_message_tx.send(Ok(message));
                }
                Err(e) => {
                    let _ = self.raw_message_tx.send(Err(format!("unrecognized JSON-RPC message: {e}")));
                }
            }
        }

        debug!("Stdout reader closed");
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Read from stderr and log it
    async fn read_stderr(stderr: tokio::process::ChildStderr) {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            debug!("MCP server stderr: {line}");
        }
    }

    /// Send a raw JSON-RPC request
    async fn send_request(&self, request: JsonRpcRequest) -> Result<Value> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed);
        }

        let id = request
            .id
            .ok_or_else(|| McpError::Protocol("Request must have an ID".to_string()))?;

        // Create a oneshot channel for the response
        let (tx, rx) = oneshot::channel();

        // Store the pending request
        {
            let mut pending = self.pending_requests.lock().unwrap();
            pending.insert(id, tx);
        }

        // Serialize and send the request
        let json = serde_json::to_string(&request)?;
        trace!("Sending: {json}");

        {
            let mut writer = self.stdin_writer.lock().await;
            writer.write_all(json.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }

        // Wait for the response with timeout
        let timeout = tokio::time::Duration::from_secs(self.config.timeout_secs);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(McpError::ConnectionClosed),
            Err(_) => {
                // Remove the pending request on timeout
                let mut pending = self.pending_requests.lock().unwrap();
                pending.remove(&id);
                Err(McpError::Timeout(timeout))
            }
        }
    }

    /// Send a raw JSON-RPC notification (no response expected)
    async fn send_notification(&self, request: JsonRpcRequest) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed);
        }

        let json = serde_json::to_string(&request)?;
        trace!("Sending notification: {json}");

        let mut writer = self.stdin_writer.lock().await;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        Ok(())
    }

    /// Get the process ID if available
    pub async fn process_id(&self) -> Option<u32> {
        let process = self.process.read().await;
        process.id()
    }

    /// Check if the process is still running
    pub async fn is_process_running(&self) -> bool {
        let mut process = self.process.write().await;
        match process.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }
}

impl StdioTransport {
    /// Send a raw JSON-RPC message
    pub async fn send_message(&self, message: JsonRpcMessage) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed);
        }
        let json = serde_json::to_string(&message)?;
        trace!("Sending raw message: {json}");
        let mut writer = self.stdin_writer.lock().await;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(())
    }

    /// Receive the next raw JSON-RPC message (only messages that arrive
    /// after this call subscribes; `Ok(None)` if this listener lagged).
    pub async fn receive_message(&self) -> Result<Option<JsonRpcMessage>> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(McpError::ConnectionClosed);
        }
        let mut rx = self.raw_message_tx.subscribe();
        match rx.recv().await {
            Ok(Ok(msg)) => Ok(Some(msg)),
            Ok(Err(e)) => Err(McpError::Protocol(e)),
            Err(broadcast::error::RecvError::Closed) => Err(McpError::ConnectionClosed),
            Err(broadcast::error::RecvError::Lagged(_)) => Ok(None),
        }
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(&self, message: JsonRpcMessage) -> Result<()> {
        self.send_message(message).await
    }

    async fn receive(&self) -> Result<Option<JsonRpcMessage>> {
        self.receive_message().await
    }

    async fn request(&self, method: &str, params: Option<Value>) -> crate::error::Result<Value> {
        let id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);
        self.send_request(request).await
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> crate::error::Result<()> {
        let request = JsonRpcRequest::notification(method, params);
        self.send_notification(request).await
    }

    async fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::SeqCst) && self.is_process_running().await
    }

    async fn close(&self) -> crate::error::Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        info!("Closing stdio transport");

        // Send a shutdown notification if possible
        let _ = self.notify("notifications/initialized", None).await;

        // Kill the process
        let mut process = self.process.write().await;
        match process.kill().await {
            Ok(_) => {
                info!("Process killed successfully");
                Ok(())
            }
            Err(e) => {
                warn!("Failed to kill process: {e}");
                // Process might already be dead, which is fine
                Ok(())
            }
        }
    }

    fn transport_type(&self) -> TransportType {
        TransportType::Stdio
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Best effort cleanup - the process will be killed due to kill_on_drop
        debug!("StdioTransport dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stdio_transport_basic() {
        // This would require a mock MCP server binary
        // For now, just test the config creation
        let config = StdioConfig {
            command: "echo".to_string(),
            args: vec!["test".to_string()],
            env: HashMap::new(),
            cwd: None,
            timeout_secs: 5,
        };

        assert_eq!(config.command, "echo");
        assert_eq!(config.args, vec!["test"]);
    }
}
