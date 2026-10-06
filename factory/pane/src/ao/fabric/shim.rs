//! Loopback HTTP+WS shim (spike D4): axum gateway bound to 127.0.0.1 on its
//! own port (default 8014 — distinct from Desktop's 8013), exposing the
//! gizzi-code remote-control surface the Fabric PWA drives
//! (`RemoteControlClient` in sdk/allternit-sdk) and translating it to the
//! engine socket API (session `ao`, ND-JSON over UDS). Bound to loopback
//! only; the relay client is the only intended caller.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::api::client::ApiClient;
use crate::api::schema::{
    EmptyParams, Method, PaneListParams, PaneReadParams, PaneReadResult, PaneSendInputParams,
    ReadFormat, ReadSource, Request,
};

/// ao session registry (`~/.agent-orchestrator/state.json`) — the same file
/// `ao spawn|kill` maintains; read-only here.
#[derive(Default, Deserialize)]
struct AoRegistry {
    sessions: BTreeMap<String, AoRegistryEntry>,
}

#[derive(Clone, Deserialize)]
struct AoRegistryEntry {
    cwd: String,
    #[serde(default)]
    log: Option<String>,
    #[serde(default)]
    dead: bool,
}

pub(crate) struct ShimState {
    pub relay_label: Arc<tokio::sync::RwLock<String>>,
}

fn registry_path() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"));
    home.join(".agent-orchestrator").join("state.json")
}

fn registry_mtime_ms() -> u64 {
    std::fs::metadata(registry_path())
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn load_registry() -> AoRegistry {
    std::fs::read_to_string(registry_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Run an engine API call without stalling the async runtime (the engine
/// socket is std-thread blocking with a 5 s app-response timeout).
async fn engine_call(method: Method) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || {
        let client = ApiClient::local();
        let request = Request {
            id: "ao-fabric-shim".into(),
            method,
        };
        let value = client
            .request_value(&request)
            .map_err(|err| format!("engine: {err}"))?;
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("engine error");
            return Err(message.to_string());
        }
        Ok(value["result"].clone())
    })
    .await
    .map_err(|err| format!("engine task: {err}"))?
}

async fn list_workspaces() -> Result<Vec<Value>, String> {
    let result = engine_call(Method::WorkspaceList(EmptyParams {})).await?;
    Ok(result["workspaces"].as_array().cloned().unwrap_or_default())
}

async fn find_workspace(label: &str) -> Result<Option<Value>, String> {
    Ok(list_workspaces()
        .await?
        .into_iter()
        .find(|ws| ws["label"].as_str() == Some(label)))
}

async fn first_pane(workspace_id: &str) -> Result<Option<Value>, String> {
    let result = engine_call(Method::PaneList(PaneListParams {
        workspace_id: Some(workspace_id.to_string()),
    }))
    .await?;
    Ok(result["panes"].as_array().and_then(|panes| panes.first().cloned()))
}

async fn pane_read(pane_id: &str, lines: u32) -> Result<PaneReadResult, String> {
    let result = engine_call(Method::PaneRead(PaneReadParams {
        pane_id: pane_id.to_string(),
        source: ReadSource::Recent,
        lines: Some(lines),
        format: ReadFormat::Text,
        strip_ansi: true,
        intent: Default::default(),
    }))
    .await?;
    serde_json::from_value(result["read"].clone()).map_err(|err| format!("pane.read shape: {err}"))
}

fn status_for_workspace(workspace: &Value) -> Value {
    let busy = matches!(
        workspace["agent_status"].as_str(),
        Some("working" | "blocked")
    );
    if busy {
        json!({ "type": "busy" })
    } else {
        json!({ "type": "idle" })
    }
}

fn remote_session(id: &str, entry: &AoRegistryEntry, mtime_ms: u64) -> Value {
    let slug = id.strip_prefix("ao-").unwrap_or(id);
    json!({
        "id": id,
        "slug": slug,
        "projectID": "ao",
        "directory": entry.cwd,
        "title": slug,
        "version": "1",
        "time": { "created": mtime_ms, "updated": mtime_ms },
        "agentID": "ao"
    })
}

async fn list_sessions() -> Result<Vec<Value>, String> {
    let registry = load_registry();
    let mtime_ms = registry_mtime_ms();
    let workspaces = list_workspaces().await.unwrap_or_default();
    let mut sessions = Vec::new();
    for (id, entry) in &registry.sessions {
        if entry.dead {
            continue;
        }
        // Presence probe: the engine removes dead workspaces, so listing
        // means alive (same semantics as `ao status`).
        let alive = workspaces
            .iter()
            .any(|ws| ws["label"].as_str() == Some(id.as_str()));
        if !alive {
            continue;
        }
        let workspace = workspaces
            .iter()
            .find(|ws| ws["label"].as_str() == Some(id.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        sessions.push(json!({
            "session": remote_session(id, entry, mtime_ms),
            "status": status_for_workspace(&workspace),
        }));
    }
    Ok(sessions)
}

async fn session_detail(id: &str) -> Result<Option<Value>, String> {
    let registry = load_registry();
    let Some(entry) = registry.sessions.get(id) else {
        return Ok(None);
    };
    let Some(workspace) = find_workspace(id).await? else {
        return Ok(None);
    };
    let mtime_ms = registry_mtime_ms();
    let status = status_for_workspace(&workspace);

    let mut messages = Vec::new();
    let workspace_id = workspace["workspace_id"].as_str().unwrap_or_default().to_string();
    if let Some(pane) = first_pane(&workspace_id).await? {
        if let Ok(read) = pane_read(pane["pane_id"].as_str().unwrap_or_default(), 400).await {
            if !read.text.trim().is_empty() {
                messages.push(json!({
                    "info": {
                        "id": format!("msg-{id}"),
                        "sessionID": id,
                        "role": "assistant",
                        "time": { "created": mtime_ms }
                    },
                    "parts": [{ "type": "text", "text": read.text }]
                }));
            }
        }
    }

    Ok(Some(json!({
        "session": remote_session(id, entry, mtime_ms),
        "status": status,
        "messages": messages,
    })))
}

async fn session_workspace_and_pane(id: &str) -> Result<Option<(Value, Value)>, String> {
    let Some(workspace) = find_workspace(id).await? else {
        return Ok(None);
    };
    let workspace_id = workspace["workspace_id"].as_str().unwrap_or_default().to_string();
    let Some(pane) = first_pane(&workspace_id).await? else {
        return Ok(None);
    };
    Ok(Some((workspace, pane)))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "node": "ao" }))
}

