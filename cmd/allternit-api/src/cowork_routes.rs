use axum::extract::Extension;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::get_user;
use crate::auth::AuthUser;
use crate::agent_session_routes::gizzi_client;
use crate::v1_routes::gizzi_base;
use crate::AppState;

fn unauthorized() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "Unauthorized"})),
    )
        .into_response()
}

pub fn cowork_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/cowork", get(cowork_status))
        .route("/cowork/sessions", get(list_sessions).post(create_session))
        .route(
            "/cowork/sessions/:id",
            get(get_session)
                .put(update_session)
                .patch(update_session)
                .delete(delete_session),
        )
        .route("/cowork/sessions/:id/dag", get(get_session_dag))
        .route("/cowork/personas", get(list_personas).post(create_persona))
        .route(
            "/cowork/personas/:id",
            get(get_persona)
                .put(update_persona)
                .patch(update_persona)
                .delete(delete_persona),
        )
        .route("/cowork/projects", get(list_projects).post(create_project))
        .route(
            "/cowork/projects/:id",
            get(get_project)
                .put(update_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route(
            "/cowork/projects/:id/files",
            get(list_project_files).post(create_project_file),
        )
        .route(
            "/cowork/projects/:id/files/:file_id",
            delete(delete_project_file),
        )
        .route("/cowork/memory", get(get_memory).post(store_memory))
        .route(
            "/cowork/memory/search",
            get(search_memory_get).post(search_memory),
        )
        .route("/cowork/memory/health", get(memory_health))
        .route("/cowork/memory/:id", delete(delete_memory))
        .route("/cowork/connectors", get(list_connectors))
        .route("/cowork/approvals", get(list_approvals).post(decide_approval))
        .route(
            "/cowork/suggestions",
            get(list_suggestions).post(create_suggestion),
        )
        .route("/cowork/team-execute", post(team_execute))
        .route("/cowork/run-agent", post(run_agent))
}

// ─── Data models ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct SessionRow {
    id: String,
    user_id: String,
    project_id: Option<String>,
    title: Option<String>,
    status: String,
    mode: String,
    checkpoint: Option<String>,
    metadata: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct ProjectRow {
    id: String,
    user_id: String,
    title: String,
    description: Option<String>,
    instructions: Option<String>,
    metadata: Option<String>,
    git_remote: Option<String>,
    default_branch: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct ProjectFileRow {
    id: String,
    project_id: String,
    user_id: String,
    name: String,
    url: Option<String>,
    upload_id: Option<String>,
    media_type: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct PersonaRow {
    id: String,
    user_id: String,
    name: String,
    description: Option<String>,
    system_prompt: String,
    tools: Option<String>,
    #[serde(rename = "isDefault")]
    is_default: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct MemoryEntryRow {
    id: String,
    user_id: String,
    project_id: Option<String>,
    session_id: Option<String>,
    content: String,
    #[serde(rename = "type")]
    type_: String,
    tags: Option<String>,
    source: Option<String>,
    created_at: String,
}

#[derive(Serialize)]
struct ConnectorRow {
    id: String,
    user_id: String,
    name: String,
    enabled: i64,
    config: Option<String>,
    last_used: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct SuggestionRow {
    id: String,
    user_id: Option<String>,
    content: String,
    source: String,
    dismissed: i64,
    created_at: String,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn is_no_such_table(err: &rusqlite::Error) -> bool {
    if let rusqlite::Error::SqliteFailure(_, Some(msg)) = err {
        msg.contains("no such table")
    } else {
        false
    }
}

// ─── List window (pagination) ────────────────────────────────────────────────

const LIST_DEFAULT_LIMIT: i64 = 100;
const LIST_MAX_LIMIT: i64 = 1000;

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

/// Bound an unbounded list query: caller-provided limit wins when under the
/// cap, otherwise the default; offset is clamped non-negative.
fn clamp_list_window(q: &ListQuery) -> (i64, i64) {
    let limit = q
        .limit
        .unwrap_or(LIST_DEFAULT_LIMIT)
        .clamp(1, LIST_MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);
    (limit, offset)
}

// ─── Sessions ─────────────────────────────────────────────────────────────────

async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let (limit, offset) = clamp_list_window(&q);

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, project_id, title, status, mode, checkpoint, metadata,
                    started_at, completed_at, created_at, updated_at
             FROM cowork_sessions WHERE user_id = ?1 ORDER BY updated_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![user_id, limit, offset], |row| {
                Ok(SessionRow {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    project_id: row.get(2)?,
                    title: row.get(3)?,
                    status: row.get(4)?,
                    mode: row.get(5)?,
                    checkpoint: row.get(6)?,
                    metadata: row.get(7)?,
                    started_at: row.get(8)?,
                    completed_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(sessions)) => Json(json!({ "sessions": sessions })).into_response(),
        Ok(Err(e)) => {
            warn!("DB error listing sessions: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateSessionBody {
    project_id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    mode: Option<String>,
    checkpoint: Option<String>,
    metadata: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    /// Native (gizzi/mode-session) chat id, when this row backs a UI chat
    /// session. Stored in metadata as `a_native_session_id`; the agent-chat
    /// bridge resolves the session's A:// run through it. Idempotency key:
    /// posting again with the same native id returns the existing row.
    #[serde(rename = "nativeSessionId", alias = "native_session_id", alias = "chatId")]
    native_session_id: Option<String>,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreateSessionBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    // A:// §7: session start participates in the canonical lifecycle — the
    // row creation submits the session's canonical intent (creating its run)
    // and stamps the linkage into the session metadata. Idempotent on
    // `nativeSessionId`: a repost for a chat that already has a row returns
    // the existing session. Best-effort like all session recording: a DAG
    // failure must never block session creation.
    let native_for_lookup = body.native_session_id.clone();
    let lookup = tokio::task::spawn_blocking({
        let db = db.clone();
        let user_id = user_id.clone();
        move || {
            let mut conn = db.connect()?;
            match native_for_lookup.as_deref().filter(|s| !s.is_empty()) {
                Some(native) => match crate::cowork::dag::find_session_run(&conn, &user_id, native)? {
                    Some(link) => Ok::<_, rusqlite::Error>(Some(link)),
                    None => Ok(None),
                },
                None => Ok(None),
            }
        }
    })
    .await;

    if let Ok(Ok(Some(existing))) = lookup {
        // Idempotent repost: the session row and its run already exist.
        return (
            StatusCode::CREATED,
            Json(json!({
                "session": { "id": existing.session_id },
                "a_intent_id": existing.intent_id,
                "a_run_id": existing.run_id,
            })),
        )
            .into_response();
    }
    if let Ok(Err(e)) = lookup {
        warn!("session DAG lookup failed: {}", e);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let id2 = id.clone();
    let body_native = body.native_session_id.clone();
    let body_title = body.title.clone();
    let link_user_id = user_id.clone();
    let metadata = {
        let caller_meta = body.metadata.clone();
        let mut meta: serde_json::Value = caller_meta
            .as_deref()
            .and_then(|m| serde_json::from_str(m).ok())
            .filter(|v: &serde_json::Value| v.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(native) = body.native_session_id.as_deref() {
            meta["a_native_session_id"] = serde_json::json!(native);
        }
        Some(meta.to_string())
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO cowork_sessions (id, user_id, project_id, title, status, mode,
                                          checkpoint, metadata, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id2,
                user_id,
                body.project_id,
                body.title,
                body.status.unwrap_or_else(|| "idle".to_string()),
                body.mode.unwrap_or_else(|| "agent".to_string()),
                body.checkpoint,
                metadata,
                body.started_at,
                body.completed_at,
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            // Now that the row exists, create the session's A:// run and
            // stamp the linkage. Failure here leaves a working session
            // without DAG linkage (warned above); the agent-chat bridge
            // backfills it lazily on the next turn.
            let link = tokio::task::spawn_blocking({
                let db = state.db.clone();
                let user_id = link_user_id.clone();
                let id = id.clone();
                let native = body_native.clone();
                let title = body_title.clone();
                move || {
                    let mut conn = db.connect()?;
                    crate::cowork::dag::link_run_to_session(
                        &mut conn,
                        &user_id,
                        &id,
                        native.as_deref(),
                        title.as_deref(),
                    )
                }
            })
            .await;
            match link {
                Ok(Ok(link)) => (
                    StatusCode::CREATED,
                    Json(json!({
                        "session": { "id": id },
                        "a_intent_id": link.intent_id,
                        "a_run_id": link.run_id,
                    })),
                )
                    .into_response(),
                Ok(Err(e)) => {
                    warn!("session DAG linkage failed: {}", e);
                    (
                        StatusCode::CREATED,
                        Json(json!({ "session": { "id": id } })),
                    )
                        .into_response()
                }
                Err(e) => {
                    warn!("session DAG task panicked: {}", e);
                    (
                        StatusCode::CREATED,
                        Json(json!({ "session": { "id": id } })),
                    )
                        .into_response()
                }
            }
        }
        Ok(Err(e)) => {
            warn!("DB error creating session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, project_id, title, status, mode, checkpoint, metadata,
                    started_at, completed_at, created_at, updated_at
             FROM cowork_sessions WHERE id = ?1 AND user_id = ?2",
        )?;
        let row = stmt.query_row(params![id, user_id], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                project_id: row.get(2)?,
                title: row.get(3)?,
                status: row.get(4)?,
                mode: row.get(5)?,
                checkpoint: row.get(6)?,
                metadata: row.get(7)?,
                started_at: row.get(8)?,
                completed_at: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;
        Ok::<_, rusqlite::Error>(row)
    })
    .await;

    match row {
        Ok(Ok(session)) => Json(json!({ "session": session })).into_response(),
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error getting session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// A:// DAG state for a session: the linked run, its jobs, and its recent
/// attributed events. User-scoped through the session row (V142 pattern).
async fn get_session_dag(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // `:id` may be the session row id OR the native (mode-session) chat
        // id — the UI rail only has the latter. Resolve both through the
        // metadata linkage.
        let link = match crate::cowork::dag::session_link(&conn, &user_id, &id)? {
            Some(l) => Some(l),
            None => crate::cowork::dag::find_session_run(&conn, &user_id, &id)?,
        }
        .ok_or_else(|| rusqlite::Error::QueryReturnedNoRows)?;

        let run: Option<serde_json::Value> = conn
            .query_row(
                "SELECT id, workspace_id, initiator, state, entrypoint, created_at, completed_at
                 FROM cowork_runs WHERE id = ?1 AND (user_id = ?2 OR user_id IS NULL)",
                params![link.run_id, user_id],
                |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, String>(0)?,
                        "workspace": row.get::<_, String>(1)?,
                        "initiator": row.get::<_, String>(2)?,
                        "state": row.get::<_, String>(3)?,
                        "entrypoint": row.get::<_, String>(4)?,
                        "created_at": row.get::<_, String>(5)?,
                        "completed_at": row.get::<_, Option<String>>(6)?,
                    }))
                },
            )
            .optional()?;

        let mut jobs_stmt = conn.prepare(
            "SELECT id, job_type, state, payload, result, created_at, completed_at
             FROM cowork_jobs WHERE run_id = ?1 ORDER BY created_at DESC LIMIT 50",
        )?;
        let jobs: Vec<serde_json::Value> = jobs_stmt
            .query_map(params![link.run_id], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "job_type": row.get::<_, String>(1)?,
                    "state": row.get::<_, String>(2)?,
                    "payload": row.get::<_, String>(3)?,
                    "result": row.get::<_, Option<String>>(4)?,
                    "created_at": row.get::<_, String>(5)?,
                    "completed_at": row.get::<_, Option<String>>(6)?,
                }))
            })?
            .collect::<Result<_, _>>()?;

        let mut events_stmt = conn.prepare(
            "SELECT event_type, payload, initiator, delegator, executor, created_at
             FROM cowork_run_events WHERE run_id = ?1 ORDER BY created_at DESC LIMIT 50",
        )?;
        let events: Vec<serde_json::Value> = events_stmt
            .query_map(params![link.run_id], |row| {
                Ok(serde_json::json!({
                    "event_type": row.get::<_, String>(0)?,
                    "payload": row.get::<_, String>(1)?,
                    "initiator": row.get::<_, Option<String>>(2)?,
                    "delegator": row.get::<_, Option<String>>(3)?,
                    "executor": row.get::<_, Option<String>>(4)?,
                    "created_at": row.get::<_, String>(5)?,
                }))
            })?
            .collect::<Result<_, _>>()?;

        Ok::<_, rusqlite::Error>(serde_json::json!({
            "session_id": link.session_id,
            "intent_id": link.intent_id,
            "run": run,
            "jobs": jobs,
            "events": events,
        }))
    })
    .await;

    match result {
        Ok(Ok(dag)) => Json(dag).into_response(),
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "session not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error getting session DAG: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct UpdateSessionBody {
    // Option<Option<T>> patch semantics: outer None = field absent from the
    // request (skip), inner Some(v) = set column to v, where v may be an
    // explicit null that CLEARS the column. Plain values still deserialize as
    // Some(Some(v)), so existing clients are unaffected.
    status: Option<Option<String>>,
    title: Option<Option<String>>,
    checkpoint: Option<Option<String>>,
    metadata: Option<Option<String>>,
    completed_at: Option<Option<String>>,
}

/// Build the SET-clause pieces for update_session from the fields present in
/// the request. Outer None = field absent (skip); inner Some(v) sets the
/// column, where v may be an explicit null that CLEARS the column.
fn session_update_sets(body: &UpdateSessionBody) -> Vec<(&'static str, Option<String>)> {
    let mut sets = Vec::new();
    if let Some(v) = &body.status {
        sets.push(("status = ?", v.clone()));
    }
    if let Some(v) = &body.title {
        sets.push(("title = ?", v.clone()));
    }
    if let Some(v) = &body.checkpoint {
        sets.push(("checkpoint = ?", v.clone()));
    }
    if let Some(v) = &body.metadata {
        sets.push(("metadata = ?", v.clone()));
    }
    if let Some(v) = &body.completed_at {
        sets.push(("completed_at = ?", v.clone()));
    }
    sets
}

async fn update_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateSessionBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    // Captured before the body/id move into the update closure: a terminal
    // status finalizes the session's A:// run after the PATCH succeeds.
    let completing = matches!(
        body.status.as_ref().and_then(|s| s.as_deref()),
        Some("completed") | Some("complete") | Some("ended")
    );
    let session_id = id.clone();
    let fin_user_id = user_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // Build the SET clause only from fields present in the request so an
        // explicit null can clear a column (COALESCE(?, col) made that
        // impossible before).
        let sets = session_update_sets(&body);
        if !sets.is_empty() {
            let clause = sets
                .iter()
                .map(|(column, _)| *column)
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "UPDATE cowork_sessions SET {}, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?",
                clause
            );
            let mut all: Vec<&dyn rusqlite::ToSql> = sets
                .iter()
                .map(|(_, value)| value as &dyn rusqlite::ToSql)
                .collect();
            all.push(&id);
            all.push(&user_id);
            conn.execute(&sql, rusqlite::params_from_iter(all))?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            // Session reaching a terminal status finalizes its A:// run
            // (typed Result + run.completed). Best-effort: the PATCH itself
            // already succeeded.
            if completing {
                let fin = tokio::task::spawn_blocking({
                    let db = state.db.clone();
                    let user_id = fin_user_id.clone();
                    let id = session_id.clone();
                    move || {
                        let mut conn = db.connect()?;
                        crate::cowork::dag::finalize_session_run(&mut conn, &id, &user_id)
                    }
                })
                .await;
                if let Ok(Err(e)) = fin {
                    warn!("session run finalize failed: {}", e);
                }
            }
            Json(json!({"ok": true})).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error updating session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let session_id = id.clone();
    let fin_user_id = user_id.clone();

    // Finalize the session's A:// run before the row (and its linkage)
    // disappears. Best-effort: deletion must not fail on a DAG error.
    let fin = tokio::task::spawn_blocking({
        let db = db.clone();
        let user_id = fin_user_id;
        let id = session_id.clone();
        move || {
            let mut conn = db.connect()?;
            crate::cowork::dag::finalize_session_run(&mut conn, &id, &user_id)
        }
    })
    .await;
    if let Ok(Err(e)) = fin {
        warn!("session run finalize failed: {}", e);
    }

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM cowork_sessions WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => Json(json!({"success": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error deleting session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── Personas ─────────────────────────────────────────────────────────────────

async fn list_personas(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let (limit, offset) = clamp_list_window(&q);

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, description, system_prompt, tools, is_default, created_at, updated_at
             FROM cowork_personas WHERE user_id = ?1 ORDER BY updated_at DESC
             LIMIT ?2 OFFSET ?3"
        )?;
        let rows = stmt.query_map(params![user_id, limit, offset], |row| {
            Ok(PersonaRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                system_prompt: row.get(4)?,
                tools: row.get(5)?,
                is_default: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(personas)) => Json(json!({ "personas": personas })).into_response(),
        Ok(Err(e)) => {
            warn!("DB error listing personas: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreatePersonaBody {
    name: String,
    description: Option<String>,
    system_prompt: String,
    tools: Option<String>,
    #[serde(rename = "isDefault")]
    is_default: Option<bool>,
}

async fn create_persona(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreatePersonaBody>,
) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    let db = state.db.clone();
    let id2 = id.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO cowork_personas (id, user_id, name, description, system_prompt, tools, is_default)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id2,
                user_id,
                body.name,
                body.description,
                body.system_prompt,
                body.tools,
                body.is_default.map(|b| if b { 1 } else { 0 }).unwrap_or(0),
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => (
            StatusCode::CREATED,
            Json(json!({ "persona": { "id": id } })),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error creating persona: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn get_persona(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, description, system_prompt, tools, is_default, created_at, updated_at
             FROM cowork_personas WHERE id = ?1 AND user_id = ?2"
        )?;
        let row = stmt.query_row(params![id, user_id], |row| {
            Ok(PersonaRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                system_prompt: row.get(4)?,
                tools: row.get(5)?,
                is_default: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        Ok::<_, rusqlite::Error>(row)
    })
    .await;

    match row {
        Ok(Ok(persona)) => Json(json!({ "persona": persona })).into_response(),
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error getting persona: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct UpdatePersonaBody {
    // Option<Option<T>> patch semantics: outer None = absent (skip), inner
    // Some(v) = set (explicit null clears the column).
    name: Option<Option<String>>,
    description: Option<Option<String>>,
    system_prompt: Option<Option<String>>,
    tools: Option<Option<String>>,
    #[serde(rename = "isDefault")]
    is_default: Option<bool>,
}

async fn update_persona(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdatePersonaBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // Build the SET clause only from fields present in the request so an
        // explicit null can clear a column (COALESCE(?, col) made that
        // impossible before). is_default stays plain Option<bool> since the
        // column is NOT NULL; present = set, absent = skip.
        let mut sets: Vec<&str> = Vec::new();
        let mut vals: Vec<Option<String>> = Vec::new();
        if let Some(v) = body.name {
            sets.push("name = ?");
            vals.push(v);
        }
        if let Some(v) = body.description {
            sets.push("description = ?");
            vals.push(v);
        }
        if let Some(v) = body.system_prompt {
            sets.push("system_prompt = ?");
            vals.push(v);
        }
        if let Some(v) = body.tools {
            sets.push("tools = ?");
            vals.push(v);
        }
        let flag: Option<i64> = body.is_default.map(|b| if b { 1 } else { 0 });
        if flag.is_some() {
            sets.push("is_default = ?");
        }
        if !sets.is_empty() {
            let sql = format!(
                "UPDATE cowork_personas SET {}, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?",
                sets.join(", ")
            );
            let mut all: Vec<&dyn rusqlite::ToSql> =
                vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            if let Some(ref f) = flag {
                all.push(f);
            }
            all.push(&id);
            all.push(&user_id);
            conn.execute(&sql, rusqlite::params_from_iter(all))?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => Json(json!({"success": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error updating persona: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn delete_persona(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM cowork_personas WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => Json(json!({"success": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error deleting persona: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── Projects ─────────────────────────────────────────────────────────────────

async fn list_projects(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let (limit, offset) = clamp_list_window(&q);

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, title, description, instructions, metadata, git_remote, default_branch, created_at, updated_at
             FROM cowork_projects WHERE user_id = ?1 ORDER BY updated_at DESC
             LIMIT ?2 OFFSET ?3"
        )?;
        let rows = stmt.query_map(params![user_id, limit, offset], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                instructions: row.get(4)?,
                metadata: row.get(5)?,
                git_remote: row.get(6)?,
                default_branch: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(projects)) => Json(json!({ "projects": projects })).into_response(),
        Ok(Err(e)) => {
            warn!("DB error listing projects: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateProjectBody {
    title: String,
    description: Option<String>,
    instructions: Option<String>,
    metadata: Option<String>,
    git_remote: Option<String>,
    default_branch: Option<String>,
}

async fn create_project(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreateProjectBody>,
) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    let db = state.db.clone();
    let id2 = id.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO cowork_projects (id, user_id, title, description, instructions, metadata, git_remote, default_branch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id2,
                user_id,
                body.title,
                body.description,
                body.instructions,
                body.metadata,
                body.git_remote,
                body.default_branch,
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => (
            StatusCode::CREATED,
            Json(json!({ "project": { "id": id } })),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error creating project: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn get_project(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, title, description, instructions, metadata, git_remote, default_branch, created_at, updated_at
             FROM cowork_projects WHERE id = ?1 AND user_id = ?2"
        )?;
        let row = stmt.query_row(params![id, user_id], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                instructions: row.get(4)?,
                metadata: row.get(5)?,
                git_remote: row.get(6)?,
                default_branch: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;
        Ok::<_, rusqlite::Error>(row)
    })
    .await;

    match row {
        Ok(Ok(project)) => Json(json!({ "project": project })).into_response(),
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error getting project: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct UpdateProjectBody {
    // Option<Option<T>> patch semantics: outer None = absent (skip), inner
    // Some(v) = set (explicit null clears the column).
    title: Option<Option<String>>,
    description: Option<Option<String>>,
    instructions: Option<Option<String>>,
    metadata: Option<Option<String>>,
    git_remote: Option<Option<String>>,
    default_branch: Option<Option<String>>,
}

async fn update_project(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateProjectBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // Build the SET clause only from fields present in the request so an
        // explicit null can clear a column (COALESCE(?, col) made that
        // impossible before).
        let mut sets: Vec<&str> = Vec::new();
        let mut vals: Vec<Option<String>> = Vec::new();
        if let Some(v) = body.title {
            sets.push("title = ?");
            vals.push(v);
        }
        if let Some(v) = body.description {
            sets.push("description = ?");
            vals.push(v);
        }
        if let Some(v) = body.instructions {
            sets.push("instructions = ?");
            vals.push(v);
        }
        if let Some(v) = body.metadata {
            sets.push("metadata = ?");
            vals.push(v);
        }
        if let Some(v) = body.git_remote {
            sets.push("git_remote = ?");
            vals.push(v);
        }
        if let Some(v) = body.default_branch {
            sets.push("default_branch = ?");
            vals.push(v);
        }
        if !sets.is_empty() {
            let sql = format!(
                "UPDATE cowork_projects SET {}, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?",
                sets.join(", ")
            );
            let mut all: Vec<&dyn rusqlite::ToSql> =
                vals.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
            all.push(&id);
            all.push(&user_id);
            conn.execute(&sql, rusqlite::params_from_iter(all))?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => Json(json!({"success": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error updating project: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// Delete a project and what hangs off it, in one transaction. Its bot
/// threads stay with their bots, detached from the project (the bots' work
/// isn't lost); the project's team rows and Al's project chat go with it.
/// Before, only the project row went, leaving threads and team rows pointing
/// at a project that no longer existed. Returns whether a project was deleted.
fn delete_project_rows(conn: &rusqlite::Connection, id: &str, user_id: &str) -> rusqlite::Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let deleted = tx.execute(
        "DELETE FROM cowork_projects WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    if deleted > 0 {
        tx.execute(
            "UPDATE bot_threads SET project_id = NULL WHERE project_id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
        tx.execute("DELETE FROM project_bots WHERE project_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM project_messages WHERE project_id = ?1 AND user_id = ?2",
            params![id, user_id],
        )?;
    }
    tx.commit()?;
    Ok(deleted > 0)
}

async fn delete_project(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        delete_project_rows(&conn, &id, &user_id).map(|_| ())
    })
    .await;

    match result {
        Ok(Ok(())) => Json(json!({"success": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error deleting project: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── Project files ────────────────────────────────────────────────────────────
//
// Metadata rows for files attached to a project (Claude-style project
// knowledge). Blobs live behind /api/v1/uploads (`upload_id`) or external
// URLs (`url`); these rows only record what belongs to which project.

async fn list_project_files(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let (limit, offset) = clamp_list_window(&q);

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, project_id, user_id, name, url, upload_id, media_type, created_at
             FROM cowork_project_files WHERE project_id = ?1 AND user_id = ?2 ORDER BY created_at DESC
             LIMIT ?3 OFFSET ?4",
        )?;
        let rows = stmt
            .query_map(params![id, user_id, limit, offset], |row| {
                Ok(ProjectFileRow {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    user_id: row.get(2)?,
                    name: row.get(3)?,
                    url: row.get(4)?,
                    upload_id: row.get(5)?,
                    media_type: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(files)) => Json(json!({ "files": files })).into_response(),
        Ok(Err(e)) if is_no_such_table(&e) => {
            // Backend predates the V28 migration — report an empty list rather
            // than 500ing so clients can render their normal empty state.
            Json(json!({ "files": [] })).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error listing project files: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateProjectFileBody {
    name: String,
    url: Option<String>,
    upload_id: Option<String>,
    media_type: Option<String>,
}

async fn create_project_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(body): Json<CreateProjectFileBody>,
) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    let db = state.db.clone();
    let id2 = id.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO cowork_project_files (id, project_id, user_id, name, url, upload_id, media_type)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id2,
                project_id,
                user_id,
                body.name,
                body.url,
                body.upload_id,
                body.media_type,
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => (
            StatusCode::CREATED,
            Json(json!({ "file": { "id": id } })),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error creating project file: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn delete_project_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path((project_id, file_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM cowork_project_files WHERE id = ?1 AND project_id = ?2 AND user_id = ?3",
            params![file_id, project_id, user_id],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => Json(json!({"success": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error deleting project file: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── Memory ───────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct MemoryPrincipalQuery {
    /// Principal scope (A-T2): when set, only entries owned by or granted to
    /// this principal are returned (default-deny cross-principal).
    pub principal: Option<String>,
    /// With `principal`: only entries that principal owns (a bot's own
    /// memory), not unowned or granted ones.
    #[serde(default)]
    pub owned: bool,
    /// A bot's id: scopes to that bot's own principal (resolved here, so
    /// clients never build A:// ids). Implies `owned`.
    pub bot: Option<String>,
    /// List window bounds, clamped the same way as every other list endpoint.
    #[serde(flatten)]
    pub window: ListQuery,
}

async fn get_memory(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(query): Query<MemoryPrincipalQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let owned = query.owned || query.bot.is_some();
    let bot = query.bot;
    let (limit, offset) = clamp_list_window(&query.window);
    let explicit = query.principal;

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let principal = match bot.as_deref() {
            Some(b) => match bot_memory_principal(&conn, &user_id, b)? {
                Some(p) => Some(p),
                None => return Ok((Vec::new(), None)),
            },
            None => explicit,
        };
        // Principal-scoped path enforces owner+grants (A-T2); unscoped keeps
        // the legacy user-filtered behavior.
        let memories = allternit_cowork_runtime::sqlite_store::search_memory_entries(
            &conn,
            &user_id,
            principal.as_deref(),
            None,
            limit,
            offset,
        )
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok::<_, rusqlite::Error>((memories, principal))
    })
    .await;

    match rows {
        Ok(Ok((mut memories, principal))) => {
            if let (true, Some(p)) = (owned, principal.as_deref()) {
                memories.retain(|m| m["owner_principal"].as_str() == Some(p));
            }
            Json(json!({ "memories": memories })).into_response()
        }
        Ok(Err(e)) if is_no_such_table(&e) => {
            Json(json!({ "memories": Vec::<MemoryEntryRow>::new() })).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error getting memories: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// The principal that owns a bot's memory: its A:// principal when it has
/// one, else a stable local id (bots that only exist on this device, like
/// the packaged Gizzi, have no agents row). `None` when the id belongs to
/// another user's bot. Entries stay user-scoped either way.
pub(crate) fn bot_memory_principal(conn: &rusqlite::Connection, user_id: &str, bot_id: &str) -> rusqlite::Result<Option<String>> {
    let local = || format!("a://local/bot/{bot_id}");
    match conn.query_row(
        "SELECT user_id, principal_id FROM agents WHERE id = ?1",
        rusqlite::params![bot_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
    ) {
        Ok((owner, _)) if owner != user_id => Ok(None),
        Ok((_, p)) => Ok(Some(p.filter(|p| !p.is_empty()).unwrap_or_else(local))),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Some(local())),
        Err(e) => Err(e),
    }
}

/// Forget one memory entry (the bot detail Memory tab's "Forget").
async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        allternit_cowork_runtime::sqlite_store::delete_memory_entry(&conn, &user_id, &id)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })
    .await;
    match result {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => (StatusCode::NOT_FOUND, Json(json!({"error": "memory not found"}))).into_response(),
        Ok(Err(e)) => {
            warn!("DB error deleting memory: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal error"}))).into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal error"}))).into_response()
        }
    }
}

#[derive(Deserialize)]
struct StoreMemoryBody {
    project_id: Option<String>,
    session_id: Option<String>,
    content: String,
    #[serde(rename = "type")]
    type_: Option<String>,
    tags: Option<String>,
    source: Option<String>,
    /// Owning principal (A-T2); writes are attributed to this principal.
    principal: Option<String>,
    /// Principals explicitly granted access (default-deny otherwise).
    grants: Option<Vec<String>>,
    /// A bot's id: the entry is owned by that bot's principal.
    bot: Option<String>,
}

async fn store_memory(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<StoreMemoryBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let type_ = body.type_.clone().unwrap_or_else(|| "fact".to_string());

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let principal = match body.bot.as_deref() {
            Some(b) => Some(
                bot_memory_principal(&conn, &user_id, b)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?,
            ),
            None => body.principal.clone(),
        };
        allternit_cowork_runtime::sqlite_store::store_memory_entry(
            &mut conn,
            &user_id,
            body.project_id.as_deref(),
            body.session_id.as_deref(),
            &body.content,
            &type_,
            body.tags.as_deref(),
            body.source.as_deref(),
            principal.as_deref(),
            &body.grants.clone().unwrap_or_default(),
        )
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })
    .await;

    match result {
        Ok(Ok(id)) => {
            (StatusCode::CREATED, Json(json!({ "memory": { "id": id } }))).into_response()
        }
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "bot not found"}))).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error storing memory: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct SearchMemoryBody {
    query: String,
    limit: Option<i64>,
}

async fn search_memory(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<SearchMemoryBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let limit = body
        .limit
        .unwrap_or(LIST_DEFAULT_LIMIT)
        .clamp(1, LIST_MAX_LIMIT);
    let pattern = format!("%{}%", body.query.replace('%', "\\%").replace('_', "\\_"));

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, project_id, session_id, content, type, tags, source, created_at
             FROM cowork_memory_entries
             WHERE user_id = ?1 AND content LIKE ?2 ESCAPE '\\'
             ORDER BY created_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![user_id, pattern, limit], |row| {
                Ok(MemoryEntryRow {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    project_id: row.get(2)?,
                    session_id: row.get(3)?,
                    content: row.get(4)?,
                    type_: row.get(5)?,
                    tags: row.get(6)?,
                    source: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(memories)) => Json(json!({ "results": memories })).into_response(),
        Ok(Err(e)) if is_no_such_table(&e) => {
            Json(json!({ "results": Vec::<MemoryEntryRow>::new() })).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error searching memories: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct SearchMemoryQuery {
    query: Option<String>,
    limit: Option<i64>,
}

async fn search_memory_get(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(params): Query<SearchMemoryQuery>,
) -> impl IntoResponse {
    let query = params.query.unwrap_or_default();
    let body = SearchMemoryBody {
        query,
        limit: params.limit,
    };
    search_memory(State(state), Extension(user), HeaderMap::new(), Json(body)).await
}

async fn memory_health(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };
    let db = state.db.clone();

    let healthy = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute("SELECT 1", [])?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match healthy {
        Ok(Ok(())) => Json(json!({"status": "healthy", "connected": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB health check failed: {}", e);
            Json(json!({"status": "unhealthy", "connected": false, "error": e.to_string()}))
                .into_response()
        }
        Err(e) => {
            warn!("DB health task panicked: {}", e);
            Json(json!({"status": "unhealthy", "connected": false, "error": "internal error"}))
                .into_response()
        }
    }
}

// ─── Connectors / Approvals / Suggestions ─────────────────────────────────────

async fn list_connectors(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let (limit, offset) = clamp_list_window(&q);

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, enabled, config, last_used, created_at, updated_at
             FROM cowork_connectors WHERE user_id = ?1 ORDER BY updated_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![user_id, limit, offset], |row| {
                Ok(ConnectorRow {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    name: row.get(2)?,
                    enabled: row.get(3)?,
                    config: row.get(4)?,
                    last_used: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(connectors)) => Json(json!({ "connectors": connectors })).into_response(),
        Ok(Err(e)) if is_no_such_table(&e) => {
            Json(json!({ "connectors": Vec::<ConnectorRow>::new() })).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error listing connectors: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

/// Prepare the approvals list statement, filtering out decided rows.
///
/// Prefers the V143+ filter (`dismissed = 0`) so decided/dismissed approvals
/// leave the pending set; on schemas where the dismissed column does not
/// exist yet, degrades to the unfiltered select rather than erroring. A
/// missing table surfaces as a prepare error and is handled by the caller's
/// is_no_such_table fallback.
fn prepare_approvals_list_stmt(
    conn: &rusqlite::Connection,
) -> rusqlite::Result<rusqlite::Statement<'_>> {
    conn.prepare(
        "SELECT id, user_id, content, source, dismissed, created_at
         FROM cowork_approvals
         WHERE (user_id = ?1 OR user_id IS NULL) AND dismissed = 0
         ORDER BY created_at DESC
         LIMIT ?2 OFFSET ?3",
    )
    .or_else(|e| {
        if e.to_string().contains("no such column") {
            // The fallback cannot project `dismissed` either on a pre-V143
            // schema; substitute a literal 0 (never dismissed) instead.
            conn.prepare(
                "SELECT id, user_id, content, source, 0 AS dismissed, created_at
                 FROM cowork_approvals
                 WHERE (user_id = ?1 OR user_id IS NULL)
                 ORDER BY created_at DESC
                 LIMIT ?2 OFFSET ?3",
            )
        } else {
            Err(e)
        }
    })
}

async fn list_approvals(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };
    let db = state.db.clone();
    let user_id = _user.user_id;
    let (limit, offset) = clamp_list_window(&q);

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // A:// §8.18 / §16: approvals are per-user; never return other users' rows.
        let mut stmt = prepare_approvals_list_stmt(&conn)?;
        let rows = stmt
            .query_map(params![user_id, limit, offset], |row| {
                Ok(SuggestionRow {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    content: row.get(2)?,
                    source: row.get(3)?,
                    dismissed: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(approvals)) => Json(json!({
            "approvals": &approvals,
            "pending": &approvals,
        }))
            .into_response(),
        Ok(Err(e)) if is_no_such_table(&e) => Json(json!({
            "approvals": Vec::<SuggestionRow>::new(),
            "pending": Vec::<SuggestionRow>::new(),
        }))
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error listing approvals: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── Approval decisions ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ApprovalDecisionBody {
    /// Frontend sends `actionId` (the cowork_approvals row id).
    #[serde(rename = "actionId")]
    action_id: Option<String>,
    /// Tolerated alias for non-gate callers.
    id: Option<String>,
    /// Frontend sends `decision`; `action` is accepted as an alias.
    decision: Option<String>,
    action: Option<String>,
}

enum ApprovalOutcome {
    Approved,
    Rejected,
    Dismissed,
}

fn normalize_approval_decision(raw: &str) -> Option<ApprovalOutcome> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "approved" | "approve" | "allow" | "grant" => Some(ApprovalOutcome::Approved),
        "rejected" | "reject" | "deny" | "denied" => Some(ApprovalOutcome::Rejected),
        "dismissed" | "dismiss" => Some(ApprovalOutcome::Dismissed),
        _ => None,
    }
}

/// Apply an approval decision to a cowork_approvals row, scoped to the
/// requesting user (NULL-user rows are global and match anyone). Returns the
/// number of rows updated — 0 means the row is missing or owned by someone
/// else, which the caller maps to 404.
fn apply_approval_decision(
    conn: &rusqlite::Connection,
    approval_id: &str,
    outcome: &ApprovalOutcome,
    user_id: &str,
) -> rusqlite::Result<usize> {
    let stored: Option<&str> = match outcome {
        ApprovalOutcome::Approved => Some("approved"),
        ApprovalOutcome::Rejected => Some("rejected"),
        ApprovalOutcome::Dismissed => None,
    };
    conn.execute(
        "UPDATE cowork_approvals
         SET dismissed = 1, decision = ?2, decided_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND (user_id = ?3 OR user_id IS NULL)",
        params![approval_id, stored, user_id],
    )
    .or_else(|e| {
        // Schema predating V144 has no decision/decided_at columns;
        // degrade to a plain dismiss so the endpoint still resolves.
        if e.to_string().contains("no such column") {
            conn.execute(
                "UPDATE cowork_approvals
                 SET dismissed = 1
                 WHERE id = ?1 AND (user_id = ?2 OR user_id IS NULL)",
                params![approval_id, user_id],
            )
        } else {
            Err(e)
        }
    })
}

/// Pull the gizzi permission request id out of a cowork_approvals content
/// payload written by the agent-chat bridge (see
/// `v1_routes::gizzi_permission_approval_content`). Only `gizzi-permission`
/// rows carry it; returns None for gate rows and malformed content.
fn extract_gizzi_request_id(content: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()?
        .get("requestId")?
        .as_str()
        .map(str::to_string)
}

/// What an approval row asks for, as far as Subscription Fabric (D16) cares.
#[derive(Debug, PartialEq)]
enum SubscriptionApproval {
    /// Not a subscription approval.
    None,
    /// A gizzi `subscription` permission ask (a tool-belt call waiting on
    /// the person): approving it mints the human action the tool submits with.
    ToolAsk,
    /// A task an agent prepared through the subscription MCP server:
    /// approving it runs the task.
    Prepared,
}

fn subscription_approval(content: &str) -> SubscriptionApproval {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return SubscriptionApproval::None;
    };
    if value.get("kind").and_then(|v| v.as_str()) == Some(crate::subscription_mcp::PREPARED_KIND) {
        return SubscriptionApproval::Prepared;
    }
    if value.get("requestId").and_then(|v| v.as_str()).is_some()
        && value.pointer("/details/actionType").and_then(|v| v.as_str()) == Some("subscription")
    {
        return SubscriptionApproval::ToolAsk;
    }
    SubscriptionApproval::None
}

async fn decide_approval(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    headers: HeaderMap,
    body: Result<Json<ApprovalDecisionBody>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };
    let Json(body) = match body {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid request body"})),
            )
                .into_response()
        }
    };
    let raw_decision = body
        .decision
        .as_deref()
        .or(body.action.as_deref())
        .unwrap_or("");
    let outcome = match normalize_approval_decision(raw_decision) {
        Some(o) => o,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid decision"})),
            )
                .into_response()
        }
    };
    let approval_id = match body.action_id.as_deref().or(body.id.as_deref()) {
        Some(id) if !id.trim().is_empty() => id.to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "actionId is required"})),
            )
                .into_response()
        }
    };

    let decision_label = match outcome {
        ApprovalOutcome::Approved => "approved",
        ApprovalOutcome::Rejected => "rejected",
        ApprovalOutcome::Dismissed => "dismissed",
    };
    // Relay mapping to the gizzi permission reply endpoint: approving answers
    // the ask once; rejecting denies it. Dismissals are local-only.
    let gizzi_reply = match outcome {
        ApprovalOutcome::Approved => Some("once"),
        ApprovalOutcome::Rejected => Some("reject"),
        ApprovalOutcome::Dismissed => None,
    };
    let db = state.db.clone();
    let user_id = user.user_id;
    let id_for_response = approval_id.clone();
    let label_for_response = decision_label.to_string();
    let approved = matches!(outcome, ApprovalOutcome::Approved);

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // Read the row before deciding: its content carries the gizzi
        // request id we must relay to, and its dismissed flag guarantees a
        // single relay per row (a re-decide must not re-answer the runtime).
        let prior: Option<(String, i64)> = conn
            .query_row(
                "SELECT content, dismissed FROM cowork_approvals
                 WHERE id = ?1 AND (user_id = ?2 OR user_id IS NULL)",
                params![approval_id, user_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .or_else(|e| {
                // Pre-V143 schema without the dismissed column.
                if e.to_string().contains("no such column") {
                    conn.query_row(
                        "SELECT content FROM cowork_approvals
                         WHERE id = ?1 AND (user_id = ?2 OR user_id IS NULL)",
                        params![approval_id, user_id],
                        |row| Ok((row.get(0)?, 0i64)),
                    )
                } else {
                    Err(e)
                }
            })
            .optional()?;
        let updated = apply_approval_decision(&conn, &approval_id, &outcome, &user_id)?;
        Ok::<_, rusqlite::Error>((updated, prior))
    })
    .await;

    match result {
        Ok(Ok((0, _))) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "approval not found"})),
        )
            .into_response(),
        Ok(Ok((_, prior))) => {
            // Relay to the gizzi runtime when this decision answers one of
            // its permission asks. A 4xx means the ask is unanswerable
            // (unknown/expired request id) — surface that to the caller; a
            // transient transport failure only warns so a briefly unreachable
            // runtime never blocks the local decision.
            let mut execution = None;
            if let (Some(reply), Some((content, dismissed))) = (gizzi_reply, prior) {
                let subscription = subscription_approval(&content);
                if dismissed == 0 && approved && subscription == SubscriptionApproval::Prepared {
                    // D16: the person approving an agent-prepared task is the
                    // human act; it runs now (once), through the forwarder.
                    execution = Some(
                        crate::subscription_mcp::execute_prepared(&state, &auth_user, &id_for_response).await,
                    );
                }
                if dismissed == 0 {
                    if let Some(request_id) = extract_gizzi_request_id(&content) {
                        let mut relay = json!({ "reply": reply });
                        // D16: a `subscription` ask carries back the human
                        // action minted here, where the person approved it —
                        // the only place a tool-belt task gets one.
                        if approved && subscription == SubscriptionApproval::ToolAsk {
                            match crate::subscription_routes::mint_human_action(
                                &state.db,
                                &auth_user.user_id,
                                "approval.confirm",
                            ) {
                                Ok((action_id, _)) => relay["humanAction"] = json!(action_id),
                                Err(e) => warn!(error = %e, "could not mint the subscription human action"),
                            }
                        }
                        let gizzi = gizzi_base();
                        let client = gizzi_client(&headers);
                        match client
                            .post(format!("{}/permission/{}/reply", gizzi, request_id))
                            .json(&relay)
                            .send()
                            .await
                        {
                            Ok(resp) if resp.status().is_success() => {}
                            Ok(resp) => {
                                let status = resp.status();
                                warn!(
                                    status = %status,
                                    request_id = %request_id,
                                    "gizzi rejected permission reply relay"
                                );
                                return (
                                    StatusCode::BAD_GATEWAY,
                                    Json(json!({
                                        "error": format!(
                                            "agent runtime rejected the permission reply ({})",
                                            status
                                        ),
                                    })),
                                )
                                    .into_response();
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    request_id = %request_id,
                                    "failed to relay permission reply to gizzi"
                                );
                            }
                        }
                    }
                }
            }
            let mut body = json!({
                "ok": true,
                "id": id_for_response,
                "decision": label_for_response,
            });
            if let Some(execution) = execution {
                body["execution"] = execution;
            }
            Json(body).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error deciding approval: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn list_suggestions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let (limit, offset) = clamp_list_window(&q);

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, content, source, dismissed, created_at
             FROM cowork_suggestions
             WHERE (user_id = ?1 OR user_id IS NULL) AND dismissed = 0
             ORDER BY created_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt
            .query_map(params![user_id, limit, offset], |row| {
                Ok(SuggestionRow {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    content: row.get(2)?,
                    source: row.get(3)?,
                    dismissed: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(suggestions)) => Json(json!({ "suggestions": suggestions })).into_response(),
        Ok(Err(e)) if is_no_such_table(&e) => {
            Json(json!({ "suggestions": Vec::<SuggestionRow>::new() })).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error listing suggestions: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct CreateSuggestionBody {
    content: String,
    source: Option<String>,
    // NOTE: a client-supplied user_id used to be accepted here and allowed
    // forging the owning user. It was removed; the owner always comes from
    // the authenticated AuthUser extension.
}

async fn create_suggestion(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreateSuggestionBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let id2 = id.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO cowork_suggestions (id, user_id, content, source, dismissed)
             VALUES (?1, ?2, ?3, ?4, 0)",
            params![
                id2,
                user_id,
                body.content,
                body.source.unwrap_or_else(|| "system".to_string())
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Ok(Err(e)) => {
            warn!("DB error creating suggestion: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── Execution ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TeamExecuteBody {
    command: Option<String>,
    agent_id: Option<String>,
    prompt: Option<String>,
}

async fn team_execute(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<TeamExecuteBody>,
) -> impl IntoResponse {
    // A:// §5: this endpoint now submits a canonical IntentEnvelope instead
    // of the dead-end cowork_executions insert (the table was written and
    // never read). The intent resolves to a real queued run.
    let intent_id = format!("intent_{}", uuid::Uuid::new_v4());
    let db = state.db.clone();
    let user_id = user.user_id;
    let description = body
        .prompt
        .clone()
        .or_else(|| body.command.clone())
        .unwrap_or_else(|| "team execution".to_string());
    let envelope = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: intent_id.clone(),
        workspace: "a://workspace/default".to_string(),
        initiator: user_id.clone(),
        delegator: None,
        target: body.agent_id.clone(),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "team_execute".to_string(),
            description,
            payload: Some(json!({
                "command": body.command,
                "prompt": body.prompt,
            })),
        },
        permissions: vec![],
        compute: None,
        model: None,
        approval: None,
        return_channel: None,
        causation_chain: body
            .agent_id
            .clone()
            .map(|a| vec![user_id.clone(), a])
            .unwrap_or_default(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        allternit_cowork_runtime::sqlite_store::submit_intent_for_user(
            &mut conn,
            &envelope,
            Some(user_id.as_str()),
        )
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })
    .await;

    match result {
        Ok(Ok(submission)) => (
            StatusCode::OK,
            Json(json!({
                "intent_id": submission.intent_id,
                "run_id": submission.run_id,
                "created": submission.created,
                "status": "queued",
            })),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error creating team execution: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct RunAgentBody {
    agent_id: Option<String>,
    role: Option<String>,
    prompt: Option<String>,
    spec: Option<serde_json::Value>,
}

async fn run_agent(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<RunAgentBody>,
) -> impl IntoResponse {
    // A:// §5: canonical intent submission replaces the dead-end
    // cowork_executions insert.
    let intent_id = format!("intent_{}", uuid::Uuid::new_v4());
    let db = state.db.clone();
    let user_id = user.user_id;
    let agent_id = body.agent_id.or_else(|| {
        body.spec
            .as_ref()
            .and_then(|s| s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
    });
    let prompt = body.prompt.or_else(|| {
        body.spec.as_ref().and_then(|s| {
            s.get("prompt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
    });
    let command = body.role.or_else(|| {
        body.spec.as_ref().and_then(|s| {
            s.get("role")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
    });
    let description = prompt.clone().or_else(|| command.clone()).unwrap_or_else(|| "agent run".to_string());
    let envelope = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: intent_id.clone(),
        workspace: "a://workspace/default".to_string(),
        initiator: user_id.clone(),
        delegator: None,
        target: agent_id.clone(),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "agent_run".to_string(),
            description,
            payload: Some(json!({ "command": command, "prompt": prompt })),
        },
        permissions: vec![],
        compute: None,
        model: None,
        approval: None,
        return_channel: None,
        causation_chain: agent_id
            .clone()
            .map(|a| vec![user_id.clone(), a])
            .unwrap_or_default(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        allternit_cowork_runtime::sqlite_store::submit_intent_for_user(
            &mut conn,
            &envelope,
            Some(user_id.as_str()),
        )
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })
    .await;

    match result {
        Ok(Ok(submission)) => (
            StatusCode::OK,
            Json(json!({
                "intent_id": submission.intent_id,
                "run_id": submission.run_id,
                "created": submission.created,
                "status": "queued",
            })),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error creating agent execution: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

async fn cowork_status() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "service": "cowork",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_a_project_detaches_its_threads_and_drops_its_team_and_chat() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE cowork_projects (id TEXT PRIMARY KEY, user_id TEXT NOT NULL);
             CREATE TABLE bot_threads (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, project_id TEXT);
             CREATE TABLE project_bots (project_id TEXT NOT NULL, bot_id TEXT NOT NULL);
             CREATE TABLE project_messages (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, user_id TEXT NOT NULL);
             INSERT INTO cowork_projects VALUES ('p1','u1'), ('p2','u1');
             INSERT INTO bot_threads VALUES ('t1','u1','p1'), ('t2','u1','p2');
             INSERT INTO project_bots VALUES ('p1','b1'), ('p2','b1');
             INSERT INTO project_messages VALUES ('m1','p1','u1'), ('m2','p2','u1');",
        )
        .unwrap();
        assert!(!delete_project_rows(&conn, "p1", "u2").unwrap(), "not your project");
        assert!(delete_project_rows(&conn, "p1", "u1").unwrap());
        let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(one("SELECT COUNT(*) FROM cowork_projects"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM bot_threads WHERE id='t1' AND project_id IS NULL"), 1, "thread kept, detached");
        assert_eq!(one("SELECT COUNT(*) FROM bot_threads WHERE project_id='p2'"), 1, "other project untouched");
        assert_eq!(one("SELECT COUNT(*) FROM project_bots"), 1);
        assert_eq!(one("SELECT COUNT(*) FROM project_messages"), 1);
    }

    fn normalize(s: &str) -> Option<ApprovalOutcome> {
        normalize_approval_decision(s)
    }

    #[test]
    fn bot_memory_principal_uses_the_bots_principal_or_a_stable_local_id() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agents (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, principal_id TEXT);
            INSERT INTO agents VALUES ('b1', 'u1', 'a://workspace/ws/bot/b1'), ('b2', 'u1', NULL);").unwrap();
        assert_eq!(bot_memory_principal(&conn, "u1", "b1").unwrap().as_deref(), Some("a://workspace/ws/bot/b1"));
        assert_eq!(bot_memory_principal(&conn, "u1", "b2").unwrap().as_deref(), Some("a://local/bot/b2"));
        assert_eq!(bot_memory_principal(&conn, "u2", "b1").unwrap(), None, "not your bot");
        assert_eq!(
            bot_memory_principal(&conn, "u1", "local-agent-9").unwrap().as_deref(),
            Some("a://local/bot/local-agent-9"),
            "device-only bots still get their own memory"
        );
    }

    #[test]
    fn normalize_approval_decision_frontend_values() {
        assert!(matches!(normalize("approved"), Some(ApprovalOutcome::Approved)));
        assert!(matches!(normalize("rejected"), Some(ApprovalOutcome::Rejected)));
    }

    #[test]
    fn normalize_approval_decision_aliases_and_case() {
        assert!(matches!(normalize("Approve"), Some(ApprovalOutcome::Approved)));
        assert!(matches!(normalize(" deny "), Some(ApprovalOutcome::Rejected)));
        assert!(matches!(normalize("dismiss"), Some(ApprovalOutcome::Dismissed)));
    }

    #[test]
    fn normalize_approval_decision_approved_aliases() {
        for alias in ["approve", "allow", "grant", "APPROVED", " Allow "] {
            assert!(
                matches!(normalize(alias), Some(ApprovalOutcome::Approved)),
                "expected {alias:?} to normalize to Approved"
            );
        }
    }

    #[test]
    fn normalize_approval_decision_rejected_aliases() {
        for alias in ["reject", "deny", "denied", "REJECTED", " Deny "] {
            assert!(
                matches!(normalize(alias), Some(ApprovalOutcome::Rejected)),
                "expected {alias:?} to normalize to Rejected"
            );
        }
    }

    #[test]
    fn normalize_approval_decision_dismissed_aliases() {
        for alias in ["dismissed", "DISMISS"] {
            assert!(
                matches!(normalize(alias), Some(ApprovalOutcome::Dismissed)),
                "expected {alias:?} to normalize to Dismissed"
            );
        }
    }

    #[test]
    fn normalize_approval_decision_rejects_unknown() {
        assert!(normalize("").is_none());
        assert!(normalize("maybe").is_none());
        assert!(normalize("approved!").is_none());
    }

    // ── Scratch-DB tests over the cowork_approvals table ─────────────────────
    //
    // V143 DDL + the V144 decision columns, matching the production schema a
    // migrated node ends up with.

    const APPROVALS_DDL_V144: &str = "
        CREATE TABLE cowork_approvals (
            id         TEXT PRIMARY KEY,
            user_id    TEXT,
            content    TEXT NOT NULL,
            source     TEXT NOT NULL DEFAULT 'system',
            dismissed  INTEGER NOT NULL DEFAULT 0,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            decision   TEXT,
            decided_at DATETIME
        );
    ";

    fn scratch_approvals_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(APPROVALS_DDL_V144).expect("ddl");
        conn
    }

    fn insert_approval(conn: &rusqlite::Connection, id: &str, user_id: Option<&str>, content: &str) {
        conn.execute(
            "INSERT INTO cowork_approvals (id, user_id, content) VALUES (?1, ?2, ?3)",
            params![id, user_id, content],
        )
        .expect("insert");
    }

    /// Mirror of list_approvals' row mapping, fed by prepare_approvals_list_stmt.
    fn list_pending_ids(
        conn: &rusqlite::Connection,
        user_id: &str,
    ) -> Vec<(String, Option<String>)> {
        let mut stmt = prepare_approvals_list_stmt(conn).expect("prepare");
        stmt.query_map(params![user_id, 100, 0], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows")
    }

    #[test]
    fn list_approvals_excludes_decided_rows_from_pending() {
        let conn = scratch_approvals_db();
        insert_approval(&conn, "a-pending", Some("user-a"), "still pending");
        insert_approval(&conn, "a-decided", Some("user-a"), "decided");
        apply_approval_decision(&conn, "a-decided", &ApprovalOutcome::Approved, "user-a")
            .expect("decide");

        let ids = list_pending_ids(&conn, "user-a");
        assert_eq!(ids.len(), 1, "decided rows must leave the pending set");
        assert_eq!(ids[0].0, "a-pending");
    }

    #[test]
    fn list_approvals_scopes_to_user_and_null_rows() {
        let conn = scratch_approvals_db();
        insert_approval(&conn, "own", Some("user-a"), "own");
        insert_approval(&conn, "global", None, "global");
        insert_approval(&conn, "other", Some("user-b"), "other user");

        let ids = list_pending_ids(&conn, "user-a");
        let keys: Vec<String> = ids.iter().map(|(id, _)| id.clone()).collect();
        assert!(keys.contains(&"own".to_string()));
        assert!(keys.contains(&"global".to_string()));
        assert!(!keys.contains(&"other".to_string()));
    }

    #[test]
    fn list_approvals_falls_back_when_dismissed_column_missing() {
        // Pre-V143 schema: no dismissed column. The statement builder must
        // degrade to the unfiltered select instead of erroring.
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE cowork_approvals (
                id         TEXT PRIMARY KEY,
                user_id    TEXT,
                content    TEXT NOT NULL,
                source     TEXT NOT NULL DEFAULT 'system',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .expect("ddl");
        insert_approval(&conn, "legacy-1", Some("user-a"), "legacy");
        insert_approval(&conn, "legacy-2", Some("user-a"), "legacy");

        let ids = list_pending_ids(&conn, "user-a");
        assert_eq!(ids.len(), 2, "fallback select must still return rows");
    }

    #[test]
    fn list_approvals_missing_table_errors_for_caller_fallback() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        let err = prepare_approvals_list_stmt(&conn).unwrap_err();
        assert!(is_no_such_table(&err));
    }

    #[test]
    fn decide_approval_does_not_touch_another_users_row() {
        let conn = scratch_approvals_db();
        insert_approval(&conn, "row-a", Some("user-a"), "A");
        insert_approval(&conn, "row-b", Some("user-b"), "B");

        // user-b tries to decide user-a's row: 0 rows updated, row untouched.
        let updated = apply_approval_decision(&conn, "row-a", &ApprovalOutcome::Approved, "user-b")
            .expect("update");
        assert_eq!(updated, 0, "cross-user decide must update nothing");

        let dismissed: i64 = conn
            .query_row(
                "SELECT dismissed FROM cowork_approvals WHERE id = 'row-a'",
                [],
                |row| row.get(0),
            )
            .expect("select");
        assert_eq!(dismissed, 0, "row-a must remain pending");
    }

    #[test]
    fn decide_approval_null_user_row_is_global() {
        let conn = scratch_approvals_db();
        insert_approval(&conn, "global", None, "global");
        let updated = apply_approval_decision(&conn, "global", &ApprovalOutcome::Rejected, "user-a")
            .expect("update");
        assert_eq!(updated, 1);
        let (dismissed, decision): (i64, Option<String>) = conn
            .query_row(
                "SELECT dismissed, decision FROM cowork_approvals WHERE id = 'global'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("select");
        assert_eq!(dismissed, 1);
        assert_eq!(decision.as_deref(), Some("rejected"));
    }

    #[test]
    fn decide_approval_records_decision_columns() {
        let conn = scratch_approvals_db();
        insert_approval(&conn, "own", Some("user-a"), "A");
        apply_approval_decision(&conn, "own", &ApprovalOutcome::Approved, "user-a").expect("update");
        let (dismissed, decision): (i64, Option<String>) = conn
            .query_row(
                "SELECT dismissed, decision FROM cowork_approvals WHERE id = 'own'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("select");
        assert_eq!(dismissed, 1);
        assert_eq!(decision.as_deref(), Some("approved"));
    }

    #[test]
    fn decide_approval_falls_back_without_decision_columns() {
        // Pre-V144 schema: no decision/decided_at columns.
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE cowork_approvals (
                id         TEXT PRIMARY KEY,
                user_id    TEXT,
                content    TEXT NOT NULL,
                source     TEXT NOT NULL DEFAULT 'system',
                dismissed  INTEGER NOT NULL DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        )
        .expect("ddl");
        insert_approval(&conn, "own", Some("user-a"), "A");
        insert_approval(&conn, "other", Some("user-b"), "B");

        let updated = apply_approval_decision(&conn, "own", &ApprovalOutcome::Approved, "user-a")
            .expect("fallback update");
        assert_eq!(updated, 1);
        let cross = apply_approval_decision(&conn, "other", &ApprovalOutcome::Approved, "user-a")
            .expect("cross update");
        assert_eq!(cross, 0, "fallback path must keep user scoping");
    }

    #[test]
    fn subscription_approvals_are_recognised_by_content() {
        let tool_ask = serde_json::json!({
            "actionId": "per_1", "requestId": "per_1",
            "details": {"actionType": "subscription"}
        })
        .to_string();
        assert_eq!(subscription_approval(&tool_ask), SubscriptionApproval::ToolAsk);
        let prepared = serde_json::json!({"kind": "subscription_task", "actionId": "subsprep_1"}).to_string();
        assert_eq!(subscription_approval(&prepared), SubscriptionApproval::Prepared);
        // An ordinary tool ask, a gate row without a runtime request, junk.
        let bash = serde_json::json!({"requestId": "per_2", "details": {"actionType": "bash"}}).to_string();
        assert_eq!(subscription_approval(&bash), SubscriptionApproval::None);
        let no_request = serde_json::json!({"details": {"actionType": "subscription"}}).to_string();
        assert_eq!(subscription_approval(&no_request), SubscriptionApproval::None);
        assert_eq!(subscription_approval("not json"), SubscriptionApproval::None);
    }

    #[test]
    fn extract_gizzi_request_id_reads_bridge_content() {
        let content = serde_json::json!({
            "actionId": "perm_1",
            "sessionId": "ses_1",
            "riskLevel": "medium",
            "summary": "bash requested by Bash",
            "details": { "actionType": "bash", "target": "rm -rf tmp", "consequence": "…" },
            "requestId": "perm_1",
            "toolName": "Bash",
            "patterns": ["rm -rf tmp"],
            "always": [],
            "messageId": "msg_1",
        })
        .to_string();
        assert_eq!(extract_gizzi_request_id(&content).as_deref(), Some("perm_1"));
    }

    #[test]
    fn extract_gizzi_request_id_ignores_gate_rows_and_malformed_content() {
        // Plain-text gate rows have no JSON requestId.
        assert_eq!(extract_gizzi_request_id("still pending"), None);
        // Malformed JSON must not panic.
        assert_eq!(extract_gizzi_request_id("{not json"), None);
        // Valid JSON without a requestId (approval-gate payload).
        let gate = serde_json::json!({
            "actionId": "act_1", "sessionId": "ses_1", "riskLevel": "high",
            "summary": "Approve act_1?",
            "details": { "actionType": "bash", "target": "x", "consequence": "y" },
        })
        .to_string();
        assert_eq!(extract_gizzi_request_id(&gate), None);
        // Non-string requestId is ignored.
        let numeric = serde_json::json!({ "requestId": 42 }).to_string();
        assert_eq!(extract_gizzi_request_id(&numeric), None);
    }

    // ── update_session SET-clause semantics (scratch DB over V1 DDL) ─────────

    const SESSIONS_DDL_V1: &str = "
        CREATE TABLE cowork_sessions (
            id           TEXT PRIMARY KEY,
            user_id      TEXT NOT NULL,
            project_id   TEXT,
            title        TEXT,
            status       TEXT NOT NULL DEFAULT 'idle',
            mode         TEXT NOT NULL DEFAULT 'agent',
            checkpoint   TEXT,
            metadata     TEXT,
            started_at   DATETIME,
            completed_at DATETIME,
            created_at   DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at   DATETIME DEFAULT CURRENT_TIMESTAMP
        );
    ";

    fn scratch_sessions_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(SESSIONS_DDL_V1).expect("ddl");
        conn
    }

    fn insert_session(conn: &rusqlite::Connection, id: &str, user_id: &str) {
        conn.execute(
            "INSERT INTO cowork_sessions (id, user_id, title, status, checkpoint)
             VALUES (?1, ?2, 'orig-title', 'idle', 'cp-1')",
            params![id, user_id],
        )
        .expect("insert");
    }

    /// Apply update_session's SET-clause builder + UPDATE to a scratch DB,
    /// exactly as the handler does.
    fn apply_session_update(
        conn: &rusqlite::Connection,
        id: &str,
        user_id: &str,
        body: &UpdateSessionBody,
    ) {
        let sets = session_update_sets(body);
        if sets.is_empty() {
            return;
        }
        let clause = sets
            .iter()
            .map(|(column, _)| *column)
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE cowork_sessions SET {}, updated_at = CURRENT_TIMESTAMP WHERE id = ? AND user_id = ?",
            clause
        );
        let mut all: Vec<&dyn rusqlite::ToSql> = sets
            .iter()
            .map(|(_, value)| value as &dyn rusqlite::ToSql)
            .collect();
        all.push(&id);
        all.push(&user_id);
        conn.execute(&sql, rusqlite::params_from_iter(all))
            .expect("update");
    }

    fn session_field(conn: &rusqlite::Connection, id: &str, column: &str) -> Option<String> {
        conn.query_row(
            &format!("SELECT {column} FROM cowork_sessions WHERE id = ?1"),
            params![id],
            |row| row.get(0),
        )
        .expect("select")
    }

    #[test]
    fn update_session_absent_field_leaves_value() {
        let conn = scratch_sessions_db();
        insert_session(&conn, "s1", "user-a");
        // Only title present: status and checkpoint must be untouched.
        let body = UpdateSessionBody {
            status: None,
            title: Some(Some("new-title".to_string())),
            checkpoint: None,
            metadata: None,
            completed_at: None,
        };
        apply_session_update(&conn, "s1", "user-a", &body);

        assert_eq!(session_field(&conn, "s1", "title").as_deref(), Some("new-title"));
        assert_eq!(session_field(&conn, "s1", "status").as_deref(), Some("idle"));
        assert_eq!(session_field(&conn, "s1", "checkpoint").as_deref(), Some("cp-1"));
    }

    #[test]
    fn update_session_explicit_null_clears_value() {
        let conn = scratch_sessions_db();
        insert_session(&conn, "s1", "user-a");
        let body = UpdateSessionBody {
            status: None,
            title: Some(None),
            checkpoint: Some(None),
            metadata: None,
            completed_at: None,
        };
        apply_session_update(&conn, "s1", "user-a", &body);

        assert_eq!(session_field(&conn, "s1", "title"), None);
        assert_eq!(session_field(&conn, "s1", "checkpoint"), None);
        assert_eq!(session_field(&conn, "s1", "status").as_deref(), Some("idle"));
    }

    #[test]
    fn update_session_plain_value_sets_and_scopes_to_user() {
        let conn = scratch_sessions_db();
        insert_session(&conn, "s1", "user-a");
        insert_session(&conn, "s2", "user-b");

        // user-b attempts to update user-a's session: WHERE clause blocks it.
        let body = UpdateSessionBody {
            status: Some(Some("running".to_string())),
            title: None,
            checkpoint: None,
            metadata: None,
            completed_at: None,
        };
        apply_session_update(&conn, "s1", "user-b", &body);
        assert_eq!(session_field(&conn, "s1", "status").as_deref(), Some("idle"));

        // Own session updates fine.
        apply_session_update(&conn, "s1", "user-a", &body);
        assert_eq!(session_field(&conn, "s1", "status").as_deref(), Some("running"));
        assert_eq!(session_field(&conn, "s2", "status").as_deref(), Some("idle"));
    }
}
