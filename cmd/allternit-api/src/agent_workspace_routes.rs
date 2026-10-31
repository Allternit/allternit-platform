//! Agent Workspace file read/write API routes.
//!
//! Complements the write-only `POST /agents/:id/workspace/initialize` in
//! `agent_routes.rs` with per-file list/read/write endpoints. All paths are
//! relative to the agent's workspace directory and are guarded against
//! traversal (absolute paths, `..`, backslashes, NUL bytes are rejected).

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Component;
use std::sync::Arc;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::agent_workspace_paths::workspace_dir_for;
use crate::auth::AuthUser;
use crate::AppState;

pub fn agent_workspace_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agents/:id/workspace/files", get(list_workspace_files))
        .route(
            "/agents/:id/workspace/file",
            get(read_workspace_file).put(write_workspace_file),
        )
}


// ─── One Soul: the workspace root is canonical ───────────────────────────────

/// Persona files whose canonical home is the workspace root. Older web
/// builds wrote them under `.allternit/bot/`.
pub(crate) const ROOT_FILES: [&str; 5] = ["SOUL.md", "USER.md", "AGENTS.md", "TOOLS.md", "HEARTBEAT.md"];
const LEGACY_PREFIX: &str = ".allternit/bot/";

/// `.allternit/bot/<root file>` -> `<root file>`; any other path is unchanged.
pub(crate) fn canonical_path(path: &str) -> String {
    let trimmed = path.strip_prefix("./").unwrap_or(path);
    match trimmed.strip_prefix(LEGACY_PREFIX) {
        Some(name) if ROOT_FILES.contains(&name) => name.to_string(),
        _ => path.to_string(),
    }
}

fn mtime_of(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

pub(crate) fn sha256_hex(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

/// Move legacy `.allternit/bot/<file>` copies to the root. Legacy only: moved.
/// Both: the newer mtime wins at the root and the loser is kept as
/// `.allternit/bot/<file>.pre-unify`. Idempotent and best-effort: errors are
/// logged, never returned.
pub(crate) fn unify_legacy_files(dir: &std::path::Path) {
    let legacy_dir = dir.join(".allternit").join("bot");
    if !legacy_dir.is_dir() {
        return;
    }
    for name in ROOT_FILES {
        if let Err(e) = unify_one(dir, &legacy_dir, name) {
            warn!("workspace unify {} failed: {}", name, e);
        }
    }
}

fn unify_one(dir: &std::path::Path, legacy_dir: &std::path::Path, name: &str) -> std::io::Result<()> {
    let legacy = legacy_dir.join(name);
    if !legacy.is_file() {
        return Ok(());
    }
    let root = dir.join(name);
    let keep = legacy_dir.join(format!("{name}.pre-unify"));
    if !root.is_file() {
        return move_file(&legacy, &root);
    }
    if std::fs::read(&legacy)? == std::fs::read(&root)? {
        return std::fs::remove_file(&legacy);
    }
    let legacy_newer = match (mtime_of(&legacy), mtime_of(&root)) {
        (Some(l), Some(r)) => l > r,
        _ => false,
    };
    if legacy_newer {
        move_file(&root, &keep)?;
        move_file(&legacy, &root)
    } else {
        move_file(&legacy, &keep)
    }
}

fn move_file(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// Serializes the hash check + write of PUT so two writers cannot both pass.
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Reject anything that could escape the workspace dir: absolute paths,
/// `..` components, backslashes, and NUL bytes.
fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.contains(['\\', '\0']) {
        return false;
    }
    let rel = std::path::Path::new(path);
    !rel.is_absolute()
        && rel
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn invalid_path() -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "invalid_path"})),
    )
        .into_response()
}

/// Verify the agent exists and belongs to the user. Returns None when
/// authorized, otherwise the error response to return.
pub(crate) async fn authorize_agent(
    db: &crate::db::DbHandle,
    user_id: String,
    agent_id: String,
) -> Option<axum::response::Response> {
    let db = db.clone();
    let authorized = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM agents WHERE id = ?1 AND user_id = ?2",
            params![agent_id, user_id],
            |row| row.get(0),
        )?;
        Ok::<_, rusqlite::Error>(count > 0)
    })
    .await;

    match authorized {
        Ok(Ok(true)) => None,
        Ok(Ok(false)) | Ok(Err(_)) => Some(
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "Agent not found"})),
            )
                .into_response(),
        ),
        Err(_) => Some(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response(),
        ),
    }
}

