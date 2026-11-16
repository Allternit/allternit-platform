//! Model hosts: a device (phase 1: an Allternit browser tab with WebGPU)
//! serves a small model to its owner's agents through this relay.
//!
//! The device mints a ticket, opens a WebSocket out to cloud-api and says
//! which models it serves. Gizzi (or anything else holding the owner's
//! credential) then calls an OpenAI-compatible surface here, and each chat
//! request is forwarded to the device as a `request` frame; the device's
//! `chunk` frames stream back as SSE or are folded into one completion.
//!
//! Routes:
//! - `POST /api/v1/model-hosts/ws-ticket` (Clerk session or `allternit_*`
//!   token): `{host_id, name}` → `{ticket, ws_url}`. Single-use, 60 s,
//!   bound to user + host id + name.
//! - `GET /api/v1/model-hosts/connect?ticket=…`: the device's WebSocket.
//! - `GET /api/v1/model-hosts` (user auth): the caller's online hosts.
//! - `GET /api/v1/model-hosts/v1/models` and
//!   `POST /api/v1/model-hosts/v1/chat/completions` (user auth or a paired
//!   runtime device token, acting as its owner): model ids are
//!   `<host_id>/<model_id>`; a caller only ever sees and calls its own hosts.
//!
//! Frames (JSON text). Device → cloud: `hello {models}` (first, within 10 s),
//! `chunk {request_id, data}`, `end {request_id}`, `error {request_id,
//! message}`, `models {models}`, `ping`, `pong`. Cloud → device: `ready
//! {host_id}`, `request {request_id, body}`, `cancel {request_id}`, `ping`
//! (every 25 s), `pong`.
//!
//! Everything lives in memory (same single-instance assumption as
//! `runtime_relay`'s hub). One connection per `(user_id, host_id)`: a new one
//! replaces the old, which is closed and fails its in-flight requests.
//! Docs: surfaces/docs/api/model-host-relay.mdx.

use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{Message, WebSocket},
        DefaultBodyLimit, Query, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures_util::{stream, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex as StdMutex, OnceLock, RwLock as StdRwLock},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use super::runtime_viewer::ws_base_from;
use crate::{auth, ApiError, ApiState};

