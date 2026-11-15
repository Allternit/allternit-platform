//! OpenAI-compatible chat completions proxy.
//!
//! Routes `/v1/chat/completions` to a healthy local runtime serving the
//! requested model. Streams pass through as they arrive. The proxy always
//! asks the backend for `usage` and adds an `allternit` object with real
//! numbers (`tok_per_sec`, `first_token_ms`, `ctx_used`, `ctx_max`,
//! `mem_bytes`) to the reply (non-stream) or the usage chunk (stream); see
//! `runtime::usage`. Each request also resets the runtime's idle timer.

use crate::runtime::backends::llamacpp::chat_completions_url;
use crate::runtime::manager::process_mem_bytes;
use crate::runtime::usage::{self, SseOut, SseRewriter, UsageStats};
use crate::AppState;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;

/// Create the OpenAI-compatible chat router.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(proxy_chat_completions))
        .with_state(state)
}

async fn proxy_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut req = match serde_json::from_slice::<Value>(&body) {
        Ok(v) if v.get("model").and_then(Value::as_str).is_some() => v,
        Ok(_) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid request body: missing field `model`" })))
                .into_response();
        }
        Err(err) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("invalid request body: {}", err) })))
                .into_response();
        }
    };
    let model_id = req["model"].as_str().unwrap_or_default().to_string();

    let runtime = match state.manager.find_running_runtime_by_model_id(&model_id).await {
        Some(rt) => rt,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "no_running_runtime",
                    "message": format!("No healthy runtime is serving model '{}'", model_id)
                })),
            )
                .into_response();
        }
    };
    // Held until the reply (or the stream) is done: the idle timer waits for it.
    let guard = state.manager.begin_request(&runtime.id).await;

    let (stream, client_wants_usage) = usage::prepare_request(&mut req);
    let ctx_max = runtime.recipe_value.ctx_max();
    let pid = runtime.pid;
    let ext = move |s: &UsageStats| usage::extension(s, ctx_max, pid.and_then(process_mem_bytes));

    let url = chat_completions_url(runtime.port);
    // No overall timeout: a long generation streams for as long as it needs.
    // Connect and per-read limits catch a dead backend.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_default();
    let mut upstream_req = client.post(&url).body(serde_json::to_vec(&req).unwrap_or_else(|_| body.to_vec()));
    for name in ["content-type", "accept", "authorization"] {
        if let Some(value) = headers.get(name) {
            upstream_req = upstream_req.header(name, value);
        }
    }

    let started = Instant::now();
    let res = match upstream_req.send().await {
        Ok(res) => res,
        Err(err) => {
            warn!(error = %err, %url, "chat completions proxy failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "runtime_unavailable", "message": err.to_string() })),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = res.headers().clone();
    let mut builder = Response::builder().status(status);
    for (name, value) in headers.iter() {
        // Length changes when the body is rewritten.
        if is_hop_by_hop_header(name.as_str()) || name.as_str().eq_ignore_ascii_case("content-length") {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }

    let is_sse = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));

    if stream && is_sse && status.is_success() {
        let manager = state.manager.clone();
        let runtime_id = runtime.id.clone();
        let mut upstream = res.bytes_stream();
        let body = async_stream::stream! {
            let _guard = guard;
            let mut rw = SseRewriter::new(client_wants_usage);
            while let Some(chunk) = upstream.next().await {
                match chunk {
                    Ok(bytes) => {
                        let ms = started.elapsed().as_millis() as u64;
                        for out in rw.push(&bytes, ms, &ext) {
                            if let SseOut::Emit(text) = out {
                                yield Ok::<Bytes, std::io::Error>(Bytes::from(text));
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "runtime stream ended early");
                        break;
                    }
                }
            }
            if let Some(SseOut::Emit(text)) = rw.finish(started.elapsed().as_millis() as u64, &ext) {
                yield Ok(Bytes::from(text));
            }
            if let Some(stats) = rw.stats {
                manager.record_usage(&runtime_id, stats).await;
            }
        };
        return builder
            .body(Body::from_stream(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    let body_bytes = match res.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!(error = %err, "failed to read runtime response body");
            return (StatusCode::BAD_GATEWAY, Json(json!({ "error": "failed to read runtime response" })))
                .into_response();
        }
    };
    let elapsed = started.elapsed().as_millis() as u64;
    let out = match serde_json::from_slice::<Value>(&body_bytes) {
        Ok(mut reply) if status.is_success() => match usage::stats_from(&reply, None, elapsed) {
            Some(stats) => {
                state.manager.record_usage(&runtime.id, stats).await;
                reply["allternit"] = ext(&stats);
                Bytes::from(serde_json::to_vec(&reply).unwrap_or_default())
            }
            None => body_bytes,
        },
        _ => body_bytes,
    };
    drop(guard);
    builder
        .body(Body::from(out))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn is_hop_by_hop_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}
