//! Task Management Routes
//!
//! CRUD operations for tasks — supports personal and workspace-scoped tasks.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::error;

use crate::auth::get_user;
use crate::cowork_nodes::{self, CoworkError, Created, TaskFields};
use crate::AppState;

// ─── Request/Response Types ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub user_id: String,
    pub workspace_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee_id: Option<String>,
    pub due_date: Option<String>,
    pub tags: Option<String>,
    pub metadata: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub assignee_type: Option<String>,
    pub assignee_name: Option<String>,
    /// The task's Factory node (`None` for history from before the fold).
    #[serde(default, rename = "dagId", skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    #[serde(default, rename = "nodeId", skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateTaskRequest {
    /// Optional client-supplied task id. When present, create is idempotent:
    /// a conflicting id returns the existing row instead of an error.
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<serde_json::Value>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    /// Free-form metadata. The column is TEXT; strings are stored as-is and
    /// object/array payloads are stringified into the same column.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub assignee_type: Option<String>,
    #[serde(default)]
    pub assignee_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateTaskRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<serde_json::Value>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub metadata: Option<String>,
    #[serde(default)]
    pub assignee_type: Option<String>,
    #[serde(default)]
    pub assignee_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn task_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/tasks", get(list_tasks))
        .route("/tasks", post(create_task))
        .route("/tasks/:id", get(get_task))
        .route("/tasks/:id", put(update_task))
        .route("/tasks/:id", delete(delete_task))
        .route("/tasks/:id/assign", post(assign_task))
        .route(
            "/tasks/:id/comments",
            get(list_task_comments).post(add_task_comment),
        )
        .route("/tasks/:id/audit-logs", get(get_task_audit_logs))
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `?dryRun=true` (or `dry_run`) on a write prints the plan and writes nothing.
#[derive(Debug, Deserialize, Default)]
pub struct DryRunQuery {
    #[serde(default, rename = "dryRun", alias = "dry_run")]
    pub dry_run: Option<bool>,
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response()
}

fn db_error(e: impl std::fmt::Display) -> Response {
    error!("DB error: {}", e);
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Database error"}))).into_response()
}

fn cowork_error(e: CoworkError) -> Response {
    (e.http_status(), Json(e.body())).into_response()
}

fn priority_string(v: &Option<serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// The node for `id`, folding a pre-fold `tasks` row the user owns first.
/// `Err` is the response to send (404, or 403 for someone else's task).
// No rusqlite::Connection is ever held across an .await here: it isn't Sync, so
// the handler future wouldn't be Send and axum would reject it. Connections are
// opened in short synchronous scopes instead (cheap for SQLite).
async fn node_task(state: &AppState, user_id: &str, id: &str) -> Result<(), Response> {
    let events = cowork_nodes::ledger_events(&state.rails).await.map_err(cowork_error)?;
    if cowork_nodes::find(&events, user_id, id).is_some() {
        return Ok(());
    }
    let legacy = {
        let conn = state.db.connect().map_err(db_error)?;
        cowork_nodes::legacy_row(&conn, id)
    };
    match legacy {
        Ok(Some(t)) if t.user_id != user_id => Err((StatusCode::FORBIDDEN, Json(json!({"error": "Access denied"}))).into_response()),
        Ok(Some(t)) => cowork_nodes::fold_row(&state.rails, &t).await.map_err(cowork_error),
        Ok(None) => Err(cowork_error(CoworkError::not_found(format!("task {id} not found")))),
        Err(e) => Err(db_error(e)),
    }
}

/// Lists the user's tasks: their nodes, plus done/closed history from before
/// the fold. Newest first.
async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListTasksQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };
    let events = match cowork_nodes::ledger_events(&state.rails).await {
        Ok(e) => e,
        Err(e) => return cowork_error(e),
    };
    let ws = query.workspace_id.as_deref();
    let mut tasks: Vec<Task> = cowork_nodes::user_task_nodes(&events, &user.user_id, ws)
        .iter()
        .map(|(d, n)| cowork_nodes::task_of(d, n))
        .collect();
    match cowork_nodes::legacy_rows(&conn, &events, &user.user_id, ws) {
        Ok(rows) => tasks.extend(rows),
        Err(e) => return db_error(e),
    }
    if let Some(st) = query.status.as_deref() {
        let want = cowork_nodes::TaskStatus::parse(st).map(|s| s.as_str().to_string()).unwrap_or_else(|| st.to_string());
        tasks.retain(|t| t.status == want || t.status == st);
    }
    tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
    let total = tasks.len();
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);
    let tasks: Vec<Task> = tasks.into_iter().skip(offset).take(limit).collect();
    (StatusCode::OK, Json(json!({ "tasks": tasks, "total": total }))).into_response()
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    Query(dry): Query<DryRunQuery>,
    headers: HeaderMap,
    Json(body): Json<CreateTaskRequest>,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let id = body.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let fields = TaskFields {
        title: Some(body.title.clone()),
        description: body.description.clone(),
        status: body.status.clone(),
        priority: priority_string(&body.priority),
        assignee_id: body.assignee_id.clone(),
        assignee_type: body.assignee_type.clone(),
        assignee_name: body.assignee_name.clone(),
        due_date: body.due_date.clone(),
        tags: body.tags.clone(),
        metadata: body.metadata.as_ref().map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        }),
    };
    let ws = body.workspace_id.clone().unwrap_or_default();
    // An id that names a pre-fold row is the same task (idempotent create).
    let legacy = {
        let conn = match state.db.connect() {
            Ok(c) => c,
            Err(e) => return db_error(e),
        };
        cowork_nodes::legacy_row(&conn, &id)
    };
    if let Ok(Some(t)) = legacy {
        if t.user_id != user.user_id {
            return cowork_error(CoworkError::usage(format!("task id {id:?} is taken"), "Create it without an id, or with a different one."));
        }
        if let Err(r) = node_task(&state, &user.user_id, &id).await {
            return r;
        }
    }
    match cowork_nodes::create(&state.rails, &user.user_id, &ws, &id, &fields, dry.dry_run.unwrap_or(false)).await {
        Ok(Created::Planned(plan)) => (StatusCode::OK, Json(json!({ "dryRun": true, "plan": plan }))).into_response(),
        Ok(Created::Task(task, created)) => {
            let conn = match state.db.connect() {
                Ok(c) => c,
                Err(e) => return db_error(e),
            };
            if let Err(e) = cowork_nodes::write_row(&conn, &task) {
                error!("tasks read model: {}", e);
            }
            if created {
                let _ = write_audit_log(&conn, &id, "create", "human", &user.user_id, Some(&serde_json::to_string(&body).unwrap_or_default()));
            }
            let status = if created { StatusCode::CREATED } else { StatusCode::OK };
            (status, Json(json!({ "task": task }))).into_response()
        }
        Err(e) => cowork_error(e),
    }
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let events = match cowork_nodes::ledger_events(&state.rails).await {
        Ok(e) => e,
        Err(e) => return cowork_error(e),
    };
    if let Some((d, n)) = cowork_nodes::find(&events, &user.user_id, &id) {
        return (StatusCode::OK, Json(cowork_nodes::task_of(&d, &n))).into_response();
    }
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };
    match cowork_nodes::legacy_row(&conn, &id) {
        Ok(Some(t)) if t.user_id == user.user_id => (StatusCode::OK, Json(t)).into_response(),
        Ok(Some(_)) => (StatusCode::FORBIDDEN, Json(json!({"error": "Access denied"}))).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "Task not found"}))).into_response(),
        Err(e) => db_error(e),
    }
}

