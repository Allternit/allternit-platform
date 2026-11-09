//! Teleport: move a live coding session between computers (Mac <-> cloud computer).
//!
//! Contract (all under `/api/v1`, owner-scoped):
//! - `POST /teleports` stages a transfer from a manifest (one native session file with
//!   its sha256, model, effort, handoff text, queued prompts, `request_id`). The same
//!   `request_id` with the same manifest returns the same transfer; a different manifest
//!   under that `request_id` is a conflict.
//! - `PUT /teleports/:id/files/:index` uploads one file; size and sha256 are checked.
//! - `POST /teleports/:id/activate` installs the native file where the harness reads it
//!   and claims the native session id in one transaction. A native id that another
//!   transfer already owns is refused.
//! - `GET /teleports/:id` reads the transfer; `POST /teleports/:id/cancel` drops a staged one.
//!
//! Tables are created on first use (no numbered migration), like `phone_seen_routes`.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post, put},
    Extension, Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::AppState;

const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 4;

type ApiError = (StatusCode, Json<Value>);

fn err(status: StatusCode, code: &str, message: impl std::fmt::Display) -> ApiError {
    (status, Json(json!({ "error": code, "message": message.to_string() })))
}
fn internal(e: impl std::fmt::Display) -> ApiError {
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal", e)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// Bare file name of the native session file (no directories).
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub request_id: String,
    /// `claude` or `codex`.
    pub harness: String,
    pub native_id: String,
    pub model: String,
    pub reasoning: Option<String>,
    /// Folder name for the workspace on this computer.
    pub workspace_name: String,
    /// `local_to_cloud` or `cloud_to_local`; informational, kept in the record.
    #[serde(default)]
    pub direction: Option<String>,
    pub files: Vec<FileEntry>,
    /// Text the resumed session reads first, so it knows it moved.
    pub handoff: String,
    #[serde(default)]
    pub queued: Vec<String>,
}

fn valid_token(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_file_name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 200
        && !v.starts_with('.')
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

pub fn validate(m: &Manifest) -> Result<(), String> {
    if !valid_token(&m.request_id) {
        return Err("request_id must be 1-64 letters, digits, - or _".into());
    }
    if !matches!(m.harness.as_str(), "claude" | "codex") {
        return Err("only claude and codex sessions can teleport".into());
    }
    if !valid_token(&m.native_id) {
        return Err("native_id is not a valid session id".into());
    }
    if m.model.trim().is_empty() || m.model.len() > 200 {
        return Err("model is required".into());
    }
    if !valid_token(&m.workspace_name) {
        return Err("workspace_name must be 1-64 letters, digits, - or _".into());
    }
    if m.files.len() != 1 || m.files.len() > MAX_FILES {
        return Err("a teleport carries exactly one native session file".into());
    }
    for f in &m.files {
        if !valid_file_name(&f.name) || !f.name.ends_with(".jsonl") {
            return Err("file name must be a plain .jsonl name".into());
        }
        if f.size == 0 || f.size > MAX_FILE_BYTES {
            return Err("file size is out of range".into());
        }
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("sha256 must be 64 hex characters".into());
        }
    }
    if m.handoff.len() > 64 * 1024 || m.queued.len() > 20 || m.queued.iter().any(|q| q.len() > 64 * 1024) {
        return Err("handoff or queued prompts are too large".into());
    }
    Ok(())
}

fn manifest_hash(m: &Manifest) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(m).unwrap_or_default()))
}

fn ensure_tables(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS teleports (
             id TEXT PRIMARY KEY,
             user_id TEXT NOT NULL,
             request_id TEXT NOT NULL,
             manifest_hash TEXT NOT NULL,
             manifest_json TEXT NOT NULL,
             state TEXT NOT NULL DEFAULT 'staged',
             uploaded TEXT NOT NULL DEFAULT '[]',
             cwd TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             activated_at TEXT,
             UNIQUE (user_id, request_id)
         );
         CREATE TABLE IF NOT EXISTS teleport_native_owners (
             native_key TEXT PRIMARY KEY,
             teleport_id TEXT NOT NULL
         );",
    )
}

struct Row {
    id: String,
    hash: String,
    manifest: Manifest,
    state: String,
    uploaded: Vec<usize>,
    cwd: Option<String>,
}

fn load(conn: &rusqlite::Connection, user: &str, where_col: &str, key: &str) -> Result<Option<Row>, ApiError> {
    let sql = format!(
        "SELECT id, manifest_hash, manifest_json, state, uploaded, cwd FROM teleports WHERE user_id = ?1 AND {where_col} = ?2"
    );
    let raw = conn
        .query_row(&sql, params![user, key], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?))
        })
        .optional()
        .map_err(internal)?;
    let Some((id, hash, mj, state, up, cwd)) = raw else { return Ok(None) };
    Ok(Some(Row {
        id,
        hash,
        manifest: serde_json::from_str(&mj).map_err(internal)?,
        state,
        uploaded: serde_json::from_str(&up).unwrap_or_default(),
        cwd,
    }))
}

