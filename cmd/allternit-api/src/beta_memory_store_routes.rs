//! Memory Stores API (`/beta/memory-stores`).
//!
//! A memory store is a named, user-scoped container agents can read/write
//! long-term memory into. A store owns its `redaction_policy` — the policy
//! applied to content before it is persisted or surfaced to a model — and
//! its contents: namespaced key/value entries (`beta_memory_entries`).
//!
//! Entry surface (all scoped to the owning user, same as the store CRUD):
//! - `GET /beta/memory-stores/:id/entries?namespace=&limit=&cursor=` — list,
//!   cursor-paginated (`created_at|id`, the repo convention).
//! - `GET /beta/memory-stores/:id/entries/:key?namespace=` — read one entry
//!   (namespace defaults to `default`).
//! - `PUT /beta/memory-stores/:id/entries/:key` — upsert `{value, namespace?}`.
//! - `DELETE /beta/memory-stores/:id/entries/:key?namespace=` — delete one.
//! - `GET /beta/memory-stores/:id/search?q=&limit=` — substring match over
//!   key + value within the store.
//!
//! Store GET/list responses include `entry_count` and `last_write_at` stats,
//! computed as subqueries at read time (no denormalized counters to keep in
//! sync). Deleting a store explicitly deletes its entries — connections from
//! `DbHandle::connect` do not enable SQLite foreign keys, so the cascade is
//! enforced in code.
//!
//! Session wiring: session create accepts `memory_store_ids` (validated the
//! same way as `vault_ids`, see `cloud_agents_routes`) and stores them on
//! `beta_sessions.metadata`. When a work task tied to such a session is
//! leased, `build_memory_context` loads the stores' entries grouped by
//! namespace into the task payload as a `memory_context` object (capped per
//! store, see the consts below). **Workers consume `memory_context`**: they
//! inject it into the agent's context so the agent can act on the user's
//! long-term memory.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, error::ApiError, AppState};

/// Namespace used when a request does not name one explicitly.
pub(crate) const DEFAULT_NAMESPACE: &str = "default";

/// Per-store cap on entries shipped in a `memory_context` payload.
pub(crate) const MEMORY_CONTEXT_MAX_ENTRIES_PER_STORE: usize = 100;

/// Per-store cap on total key+value bytes shipped in a `memory_context`
/// payload (64 KiB).
pub(crate) const MEMORY_CONTEXT_MAX_BYTES_PER_STORE: usize = 64 * 1024;

fn empty_object() -> Value {
    json!({})
}

pub fn beta_memory_store_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/beta/memory-stores",
            get(list_memory_stores).post(create_memory_store),
        )
        .route(
            "/beta/memory-stores/:id",
            get(get_memory_store).delete(delete_memory_store),
        )
        .route("/beta/memory-stores/:id/entries", get(list_memory_entries))
        .route(
            "/beta/memory-stores/:id/entries/:key",
            get(get_memory_entry).put(upsert_memory_entry).delete(delete_memory_entry),
        )
        .route("/beta/memory-stores/:id/search", get(search_memory_entries))
}

#[derive(Debug, Deserialize)]
struct CreateMemoryStoreBody {
    name: String,
    #[serde(default = "empty_object")]
    redaction_policy: Value,
    #[serde(default = "empty_object")]
    metadata: Value,
}

#[derive(Debug, Serialize)]
struct MemoryStoreRow {
    id: String,
    organization_id: Option<String>,
    name: String,
    redaction_policy: Value,
    metadata: Value,
    entry_count: i64,
    last_write_at: Option<String>,
    created_at: String,
    updated_at: String,
}

const MEMORY_STORE_SELECT: &str = "SELECT id, organization_id, name, redaction_policy, metadata,
    created_at, updated_at,
    (SELECT COUNT(*) FROM beta_memory_entries e WHERE e.store_id = beta_memory_stores.id),
    (SELECT MAX(updated_at) FROM beta_memory_entries e WHERE e.store_id = beta_memory_stores.id)
    FROM beta_memory_stores";

