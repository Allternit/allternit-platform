//! Eval routes: a thin proxy from `/api/v1/agent-evals/*` to the engine's
//! `/v1/eval/*` (eval loop, CONTRACT.md sections 6 and 7).
//!
//! Unlike the session proxies, these pass the engine's status code and JSON
//! body through unchanged (201, 202, 204, 404, 409, 422 all carry meaning for
//! the clients). Auth and the engine address come from the session routes'
//! `gizzi_client` and `gizzi_base`.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::agent_session_routes::{gizzi_base, gizzi_client};
use crate::AppState;

async fn proxy(
    headers: &HeaderMap,
    method: reqwest::Method,
    path: String,
    query: Option<String>,
    body: Option<Bytes>,
) -> Response {
    let client = gizzi_client(headers);
    let mut url = format!("{}{}", gizzi_base(), path);
    if let Some(query) = query.filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(&query);
    }
    let mut request = client.request(method, &url);
    if let Some(body) = body.filter(|b| !b.is_empty()) {
        request = request.header(reqwest::header::CONTENT_TYPE, "application/json").body(body);
    }
    let upstream = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            warn!("Gizzi eval request failed: {}", error);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Gizzi request failed: {}", error) })),
            )
                .into_response();
        }
    };
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!("Failed to read Gizzi eval response: {}", error);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Failed to read Gizzi response: {}", error) })),
            )
                .into_response();
        }
    };
    if bytes.is_empty() {
        return status.into_response();
    }
    let content_type = content_type.unwrap_or_else(|| "application/json".to_string());
    (status, [(header::CONTENT_TYPE, content_type)], bytes).into_response()
}

async fn list_cases(headers: HeaderMap, RawQuery(query): RawQuery) -> Response {
    proxy(&headers, reqwest::Method::GET, "/v1/eval/cases".to_string(), query, None).await
}

async fn create_case(headers: HeaderMap, body: Bytes) -> Response {
    proxy(&headers, reqwest::Method::POST, "/v1/eval/cases".to_string(), None, Some(body)).await
}

async fn get_case(headers: HeaderMap, Path(id): Path<String>) -> Response {
    let path = format!("/v1/eval/cases/{}", urlencoding::encode(&id));
    proxy(&headers, reqwest::Method::GET, path, None, None).await
}

async fn delete_case(headers: HeaderMap, Path(id): Path<String>) -> Response {
    let path = format!("/v1/eval/cases/{}", urlencoding::encode(&id));
    proxy(&headers, reqwest::Method::DELETE, path, None, None).await
}

async fn list_suites(headers: HeaderMap) -> Response {
    proxy(&headers, reqwest::Method::GET, "/v1/eval/suites".to_string(), None, None).await
}

async fn estimate_run(headers: HeaderMap, body: Bytes) -> Response {
    proxy(&headers, reqwest::Method::POST, "/v1/eval/runs/estimate".to_string(), None, Some(body)).await
}

async fn list_runs(headers: HeaderMap, RawQuery(query): RawQuery) -> Response {
    proxy(&headers, reqwest::Method::GET, "/v1/eval/runs".to_string(), query, None).await
}

async fn create_run(headers: HeaderMap, body: Bytes) -> Response {
    proxy(&headers, reqwest::Method::POST, "/v1/eval/runs".to_string(), None, Some(body)).await
}

async fn get_run(headers: HeaderMap, Path(id): Path<String>) -> Response {
    let path = format!("/v1/eval/runs/{}", urlencoding::encode(&id));
    proxy(&headers, reqwest::Method::GET, path, None, None).await
}

async fn cancel_run(headers: HeaderMap, Path(id): Path<String>) -> Response {
    let path = format!("/v1/eval/runs/{}/cancel", urlencoding::encode(&id));
    proxy(&headers, reqwest::Method::POST, path, None, Some(Bytes::new())).await
}

pub fn agent_eval_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent-evals/cases", get(list_cases).post(create_case))
        .route("/agent-evals/cases/:id", get(get_case).delete(delete_case))
        .route("/agent-evals/suites", get(list_suites))
        // A literal segment, so it wins over `/agent-evals/runs/:id`
        // (matchit prefers static segments).
        .route("/agent-evals/runs/estimate", post(estimate_run))
        .route("/agent-evals/runs", get(list_runs).post(create_run))
        .route("/agent-evals/runs/:id", get(get_run))
        .route("/agent-evals/runs/:id/cancel", post(cancel_run))
}
