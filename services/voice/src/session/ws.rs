//! `GET /v1/voice/session`: the Voice Session protocol over WebSocket.
//!
//! JSON text frames ⇄ [`ClientMessage`]/[`ServerEvent`], binary frames ⇄
//! PCM16 audio. All behaviour lives in [`VoiceSession`]; this file only
//! authenticates and moves frames.
//!
//! Auth (checked before the upgrade; failures are HTTP 401):
//! - `?ticket=` — verified by the configured [`TicketVerifier`] (cloud).
//!   [`CloudTicketVerifier`] redeems it against cloud-api when
//!   `ALLTERNIT_CLOUD_API_URL` + `ALLTERNIT_VOICE_WORKER_TOKEN` are set;
//!   otherwise every ticket is rejected. Ticket sessions are cloud sessions:
//!   `session.ready` says `cloud`, `maxSeconds` is enforced, and the
//!   wall-clock minutes are reported to cloud-api on close.
//! - `?token=` — must equal env `ALLTERNIT_VOICE_TOKEN` (the sidecar token
//!   Desktop passes in).
//! - neither, and `ALLTERNIT_VOICE_TOKEN` unset — loopback peers only.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::time::Instant;
use tracing::{debug, warn};

use super::core::{CoreConfig, SessionInput, SessionOutput, VoiceSession};
use super::engine::EngineFactory;
use super::protocol::{codes, ClientMessage, EngineKind, ServerEvent};

pub const SESSION_PATH: &str = "/v1/voice/session";
pub const TOKEN_ENV: &str = "ALLTERNIT_VOICE_TOKEN";
/// Largest accepted frame: 1 MiB (≈ 30 s of 16 kHz PCM16).
const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// What a redeemed cloud ticket grants.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketClaims {
    pub sub: String,
    pub plan: String,
    pub max_seconds: u64,
}

pub type VerifyFuture<'a> = Pin<Box<dyn Future<Output = Result<TicketClaims, String>> + Send + 'a>>;

/// Verifies a short-lived cloud session ticket issued by allternit-api.
/// Runs before the WebSocket upgrade, so a bad ticket is a clean HTTP 401.
pub trait TicketVerifier: Send + Sync + 'static {
    fn verify<'a>(&'a self, ticket: &'a str) -> VerifyFuture<'a>;
}

/// The default: no cloud ticket is accepted.
pub struct RejectTickets;

impl TicketVerifier for RejectTickets {
    fn verify<'a>(&'a self, _ticket: &'a str) -> VerifyFuture<'a> {
        Box::pin(async {
            Err("this voice service cannot verify cloud session tickets (no ticket verifier configured)".into())
        })
    }
}

pub const CLOUD_API_URL_ENV: &str = "ALLTERNIT_CLOUD_API_URL";
pub const WORKER_TOKEN_ENV: &str = "ALLTERNIT_VOICE_WORKER_TOKEN";
const CLOUD_TIMEOUT: Duration = Duration::from_secs(5);
/// Waits between usage-report attempts: five attempts over about a minute.
const USAGE_BACKOFF: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(8),
    Duration::from_secs(20),
    Duration::from_secs(30),
];

/// allternit-cloud-api client for the voice worker endpoints.
#[derive(Clone)]
pub struct CloudApi {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl CloudApi {
    pub fn new(base_url: &str, worker_token: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(CLOUD_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            base: base_url.trim_end_matches('/').to_string(),
            token: worker_token.to_string(),
            http,
        }
    }

    /// `Some` only when both env vars are set and non-empty.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var(CLOUD_API_URL_ENV)
            .ok()
            .filter(|v| !v.is_empty())?;
        let token = std::env::var(WORKER_TOKEN_ENV)
            .ok()
            .filter(|v| !v.is_empty())?;
        Some(Self::new(&url, &token))
    }
}

/// Redeems tickets against cloud-api (`POST /api/v1/voice/tickets/redeem`).
pub struct CloudTicketVerifier {
    api: CloudApi,
}