/// Apply `fields` to the task's node, then refresh its read-model row.
async fn write_task(state: &AppState, user_id: &str, id: &str, fields: TaskFields, dry_run: bool, action: &str, audit: String) -> Response {
    if let Err(r) = node_task(state, user_id, id).await {
        return r;
    }
    match cowork_nodes::update(&state.rails, user_id, id, &fields, dry_run).await {
        Ok(Ok(plan)) => (StatusCode::OK, Json(json!({ "dryRun": true, "plan": plan }))).into_response(),
        Ok(Err(task)) => {
            let conn = match state.db.connect() {
                Ok(c) => c,
                Err(e) => return db_error(e),
            };
            if let Err(e) = cowork_nodes::write_row(&conn, &task) {
                error!("tasks read model: {}", e);
            }
            let _ = write_audit_log(&conn, id, action, "human", user_id, Some(&audit));
            (StatusCode::OK, Json(task)).into_response()
        }
        Err(e) => cowork_error(e),
    }
}

async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(dry): Query<DryRunQuery>,
    headers: HeaderMap,
    Json(body): Json<UpdateTaskRequest>,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    let fields = TaskFields {
        title: body.title.clone(),
        description: body.description.clone(),
        status: body.status.clone(),
        priority: priority_string(&body.priority),
        assignee_id: body.assignee_id.clone(),
        assignee_type: body.assignee_type.clone(),
        assignee_name: body.assignee_name.clone(),
        due_date: body.due_date.clone(),
        tags: body.tags.clone(),
        metadata: body.metadata.clone(),
    };
    let audit = serde_json::to_string(&body).unwrap_or_default();
    write_task(&state, &user.user_id, &id, fields, dry.dry_run.unwrap_or(false), "update", audit).await
}

async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    if let Err(r) = node_task(&state, &user.user_id, &id).await {
        return r;
    }
    if let Err(e) = cowork_nodes::delete(&state.rails, &user.user_id, &id).await {
        return cowork_error(e);
    }
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };
    match conn.execute("DELETE FROM tasks WHERE id = ?1 AND user_id = ?2", rusqlite::params![&id, &user.user_id]) {
        Ok(_) => {
            let _ = write_audit_log(&conn, &id, "delete", "human", &user.user_id, None);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => db_error(e),
    }
}

