//! Model Lab training API routes.
//!
//! Proxies authenticated requests to the local Python Model Lab service
//! (default `http://127.0.0.1:9020`). The service wraps the Apache 2.0
//! Unsloth core library for LoRA/QLoRA/full fine-tune/DPO training.

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::auth::get_user;
use crate::AppState;

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "Unauthorized"})),
    )
        .into_response()
}

fn service_url(_state: &AppState) -> String {
    std::env::var("ALLTERNIT_MODEL_TRAINING_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:9020".to_string())
}

/// Build a request to the Model Lab Python service and stream the response back.
async fn proxy_to_model_lab(
    state: &AppState,
    headers: &HeaderMap,
    method: reqwest::Method,
    path: &str,
    query: Option<HashMap<String, String>>,
    body: Option<serde_json::Value>,
) -> Response {
    if get_user(headers).is_none() {
        return unauthorized();
    }

    let base = service_url(state).trim_end_matches('/').to_string();
    let url = format!("{}/{}", base, path.trim_start_matches('/'));
    let client = crate::net_client::client();
    let mut req = client.request(method, &url);
    if let Some(q) = query {
        req = req.query(&q);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }

    match req.timeout(std::time::Duration::from_secs(30)).send().await {
        Ok(upstream) => {
            let status =
                StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body = match upstream.bytes().await {
                Ok(b) => b,
                Err(err) => {
                    warn!("model-lab proxy body error: {}", err);
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"error": "model_lab_body_read_error", "message": err.to_string()})),
                    )
                        .into_response();
                }
            };
            (status, Body::from(body)).into_response()
        }
        Err(err) => {
            warn!("model-lab proxy error: {}", err);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "model_lab_unreachable",
                    "message": err.to_string(),
                    "service": base,
                })),
            )
                .into_response()
        }
    }
}

pub fn model_training_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/model-training/health", get(model_training_health))
        .route("/model-training/base-models", get(list_base_models))
        .route("/model-training/datasets", post(upload_dataset))
        .route("/model-training/jobs", get(list_jobs).post(create_job))
        .route("/model-training/jobs/:id", get(get_job))
        .route("/model-training/jobs/:id/cancel", post(cancel_job))
        .route("/model-training/jobs/:id/checkpoints", get(list_checkpoints))
        .route("/model-training/jobs/:id/export", post(export_job))
        .route("/model-training/exports/:export_id", get(get_export))
}

async fn model_training_health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    proxy_to_model_lab(&state, &headers, reqwest::Method::GET, "/health", None, None).await
}

async fn list_base_models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    proxy_to_model_lab(
        &state,
        &headers,
        reqwest::Method::GET,
        "/base-models",
        None,
        None,
    )
    .await
}

async fn upload_dataset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    // The dataset upload is multipart/form-data; proxy it as raw bytes.
    if get_user(&headers).is_none() {
        return unauthorized();
    }

    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(err) => {
            warn!("model-lab upload body read error: {}", err);
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "body_read_error", "message": err.to_string()})),
            )
                .into_response();
        }
    };

    let base = service_url(&state).trim_end_matches('/').to_string();
    let client = crate::net_client::client();
    let mut req = client
        .post(format!("{}/datasets", base))
        .header(
            reqwest::header::CONTENT_TYPE,
            headers
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("multipart/form-data"),
        )
        .body(bytes);

    match req.timeout(std::time::Duration::from_secs(120)).send().await {
        Ok(upstream) => {
            let status =
                StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let body = upstream.bytes().await.unwrap_or_default();
            (status, Body::from(body)).into_response()
        }
        Err(err) => {
            warn!("model-lab upload proxy error: {}", err);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "model_lab_unreachable", "message": err.to_string()})),
            )
                .into_response()
        }
    }
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    proxy_to_model_lab(&state, &headers, reqwest::Method::GET, "/jobs", Some(query), None).await
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    proxy_to_model_lab(
        &state,
        &headers,
        reqwest::Method::POST,
        "/jobs",
        None,
        Some(payload),
    )
    .await
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    proxy_to_model_lab(
        &state,
        &headers,
        reqwest::Method::GET,
        &format!("/jobs/{}", urlencoding::encode(&id)),
        None,
        None,
    )
    .await
}

async fn cancel_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    proxy_to_model_lab(
        &state,
        &headers,
        reqwest::Method::POST,
        &format!("/jobs/{}/cancel", urlencoding::encode(&id)),
        None,
        None,
    )
    .await
}

async fn list_checkpoints(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    proxy_to_model_lab(
        &state,
        &headers,
        reqwest::Method::GET,
        &format!("/jobs/{}/checkpoints", urlencoding::encode(&id)),
        None,
        None,
    )
    .await
}

async fn export_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    proxy_to_model_lab(
        &state,
        &headers,
        reqwest::Method::POST,
        &format!("/jobs/{}/export", urlencoding::encode(&id)),
        None,
        Some(payload),
    )
    .await
}

async fn get_export(
    State(state): State<Arc<AppState>>,
    Path(export_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    proxy_to_model_lab(
        &state,
        &headers,
        reqwest::Method::GET,
        &format!("/exports/{}", urlencoding::encode(&export_id)),
        None,
        None,
    )
    .await
}
