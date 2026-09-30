//! Integration tests for MCP stdio transport
//!
//! These tests verify the stdio transport by spawning actual subprocesses and
//! exchanging JSON-RPC messages with them over stdin/stdout.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mcp::transport::{McpTransport, StdioTransport};
use mcp::types::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest};
use mcp::StdioConfig;

fn config(command: &str, args: &[&str]) -> StdioConfig {
    StdioConfig {
        command: command.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        env: HashMap::new(),
        cwd: None,
        timeout_secs: 5,
    }
}

/// `receive()` only sees messages that arrive after it subscribes, so start
/// listening before anything is sent.
fn listen(
    transport: &Arc<StdioTransport>,
) -> tokio::task::JoinHandle<mcp::Result<Option<JsonRpcMessage>>> {
    let transport = transport.clone();
    tokio::spawn(async move { transport.receive().await })
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Test that we can spawn a simple cat process and echo a message through it
#[tokio::test]
async fn test_stdio_spawn_cat() {
    // 'cat' simply echoes back what we send to it
    let transport = StdioTransport::spawn(config("cat", &[]))
        .await
        .expect("Failed to spawn transport");
    assert!(transport.is_healthy().await);

    let listener = listen(&transport);
    settle().await;

    let notification = JsonRpcNotification::new("test/notification", None);
    transport
        .send(JsonRpcMessage::Notification(notification))
        .await
        .expect("Failed to send");

    let response = tokio::time::timeout(Duration::from_secs(5), listener)
        .await
        .expect("timed out waiting for echo")
        .expect("listener panicked")
        .expect("Failed to receive");
    assert!(
        response.is_some(),
        "Should receive echoed message from cat"
    );

    transport.close().await.expect("Failed to close");
    assert!(!transport.is_healthy().await);
}

/// Test sending and receiving JSON-RPC messages with echo
#[tokio::test]
async fn test_stdio_jsonrpc_echo() {
    let transport = StdioTransport::spawn(config("cat", &[]))
        .await
        .expect("Failed to spawn transport");

    let listener = listen(&transport);
    settle().await;

    let request = JsonRpcRequest::new(1, "test/method", Some(serde_json::json!({"key": "value"})));
    transport
        .send(JsonRpcMessage::Request(request))
        .await
        .expect("Failed to send request");

    let echoed = tokio::time::timeout(Duration::from_secs(5), listener)
        .await
        .expect("timed out waiting for echo")
        .expect("listener panicked")
        .expect("Failed to receive")
        .expect("expected an echoed message");

    // Verify the echoed message is valid JSON-RPC carrying our method
    let echoed = serde_json::to_value(&echoed).expect("serialize echoed message");
    assert_eq!(echoed["method"], "test/method");
    assert_eq!(echoed["params"]["key"], "value");

    transport.close().await.expect("Failed to close");
}

/// Test that transport properly handles process exit
#[tokio::test]
async fn test_stdio_process_exit() {
    // 'echo' exits immediately after printing
    let transport = StdioTransport::spawn(config("echo", &["hello"]))
        .await
        .expect("Failed to spawn transport");

    // Give the process time to exit
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        !transport.is_healthy().await,
        "transport should report unhealthy once the process has exited"
    );
    assert!(
        transport.request("ping", None).await.is_err(),
        "requests to an exited process must fail"
    );

    let _ = transport.close().await;
}

/// Test environment variable passing
#[tokio::test]
async fn test_stdio_env_vars() {
    let mut cfg = config("sh", &["-c", "echo \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"method\\\":\\\"env/report\\\",\\\"params\\\":{\\\"value\\\":\\\"$MCP_TEST_VAR\\\"}}\"; sleep 1"]);
    cfg.env
        .insert("MCP_TEST_VAR".to_string(), "test_value".to_string());

    let transport = StdioTransport::spawn(cfg)
        .await
        .expect("Failed to spawn transport");
    let listener = listen(&transport);

    let message = tokio::time::timeout(Duration::from_secs(5), listener)
        .await
        .expect("timed out waiting for env report")
        .expect("listener panicked")
        .expect("Failed to receive")
        .expect("expected a message from the child");
    let message = serde_json::to_value(&message).expect("serialize message");
    assert_eq!(message["params"]["value"], "test_value");

    transport.close().await.ok();
}

/// Test multiple sequential connections
#[tokio::test]
async fn test_stdio_multiple_connections() {
    for i in 0..3 {
        let transport = StdioTransport::spawn(config("cat", &[]))
            .await
            .unwrap_or_else(|e| panic!("Failed to spawn transport {i}: {e}"));
        assert!(transport.is_healthy().await);

        let listener = listen(&transport);
        settle().await;

        let notification = JsonRpcNotification::new("test", None);
        transport
            .send(JsonRpcMessage::Notification(notification))
            .await
            .unwrap_or_else(|e| panic!("Failed to send {i}: {e}"));

        let response = tokio::time::timeout(Duration::from_secs(5), listener)
            .await
            .expect("timed out waiting for echo")
            .expect("listener panicked")
            .expect("Failed to receive");
        assert!(response.is_some());

        transport
            .close()
            .await
            .unwrap_or_else(|e| panic!("Failed to close {i}: {e}"));
        assert!(!transport.is_healthy().await);
    }
}

/// Test that spawning fails for a non-existent command
#[tokio::test]
async fn test_stdio_nonexistent_command() {
    let result = StdioTransport::spawn(config("nonexistent_command_xyz_abc", &[])).await;
    assert!(
        result.is_err(),
        "Should fail to spawn a non-existent command"
    );
}

/// Test sending after close fails
#[tokio::test]
async fn test_stdio_send_after_close() {
    let transport = StdioTransport::spawn(config("cat", &[]))
        .await
        .expect("Failed to spawn transport");
    transport.close().await.expect("Failed to close");

    let notification = JsonRpcNotification::new("test", None);
    let result = transport
        .send(JsonRpcMessage::Notification(notification))
        .await;

    assert!(result.is_err(), "Should fail to send when closed");
}

/// Test receiving after close fails
#[tokio::test]
async fn test_stdio_receive_after_close() {
    let transport = StdioTransport::spawn(config("cat", &[]))
        .await
        .expect("Failed to spawn transport");
    transport.close().await.expect("Failed to close");

    let result = transport.receive().await;

    assert!(result.is_err(), "Should fail to receive when closed");
}
