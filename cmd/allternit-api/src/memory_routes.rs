use axum::extract::Extension;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{delete, get, post},
    Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::auth::get_user;
use crate::auth::AuthUser;
use crate::session_memory_service::{
    delete_session_memory, list_session_memory, read_session_memory, write_session_memory,
    DeleteSessionMemoryQuery, ReadSessionMemoryQuery, WriteSessionMemoryRequest,
};
use crate::AppState;

pub fn memory_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memory", get(list_memory))
        .route("/memory/health", get(memory_health))
        .route("/memory/consolidate", post(memory_consolidate))
        .route(
            "/memory/documents",
            get(list_documents).post(create_document),
        )
        .route("/memory/events", get(list_events).post(create_event))
        .route("/memory/query", post(query_memory))
        .route("/memory/edges", get(list_edges))
        .route("/memory/entities", get(list_entities).post(create_entity))
        .route("/memory/stats", get(memory_stats))
        .route("/memory/session", get(read_session_memory_handler))
        .route("/memory/session/list", get(list_session_memory_handler))
        .route("/memory/session", post(write_session_memory_handler))
        .route("/memory/session", delete(delete_session_memory_handler))
        .route("/memory/v2/observation", post(record_observation_v2_handler))
        .route("/memory/v2/observations", get(list_observations_v2_handler))
        .route("/memory/v2/recall", post(recall_v2_handler))
        .route("/memory/v2/retain", post(retain_turn_v2_handler))
        .route("/memory/v2/facts", get(list_facts_v2_handler))
        .route("/memory/v2/entities", get(list_entities_v2_handler))
        .route("/memory/browser-history/visit", post(record_browser_visit_handler))
        .route("/memory/browser-history", get(list_browser_history_handler))
        .route("/memory/browser-history/search", get(search_browser_history_handler))
        .route("/memory/procedural", get(list_procedural_memory_handler).post(create_procedural_memory_handler))
        .route("/memory/procedural/extract", post(extract_procedural_memory_handler))
        .route("/memory/procedural/match", get(match_procedural_memory_handler))
        .route("/memory/procedural/:id/use", post(record_procedural_memory_use_handler))
        .route("/memory/procedural/:id", get(get_procedural_memory_handler).post(update_procedural_memory_handler).delete(delete_procedural_memory_handler))
}

// ── Health ──────────────────────────────────────────────────────────────────

