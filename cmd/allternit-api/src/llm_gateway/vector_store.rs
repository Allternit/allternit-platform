//! Vector Store & Semantic Search API (A7).
//!
//! Provides CRUD for vector stores and file attachments, plus semantic search
//! over stored file contents. Mounted under `/v1` by the LLM gateway router.
//!
//! Endpoints:
//! - `POST /v1/vector_stores` — create a vector store
//! - `GET /v1/vector_stores` — list vector stores
//! - `GET /v1/vector_stores/:id` — get a vector store
//! - `DELETE /v1/vector_stores/:id` — delete a vector store
//! - `POST /v1/vector_stores/:id/files` — attach a file to a vector store
//! - `GET /v1/vector_stores/:id/files` — list files in a vector store
//! - `DELETE /v1/vector_stores/:id/files/:file_id` — detach a file
//! - `POST /v1/vector_stores/:id/search` — semantic search over store contents

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::AppState;

use super::{
    auth::LlmKeyContext,
    translate::{error_code, OpenAiErrorResponse},
};

// ─── Router ─────────────────────────────────────────────────────────────────

pub fn vector_store_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/vector_stores", post(create_store).get(list_stores))
        .route(
            "/vector_stores/:id",
            get(get_store).delete(delete_store),
        )
        .route(
            "/vector_stores/:id/files",
            post(attach_file).get(list_files),
        )
        .route(
            "/vector_stores/:id/files/:file_id",
            delete(detach_file),
        )
        .route("/vector_stores/:id/search", post(search_store))
}

// ─── Request / Response types ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateStoreRequest {
    pub name: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub expires_after: Option<ExpiresAfter>,
}

#[derive(Debug, Deserialize)]
pub struct ExpiresAfter {
    pub anchor: String,
    pub days: u32,
}

#[derive(Debug, Serialize)]
pub struct VectorStoreObject {
    pub id: String,
    pub object: &'static str,
    pub name: String,
    pub status: &'static str,
    pub created_at: i64,
    pub file_counts: FileCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct FileCounts {
    pub in_progress: u32,
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
    pub total: u32,
}

#[derive(Debug, Deserialize)]
pub struct AttachFileRequest {
    pub file_id: String,
}

#[derive(Debug, Serialize)]
pub struct VectorStoreFileObject {
    pub id: String,
    pub object: &'static str,
    pub vector_store_id: String,
    pub file_id: String,
    pub status: &'static str,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: u32,
    #[serde(default)]
    pub filter: Option<serde_json::Value>,
}

fn default_max_results() -> u32 {
    10
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub file_id: String,
    pub content: String,
    pub score: f64,
}

// ─── DB helpers ─────────────────────────────────────────────────────────────

fn ensure_tables(db: &crate::db::DbHandle) -> rusqlite::Result<()> {
    let conn = db.connect()?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS vector_stores (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'completed',
            metadata_json TEXT,
            expires_at INTEGER,
            created_at INTEGER NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS vector_store_files (
            id TEXT PRIMARY KEY,
            vector_store_id TEXT NOT NULL,
            file_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'completed',
            created_at INTEGER NOT NULL,
            FOREIGN KEY (vector_store_id) REFERENCES vector_stores(id) ON DELETE CASCADE
        )",
        [],
    )?;
    Ok(())
}

// ─── Handlers ───────────────────────────────────────────────────────────────

async fn create_store(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Json(body): Json<CreateStoreRequest>,
) -> Response {
    if body.name.is_empty() {
        return OpenAiErrorResponse::invalid_request(
            "`name` must not be empty.",
            Some("name"),
        )
        .into_response();
    }

    if let Err(e) = ensure_tables(&state.db) {
        return OpenAiErrorResponse::upstream(
            format!("Database error: {e}"),
            "internal_error",
        )
        .into_response();
    }

    let id = format!("vs_{}", uuid::Uuid::new_v4().simple());
    let created_at = chrono::Utc::now().timestamp();
    let metadata_json = body
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default());
    let expires_at = body.expires_after.as_ref().map(|ea| {
        created_at + (ea.days as i64 * 86400)
    });

