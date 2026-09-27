//! Task Management Routes
//!
//! CRUD operations for tasks — supports personal and workspace-scoped tasks.

use axum::extract::Extension;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::error;

use crate::auth::get_user;
use crate::auth::AuthUser;
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
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateTaskRequest {
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
    #[serde(default)]
    pub metadata: Option<String>,
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

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListTasksQuery>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            );
        }
    };

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => {
            error!("DB error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            );
        }
    };

    let mut sql = "SELECT id, user_id, workspace_id, title, description, status, priority, assignee_id, due_date, tags, metadata, created_at, updated_at, assignee_type, assignee_name FROM tasks WHERE user_id = ?1".to_string();
    let mut param_count = 1;
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user.user_id.clone())];

    if let Some(ref ws) = query.workspace_id {
        param_count += 1;
        sql.push_str(&format!(" AND workspace_id = ?{}", param_count));
        params.push(Box::new(ws.clone()));
    }
    if let Some(ref st) = query.status {
        param_count += 1;
        sql.push_str(&format!(" AND status = ?{}", param_count));
        params.push(Box::new(st.clone()));
    }

    let limit = query.limit.unwrap_or(100) as i64;
    let offset = query.offset.unwrap_or(0) as i64;

    param_count += 1;
    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", param_count));
    params.push(Box::new(limit));

    param_count += 1;
    sql.push_str(&format!(" OFFSET ?{}", param_count));
    params.push(Box::new(offset));

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            error!("SQL prepare error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Query error"})),
            );
        }
    };

    let tasks: Vec<Task> = match stmt.query_map(
        rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        row_to_task,
    ) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            error!("Query error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Query error"})),
            );
        }
    };

    (
        StatusCode::OK,
        Json(json!({ "tasks": tasks, "count": tasks.len() })),
    )
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            );
        }
    };

    let id = uuid::Uuid::new_v4().to_string();
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => {
            error!("DB error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            );
        }
    };

    let priority_str = match &body.priority {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        _ => "50".to_string(),
    };

    let result = conn.execute(
        "INSERT INTO tasks
          (id, user_id, workspace_id, title, description, status, priority, assignee_id, due_date, tags, metadata, assignee_type, assignee_name)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            &id,
            &user.user_id,
            body.workspace_id.as_deref().unwrap_or(""),
            &body.title,
            body.description.as_deref().unwrap_or(""),
            body.status.as_deref().unwrap_or("todo"),
            &priority_str,
            body.assignee_id.as_deref().unwrap_or(""),
            body.due_date.as_deref().unwrap_or(""),
            body.tags.as_deref().unwrap_or(""),
            body.metadata.as_deref().unwrap_or(""),
            body.assignee_type.as_deref().unwrap_or(""),
            body.assignee_name.as_deref().unwrap_or(""),
        ],
    );

    match result {
        Ok(_) => {
            let _ = write_audit_log(
                &conn,
                &id,
                "create",
                "human",
                &user.user_id,
                Some(&serde_json::to_string(&body).unwrap_or_default()),
            );
            match get_task_by_id(&conn, &id) {
                Ok(Some(task)) => (StatusCode::CREATED, Json(json!({ "task": task }))),
                _ => (StatusCode::CREATED, Json(json!({ "id": id }))),
            }
        }
        Err(e) => {
            error!("Insert error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to create task"})),
            )
        }
    }
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            );
        }
    };

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => {
            error!("DB error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            );
        }
    };

    match get_task_by_id(&conn, &id) {
        Ok(Some(task)) => {
            if task.user_id != user.user_id {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": "Access denied"})),
                );
            }
            (StatusCode::OK, Json(json!({ "task": task })))
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Task not found"})),
        ),
        Err(e) => {
            error!("Query error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Query error"})),
            )
        }
    }
}

