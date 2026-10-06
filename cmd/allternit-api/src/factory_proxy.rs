//! `/api/factory/*` → the Factory engine (`allternit-factory serve`).
//!
//! Desktop, web and phone call allternit-api; this forwards their Factory calls
//! to the engine on loopback, behind the same auth as every other protected
//! route. Approvals (`/api/factory/approvals*`) are NOT proxied: they're static
//! routes in `factory_approvals`, which win over this wildcard (a nest or
//! fallback at `/api/factory` would collide with them).
//!
//! - The engine is `$ALLTERNIT_FACTORY_URL`, else `http://127.0.0.1:3011`.
//! - The caller's `Authorization` (or Desktop's access-token + user-id pair) goes through, plus `x-allternit-user` (the
//!   signed-in user) and `x-allternit-api-base` (this process), so the
//!   engine's hosted / vendor / channel sends call back here as that user.
//! - Engine down → `502 {error:{code:'transport'}}`; engine slow → `504`.
//!   Never a silent fallback.
//! - `/api/factory/agents/:id/stream` (the pane mirror SSE) is passed
//!   through as it arrives, with no request timeout.
//! - `/api/factory/events` merges the engine's SSE with this process's
//!   approval events (`factory_approvals::subscribe()`). Engine events keep the
//!   engine's ledger id. An approval event has no engine id, so its SSE id is
//!   `<last engine id>~apr-<approvalId>`: on reconnect the engine part resumes
//!   the engine replay exactly; approvals are not replayed (the app re-reads
//!   `GET /api/factory/approvals?state=pending` when it reconnects).

use std::convert::Infallible;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Extension, Json, Router};
use futures::StreamExt;
use serde_json::json;

use crate::auth::AuthUser;
use crate::AppState;

/// Default engine address (API.md §1).
pub const DEFAULT_ENGINE_URL: &str = "http://127.0.0.1:3011";

static SELF_BASE: OnceLock<String> = OnceLock::new();

/// Record this process's own loopback address (set once at startup), so the
/// engine knows where to call back for hosted / vendor / channel sends.
pub fn set_self_base(base: String) {
    let _ = SELF_BASE.set(base);
}

pub fn engine_url() -> String {
    std::env::var("ALLTERNIT_FACTORY_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENGINE_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Mounted under `/api` next to `factory_approvals::router()`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/factory/events", get(events))
        .route("/factory/*rest", any(proxy))
}

fn transport(fact: String) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "error": { "code": "transport", "fact": fact,
            "action": "Start the Factory engine (allternit-factory serve; Allternit Desktop starts it) and retry." } })),
    )
        .into_response()
}

fn client(timeout: Option<Duration>) -> reqwest::Client {
    let mut b = reqwest::Client::builder().connect_timeout(Duration::from_secs(3));
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    b.build().unwrap_or_else(|_| reqwest::Client::new())
}

/// Headers forwarded to the engine.
fn forward_headers(incoming: &HeaderMap, user: &AuthUser) -> reqwest::header::HeaderMap {
    let mut out = reqwest::header::HeaderMap::new();
    // The desktop pair is how Desktop callers authenticate; the engine needs it
    // to call back here as them (sends, team registration).
    for name in [
        "authorization",
        "content-type",
        "accept",
        "idempotency-key",
        "x-allternit-desktop-access-token",
        "x-allternit-user-id",
    ] {
        if let Some(v) = incoming.get(name) {
            if let Ok(v) = reqwest::header::HeaderValue::from_bytes(v.as_bytes()) {
                out.insert(name, v);
            }
        }
    }
    if let Ok(v) = reqwest::header::HeaderValue::from_str(&user.user_id) {
        out.insert("x-allternit-user", v);
    }
    if let Some(base) = SELF_BASE.get() {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(base) {
            out.insert("x-allternit-api-base", v);
        }
    }
    out
}