const TICKET_TTL: Duration = Duration::from_secs(60);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_INTERVAL: Duration = Duration::from_secs(25);
/// A device that has sent nothing (not even a pong) for this long is gone;
/// a backgrounded phone tab stops answering without closing its socket.
const HOST_SILENCE_LIMIT: Duration = Duration::from_secs(75);
/// No first frame from the device within this → 504. Also the longest gap
/// allowed between later frames.
const FRAME_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_HOSTS_PER_USER: usize = 8;
const MAX_MODELS_PER_HOST: usize = 16;
const MAX_NAME_CHARS: usize = 80;
/// Same scope as the rest of the inference surface for `allternit_*` tokens.
const SCOPE: &str = "inference";
const WS_BASE_ENV: &str = "ALLTERNIT_MODEL_HOST_WS_BASE";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/model-hosts", get(list_hosts))
        .route("/api/v1/model-hosts/ws-ticket", post(mint_ticket))
        .route("/api/v1/model-hosts/connect", get(connect_host))
        .route("/api/v1/model-hosts/v1/models", get(list_models))
        .route(
            "/api/v1/model-hosts/v1/chat/completions",
            post(chat_completions).layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct HostModel {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context_length: Option<u64>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum HostFrame {
    Hello {
        #[serde(default)]
        models: Vec<HostModel>,
    },
    Chunk {
        request_id: String,
        data: Value,
    },
    End {
        request_id: String,
    },
    Error {
        request_id: String,
        #[serde(default)]
        message: String,
    },
    Models {
        #[serde(default)]
        models: Vec<HostModel>,
    },
    Ping,
    Pong,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CloudFrame {
    Ready { host_id: String },
    Request { request_id: String, body: Value },
    Cancel { request_id: String },
    Ping,
    Pong,
}

/// What the host socket sends; `Close` is how a replacing connection ends
/// the old one.
enum Outbound {
    Frame(CloudFrame),
    Close,
}

/// One request's view of the device's answer.
#[derive(Debug)]
enum HostEvent {
    Chunk(Value),
    End,
    Error(String),
}

struct HostConnection {
    user_id: String,
    host_id: String,
    name: String,
    connected_at: DateTime<Utc>,
    models: StdRwLock<Vec<HostModel>>,
    sender: mpsc::UnboundedSender<Outbound>,
    /// Sync mutex so a dropped response body can clean up in `Drop`.
    pending: StdMutex<HashMap<String, mpsc::UnboundedSender<HostEvent>>>,
}

impl HostConnection {
    fn models(&self) -> Vec<HostModel> {
        self.models.read().map(|m| m.clone()).unwrap_or_default()
    }

    fn serves(&self, model_id: &str) -> bool {
        self.models().iter().any(|m| m.id == model_id)
    }

    fn take_pending(&self, request_id: &str) -> Option<mpsc::UnboundedSender<HostEvent>> {
        self.pending.lock().ok()?.remove(request_id)
    }

    fn pending_sender(&self, request_id: &str) -> Option<mpsc::UnboundedSender<HostEvent>> {
        self.pending.lock().ok()?.get(request_id).cloned()
    }
}

type HostKey = (String, String);

fn host_hub() -> &'static RwLock<HashMap<HostKey, Arc<HostConnection>>> {
    static HUB: OnceLock<RwLock<HashMap<HostKey, Arc<HostConnection>>>> = OnceLock::new();
    HUB.get_or_init(|| RwLock::new(HashMap::new()))
}

/// The user's online hosts, oldest first.
async fn hosts_for(user_id: &str) -> Vec<Arc<HostConnection>> {
    let mut hosts: Vec<_> = host_hub()
        .read()
        .await
        .values()
        .filter(|c| c.user_id == user_id)
        .cloned()
        .collect();
    hosts.sort_by(|a, b| a.connected_at.cmp(&b.connected_at).then(a.host_id.cmp(&b.host_id)));
    hosts
}

/// Whether adding `host_id` would take the user past the per-user cap
/// (reconnecting an existing host id never counts).
async fn over_host_cap(user_id: &str, host_id: &str) -> bool {
    let hub = host_hub().read().await;
    let others = hub.keys().filter(|(u, h)| u == user_id && h != host_id).count();
    others >= MAX_HOSTS_PER_USER
}

// ---- Validation -----------------------------------------------------------

/// `host_id`: 1–64 of `[A-Za-z0-9_-]`.
pub(crate) fn valid_host_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Model ids are what the device reports (`lfm2.5-350m`): 1–64 of
/// `[A-Za-z0-9._:-]`, never a `/` (it separates host and model).
pub(crate) fn valid_model_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

/// Split a public model id `<host_id>/<model_id>`.
pub(crate) fn parse_model_ref(model: &str) -> Option<(&str, &str)> {
    let (host, model) = model.split_once('/')?;
    (valid_host_id(host) && valid_model_id(model)).then_some((host, model))
}

/// Keep the valid, distinct models a device reported (capped), with a name.
pub(crate) fn sanitize_models(models: Vec<HostModel>) -> Vec<HostModel> {
    let mut out: Vec<HostModel> = Vec::new();
    for mut model in models {
        if !valid_model_id(&model.id) || out.iter().any(|m| m.id == model.id) {
            continue;
        }
        let name = model.name.trim();
        model.name = if name.is_empty() { model.id.clone() } else { name.chars().take(MAX_NAME_CHARS).collect() };
        out.push(model);
        if out.len() == MAX_MODELS_PER_HOST {
            break;
        }
    }
    out
}

pub(crate) fn parse_host_frame(text: &str) -> Option<HostFrame> {
    serde_json::from_str(text).ok()
}

// ---- Tickets --------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct HostTicket {
    pub(crate) user_id: String,
    pub(crate) host_id: String,
    pub(crate) name: String,
    expires_at: Instant,
}

fn tickets() -> &'static StdMutex<HashMap<String, HostTicket>> {
    static TICKETS: OnceLock<StdMutex<HashMap<String, HostTicket>>> = OnceLock::new();
    TICKETS.get_or_init(|| StdMutex::new(HashMap::new()))
}

