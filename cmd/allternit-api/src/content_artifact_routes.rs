//! Content-artifact API routes — A:// Artifacts API Phase 1
//! (docs/design/artifacts-api.md §3).
//!
//! Renderable, HTML-first artifacts with append-only immutable versions,
//! idempotent create/append, cursor pagination, and soft delete. Served under
//! the protected `/api/v1` nest alongside `artifact_router()`; distinct from
//! the sectioned-document model in `artifact_routes.rs`.
//!
//! Phase 1 decisions implemented here (2026-09-12):
//! - Version retention: cap per artifact (default 50, env
//!   `ALLTERNIT_CONTENT_ARTIFACT_MAX_VERSIONS`), oldest versions pruned at
//!   append time so the append-only store never grows unbounded.
//! - Storage: bodies ≤ 256 KB inline in the version row; larger bodies on
//!   disk under `<data_dir>/content-artifacts/<artifact_id>/<version>.<ext>`
//!   with `storage = 'file'`.
//! - Idempotency: `Idempotency-Key` header or `idempotencyKey` body field,
//!   keyed on (user_id, key), 24h TTL swept lazily on write. A replayed
//!   create returns the original artifact (200, not 201); a replayed append
//!   returns the original version number.

use axum::extract::Extension;
use axum::{
    extract::{Json, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Deserialize;
use serde_json::json;
use sha2::Digest;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

pub fn content_artifact_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/content-artifacts",
            get(list_content_artifacts).post(create_content_artifact),
        )
        .route(
            "/content-artifacts/:id",
            get(get_content_artifact)
                .patch(update_content_artifact)
                .delete(delete_content_artifact),
        )
        .route(
            "/content-artifacts/:id/versions",
            get(list_content_artifact_versions).put(append_content_artifact_version),
        )
        .route(
            "/content-artifacts/:id/versions/:version",
            get(get_content_artifact_version),
        )
}

// ═══════════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════════

/// Bodies at or below this size stay inline in `content_artifact_versions.body`;
/// larger bodies are written to disk under `<data_dir>/content-artifacts/…`.
const INLINE_BODY_MAX_BYTES: usize = 256 * 1024;

/// Default per-artifact version cap (design §8, decided 2026-09-12).
const DEFAULT_VERSION_CAP: i64 = 50;

/// Idempotency records older than this are swept lazily on write (§3).
const IDEMPOTENCY_TTL_HOURS: i64 = 24;