async fn status(State(state): State<Arc<ShimState>>) -> Json<Value> {
    let sessions = list_sessions().await.unwrap_or_default();
    let names: Vec<&str> = sessions
        .iter()
        .filter_map(|entry| entry["session"]["id"].as_str())
        .collect();
    let relay = state.relay_label.read().await.clone();
    Json(json!({
        "status": "ok",
        "node": "ao",
        "relay": relay,
        "sessions": names,
    }))
}

async fn remote_sessions() -> Response {
    match list_sessions().await {
        Ok(sessions) => Json(sessions).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "runtime_proxy_error", "message": err })),
        )
            .into_response(),
    }
}

async fn remote_session_detail(AxumPath(id): AxumPath<String>) -> Response {
    match session_detail(&id).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found", "message": format!("no session {id}") })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "runtime_proxy_error", "message": err })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct SendMessageBody {
    text: String,
    #[serde(default)]
    attachments: Option<Value>,
}

async fn remote_session_message(
    AxumPath(id): AxumPath<String>,
    Json(body): Json<SendMessageBody>,
) -> Response {
    if body.text.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "bad_request", "message": "text must not be empty" })),
        )
            .into_response();
    }
    match session_workspace_and_pane(&id).await {
        Ok(Some((_workspace, pane))) => {
            let pane_id = pane["pane_id"].as_str().unwrap_or_default().to_string();
            let result = engine_call(Method::PaneSendInput(PaneSendInputParams {
                pane_id,
                text: body.text,
                keys: vec!["enter".to_string()],
            }))
            .await;
            match result {
                Ok(_) => Json(json!({ "accepted": true, "sessionID": id })).into_response(),
                Err(err) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "runtime_proxy_error", "message": err })),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found", "message": format!("no session {id}") })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "runtime_proxy_error", "message": err })),
        )
            .into_response(),
    }
}