fn view(row: &Row) -> Value {
    let pending: Vec<usize> = (0..row.manifest.files.len()).filter(|i| !row.uploaded.contains(i)).collect();
    json!({
        "id": row.id,
        "state": row.state,
        "request_id": row.manifest.request_id,
        "harness": row.manifest.harness,
        "native_id": row.manifest.native_id,
        "workspace": row.cwd,
        "files_pending": pending,
        "ready": row.state == "staged" && pending.is_empty(),
        "resume": resume_hint(&row.manifest),
    })
}

fn resume_hint(m: &Manifest) -> String {
    match m.harness.as_str() {
        "claude" => format!("claude --resume {}", m.native_id),
        _ => format!("codex resume {}", m.native_id),
    }
}

fn teleport_dir(state: &AppState, id: &str) -> PathBuf {
    state.data_dir.join("teleports").join(id)
}

/// Where the session's project lives on this computer.
fn workspace_dir(state: &AppState, m: &Manifest) -> PathBuf {
    state.data_dir.join("teleport-workspaces").join(&m.workspace_name)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Same slug rule Claude Code uses for `~/.claude/projects/<slug>`.
pub fn claude_cwd_slug(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Where this harness reads the native file for a session working in `cwd`.
pub fn native_target(m: &Manifest, cwd: &str, name: &str) -> PathBuf {
    match m.harness.as_str() {
        "claude" => {
            let base = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".claude"));
            base.join("projects").join(claude_cwd_slug(cwd)).join(format!("{}.jsonl", m.native_id))
        }
        _ => {
            let base = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".codex"));
            let now = chrono::Utc::now();
            base.join("sessions").join(now.format("%Y").to_string()).join(now.format("%m").to_string()).join(now.format("%d").to_string()).join(name)
        }
    }
}

pub fn teleport_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/teleports", post(prepare))
        .route("/teleports/:id", get(status))
        .route("/teleports/:id/cancel", post(cancel))
        .route("/teleports/:id/activate", post(activate))
        .route(
            "/teleports/:id/files/:index",
            put(upload).layer(DefaultBodyLimit::max(MAX_FILE_BYTES as usize + 1024)),
        )
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)?
}

async fn prepare(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(manifest): Json<Manifest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    validate(&manifest).map_err(|m| err(StatusCode::BAD_REQUEST, "teleport_rejected", m))?;
    let cwd = workspace_dir(&state, &manifest);
    let db = state.db.clone();
    blocking(move || {
        let conn = db.connect().map_err(internal)?;
        ensure_tables(&conn).map_err(internal)?;
        let hash = manifest_hash(&manifest);
        if let Some(existing) = load(&conn, &user.user_id, "request_id", &manifest.request_id)? {
            if existing.hash != hash {
                return Err(err(StatusCode::CONFLICT, "request_conflict", "that request_id was used with a different manifest"));
            }
            return Ok((StatusCode::OK, Json(view(&existing))));
        }
        let id = format!("tp_{}", uuid::Uuid::new_v4().simple());
        std::fs::create_dir_all(&cwd).map_err(internal)?;
        conn.execute(
            "INSERT INTO teleports (id, user_id, request_id, manifest_hash, manifest_json, cwd) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, user.user_id, manifest.request_id, hash, serde_json::to_string(&manifest).map_err(internal)?, cwd.to_string_lossy()],
        )
        .map_err(internal)?;
        let row = load(&conn, &user.user_id, "id", &id)?.ok_or_else(|| internal("transfer vanished"))?;
        Ok((StatusCode::CREATED, Json(view(&row))))
    })
    .await
}

async fn status(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    blocking(move || {
        let conn = db.connect().map_err(internal)?;
        ensure_tables(&conn).map_err(internal)?;
        let row = load(&conn, &user.user_id, "id", &id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such teleport"))?;
        Ok(Json(view(&row)))
    })
    .await
}

async fn upload(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, index)): Path<(String, usize)>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let dir = teleport_dir(&state, &id);
    let db = state.db.clone();
    blocking(move || {
        let conn = db.connect().map_err(internal)?;
        ensure_tables(&conn).map_err(internal)?;
        let mut row = load(&conn, &user.user_id, "id", &id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such teleport"))?;
        if row.state != "staged" {
            return Err(err(StatusCode::CONFLICT, "teleport_running", format!("transfer is {}", row.state)));
        }
        let entry = row.manifest.files.get(index).ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such file index"))?.clone();
        if body.len() as u64 != entry.size {
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "teleport_rejected", "file size does not match the manifest"));
        }
        if hex::encode(Sha256::digest(&body)) != entry.sha256.to_lowercase() {
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "teleport_rejected", "file sha256 does not match the manifest"));
        }
        std::fs::create_dir_all(&dir).map_err(internal)?;
        let part = dir.join(format!("{index}.part"));
        let tmp = dir.join(format!("{index}.tmp"));
        std::fs::write(&tmp, &body).map_err(internal)?;
        std::fs::rename(&tmp, &part).map_err(internal)?;
        if !row.uploaded.contains(&index) {
            row.uploaded.push(index);
        }
        conn.execute("UPDATE teleports SET uploaded = ?1 WHERE id = ?2", params![serde_json::to_string(&row.uploaded).map_err(internal)?, id])
            .map_err(internal)?;
        Ok(Json(view(&row)))
    })
    .await
}