async fn memory_health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let ollama_url = state.config.ollama_url();
    let memory_url = state.config.memory_url();

    let client = reqwest::Client::new();

    let ollama_connected = client
        .get(format!("{}/api/tags", ollama_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .is_ok();

    let db_connected = state.db.connect().is_ok();

    let watcher_active = client
        .get(format!("{}/health", memory_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .is_ok();

    Json(json!({
        "status": if ollama_connected && db_connected && watcher_active { "healthy" } else { "degraded" },
        "memory": {
            "ollamaConnected": ollama_connected,
            "databaseConnected": db_connected,
            "watcherActive": watcher_active,
        },
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

// ── List memory (stub) ──────────────────────────────────────────────────────

async fn list_memory() -> Json<serde_json::Value> {
    Json(json!({
        "documents": [],
        "events": [],
        "entities": [],
        "edges": [],
        "total": 0,
    }))
}

// ── Consolidate ─────────────────────────────────────────────────────────────

async fn memory_consolidate(
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    let memory_url = state.config.memory_url();

    let client = reqwest::Client::new();
    match client
        .post(format!(
            "{}/api/consolidate",
            memory_url.trim_end_matches('/')
        ))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
    {
        Ok(res) => {
            if let Ok(body) = res.json::<serde_json::Value>().await {
                (StatusCode::OK, Json(body))
            } else {
                (
                    StatusCode::OK,
                    Json(json!({
                        "success": true,
                        "message": "Consolidation triggered",
                    })),
                )
            }
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "success": false,
                "message": format!("Memory service unreachable: {}", e),
            })),
        ),
    }
}

// ── Documents ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DocQuery {
    agent_id: Option<String>,
}

#[derive(Serialize)]
struct MemoryDoc {
    id: String,
    agent_id: Option<String>,
    title: String,
    source_type: String,
    source_url: Option<String>,
    chunk_count: i64,
    is_indexed: bool,
    created_at: String,
    updated_at: String,
}

async fn list_documents(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(params): Query<DocQuery>,
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

    let mut sql = "SELECT id, agent_id, title, source_type, source_url, chunk_count, is_indexed, created_at, updated_at FROM memory_documents WHERE user_id = ?1".to_string();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user.user_id)];

    if let Some(ref agent_id) = params.agent_id {
        sql.push_str(" AND agent_id = ?");
        args.push(Box::new(agent_id.clone()));
    }
    sql.push_str(" ORDER BY updated_at DESC");

    let args_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "DB error"})),
            )
        }
    };

    let docs: Vec<MemoryDoc> = match stmt.query_map(rusqlite::params_from_iter(args_refs), |row| {
        Ok(MemoryDoc {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            title: row.get(2)?,
            source_type: row.get(3)?,
            source_url: row.get(4)?,
            chunk_count: row.get(5)?,
            is_indexed: row.get::<_, i64>(6)? != 0,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    }) {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => vec![],
    };

    (StatusCode::OK, Json(json!(docs)))
}

#[derive(Deserialize)]
struct CreateDocBody {
    agent_id: Option<String>,
    title: String,
    content: Option<String>,
    source_type: Option<String>,
    source_url: Option<String>,
    chunk_count: Option<i64>,
}

async fn create_document(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<CreateDocBody>,
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

    match conn.execute(
        "INSERT INTO memory_documents (id, user_id, agent_id, title, content, source_type, source_url, chunk_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            &id,
            &user.user_id,
            &body.agent_id,
            &body.title,
            &body.content,
            body.source_type.as_deref().unwrap_or("upload"),
            &body.source_url,
            body.chunk_count.unwrap_or(0),
        ],
    ) {
        Ok(_) => (StatusCode::CREATED, Json(json!({"id": id, "title": body.title, "isIndexed": false}))),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to create document"}))),
    }
}

// ── Events ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct EventQuery {
    agent_id: Option<String>,
    limit: Option<i64>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Serialize)]
struct MemoryEvent {
    id: String,
    timestamp: String,
    #[serde(rename = "type")]
    event_type: String,
    payload: Option<String>,
    agent_id: Option<String>,
    source: String,
}

async fn list_events(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(params): Query<EventQuery>,
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

    let limit = params.limit.unwrap_or(100);
    let mut sql = "SELECT id, timestamp, type, payload, agent_id, source FROM memory_events WHERE user_id = ?1".to_string();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user.user_id)];

    if let Some(ref agent_id) = params.agent_id {
        sql.push_str(" AND agent_id = ?");
        args.push(Box::new(agent_id.clone()));
    }
    if let Some(ref t) = params.event_type {
        sql.push_str(" AND type = ?");
        args.push(Box::new(t.clone()));
    }
    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    args.push(Box::new(limit));

    let args_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "DB error"})),
            )
        }
    };

    let events: Vec<MemoryEvent> =
        match stmt.query_map(rusqlite::params_from_iter(args_refs), |row| {
            Ok(MemoryEvent {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                event_type: row.get(2)?,
                payload: row.get(3)?,
                agent_id: row.get(4)?,
                source: row.get(5)?,
            })
        }) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(_) => vec![],
        };

    (StatusCode::OK, Json(json!(events)))
}

#[derive(Deserialize)]
struct CreateEventBody {
    agent_id: Option<String>,
    #[serde(rename = "type")]
    event_type: String,
    payload: Option<String>,
    source: Option<String>,
}

