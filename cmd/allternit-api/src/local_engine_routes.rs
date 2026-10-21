//! Local Engine API proxy routes.
//!
//! Proxies `/api/local-engine/*` to the Allternit Local Engine controller
//! running at `LOCAL_ENGINE_URL` (default `http://127.0.0.1:8090`).

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::get_user;
use crate::AppState;

fn local_engine_url() -> String {
    std::env::var("LOCAL_ENGINE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:3015".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "Unauthorized" })),
    )
        .into_response()
}

pub fn local_engine_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/local-engine/health", get(proxy_health))
        .route("/local-engine/status", get(proxy_status))
        .route("/local-engine/catalog", get(proxy_catalog))
        .route("/local-engine/catalog/refresh", post(proxy_catalog_refresh))
        .route("/local-engine/assess", post(proxy_assess))
        .route("/local-engine/assess/batch", post(proxy_assess_batch))
        .route("/local-engine/recommend", post(proxy_recommend))
        .route("/local-engine/models", get(proxy_list_models))
        .route("/local-engine/models/import", post(proxy_import_model))
        .route("/local-engine/models/download", post(proxy_download_model))
        .route("/local-engine/runtimes", get(proxy_list_runtimes))
        .route("/local-engine/runtimes/launch", post(proxy_launch_runtime))
        .route("/local-engine/runtimes/:id/stop", post(proxy_stop_runtime))
        .route("/local-engine/v1/models", get(proxy_openai_models))
        .route(
            "/local-engine/v1/chat/completions",
            post(proxy_chat_completions),
        )
}

// ─── Simple GET proxies ───────────────────────────────────────────────────────

async fn proxy_health(headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_get("/health").await
}

async fn proxy_status(State(_state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_get("/status").await
}

async fn proxy_catalog(
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_get_with_query("/catalog", params).await
}

async fn proxy_catalog_refresh(headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/catalog/refresh", Bytes::new()).await
}

async fn proxy_assess(headers: HeaderMap, body: Bytes) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/assess", body).await
}

async fn proxy_assess_batch(headers: HeaderMap, body: Bytes) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/assess/batch", body).await
}

async fn proxy_recommend(headers: HeaderMap, body: Bytes) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/recommend", body).await
}

async fn proxy_list_models(headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_get("/models").await
}

async fn proxy_list_runtimes(headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_get("/runtimes").await
}

async fn proxy_openai_models(headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_get("/v1/models").await
}

// ─── Body-bearing POST proxies ────────────────────────────────────────────────

async fn proxy_import_model(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/models/import", body).await
}

async fn proxy_download_model(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/models/download", body).await
}

async fn proxy_launch_runtime(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/runtimes/launch", body).await
}

async fn proxy_stop_runtime(Path(id): Path<String>, headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post(&format!("/runtimes/{}/stop", id), Bytes::new()).await
}

async fn proxy_chat_completions(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    proxy_post("/v1/chat/completions", body).await
}

// ─── Generic proxy helpers ────────────────────────────────────────────────────

async fn proxy_get_with_query(
    path: &str,
    params: std::collections::HashMap<String, String>,
) -> Response {
    if params.is_empty() {
        return proxy_get(path).await;
    }
    let qs: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    proxy_get(&format!("{path}?{}", qs.join("&"))).await
}

async fn proxy_get(path: &str) -> Response {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!("{}{}", local_engine_url(), path);
    match client.get(&url).send().await {
        Ok(res) => forward_response(res).await,
        Err(err) => {
            warn!(error = %err, url = %url, "Local Engine proxy GET failed");
            engine_unavailable(err)
        }
    }
}

async fn proxy_post(path: &str, body: Bytes) -> Response {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let url = format!("{}{}", local_engine_url(), path);
    match client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(res) => forward_response(res).await,
        Err(err) => {
            warn!(error = %err, url = %url, "Local Engine proxy POST failed");
            engine_unavailable(err)
        }
    }
}

async fn forward_response(res: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = res.headers().clone();
    let body = match res.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!(error = %err, "Failed to read Local Engine response body");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "failed to read upstream response" })),
            )
                .into_response();
        }
    };

    let mut builder = Response::builder().status(status);
    for (name, value) in headers.iter() {
        if is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }

    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn engine_unavailable(err: reqwest::Error) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": "local_engine_unavailable",
            "message": err.to_string(),
            "note": "Ensure the Local Engine controller is running."
        })),
    )
        .into_response()
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