async fn remote_session_abort(AxumPath(id): AxumPath<String>) -> Response {
    match session_workspace_and_pane(&id).await {
        Ok(Some((_workspace, pane))) => {
            let pane_id = pane["pane_id"].as_str().unwrap_or_default().to_string();
            let result = engine_call(Method::PaneSendInput(PaneSendInputParams {
                pane_id,
                text: String::new(),
                keys: vec!["ctrl+c".to_string()],
            }))
            .await;
            match result {
                Ok(_) => Json(true).into_response(),
                Err(err) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "runtime_proxy_error", "message": err })),
                )
                    .into_response(),
            }
        }
        Ok(None) => Json(false).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "runtime_proxy_error", "message": err })),
        )
            .into_response(),
    }
}

async fn permission_list() -> Json<Value> {
    // The ao engine has no permission/question queues (gizzi-code concepts);
    // empty lists keep the PWA pending-counts views working.
    Json(json!([]))
}

async fn question_list() -> Json<Value> {
    Json(json!([]))
}

async fn reply_ok() -> Json<bool> {
    Json(true)
}

async fn create_session() -> Response {
    // Creating a gizzi-style chat session has no ao-engine equivalent; ao
    // sessions come from `ao spawn`. Surfaced explicitly rather than faked.
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "not_implemented",
            "message": "ao sessions are created with `ao spawn <slug> <repo-dir> <agent-cmd…>` on the node"
        })),
    )
        .into_response()
}

/// `GET /v1/remote-control/sessions/:id/events` over the socket tunnel:
/// remote.connected first, then message.part.updated frames whenever the
/// pane transcript changes, plus a 10 s heartbeat (remote-control.ts:150-221
/// semantics, adapted: ao has no message bus, it polls the pane revision).
async fn remote_session_events(AxumPath(id): AxumPath<String>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| session_events(socket, id))
}

async fn session_events(mut socket: WebSocket, id: String) {
    let Some(workspace) = (match find_workspace(&id).await {
        Ok(workspace) => workspace,
        Err(_) => None,
    }) else {
        let _ = socket
            .send(WsMessage::Text(
                json!({ "type": "session.status", "properties": { "sessionID": id, "status": { "type": "idle" } } })
                    .to_string(),
            ))
            .await;
        let _ = socket.close().await;
        return;
    };
    let status = status_for_workspace(&workspace);
    let _ = socket
        .send(WsMessage::Text(
            json!({ "type": "remote.connected", "properties": { "sessionID": id, "status": status } })
                .to_string(),
        ))
        .await;

    let workspace_id = workspace["workspace_id"].as_str().unwrap_or_default().to_string();
    let Ok(Some(pane)) = first_pane(&workspace_id).await else {
        let _ = socket.close().await;
        return;
    };
    let pane_id = pane["pane_id"].as_str().unwrap_or_default().to_string();
    let mut last_revision = 0u64;
    let mut last_status = status;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.tick().await; // first tick is immediate

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if socket.send(WsMessage::Text(
                    json!({ "type": "remote.heartbeat", "properties": { "sessionID": id } }).to_string()
                )).await.is_err() { break; }
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                // Poll the pane; stream transcript + status changes.
                let (workspace, pane) = match session_workspace_and_pane(&id).await {
                    Ok(Some(pair)) => pair,
                    _ => break, // workspace gone — session ended
                };
                let status = status_for_workspace(&workspace);
                if status != last_status {
                    last_status = status.clone();
                    if socket.send(WsMessage::Text(
                        json!({ "type": "session.status", "properties": { "sessionID": id, "status": status } }).to_string()
                    )).await.is_err() { break; }
                }
                let current_pane_id = pane["pane_id"].as_str().unwrap_or_default().to_string();
                if current_pane_id != pane_id { break; }
                if let Ok(read) = pane_read(&pane_id, 400).await {
                    if read.revision != last_revision {
                        last_revision = read.revision;
                        let event = json!({
                            "type": "message.part.updated",
                            "properties": {
                                "part": {
                                    "id": format!("part-{id}"),
                                    "messageID": format!("msg-{id}"),
                                    "sessionID": id,
                                    "type": "text",
                                    "text": read.text
                                }
                            }
                        });
                        if socket.send(WsMessage::Text(event.to_string())).await.is_err() { break; }
                    }
                }
            }
        }
    }
}

