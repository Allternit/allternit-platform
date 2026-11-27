//! Outbound runtime-relay client — the same protocol the desktop app speaks
//! (see `surfaces/allternit-desktop/src/main/auth-manager.ts` and PR #424):
//! WSS to `/api/v1/runtime-relay/connect/:id`, `authenticate` with the paired
//! device token, request/response + streaming envelopes, 25s-cadence server
//! pings guarded by a heartbeat watchdog, exponential backoff reconnect.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio_tungstenite::tungstenite::Message;

use crate::config::NodeConfig;
use crate::handlers::{self, DaemonState, InboundRequest, ResponseBody};
use crate::identity::RuntimeIdentity;

/// Server relay ping cadence is 25s; two missed pings plus jitter means the
/// connection is silently dead (same 75s budget as the desktop watchdog).
const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);
const WATCHDOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const BACKOFF_INITIAL: std::time::Duration = std::time::Duration::from_secs(1);
const BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// Capabilities the daemon advertises at authenticate time. `node.core` is
/// the product-level feature set; the runtime:* entries are the pairing-space
/// capabilities the relay's path table maps requests onto. Screen capture
/// (`runtime:remote_control`), ACI, voice, and providers are deliberately
/// absent — those need the desktop app.
pub const ADVERTISED_CAPABILITIES: &[&str] = &[
    "node.core",
    "runtime:connect",
    "runtime:execute",
    "runtime:terminal",
    "runtime:files",
];

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutboundMessage {
    Authenticate {
        runtime_id: String,
        device_token: String,
        client: String,
        capabilities: Vec<String>,
        /// Start anywhere (D6): a cloud computer has no Desktop safety switch, so it always
        /// takes `POST /api/v1/session-start`. Without it cloud-api never matches a woken
        /// cloud computer for a start and answers "warming".
        remote_start: bool,
    },
    ResponseStart {
        request_id: String,
        status: u16,
        headers: std::collections::HashMap<String, String>,
    },
    ResponseChunk {
        request_id: String,
        body: String,
        body_encoding: String,
    },
    ResponseEnd {
        request_id: String,
    },
    SocketClose {
        socket_id: String,
        code: u16,
        reason: String,
    },
    Pong,
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InboundMessage {
    Authenticated { runtime_id: String },
    Request {
        request_id: String,
        method: String,
        path: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        body_encoding: String,
        /// The allow-listed request headers cloud-api forwards (`Idempotency-Key`, …).
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
    },
    SocketOpen { socket_id: String, path: String },
    Ping,
}

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;
type SharedSink =
    Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WsStream, Message>>>;

fn relay_url(config: &NodeConfig, runtime_id: &str) -> String {
    let base = config.cloud_api_url.trim_end_matches('/');
    let mut url = format!("{base}/api/v1/runtime-relay/connect/{runtime_id}");
    if let Some(rest) = url.strip_prefix("https://") {
        url = format!("wss://{rest}");
    } else if let Some(rest) = url.strip_prefix("http://") {
        url = format!("ws://{rest}");
    }
    url
}

/// Run the connect → serve → reconnect loop forever (until the process is
/// signalled). Before every connect attempt the identity file is re-read: the
/// desktop app rotates the device token in place, and the daemon adopts the
/// rotated credential without a restart. A missing or unreadable file keeps
/// the last-known in-memory identity — a transient disk issue must not kill a
/// healthy relay.
pub async fn run(config: NodeConfig, identity: RuntimeIdentity, state: Arc<DaemonState>) {
    let mut identity = identity;
    let mut backoff = BACKOFF_INITIAL;
    loop {
        identity = reload_identity(&config.identity_path, &identity);
        match serve(&config, &identity, &state).await {
            Ok(()) => tracing::info!("relay connection closed; reconnecting"),
            Err(error) => tracing::warn!("relay connection failed: {error:#}"),
        }
        state.relay.reconnects.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Re-read the identity file and adopt it when the credential changed.
/// Returns `current` untouched when the file is missing, unreadable, or
/// carries the same token/expiry.
fn reload_identity(identity_path: &std::path::Path, current: &RuntimeIdentity) -> RuntimeIdentity {
    match RuntimeIdentity::load(identity_path) {
        Ok(latest) if latest.same_credential(current) => current.clone(),
        Ok(latest) => {
            tracing::info!(
                "adopted rotated identity from {}",
                identity_path.display()
            );
            latest
        }
        Err(error) => {
            tracing::warn!(
                "identity reload from {} failed ({error}); keeping last-known identity",
                identity_path.display()
            );
            current.clone()
        }
    }
}

/// One relay connection lifecycle. Returns when the socket closes or the
/// heartbeat watchdog fires.
async fn serve(
    config: &NodeConfig,
    identity: &RuntimeIdentity,
    state: &Arc<DaemonState>,
) -> anyhow::Result<()> {
    let url = relay_url(config, &identity.runtime_id);
    let (socket, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|error| anyhow::anyhow!("connect {url}: {error}"))?;
    let (sink, mut stream) = socket.split();
    let sink: SharedSink = Arc::new(tokio::sync::Mutex::new(sink));

    send(
        &mut *sink.lock().await,
        &OutboundMessage::Authenticate {
            runtime_id: identity.runtime_id.clone(),
            device_token: identity.device_token.clone(),
            client: "allternit-node".to_string(),
            capabilities: ADVERTISED_CAPABILITIES
                .iter()
                .map(|cap| cap.to_string())
                .collect(),
            remote_start: true,
        },
    )
    .await?;

    let mut last_message = Instant::now();
    let mut watchdog = tokio::time::interval(WATCHDOG_INTERVAL);
    watchdog.tick().await; // first tick is immediate

    loop {
        tokio::select! {
            inbound = stream.next() => {
                let Some(inbound) = inbound else {
                    anyhow::bail!("relay socket closed");
                };
                let message = inbound.map_err(|error| anyhow::anyhow!("relay read: {error}"))?;
                last_message = Instant::now();
                match message {
                    Message::Text(text) => {
                        match serde_json::from_str::<InboundMessage>(&text) {
                            Ok(InboundMessage::Authenticated { runtime_id }) => {
                                tracing::info!(%runtime_id, "runtime relay connected (node.core)");
                                state.relay.connects.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(InboundMessage::Ping) => {
                                send(&mut *sink.lock().await, &OutboundMessage::Pong).await?;
                            }
                            Ok(InboundMessage::Request { request_id, method, path, body, body_encoding, headers }) => {
                                let body = if body_encoding == "base64" {
                                    base64_decode(&body)
                                } else {
                                    body.into_bytes()
                                };
                                let state = state.clone();
                                let sink = sink.clone();
                                tokio::spawn(async move {
                                    let request = InboundRequest { method, path, body, headers };
                                    if let Err(error) =
                                        respond(request_id, request, state, sink).await
                                    {
                                        tracing::warn!(%error, "relay response failed");
                                    }
                                });
                            }
                            Ok(InboundMessage::SocketOpen { socket_id, .. }) => {
                                // node.core does not tunnel browser sockets;
                                // refuse so the browser pump closes instead of
                                // hanging. Terminal streaming rides request/
                                // response envelopes, not socket tunnels.
                                send(&mut *sink.lock().await, &OutboundMessage::SocketClose {
                                    socket_id,
                                    code: 1008,
                                    reason: "allternit-node does not tunnel sockets (node.core)"
                                        .to_string(),
                                })
                                .await?;
                            }
                            Err(_) => {} // unknown envelope: ignore
                        }
                    }
                    Message::Close(_) => anyhow::bail!("relay close frame"),
                    Message::Ping(bytes) => {
                        sink.lock()
                            .await
                            .send(Message::Pong(bytes))
                            .await
                            .map_err(|error| anyhow::anyhow!("relay pong: {error}"))?;
                    }
                    _ => {}
                }
            }
            _ = watchdog.tick() => {
                if last_message.elapsed() > HEARTBEAT_TIMEOUT {
                    tracing::warn!("relay heartbeat timed out; reconnecting");
                    anyhow::bail!("heartbeat timeout");
                }
            }
        }
    }
}

async fn send(
    sink: &mut (impl futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    message: &OutboundMessage,
) -> anyhow::Result<()> {
    let text = serde_json::to_string(message).expect("envelope serializes");
    sink.send(Message::Text(text))
        .await
        .map_err(|error| anyhow::anyhow!("relay write: {error}"))
}

/// Drive one request through the handlers and stream its envelopes onto the
/// shared sink. Buffered responses complete immediately; streaming responses
/// (terminal SSE) keep flowing until the pane exits or the socket dies.
async fn respond(
    request_id: String,
    request: InboundRequest,
    state: Arc<DaemonState>,
    sink: SharedSink,
) -> anyhow::Result<()> {
    let response = handlers::handle(&state, &request).await;
    let mut headers = std::collections::HashMap::new();
    headers.insert(
        "content-type".to_string(),
        response.content_type.to_string(),
    );
    send(
        &mut *sink.lock().await,
        &OutboundMessage::ResponseStart {
            request_id: request_id.clone(),
            status: response.status,
            headers,
        },
    )
    .await?;

    match response.body {
        ResponseBody::Full(bytes) => {
            send_chunk(&sink, &request_id, &bytes).await?;
        }
        ResponseBody::Stream(mut rx, _) => {
            while let Some(chunk) = rx.recv().await {
                send_chunk(&sink, &request_id, &chunk).await?;
            }
        }
    }
    send(
        &mut *sink.lock().await,
        &OutboundMessage::ResponseEnd { request_id },
    )
    .await
}

async fn send_chunk(
    sink: &SharedSink,
    request_id: &str,
    chunk: &[u8],
) -> anyhow::Result<()> {
    send(
        &mut *sink.lock().await,
        &OutboundMessage::ResponseChunk {
            request_id: request_id.to_string(),
            body: String::from_utf8_lossy(chunk).into_owned(),
            body_encoding: "utf8".to_string(),
        },
    )
    .await
}

fn base64_decode(body: &str) -> Vec<u8> {
    // Minimal base64 decode without pulling a dep: relayed node.* requests
    // are JSON/UTF-8 in practice; fall back to empty on odd input.
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut nbits = 0u32;
    for byte in body.bytes() {
        if byte == b'=' {
            break;
        }
        let Some(pos) = TABLE.iter().position(|candidate| *candidate == byte) else {
            continue;
        };
        acc = (acc << 6) | pos as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_identity(token: &str) -> RuntimeIdentity {
        RuntimeIdentity {
            runtime_id: "rt_1".to_string(),
            device_token: token.to_string(),
            user_id: "u_1".to_string(),
            expires_at: Some("2027-01-01T00:00:00Z".to_string()),
        }
    }

    fn write_identity(path: &std::path::Path, token: &str) {
        std::fs::write(
            path,
            serde_json::to_string(&json!({
                "runtimeId": "rt_1",
                "deviceToken": token,
                "userId": "u_1",
                "expiresAt": "2027-01-01T00:00:00Z",
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn reload_adopts_swapped_token() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime-identity.json");
        let current = test_identity("tok_old");
        write_identity(&path, "tok_new");

        let adopted = reload_identity(&path, &current);
        assert_eq!(adopted.device_token, "tok_new");
        assert_eq!(adopted.runtime_id, "rt_1");
    }

    #[test]
    fn reload_keeps_last_known_identity_when_file_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime-identity.json");
        let current = test_identity("tok_old");

        let kept = reload_identity(&path, &current);
        assert_eq!(kept.device_token, "tok_old");
    }

    #[test]
    fn reload_keeps_last_known_identity_when_file_is_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime-identity.json");
        let current = test_identity("tok_old");
        std::fs::write(&path, "{not json").unwrap();

        let kept = reload_identity(&path, &current);
        assert_eq!(kept.device_token, "tok_old");
    }

    #[test]
    fn reload_keeps_current_when_credential_is_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime-identity.json");
        let current = test_identity("tok_same");
        write_identity(&path, "tok_same");

        let kept = reload_identity(&path, &current);
        assert_eq!(kept.device_token, "tok_same");
    }

    #[test]
    fn relay_url_upgrades_to_secure_websocket() {
        let config = NodeConfig {
            cloud_api_url: "https://api.allternit.com/".to_string(),
            ..NodeConfig::default()
        };
        assert_eq!(
            relay_url(&config, "rt_1"),
            "wss://api.allternit.com/api/v1/runtime-relay/connect/rt_1"
        );
        let local = NodeConfig {
            cloud_api_url: "http://127.0.0.1:9999".to_string(),
            ..NodeConfig::default()
        };
        assert_eq!(
            relay_url(&local, "rt_1"),
            "ws://127.0.0.1:9999/api/v1/runtime-relay/connect/rt_1"
        );
    }

    #[test]
    fn base64_decodes_relay_bodies() {
        assert_eq!(base64_decode("aGVsbG8="), b"hello");
        assert_eq!(base64_decode(""), b"");
        assert!(base64_decode("@@@===@@@").is_empty());
    }
}