pub(crate) fn issue_ticket(user_id: &str, host_id: &str, name: &str, now: Instant) -> String {
    let ticket = Uuid::new_v4().simple().to_string();
    let mut map = tickets().lock().unwrap_or_else(|e| e.into_inner());
    map.retain(|_, t| t.expires_at > now);
    map.insert(
        ticket.clone(),
        HostTicket {
            user_id: user_id.to_string(),
            host_id: host_id.to_string(),
            name: name.to_string(),
            expires_at: now + TICKET_TTL,
        },
    );
    ticket
}

/// Consume a ticket: `None` when unknown, already used or expired.
pub(crate) fn take_ticket(ticket: &str, now: Instant) -> Option<HostTicket> {
    tickets()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(ticket)
        .filter(|t| t.expires_at > now)
}

// ---- Responses ------------------------------------------------------------

fn openai_error(status: StatusCode, kind: &str, message: &str) -> Response {
    (status, Json(json!({ "error": { "message": message, "type": kind } }))).into_response()
}

/// Fold a non-streamed answer's `chat.completion.chunk`s into one
/// `chat.completion`: content concatenated, the last `finish_reason`, and
/// `usage` from the last chunk that carries it.
pub(crate) fn aggregate_chunks(model: &str, chunks: &[Value], now_unix: i64) -> Value {
    let mut content = String::new();
    let mut finish_reason = Value::Null;
    let mut usage = Value::Null;
    for chunk in chunks {
        let choice = &chunk["choices"][0];
        if let Some(text) = choice["delta"]["content"].as_str() {
            content.push_str(text);
        }
        if !choice["finish_reason"].is_null() {
            finish_reason = choice["finish_reason"].clone();
        }
        if chunk.get("usage").is_some_and(|u| !u.is_null()) {
            usage = chunk["usage"].clone();
        }
    }
    let first = chunks.first();
    let id = first
        .and_then(|c| c["id"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("chatcmpl-{}", Uuid::new_v4().simple()));
    let created = first.and_then(|c| c["created"].as_i64()).unwrap_or(now_unix);
    let mut completion = json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": finish_reason,
        }],
    });
    if !usage.is_null() {
        completion["usage"] = usage;
    }
    completion
}

// ---- Auth -----------------------------------------------------------------

/// Clerk session or `allternit_*` token, else a paired runtime's device
/// token acting as its owner (same fallback as `/api/v1/me/usage`).
async fn caller_user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    match auth::resolve_user_scoped(&state.db, headers, SCOPE).await {
        Ok(user) => Ok(user.id),
        Err(primary) => {
            let Some(token) = device_token_from_headers(headers) else {
                return Err(primary);
            };
            Ok(runtime_device_for_token(&state.db, token, None).await?.user_id)
        }
    }
}

// ---- HTTP handlers --------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TicketRequest {
    host_id: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct TicketQuery {
    ticket: String,
}

async fn mint_ticket(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<TicketRequest>,
) -> Result<Response, ApiError> {
    let user_id = auth::resolve_user_scoped(&state.db, &headers, SCOPE).await?.id;
    if !valid_host_id(&body.host_id) {
        return Err(ApiError::BadRequest(
            "host_id must be 1-64 letters, digits, '_' or '-'.".to_string(),
        ));
    }
    let name = body.name.trim();
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::BadRequest(format!("name is {MAX_NAME_CHARS} characters max.")));
    }
    let name = if name.is_empty() { "Device" } else { name };
    if over_host_cap(&user_id, &body.host_id).await {
        return Err(ApiError::Conflict(format!(
            "You already have {MAX_HOSTS_PER_USER} devices serving models."
        )));
    }
    let Some(base) = ws_base_from(WS_BASE_ENV, &headers) else {
        return Err(ApiError::BadRequest(
            "Cannot work out the public address for the model host socket.".to_string(),
        ));
    };
    let ticket = issue_ticket(&user_id, &body.host_id, name, Instant::now());
    Ok(Json(json!({
        "ticket": ticket,
        "ws_url": format!("{base}/api/v1/model-hosts/connect?ticket={}", urlencoding::encode(&ticket)),
    }))
    .into_response())
}