fn desktop_capture_base() -> String {
    std::env::var("ALLTERNIT_DESKTOP_CAPTURE_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8477".to_string())
        .trim_end_matches('/')
        .to_string()
}

async fn desktop_forward(method: reqwest::Method, path: &str, body: Option<Bytes>) -> Response {
    let url = format!("{}{path}", desktop_capture_base());
    let client = reqwest::Client::new();
    let mut req = client.request(method, url);
    if let Some(payload) = body {
        req = req.header("content-type", "application/json").body(payload);
    }
    match req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut out = Response::new(axum::body::Body::from(bytes));
                    *out.status_mut() = status;
                    if let Ok(val) = axum::http::HeaderValue::from_str(&ct) {
                        out.headers_mut().insert(axum::http::header::CONTENT_TYPE, val);
                    }
                    out.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-store"),
                    );
                    out
                }
                Err(_) => (StatusCode::BAD_GATEWAY, "desktop capture read failed").into_response(),
            }
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "desktop capture is not running on this machine",
        )
            .into_response(),
    }
}

async fn desktop_hello() -> Response {
    desktop_forward(reqwest::Method::GET, "/hello", None).await
}

async fn desktop_frame() -> Response {
    desktop_forward(reqwest::Method::GET, "/frame", None).await
}

async fn desktop_input(body: Bytes) -> Response {
    desktop_forward(reqwest::Method::POST, "/input", Some(body)).await
}

fn acu_base() -> String {
    std::env::var("ALLTERNIT_ACU_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8760".to_string())
        .trim_end_matches('/')
        .to_string()
}

async fn acu_forward(method: reqwest::Method, path: &str, body: Option<Bytes>) -> Response {
    let url = format!("{}{path}", acu_base());
    let client = reqwest::Client::new();
    let mut req = client.request(method, &url);
    if let Some(body) = body {
        req = req.header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let ct = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut out = Response::new(axum::body::Body::from(bytes));
                    *out.status_mut() = status;
                    if let Ok(val) = axum::http::HeaderValue::from_str(&ct) {
                        out.headers_mut().insert(axum::http::header::CONTENT_TYPE, val);
                    }
                    out
                }
                Err(_) => (StatusCode::BAD_GATEWAY, "ACU read failed").into_response(),
            }
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "computer-use gateway is not running on this machine",
        )
            .into_response(),
    }
}

async fn acu_browser_skills_get() -> Response {
    acu_forward(reqwest::Method::GET, "/v1/browser-skills", None).await
}

async fn acu_browser_skills_post(body: Bytes) -> Response {
    acu_forward(reqwest::Method::POST, "/v1/browser-skills", Some(body)).await
}

async fn acu_browser_skills_get_rest(AxumPath(rest): AxumPath<String>) -> Response {
    acu_forward(
        reqwest::Method::GET,
        &format!("/v1/browser-skills/{rest}"),
        None,
    )
    .await
}

async fn acu_browser_skills_post_rest(AxumPath(rest): AxumPath<String>, body: Bytes) -> Response {
    acu_forward(
        reqwest::Method::POST,
        &format!("/v1/browser-skills/{rest}"),
        Some(body),
    )
    .await
}

