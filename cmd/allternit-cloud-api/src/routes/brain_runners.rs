//! The brain runner socket (CONTRACT-connections-2026-11 section 9.7, D5).
//!
//! `GET /api/v1/brain-runner/socket` (WebSocket). The runner program inside a
//! person's box dials out here; the box takes no inbound connections.
//!
//! - Auth: `Authorization: Bearer <runner token>` on the upgrade. The token's
//!   SHA-256 is looked up in `brain_runners`; the runner id comes from that
//!   row, never from the runner. A bad or missing token is 401 before any
//!   upgrade. A second connection for the same runner replaces the first.
//! - Frames: one JSON object per text frame, at most 1 MiB, correlated by
//!   `id` (services::brain_runners does the request/response side).
//! - Not a session route: no Clerk session, no `require_compute`, and runners
//!   never appear in compute-status.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;

use crate::services::brain_runners::BrainRunnerService;

pub const SOCKET_PATH: &str = "/api/v1/brain-runner/socket";
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const PING_EVERY: Duration = Duration::from_secs(30);

/// The socket route, holding the process's runner service as its state.
pub fn routes<S: Clone + Send + Sync + 'static>(service: Arc<BrainRunnerService>) -> Router<S> {
    Router::new().route(SOCKET_PATH, get(socket)).with_state(service)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": { "code": "unauthorized", "message": "Unauthorized" } }))).into_response()
}

async fn socket(
    State(service): State<Arc<BrainRunnerService>>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    // Auth before the upgrade is even looked at.
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let Some(token) = token else {
        return unauthorized();
    };
    let runner_id = match service.authenticate_runner(token).await {
        Ok(Some(id)) => id,
        Ok(None) => return unauthorized(),
        Err(error) => return error.into_response(),
    };
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| pump(socket, service, runner_id))
}

/// Moves frames between the WebSocket and the service until either side
/// closes. The service dropping this runner's entry (replaced, idle stop,
/// turn timeout) ends `outgoing`, which closes the socket.
async fn pump(socket: WebSocket, service: Arc<BrainRunnerService>, runner_id: String) {
    let mut conn = service.attach_runner(&runner_id);
    let (mut sink, mut stream) = socket.split();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ping.tick().await;
    let mut pings: u64 = 0;
    loop {
        tokio::select! {
            outgoing = conn.outgoing.recv() => match outgoing {
                Some(text) => {
                    if sink.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if text.len() <= MAX_FRAME_BYTES {
                        service.runner_frame(&runner_id, conn.conn_id, &text).await;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = ping.tick() => {
                pings += 1;
                let frame = json!({ "type": "ping", "id": format!("ping-{pings}") }).to_string();
                if sink.send(Message::Text(frame)).await.is_err() {
                    break;
                }
            }
        }
    }
    service.detach_runner(&runner_id, conn.conn_id);
    let _ = sink.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::brain_runners::{test_runner_db, test_service_with_box, Vendor, VendorState};
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Error as WsError, Message as WsMessage};

    /// Serve the real route on a local port.
    async fn serve(service: Arc<BrainRunnerService>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app: Router = routes(service);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("ws://{addr}{SOCKET_PATH}")
    }

    async fn dial(url: &str, token: Option<&str>) -> Result<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        WsError,
    > {
        let mut request = url.into_client_request().unwrap();
        if let Some(token) = token {
            request.headers_mut().insert("Authorization", format!("Bearer {token}").parse().unwrap());
        }
        tokio_tungstenite::connect_async(request).await.map(|(ws, _)| ws)
    }

    fn status_of(result: Result<impl Sized, WsError>) -> u16 {
        match result {
            Err(WsError::Http(response)) => response.status().as_u16(),
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => 101,
        }
    }

    #[tokio::test]
    async fn brain_runner_socket_rejects_a_bad_token_and_accepts_the_box_token() {
        let db = test_runner_db().await;
        // A box for user_a; its runner id and a token we control.
        let (service, _backend) = test_service_with_box(db.clone(), "user_a").await;
        let token = "known-runner-token";
        sqlx::query("UPDATE brain_runners SET token_hash = encode(sha256($1::bytea), 'hex') WHERE user_id = 'user_a'")
            .bind(token.as_bytes())
            .execute(&db)
            .await
            .unwrap();
        let url = serve(service.clone()).await;

        assert_eq!(status_of(dial(&url, None).await), 401);
        assert_eq!(status_of(dial(&url, Some("not-a-runner-token")).await), 401);

        // The right token upgrades; its hello is recorded against that box.
        let mut ws = dial(&url, Some(token)).await.expect("the box token upgrades");
        let hello = json!({ "type": "hello", "runner": "0.1.0", "vendors": { "grok": "signed_in" } });
        ws.send(WsMessage::Text(hello.to_string())).await.unwrap();
        let mut found = false;
        for _ in 0..40 {
            if service.vendor_state("user_a").await.unwrap().contains(&(Vendor::Grok, VendorState::On { found: None })) {
                found = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(found, "hello over the real socket updates the row");
    }
}