async fn list_hosts(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user_id = auth::resolve_user_scoped(&state.db, &headers, SCOPE).await?.id;
    let hosts: Vec<Value> = hosts_for(&user_id)
        .await
        .iter()
        .map(|c| {
            json!({
                "host_id": c.host_id,
                "name": c.name,
                "models": c.models(),
                "connected_at": c.connected_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(Json(json!({ "hosts": hosts })))
}

async fn list_models(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let mut data = Vec::new();
    for host in hosts_for(&user_id).await {
        for model in host.models() {
            data.push(json!({
                "id": format!("{}/{}", host.host_id, model.id),
                "object": "model",
                "owned_by": host.name,
                "name": model.name,
                "context_length": model.context_length,
            }));
        }
    }
    Ok(Json(json!({ "object": "list", "data": data })))
}

/// Cancels the device-side request when the caller goes away (the response
/// body or the handler future is dropped) before the device finished.
struct RequestGuard {
    connection: Arc<HostConnection>,
    request_id: String,
    finished: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.connection.take_pending(&self.request_id).is_some() {
            let _ = self.connection.sender.send(Outbound::Frame(CloudFrame::Cancel {
                request_id: self.request_id.clone(),
            }));
        }
    }
}

async fn next_event(rx: &mut mpsc::UnboundedReceiver<HostEvent>) -> Result<HostEvent, StatusCode> {
    match tokio::time::timeout(FRAME_TIMEOUT, rx.recv()).await {
        Ok(Some(event)) => Ok(event),
        Ok(None) => Err(StatusCode::BAD_GATEWAY),
        Err(_) => Err(StatusCode::GATEWAY_TIMEOUT),
    }
}

fn wait_error(status: StatusCode) -> Response {
    if status == StatusCode::GATEWAY_TIMEOUT {
        openai_error(status, "timeout", "The device did not answer in time.")
    } else {
        openai_error(status, "host_disconnected", "The device disconnected.")
    }
}

async fn chat_completions(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let mut request = match serde_json::from_slice::<Value>(&body) {
        Ok(value @ Value::Object(_)) => value,
        _ => {
            return Ok(openai_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "The body must be a JSON chat completion request.",
            ))
        }
    };
    let requested = request["model"].as_str().unwrap_or_default().to_string();
    let not_found = || {
        openai_error(
            StatusCode::NOT_FOUND,
            "model_not_found",
            &format!("The model '{requested}' is not on any of your online devices."),
        )
    };
    let Some((host_id, model_id)) = parse_model_ref(&requested) else {
        return Ok(not_found());
    };
    let connection = host_hub()
        .read()
        .await
        .get(&(user_id, host_id.to_string()))
        .cloned()
        .filter(|c| c.serves(model_id));
    let Some(connection) = connection else {
        return Ok(not_found());
    };
    request["model"] = Value::String(model_id.to_string());
    let streaming = request["stream"].as_bool().unwrap_or(false);

    let request_id = Uuid::new_v4().to_string();
    let (events, mut rx) = mpsc::unbounded_channel();
    if let Ok(mut pending) = connection.pending.lock() {
        pending.insert(request_id.clone(), events);
    }
    let mut guard = RequestGuard {
        connection: connection.clone(),
        request_id: request_id.clone(),
        finished: false,
    };
    let sent = connection.sender.send(Outbound::Frame(CloudFrame::Request {
        request_id,
        body: request,
    }));
    if sent.is_err() {
        return Ok(not_found());
    }

    let first = match next_event(&mut rx).await {
        Ok(event) => event,
        Err(status) => return Ok(wait_error(status)),
    };

    if streaming {
        let state = SseState { first: Some(first), rx, guard, done: false };
        let body = stream::unfold(state, |mut s| async move {
            if s.done {
                return None;
            }
            let event = match s.first.take() {
                Some(event) => Ok(event),
                None => next_event(&mut s.rx).await,
            };
            let text = match event {
                Ok(HostEvent::Chunk(data)) => format!("data: {data}\n\n"),
                Ok(HostEvent::End) => {
                    s.finish();
                    "data: [DONE]\n\n".to_string()
                }
                Ok(HostEvent::Error(message)) => {
                    s.finish();
                    sse_error(&message)
                }
                Err(status) => {
                    s.done = true; // the guard cancels on drop
                    sse_error(if status == StatusCode::GATEWAY_TIMEOUT {
                        "The device did not answer in time."
                    } else {
                        "The device disconnected."
                    })
                }
            };
            Some((Ok::<Bytes, Infallible>(Bytes::from(text)), s))
        });
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header("x-accel-buffering", "no")
            .body(Body::from_stream(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()));
    }

    let mut chunks = Vec::new();
    let mut event = first;
    loop {
        match event {
            HostEvent::Chunk(data) => chunks.push(data),
            HostEvent::End => break,
            HostEvent::Error(message) => {
                guard.finished = true;
                return Ok(openai_error(StatusCode::BAD_GATEWAY, "host_error", &message));
            }
        }
        event = match next_event(&mut rx).await {
            Ok(event) => event,
            Err(status) => return Ok(wait_error(status)),
        };
    }
    guard.finished = true;
    Ok(Json(aggregate_chunks(&requested, &chunks, Utc::now().timestamp())).into_response())
}

struct SseState {
    first: Option<HostEvent>,
    rx: mpsc::UnboundedReceiver<HostEvent>,
    guard: RequestGuard,
    done: bool,
}

impl SseState {
    fn finish(&mut self) {
        self.done = true;
        self.guard.finished = true;
    }
}

fn sse_error(message: &str) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n", json!({ "error": { "message": message } }))
}

// ---- Host WebSocket -------------------------------------------------------

async fn connect_host(ws: WebSocketUpgrade, Query(query): Query<TicketQuery>) -> Result<Response, ApiError> {
    let ticket = take_ticket(&query.ticket, Instant::now())
        .ok_or_else(|| ApiError::Unauthorized("Invalid or expired model host ticket".to_string()))?;
    Ok(ws
        .max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| host_socket(socket, ticket)))
}