fn read_memory_store(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryStoreRow> {
    let redaction_policy: String = row.get(3)?;
    let metadata: String = row.get(4)?;
    Ok(MemoryStoreRow {
        id: row.get(0)?,
        organization_id: row.get(1)?,
        name: row.get(2)?,
        redaction_policy: serde_json::from_str(&redaction_policy).unwrap_or_else(|_| json!({})),
        metadata: serde_json::from_str(&metadata).unwrap_or_else(|_| json!({})),
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        entry_count: row.get(7)?,
        last_write_at: row.get(8)?,
    })
}

async fn create_memory_store(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateMemoryStoreBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if !body.redaction_policy.is_object() || !body.metadata.is_object() {
        return Err(ApiError::BadRequest(
            "redaction_policy and metadata must be objects".into(),
        ));
    }
    let db = state.db.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let result_id = id.clone();
    let name = name.to_string();
    let store = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO beta_memory_stores
             (id, user_id, organization_id, name, redaction_policy, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                user.user_id,
                user.organization_id,
                name,
                body.redaction_policy.to_string(),
                body.metadata.to_string()
            ],
        )?;
        conn.query_row(
            &format!("{MEMORY_STORE_SELECT} WHERE id = ?1"),
            params![id],
            read_memory_store,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            ApiError::BadRequest("a memory store with this name already exists".into())
        }
        other => ApiError::DbError(other.to_string()),
    })?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"memory_store": store, "id": result_id})),
    ))
}

async fn list_memory_stores(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "{MEMORY_STORE_SELECT} WHERE user_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt
            .query_map(params![user.user_id], read_memory_store)?
            .collect::<Result<Vec<MemoryStoreRow>, _>>()?;
        Ok::<Vec<MemoryStoreRow>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(json!({"memory_stores": rows})))
}

async fn get_memory_store(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    let user_id = user.user_id;
    let store = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            &format!("{MEMORY_STORE_SELECT} WHERE id = ?1 AND user_id = ?2"),
            params![id, user_id],
            read_memory_store,
        )
        .optional()
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??
    .ok_or_else(|| ApiError::NotFound("memory store not found".into()))?;
    Ok(Json(json!({"memory_store": store})))
}

