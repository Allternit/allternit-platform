//! Transparent proxy to the TypeScript office engine (`services/office-engine`).
//!
//! The office engine is a standalone Hono service that parses and re-serializes
//! Office documents. This router only forwards the raw request body plus the
//! `x-office-filename` header so the gateway never becomes a second source of
//! document state, mirroring the `orchestrator_routes` proxy model.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::office_session_guard::{forward_session_op, OPEN_BODY_LIMIT};
use crate::AppState;

pub fn office_engine_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/office/parse", post(proxy_office_parse))
        .route("/office/roundtrip", post(proxy_office_roundtrip))
        .route("/office/pptx/parse", post(proxy_office_pptx_parse))
        .route("/office/pptx/roundtrip", post(proxy_office_pptx_roundtrip))
        .route("/office/extract", post(proxy_office_extract))
        .route("/office/markdown", post(proxy_office_markdown))
        .route("/office/markdown-url", post(proxy_office_markdown_url))
        .route("/office/xlsx/parse", post(proxy_office_xlsx_parse))
        .route("/office/xlsx/recalc", post(proxy_office_xlsx_recalc))
        .route("/office/xlsx/read", post(proxy_office_xlsx_read))
        .route(
            "/office/xlsx/session/{op}",
            post(proxy_office_xlsx_session).layer(DefaultBodyLimit::max(OPEN_BODY_LIMIT)),
        )
}

/// v1 mount of the same proxy at `/api/v1/office/engines/*`, per the add-in
/// convergence plan: GenOffice engine routes coexist with the legacy
/// `/api/v1/office/cli/*` (OfficeCLI stays operational until the engine
/// backend is proven in production).
pub fn office_engine_v1_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/office/engines/parse", post(proxy_office_parse))
        .route("/office/engines/roundtrip", post(proxy_office_roundtrip))
        .route("/office/engines/pptx/parse", post(proxy_office_pptx_parse))
        .route("/office/engines/pptx/roundtrip", post(proxy_office_pptx_roundtrip))
        .route("/office/engines/extract", post(proxy_office_extract))
        .route("/office/engines/markdown", post(proxy_office_markdown))
        .route("/office/engines/markdown-url", post(proxy_office_markdown_url))
        .route("/office/engines/xlsx/parse", post(proxy_office_xlsx_parse))
        .route("/office/engines/xlsx/recalc", post(proxy_office_xlsx_recalc))
        .route("/office/engines/xlsx/read", post(proxy_office_xlsx_read))
        .route(
            "/office/engines/xlsx/session/{op}",
            post(proxy_office_xlsx_session).layer(DefaultBodyLimit::max(OPEN_BODY_LIMIT)),
        )
}

/// POST /api/office/parse → POST {OFFICE_ENGINE_URL}/parse
async fn proxy_office_parse(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/parse", headers, body).await
}

/// POST /api/office/roundtrip → POST {OFFICE_ENGINE_URL}/docx/roundtrip
async fn proxy_office_roundtrip(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/docx/roundtrip", headers, body).await
}

/// POST /api/office/pptx/parse → POST {OFFICE_ENGINE_URL}/pptx/parse
async fn proxy_office_pptx_parse(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/pptx/parse", headers, body).await
}

/// POST /api/office/pptx/roundtrip → POST {OFFICE_ENGINE_URL}/pptx/roundtrip
async fn proxy_office_pptx_roundtrip(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/pptx/roundtrip", headers, body).await
}

/// POST /api/office/extract → POST {OFFICE_ENGINE_URL}/extract
async fn proxy_office_extract(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/extract", headers, body).await
}

/// POST /api/office/markdown → POST {OFFICE_ENGINE_URL}/markdown
async fn proxy_office_markdown(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/markdown", headers, body).await
}

/// POST /api/office/markdown-url → POST {OFFICE_ENGINE_URL}/markdown-url
/// URL → Markdown conversion; the JSON `{url}` body is forwarded verbatim by
/// the shared proxy (content-type passthrough), same as the raw byte routes.
async fn proxy_office_markdown_url(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/markdown-url", headers, body).await
}

/// POST /api/office/xlsx/parse → POST {OFFICE_ENGINE_URL}/xlsx/parse
async fn proxy_office_xlsx_parse(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/xlsx/parse", headers, body).await
}

/// POST /api/office/xlsx/recalc → POST {OFFICE_ENGINE_URL}/xlsx/recalc
async fn proxy_office_xlsx_recalc(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/xlsx/recalc", headers, body).await
}

/// POST /api/office/xlsx/read → POST {OFFICE_ENGINE_URL}/xlsx/read
async fn proxy_office_xlsx_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_office_engine(state, "/xlsx/read", headers, body).await
}

/// POST /api/office/xlsx/session/:op → POST {OFFICE_ENGINE_URL}/xlsx/session/:op
/// Workbook session API for the sheets editor (open/range/formulas/
/// session-recalc/save/close). Sessions belong to whoever opened them: see
/// `office_session_guard`.
async fn proxy_office_xlsx_session(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    axum::extract::Path(op): axum::extract::Path<String>,
    body: Bytes,
) -> Response {
    let Some(Extension(user)) = user else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response();
    };
    let reply =
        forward_session_op(&state.config.office_engine_url(), &user.user_id, &op, body.to_vec()).await;
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = (status, reply.body).into_response();
    if let Some(value) = reply.content_type.and_then(|value| value.parse().ok()) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

async fn proxy_office_engine(
    state: Arc<AppState>,
    upstream_path: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let target = format!(
        "{}{}",
        state.config.office_engine_url().trim_end_matches('/'),
        upstream_path,
    );

    let client = crate::net_client::client();
    let mut request = client.post(&target);
    if let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        request = request.header(reqwest::header::CONTENT_TYPE, content_type);
    }
    if let Some(filename) = headers
        .get("x-office-filename")
        .and_then(|value| value.to_str().ok())
    {
        request = request.header("x-office-filename", filename);
    }
    if !body.is_empty() {
        request = request.body(body.to_vec());
    }

    let upstream = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            warn!(target = %target, error = %error, "Office engine proxy failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "office_engine_unavailable",
                    "message": format!("Cannot reach the office engine: {error}"),
                })),
            )
                .into_response();
        }
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "invalid_office_engine_response", "message": error.to_string() })),
            )
                .into_response();
        }
    };

    let mut response = (status, bytes).into_response();
    if let Some(value) =
        content_type.and_then(|value| value.to_str().ok().and_then(|text| text.parse().ok()))
    {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}
