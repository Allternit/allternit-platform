//! `GET /v1/voice/session`: the Voice Session protocol over WebSocket.
//!
//! JSON text frames ⇄ [`ClientMessage`]/[`ServerEvent`], binary frames ⇄
//! PCM16 audio. All behaviour lives in [`VoiceSession`]; this file only
//! authenticates and moves frames.
//!
//! Auth (checked before the upgrade; failures are HTTP 401):
//! - `?ticket=` — verified by the configured [`TicketVerifier`] (cloud).
//!   The default verifier rejects every ticket: this build cannot verify
//!   allternit-api tickets yet, and it never pretends to.
//! - `?token=` — must equal env `ALLTERNIT_VOICE_TOKEN` (the sidecar token
//!   Desktop passes in).
//! - neither, and `ALLTERNIT_VOICE_TOKEN` unset — loopback peers only.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tracing::{debug, warn};

use super::core::{CoreConfig, SessionInput, SessionOutput, VoiceSession};
use super::engine::EngineFactory;
use super::protocol::ClientMessage;

pub const SESSION_PATH: &str = "/v1/voice/session";
pub const TOKEN_ENV: &str = "ALLTERNIT_VOICE_TOKEN";
/// Largest accepted frame: 1 MiB (≈ 30 s of 16 kHz PCM16).
const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// Verifies a short-lived cloud session ticket issued by allternit-api.
pub trait TicketVerifier: Send + Sync + 'static {
    fn verify(&self, ticket: &str) -> Result<(), String>;
}

/// The default: no cloud ticket is accepted.
pub struct RejectTickets;

impl TicketVerifier for RejectTickets {
    fn verify(&self, _ticket: &str) -> Result<(), String> {
        Err("this voice service cannot verify cloud session tickets (no ticket verifier configured)".into())
    }
}

#[derive(Clone)]
pub struct SessionRouteState {
    pub factory: Arc<dyn EngineFactory>,
    pub token: Option<String>,
    pub verifier: Arc<dyn TicketVerifier>,
    pub config: CoreConfig,
}

impl SessionRouteState {
    pub fn new(factory: Arc<dyn EngineFactory>, token: Option<String>) -> Self {
        Self {
            factory,
            token: token.filter(|t| !t.is_empty()),
            verifier: Arc::new(RejectTickets),
            config: CoreConfig::default(),
        }
    }
}

pub fn router_with(state: SessionRouteState) -> Router {
    Router::new()
        .route(SESSION_PATH, get(upgrade))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct AuthParams {
    token: Option<String>,
    ticket: Option<String>,
}

async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<SessionRouteState>,
    Query(params): Query<AuthParams>,
    peer: Option<ConnectInfo<SocketAddr>>,
) -> Response {
    if let Err(message) = authorize(&state, &params, peer.map(|p| p.0)) {
        warn!(%message, "voice session rejected");
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized", "message": message })),
        )
            .into_response();
    }
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| serve(socket, state))
}

fn authorize(
    state: &SessionRouteState,
    p: &AuthParams,
    peer: Option<SocketAddr>,
) -> Result<(), String> {
    if let Some(ticket) = p.ticket.as_deref() {
        return state.verifier.verify(ticket);
    }
    match (&state.token, p.token.as_deref()) {
        (Some(expected), Some(given))
            if constant_time_eq(expected.as_bytes(), given.as_bytes()) =>
        {
            Ok(())
        }
        (Some(_), Some(_)) => Err("invalid token".into()),
        (Some(_), None) => Err("missing token".into()),
        (None, _) => match peer {
            Some(addr) if addr.ip().is_loopback() => Ok(()),
            Some(_) => Err(format!(
                "{TOKEN_ENV} is not set: only loopback connections are allowed"
            )),
            None => Err(format!(
                "{TOKEN_ENV} is not set and the peer address is unknown"
            )),
        },
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn serve(socket: WebSocket, state: SessionRouteState) {
    let handle = VoiceSession::spawn(state.factory.clone(), state.config.clone());
    let input = handle.input;
    let mut output = handle.output;
    let (mut sink, mut stream) = socket.split();

    let writer = tokio::spawn(async move {
        while let Some(out) = output.recv().await {
            let msg = match out {
                SessionOutput::Event(ev) => match serde_json::to_string(&ev) {
                    Ok(json) => Message::Text(json),
                    Err(e) => {
                        warn!("serialise event: {e}");
                        continue;
                    }
                },
                SessionOutput::Audio(bytes) => Message::Binary(bytes),
                SessionOutput::Close => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        let input_msg = match msg {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(&text) {
                Ok(m) => SessionInput::Control(m),
                Err(e) => SessionInput::BadMessage(format!("unrecognised message: {e}")),
            },
            Message::Binary(bytes) => SessionInput::Audio(bytes),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        if input.send(input_msg).await.is_err() {
            break; // session ended
        }
    }
    drop(input);
    let _ = writer.await;
    debug!("voice session socket closed");
}