fn phone_remote_dir() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("ALLTERNIT_PHONE_REMOTE") {
        return Some(std::path::PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        // target/{debug,release}/ao → repo root
        if let Some(root) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            let candidate = root.join("surfaces/phone-remote");
            if candidate.join("server/index.mjs").is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let fallback = home.join("Desktop/allternit-workspace/allternit/surfaces/phone-remote");
    if fallback.join("server/index.mjs").is_file() {
        return Some(fallback);
    }
    None
}

async fn desktop_start() -> Response {
    let probe = desktop_forward(reqwest::Method::GET, "/healthz", None).await;
    if probe.status().is_success() {
        return (
            StatusCode::OK,
            [("content-type", "application/json")],
            r#"{"ok":true,"already":true}"#,
        )
            .into_response();
    }
    let Some(dir) = phone_remote_dir() else {
        return (
            StatusCode::NOT_FOUND,
            "phone-remote is not installed on this node (set ALLTERNIT_PHONE_REMOTE)",
        )
            .into_response();
    };
    let log = dir.join("phone-remote.fabric.log");
    let result = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("node");
        cmd.arg("server/index.mjs")
            .arg("--bind")
            .arg("127.0.0.1")
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log).map(std::process::Stdio::from).unwrap_or(std::process::Stdio::null()));
        cmd.spawn().map(|c| c.id())
    })
    .await;
    match result {
        Ok(Ok(pid)) => {
            tokio::time::sleep(Duration::from_millis(800)).await;
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                format!(r#"{{"ok":true,"pid":{pid}}}"#),
            )
                .into_response()
        }
        Ok(Err(err)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to start phone-remote: {err}"),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to start phone-remote: {err}"),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Capability-native harness surface (FabricSessionClient)
// ---------------------------------------------------------------------------
//
// The live Fabric PWA (`fabric-session` App, FabricSessionPanel) lists and
// drives sessions through the capability-native session-worker contract
// (sdk/allternit-sdk `FabricSessionClient`): POST /api/v1/fabric/leases mints
// a lease, POST /api/v1/session-worker/invoke dispatches `harness.session*`
// capabilities, and GET /api/v1/session-worker/sessions/:id/events streams
// RemoteControlEvent frames over SSE. Leases are self-issued and opaque —
// the loopback shim is the only consumer of the signature, so there is
// nothing to verify against.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseBody {
    capability_id: String,
    #[serde(default)]
    grantee: Option<String>,
    #[serde(default)]
    ttl_seconds: Option<u64>,
}

fn random_id(prefix: &str) -> String {
    use rand::Rng as _;
    let mut bytes = [0u8; 12];
    rand::thread_rng().fill(&mut bytes);
    let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}_{suffix}")
}

async fn issue_lease(Json(body): Json<LeaseBody>) -> Response {
    let ttl = body.ttl_seconds.unwrap_or(300).clamp(1, 86400);
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let issued_at = humantime_or_epoch(now);
    let expires_at = humantime_or_epoch(now + ttl);
    Json(json!({
        "id": random_id("lease"),
        "capabilityId": body.capability_id,
        "grantee": body.grantee.unwrap_or_else(|| "web-client".to_string()),
        "issuedAt": issued_at,
        "expiresAt": expires_at,
        "status": "active",
        "signature": random_id("sig"),
    }))
    .into_response()
}