async fn create_event(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<CreateEventBody>,
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

    match conn.execute(
        "INSERT INTO memory_events (id, user_id, agent_id, type, payload, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            &id,
            &user.user_id,
            &body.agent_id,
            &body.event_type,
            &body.payload,
            body.source.as_deref().unwrap_or("user"),
        ],
    ) {
        Ok(_) => (StatusCode::CREATED, Json(json!({
            "id": id,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "type": body.event_type,
            "payload": body.payload,
        }))),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to create event"}))),
    }
}

// ── Query ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct QueryBody {
    query: String,
}

async fn query_memory(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<QueryBody>,
) -> impl axum::response::IntoResponse {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            )
        }
    };
    // Proxy to memory agent if available, otherwise return stub
    let memory_url = state.config.memory_url();

    let client = reqwest::Client::new();
    match client
        .post(format!("{}/api/query", memory_url))
        .header("Content-Type", "application/json")
        .header("x-allternit-user-id", _user.user_id)
        .body(json!({"query": body.query}).to_string())
        .send()
        .await
    {
        Ok(res) => {
            if let Ok(json) = res.json::<serde_json::Value>().await {
                (StatusCode::OK, Json(json))
            } else {
                (
                    StatusCode::OK,
                    Json(json!({"results": [], "query": body.query})),
                )
            }
        }
        Err(_) => (
            StatusCode::OK,
            Json(json!({"results": [], "query": body.query, "note": "Memory agent unavailable"})),
        ),
    }
}

// ── Edges ───────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct EdgeRow {
    id: String,
    source: String,
    relationship: String,
    target: String,
    confidence: f64,
    created_at: String,
}

async fn list_edges(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            ) as (StatusCode, Json<serde_json::Value>)
        }
    };

    let db = state.db.clone();
    let user_id = user.user_id;
    let source_filter = params.get("source").cloned();

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let query = if let Some(ref src) = source_filter {
            let mut stmt = conn.prepare(
                "SELECT id, source, relationship, target, confidence, created_at
                 FROM memory_edges WHERE user_id = ?1 AND source = ?2 ORDER BY created_at DESC LIMIT 500"
            )?;
            let rows = stmt.query_map(params![user_id, src], |row| {
                Ok(EdgeRow {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    relationship: row.get(2)?,
                    target: row.get(3)?,
                    confidence: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok::<_, rusqlite::Error>(rows)
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, source, relationship, target, confidence, created_at
                 FROM memory_edges WHERE user_id = ?1 ORDER BY created_at DESC LIMIT 500"
            )?;
            let rows = stmt.query_map(params![user_id], |row| {
                Ok(EdgeRow {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    relationship: row.get(2)?,
                    target: row.get(3)?,
                    confidence: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok::<_, rusqlite::Error>(rows)
        };
        query
    }).await;

    match rows {
        Ok(Ok(data)) => (StatusCode::OK, Json(json!({"edges": data}))),
        Ok(Err(e)) => {
            tracing::warn!("DB error listing edges: {}", e);
            (StatusCode::OK, Json(json!({"edges": []})))
        }
        Err(e) => {
            tracing::warn!("DB task panicked: {}", e);
            (StatusCode::OK, Json(json!({"edges": []})))
        }
    }
}

// ── Entities ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct EntityRow {
    id: String,
    entity_id: String,
    name: String,
    entity_type: String,
    content: Option<String>,
    vector_id: Option<String>,
    last_updated: String,
}

async fn list_entities(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            ) as (StatusCode, Json<serde_json::Value>)
        }
    };

    let db = state.db.clone();
    let user_id = user.user_id;
    let agent_id = params.get("agentId").cloned();
    let entity_type = params.get("type").cloned();
    let q = params.get("q").cloned();

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut sql = String::from(
            "SELECT id, entity_id, name, type, content, vector_id, last_updated
             FROM memory_entities WHERE user_id = ?1",
        );
        if agent_id.is_some() {
            sql.push_str(" AND agent_id = ?2");
        }
        if entity_type.is_some() {
            sql.push_str(" AND type = ?3");
        }
        if q.is_some() {
            sql.push_str(" AND (name LIKE ?4 OR content LIKE ?4)");
        }
        sql.push_str(" ORDER BY last_updated DESC LIMIT 200");

        let mut stmt = conn.prepare(&sql)?;
        let like = q.map(|s| format!("%{}%", s));
        let rows = match (&agent_id, &entity_type, &like) {
            (Some(a), Some(t), Some(l)) => {
                stmt.query_map(params![user_id, a, t, l], row_to_entity)?
            }
            (Some(a), Some(t), None) => stmt.query_map(params![user_id, a, t], row_to_entity)?,
            (Some(a), None, Some(l)) => stmt.query_map(params![user_id, a, l], row_to_entity)?,
            (Some(a), None, None) => stmt.query_map(params![user_id, a], row_to_entity)?,
            (None, Some(t), Some(l)) => stmt.query_map(params![user_id, t, l], row_to_entity)?,
            (None, Some(t), None) => stmt.query_map(params![user_id, t], row_to_entity)?,
            (None, None, Some(l)) => stmt.query_map(params![user_id, l], row_to_entity)?,
            (None, None, None) => stmt.query_map(params![user_id], row_to_entity)?,
        }
        .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match rows {
        Ok(Ok(data)) => (StatusCode::OK, Json(json!({"entities": data}))),
        Ok(Err(e)) => {
            tracing::warn!("DB error listing entities: {}", e);
            (StatusCode::OK, Json(json!({"entities": []})))
        }
        Err(e) => {
            tracing::warn!("DB task panicked: {}", e);
            (StatusCode::OK, Json(json!({"entities": []})))
        }
    }
}