async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<UpdateTaskRequest>,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            );
        }
    };

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => {
            error!("DB error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            );
        }
    };

    // Verify ownership
    match get_task_by_id(&conn, &id) {
        Ok(Some(ref task)) if task.user_id != user.user_id => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "Access denied"})),
            );
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Task not found"})),
            );
        }
        _ => {}
    }

    let mut updates: Vec<String> = Vec::new();
    let mut param_count = 0;
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref title) = body.title {
        param_count += 1;
        updates.push(format!("title = ?{}", param_count));
        params.push(Box::new(title.clone()));
    }
    if let Some(ref description) = body.description {
        param_count += 1;
        updates.push(format!("description = ?{}", param_count));
        params.push(Box::new(description.clone()));
    }
    if let Some(ref status) = body.status {
        param_count += 1;
        updates.push(format!("status = ?{}", param_count));
        params.push(Box::new(status.clone()));
    }
    if let Some(ref priority) = body.priority {
        param_count += 1;
        updates.push(format!("priority = ?{}", param_count));
        let priority_str = match priority {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => "50".to_string(),
        };
        params.push(Box::new(priority_str));
    }
    if let Some(ref assignee_id) = body.assignee_id {
        param_count += 1;
        updates.push(format!("assignee_id = ?{}", param_count));
        params.push(Box::new(assignee_id.clone()));
    }
    if let Some(ref due_date) = body.due_date {
        param_count += 1;
        updates.push(format!("due_date = ?{}", param_count));
        params.push(Box::new(due_date.clone()));
    }
    if let Some(ref tags) = body.tags {
        param_count += 1;
        updates.push(format!("tags = ?{}", param_count));
        params.push(Box::new(tags.clone()));
    }
    if let Some(ref metadata) = body.metadata {
        param_count += 1;
        updates.push(format!("metadata = ?{}", param_count));
        params.push(Box::new(metadata.clone()));
    }
    if let Some(ref assignee_type) = body.assignee_type {
        param_count += 1;
        updates.push(format!("assignee_type = ?{}", param_count));
        params.push(Box::new(assignee_type.clone()));
    }
    if let Some(ref assignee_name) = body.assignee_name {
        param_count += 1;
        updates.push(format!("assignee_name = ?{}", param_count));
        params.push(Box::new(assignee_name.clone()));
    }

    if updates.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "No fields to update"})),
        );
    }

    let sql = format!(
        "UPDATE tasks SET {}, updated_at = CURRENT_TIMESTAMP WHERE id = ?{}",
        updates.join(", "),
        param_count + 1
    );
    params.push(Box::new(id.clone()));

    match conn.execute(&sql, rusqlite::params_from_iter(params)) {
        Ok(0) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Task not found"})),
        ),
        Ok(_) => {
            let _ = write_audit_log(
                &conn,
                &id,
                "update",
                "human",
                &user.user_id,
                Some(&serde_json::to_string(&body).unwrap_or_default()),
            );
            match get_task_by_id(&conn, &id) {
                Ok(Some(task)) => (StatusCode::OK, Json(json!({ "task": task }))),
                _ => (StatusCode::OK, Json(json!({ "updated": true }))),
            }
        }
        Err(e) => {
            error!("Update error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to update task"})),
            )
        }
    }
}

async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            );
        }
    };

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => {
            error!("DB error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Database error"})),
            );
        }
    };

    // Verify ownership
    match get_task_by_id(&conn, &id) {
        Ok(Some(ref task)) if task.user_id != user.user_id => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "Access denied"})),
            );
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Task not found"})),
            );
        }
        _ => {}
    }

    match conn.execute("DELETE FROM tasks WHERE id = ?1", [&id]) {
        Ok(0) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Task not found"})),
        ),
        Ok(_) => {
            let _ = write_audit_log(&conn, &id, "delete", "human", &user.user_id, None);
            (StatusCode::NO_CONTENT, Json(serde_json::Value::Null))
        }
        Err(e) => {
            error!("Delete error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to delete task"})),
            )
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn get_task_by_id(conn: &rusqlite::Connection, id: &str) -> rusqlite::Result<Option<Task>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, workspace_id, title, description, status, priority,
                assignee_id, due_date, tags, metadata, created_at, updated_at,
                assignee_type, assignee_name
         FROM tasks WHERE id = ?1",
    )?;

    let mut rows = stmt.query_map([id], row_to_task)?;
    rows.next().transpose()
}

fn row_to_task(row: &rusqlite::Row) -> rusqlite::Result<Task> {
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
    headers: HeaderMap,
    Json(body): Json<AssignTaskRequest>,
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

    let result = conn.execute(
        "UPDATE tasks 
         SET assignee_type = ?1, assignee_id = ?2, assignee_name = ?3, updated_at = CURRENT_TIMESTAMP 
         WHERE id = ?4 AND user_id = ?5",
        rusqlite::params![
            body.assignee_type.as_deref().unwrap_or(""),
            body.assignee_id.as_deref().unwrap_or(""),
            body.assignee_name.as_deref().unwrap_or(""),
            &id,
            &user.user_id,
        ],
    );

    match result {
        Ok(_) => {
            let _ = write_audit_log(
                &conn,
                &id,
                "assign",
                "human",
                &user.user_id,
                Some(&serde_json::to_string(&body).unwrap_or_default()),
            );
            match get_task_by_id(&conn, &id) {
                Ok(Some(task)) => (StatusCode::OK, Json(task)).into_response(),
                _ => (StatusCode::OK, Json(json!({"id": id}))).into_response(),
            }
        }
        Err(e) => {
            error!("Assign error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to assign task"})),
            )
                .into_response()
        }
    }
}

async fn list_task_comments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let _user = match get_user(&headers) {
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
    let _user = match get_user(&headers) {
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
