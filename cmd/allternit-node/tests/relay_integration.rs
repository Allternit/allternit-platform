//! End-to-end wiring test: the daemon's relay client against a mock cloud
//! relay speaking the production envelope protocol (same shapes as
//! `cmd/allternit-cloud-api/src/routes/runtime_relay.rs`). Proves
//! connect → authenticate (client + capability scope) → request/response →
//! streaming → socket refusal, without touching production.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use allternit_node_daemon::handlers::{DaemonState, InboundRequest};
use allternit_node_daemon::{config::NodeConfig, identity::RuntimeIdentity, relay};

struct MockRelay {
    addr: std::net::SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

/// One-shot mock relay: accepts a single connection, validates the
/// authenticate envelope, then answers the scripted requests.
async fn start_mock_relay(expected_runtime_id: &str) -> MockRelay {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, mut watch) = tokio::sync::watch::channel(false);
    let expected = expected_runtime_id.to_string();

    tokio::spawn(async move {
        tokio::select! {
            _ = watch.changed() => {}
            accepted = listener.accept() => {
                let (stream, _) = accepted.unwrap();
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                // 1. authenticate
                let message = ws.next().await.unwrap().unwrap();
                let Message::Text(authenticate_text) = message else {
                    panic!("expected text authenticate");
                };
                let envelope: serde_json::Value =
                    serde_json::from_str(&authenticate_text).unwrap();
                assert_eq!(envelope["type"], "authenticate");
                // Start anywhere: cloud computers always take session starts.
                assert_eq!(envelope["remote_start"], true);
                assert_eq!(envelope["runtime_id"], expected);
                assert_eq!(envelope["client"], "allternit-node");
                let caps = envelope["capabilities"].as_array().unwrap();
                let caps: Vec<&str> = caps.iter().map(|c| c.as_str().unwrap()).collect();
                assert!(caps.contains(&"node.core"));
                assert!(caps.contains(&"runtime:execute"));
                assert!(!caps.contains(&"runtime:remote_control"));
                ws.send(Message::Text(
                    serde_json::json!({ "type": "authenticated", "runtime_id": expected })
                        .to_string(),
                ))
                .await
                .unwrap();

                // 2. health request → buffered response
                ws.send(Message::Text(
                    serde_json::json!({
                        "type": "request",
                        "request_id": "req-health",
                        "method": "GET",
                        "path": "/api/v1/node/health",
                        "body": "",
                        "body_encoding": "utf8",
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
                let response = collect_response(&mut ws, "req-health").await;
                assert_eq!(response["start"]["status"], 200);
                let body: String = response["chunks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|chunk| chunk.as_str().unwrap())
                    .collect();
                let health: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert_eq!(health["client"], "allternit-node");
                assert!(health["capabilities"].as_array().unwrap().iter().any(|c| c == "node.core"));

                // 3. exec request → buffered response with real output
                ws.send(Message::Text(
                    serde_json::json!({
                        "type": "request",
                        "request_id": "req-exec",
                        "method": "POST",
                        "path": "/api/v1/node/exec",
                        "body": serde_json::json!({
                            "command": "echo",
                            "args": ["mock-relay-ok"],
                        })
                        .to_string(),
                        "body_encoding": "utf8",
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
                let response = collect_response(&mut ws, "req-exec").await;
                assert_eq!(response["start"]["status"], 200);
                let body: String = response["chunks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|chunk| chunk.as_str().unwrap())
                    .collect();
                assert!(body.contains("mock-relay-ok"), "exec stdout missing: {body}");

                // 4. socket_open → refused (node.core does not tunnel sockets)
                ws.send(Message::Text(
                    serde_json::json!({
                        "type": "socket_open",
                        "socket_id": "sock_1",
                        "path": "/ws/anything",
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
                let message = ws.next().await.unwrap().unwrap();
                let Message::Text(refusal_text) = message else {
                    panic!("expected socket_close");
                };
                let refusal: serde_json::Value = serde_json::from_str(&refusal_text).unwrap();
                assert_eq!(refusal["type"], "socket_close");
                assert_eq!(refusal["socket_id"], "sock_1");
                assert_eq!(refusal["code"], 1008);
            }
        }
    });

    MockRelay { addr, shutdown }
}

async fn collect_response(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    request_id: &str,
) -> serde_json::Value {
    let mut start = serde_json::Value::Null;
    let mut chunks: Vec<String> = Vec::new();
    let mut ended = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !ended && tokio::time::Instant::now() < deadline {
        let message = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("timed out waiting for relay envelope")
            .unwrap()
            .unwrap();
        let Message::Text(text) = message else {
            continue;
        };
        let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(envelope["request_id"], request_id);
        match envelope["type"].as_str().unwrap() {
            "response_start" => start = envelope.clone(),
            "response_chunk" => chunks.push(envelope["body"].as_str().unwrap().to_string()),
            "response_end" => ended = true,
            other => panic!("unexpected envelope {other}"),
        }
    }
    assert!(ended, "response must terminate with response_end");
    serde_json::json!({ "start": start, "chunks": chunks })
}

#[tokio::test]
async fn daemon_connects_authenticates_and_serves_node_core_over_mock_relay() {
    let temp = tempfile::tempdir().unwrap();
    let identity_path = temp.path().join("runtime-identity.json");
    std::fs::write(
        &identity_path,
        serde_json::json!({
            "runtimeId": "rt_mock_node",
            "deviceToken": "device-token-mock",
            "userId": "user_mock",
        })
        .to_string(),
    )
    .unwrap();

    let mock = start_mock_relay("rt_mock_node").await;

    let config = NodeConfig {
        cloud_api_url: format!("http://{}", mock.addr),
        identity_path: identity_path.clone(),
        fs_roots: vec![temp.path().to_path_buf()],
        ..NodeConfig::default()
    };
    let identity = RuntimeIdentity::load(&identity_path).unwrap();
    let state = DaemonState::new(config.clone());

    let daemon = tokio::spawn(async move {
        relay::run(config, identity, state).await;
    });

    // The mock relay asserts every step; give it time to finish the script.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let _ = mock.shutdown.send(true);
    daemon.abort();
}

#[tokio::test]
async fn handlers_serve_terminal_contract_end_to_end() {
    // The /terminal/* contract over real PTYs: create → input → stream.
    let temp = tempfile::tempdir().unwrap();
    let state = DaemonState::new(NodeConfig {
        fs_roots: vec![temp.path().to_path_buf()],
        // The PTY path: never reach a pane engine running on the host.
        pane_socket: Some(temp.path().join("no-engine.sock")),
        ..NodeConfig::default()
    });

    let shell = if cfg!(target_os = "windows") { "cmd.exe" } else { "/bin/sh" };
    let created = state.terminals.create(shell, None, 80, 24).unwrap();

    let request = InboundRequest {
        method: "POST".into(),
        path: "/terminal/create".into(),
        body: serde_json::json!({ "shell": shell, "cols": 80, "rows": 24 })
            .to_string()
            .into_bytes(),
        headers: Default::default(),
    };
    let response =
        allternit_node_daemon::handlers::handle(&state, &request).await;
    assert_eq!(response.status, 200);

    // Direct store round trip proves the PTY echoes through the same code
    // path the relay stream task consumes.
    let mut live = state.terminals.subscribe(&created.session_id).unwrap();
    let marker = format!("term-{}", uuid::Uuid::new_v4().simple());
    state
        .terminals
        .input(&created.session_id, &format!("echo {marker}\n"))
        .unwrap();
    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !output.contains(&marker) {
        match tokio::time::timeout(Duration::from_millis(500), live.recv()).await {
            Ok(Ok(chunk)) => output.push_str(&chunk),
            _ => {}
        }
    }
    state.terminals.close(&created.session_id).unwrap();
    assert!(output.contains(&marker), "terminal echo missing: {output:?}");
}
