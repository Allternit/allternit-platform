//! Process-wide subscriber to the Gizzi runtime event bus.
//!
//! A single reconnecting SSE connection to `GET /v1/event` fans every bus
//! event out through a `tokio::sync::broadcast` channel, so any number of
//! in-flight gateway requests can watch their own Gizzi session without each
//! opening a dedicated SSE connection (correct under concurrency, unlike the
//! per-request connection pattern in `gizzi_completion.rs`).
//!
//! The connection starts lazily on the first `subscribe()` call and then
//! reconnects forever with exponential backoff (1s doubling to a 30s cap).
//! While Gizzi is down nothing is buffered — events are simply missed, and
//! subscribers treat `Lagged`/`Closed` as tolerable (their own timeouts
//! bound how long they wait).

use async_stream::stream;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::{broadcast, OnceCell};
use tracing::{info, warn};

/// One event from the Gizzi bus: `{type, properties}`.
#[derive(Debug, Clone)]
pub struct GizziEvent {
    pub event_type: String,
    pub properties: Value,
}

impl GizziEvent {
    /// Session id this event belongs to, if any. Bus events carry it either
    /// at `properties.sessionID` (`session.status`, `message.part.delta`,
    /// `session.error`, ...) or at `properties.info.sessionID`
    /// (`message.updated`, `session.updated`).
    pub fn session_id(&self) -> Option<&str> {
        self.properties
            .get("sessionID")
            .and_then(Value::as_str)
            .or_else(|| {
                self.properties
                    .get("info")
                    .and_then(|info| info.get("sessionID"))
                    .and_then(Value::as_str)
            })
    }
}

/// Broadcast capacity. Slow receivers drop the oldest events (`Lagged`),
/// which the collection loops in proxy.rs tolerate via their deadlines.
const CHANNEL_CAPACITY: usize = 512;

const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

static BUS: OnceCell<broadcast::Sender<GizziEvent>> = OnceCell::const_new();

/// Subscribe to the shared Gizzi event stream, starting the bus connection
/// on first use. The returned receiver only sees events published after it
/// was created — subscribe *before* triggering the work you want to watch.
pub async fn subscribe() -> broadcast::Receiver<GizziEvent> {
    let tx = BUS
        .get_or_init(|| async {
            let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
            tokio::spawn(pump(tx.clone()));
            tx
        })
        .await;
    tx.subscribe()
}

/// Stream of events belonging to one Gizzi session. Lagged receivers log and
/// continue; a closed channel (never in practice — the sender is
/// process-wide) ends the stream. Takes the session id by value so the
/// returned stream is free of borrows (it is commonly moved into response
/// streams alongside the caller's own copy of the id). The `Send` bound is
/// required by `Sse::new` on the proxy's streaming path.
pub async fn session_events(
    session_id: impl Into<String>,
) -> impl Stream<Item = GizziEvent> + Send {
    let mut rx = subscribe().await;
    let session_id = session_id.into();
    stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if event.session_id() == Some(session_id.as_str()) {
                        yield event;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, session_id, "Gizzi bus receiver lagged; events dropped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

/// Base URL of the Gizzi runtime, from the unified app config.
fn gizzi_base() -> String {
    crate::v1_routes::gizzi_base()
}

/// Reconnect-forever pump: one SSE connection to Gizzi, fan-out to `tx`.
async fn pump(tx: broadcast::Sender<GizziEvent>) {
    let base = gizzi_base();
    // No overall request timeout — this is a long-lived SSE stream. Only the
    // connect phase is bounded.
    let client = match reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            warn!(error = %err, "Failed to build Gizzi event client; bus disabled");
            return;
        }
    };

    let mut backoff = BACKOFF_INITIAL;
    loop {
        match client
            .get(format!("{base}/v1/event"))
            .header("Accept", "text/event-stream")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                info!(base = %base, "Connected to Gizzi event bus");
                backoff = BACKOFF_INITIAL;
                read_stream(resp, &tx).await;
                warn!("Gizzi event bus connection closed; reconnecting");
            }
            Ok(resp) => {
                warn!(status = %resp.status(), "Gizzi event bus subscribe failed; retrying");
            }
            Err(err) => {
                warn!(error = %err, "Gizzi event bus unreachable; retrying");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Read SSE frames (`data: {json}\n\n`) from the response body until it ends
/// or errors, forwarding each parsed event to the broadcast channel.
async fn read_stream(resp: reqwest::Response, tx: &broadcast::Sender<GizziEvent>) {
    let mut buf = String::new();
    let mut byte_stream = resp.bytes_stream();

    while let Some(chunk) = byte_stream.next().await {
        match chunk {
            Ok(bytes) => {
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(block_end) = buf.find("\n\n") {
                    let block = buf[..block_end].to_string();
                    buf = buf[block_end + 2..].to_string();
                    if let Some(event) = parse_frame(&block) {
                        // SendError just means zero subscribers right now.
                        let _ = tx.send(event);
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, "Gizzi event bus read error");
                return;
            }
        }
    }
}

/// Parse one SSE block into an event. Multi-line `data:` fields are joined
/// per the SSE spec; frames without JSON `data` (comments, heartbeats of
/// other shapes) are dropped.
fn parse_frame(block: &str) -> Option<GizziEvent> {
    let data = block
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(&data).ok()?;
    let event_type = value.get("type").and_then(Value::as_str)?.to_string();
    let properties = value.get("properties").cloned().unwrap_or(Value::Null);
    Some(GizziEvent {
        event_type,
        properties,
    })
}