fn row_to_entity(row: &rusqlite::Row) -> Result<EntityRow, rusqlite::Error> {
    Ok(EntityRow {
        id: row.get(0)?,
        entity_id: row.get(1)?,
        name: row.get(2)?,
        entity_type: row.get(3)?,
        content: row.get(4)?,
        vector_id: row.get(5)?,
        last_updated: row.get(6)?,
    })
}

#[derive(Deserialize)]
struct CreateEntityBody {
    #[serde(alias = "agentId")]
    agent_id: Option<String>,
    #[serde(alias = "entityId")]
    entity_id: Option<String>,
    name: String,
    #[serde(alias = "type")]
    entity_type: Option<String>,
    content: Option<String>,
    #[serde(alias = "vectorId")]
    vector_id: Option<String>,
}

async fn create_entity(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<CreateEntityBody>,
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

    let db = state.db.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let id2 = id.clone();
    let id3 = id.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO memory_entities (id, user_id, agent_id, entity_id, name, type, content, vector_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id2,
                user_id,
                body.agent_id,
                body.entity_id.unwrap_or_else(|| id3.clone()),
                body.name,
                body.entity_type.unwrap_or_else(|| "General".to_string()),
                body.content,
                body.vector_id,
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    }).await;

    match result {
        Ok(Ok(())) => (
            StatusCode::CREATED,
            Json(json!({"success": true, "entity": {"id": id}})),
        ),
        Ok(Err(e)) => {
            tracing::warn!("DB error creating entity: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
        Err(e) => {
            tracing::warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
        }
    }
}

// ── Stats ─────────────────────────────────────────────────────────────────────

async fn memory_stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
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

    let db = state.db.clone();
    let user_id = user.user_id;

    let stats =
        tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            let total_events: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_events WHERE user_id = ?1",
                    params![user_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let total_entities: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_entities WHERE user_id = ?1",
                    params![user_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let total_edges: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_edges WHERE user_id = ?1",
                    params![user_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let total_vectors: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memory_entities WHERE user_id = ?1 AND vector_id IS NOT NULL",
            params![user_id],
            |row| row.get(0),
        ).unwrap_or(0);
            Ok::<_, rusqlite::Error>((total_events, total_entities, total_edges, total_vectors))
        })
        .await;

    match stats {
        Ok(Ok((events, entities, edges, vectors))) => (
            StatusCode::OK,
            Json(json!({
                "memories": { "total": events },
                "insights": entities,
                "connections": edges,
                "vectors": vectors,
            })),
        ),
        _ => (
            StatusCode::OK,
            Json(json!({
                "memories": { "total": 0 },
                "insights": 0,
                "connections": 0,
                "vectors": 0,
            })),
        ),
    }
}

