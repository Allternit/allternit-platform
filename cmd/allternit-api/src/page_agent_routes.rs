//! Page-agent routes
//!
//! Shared surface-agnostic proxy/stub for the Allternit page-agent runtime.
//! Phase 1 forwards to the local gizzi brain runtime (default
//! `http://127.0.0.1:4096`). Future phases can replace the proxy with an
//! in-process service implementation backed by `services/page-agent`.
//!
//! Public surface:
//!   POST /api/page-agent/run
//!   GET  /api/page-agent/stream/:session_id
//!   POST /api/page-agent/stop/:session_id
//!   GET  /api/page-agent/status/:session_id
//!   POST /api/page-agent/config

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::AppState;

/// Default page-agent runtime base URL.
const DEFAULT_PAGE_AGENT_BASE: &str = "http://127.0.0.1:4096";

pub fn page_agent_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/page-agent/run", post(page_agent_run))
        .route("/page-agent/stream/:session_id", get(page_agent_stream))
        .route("/page-agent/stop/:session_id", post(page_agent_stop))
        .route("/page-agent/status/:session_id", get(page_agent_status))
        .route("/page-agent/config", post(page_agent_config))
}

fn runtime_base(_state: &AppState) -> String {
    // In Phase 1 the runtime is always the local gizzi brain. A future phase
    // can read this from AppConfig (e.g. `state.config.page_agent_url()`).
    std::env::var("ALLTERNIT_PAGE_AGENT_URL")
        .unwrap_or_else(|_| DEFAULT_PAGE_AGENT_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn runtime_unavailable(e: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": "page_agent_unavailable",
            "message": format!("Page-agent runtime unavailable: {}", e),
        })),
    )
        .into_response()
}

async fn forward_runtime_error(resp: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap_or_default();
    (
        status,
        Json(json!({
            "error": "page_agent_error",
            "message": format!("Page-agent runtime returned {}: {}", status, text),
        })),
    )
        .into_response()
}

// ─── POST /api/page-agent/run ───────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageAgentRunBody {
    goal: String,
    /// Runtime configuration forwarded by the client. Phase 1 proxies only the
    /// session creation; future phases can pass this through to the runtime.
    #[serde(default)]
    #[allow(dead_code)]
    config: serde_json::Value,
}

async fn page_agent_run(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PageAgentRunBody>,
) -> Response {
    let goal = body.goal.trim().to_string();
    if goal.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "goal is required"})),
        )
            .into_response();
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let runtime = runtime_base(&state);
    let client = crate::net_client::client();

    let payload = json!({
        "title": goal.chars().take(80).collect::<String>(),
        "surface": "browser",
        "permission": [{"permission": "browser", "action": "allow", "pattern": "*"}],
    });

    match client
        .post(format!("{}/v1/session", runtime))
        .json(&payload)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => {
                let runtime_session_id = v
                    .get("id")
                    .and_then(|id| id.as_str())
                    .unwrap_or(&session_id)
                    .to_string();
                (
                    StatusCode::OK,
                    Json(json!({
                        "sessionId": runtime_session_id,
                        "goal": goal,
                    })),
                )
                    .into_response()
            }
            Err(_) => (
                StatusCode::OK,
                Json(json!({ "sessionId": session_id, "goal": goal })),
            )
                .into_response(),
        },
        Ok(r) => forward_runtime_error(r).await,
        Err(e) => runtime_unavailable(e),
    }
}

// ─── GET /api/page-agent/stream/:session_id ─────────────────────────────────

async fn page_agent_stream(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    let runtime = runtime_base(&state);
    let client = crate::net_client::client();

    let resp = match client
        .get(format!("{}/v1/event", runtime))
        .header("Accept", "text/event-stream")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return runtime_unavailable(e),
    };

    if !resp.status().is_success() {
        return forward_runtime_error(resp).await;
    }

    let stream = async_stream::stream! {
        let mut byte_stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                    while let Some(end) = buf.find("\n\n") {
                        let block = buf[..end].to_string();
                        buf = buf[end + 2..].to_string();
                        let data = block
                            .lines()
                            .find(|l| l.starts_with("data:"))
                            .and_then(|l| l.strip_prefix("data:"))
                            .map(str::trim)
                            .unwrap_or("");
                        if data.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(data) {
                            Ok(mut frame) => {
                                // Tag the frame with the requested session id so
                                // clients can correlate it even if the upstream
                                // runtime does not.
                                if let Some(obj) = frame.as_object_mut() {
                                    obj.entry("requestedSessionId")
                                        .or_insert(json!(session_id.clone()));
                                }
                                yield Ok::<Event, std::convert::Infallible>(
                                    Event::default().data(frame.to_string()),
                                );
                            }
                            Err(_) => {
                                // Pass through raw data so the client can still
                                // see something useful.
                                yield Ok::<Event, std::convert::Infallible>(
                                    Event::default().data(data.to_string()),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("page-agent stream: upstream read error: {}", e);
                    break;
                }
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ─── POST /api/page-agent/stop/:session_id ──────────────────────────────────

async fn page_agent_stop(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    let runtime = runtime_base(&state);
    let client = crate::net_client::client();

    match client
        .post(format!(
            "{}/v1/session/{}/abort",
            runtime,
            urlencoding::encode(&session_id)
        ))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => Json(v).into_response(),
            Err(_) => Json(json!({"sessionId": session_id, "status": "stopped"})).into_response(),
        },
        Ok(r) => forward_runtime_error(r).await,
        Err(e) => runtime_unavailable(e),
    }
}

// ─── GET /api/page-agent/status/:session_id ─────────────────────────────────

async fn page_agent_status(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Response {
    let runtime = runtime_base(&state);
    let client = crate::net_client::client();

    // The gizzi runtime has no dedicated status endpoint; proxy to the stream
    // health-check-style and return a stub status. Phase 2 can implement a
    // persistent session store.
    match client
        .get(format!("{}/health", runtime))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => (
            StatusCode::OK,
            Json(json!({
                "sessionId": session_id,
                "status": "running",
            })),
        )
            .into_response(),
        Ok(r) => forward_runtime_error(r).await,
        Err(e) => runtime_unavailable(e),
    }
}

// ─── POST /api/page-agent/config ────────────────────────────────────────────

async fn page_agent_config(Json(body): Json<serde_json::Value>) -> Response {
    // Phase 1 simply echoes the config back as a validation/save stub. The
    // desktop surface reads config from the extension directly; the web surface
    // can persist it here in a later phase.
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "config": body,
        })),
    )
        .into_response()
}
