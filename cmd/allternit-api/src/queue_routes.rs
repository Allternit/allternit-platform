//! Cowork Agent Queue Routes
//!
//! Claiming, starting and completing agent work on a task. Since the cowork
//! fold (stream F9) a queue item is a claim on the task's Factory node,
//! written through the Gate (`cowork_nodes`), and only the signed-in user's
//! own tasks are visible. The HTTP shapes are unchanged for one release.
//!
//! A completed item puts its task in review: it is never marked done by the
//! worker's word. Done/failed rows from before the fold stay readable here.

use axum::extract::State;
use axum::{
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::auth::get_user;
use crate::cowork_nodes::{self, CoworkError};
use crate::AppState;

pub use crate::cowork_nodes::QueueItem;

#[derive(Debug, Deserialize)]
pub struct ListQueueQuery {
    pub workspace_id: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClaimQueueRequest {
    pub agent_id: String,
    pub agent_role: Option<String>,
    pub workspace_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateQueueRequest {
    pub id: Option<String>,
    pub task_id: String,
    pub agent_id: Option<String>,
    pub agent_role: Option<String>,
    /// Ignored: a new item is always `pending`. Kept so old bodies parse.
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteQueueRequest {
    pub result: Option<String>,
    pub error: Option<String>,
}

pub fn queue_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/queue", get(list_queue).post(create_queue))
        .route("/queue/claim", post(claim_queue))
        .route("/queue/:id/start", post(start_queue))
        .route("/queue/:id/complete", post(complete_queue))
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response()
}

fn cowork_error(e: CoworkError) -> Response {
    (e.http_status(), Json(e.body())).into_response()
}

fn db_error(e: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("Database error: {e}")}))).into_response()
}

/// Refresh the task's read-model row after a queue write.
async fn refresh_row(state: &AppState, user_id: &str, queue_id: &str) {
    if let (Some(task), Ok(conn)) = (cowork_nodes::task_for_queue(&state.rails, user_id, queue_id).await, state.db.connect()) {
        if let Err(e) = cowork_nodes::write_row(&conn, &task) {
            tracing::error!("tasks read model: {}", e);
        }
    }
}

async fn list_queue(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQueueQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let events = match cowork_nodes::ledger_events(&state.rails).await {
        Ok(e) => e,
        Err(e) => return cowork_error(e),
    };
    let ws = query.workspace_id.as_deref();
    let status = query.status.as_deref();
    let mut items = cowork_nodes::list_queue(&events, &user.user_id, ws, status);
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };
    match cowork_nodes::legacy_queue_rows(&conn, &user.user_id, ws, status) {
        Ok(rows) => {
            let known: std::collections::HashSet<String> = items.iter().map(|q| q.id.clone()).collect();
            items.extend(rows.into_iter().filter(|q| !known.contains(&q.id)));
        }
        Err(e) if e.to_string().contains("no such table") => {}
        Err(e) => return db_error(e),
    }
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    (StatusCode::OK, Json(items)).into_response()
}

async fn claim_queue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ClaimQueueRequest>,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    if payload.agent_id.trim().is_empty() {
        return cowork_error(CoworkError::usage("agent_id is required", "Send the claiming agent's id."));
    }
    match cowork_nodes::claim(&state.rails, &user.user_id, &payload.agent_id, payload.agent_role.as_deref(), payload.workspace_id.as_deref()).await {
        Ok(Some(item)) => {
            refresh_row(&state, &user.user_id, &item.id).await;
            (StatusCode::OK, Json(json!(item))).into_response()
        }
        // Nothing pending: `null`, as before.
        Ok(None) => (StatusCode::OK, Json(serde_json::Value::Null)).into_response(),
        Err(e) => cowork_error(e),
    }
}

async fn start_queue(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    match cowork_nodes::start(&state.rails, &user.user_id, &id).await {
        Ok(item) => {
            refresh_row(&state, &user.user_id, &id).await;
            (StatusCode::OK, Json(json!(item))).into_response()
        }
        Err(e) => cowork_error(e),
    }
}

async fn complete_queue(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CompleteQueueRequest>,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    match cowork_nodes::complete(&state.rails, &user.user_id, &id, body.result.as_deref(), body.error.as_deref()).await {
        Ok(item) => {
            refresh_row(&state, &user.user_id, &id).await;
            (StatusCode::OK, Json(json!(item))).into_response()
        }
        Err(e) => cowork_error(e),
    }
}

async fn create_queue(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<CreateQueueRequest>,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let queue_id = payload.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };
    match cowork_nodes::enqueue(&state.rails, &conn, &user.user_id, &payload.task_id, &queue_id, payload.agent_id.as_deref(), payload.agent_role.as_deref()).await {
        Ok(item) => {
            refresh_row(&state, &user.user_id, &queue_id).await;
            (StatusCode::CREATED, Json(json!(item))).into_response()
        }
        Err(e) => cowork_error(e),
    }
}
