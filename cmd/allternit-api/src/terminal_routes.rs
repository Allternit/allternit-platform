//! Terminal API routes: Code Mode terminals backed by the Allternit Factory
//! pane engine.
//!
//! The terminal service itself (the pane engine socket client, the terminal
//! records, list, create validation, graceful close and the SSE stream body)
//! is the `allternit-terminal-service` crate (`factory/terminal-service`),
//! shared with allternit-node. This module is the axum adapter: the same
//! routes, bodies, status codes and JSON as before, mounted at `/terminal`.

use axum::{
    body::Body,
    extract::{FromRef, Json, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tracing::debug;

pub(crate) use allternit_terminal_service::{agent_request, factory_bin};
pub use allternit_terminal_service::{
    CreateTerminalRequest, ListQuery, PaneError, Reply, TerminalCloseRequest, TerminalInputRequest,
    TerminalMessageResponse, TerminalResizeRequest, TerminalService,
    TerminalService as TerminalSessionStore,
};

use crate::AppState;

/// The service as an axum state part. A local type, because
/// `FromRef<Arc<AppState>>` can't be implemented here for the crate's own
/// type (orphan rule). Routes take it from the app state, or from a bare
/// store (tests).
#[derive(Clone)]
pub struct Terminals(pub TerminalSessionStore);

impl FromRef<Arc<AppState>> for Terminals {
    fn from_ref(state: &Arc<AppState>) -> Self {
        Terminals(state.terminal_sessions.clone())
    }
}

impl FromRef<TerminalSessionStore> for Terminals {
    fn from_ref(store: &TerminalSessionStore) -> Self {
        Terminals(store.clone())
    }
}

fn reply(r: Reply) -> (StatusCode, Json<TerminalMessageResponse>) {
    (
        StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(r.body),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Routes
// ─────────────────────────────────────────────────────────────────────────────

pub fn terminal_router() -> Router<Arc<AppState>> {
    routes()
}

/// The routes over any state that holds the store (tests use the store
/// alone).
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    Terminals: FromRef<S>,
{
    Router::new()
        .route("/create", post(create_terminal))
        .route("/list", get(list_terminals))
        .route("/:session_id/input", post(terminal_input))
        .route("/:session_id/close", post(terminal_close))
        .route("/:session_id/resize", post(terminal_resize))
        .route("/:session_id/stream", get(terminal_stream))
}

async fn create_terminal(
    State(Terminals(store)): State<Terminals>,
    peer: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<CreateTerminalRequest>,
) -> Response {
    // Start anywhere safety switch (D6): off refuses a terminal another device asks for.
    if let Some(refused) = crate::session_start_routes::refuse_if_off(&headers, peer.map(|c| c.0)) {
        return refused;
    }
    match create_with(&store, request, None).await {
        Ok(session_id) => {
            debug!(%session_id, "Terminal session created (factory pane)");
            (
                StatusCode::OK,
                Json(TerminalMessageResponse {
                    success: true,
                    message: "Terminal session created".into(),
                    data: Some(serde_json::json!({ "session_id": session_id })),
                }),
            )
                .into_response()
        }
        Err(err) => err.into_response(),
    }
}

/// Create a terminal (what `POST /create` does) and return its id. `shown`: the command
/// `GET /list` reports when the launched argv wraps it (adopt runs the agent through the
/// user's login shell).
pub(crate) async fn create_with(
    store: &TerminalSessionStore,
    request: CreateTerminalRequest,
    shown: Option<Vec<String>>,
) -> Result<String, (StatusCode, Json<TerminalMessageResponse>)> {
    store.create(request, shown).await.map_err(reply)
}

async fn list_terminals(State(Terminals(store)): State<Terminals>, Query(query): Query<ListQuery>) -> Response {
    match store.list(query.preview).await {
        Ok(v) => Json(v).into_response(),
        Err(err) => reply(err).into_response(),
    }
}

async fn terminal_input(
    State(Terminals(store)): State<Terminals>,
    Path(session_id): Path<String>,
    Json(request): Json<TerminalInputRequest>,
) -> impl IntoResponse {
    reply(store.input(&session_id, &request.content).await)
}

async fn terminal_resize(
    State(Terminals(store)): State<Terminals>,
    Path(session_id): Path<String>,
    Json(request): Json<TerminalResizeRequest>,
) -> impl IntoResponse {
    reply(store.resize(&session_id, request.cols, request.rows).await)
}

async fn terminal_close(
    State(Terminals(store)): State<Terminals>,
    Path(session_id): Path<String>,
    body: Option<Json<TerminalCloseRequest>>,
) -> impl IntoResponse {
    let graceful = body.is_some_and(|Json(b)| b.graceful);
    reply(store.close(&session_id, graceful).await)
}

async fn terminal_stream(State(Terminals(store)): State<Terminals>, Path(session_id): Path<String>) -> Response {
    match store.stream(&session_id).await {
        Ok(stream) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response(),
        Err(err) => reply(err).into_response(),
    }
}

// The fake engine below listens on a Unix socket.
#[cfg(all(test, unix))]
mod tests {
    //! The routes against a fake pane engine that speaks the real wire
    //! protocol (one request per connection, `factory.terminal.*`) and echoes
    //! input back as output. The real engine is covered by the pane crate's
    //! tests and the smoke boot.
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    #[derive(Default)]
    struct FakeTerm {
        output: String,
        size: (u64, u64),
        exit: Option<i64>,
    }

    #[derive(Default)]
    struct Fake {
        terms: std::sync::Mutex<HashMap<String, FakeTerm>>,
        changed: Notify,
    }

    async fn serve_conn(fake: Arc<Fake>, stream: tokio::net::UnixStream) {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read);
        let mut line = String::new();
        if lines.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let req: Value = serde_json::from_str(line.trim()).unwrap();
        let id = req["id"].clone();
        let p = &req["params"];
        let tid = p["terminal_id"].as_str().unwrap_or_default().to_string();
        let missing = json!({ "id": id, "error": { "code": "terminal_not_found", "message": format!("terminal {tid} not found") } });
        let reply = |v: Value| format!("{v}\n");
        let resp = {
            let mut terms = fake.terms.lock().unwrap();
            match req["method"].as_str().unwrap() {
                "factory.terminal.create" => {
                    assert_eq!(p["env"]["TERM"], "xterm-256color");
                    terms.insert(tid.clone(), FakeTerm {
                        output: "$ ".into(),
                        size: (p["cols"].as_u64().unwrap(), p["rows"].as_u64().unwrap()),
                        exit: None,
                    });
                    Some(json!({ "id": id, "result": { "terminal": { "terminal_id": tid, "running": true } } }))
                }
                "factory.terminal.write" => match terms.get_mut(&tid) {
                    Some(t) => {
                        let data = p["data"].as_str().unwrap();
                        if data == "exit\r" {
                            t.exit = Some(0);
                        } else {
                            t.output.push_str(&data.replace('\r', "\r\n"));
                        }
                        Some(json!({ "id": id, "result": {} }))
                    }
                    None => Some(missing.clone()),
                },
                "factory.terminal.resize" => match terms.get_mut(&tid) {
                    Some(t) => {
                        t.size = (p["cols"].as_u64().unwrap(), p["rows"].as_u64().unwrap());
                        Some(json!({ "id": id, "result": {} }))
                    }
                    None => Some(missing.clone()),
                },
                "factory.terminal.close" => match terms.remove(&tid) {
                    Some(_) => Some(json!({ "id": id, "result": {} })),
                    None => Some(missing.clone()),
                },
                "factory.terminal.output" => {
                    if terms.contains_key(&tid) {
                        None
                    } else {
                        Some(missing.clone())
                    }
                }
                other => panic!("unexpected method {other}"),
            }
        };
        fake.changed.notify_waiters();
        if let Some(resp) = resp {
            let _ = write.write_all(reply(resp).as_bytes()).await;
            return;
        }
        // factory.terminal.output: replay, then follow until exit/close.
        let _ = write.write_all(reply(json!({ "id": id, "result": { "terminal_id": tid } })).as_bytes()).await;
        let mut sent = 0;
        loop {
            let notified = fake.changed.notified();
            let (chunk, exit, gone) = {
                let terms = fake.terms.lock().unwrap();
                match terms.get(&tid) {
                    Some(t) => (t.output[sent..].to_string(), t.exit, false),
                    None => (String::new(), None, true),
                }
            };
            if gone {
                return;
            }
            if !chunk.is_empty() {
                sent += chunk.len();
                if write.write_all(reply(json!({ "type": "output", "data": chunk })).as_bytes()).await.is_err() {
                    return;
                }
            }
            if let Some(code) = exit {
                let _ = write.write_all(reply(json!({ "type": "exit", "exit_code": code })).as_bytes()).await;
                return;
            }
            let _ = tokio::time::timeout(Duration::from_millis(200), notified).await;
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        fake: Arc<Fake>,
        app: Router,
    }

    fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("pane.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let fake = Arc::new(Fake::default());
        let f = fake.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_conn(f.clone(), stream));
            }
        });
        let store = TerminalSessionStore::with_socket(sock, dir.path().join("no-allternit-factory"));
        Harness { _dir: dir, fake, app: routes().with_state(store) }
    }

    async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::post(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn create(app: &Router) -> String {
        let (status, body) = post(app, "/create", json!({ "cols": 120, "rows": 40 })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["success"], true);
        body["data"]["session_id"].as_str().unwrap().to_string()
    }

    /// Reads the SSE stream until `until` is in the decoded `data` text (or
    /// an exit event when `until` is `None`); returns the text and exit code.
    async fn read_stream(app: &Router, id: &str, until: Option<&str>) -> (String, Option<Value>) {
        let resp = app
            .clone()
            .oneshot(Request::get(format!("/{id}/stream")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        let mut body = resp.into_body();
        let mut raw = String::new();
        let mut text = String::new();
        let mut exit = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let frame = tokio::time::timeout_at(deadline, body.frame()).await.expect("stream timed out");
            let Some(Ok(frame)) = frame else { break };
            if let Ok(data) = frame.into_data() {
                raw.push_str(&String::from_utf8_lossy(&data));
            }
            while let Some(end) = raw.find("\n\n") {
                let event: String = raw.drain(..end + 2).collect();
                if let Some(payload) = event.strip_prefix("data: ") {
                    let v: Value = serde_json::from_str(payload.trim()).unwrap();
                    match v["type"].as_str() {
                        Some("data") => text.push_str(v["data"].as_str().unwrap()),
                        Some("exit") => exit = Some(v["exit_code"].clone()),
                        _ => {}
                    }
                }
            }
            match until {
                Some(needle) if text.contains(needle) => return (text, exit),
                None if exit.is_some() => return (text, exit),
                _ => {}
            }
        }
        (text, exit)
    }

    #[tokio::test]
    async fn create_write_read_resize_close() {
        let h = harness();
        let id = create(&h.app).await;
        assert_eq!(h.fake.terms.lock().unwrap()[&id].size, (120, 40));

        let (status, _) = post(&h.app, &format!("/{id}/input"), json!({ "content": "echo hi\r" })).await;
        assert_eq!(status, StatusCode::OK);
        let (text, _) = read_stream(&h.app, &id, Some("echo hi\r\n")).await;
        assert!(text.starts_with("$ echo hi"), "{text:?}");

        let (status, body) = post(&h.app, &format!("/{id}/resize"), json!({ "cols": 100, "rows": 30 })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(h.fake.terms.lock().unwrap()[&id].size, (100, 30));

        let (status, body) = post(&h.app, &format!("/{id}/close"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert!(!h.fake.terms.lock().unwrap().contains_key(&id));

        // Gone after close.
        let (status, _) = post(&h.app, &format!("/{id}/input"), json!({ "content": "x" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // Closing again is still a success (idempotent, as before).
        let (status, _) = post(&h.app, &format!("/{id}/close"), json!({})).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn reconnect_replays_scrollback_and_exit_is_reported() {
        let h = harness();
        let id = create(&h.app).await;
        post(&h.app, &format!("/{id}/input"), json!({ "content": "one\r" })).await;
        let (first, _) = read_stream(&h.app, &id, Some("one\r\n")).await;
        // A second connection (a UI reconnect) gets the same scrollback.
        let (again, _) = read_stream(&h.app, &id, Some("one\r\n")).await;
        assert_eq!(first, again);

        post(&h.app, &format!("/{id}/input"), json!({ "content": "exit\r" })).await;
        let (_, exit) = read_stream(&h.app, &id, None).await;
        assert_eq!(exit, Some(json!(0)));
    }

    #[tokio::test]
    async fn two_terminals_are_independent() {
        let h = harness();
        let a = create(&h.app).await;
        let b = create(&h.app).await;
        assert_ne!(a, b);
        post(&h.app, &format!("/{a}/input"), json!({ "content": "alpha\r" })).await;
        post(&h.app, &format!("/{b}/input"), json!({ "content": "beta\r" })).await;
        let (ta, _) = read_stream(&h.app, &a, Some("alpha\r\n")).await;
        let (tb, _) = read_stream(&h.app, &b, Some("beta\r\n")).await;
        assert!(!ta.contains("beta"), "{ta:?}");
        assert!(!tb.contains("alpha"), "{tb:?}");
        post(&h.app, &format!("/{a}/close"), json!({})).await;
        // Closing one leaves the other running.
        let (status, _) = post(&h.app, &format!("/{b}/resize"), json!({ "cols": 90, "rows": 20 })).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_terminal_is_404() {
        let h = harness();
        let resp = h
            .app
            .clone()
            .oneshot(Request::get("/nope/stream").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let (status, _) = post(&h.app, "/nope/resize", json!({ "cols": 80, "rows": 24 })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = post(&h.app, "/bad%2Fid/input", json!({ "content": "x" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn engine_down_is_503_not_a_silent_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = TerminalSessionStore::with_socket(
            dir.path().join("absent.sock"),
            dir.path().join("no-allternit-factory"),
        );
        let app: Router = routes().with_state(store);
        let (status, body) = post(&app, "/create", json!({})).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["success"], false);
        assert!(body["message"].as_str().unwrap().contains("pane engine"), "{body}");
    }
}

#[cfg(test)]
mod list_route_tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;
    use uuid::Uuid;

    /// `GET /list` is its own route, not a session id: with the engine down it
    /// reaches the engine call (503 JSON) instead of falling through to a 404.
    #[tokio::test]
    async fn terminal_list_route() {
        let dir = std::env::temp_dir().join(format!("terminal-list-route-{}", Uuid::new_v4()));
        let store = TerminalSessionStore::with_socket(dir.join("absent.sock"), dir.join("no-allternit-factory"));
        let app: Router = routes().with_state(store);
        let resp = app
            .oneshot(Request::get("/list?preview=16").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