async fn delete_memory_store(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let db = state.db.clone();
    let deleted = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        // DbHandle connections do not enable SQLite foreign keys, so the
        // entry cascade is enforced explicitly here.
        let deleted = tx.execute(
            "DELETE FROM beta_memory_stores WHERE id = ?1 AND user_id = ?2",
            params![id, user.user_id],
        )?;
        if deleted > 0 {
            tx.execute(
                "DELETE FROM beta_memory_entries WHERE store_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok::<usize, rusqlite::Error>(deleted)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    if deleted == 0 {
        return Err(ApiError::NotFound("memory store not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ─── Entries ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListEntriesQuery {
    namespace: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GetEntryQuery {
    namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpsertEntryBody {
    value: String,
    #[serde(default)]
    namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct MemoryEntryRow {
    id: String,
    store_id: String,
    namespace: String,
    key: String,
    value: String,
    created_at: String,
    updated_at: String,
}

const ENTRY_SELECT: &str = "SELECT id, store_id, namespace, key, value, created_at, updated_at
    FROM beta_memory_entries";

fn read_memory_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryEntryRow> {
    Ok(MemoryEntryRow {
        id: row.get(0)?,
        store_id: row.get(1)?,
        namespace: row.get(2)?,
        key: row.get(3)?,
        value: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn namespace_or_default(namespace: &Option<String>) -> String {
    namespace
        .as_deref()
        .map(str::trim)
        .filter(|ns| !ns.is_empty())
        .unwrap_or(DEFAULT_NAMESPACE)
        .to_string()
}

/// A store the caller owns, or 404 — the same semantics as the store CRUD
/// (non-owner ids are indistinguishable from missing ones).
fn load_owned_store(
    conn: &rusqlite::Connection,
    id: &str,
    user_id: &str,
) -> Result<(), ApiError> {
    let owned = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM beta_memory_stores WHERE id = ?1 AND user_id = ?2)",
            params![id, user_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| ApiError::DbError(e.to_string()))?;
    if !owned {
        return Err(ApiError::NotFound("memory store not found".into()));
    }
    Ok(())
}

async fn list_memory_entries(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(store_id): Path<String>,
    Query(query): Query<ListEntriesQuery>,
) -> Result<Json<Value>, ApiError> {
    let namespace = namespace_or_default(&query.namespace);
    let limit = query.limit.unwrap_or(50).min(100);
    let db = state.db.clone();
    let user_id = user.user_id;
    let entries = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        load_owned_store(&conn, &store_id, &user_id)?;
        // Cursor pagination on (created_at, id) — the repo convention (see
        // admin_audit_routes). Ties on the second-precision timestamp are
        // broken by id.
        let rows = if let Some(cursor) = query.cursor {
            let parts: Vec<&str> = cursor.split('|').collect();
            if parts.len() != 2 {
                return Err(ApiError::BadRequest(
                    "cursor must be created_at|id".into(),
                ));
            }
            let mut stmt = conn.prepare(&format!(
                "{ENTRY_SELECT} WHERE store_id = ?1 AND namespace = ?2
                 AND (created_at > ?3 OR (created_at = ?3 AND id > ?4))
                 ORDER BY created_at ASC, id ASC LIMIT ?5"
            ))?;
            let mapped = stmt.query_map(
                params![store_id, namespace, parts[0], parts[1], limit + 1],
                read_memory_entry,
            )?;
            mapped.collect::<Result<Vec<MemoryEntryRow>, _>>()?
        } else {
            let mut stmt = conn.prepare(&format!(
                "{ENTRY_SELECT} WHERE store_id = ?1 AND namespace = ?2
                 ORDER BY created_at ASC, id ASC LIMIT ?3"
            ))?;
            let mapped = stmt.query_map(params![store_id, namespace, limit + 1], read_memory_entry)?;
            mapped.collect::<Result<Vec<MemoryEntryRow>, _>>()?
        };
        let has_more = rows.len() > limit;
        let mut rows = rows;
        if has_more {
            rows.truncate(limit);
        }
        let next_cursor = has_more
            .then(|| rows.last().map(|r| format!("{}|{}", r.created_at, r.id)))
            .flatten();
        Ok::<Json<Value>, ApiError>(Json(json!({
            "entries": rows,
            "next_cursor": next_cursor,
            "limit": limit,
        })))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(entries)
}

async fn get_memory_entry(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((store_id, key)): Path<(String, String)>,
    Query(query): Query<GetEntryQuery>,
) -> Result<Json<Value>, ApiError> {
    let namespace = namespace_or_default(&query.namespace);
    let db = state.db.clone();
    let user_id = user.user_id;
    let entry = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        load_owned_store(&conn, &store_id, &user_id)?;
        conn.query_row(
            &format!(
                "{ENTRY_SELECT} WHERE store_id = ?1 AND namespace = ?2 AND key = ?3"
            ),
            params![store_id, namespace, key],
            read_memory_entry,
        )
        .optional()
        .map_err(|e| ApiError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??
    .ok_or_else(|| ApiError::NotFound("memory entry not found".into()))?;
    Ok(Json(json!({"entry": entry})))
}

async fn upsert_memory_entry(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((store_id, key)): Path<(String, String)>,
    Json(body): Json<UpsertEntryBody>,
) -> Result<Json<Value>, ApiError> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(ApiError::BadRequest("key must not be empty".into()));
    }
    let namespace = namespace_or_default(&body.namespace);
    let db = state.db.clone();
    let user_id = user.user_id;
    let entry = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        load_owned_store(&conn, &store_id, &user_id)?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO beta_memory_entries (id, store_id, namespace, key, value)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(store_id, namespace, key) DO UPDATE SET
                 value = excluded.value, updated_at = CURRENT_TIMESTAMP",
            params![id, store_id, namespace, key, body.value],
        )?;
        conn.query_row(
            &format!(
                "{ENTRY_SELECT} WHERE store_id = ?1 AND namespace = ?2 AND key = ?3"
            ),
            params![store_id, namespace, key],
            read_memory_entry,
        )
        .map_err(|e| ApiError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(json!({"entry": entry})))
}

async fn delete_memory_entry(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((store_id, key)): Path<(String, String)>,
    Query(query): Query<GetEntryQuery>,
) -> Result<StatusCode, ApiError> {
    let namespace = namespace_or_default(&query.namespace);
    let db = state.db.clone();
    let user_id = user.user_id;
    let deleted = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        load_owned_store(&conn, &store_id, &user_id)?;
        conn.execute(
            "DELETE FROM beta_memory_entries WHERE store_id = ?1 AND namespace = ?2 AND key = ?3",
            params![store_id, namespace, key],
        )
        .map_err(|e| ApiError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    if deleted == 0 {
        return Err(ApiError::NotFound("memory entry not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn search_memory_entries(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(store_id): Path<String>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let q = query.q.unwrap_or_default();
    if q.trim().is_empty() {
        return Err(ApiError::BadRequest("q is required".into()));
    }
    let limit = query.limit.unwrap_or(50).min(100);
    let like = format!("%{}%", q);
    let db = state.db.clone();
    let user_id = user.user_id;
    let entries = tokio::task::spawn_blocking(move || -> Result<Vec<MemoryEntryRow>, ApiError> {
        let conn = db.connect()?;
        load_owned_store(&conn, &store_id, &user_id)?;
        let mut stmt = conn.prepare(&format!(
            "{ENTRY_SELECT} WHERE store_id = ?1 AND (key LIKE ?2 OR value LIKE ?2)
             ORDER BY updated_at DESC LIMIT ?3"
        ))?;
        let rows = stmt
            .query_map(params![store_id, like, limit], read_memory_entry)?
            .collect::<Result<Vec<MemoryEntryRow>, _>>()?;
        Ok(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(json!({"entries": entries, "limit": limit})))
}

// ─── Worker memory context ───────────────────────────────────────────────────

/// Build the `memory_context` object injected into leased work-task payloads
/// for sessions bound to memory stores (metadata `memory_store_ids`). Per
/// store the entries are grouped by namespace; both the per-store entry count
/// and total key+value bytes are capped — an over-cap store yields a
/// `truncated: true` marker rather than failing the lease. Stores not owned
/// by the user are skipped (they cannot be bound at session create, so this
/// is only a defensive check for stores deleted after binding).
pub(crate) fn build_memory_context(
    conn: &rusqlite::Connection,
    user_id: &str,
    store_ids: &[String],
) -> Result<Value, rusqlite::Error> {
    let mut stores = Vec::new();
    for store_id in store_ids {
        let owned = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM beta_memory_stores WHERE id = ?1 AND user_id = ?2)",
            params![store_id, user_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !owned {
            continue;
        }
        let name: String = conn.query_row(
            "SELECT name FROM beta_memory_stores WHERE id = ?1",
            params![store_id],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(
            "SELECT namespace, key, value FROM beta_memory_entries
             WHERE store_id = ?1 ORDER BY namespace ASC, key ASC",
        )?;
        let rows = stmt
            .query_map(params![store_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<(String, String, String)>, _>>()?;
        let mut namespaces = Map::new();
        let mut count = 0usize;
        let mut total_bytes = 0usize;
        let mut truncated = false;
        for (namespace, key, value) in rows {
            let entry_bytes = key.len() + value.len();
            if count >= MEMORY_CONTEXT_MAX_ENTRIES_PER_STORE
                || total_bytes + entry_bytes > MEMORY_CONTEXT_MAX_BYTES_PER_STORE
            {
                truncated = true;
                break;
            }
            count += 1;
            total_bytes += entry_bytes;
            let namespace = namespaces
                .entry(namespace)
                .or_insert_with(|| json!({}));
            if let Some(object) = namespace.as_object_mut() {
                object.insert(key, json!(value));
            }
        }
        stores.push(json!({
            "store_id": store_id,
            "name": name,
            "namespaces": namespaces,
            "truncated": truncated,
        }));
    }
    Ok(json!({ "stores": stores }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::path::Path as FsPath;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    fn test_user(id: &str, org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: Some(format!("{}@example.test", id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: org_id.map(str::to_string),
            organization_role: None,
            organization_slug: None,
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "allternit-beta-memory-stores-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn test_app_state(temp: &FsPath) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
                "__test__".to_string(),
            ),
            fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn json_body(value: &Value) -> Body {
        Body::from(value.to_string())
    }

    #[test]
    fn empty_object_default_is_a_json_object() {
        assert!(empty_object().is_object());
    }

    #[tokio::test]
    async fn memory_store_crud_and_isolation() {
        let temp = temp_dir("crud");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state);

        // Create a store.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/memory-stores")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org-1")))
                    .body(json_body(&json!({
                        "name": "knowledge-base",
                        "redaction_policy": {"pii": true},
                        "metadata": {"domain": "support"}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let store_id = body["memory_store"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["memory_store"]["name"], json!("knowledge-base"));
        assert_eq!(body["memory_store"]["organization_id"], json!("org-1"));
        assert_eq!(body["memory_store"]["redaction_policy"]["pii"], json!(true));

        // Duplicate name for the same user is rejected.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/memory-stores")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", None))
                    .body(json_body(&json!({"name": "knowledge-base"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // List only shows the owner's stores.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/memory-stores")
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["memory_stores"].as_array().unwrap().len(), 1);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/memory-stores")
                    .extension(test_user("user-b", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["memory_stores"].as_array().unwrap().len(), 0);

        // Get by id.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Another user cannot get it.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}", store_id))
                    .extension(test_user("user-b", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Delete.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/beta/memory-stores/{}", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Getting it after deletion returns 404.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn rejects_invalid_store_input() {
        let temp = temp_dir("validation");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/memory-stores")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", None))
                    .body(json_body(&json!({"name": "   "})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/memory-stores")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", None))
                    .body(json_body(&json!({
                        "name": "ok",
                        "redaction_policy": "not-an-object"
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(&temp);
    }

    async fn create_store(app: &axum::Router, user: &str, name: &str) -> String {
        let resp = (*app)
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/memory-stores")
                    .header("content-type", "application/json")
                    .extension(test_user(user, None))
                    .body(json_body(&json!({"name": name})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        body["memory_store"]["id"].as_str().unwrap().to_string()
    }

    async fn put_entry(
        app: &axum::Router,
        user: &str,
        store_id: &str,
        key: &str,
        value: &str,
        namespace: Option<&str>,
    ) {
        let mut body = json!({"value": value});
        if let Some(ns) = namespace {
            body["namespace"] = json!(ns);
        }
        let resp = (*app)
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/beta/memory-stores/{}/entries/{}", store_id, key))
                    .header("content-type", "application/json")
                    .extension(test_user(user, None))
                    .body(json_body(&body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "key {key}");
    }

    #[tokio::test]
    async fn memory_entries_crud_roundtrip_and_isolation() {
        let temp = temp_dir("entries-crud");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state);
        let store_id = create_store(&app, "user-a", "kb").await;

        // Upsert into two namespaces + the default namespace.
        put_entry(&app, "user-a", &store_id, "color", "blue", None).await;
        put_entry(&app, "user-a", &store_id, "color", "red", Some("prefs")).await;
        put_entry(&app, "user-a", &store_id, "lang", "rust", None).await;

        // Default namespace read.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/entries/color", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["entry"]["value"], json!("blue"));
        assert_eq!(body["entry"]["namespace"], json!("default"));

        // Namespaced read of the same key returns the other value.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/beta/memory-stores/{}/entries/color?namespace=prefs",
                        store_id
                    ))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["entry"]["value"], json!("red"));

        // Upsert overwrites in place.
        put_entry(&app, "user-a", &store_id, "color", "green", None).await;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/entries/color", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["entry"]["value"], json!("green"));

        // List in the default namespace sees only its own keys.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/entries", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        let mut keys: Vec<&str> = body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["key"].as_str().unwrap())
            .collect();
        keys.sort();
        assert_eq!(keys, vec!["color", "lang"]);

        // Missing key and missing store are 404; another user's store is 404.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/entries/nope", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/entries/color", store_id))
                    .extension(test_user("user-b", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/beta/memory-stores/no-store/entries/k"))
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", None))
                    .body(json_body(&json!({"value": "v"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Delete one entry; the namespaced twin survives.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/beta/memory-stores/{}/entries/color", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/beta/memory-stores/{}/entries/color?namespace=prefs",
                        store_id
                    ))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Deleting a missing entry is 404.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/beta/memory-stores/{}/entries/color", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn memory_entries_paginate_with_cursor() {
        let temp = temp_dir("entries-page");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state);
        let store_id = create_store(&app, "user-a", "paginated").await;
        for i in 0..5 {
            put_entry(&app, "user-a", &store_id, &format!("k{i}"), "v", None).await;
        }

        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut page_sizes: Vec<usize> = Vec::new();
        for _ in 0..3 {
            let uri = match &cursor {
                Some(c) => format!(
                    "/beta/memory-stores/{}/entries?limit=2&cursor={}",
                    store_id,
                    urlencoding_encode(c)
                ),
                None => format!("/beta/memory-stores/{}/entries?limit=2", store_id),
            };
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(uri)
                        .extension(test_user("user-a", None))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_json(resp.into_body()).await;
            let keys: Vec<String> = body["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["key"].as_str().unwrap().to_string())
                .collect();
            page_sizes.push(keys.len());
            seen.extend(keys);
            cursor = body["next_cursor"].as_str().map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        // All five entries surface exactly once; the pages are 2+2+1.
        // (Within-second ties order by entry id, so compare as a set.)
        seen.sort();
        assert_eq!(seen, vec!["k0", "k1", "k2", "k3", "k4"]);
        assert_eq!(page_sizes, vec![2, 2, 1]);

        let _ = std::fs::remove_dir_all(&temp);
    }

    fn urlencoding_encode(value: &str) -> String {
        value
            .replace('|', "%7C")
            .replace(':', "%3A")
            .replace(' ', "%20")
    }

    #[tokio::test]
    async fn memory_entries_search_matches_key_and_value() {
        let temp = temp_dir("entries-search");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state);
        let store_id = create_store(&app, "user-a", "searchable").await;
        put_entry(&app, "user-a", &store_id, "favorite-color", "blue", None).await;
        put_entry(&app, "user-a", &store_id, "hobby", "skydiving", None).await;
        put_entry(&app, "user-a", &store_id, "lang", "rust", None).await;

        // Query matches a key substring.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/search?q=color", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        let keys: Vec<&str> = body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["key"].as_str().unwrap())
            .collect();
        assert_eq!(keys, vec!["favorite-color"]);

        // Query matches a value substring across namespaces.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/search?q=sky", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["entries"][0]["key"], json!("hobby"));

        // Empty query is a 400; searching someone else's store is a 404.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/search?q=", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}/search?q=rust", store_id))
                    .extension(test_user("user-b", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn store_delete_cascades_entries() {
        let temp = temp_dir("entries-cascade");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state.clone());
        let store_id = create_store(&app, "user-a", "doomed").await;
        put_entry(&app, "user-a", &store_id, "k1", "v", None).await;
        put_entry(&app, "user-a", &store_id, "k2", "v", Some("ns")).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/beta/memory-stores/{}", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Store row is gone; entries are too.
        let conn = state.db.connect().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM beta_memory_entries WHERE store_id = ?1",
                params![store_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        drop(conn);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn store_responses_include_entry_stats() {
        let temp = temp_dir("store-stats");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state);
        let store_id = create_store(&app, "user-a", "stats").await;

        // No entries yet: count 0, no last write.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["memory_store"]["entry_count"], json!(0));
        assert!(body["memory_store"]["last_write_at"].is_null());

        put_entry(&app, "user-a", &store_id, "k1", "v", None).await;
        put_entry(&app, "user-a", &store_id, "k2", "v", Some("ns")).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/memory-stores/{}", store_id))
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["memory_store"]["entry_count"], json!(2));
        assert!(body["memory_store"]["last_write_at"].is_string());

        // List responses carry the same stats.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/memory-stores")
                    .extension(test_user("user-a", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["memory_stores"][0]["entry_count"], json!(2));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn memory_context_groups_by_namespace_and_caps() {
        let temp = temp_dir("memory-context");
        let state = test_app_state(&temp).await;
        let app = beta_memory_store_router().with_state(state.clone());
        let store_id = create_store(&app, "user-a", "ctx").await;
        put_entry(&app, "user-a", &store_id, "color", "blue", None).await;
        put_entry(&app, "user-a", &store_id, "lang", "rust", Some("prefs")).await;

        let conn = state.db.connect().unwrap();
        let ctx = build_memory_context(&conn, "user-a", &[store_id.clone()]).unwrap();
        let store = &ctx["stores"][0];
        assert_eq!(store["store_id"], json!(store_id));
        assert_eq!(store["name"], json!("ctx"));
        assert_eq!(store["namespaces"]["default"]["color"], json!("blue"));
        assert_eq!(store["namespaces"]["prefs"]["lang"], json!("rust"));
        assert_eq!(store["truncated"], json!(false));

        // A store owned by someone else is skipped.
        let ctx = build_memory_context(&conn, "user-b", &[store_id.clone()]).unwrap();
        assert_eq!(ctx["stores"].as_array().unwrap().len(), 0);

        // Entry-count cap: 150 entries -> truncated after the cap.
        let big_store = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO beta_memory_stores (id, user_id, name) VALUES (?1, 'user-a', 'big')",
            params![big_store],
        )
        .unwrap();
        for i in 0..150 {
            conn.execute(
                "INSERT INTO beta_memory_entries (id, store_id, namespace, key, value)
                 VALUES (?1, ?2, 'default', ?3, 'x')",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    big_store,
                    format!("key-{i:04}")
                ],
            )
            .unwrap();
        }
        let ctx = build_memory_context(&conn, "user-a", &[big_store.clone()]).unwrap();
        let store = &ctx["stores"][0];
        assert_eq!(store["truncated"], json!(true));
        let entries: usize = store["namespaces"]["default"]
            .as_object()
            .unwrap()
            .len();
        assert_eq!(entries, MEMORY_CONTEXT_MAX_ENTRIES_PER_STORE);

        // Byte cap: one entry over 64 KiB alone -> truncated, zero entries.
        let wide_store = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO beta_memory_stores (id, user_id, name) VALUES (?1, 'user-a', 'wide')",
            params![wide_store],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO beta_memory_entries (id, store_id, namespace, key, value)
             VALUES (?1, ?2, 'default', 'blob', ?3)",
            params![
                uuid::Uuid::new_v4().to_string(),
                wide_store,
                "y".repeat(MEMORY_CONTEXT_MAX_BYTES_PER_STORE + 1)
            ],
        )
        .unwrap();
        let ctx = build_memory_context(&conn, "user-a", &[wide_store.clone()]).unwrap();
        let store = &ctx["stores"][0];
        assert_eq!(store["truncated"], json!(true));
        assert!(store["namespaces"].as_object().unwrap().is_empty());
        drop(conn);

        let _ = std::fs::remove_dir_all(&temp);
    }
}