fn humantime_or_epoch(secs: u64) -> String {
    // RFC 3339 without chrono: the PWA only round-trips these strings.
    let days = secs / 86400;
    let seconds_in_day = secs % 86400;
    let (year, day_of_year) = epoch_to_ymd(days);
    let (month, day) = day_of_year_to_month_day(year, day_of_year);
    let h = seconds_in_day / 3600;
    let m = (seconds_in_day % 3600) / 60;
    let s = seconds_in_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

fn epoch_to_ymd(days_since_epoch: u64) -> (u64, u64) {
    // Days since epoch → (civil year, day-of-year). Gregorian calendar.
    let mut year = 1970u64;
    let mut remaining = days_since_epoch;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let year_days = if leap { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        year += 1;
    }
    (year, remaining)
}

fn day_of_year_to_month_day(year: u64, day_of_year: u64) -> (u64, u64) {
    const COMMON: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let mut day = day_of_year;
    for (index, &len) in COMMON.iter().enumerate() {
        let len = if index == 1 && leap { 29 } else { len };
        if day < len {
            return ((index as u64) + 1, day + 1);
        }
        day -= len;
    }
    (12, 31)
}

#[derive(Deserialize)]
struct InvokeBody {
    capability: String,
    #[serde(default)]
    inputs: Option<Value>,
    #[serde(default)]
    lease: Option<Value>,
}

async fn invoke_capability(Json(body): Json<InvokeBody>) -> Response {
    let inputs = body.inputs.unwrap_or(json!({}));
    let _ = body.lease; // self-issued leases are opaque; nothing to verify
    let session_id = inputs["sessionID"].as_str().unwrap_or_default();

    match body.capability.as_str() {
        "harness.session" => match list_sessions().await {
            Ok(sessions) => Json(json!({ "result": sessions })).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "runtime_proxy_error", "message": err })),
            )
                .into_response(),
        },
        "harness.session.get" => match session_detail(session_id).await {
            Ok(Some(detail)) => Json(json!({ "result": detail })).into_response(),
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "message": format!("no session {session_id}") })),
            )
                .into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "runtime_proxy_error", "message": err })),
            )
                .into_response(),
        },
        "harness.session.message" => {
            let text = inputs["text"].as_str().unwrap_or_default().to_string();
            if text.trim().is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "bad_request", "message": "text must not be empty" })),
                )
                    .into_response();
            }
            match session_workspace_and_pane(session_id).await {
                Ok(Some((_workspace, pane))) => {
                    let pane_id = pane["pane_id"].as_str().unwrap_or_default().to_string();
                    match engine_call(Method::PaneSendInput(PaneSendInputParams {
                        pane_id,
                        text,
                        keys: vec!["enter".to_string()],
                    }))
                    .await
                    {
                        Ok(_) => {
                            Json(json!({ "result": { "accepted": true, "sessionID": session_id } }))
                                .into_response()
                        }
                        Err(err) => (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": "runtime_proxy_error", "message": err })),
                        )
                            .into_response(),
                    }
                }
                Ok(None) => (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "not_found", "message": format!("no session {session_id}") })),
                )
                    .into_response(),
                Err(err) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "runtime_proxy_error", "message": err })),
                )
                    .into_response(),
            }
        }
        "harness.session.abort" => match session_workspace_and_pane(session_id).await {
            Ok(Some((_workspace, pane))) => {
                let pane_id = pane["pane_id"].as_str().unwrap_or_default().to_string();
                match engine_call(Method::PaneSendInput(PaneSendInputParams {
                    pane_id,
                    text: String::new(),
                    keys: vec!["ctrl+c".to_string()],
                }))
                .await
                {
                    Ok(_) => Json(json!({ "result": true })).into_response(),
                    Err(err) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "runtime_proxy_error", "message": err })),
                    )
                        .into_response(),
                }
            }
            Ok(None) => Json(json!({ "result": false })).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "runtime_proxy_error", "message": err })),
            )
                .into_response(),
        },
        "harness.session.create" => (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "error": "not_implemented",
                "message": "ao sessions are created with `ao spawn <slug> <repo-dir> <agent-cmd…>` on the node"
            })),
        )
            .into_response(),
        "harness.session.permissions.list" | "harness.session.questions.list" => {
            Json(json!({ "result": [] })).into_response()
        }
        "harness.session.permissions.reply"
        | "harness.session.questions.reply"
        | "harness.session.questions.reject" => Json(json!({ "result": true })).into_response(),
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown_capability", "message": format!("unknown capability {other}") })),
        )
            .into_response(),
    }
}

