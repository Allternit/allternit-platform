//! MCP edge: Server Cards (plan v3 lane 48 fills this file).
//!
//! `/.well-known/mcp` style server cards and `ai-catalog.json`. The router
//! below is NOT merged into `mcp_edge::routes()` yet; lane 48 merges it and
//! replaces the handler. Until then the paths answer 404.

use std::sync::Arc;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::ApiState;

/// Card routes. Unregistered until lane 48.
#[allow(dead_code)]
pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/.well-known/mcp", get(not_ready))
        .route("/.well-known/ai-catalog.json", get(not_ready))
}

#[allow(dead_code)]
async fn not_ready() -> Response {
    StatusCode::NOT_FOUND.into_response()
}