async fn proxy(
    State(_state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(rest): Path<String>,
    RawQuery(query): RawQuery,
    req: Request,
) -> Response {
    let method = req.method().clone();
    let headers = req.headers().clone();
    let body = match axum::body::to_bytes(req.into_body(), 8 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": { "code": "usage", "fact": format!("request body: {e}"), "action": "Send a JSON body under 8 MB." } })),
            )
                .into_response()
        }
    };
    let base = engine_url();
    let url = match &query {
        Some(q) if !q.is_empty() => format!("{base}/api/factory/{rest}?{q}"),
        _ => format!("{base}/api/factory/{rest}"),
    };
    let Ok(rmethod) = reqwest::Method::from_bytes(method.as_str().as_bytes()) else {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    };
    // A pane mirror stream (`/agents/:id/stream`, SSE) stays open as long as
    // the tile does: no request timeout, and the body is passed through as it
    // arrives instead of buffered.
    let streaming = method == Method::GET && is_stream_path(&rest);
    let timeout = (!streaming).then(|| Duration::from_secs(60));
    let mut rb = client(timeout).request(rmethod, &url).headers(forward_headers(&headers, &user));
    if method != Method::GET && method != Method::HEAD {
        rb = rb.body(body);
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({ "error": { "code": "timeout", "fact": format!("the Factory engine at {base} did not answer in time"), "action": "Retry, or check the engine." } })),
            )
                .into_response()
        }
        Err(e) => return transport(format!("the Factory engine at {base} is not reachable: {e}")),
    };
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_string);
    if streaming && content_type.as_deref().is_some_and(|c| c.starts_with("text/event-stream")) {
        let mut out = Response::new(Body::from_stream(resp.bytes_stream()));
        *out.status_mut() = status;
        out.headers_mut().insert("content-type", HeaderValue::from_static("text/event-stream"));
        out.headers_mut().insert("cache-control", HeaderValue::from_static("no-cache"));
        return out;
    }
    let bytes: Bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return transport(format!("the Factory engine at {base} dropped the response: {e}")),
    };
    let mut out = Response::new(Body::from(bytes));
    *out.status_mut() = status;
    if let Some(ct) = content_type.and_then(|c| HeaderValue::from_str(&c).ok()) {
        out.headers_mut().insert("content-type", ct);
    }
    out
}

/// Engine routes that answer with a long-lived SSE body (besides `/events`,
/// which is merged with approvals above): the pane mirror.
pub fn is_stream_path(rest: &str) -> bool {
    let parts: Vec<&str> = rest.trim_matches('/').split('/').collect();
    matches!(parts.as_slice(), ["agents", id, "stream"] if !id.is_empty())
}

/// One SSE frame from the engine.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Frame {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

/// Split complete SSE frames off the front of `buf` (leaving any partial one).
pub fn take_frames(buf: &mut String) -> Vec<Frame> {
    let mut frames = Vec::new();
    loop {
        let norm = buf.replace("\r\n", "\n");
        let Some(end) = norm.find("\n\n") else {
            *buf = norm;
            return frames;
        };
        let block = norm[..end].to_string();
        *buf = norm[end + 2..].to_string();
        let mut f = Frame::default();
        let mut data = Vec::new();
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("id:") {
                f.id = Some(v.trim_start().to_string());
            } else if let Some(v) = line.strip_prefix("event:") {
                f.event = Some(v.trim_start().to_string());
            } else if let Some(v) = line.strip_prefix("data:") {
                data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
            }
        }
        f.data = data.join("\n");
        if f.event.is_some() || !f.data.is_empty() {
            frames.push(f);
        }
    }
}

/// The engine part of a merged SSE id (`<engine id>~apr-…` → `<engine id>`).
pub fn engine_cursor(last_event_id: &str) -> Option<String> {
    let engine = last_event_id.split('~').next().unwrap_or("");
    (!engine.is_empty() && !engine.starts_with("apr-")).then(|| engine.to_string())
}