/// GET /api/v1/session-worker/sessions/:id/events — SSE stream of
/// RemoteControlEvent frames (remote-control.ts:150-221 semantics over SSE
/// instead of WS: remote.connected first, then message.part.updated on pane
/// revision changes, session.status on transitions, 10 s heartbeat).
async fn harness_session_events(AxumPath(id): AxumPath<String>) -> Response {
    let workspace = match find_workspace(&id).await {
        Ok(Some(workspace)) => workspace,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "message": format!("no session {id}") })),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "runtime_proxy_error", "message": err })),
            )
                .into_response();
        }
    };
    let status = status_for_workspace(&workspace);

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<SseEvent, std::convert::Infallible>>();
    let connected = json!({ "type": "remote.connected", "properties": { "sessionID": id, "status": status } });
    let _ = tx.send(Ok(
        SseEvent::default().event("remote.connected").data(connected.to_string()),
    ));

    tokio::spawn(async move {
        let send = |event: Value| {
            let _ = tx.send(Ok(SseEvent::default()
                .event(event["type"].as_str().unwrap_or("message"))
                .data(event.to_string())));
        };
        let mut last_revision = 0u64;
        let mut last_status = status;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
        heartbeat.tick().await; // first tick is immediate
        loop {
            tokio::select! {
                _ = heartbeat.tick() => {
                    send(json!({ "type": "remote.heartbeat", "properties": { "sessionID": id } }));
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    let (workspace, pane) = match session_workspace_and_pane(&id).await {
                        Ok(Some(pair)) => pair,
                        _ => break, // workspace gone — session ended
                    };
                    let status = status_for_workspace(&workspace);
                    if status != last_status {
                        last_status = status.clone();
                        send(json!({ "type": "session.status", "properties": { "sessionID": id, "status": status } }));
                    }
                    let pane_id = pane["pane_id"].as_str().unwrap_or_default().to_string();
                    if let Ok(read) = pane_read(&pane_id, 400).await {
                        if read.revision != last_revision {
                            last_revision = read.revision;
                            send(json!({
                                "type": "message.part.updated",
                                "properties": {
                                    "part": {
                                        "id": format!("part-{id}"),
                                        "messageID": format!("msg-{id}"),
                                        "sessionID": id,
                                        "type": "text",
                                        "text": read.text
                                    }
                                }
                            }));
                        }
                    }
                }
            }
        }
        // Channel drops when this task ends, closing the SSE stream.
    });

    Sse::new(UnboundedReceiverStream::new(rx))
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

pub(crate) async fn serve(port: u16, state: Arc<ShimState>) -> Result<(), String> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/v1/remote-control/sessions", get(remote_sessions))
        .route(
            "/v1/remote-control/sessions/:id",
            get(remote_session_detail),
        )
        .route(
            "/v1/remote-control/sessions/:id/messages",
            post(remote_session_message),
        )
        .route(
            "/v1/remote-control/sessions/:id/abort",
            post(remote_session_abort),
        )
        .route(
            "/v1/remote-control/sessions/:id/events",
            get(remote_session_events),
        )
        .route("/v1/permission", get(permission_list))
        .route("/v1/permission/:id/reply", post(reply_ok))
        .route("/v1/question", get(question_list))
        .route("/v1/question/:id/reply", post(reply_ok))
        .route("/v1/question/:id/reject", post(reply_ok))
        .route("/v1/session", post(create_session))
        .route("/v1/remote-control/desktop/hello", get(desktop_hello))
        .route("/v1/remote-control/desktop/frame", get(desktop_frame))
        .route("/v1/remote-control/desktop/input", post(desktop_input))
        .route("/api/v1/fabric/leases", post(issue_lease))
        .route("/api/v1/session-worker/invoke", post(invoke_capability))
        .route(
            "/api/v1/session-worker/sessions/:id/events",
            get(harness_session_events),
        )
        .route("/v1/remote-control/desktop/start", post(desktop_start))
        .route("/v1/browser-skills", get(acu_browser_skills_get).post(acu_browser_skills_post))
        .route(
            "/v1/browser-skills/*rest",
            get(acu_browser_skills_get_rest).post(acu_browser_skills_post_rest),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|err| format!("cannot bind 127.0.0.1:{port}: {err}"))?;
    println!("[ao-fabric] loopback shim listening on http://127.0.0.1:{port}");
    axum::serve(listener, app)
        .await
        .map_err(|err| format!("shim server failed: {err}"))
}