fn send_text(frame: &CloudFrame) -> Option<Message> {
    serde_json::to_string(frame).ok().map(Message::Text)
}

async fn host_socket(socket: WebSocket, ticket: HostTicket) {
    let (mut sink, mut stream) = socket.split();
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => parse_host_frame(&text),
        _ => None,
    };
    let Some(HostFrame::Hello { models }) = hello else {
        let _ = sink.send(Message::Close(None)).await;
        return;
    };
    if over_host_cap(&ticket.user_id, &ticket.host_id).await {
        let _ = sink.send(Message::Close(None)).await;
        return;
    }

    let (sender, mut outgoing) = mpsc::unbounded_channel();
    let connection = Arc::new(HostConnection {
        user_id: ticket.user_id.clone(),
        host_id: ticket.host_id.clone(),
        name: ticket.name.clone(),
        connected_at: Utc::now(),
        models: StdRwLock::new(sanitize_models(models)),
        sender,
        pending: StdMutex::new(HashMap::new()),
    });
    let key = (ticket.user_id.clone(), ticket.host_id.clone());
    let replaced = host_hub().write().await.insert(key.clone(), connection.clone());
    if let Some(old) = replaced {
        let _ = old.sender.send(Outbound::Close);
    }

    let ready = CloudFrame::Ready { host_id: ticket.host_id.clone() };
    let mut alive = match send_text(&ready) {
        Some(message) => sink.send(message).await.is_ok(),
        None => false,
    };
    let mut last_heard = Instant::now();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await; // the first tick is immediate
    while alive {
        tokio::select! {
            outbound = outgoing.recv() => {
                match outbound {
                    Some(Outbound::Frame(frame)) => {
                        if let Some(message) = send_text(&frame) {
                            alive = sink.send(message).await.is_ok();
                        }
                    }
                    Some(Outbound::Close) | None => {
                        let _ = sink.send(Message::Close(None)).await;
                        alive = false;
                    }
                }
            }
            inbound = stream.next() => {
                last_heard = Instant::now();
                match inbound {
                    Some(Ok(Message::Text(text))) => match parse_host_frame(&text) {
                        Some(HostFrame::Chunk { request_id, data }) => {
                            if let Some(events) = connection.pending_sender(&request_id) {
                                let _ = events.send(HostEvent::Chunk(data));
                            }
                        }
                        Some(HostFrame::End { request_id }) => {
                            if let Some(events) = connection.take_pending(&request_id) {
                                let _ = events.send(HostEvent::End);
                            }
                        }
                        Some(HostFrame::Error { request_id, message }) => {
                            if let Some(events) = connection.take_pending(&request_id) {
                                let message = if message.trim().is_empty() { "The device could not answer.".to_string() } else { message };
                                let _ = events.send(HostEvent::Error(message));
                            }
                        }
                        Some(HostFrame::Models { models }) => {
                            if let Ok(mut current) = connection.models.write() {
                                *current = sanitize_models(models);
                            }
                        }
                        Some(HostFrame::Ping) => {
                            if let Some(message) = send_text(&CloudFrame::Pong) {
                                alive = sink.send(message).await.is_ok();
                            }
                        }
                        Some(HostFrame::Pong) | Some(HostFrame::Hello { .. }) | None => {}
                    },
                    Some(Ok(Message::Ping(body))) => {
                        alive = sink.send(Message::Pong(body)).await.is_ok();
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => alive = false,
                    Some(Ok(_)) => {}
                }
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > HOST_SILENCE_LIMIT {
                    let _ = sink.send(Message::Close(None)).await;
                    alive = false;
                } else if let Some(message) = send_text(&CloudFrame::Ping) {
                    alive = sink.send(message).await.is_ok();
                }
            }
        }
    }

    {
        let mut hub = host_hub().write().await;
        if hub.get(&key).is_some_and(|c| Arc::ptr_eq(c, &connection)) {
            hub.remove(&key);
        }
    }
    let pending = connection
        .pending
        .lock()
        .map(|mut p| std::mem::take(&mut *p))
        .unwrap_or_default();
    for (_, events) in pending {
        let _ = events.send(HostEvent::Error("The device disconnected.".to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_refs_split_on_the_first_slash_and_validate_both_halves() {
        assert_eq!(parse_model_ref("h_1-a/lfm2.5-350m"), Some(("h_1-a", "lfm2.5-350m")));
        assert_eq!(parse_model_ref("lfm2.5-350m"), None);
        assert_eq!(parse_model_ref("/lfm"), None);
        assert_eq!(parse_model_ref("host/"), None);
        assert_eq!(parse_model_ref("host/a/b"), None);
        assert_eq!(parse_model_ref("ho st/lfm"), None);
        assert_eq!(parse_model_ref(&format!("{}/m", "h".repeat(65))), None);
        assert!(valid_host_id(&"h".repeat(64)));
    }

    #[test]
    fn tickets_are_single_use_and_expire() {
        let now = Instant::now();
        let ticket = issue_ticket("user_a", "host_a", "iPhone", now);
        let taken = take_ticket(&ticket, now + Duration::from_secs(59)).expect("valid ticket");
        assert_eq!((taken.user_id.as_str(), taken.host_id.as_str(), taken.name.as_str()), ("user_a", "host_a", "iPhone"));
        assert!(take_ticket(&ticket, now).is_none(), "second use");

        let late = issue_ticket("user_a", "host_a", "iPhone", now);
        assert!(take_ticket(&late, now + TICKET_TTL).is_none(), "expired at 60 s");
        assert!(take_ticket("nope", now).is_none());
    }

    #[test]
    fn host_frames_parse_and_unknown_or_malformed_ones_do_not() {
        assert_eq!(
            parse_host_frame(r#"{"type":"hello","models":[{"id":"lfm2.5-350m","name":"LFM2.5 350M (on this phone)","context_length":8192}]}"#),
            Some(HostFrame::Hello {
                models: vec![HostModel { id: "lfm2.5-350m".into(), name: "LFM2.5 350M (on this phone)".into(), context_length: Some(8192) }],
            })
        );
        assert_eq!(parse_host_frame(r#"{"type":"hello"}"#), Some(HostFrame::Hello { models: vec![] }));
        assert_eq!(
            parse_host_frame(r#"{"type":"chunk","request_id":"r1","data":{"x":1}}"#),
            Some(HostFrame::Chunk { request_id: "r1".into(), data: json!({"x":1}) })
        );
        assert_eq!(parse_host_frame(r#"{"type":"end","request_id":"r1"}"#), Some(HostFrame::End { request_id: "r1".into() }));
        assert_eq!(
            parse_host_frame(r#"{"type":"error","request_id":"r1","message":"oom"}"#),
            Some(HostFrame::Error { request_id: "r1".into(), message: "oom".into() })
        );
        assert_eq!(parse_host_frame(r#"{"type":"ping"}"#), Some(HostFrame::Ping));
        assert_eq!(parse_host_frame(r#"{"type":"chunk","data":{}}"#), None);
        assert_eq!(parse_host_frame(r#"{"type":"socket_open"}"#), None);
        assert_eq!(parse_host_frame("not json"), None);
        assert_eq!(
            serde_json::to_value(CloudFrame::Request { request_id: "r1".into(), body: json!({"model":"m"}) }).unwrap(),
            json!({"type":"request","request_id":"r1","body":{"model":"m"}})
        );
    }

    #[test]
    fn reported_models_are_validated_deduped_named_and_capped() {
        let m = |id: &str, name: &str| HostModel { id: id.into(), name: name.into(), context_length: None };
        let out = sanitize_models(vec![m("a", ""), m("a", "dup"), m("bad/id", "x"), m("b", &"n".repeat(100))]);
        assert_eq!(out.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(out[0].name, "a");
        assert_eq!(out[1].name.chars().count(), MAX_NAME_CHARS);
        let many: Vec<_> = (0..40).map(|i| m(&format!("m{i}"), "")).collect();
        assert_eq!(sanitize_models(many).len(), MAX_MODELS_PER_HOST);
    }

    #[test]
    fn chunks_fold_into_one_completion() {
        let chunk = |delta: Value, finish: Value| {
            json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":100,"model":"lfm2.5-350m",
                   "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
        };
        let mut last = chunk(json!({}), json!("length"));
        last["usage"] = json!({"prompt_tokens":5,"completion_tokens":3,"total_tokens":8});
        let chunks = vec![
            chunk(json!({"role":"assistant","content":"Hel"}), Value::Null),
            chunk(json!({"content":"lo"}), Value::Null),
            last,
        ];
        let out = aggregate_chunks("host_a/lfm2.5-350m", &chunks, 999);
        assert_eq!(
            out,
            json!({
                "id": "chatcmpl-1", "object": "chat.completion", "created": 100, "model": "host_a/lfm2.5-350m",
                "choices": [{"index":0,"message":{"role":"assistant","content":"Hello"},"finish_reason":"length"}],
                "usage": {"prompt_tokens":5,"completion_tokens":3,"total_tokens":8},
            })
        );

        let empty = aggregate_chunks("h/m", &[], 999);
        assert_eq!(empty["created"], 999);
        assert_eq!(empty["choices"][0]["message"]["content"], "");
        assert!(empty.get("usage").is_none());
    }
}