async fn activate(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let dir = teleport_dir(&state, &id);
    let db = state.db.clone();
    blocking(move || {
        let mut conn = db.connect().map_err(internal)?;
        ensure_tables(&conn).map_err(internal)?;
        let mut row = load(&conn, &user.user_id, "id", &id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such teleport"))?;
        match row.state.as_str() {
            "active" => return Ok(Json(view(&row))), // retry after success
            "cancelled" => return Err(err(StatusCode::CONFLICT, "teleport_cancelled", "this transfer was cancelled")),
            _ => {}
        }
        if (0..row.manifest.files.len()).any(|i| !row.uploaded.contains(&i)) {
            return Err(err(StatusCode::CONFLICT, "teleport_rejected", "files are still missing"));
        }
        let cwd = row.cwd.clone().ok_or_else(|| internal("transfer has no workspace"))?;
        let entry = &row.manifest.files[0];
        let target = native_target(&row.manifest, &cwd, &entry.name);
        let native_key = format!("{}:{}", row.manifest.harness, row.manifest.native_id);

        // One atomic write: the claim on the native id and the state flip commit together,
        // and the file lands inside the transaction so a failed install rolls both back.
        let tx = conn.transaction().map_err(internal)?;
        let claimed = tx
            .execute("INSERT OR IGNORE INTO teleport_native_owners (native_key, teleport_id) VALUES (?1, ?2)", params![native_key, id])
            .map_err(internal)?;
        if claimed == 0 {
            return Err(err(StatusCode::CONFLICT, "teleport_rejected", "that session already lives on this computer"));
        }
        if target.exists() {
            return Err(err(StatusCode::CONFLICT, "teleport_rejected", "a native session file already exists at the destination"));
        }
        install(&dir.join("0.part"), &target)?;
        tx.execute("UPDATE teleports SET state = 'active', activated_at = datetime('now') WHERE id = ?1", params![id]).map_err(internal)?;
        if let Err(e) = tx.commit() {
            let _ = std::fs::remove_file(&target);
            return Err(internal(e));
        }
        let _ = std::fs::remove_dir_all(&dir);
        row.state = "active".into();
        Ok(Json(view(&row)))
    })
    .await
}

fn install(part: &FsPath, target: &FsPath) -> Result<(), ApiError> {
    let parent = target.parent().ok_or_else(|| internal("bad destination"))?;
    std::fs::create_dir_all(parent).map_err(internal)?;
    let tmp = parent.join(format!(".teleport-{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::copy(part, &tmp).map_err(internal)?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        internal(e)
    })
}

async fn cancel(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let dir = teleport_dir(&state, &id);
    let db = state.db.clone();
    blocking(move || {
        let conn = db.connect().map_err(internal)?;
        ensure_tables(&conn).map_err(internal)?;
        let mut row = load(&conn, &user.user_id, "id", &id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such teleport"))?;
        if row.state == "active" {
            return Err(err(StatusCode::CONFLICT, "teleport_running", "an activated transfer cannot be cancelled"));
        }
        conn.execute("UPDATE teleports SET state = 'cancelled' WHERE id = ?1", params![id]).map_err(internal)?;
        let _ = std::fs::remove_dir_all(&dir);
        row.state = "cancelled".into();
        Ok(Json(view(&row)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            request_id: "req_1".into(),
            harness: "claude".into(),
            native_id: "0a1b2c3d-0000-4000-8000-000000000001".into(),
            model: "claude-sonnet-4".into(),
            reasoning: None,
            workspace_name: "app".into(),
            direction: None,
            files: vec![FileEntry { name: "x.jsonl".into(), size: 10, sha256: "a".repeat(64) }],
            handoff: "moved".into(),
            queued: vec![],
        }
    }

    #[test]
    fn slug_matches_claude_projects_dir() {
        assert_eq!(claude_cwd_slug("/Users/joe/my app"), "-Users-joe-my-app");
    }

    #[test]
    fn rejects_bad_manifests() {
        assert!(validate(&sample()).is_ok());
        let mut m = sample();
        m.files[0].name = "../x.jsonl".into();
        assert!(validate(&m).is_err());
        let mut m = sample();
        m.harness = "grok".into();
        assert!(validate(&m).is_err());
        let mut m = sample();
        m.files.push(m.files[0].clone());
        assert!(validate(&m).is_err());
    }

    #[test]
    fn hash_changes_with_manifest() {
        let a = sample();
        let mut b = sample();
        b.model = "other".into();
        assert_ne!(manifest_hash(&a), manifest_hash(&b));
    }
}