// ─── GET /agents/:id/workspace/files ──────────────────────────────────────────

#[derive(Serialize)]
struct WorkspaceFileEntry {
    path: String,
    size_bytes: u64,
    modified_at: Option<String>,
}

async fn list_workspace_files(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(resp) = authorize_agent(&state.db, user.user_id, id.clone()).await {
        return resp;
    }

    let workspace_dir = workspace_dir_for(&id);
    let result = tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        unify_legacy_files(&workspace_dir);
        if workspace_dir.is_dir() {
            collect_files(&workspace_dir, &workspace_dir, &mut files)?;
        }
        files.sort_by(|a: &WorkspaceFileEntry, b| a.path.cmp(&b.path));
        Ok::<_, std::io::Error>(files)
    })
    .await;

    match result {
        Ok(Ok(files)) => Json(json!({ "files": files })).into_response(),
        Ok(Err(e)) => {
            warn!("Workspace file listing failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("Workspace task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

fn collect_files(
    base: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<WorkspaceFileEntry>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else if path.is_file() {
            let meta = entry.metadata()?;
            let rel = path
                .strip_prefix(base)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let modified_at = meta
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
            out.push(WorkspaceFileEntry {
                path: rel,
                size_bytes: meta.len(),
                modified_at,
            });
        }
    }
    Ok(())
}

// ─── GET /agents/:id/workspace/file?path=… ────────────────────────────────────

#[derive(Deserialize)]
struct WorkspaceFileQuery {
    path: String,
}

async fn read_workspace_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(query): Query<WorkspaceFileQuery>,
) -> impl IntoResponse {
    if !is_safe_relative_path(&query.path) {
        return invalid_path();
    }
    if let Some(resp) = authorize_agent(&state.db, user.user_id, id.clone()).await {
        return resp;
    }

    let dir = workspace_dir_for(&id);
    let file_path = dir.join(canonical_path(&query.path));
    let rel_path = query.path.clone();
    let result = tokio::task::spawn_blocking(move || {
        unify_legacy_files(&dir);
        std::fs::read_to_string(&file_path)
    })
    .await;

    match result {
        Ok(Ok(content)) => Json(json!({
            "path": rel_path,
            "hash": sha256_hex(content.as_bytes()),
            "content": content,
        }))
        .into_response(),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "File not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("Workspace file read failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("Workspace task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── PUT /agents/:id/workspace/file ───────────────────────────────────────────

#[derive(Deserialize)]
struct WriteWorkspaceFileBody {
    path: String,
    content: String,
    /// sha256 of the content the client last read. Omitted = overwrite.
    #[serde(default)]
    expected_hash: Option<String>,
}

async fn write_workspace_file(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<WriteWorkspaceFileBody>,
) -> impl IntoResponse {
    if !is_safe_relative_path(&body.path) {
        return invalid_path();
    }
    if let Some(resp) = authorize_agent(&state.db, user.user_id, id.clone()).await {
        return resp;
    }

    let dir = workspace_dir_for(&id);
    let file_path = dir.join(canonical_path(&body.path));
    let rel_path = body.path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unify_legacy_files(&dir);
        if let Some(expected) = body.expected_hash.as_deref() {
            let current = std::fs::read(&file_path).unwrap_or_default();
            let current_hash = sha256_hex(&current);
            if !current_hash.eq_ignore_ascii_case(expected) {
                return Ok(Err((current_hash, String::from_utf8_lossy(&current).into_owned())));
            }
        }
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = file_path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&tmp, body.content.as_bytes())?;
        std::fs::rename(&tmp, &file_path)?;
        Ok::<_, std::io::Error>(Ok(sha256_hex(body.content.as_bytes())))
    })
    .await;

    let result = match result {
        Ok(Ok(Err((hash, content)))) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "changed_elsewhere", "hash": hash, "content": content})),
            )
                .into_response()
        }
        Ok(Ok(Ok(hash))) => Ok(Ok(hash)),
        Ok(Err(e)) => Ok(Err(e)),
        Err(e) => Err(e),
    };

    match result {
        Ok(Ok(hash)) => Json(json!({
            "success": true,
            "path": rel_path,
            "hash": hash,
        }))
        .into_response(),
        Ok(Err(e)) => {
            warn!("Workspace file write failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("Workspace task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}
