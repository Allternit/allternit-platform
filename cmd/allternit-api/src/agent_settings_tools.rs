//! Settings (Primitives Wave D, D2): gizzi's `settings` tool.
//!
//! Keys and their risk class come from `contracts/settings/definitions.json`.
//! The risk gate is enforced here, on the server:
//!
//! * `safe` keys are applied (account keys through the existing preference
//!   functions: `/agent-preferences`, `/cowork-preferences`); per-device keys
//!   come back `apply_on_device` for the client that sent the turn.
//! * `confirm` keys (security, privacy, instructions) are never applied by a
//!   runtime call; they come back `needs_confirmation`, and are refused for
//!   runs an automation started.
//! * `locked` keys (money, account) come back `locked` with a Settings link.
//!
//! Whose settings they are comes from the session id (the same owner check as
//! `agent_user_tools::resolve_owner`), never from the model.
//!
//! * `POST /bot-routing/settings/read` `{sessionId, keys?}` → `{settings: [...]}`.
//! * `POST /bot-routing/settings/write` `{sessionId, toolCallId, changes ≤5 [{key, value}]}`
//!   → `{changes: [{key, outcome, previous?, value?, reason?, settings_path?}]}`.
//!
//! W2 stub: every route answers 501 `not_built` until lane SS fills them.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use crate::AppState;

pub fn agent_settings_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/settings/read", post(not_built))
        .route("/bot-routing/settings/write", post(not_built))
}

async fn not_built() -> Response {
    (StatusCode::NOT_IMPLEMENTED, Json(json!({ "error": "settings routes are not built yet", "code": "not_built" }))).into_response()
}