/// `Err` when the user can't see task `id` (comments and audit logs).
fn check_owner(conn: &rusqlite::Connection, user_id: &str, id: &str) -> Result<(), Response> {
    match cowork_nodes::legacy_row(conn, id) {
        Ok(Some(t)) if t.user_id == user_id => Ok(()),
        Ok(Some(_)) => Err((StatusCode::FORBIDDEN, Json(json!({"error": "Access denied"}))).into_response()),
        Ok(None) => Err((StatusCode::NOT_FOUND, Json(json!({"error": "Task not found"}))).into_response()),
        Err(e) => Err(db_error(e)),
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

pub fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        user_id: row.get(1)?,
        workspace_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        status: row.get(5)?,
        priority: row.get(6)?,
        assignee_id: row.get(7)?,
        due_date: row.get(8)?,
        tags: row.get(9)?,
        metadata: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        assignee_type: row.get(13)?,
        assignee_name: row.get(14)?,
        dag_id: None,
        node_id: None,
    })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AssignTaskRequest {
    pub assignee_type: Option<String>,
    pub assignee_id: Option<String>,
    pub assignee_name: Option<String>,
}

async fn assign_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(dry): Query<DryRunQuery>,
    headers: HeaderMap,
    Json(body): Json<AssignTaskRequest>,
) -> Response {
    let Some(user) = get_user(&headers) else { return unauthorized() };
    // Null fields unassign (the same as the old empty-string write).
    let fields = TaskFields {
        assignee_type: Some(body.assignee_type.clone().unwrap_or_default()),
        assignee_id: Some(body.assignee_id.clone().unwrap_or_default()),
        assignee_name: Some(body.assignee_name.clone().unwrap_or_default()),
        ..Default::default()
    };
    let audit = serde_json::to_string(&body).unwrap_or_default();
    write_task(&state, &user.user_id, &id, fields, dry.dry_run.unwrap_or(false), "assign", audit).await
}

async fn list_task_comments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
                .into_response()
        }
    };

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(_e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response()
        }
    };
    if let Err(r) = check_owner(&conn, &user.user_id, &id) {
        return r.into_response();
    }

    let mut stmt = match conn.prepare(
        "SELECT id, task_id, body, author_id, author_name, created_at 
         FROM task_comments WHERE task_id = ?1 ORDER BY created_at ASC",
    ) {
        Ok(s) => s,
        Err(_e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": _e.to_string()})),
            )
                .into_response()
        }
    };

    let comments: Vec<serde_json::Value> = match stmt.query_map([&id], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "task_id": row.get::<_, String>(1)?,
            "body": row.get::<_, String>(2)?,
            "author_id": row.get::<_, String>(3)?,
            "author_name": row.get::<_, Option<String>>(4)?,
            "created_at": row.get::<_, String>(5)?,
        }))
    }) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": _e.to_string()})),
            )
                .into_response()
        }
    };

    (StatusCode::OK, Json(comments)).into_response()
}

#[derive(Debug, Deserialize)]
pub struct AddTaskCommentRequest {
    pub body: String,
}

async fn add_task_comment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AddTaskCommentRequest>,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
                .into_response()
        }
    };

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(_e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response()
        }
    };
    if let Err(r) = check_owner(&conn, &user.user_id, &id) {
        return r.into_response();
    }

    let comment_id = uuid::Uuid::new_v4().to_string();
    let author_name = user.email.clone();

    let result = conn.execute(
        "INSERT INTO task_comments (id, task_id, body, author_id, author_name) 
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![&comment_id, &id, &body.body, &user.user_id, &author_name,],
    );

    match result {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({
                "id": comment_id,
                "task_id": id,
                "body": body.body,
                "author_id": user.user_id,
                "author_name": author_name,
                "created_at": chrono::Utc::now().to_rfc3339(),
            })),
        )
            .into_response(),
        Err(e) => {
            error!("Comment error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to add comment"})),
            )
                .into_response()
        }
    }
}

async fn get_task_audit_logs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
                .into_response()
        }
    };

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(_e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            )
                .into_response()
        }
    };
    if let Err(r) = check_owner(&conn, &user.user_id, &id) {
        return r.into_response();
    }

    let mut stmt = match conn.prepare(
        "SELECT id, task_id, action, actor_type, actor_id, payload, created_at 
         FROM task_audit_logs WHERE task_id = ?1 ORDER BY created_at DESC",
    ) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };

    let logs: Vec<serde_json::Value> = match stmt.query_map([&id], |row| {
        Ok(json!({
            "id": row.get::<_, String>(0)?,
            "task_id": row.get::<_, String>(1)?,
            "action": row.get::<_, String>(2)?,
            "actor_type": row.get::<_, String>(3)?,
            "actor_id": row.get::<_, String>(4)?,
            "payload": row.get::<_, Option<String>>(5)?,
            "created_at": row.get::<_, String>(6)?,
        }))
    }) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };

    (StatusCode::OK, Json(logs)).into_response()
}

fn write_audit_log(
    conn: &rusqlite::Connection,
    task_id: &str,
    action: &str,
    actor_type: &str,
    actor_id: &str,
    payload: Option<&str>,
) -> rusqlite::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO task_audit_logs (id, task_id, action, actor_type, actor_id, payload) 
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            &id,
            task_id,
            action,
            actor_type,
            actor_id,
            payload.unwrap_or(""),
        ],
    )?;
    Ok(())
}
