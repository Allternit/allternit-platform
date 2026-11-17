//! Past chats (Primitives Wave D, D1 recall): gizzi's `thread_read`,
//! `thread_recent` and `thread_wait` tools.
//!
//! Unlike the bot thread tools (`thread_bot_tools`), these work in every
//! session the caller owns: a bot's chat, a plain chat, a Gizzi Code session.
//! Whose session it is comes from the session id (the same owner check as
//! `agent_user_tools::resolve_owner`), never from the model. Incognito chats
//! are never returned. Transcripts stay in gizzi's local session store; this
//! API only maps a thread to its session and says where it lives.
//!
//! * `POST /bot-routing/thread/resolve` `{sessionId, thread}` (`thread` is an
//!   id or `"current"`) → `{thread: {id, title, status, sessionId, local,
//!   computer?, summary?, lastActivityAt?}}`.
//! * `POST /bot-routing/thread/recent` `{sessionId, limit ≤20, before?, after?, project?}`
//!   → `{threads: [...]}`, newest first.
//! * `POST /bot-routing/thread/wait` `{sessionId, targets ≤8 [{thread, afterCursor?}], timeoutS 0-600}`
//!   → `{woke, timedOut, statuses}`: the first target that finished a turn or
//!   now needs the user (`bot_events`); `timeoutS` 0 is a snapshot.
//!
//! W1 stub: every route answers 501 `not_built` until lane TS fills them.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use crate::AppState;

pub fn thread_read_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/thread/resolve", post(not_built))
        .route("/bot-routing/thread/recent", post(not_built))
        .route("/bot-routing/thread/wait", post(not_built))
}

async fn not_built() -> Response {
    (StatusCode::NOT_IMPLEMENTED, Json(json!({ "error": "past-chat routes are not built yet", "code": "not_built" }))).into_response()
}