// ── Session Memory (model-facing `memory` tool) ───────────────────────────────

async fn read_session_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ReadSessionMemoryQuery>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            ) as (StatusCode, Json<serde_json::Value>)
        }
    };

    if params.session_id.is_empty() || params.key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "session_id and key are required"})),
        );
    }

    match read_session_memory(&state.db, &user.user_id, &params.session_id, &params.key) {
        Ok(Some(entry)) => (StatusCode::OK, Json(json!(entry))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({"key": params.key, "value": serde_json::Value::Null})),
        ),
        Err(e) => {
            tracing::warn!("Session memory read error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    }
}

async fn list_session_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl axum::response::IntoResponse {
    let user = match get_user(&headers) {
        Some(u) => u,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "Unauthorized"})),
            ) as (StatusCode, Json<serde_json::Value>)
        }
    };

    let session_id = match params.get("session_id") {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "session_id is required"})),
            );
        }
    };

    match list_session_memory(&state.db, &user.user_id, &session_id) {
        Ok(entries) => (StatusCode::OK, Json(json!({"entries": entries}))),
        Err(e) => {
            tracing::warn!("Session memory list error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    }
}

async fn write_session_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WriteSessionMemoryRequest>,
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

    if body.session_id.is_empty() || body.key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "session_id and key are required"})),
        );
    }

    match write_session_memory(&state.db, &user.user_id, &body.session_id, &body.key, &body.value) {
        Ok(entry) => (StatusCode::OK, Json(json!(entry))),
        Err(e) => {
            tracing::warn!("Session memory write error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    }
}

async fn delete_session_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<DeleteSessionMemoryQuery>,
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

    if params.session_id.is_empty() || params.key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "session_id and key are required"})),
        );
    }

    match delete_session_memory(&state.db, &user.user_id, &params.session_id, &params.key) {
        Ok(deleted) => (
            StatusCode::OK,
            Json(json!({"key": params.key, "deleted": deleted})),
        ),
        Err(e) => {
            tracing::warn!("Session memory delete error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
        }
    }
}

// ── Memory Kernel V2 Handlers ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListV2Query {
    pub agent_id: Option<String>,
    pub limit: Option<usize>,
}