async fn events(
    State(_state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let base = engine_url();
    let last = headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(engine_cursor);
    let url = match &query {
        Some(q) if !q.is_empty() => format!("{base}/api/factory/events?{q}"),
        _ => format!("{base}/api/factory/events"),
    };
    let mut fwd = forward_headers(&headers, &user);
    if let Some(id) = &last {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(id) {
            fwd.insert("last-event-id", v);
        }
    }
    let resp = match client(None).get(&url).headers(fwd).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => return transport(format!("the Factory engine's event stream answered {}", r.status())),
        Err(e) => return transport(format!("the Factory engine at {base} is not reachable: {e}")),
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);
    let mut approvals = crate::factory_approvals::subscribe();
    let owner = user.user_id.clone();
    tokio::spawn(async move {
        let mut engine = resp.bytes_stream();
        let mut buf = String::new();
        let mut cursor = last.unwrap_or_default();
        loop {
            tokio::select! {
                chunk = engine.next() => {
                    let Some(Ok(chunk)) = chunk else {
                        // The engine stream ended: end ours so the client reconnects.
                        return;
                    };
                    buf.push_str(&String::from_utf8_lossy(&chunk));
                    for f in take_frames(&mut buf) {
                        let mut ev = Event::default().data(f.data);
                        if let Some(t) = f.event { ev = ev.event(t); }
                        if let Some(id) = f.id { cursor = id.clone(); ev = ev.id(id); }
                        if tx.send(Ok(ev)).await.is_err() { return; }
                    }
                }
                approval = approvals.recv() => {
                    let evt = match approval {
                        Ok(v) => v,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => return,
                    };
                    // Only the signed-in owner's approvals (fail closed when
                    // an event names no owner).
                    if evt["owner"].as_str() != Some(owner.as_str()) {
                        continue;
                    }
                    let mut evt = evt;
                    if let Some(o) = evt.as_object_mut() {
                        o.remove("owner");
                    }
                    let ty = evt["type"].as_str().unwrap_or("approval").to_string();
                    let aid = evt["data"]["id"].as_str().unwrap_or("").to_string();
                    let ev = Event::default().event(ty).id(format!("{cursor}~apr-{aid}")).data(evt.to_string());
                    if tx.send(Ok(ev)).await.is_err() { return; }
                }
            }
        }
    });
    Sse::new(tokio_stream_from(rx)).keep_alive(KeepAlive::default()).into_response()
}

fn tokio_stream_from<T: Send + 'static>(
    mut rx: tokio::sync::mpsc::Receiver<T>,
) -> impl futures::Stream<Item = T> + Send + 'static {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_split_and_keep_the_partial_tail() {
        let mut buf = "id: evt_1\nevent: delivery\ndata: {\"a\":1}\n\n: keep-alive\n\nid: evt_2\nevent: node.status\ndata: {".to_string();
        let frames = take_frames(&mut buf);
        assert_eq!(
            frames,
            vec![Frame { id: Some("evt_1".into()), event: Some("delivery".into()), data: "{\"a\":1}".into() }]
        );
        assert_eq!(buf, "id: evt_2\nevent: node.status\ndata: {");
        buf.push_str("}\n\n");
        assert_eq!(take_frames(&mut buf)[0].id.as_deref(), Some("evt_2"));
    }

    #[test]
    fn only_the_pane_mirror_streams() {
        assert!(is_stream_path("agents/a1/stream"));
        assert!(is_stream_path("/agents/a1/stream/"));
        assert!(!is_stream_path("agents//stream"));
        assert!(!is_stream_path("agents/a1/capture"));
        assert!(!is_stream_path("agents/stream"));
        assert!(!is_stream_path("events"));
    }

    #[test]
    fn merged_ids_resume_the_engine_replay() {
        assert_eq!(engine_cursor("evt_0001_000002"), Some("evt_0001_000002".into()));
        assert_eq!(engine_cursor("evt_0001_000002~apr-fa_9"), Some("evt_0001_000002".into()));
        assert_eq!(engine_cursor("~apr-fa_9"), None);
        assert_eq!(engine_cursor("apr-fa_9"), None);
    }
}