/// Per-artifact version retention cap; overridable via env so an admin can
/// tune it without a rebuild (same pattern as the memory-store caps).
fn version_cap() -> i64 {
    std::env::var("ALLTERNIT_CONTENT_ARTIFACT_MAX_VERSIONS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_VERSION_CAP)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Request bodies
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct CreateBody {
    title: String,
    #[serde(rename = "type")]
    artifact_type: Option<String>,
    body: String,
    #[serde(alias = "projectId")]
    project_id: Option<String>,
    #[serde(alias = "sourceSessionId")]
    source_session_id: Option<String>,
    prompt: Option<String>,
    #[serde(alias = "designSystemId")]
    design_system_id: Option<String>,
    #[serde(alias = "skillId")]
    skill_id: Option<String>,
    #[serde(alias = "skillName")]
    skill_name: Option<String>,
    #[serde(alias = "sandboxPolicy")]
    sandbox_policy: Option<String>,
    thumbnail: Option<String>,
    #[serde(alias = "idempotencyKey")]
    idempotency_key: Option<String>,
}

#[derive(Deserialize)]
struct AppendBody {
    body: String,
    thumbnail: Option<String>,
    #[serde(alias = "idempotencyKey")]
    idempotency_key: Option<String>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(rename = "type")]
    artifact_type: Option<String>,
    #[serde(alias = "projectId")]
    project: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════════

pub(crate) fn sha256_hex(body: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Extension for on-disk body storage, derived from the MIME type (§4).
fn body_ext(artifact_type: &str) -> &'static str {
    match artifact_type {
        "text/html" => "html",
        "image/svg+xml" => "svg",
        "text/markdown" | "text/md" => "md",
        _ => "bin",
    }
}

/// The idempotency key from the header (preferred) or the body field (§3).
fn idempotency_key(headers: &HeaderMap, body_key: Option<String>) -> Option<String> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| body_key.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Delete expired idempotency records (24h TTL, swept lazily on write).
fn sweep_expired_idempotency(tx: &Transaction) -> Result<(), rusqlite::Error> {
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(IDEMPOTENCY_TTL_HOURS);
    tx.execute(
        "DELETE FROM content_artifact_idempotency WHERE created_at < ?1",
        params![cutoff.to_rfc3339()],
    )?;
    Ok(())
}

/// Insert the idempotency record for a write. Returns false when a concurrent
/// write already claimed the key, in which case the caller should fetch and
/// replay the recorded result instead of double-writing.
fn record_idempotency(
    tx: &Transaction,
    user_id: &str,
    key: &str,
    artifact_id: &str,
    version: Option<i64>,
) -> Result<bool, rusqlite::Error> {
    let now = now_rfc3339();
    let rows = tx.execute(
        "INSERT OR IGNORE INTO content_artifact_idempotency
             (key, user_id, artifact_id, version, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![key, user_id, artifact_id, version, &now],
    )?;
    Ok(rows > 0)
}

/// Look up an idempotency record.
fn find_idempotency(
    conn: &Connection,
    user_id: &str,
    key: &str,
) -> Result<Option<(String, Option<i64>)>, rusqlite::Error> {
    let row = conn
        .query_row(
            "SELECT artifact_id, version FROM content_artifact_idempotency
             WHERE user_id = ?1 AND key = ?2",
            params![user_id, key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .ok();
    Ok(row)
}

pub(crate) struct ArtifactMeta {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) artifact_type: String,
    pub(crate) project_id: Option<String>,
    pub(crate) source_session_id: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) design_system_id: Option<String>,
    pub(crate) skill_id: Option<String>,
    pub(crate) skill_name: Option<String>,
    pub(crate) sandbox_policy: String,
    pub(crate) thumbnail: Option<String>,
    pub(crate) current_version: i64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

fn read_artifact_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactMeta> {
    Ok(ArtifactMeta {
        id: row.get(0)?,
        title: row.get(1)?,
        artifact_type: row.get(2)?,
        project_id: row.get(3)?,
        source_session_id: row.get(4)?,
        prompt: row.get(5)?,
        design_system_id: row.get(6)?,
        skill_id: row.get(7)?,
        skill_name: row.get(8)?,
        sandbox_policy: row.get(9)?,
        thumbnail: row.get(10)?,
        current_version: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

const META_SELECT: &str = "SELECT id, title, type, project_id, source_session_id, prompt,
    design_system_id, skill_id, skill_name, sandbox_policy, thumbnail,
    current_version, created_at, updated_at
    FROM content_artifacts";

/// Fetch an artifact's metadata scoped to the user, excluding soft-deleted rows.
pub(crate) fn fetch_artifact_meta(
    conn: &Connection,
    artifact_id: &str,
    user_id: &str,
) -> Result<Option<ArtifactMeta>, rusqlite::Error> {
    conn.query_row(
        &format!("{META_SELECT} WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL"),
        params![artifact_id, user_id],
        read_artifact_meta,
    )
    .optional()
}

/// Serialize artifact metadata in the §3 response shape. `body`/`version` are
/// attached by the caller when serving a read.
fn artifact_json(meta: &ArtifactMeta) -> serde_json::Value {
    json!({
        "id": meta.id,
        "address": format!("a://artifact/{}", meta.id),
        "title": meta.title,
        "type": meta.artifact_type,
        "version": meta.current_version,
        "projectId": meta.project_id,
        "provenance": {
            "prompt": meta.prompt,
            "designSystemId": meta.design_system_id,
            "skillId": meta.skill_id,
            "skillName": meta.skill_name,
            "sourceSessionId": meta.source_session_id,
        },
        "sandboxPolicy": meta.sandbox_policy,
        "thumbnail": meta.thumbnail,
        "createdAt": meta.created_at,
        "updatedAt": meta.updated_at,
    })
}

/// Read a version's body, resolving file-backed storage from disk (§4).
pub(crate) fn read_version_body(
    conn: &Connection,
    data_dir: &std::path::Path,
    artifact_id: &str,
    version: i64,
) -> Result<Option<(String, String, String, String)>, rusqlite::Error> {
    // (body, body_sha256, storage, created_at)
    let row = conn
        .query_row(
            "SELECT body, body_sha256, storage, created_at
             FROM content_artifact_versions WHERE artifact_id = ?1 AND version = ?2",
            params![artifact_id, version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    let (body, sha, storage, created_at) = match row {
        Some(r) => r,
        None => return Ok(None),
    };
    if storage == "file" {
        let path = conn.query_row(
            "SELECT file_path FROM content_artifact_versions
             WHERE artifact_id = ?1 AND version = ?2",
            params![artifact_id, version],
            |row| row.get::<_, String>(0),
        )?;
        let resolved = if std::path::Path::new(&path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            data_dir.join(path)
        };
        let body = std::fs::read_to_string(&resolved).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?;
        Ok(Some((body, sha, storage, created_at)))
    } else {
        Ok(Some((body, sha, storage, created_at)))
    }
}

/// Write a version body: inline when small, on disk when large. Returns
/// (storage, file_path) for the version row.
fn store_version_body(
    data_dir: &std::path::Path,
    artifact_id: &str,
    artifact_type: &str,
    version: i64,
    body: &str,
) -> Result<(&'static str, Option<String>), String> {
    if body.len() <= INLINE_BODY_MAX_BYTES {
        return Ok(("inline", None));
    }
    let rel = format!(
        "content-artifacts/{}/{}.{}",
        artifact_id,
        version,
        body_ext(artifact_type)
    );
    let abs = data_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating artifact body dir: {e}"))?;
    }
    std::fs::write(&abs, body.as_bytes()).map_err(|e| format!("writing artifact body: {e}"))?;
    Ok(("file", Some(rel)))
}

/// Insert one immutable version row.
pub(crate) fn insert_version(
    tx: &Transaction,
    data_dir: &std::path::Path,
    artifact_id: &str,
    artifact_type: &str,
    version: i64,
    body: &str,
) -> Result<String, String> {
    let version_id = format!("ver_{}", uuid::Uuid::new_v4());
    let sha = sha256_hex(body);
    let (storage, file_path) = store_version_body(data_dir, artifact_id, artifact_type, version, body)
        .map_err(|e| {
            warn!("content-artifact body storage failed: {}", e);
            e
        })?;
    let now = now_rfc3339();
    tx.execute(
        "INSERT INTO content_artifact_versions
             (id, artifact_id, version, body, body_sha256, storage, file_path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            version_id,
            artifact_id,
            version,
            if storage == "inline" { body } else { "" },
            sha,
            storage,
            file_path,
            &now,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(sha)
}

/// Enforce the per-artifact version cap: prune the oldest versions beyond the
/// cap, deleting their on-disk bodies too (decided 2026-09-12, design §8).
fn prune_old_versions(
    tx: &Transaction,
    data_dir: &std::path::Path,
    artifact_id: &str,
) -> Result<i64, rusqlite::Error> {
    let cap = version_cap();
    let prunable: Vec<(i64, String, Option<String>)> = tx
        .prepare(
            "SELECT version, storage, file_path FROM content_artifact_versions
             WHERE artifact_id = ?1 ORDER BY version DESC LIMIT -1 OFFSET ?2",
        )?
        .query_map(params![artifact_id, cap], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let pruned = prunable.len() as i64;
    for (version, storage, file_path) in prunable {
        tx.execute(
            "DELETE FROM content_artifact_versions WHERE artifact_id = ?1 AND version = ?2",
            params![artifact_id, version],
        )?;
        if storage == "file" {
            if let Some(rel) = file_path {
                let abs = data_dir.join(rel);
                if abs.exists() {
                    std::fs::remove_file(&abs).ok();
                }
            }
        }
    }
    Ok(pruned)
}

fn db_error(context: &str, e: rusqlite::Error) -> axum::response::Response {
    warn!("DB error {}: {}", context, e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": e.to_string()})),
    )
        .into_response()
}

fn internal_error() -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal error"})),
    )
        .into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// POST /content-artifacts — create (also creates version 1)
// ═══════════════════════════════════════════════════════════════════════════════

async fn create_content_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> impl IntoResponse {
    let title = body.title.trim().to_string();
    if title.is_empty() || body.body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "title and body are required"})),
        )
            .into_response();
    }
    let artifact_type = body
        .artifact_type
        .unwrap_or_else(|| "text/html".to_string());
    let idem_key = idempotency_key(&headers, body.idempotency_key);
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let user_id = user.user_id;

    let title_c = title.clone();
    let artifact_type_c = artifact_type.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        sweep_expired_idempotency(&tx)?;

        // Idempotent replay: a repeated create with the same key returns the
        // original artifact (200, not 201) instead of duplicating (§3).
        if let Some(key) = &idem_key {
            if let Some((artifact_id, _)) = find_idempotency(&tx, &user_id, key)? {
                if let Some(meta) = fetch_artifact_meta(&tx, &artifact_id, &user_id)? {
                    tx.commit()?;
                    return Ok((true, artifact_json(&meta)));
                }
            }
        }

        let artifact_id = format!("art_{}", uuid::Uuid::new_v4());
        let now = now_rfc3339();
        tx.execute(
            "INSERT INTO content_artifacts
                 (id, user_id, title, type, project_id, source_session_id, prompt,
                  design_system_id, skill_id, skill_name, sandbox_policy, thumbnail,
                  current_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?13)",
            params![
                artifact_id,
                user_id,
                title_c,
                artifact_type_c,
                body.project_id,
                body.source_session_id,
                body.prompt,
                body.design_system_id,
                body.skill_id,
                body.skill_name,
                body.sandbox_policy.unwrap_or_else(|| "standard".to_string()),
                body.thumbnail,
                &now,
            ],
        )?;
        insert_version(
            &tx,
            &data_dir,
            &artifact_id,
            &artifact_type,
            1,
            &body.body,
        )
        .map_err(|e| rusqlite::Error::InvalidParameterName(e))?;
        if let Some(key) = &idem_key {
            record_idempotency(&tx, &user_id, key, &artifact_id, None)?;
        }
        tx.commit()?;
        let meta = fetch_artifact_meta(&conn, &artifact_id, &user_id)?
            .expect("artifact just inserted");
        Ok((false, artifact_json(&meta)))
    })
    .await;

    match result {
        Ok(Ok((replayed, artifact))) => {
            if replayed {
                (StatusCode::OK, Json(json!({"artifact": artifact}))).into_response()
            } else {
                (StatusCode::CREATED, Json(json!({"artifact": artifact}))).into_response()
            }
        }
        Ok(Err(e)) => db_error("creating content artifact", e),
        Err(_) => internal_error(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /content-artifacts/:id — read (current version body inline)
// ═══════════════════════════════════════════════════════════════════════════════

async fn get_content_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // a://artifact/<id>@<gateway> addressing (§6 relay tier): a local hit on
    // the prefix always wins; on a local miss with a `@peer` suffix the read
    // resolves through the named peer (read-through, nothing persisted).
    let (local_id, peer) = crate::content_artifact_relay::split_relay_address(&id);
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let user_id = user.user_id;
    let local_id_c = local_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let meta = match fetch_artifact_meta(&conn, &local_id_c, &user_id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        let (body, sha, _, _) =
            read_version_body(&conn, &data_dir, &local_id_c, meta.current_version)?
                .expect("current_version row exists");
        let mut artifact = artifact_json(&meta);
        artifact["body"] = json!(body);
        artifact["bodySha256"] = json!(sha);
        // Relay provenance (§6 decision 5): present on artifacts this
        // gateway received over the org relay.
        if let Some(relay) = crate::content_artifact_relay::fetch_relay_provenance(&conn, &local_id_c)? {
            artifact["provenance"]["relay"] = relay;
        }
        Ok(Some(artifact))
    })
    .await;

    match result {
        Ok(Ok(Some(artifact))) => Json(json!({"artifact": artifact})).into_response(),
        Ok(Ok(None)) => match peer {
            Some(peer) => {
                match crate::content_artifact_relay::resolve_from_peer(&peer, &local_id, &headers)
                    .await
                {
                    Ok(body) => Json(body).into_response(),
                    Err(resp) => resp,
                }
            }
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "not found"})),
            )
                .into_response(),
        },
        Ok(Err(e)) => db_error("getting content artifact", e),
        Err(_) => internal_error(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PUT /content-artifacts/:id/versions — append a version
// PATCH /content-artifacts/:id — convenience update (append + bump pointer)
// ═══════════════════════════════════════════════════════════════════════════════

async fn append_content_artifact_version(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AppendBody>,
) -> impl IntoResponse {
    append_version(state, user, headers, id, body).await
}

async fn update_content_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AppendBody>,
) -> impl IntoResponse {
    append_version(state, user, headers, id, body).await
}

async fn append_version(
    state: Arc<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    id: String,
    body: AppendBody,
) -> axum::response::Response {
    if body.body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "body is required"})),
        )
            .into_response();
    }
    let idem_key = idempotency_key(&headers, body.idempotency_key);
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let user_id = user.user_id;
    let body_text = body.body;
    let thumbnail = body.thumbnail;

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        sweep_expired_idempotency(&tx)?;

        // Idempotent replay: a retried append returns the original version
        // number instead of double-appending.
        if let Some(key) = &idem_key {
            if let Some((artifact_id, Some(version))) = find_idempotency(&tx, &user_id, key)? {
                if artifact_id == id {
                    tx.commit()?;
                    return Ok((true, artifact_id, version));
                }
            }
        }

        let meta = match fetch_artifact_meta(&tx, &id, &user_id)? {
            Some(m) => m,
            None => return Err(AppendError::NotFound),
        };
        let next_version = meta.current_version + 1;
        insert_version(&tx, &data_dir, &id, &meta.artifact_type, next_version, &body_text)
            .map_err(|e| AppendError::Internal(rusqlite::Error::InvalidParameterName(e)))?;
        let now = now_rfc3339();
        tx.execute(
            "UPDATE content_artifacts SET current_version = ?2, updated_at = ?3
             WHERE id = ?1",
            params![id, next_version, &now],
        )?;
        if let Some(thumb) = thumbnail {
            tx.execute(
                "UPDATE content_artifacts SET thumbnail = ?2 WHERE id = ?1",
                params![id, thumb],
            )?;
        }
        prune_old_versions(&tx, &data_dir, &id)?;
        if let Some(key) = &idem_key {
            record_idempotency(&tx, &user_id, key, &id, Some(next_version))?;
        }
        tx.commit()?;
        Ok((false, id, next_version))
    })
    .await;

    match result {
        Ok(Ok((replayed, artifact_id, version))) => {
            let payload = json!({"version": version, "artifactId": artifact_id});
            if replayed {
                (StatusCode::OK, Json(payload)).into_response()
            } else {
                (StatusCode::CREATED, Json(payload)).into_response()
            }
        }
        Ok(Err(AppendError::NotFound)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "not found"})),
        )
            .into_response(),
        Ok(Err(AppendError::Internal(e))) => db_error("appending content artifact version", e),
        Err(_) => internal_error(),
    }
}