async fn record_observation_v2_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<crate::memory_kernel_service::RecordObservationRequest>,
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

    match crate::memory_kernel_service::record_observation(
        &state.db,
        &user.user_id,
        payload.agent_id.as_deref(),
        payload.session_id.as_deref(),
        &payload.kind,
        &payload.content,
        payload.source.as_deref(),
    ) {
        Ok(id) => (StatusCode::CREATED, Json(json!({"id": id, "status": "recorded"}))),
        Err(e) => {
            tracing::warn!("Record observation error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn recall_v2_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<crate::memory_kernel_service::RecallQuery>,
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

    let limit = payload.limit.unwrap_or(10);
    match crate::memory_kernel_service::recall(
        &state.db,
        &user.user_id,
        payload.agent_id.as_deref(),
        payload.session_id.as_deref(),
        &payload.query,
        limit,
    ) {
        Ok(results) => (StatusCode::OK, Json(json!({"results": results, "count": results.len()}))),
        Err(e) => {
            tracing::warn!("Recall error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn retain_turn_v2_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<crate::memory_kernel_service::RetainTurnRequest>,
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

    match crate::memory_kernel_service::retain_turn(
        &state.db,
        &user.user_id,
        payload.agent_id.as_deref(),
        payload.session_id.as_deref(),
        &payload.role,
        &payload.content,
    ) {
        Ok(id) => (StatusCode::OK, Json(json!({"observation_id": id, "status": "retained"}))),
        Err(e) => {
            tracing::warn!("Retain turn error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn list_observations_v2_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ListV2Query>,
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

    let limit = params.limit.unwrap_or(50);
    match crate::memory_kernel_service::list_observations(
        &state.db,
        &user.user_id,
        params.agent_id.as_deref(),
        limit,
    ) {
        Ok(observations) => (StatusCode::OK, Json(json!({"observations": observations, "count": observations.len()}))),
        Err(e) => {
            tracing::warn!("List observations error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn list_facts_v2_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ListV2Query>,
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

    let limit = params.limit.unwrap_or(50);
    match crate::memory_kernel_service::list_facts(
        &state.db,
        &user.user_id,
        params.agent_id.as_deref(),
        limit,
    ) {
        Ok(facts) => (StatusCode::OK, Json(json!({"facts": facts, "count": facts.len()}))),
        Err(e) => {
            tracing::warn!("List facts error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn list_entities_v2_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ListV2Query>,
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

    let limit = params.limit.unwrap_or(50);
    match crate::memory_kernel_service::list_entities(
        &state.db,
        &user.user_id,
        params.agent_id.as_deref(),
        limit,
    ) {
        Ok(entities) => (StatusCode::OK, Json(json!({"entities": entities, "count": entities.len()}))),
        Err(e) => {
            tracing::warn!("List entities error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

// ── Browser history as memory ───────────────────────────────────────────────

async fn record_browser_visit_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<crate::browser_history_service::RecordVisitRequest>,
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

    match crate::browser_history_service::record_visit(&state.db, &user.user_id, &payload) {
        Ok(id) => (StatusCode::CREATED, Json(json!({"id": id, "status": "recorded"}))),
        Err(crate::browser_history_service::BrowserHistoryError::InvalidUrl(msg)) => {
            (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_url", "message": msg})))
        }
        Err(e) => {
            tracing::warn!("Browser history record error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

#[derive(Deserialize)]
struct BrowserHistoryQuery {
    agent_id: Option<String>,
    session_id: Option<String>,
    domain: Option<String>,
    since_hours: Option<i64>,
    limit: Option<usize>,
}

async fn list_browser_history_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<BrowserHistoryQuery>,
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

    let query = crate::browser_history_service::RecentHistoryQuery {
        agent_id: params.agent_id,
        session_id: params.session_id,
        domain: params.domain,
        since_hours: params.since_hours,
        limit: params.limit,
    };

    match crate::browser_history_service::list_recent(&state.db, &user.user_id, &query) {
        Ok(items) => (StatusCode::OK, Json(json!({"items": items, "count": items.len()}))),
        Err(e) => {
            tracing::warn!("Browser history list error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

#[derive(Deserialize)]
struct BrowserHistorySearchQuery {
    q: String,
    limit: Option<usize>,
}

async fn search_browser_history_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<BrowserHistorySearchQuery>,
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

    let limit = params.limit.unwrap_or(50);
    match crate::browser_history_service::search_history(&state.db, &user.user_id, &params.q, limit) {
        Ok(items) => (StatusCode::OK, Json(json!({"items": items, "count": items.len()}))),
        Err(e) => {
            tracing::warn!("Browser history search error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

// ── Site-specific procedural memory ─────────────────────────────────────────

#[derive(Deserialize)]
struct ExtractProceduralMemoryRequest {
    agent_id: Option<String>,
    session_id: String,
}

async fn extract_procedural_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ExtractProceduralMemoryRequest>,
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

    match crate::procedural_memory_service::extract_from_session_history(
        &state.db,
        &user.user_id,
        payload.agent_id.as_deref(),
        &payload.session_id,
    ) {
        Ok(Some(item)) => (StatusCode::CREATED, Json(json!({"memory": item, "extracted": true}))),
        Ok(None) => (StatusCode::OK, Json(json!({"extracted": false, "message": "insufficient history"}))),
        Err(e) => {
            tracing::warn!("Procedural memory extract error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn create_procedural_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<crate::procedural_memory_service::CreateProceduralMemoryRequest>,
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

    match crate::procedural_memory_service::create_procedural_memory(&state.db, &user.user_id, &payload) {
        Ok(item) => (StatusCode::CREATED, Json(json!({"memory": item}))),
        Err(crate::procedural_memory_service::ProceduralMemoryError::InvalidRequest(msg)) => {
            (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_request", "message": msg})))
        }
        Err(e) => {
            tracing::warn!("Procedural memory create error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

#[derive(Deserialize)]
struct ListProceduralMemoryQuery {
    agent_id: Option<String>,
    limit: Option<usize>,
}

async fn list_procedural_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ListProceduralMemoryQuery>,
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

    let limit = params.limit.unwrap_or(50);
    match crate::procedural_memory_service::list_procedural_memory(&state.db, &user.user_id, params.agent_id.as_deref(), limit) {
        Ok(items) => (StatusCode::OK, Json(json!({"memories": items, "count": items.len()}))),
        Err(e) => {
            tracing::warn!("Procedural memory list error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

#[derive(Deserialize)]
struct MatchProceduralMemoryQuery {
    agent_id: Option<String>,
    context: String,
    limit: Option<usize>,
}

async fn match_procedural_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<MatchProceduralMemoryQuery>,
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

    let limit = params.limit.unwrap_or(10);
    match crate::procedural_memory_service::find_matching_procedural_memory(
        &state.db,
        &user.user_id,
        params.agent_id.as_deref(),
        &params.context,
        limit,
    ) {
        Ok(items) => (StatusCode::OK, Json(json!({"memories": items, "count": items.len()}))),
        Err(e) => {
            tracing::warn!("Procedural memory match error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn get_procedural_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
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

    match crate::procedural_memory_service::get_procedural_memory_by_id(&state.db, &user.user_id, &id) {
        Ok(Some(item)) => (StatusCode::OK, Json(json!({"memory": item}))),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))),
        Err(e) => {
            tracing::warn!("Procedural memory get error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn update_procedural_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(payload): Json<crate::procedural_memory_service::UpdateProceduralMemoryRequest>,
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

    match crate::procedural_memory_service::update_procedural_memory(&state.db, &user.user_id, &id, &payload) {
        Ok(item) => (StatusCode::OK, Json(json!({"memory": item}))),
        Err(crate::procedural_memory_service::ProceduralMemoryError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"})))
        }
        Err(crate::procedural_memory_service::ProceduralMemoryError::InvalidRequest(msg)) => {
            (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_request", "message": msg})))
        }
        Err(e) => {
            tracing::warn!("Procedural memory update error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn record_procedural_memory_use_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
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

    match crate::procedural_memory_service::record_procedural_memory_use(&state.db, &user.user_id, &id) {
        Ok(item) => (StatusCode::OK, Json(json!({"memory": item}))),
        Err(crate::procedural_memory_service::ProceduralMemoryError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"})))
        }
        Err(e) => {
            tracing::warn!("Procedural memory use error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}

async fn delete_procedural_memory_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
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

    match crate::procedural_memory_service::delete_procedural_memory(&state.db, &user.user_id, &id) {
        Ok(true) => (StatusCode::NO_CONTENT, Json(json!({}))),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))),
        Err(e) => {
            tracing::warn!("Procedural memory delete error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()})))
        }
    }
}
