use axum::extract::Extension;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::auth::get_user;
use crate::auth::AuthUser;
use crate::AppState;

pub fn inbox_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/inbox",
            get(list_inbox).patch(update_inbox).post(create_inbox),
        )
        .merge(crate::inbox_needs::needs_router())
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
    #[serde(rename = "type")]
    item_type: Option<String>,
    limit: Option<i64>,
    /// `needs_you` returns only the aggregated owner inbox (`needsYou`).
    scope: Option<String>,
    /// How long an inbound message may sit before it counts as unanswered.
    minutes: Option<i64>,
}

#[derive(Serialize)]
struct InboxItem {
    id: String,
    agent_id: Option<String>,
    #[serde(rename = "type")]
    item_type: String,
    title: String,
    body: Option<String>,
    severity: String,
    status: String,
    action_url: Option<String>,
    metadata: Option<serde_json::Value>,
    created_at: String,
}

async fn list_inbox(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(params): Query<ListQuery>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
        }
    };
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("inbox: db connect failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"items": [], "unreadCount": 0})),
            )
        }
    };

    // The owner-level "Needs you" list, derived live from every source (inbox_needs.rs).
    let minutes = params.minutes.unwrap_or(crate::inbox_needs::DEFAULT_UNANSWERED_MINUTES).clamp(1, 60 * 24);
    let needs_you = match crate::inbox_needs::collect(&conn, &user.user_id, minutes) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("inbox: needs-you collect failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"items": [], "unreadCount": 0, "needsYou": [], "needsYouCount": 0})),
            );
        }
    };
    if params.scope.as_deref() == Some("needs_you") {
        return (
            StatusCode::OK,
            Json(json!({"needsYou": needs_you, "needsYouCount": needs_you.len()})),
        );
    }

    let limit = params.limit.unwrap_or(50);
    let user_id = user.user_id.clone();
    let mut sql = "SELECT id, agent_id, type, title, body, severity, status, action_url, metadata, created_at FROM inbox_items WHERE user_id = ?1".to_string();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    if let Some(ref status) = params.status {
        sql.push_str(" AND status = ?");
        args.push(Box::new(status.clone()));
    }
    if let Some(ref t) = params.item_type {
        sql.push_str(" AND type = ?");
        args.push(Box::new(t.clone()));
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    args.push(Box::new(limit));

    let args_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"items": [], "unreadCount": 0})),
            )
        }
    };

    let items: Vec<InboxItem> = match stmt.query_map(rusqlite::params_from_iter(args_refs), |row| {
        let metadata_str: Option<String> = row.get(8)?;
        let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
        Ok(InboxItem {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            item_type: row.get(2)?,
            title: row.get(3)?,
            body: row.get(4)?,
            severity: row.get(5)?,
            status: row.get(6)?,
            action_url: row.get(7)?,
            metadata,
            created_at: row.get(9)?,
        })
    }) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => vec![],
    };

    let user_id = user.user_id.clone();
    let unread_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM inbox_items WHERE user_id = ?1 AND status = 'unread'",
            [&user_id],
            |row| row.get(0),
        )
        .unwrap_or(0);

    (
        StatusCode::OK,
        Json(json!({"items": items, "unreadCount": unread_count, "needsYou": needs_you, "needsYouCount": needs_you.len()})),
    )
}

#[derive(Deserialize)]
struct UpdateBody {
    item_id: String,
    status: String,
}

async fn update_inbox(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<UpdateBody>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
        }
    };
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "DB error"})),
            )
        }
    };

    // Verify ownership
    let owned: bool = conn
        .query_row(
            "SELECT 1 FROM inbox_items WHERE id = ?1 AND user_id = ?2",
            [&body.item_id, &user.user_id],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !owned {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Access denied"})),
        );
    }

    match conn.execute(
        "UPDATE inbox_items SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2 AND user_id = ?3",
        [&body.status, &body.item_id, &user.user_id],
    ) {
        Ok(_) => (StatusCode::OK, Json(json!({"success": true}))),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to update"}))),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    agent_id: Option<String>,
    #[serde(rename = "type")]
    item_type: String,
    title: String,
    body: Option<String>,
    severity: Option<String>,
    action_url: Option<String>,
    metadata: Option<serde_json::Value>,
}

async fn create_inbox(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
        }
    };
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "DB error"})),
            )
        }
    };

    let id = uuid::Uuid::new_v4().to_string();
    let metadata_str = body.metadata.map(|m| m.to_string());

    match conn.execute(
        "INSERT INTO inbox_items (id, user_id, agent_id, type, title, body, severity, status, action_url, metadata) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'unread', ?8, ?9)",
        rusqlite::params![
            &id,
            &user.user_id,
            &body.agent_id,
            &body.item_type,
            &body.title,
            &body.body,
            body.severity.as_deref().unwrap_or("info"),
            &body.action_url,
            &metadata_str,
        ],
    ) {
        Ok(_) => (StatusCode::CREATED, Json(json!({"success": true, "id": id}))),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to create"}))),
    }
}
