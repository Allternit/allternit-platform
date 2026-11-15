//! Workbook sessions on this host's office engine, for people with no computer
//! (the iPhone app, and the web). Unlike the rest of the office namespace these
//! aren't relayed to the caller's node: they may not have one.
//!
//! Every session belongs to whoever opened it: see `office_session_guard`.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::json;

use crate::{auth, ApiState};

#[path = "../../../allternit-api/src/office_session_guard.rs"]
pub mod office_session_guard;
use office_session_guard::{forward_session_op, OPEN_BODY_LIMIT};

const DEFAULT_ENGINE_URL: &str = "http://127.0.0.1:8099";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route(
        "/api/v1/office/engines/xlsx/session/:op",
        post(session_op).layer(DefaultBodyLimit::max(OPEN_BODY_LIMIT)),
    )
}

async fn session_op(
    State(state): State<Arc<ApiState>>,
    Path(op): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let user = match auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(user) => user,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response();
        }
    };
    let engine_url = std::env::var("OFFICE_ENGINE_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_ENGINE_URL.to_string());
    let reply = forward_session_op(&engine_url, &user.id, &op, body.to_vec()).await;
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = (status, reply.body).into_response();
    if let Some(value) = reply.content_type.and_then(|value| value.parse().ok()) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}