impl CloudTicketVerifier {
    pub fn new(api: CloudApi) -> Self {
        Self { api }
    }
}

impl TicketVerifier for CloudTicketVerifier {
    fn verify<'a>(&'a self, ticket: &'a str) -> VerifyFuture<'a> {
        Box::pin(async move {
            let resp = self
                .api
                .http
                .post(format!("{}/api/v1/voice/tickets/redeem", self.api.base))
                .bearer_auth(&self.api.token)
                .json(&serde_json::json!({ "ticket": ticket }))
                .send()
                .await
                .map_err(|e| {
                    warn!(timeout = e.is_timeout(), "cloud ticket redeem unreachable");
                    "cannot reach Allternit Cloud to verify the session ticket; try again"
                        .to_string()
                })?;
            match resp.status() {
                s if s.is_success() => resp.json::<TicketClaims>().await.map_err(|_| {
                    "Allternit Cloud returned an unreadable ticket response".to_string()
                }),
                StatusCode::UNAUTHORIZED => Err("invalid or expired session ticket".into()),
                s => {
                    warn!(status = %s, "cloud ticket redeem failed");
                    Err("Allternit Cloud could not verify the session ticket; try again".into())
                }
            }
        })
    }
}

/// Reports Cloud Voice minutes (`POST /api/v1/voice/usage`) in the background.
#[derive(Clone)]
pub struct UsageReporter {
    api: CloudApi,
    backoff: Vec<Duration>,
}

impl UsageReporter {
    pub fn new(api: CloudApi) -> Self {
        Self {
            api,
            backoff: USAGE_BACKOFF.to_vec(),
        }
    }

    /// Override the waits between attempts (tests).
    pub fn with_backoff(mut self, backoff: Vec<Duration>) -> Self {
        self.backoff = backoff;
        self
    }

    /// Post usage, retrying with backoff. The cloud side is idempotent on
    /// (engine, sessionId), so a retry after a lost response is safe.
    async fn report(&self, sub: &str, session_id: &str, seconds: u64) {
        let body = serde_json::json!({
            "sub": sub, "sessionId": session_id, "seconds": seconds, "engine": "cloud",
        });
        for attempt in 0..=self.backoff.len() {
            let sent = self
                .api
                .http
                .post(format!("{}/api/v1/voice/usage", self.api.base))
                .bearer_auth(&self.api.token)
                .json(&body)
                .send()
                .await;
            match sent {
                Ok(r) if r.status().is_success() => return,
                Ok(r)
                    if r.status().is_client_error()
                        && r.status() != StatusCode::TOO_MANY_REQUESTS =>
                {
                    warn!(status = %r.status(), "voice usage report rejected; not retrying");
                    return;
                }
                Ok(r) => warn!(status = %r.status(), attempt, "voice usage report failed"),
                Err(_) => warn!(attempt, "voice usage report unreachable"),
            }
            match self.backoff.get(attempt) {
                Some(wait) => tokio::time::sleep(*wait).await,
                None => warn!(%session_id, seconds, "voice usage report dropped after retries"),
            }
        }
    }
}

#[derive(Clone)]
pub struct SessionRouteState {
    pub factory: Arc<dyn EngineFactory>,
    pub token: Option<String>,
    pub verifier: Arc<dyn TicketVerifier>,
    /// Set when ticket sessions must report Cloud Voice minutes.
    pub usage: Option<UsageReporter>,
    pub config: CoreConfig,
}

impl SessionRouteState {
    pub fn new(factory: Arc<dyn EngineFactory>, token: Option<String>) -> Self {
        Self {
            factory,
            token: token.filter(|t| !t.is_empty()),
            verifier: Arc::new(RejectTickets),
            usage: None,
            config: CoreConfig::default(),
        }
    }

    /// Verify tickets and report usage via cloud-api when
    /// `ALLTERNIT_CLOUD_API_URL` and `ALLTERNIT_VOICE_WORKER_TOKEN` are set;
    /// otherwise tickets stay rejected.
    pub fn with_cloud_from_env(self) -> Self {
        match CloudApi::from_env() {
            Some(api) => self.with_cloud(api),
            None => self,
        }
    }