enum AppendError {
    NotFound,
    Internal(rusqlite::Error),
}

impl From<rusqlite::Error> for AppendError {
    fn from(e: rusqlite::Error) -> Self {
        AppendError::Internal(e)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /content-artifacts/:id/versions — list versions (no bodies)
// GET /content-artifacts/:id/versions/:n — read one version (body inline)
// ═══════════════════════════════════════════════════════════════════════════════

async fn list_content_artifact_versions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        if fetch_artifact_meta(&conn, &id, &user_id)?.is_none() {
            return Ok(None);
        }
        let versions = conn
            .prepare(
                "SELECT version, body_sha256, storage, created_at
                 FROM content_artifact_versions WHERE artifact_id = ?1
                 ORDER BY version ASC",
            )?
            .query_map(params![id], |row| {
                Ok(json!({
                    "version": row.get::<_, i64>(0)?,
                    "bodySha256": row.get::<_, String>(1)?,
                    "storage": row.get::<_, String>(2)?,
                    "createdAt": row.get::<_, String>(3)?,
                }))
            })?
            .collect::<Result<Vec<serde_json::Value>, _>>()?;
        Ok(Some(json!({"versions": versions})))
    })
    .await;

    match result {
        Ok(Ok(Some(payload))) => Json(payload).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "not found"})),
        )
            .into_response(),
        Ok(Err(e)) => db_error("listing content artifact versions", e),
        Err(_) => internal_error(),
    }
}

