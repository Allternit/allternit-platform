//! The cowork Tasks board, served by allternit-api under `/api/factory`.
//!
//! Cowork tasks are nodes in the signed-in user's standing Tasks DAGs, kept
//! in this process's Factory ledger (`cowork_nodes`). Like approvals, these
//! routes are owned here, not proxied to the engine, because the tasks live
//! with the account (web, phone and Desktop all read the same list):
//!
//! * `GET /api/factory/tasks/board?workspace=<id>` → `Board` (the engine's
//!   board over the Tasks DAG; an empty workspace is the personal list)
//! * `GET /api/factory/tasks/nodes/:nodeId` → `NodePage`
//!
//! Static routes, so they win over the `/api/factory/*rest` engine proxy.

use std::sync::Arc;

use allternit_factory_engine::workspace::node_page;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::auth::get_user;
use crate::cowork_nodes::{self, CoworkError};
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/factory/tasks/board", get(board_h))
        .route("/factory/tasks/nodes/:node", get(node_h))
}

fn unauthorized() -> Response {
    let e = CoworkError::usage("sign in to see your tasks", "Sign in, then try again.");
    (axum::http::StatusCode::UNAUTHORIZED, Json(e.body())).into_response()
}

fn err(e: CoworkError) -> Response {
    (e.http_status(), Json(e.body())).into_response()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct BoardQuery {
    workspace: Option<String>,
}

async fn board_h(State(state): State<Arc<AppState>>, Query(q): Query<BoardQuery>, headers: HeaderMap) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let events = match cowork_nodes::ledger_events(&state.rails).await {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    match cowork_nodes::tasks_board(&state.rails, &events, &user.user_id, q.workspace.as_deref().unwrap_or("")) {
        Ok(b) => Json(b).into_response(),
        Err(e) => err(e),
    }
}

async fn node_h(State(state): State<Arc<AppState>>, Path(node): Path<String>, headers: HeaderMap) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let events = match cowork_nodes::ledger_events(&state.rails).await {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    let task_id = node.strip_prefix("task-").unwrap_or(&node);
    let Some((dag, _)) = cowork_nodes::find(&events, &user.user_id, task_id) else {
        return err(CoworkError::not_found(format!("task node {node} not found")));
    };
    match node_page::build(&state.rails.root_dir, &events, &dag, &cowork_nodes::node_id(task_id)) {
        Ok(p) => Json(p).into_response(),
        Err(e) => err(CoworkError::not_found(e.to_string())),
    }
}