    pub fn with_cloud(mut self, api: CloudApi) -> Self {
        self.verifier = Arc::new(CloudTicketVerifier::new(api.clone()));
        self.usage = Some(UsageReporter::new(api));
        self
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
    let claims = match authorize(&state, &params, peer.map(|p| p.0)).await {
        Ok(claims) => claims,
        Err(message) => {
            warn!(%message, "voice session rejected");
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "unauthorized", "message": message })),
            )
                .into_response();
        }
    };
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| serve(socket, state, claims))
}

/// `Ok(Some(claims))` for a cloud ticket, `Ok(None)` for token/loopback.
async fn authorize(
    state: &SessionRouteState,
    p: &AuthParams,
    peer: Option<SocketAddr>,
) -> Result<Option<TicketClaims>, String> {
    if let Some(ticket) = p.ticket.as_deref() {
        return state.verifier.verify(ticket).await.map(Some);
    }
    authorize_local(state, p, peer).map(|()| None)
}

fn authorize_local(
    state: &SessionRouteState,
    p: &AuthParams,
    peer: Option<SocketAddr>,
) -> Result<(), String> {
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

async fn serve(socket: WebSocket, state: SessionRouteState, claims: Option<TicketClaims>) {
    let started = Instant::now();
    let handle = VoiceSession::spawn(state.factory.clone(), state.config.clone());
    let input = handle.input;
    let mut output = handle.output;
    let (mut sink, mut stream) = socket.split();
    let cloud = claims.is_some();
    let limit = claims.as_ref().map(|c| Duration::from_secs(c.max_seconds));
    let limit_hit = Arc::new(tokio::sync::Notify::new());
    let limit_signal = limit_hit.clone();
    let session_id = Arc::new(std::sync::Mutex::new(None::<String>));
    let session_id_w = session_id.clone();

    let mut writer = tokio::spawn(async move {
        let deadline = limit.map(|l| started + l);
        loop {
            let out = tokio::select! {
                out = output.recv() => match out { Some(o) => o, None => break },
                _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                    let ev = ServerEvent::error(
                        codes::SESSION_LIMIT,
                        "this session reached the time limit of your plan",
                        true,
                    );
                    if let Ok(json) = serde_json::to_string(&ev) {
                        let _ = sink.send(Message::Text(json)).await;
                    }
                    let _ = sink.send(Message::Close(None)).await;
                    limit_signal.notify_one();
                    break;
                }
            };
            let msg = match out {
                SessionOutput::Event(mut ev) => {
                    if let ServerEvent::SessionReady {
                        engine, session_id, ..
                    } = &mut ev
                    {
                        if cloud {
                            *engine = EngineKind::Cloud;
                        }
                        *session_id_w.lock().unwrap() = Some(session_id.clone());
                    }
                    match serde_json::to_string(&ev) {
                        Ok(json) => Message::Text(json),
                        Err(e) => {
                            warn!("serialise event: {e}");
                            continue;
                        }
                    }
                }
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

    let mut writer_done = false;
    loop {
        let msg = tokio::select! {
            m = stream.next() => match m { Some(Ok(m)) => m, _ => break },
            _ = limit_hit.notified() => break,
            _ = &mut writer => { writer_done = true; break } // session ended server-side
        };
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
    if !writer_done {
        let _ = writer.await;
    }
    debug!("voice session socket closed");

    // Cloud Voice minutes: only ticket sessions, only if the session got as
    // far as `session.ready` (that is what has an id to be idempotent on).
    if let (Some(claims), Some(reporter)) = (claims, state.usage) {
        let id = session_id.lock().unwrap().take();
        if let Some(id) = id {
            let seconds = started.elapsed().as_secs_f64().ceil().max(1.0) as u64;
            tokio::spawn(async move { reporter.report(&claims.sub, &id, seconds).await });
        }
    }
}