    let db = state.db.clone();
    let id_c = id.clone();
    let name = body.name.clone();
    let meta = metadata_json.clone();

    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO vector_stores (id, name, status, metadata_json, expires_at, created_at)
             VALUES (?1, ?2, 'completed', ?3, ?4, ?5)",
            params![id_c, name, meta, expires_at, created_at],
        )?;
        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let store = VectorStoreObject {
                id,
                object: "vector_store",
                name: body.name,
                status: "completed",
                created_at,
                file_counts: FileCounts {
                    in_progress: 0,
                    completed: 0,
                    failed: 0,
                    cancelled: 0,
                    total: 0,
                },
                metadata: body.metadata,
                expires_at,
            };
            (StatusCode::OK, Json(serde_json::to_value(store).unwrap())).into_response()
        }
        _ => OpenAiErrorResponse::upstream("Failed to create vector store.", "internal_error")
            .into_response(),
    }
}

async fn list_stores(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
) -> Response {
    if let Err(e) = ensure_tables(&state.db) {
        return OpenAiErrorResponse::upstream(
            format!("Database error: {e}"),
            "internal_error",
        )
        .into_response();
    }

    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, status, metadata_json, expires_at, created_at
             FROM vector_stores ORDER BY created_at DESC LIMIT 100",
        )?;
        let stores: Vec<VectorStoreObject> = stmt
            .query_map([], |row| {
                let metadata_str: Option<String> = row.get(3)?;
                let metadata = metadata_str
                    .and_then(|s| serde_json::from_str(&s).ok());
                Ok(VectorStoreObject {
                    id: row.get(0)?,
                    object: "vector_store",
                    name: row.get(1)?,
                    status: "completed",
                    created_at: row.get(5)?,
                    file_counts: FileCounts {
                        in_progress: 0,
                        completed: 0,
                        failed: 0,
                        cancelled: 0,
                        total: 0,
                    },
                    metadata,
                    expires_at: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok::<_, rusqlite::Error>(stores)
    })
    .await;

    match result {
        Ok(Ok(stores)) => (
            StatusCode::OK,
            Json(json!({ "object": "list", "data": stores })),
        )
            .into_response(),
        _ => OpenAiErrorResponse::upstream("Failed to list vector stores.", "internal_error")
            .into_response(),
    }
}

async fn get_store(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = ensure_tables(&state.db) {
        return OpenAiErrorResponse::upstream(
            format!("Database error: {e}"),
            "internal_error",
        )
        .into_response();
    }

    let db = state.db.clone();
    let id_c = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            "SELECT id, name, status, metadata_json, expires_at, created_at
             FROM vector_stores WHERE id = ?1",
            params![id_c],
            |row| {
                let metadata_str: Option<String> = row.get(3)?;
                let metadata = metadata_str
                    .and_then(|s| serde_json::from_str(&s).ok());
                Ok(VectorStoreObject {
                    id: row.get(0)?,
                    object: "vector_store",
                    name: row.get(1)?,
                    status: "completed",
                    created_at: row.get(5)?,
                    file_counts: FileCounts {
                        in_progress: 0,
                        completed: 0,
                        failed: 0,
                        cancelled: 0,
                        total: 0,
                    },
                    metadata,
                    expires_at: row.get(4)?,
                })
            },
        )
        .optional()
    })
    .await;

    match result {
        Ok(Ok(Some(store))) => {
            (StatusCode::OK, Json(serde_json::to_value(store).unwrap())).into_response()
        }
        Ok(Ok(None)) => OpenAiErrorResponse::new(
            StatusCode::NOT_FOUND,
            format!("No vector store with id '{id}'."),
            "invalid_request_error",
            Some("vector_store"),
            Some(error_code::INVALID_REQUEST),
        )
        .into_response(),
        _ => OpenAiErrorResponse::upstream("Failed to get vector store.", "internal_error")
            .into_response(),
    }
}

