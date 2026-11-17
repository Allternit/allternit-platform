//! Connector search (Primitives Wave D, D2): gizzi's `connector_search` tool.
//!
//! Cloud connectors and their OAuth tokens live here, in allternit-api; the
//! runtime only ever reaches a connected one as proxied MCP tools
//! (`mcp__<server>__<connector>__<tool>`). Whose connectors they are comes
//! from the session id (the same owner check as
//! `agent_user_tools::resolve_owner`), never from the model. No token, link or
//! code is ever returned.
//!
//! * `POST /bot-routing/connector/search` `{sessionId, query, limit ≤10}` →
//!   `{hits: [{id, name, group, state, available, reason?, serverName?}]}`.
//!   `state` is `connected`, `not_connected` or `needs_reconnect`;
//!   `serverName` is the proxied MCP server a connected hit's tools mount
//!   under, so gizzi can `load` them.
//!
//! W2 stub: answers 501 `not_built` until lane CS fills it.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use crate::AppState;

pub fn agent_connector_tools_router() -> Router<Arc<AppState>> {
    Router::new().route("/bot-routing/connector/search", post(not_built))
}

async fn not_built() -> Response {
    (StatusCode::NOT_IMPLEMENTED, Json(json!({ "error": "connector search is not built yet", "code": "not_built" }))).into_response()
}