async fn get_content_artifact_version(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, version)): Path<(String, i64)>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        if fetch_artifact_meta(&conn, &id, &user_id)?.is_none() {
            return Ok(None);
        }
        match read_version_body(&conn, &data_dir, &id, version)? {
            Some((body, sha, _, created_at)) => Ok(Some(json!({
                "artifactId": id,
                "version": version,
                "body": body,
                "bodySha256": sha,
                "createdAt": created_at,
            }))),
            None => Ok(None),
        }
    })
    .await;

    match result {
        Ok(Ok(Some(payload))) => Json(payload).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "not found"})),
        )
            .into_response(),
        Ok(Err(e)) => db_error("getting content artifact version", e),
        Err(_) => internal_error(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /content-artifacts — list (type/project/q + cursor pagination)
// ═══════════════════════════════════════════════════════════════════════════════

async fn list_content_artifacts(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        // Cursor pagination on (created_at, id) — the repo convention (see
        // beta_memory_store_routes / admin_audit_routes). Ties on the
        // second-precision timestamp are broken by id. Each placeholder index
        // is computed immediately before its bind is pushed.
        let mut sql = format!("{META_SELECT} WHERE user_id = ?1 AND deleted_at IS NULL");
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
        let push_bind = |value: Box<dyn rusqlite::ToSql>, binds: &mut Vec<Box<dyn rusqlite::ToSql>>| -> String {
            let idx = binds.len() + 1;
            binds.push(value);
            format!("?{idx}")
        };
        if let Some(t) = &query.artifact_type {
            let ph = push_bind(Box::new(t.clone()), &mut binds);
            sql.push_str(&format!(" AND type = {ph}"));
        }
        if let Some(p) = &query.project {
            let ph = push_bind(Box::new(p.clone()), &mut binds);
            sql.push_str(&format!(" AND project_id = {ph}"));
        }
        if let Some(q) = &query.q {
            let like = format!("%{}%", q.replace('%', "").replace('_', ""));
            let ph_title = push_bind(Box::new(like.clone()), &mut binds);
            let ph_prompt = push_bind(Box::new(like), &mut binds);
            sql.push_str(&format!(" AND (title LIKE {ph_title} OR prompt LIKE {ph_prompt})"));
        }
        if let Some(cursor) = &query.cursor {
            let parts: Vec<&str> = cursor.split('|').collect();
            if parts.len() != 2 {
                return Ok(Err("cursor must be created_at|id".to_string()));
            }
            let ph_created = push_bind(Box::new(parts[0].to_string()), &mut binds);
            let ph_created_eq = push_bind(Box::new(parts[0].to_string()), &mut binds);
            let ph_id = push_bind(Box::new(parts[1].to_string()), &mut binds);
            sql.push_str(&format!(
                " AND (created_at > {ph_created} OR (created_at = {ph_created_eq} AND id > {ph_id}))"
            ));
        }
        let ph_limit = push_bind(Box::new(limit + 1), &mut binds);
        sql.push_str(&format!(" ORDER BY created_at ASC, id ASC LIMIT {ph_limit}"));

        let binds_ref: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt
            .query_map(rusqlite::params_from_iter(binds_ref), read_artifact_meta)?
            .collect::<Result<Vec<_>, _>>()?;

        let has_more = rows.len() as i64 > limit;
        if has_more {
            rows.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            rows.last().map(|r| format!("{}|{}", r.created_at, r.id))
        } else {
            None
        };
        // Relay provenance attach (one query set for the page).
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let relay_map =
            crate::content_artifact_relay::fetch_relay_provenance_map(&conn, &ids)?;
        let artifacts: Vec<serde_json::Value> = rows
            .iter()
            .map(|meta| {
                let mut v = artifact_json(meta);
                if let Some(relay) = relay_map.get(&meta.id) {
                    v["provenance"]["relay"] = relay.clone();
                }
                v
            })
            .collect();
        Ok(Ok(json!({
            "artifacts": artifacts,
            "next_cursor": next_cursor,
            "limit": limit,
        })))
    })
    .await;

    match result {
        Ok(Ok(Ok(payload))) => Json(payload).into_response(),
        Ok(Ok(Err(msg))) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": msg})),
        )
            .into_response(),
        Ok(Err(e)) => db_error("listing content artifacts", e),
        Err(_) => internal_error(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// DELETE /content-artifacts/:id — soft delete
// ═══════════════════════════════════════════════════════════════════════════════

async fn delete_content_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        if fetch_artifact_meta(&conn, &id, &user_id)?.is_none() {
            return Ok(false);
        }
        let now = now_rfc3339();
        conn.execute(
            "UPDATE content_artifacts SET deleted_at = ?2 WHERE id = ?1",
            params![id, &now],
        )?;
        Ok(true)
    })
    .await;

    match result {
        Ok(Ok(true)) => Json(json!({"ok": true})).into_response(),
        Ok(Ok(false)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "not found"})),
        )
            .into_response(),
        Ok(Err(e)) => db_error("deleting content artifact", e),
        Err(_) => internal_error(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::path::Path;
    use tower::ServiceExt;

    pub(crate) fn test_user(user_id: &str, org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: org_id.map(|s| s.to_string()),
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: Some("org:admin".to_string()),
            organization_slug: None,
        }
    }

    pub(crate) async fn test_app_state(temp: &Path) -> Arc<AppState> {
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
        let desktop_host_registry =
            crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            rails,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(tokio::sync::RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            office_cli_docs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_watches: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
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

    pub(crate) async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    pub(crate) fn post_json(uri: &str, payload: serde_json::Value, user: &AuthUser) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .extension(user.clone())
            .header("Content-Type", "application/json")
            .body(Body::from(payload.to_string()))
            .unwrap()
    }

    pub(crate) async fn create_artifact(
        app: &axum::Router,
        user: &AuthUser,
        title: &str,
        body: &str,
        extra: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut payload = json!({
            "title": title,
            "type": "text/html",
            "body": body,
            "projectId": "proj-1",
            "prompt": "build it",
        });
        merge_json(&mut payload, extra);
        let resp = app
            .clone()
            .oneshot(post_json("/content-artifacts", payload, user))
            .await
            .unwrap();
        let status = resp.status();
        let body = body_json(resp.into_body()).await;
        (status, body)
    }

    pub(crate) fn merge_json(target: &mut serde_json::Value, extra: serde_json::Value) {
        if let (Some(t), serde_json::Value::Object(e)) = (target.as_object_mut(), extra) {
            for (k, v) in e {
                t.insert(k, v);
            }
        }
    }

    #[tokio::test]
    async fn create_read_append_versions_delete_lifecycle() {
        let temp = std::env::temp_dir().join(format!("art-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = content_artifact_router().with_state(state);

        let user = test_user("user-1", None);

        // Create → 201 with address + provenance + version 1.
        let (status, created) = create_artifact(
            &app,
            &user,
            "Deck",
            "<html>v1</html>",
            json!({"skillId": "od.deck", "skillName": "Deck Builder"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let artifact = &created["artifact"];
        let id = artifact["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("art_"));
        assert_eq!(artifact["address"], format!("a://artifact/{id}"));
        assert_eq!(artifact["version"], 1);
        assert_eq!(artifact["provenance"]["skillId"], "od.deck");
        assert_eq!(artifact["sandboxPolicy"], "standard");

        // Read → body round-trips.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let read = body_json(resp.into_body()).await;
        assert_eq!(read["artifact"]["body"], "<html>v1</html>");

        // Append v2 via PUT.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/content-artifacts/{id}/versions"))
                    .extension(user.clone())
                    .header("Content-Type", "application/json")
                    .body(Body::from(json!({"body": "<html>v2</html>"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let appended = body_json(resp.into_body()).await;
        assert_eq!(appended["version"], 2);
        assert_eq!(appended["artifactId"], id);

        // PATCH convenience appends v3.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/content-artifacts/{id}"))
                    .extension(user.clone())
                    .header("Content-Type", "application/json")
                    .body(Body::from(json!({"body": "<html>v3</html>"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Read reflects the new current version.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let read = body_json(resp.into_body()).await;
        assert_eq!(read["artifact"]["version"], 3);
        assert_eq!(read["artifact"]["body"], "<html>v3</html>");

        // Version list has 3 entries, no bodies; version 1 body is intact.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}/versions"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let versions = body_json(resp.into_body()).await;
        assert_eq!(versions["versions"].as_array().unwrap().len(), 3);
        assert!(versions["versions"][0].get("body").is_none());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}/versions/1"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v1 = body_json(resp.into_body()).await;
        assert_eq!(v1["body"], "<html>v1</html>");

        // Soft delete → read 404, list empty.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/content-artifacts/{id}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn create_idempotency_replays_original() {
        let temp = std::env::temp_dir().join(format!("art-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = content_artifact_router().with_state(state);
        let user = test_user("user-1", None);

        let (s1, r1) = create_artifact(
            &app,
            &user,
            "Deck",
            "<html>v1</html>",
            json!({"idempotencyKey": "run-3"}),
        )
        .await;
        assert_eq!(s1, StatusCode::CREATED);
        let (s2, r2) = create_artifact(
            &app,
            &user,
            "Deck",
            "<html>v1</html>",
            json!({"idempotencyKey": "run-3"}),
        )
        .await;
        assert_eq!(s2, StatusCode::OK, "replay returns 200, not 201");
        assert_eq!(r1["artifact"]["id"], r2["artifact"]["id"]);

        // Exactly one artifact in the list.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts")
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn append_idempotency_replays_version() {
        let temp = std::env::temp_dir().join(format!("art-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = content_artifact_router().with_state(state);
        let user = test_user("user-1", None);

        let (_, created) = create_artifact(&app, &user, "Deck", "<html>v1</html>", json!({})).await;
        let id = created["artifact"]["id"].as_str().unwrap().to_string();

        let append = |key: &str| {
            let app = app.clone();
            let user = user.clone();
            let id = id.clone();
            let key = key.to_string();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri(format!("/content-artifacts/{id}/versions"))
                        .extension(user)
                        .header("Content-Type", "application/json")
                        .header("Idempotency-Key", &key)
                        .body(Body::from(json!({"body": "<html>v2</html>"}).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        let resp = append("append-1").await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let resp = append("append-1").await;
        assert_eq!(resp.status(), StatusCode::OK, "replayed append returns 200");
        let replayed = body_json(resp.into_body()).await;
        assert_eq!(replayed["version"], 2);

        // Only two versions exist despite the retried append.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}/versions"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let versions = body_json(resp.into_body()).await;
        assert_eq!(versions["versions"].as_array().unwrap().len(), 2);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn version_cap_prunes_oldest() {
        let temp = std::env::temp_dir().join(format!("art-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        // Small cap so the prune is observable. No other test in this module
        // appends more than three versions, so a cross-test env leak cannot
        // change their assertions (pruning only triggers beyond the cap).
        std::env::set_var("ALLTERNIT_CONTENT_ARTIFACT_MAX_VERSIONS", "3");
        let state = test_app_state(&temp).await;
        let app = content_artifact_router().with_state(state);
        let user = test_user("user-1", None);

        let (_, created) = create_artifact(&app, &user, "Deck", "<html>v1</html>", json!({})).await;
        let id = created["artifact"]["id"].as_str().unwrap().to_string();
        for n in 2..=5 {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri(format!("/content-artifacts/{id}/versions"))
                        .extension(user.clone())
                        .header("Content-Type", "application/json")
                        .body(Body::from(json!({"body": format!("<html>v{n}</html>")}).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}/versions"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let versions = body_json(resp.into_body()).await;
        let nums: Vec<i64> = versions["versions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["version"].as_i64().unwrap())
            .collect();
        assert_eq!(nums, vec![3, 4, 5], "oldest versions pruned beyond the cap");

        // Pruned version reads 404; current version intact.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}/versions/1"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let read = body_json(resp.into_body()).await;
        assert_eq!(read["artifact"]["version"], 5);
        assert_eq!(read["artifact"]["body"], "<html>v5</html>");

        std::env::remove_var("ALLTERNIT_CONTENT_ARTIFACT_MAX_VERSIONS");
        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn list_filters_paginates_and_scopes_by_user() {
        let temp = std::env::temp_dir().join(format!("art-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = content_artifact_router().with_state(state);
        let user = test_user("user-1", None);
        let other = test_user("user-2", None);

        for (title, ty, project, prompt) in [
            ("Alpha deck", "application/vnd.allternit.deck", "proj-a", "launch narrative"),
            ("Beta deck", "application/vnd.allternit.deck", "proj-b", "roadmap"),
            ("Gamma site", "text/html", "proj-a", "landing page"),
        ] {
            let (_, r) = create_artifact(
                &app,
                &user,
                title,
                "<html>x</html>",
                json!({"type": ty, "projectId": project, "prompt": prompt}),
            )
            .await;
            assert!(r["artifact"]["id"].is_string());
        }

        // type filter
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts?type=application/vnd.allternit.deck")
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 2);

        // project filter
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts?project=proj-a")
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 2);

        // free-text q over title/prompt
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts?q=narrative")
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1);
        assert_eq!(listed["artifacts"][0]["title"], "Alpha deck");

        // cursor pagination: limit 2 → 2 rows + next_cursor; follow it → 1 row.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts?limit=2")
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let page1 = body_json(resp.into_body()).await;
        assert_eq!(page1["artifacts"].as_array().unwrap().len(), 2);
        let cursor = page1["next_cursor"].as_str().unwrap().to_string();
        // RFC3339 cursors contain '+' (the UTC offset), which must be
        // percent-encoded in a query string — otherwise '+' decodes to a
        // space and the cursor comparison silently matches everything.
        let encoded_cursor = cursor.replace('+', "%2B");
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts?limit=2&cursor={encoded_cursor}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let page2 = body_json(resp.into_body()).await;
        assert_eq!(page2["artifacts"].as_array().unwrap().len(), 1);
        assert!(page2["next_cursor"].is_null());

        // Bad cursor shape → 400.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts?cursor=bogus")
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // User scoping: another user sees nothing and cannot read.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts")
                    .extension(other.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 0);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts?project=proj-a&limit=1")
                    .extension(other)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 0);

        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn large_body_uses_file_storage_and_roundtrips() {
        let temp = std::env::temp_dir().join(format!("art-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = content_artifact_router().with_state(state);
        let user = test_user("user-1", None);

        let big = format!("<html>{}</html>", "x".repeat(300 * 1024));
        let (status, created) = create_artifact(&app, &user, "Big", &big, json!({})).await;
        assert_eq!(status, StatusCode::CREATED);
        let id = created["artifact"]["id"].as_str().unwrap().to_string();

        // File landed under <data_dir>/content-artifacts/.
        let files: Vec<_> = std::fs::read_dir(temp.join("content-artifacts").join(&id))
            .unwrap()
            .collect();
        assert_eq!(files.len(), 1);

        // Body round-trips through the read endpoint.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{id}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let read = body_json(resp.into_body()).await;
        assert_eq!(read["artifact"]["body"].as_str().unwrap(), big);

        std::fs::remove_dir_all(&temp).ok();
    }
}