async fn delete_store(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = ensure_tables(&state.db) {
        return OpenAiErrorResponse::upstream(
            format!("Database error: {e}"),
            "internal_error",
        )
        .into_response();
    }

    let db = state.db.clone();
    let id_c = id.clone();
    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<bool> {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM vector_store_files WHERE vector_store_id = ?1",
            params![id_c],
        )?;
        let deleted = conn.execute(
            "DELETE FROM vector_stores WHERE id = ?1",
            params![id_c],
        )?;
        Ok(deleted > 0)
    })
    .await;

    match result {
        Ok(Ok(true)) => (
            StatusCode::OK,
            Json(json!({ "id": id, "object": "vector_store.deleted", "deleted": true })),
        )
            .into_response(),
        Ok(Ok(false)) => OpenAiErrorResponse::new(
            StatusCode::NOT_FOUND,
            format!("No vector store with id '{id}'."),
            "invalid_request_error",
            Some("vector_store"),
            Some(error_code::INVALID_REQUEST),
        )
        .into_response(),
        _ => OpenAiErrorResponse::upstream("Failed to delete vector store.", "internal_error")
            .into_response(),
    }
}

async fn attach_file(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Path(store_id): Path<String>,
    Json(body): Json<AttachFileRequest>,
) -> Response {
    if let Err(e) = ensure_tables(&state.db) {
        return OpenAiErrorResponse::upstream(
            format!("Database error: {e}"),
            "internal_error",
        )
        .into_response();
    }

    let id = format!("vsf_{}", uuid::Uuid::new_v4().simple());
    let created_at = chrono::Utc::now().timestamp();
    let db = state.db.clone();
    let id_c = id.clone();
    let sid = store_id.clone();
    let fid = body.file_id.clone();

    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
        let conn = db.connect()?;
        // Verify the store exists.
        let store_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM vector_stores WHERE id = ?1",
                params![sid],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !store_exists {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        conn.execute(
            "INSERT INTO vector_store_files (id, vector_store_id, file_id, status, created_at)
             VALUES (?1, ?2, ?3, 'completed', ?4)",
            params![id_c, sid, fid, created_at],
        )?;
        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let file_obj = VectorStoreFileObject {
                id,
                object: "vector_store.file",
                vector_store_id: store_id,
                file_id: body.file_id,
                status: "completed",
                created_at,
            };
            (StatusCode::OK, Json(serde_json::to_value(file_obj).unwrap())).into_response()
        }
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => OpenAiErrorResponse::new(
            StatusCode::NOT_FOUND,
            format!("No vector store with id '{store_id}'."),
            "invalid_request_error",
            Some("vector_store_id"),
            Some(error_code::INVALID_REQUEST),
        )
        .into_response(),
        _ => OpenAiErrorResponse::upstream("Failed to attach file.", "internal_error")
            .into_response(),
    }
}

async fn list_files(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Path(store_id): Path<String>,
) -> Response {
    if let Err(e) = ensure_tables(&state.db) {
        return OpenAiErrorResponse::upstream(
            format!("Database error: {e}"),
            "internal_error",
        )
        .into_response();
    }

    let db = state.db.clone();
    let sid = store_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, vector_store_id, file_id, status, created_at
             FROM vector_store_files WHERE vector_store_id = ?1
             ORDER BY created_at DESC",
        )?;
        let files: Vec<VectorStoreFileObject> = stmt
            .query_map(params![sid], |row| {
                Ok(VectorStoreFileObject {
                    id: row.get(0)?,
                    object: "vector_store.file",
                    vector_store_id: row.get(1)?,
                    file_id: row.get(2)?,
                    status: "completed",
                    created_at: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok::<_, rusqlite::Error>(files)
    })
    .await;

    match result {
        Ok(Ok(files)) => (
            StatusCode::OK,
            Json(json!({ "object": "list", "data": files })),
        )
            .into_response(),
        _ => OpenAiErrorResponse::upstream("Failed to list files.", "internal_error")
            .into_response(),
    }
}

async fn detach_file(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Path((store_id, file_id)): Path<(String, String)>,
) -> Response {
    if let Err(e) = ensure_tables(&state.db) {
        return OpenAiErrorResponse::upstream(
            format!("Database error: {e}"),
            "internal_error",
        )
        .into_response();
    }

    let db = state.db.clone();
    let sid = store_id.clone();
    let fid = file_id.clone();
    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<bool> {
        let conn = db.connect()?;
        let deleted = conn.execute(
            "DELETE FROM vector_store_files WHERE vector_store_id = ?1 AND file_id = ?2",
            params![sid, fid],
        )?;
        Ok(deleted > 0)
    })
    .await;

    match result {
        Ok(Ok(true)) => (
            StatusCode::OK,
            Json(json!({ "id": file_id, "object": "vector_store.file.deleted", "deleted": true })),
        )
            .into_response(),
        Ok(Ok(false)) => OpenAiErrorResponse::new(
            StatusCode::NOT_FOUND,
            format!("No file '{file_id}' in vector store '{store_id}'."),
            "invalid_request_error",
            Some("file_id"),
            Some(error_code::INVALID_REQUEST),
        )
        .into_response(),
        _ => OpenAiErrorResponse::upstream("Failed to detach file.", "internal_error")
            .into_response(),
    }
}

async fn search_store(
    State(state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Path(store_id): Path<String>,
    Json(body): Json<SearchRequest>,
) -> Response {
    if body.query.is_empty() {
        return OpenAiErrorResponse::invalid_request(
            "`query` must not be empty.",
            Some("query"),
        )
        .into_response();
    }

    if body.max_results > 100 {
        return OpenAiErrorResponse::invalid_request(
            "`max_results` must be at most 100.",
            Some("max_results"),
        )
        .into_response();
    }

    // Hybrid keyword + vector search over the store's files, through the
    // shared memory-plane index (memory_index). Files not chunked yet are
    // chunked now; their embeddings come from the background indexer, so a
    // fresh file is found by keyword at once and by meaning shortly after.
    let q = crate::memory_index::global()
        .embed_or_hash(&[body.query.clone()], crate::memory_index::InputType::Query)
        .await;
    let db = state.db.clone();
    let query = body.query.clone();
    let limit = body.max_results.max(1) as usize;
    let sid = store_id.clone();
    let found = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<Vec<SearchResult>>> {
        ensure_tables(&db)?;
        let conn = db.connect()?;
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM vector_stores WHERE id = ?1",
            params![sid],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let mut stmt = conn.prepare("SELECT file_id FROM vector_store_files WHERE vector_store_id = ?1")?;
        let file_ids: Vec<String> = stmt
            .query_map(params![sid], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        crate::memory_index::chunk_pending_files(&conn, Some(&file_ids), 256)?;
        let hits = crate::memory_index::search_files(
            &conn,
            &file_ids,
            &query,
            q.vectors.first().map(|v| (q.model.as_str(), v.as_slice())),
            limit,
        )?;
        Ok(Some(
            hits.into_iter()
                .map(|(file_id, content, score)| SearchResult { file_id, content, score })
                .collect(),
        ))
    })
    .await;
    let results: Vec<SearchResult> = match found {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) => {
            return OpenAiErrorResponse::new(
                StatusCode::NOT_FOUND,
                format!("No vector store with id '{store_id}'."),
                "invalid_request_error",
                Some("vector_store_id"),
                Some(error_code::INVALID_REQUEST),
            )
            .into_response()
        }
        _ => {
            return OpenAiErrorResponse::upstream("Vector store search failed.", "internal_error")
                .into_response()
        }
    };

    (
        StatusCode::OK,
        Json(json!({
            "object": "list",
            "data": results,
            "has_more": false,
        })),
    )
        .into_response()
}
