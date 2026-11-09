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
    http::{Method, StatusCode},
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
    /// `native` (the session file), `patch` (git diff --binary from the base commit to the
    /// Mac's working tree) or `untracked` (a tar of untracked, non-ignored files).
    #[serde(default = "native_kind")]
    pub kind: String,
}

fn native_kind() -> String {
    "native".into()
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
    /// The gizzi session the vendor session id is recorded under after the move.
    #[serde(default)]
    pub session_id: Option<String>,
    /// `local_to_cloud` or `cloud_to_local`; informational, kept in the record.
    #[serde(default)]
    pub direction: Option<String>,
    pub files: Vec<FileEntry>,
    /// Where the cloud computer gets the code: a clone URL and the commit the Mac was on.
    #[serde(default)]
    pub git_remote: Option<String>,
    #[serde(default)]
    pub git_base: Option<String>,
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
    let count = |k: &str| m.files.iter().filter(|f| f.kind == k).count();
    if count("native") != 1 || count("patch") > 1 || count("untracked") > 1 || m.files.len() != count("native") + count("patch") + count("untracked") || m.files.len() > MAX_FILES {
        return Err("a teleport carries one native session file, plus at most one patch and one untracked tar".into());
    }
    for f in &m.files {
        let ext = match f.kind.as_str() {
            "native" => ".jsonl",
            "patch" => ".patch",
            _ => ".tar",
        };
        if !valid_file_name(&f.name) || !f.name.ends_with(ext) {
            return Err(format!("a {} file must be a plain {ext} name", f.kind));
        }
        if f.size == 0 || f.size > MAX_FILE_BYTES {
            return Err("file size is out of range".into());
        }
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("sha256 must be 64 hex characters".into());
        }
    }
    if let Some(r) = &m.git_remote {
        if !(r.starts_with("https://") || r.starts_with("git@")) || r.len() > 500 || r.contains(char::is_whitespace) || r.starts_with('-') {
            return Err("git_remote must be an https or ssh clone URL".into());
        }
    }
    if let Some(b) = &m.git_base {
        if b.len() < 7 || b.len() > 64 || !b.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("git_base must be a commit hash".into());
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
             last_error TEXT,
             project_report TEXT,
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
    error: Option<String>,
    report: Option<Value>,
}

fn load(conn: &rusqlite::Connection, user: &str, where_col: &str, key: &str) -> Result<Option<Row>, ApiError> {
    let sql = format!(
        "SELECT id, manifest_hash, manifest_json, state, uploaded, cwd, last_error, project_report FROM teleports WHERE user_id = ?1 AND {where_col} = ?2"
    );
    let raw = conn
        .query_row(&sql, params![user, key], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?, r.get::<_, Option<String>>(7)?))
        })
        .optional()
        .map_err(internal)?;
    let Some((id, hash, mj, state, up, cwd, error, report)) = raw else { return Ok(None) };
    Ok(Some(Row {
        id,
        hash,
        manifest: serde_json::from_str(&mj).map_err(internal)?,
        state,
        uploaded: serde_json::from_str(&up).unwrap_or_default(),
        cwd,
        error,
        report: report.and_then(|r| serde_json::from_str(&r).ok()),
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
        "error": row.error,
        "project": row.report,
        "resume": resume_hint(&row.manifest),
    })
}

fn resume_hint(m: &Manifest) -> String {
    match m.harness.as_str() {
        "claude" => format!("claude --resume {}", m.native_id),
        _ => format!("codex resume {}", m.native_id),
    }
}

/// Put the code on this computer: clone the remote at the Mac's commit. Skipped when the
/// folder already holds a checkout (a retry, or a session that came back).
fn materialize(cwd: &FsPath, m: &Manifest) -> Result<(), ApiError> {
    let Some(remote) = &m.git_remote else { return Ok(()) };
    if cwd.join(".git").exists() {
        return Ok(());
    }
    let run = |args: &[&str], dir: &FsPath| -> Result<(), ApiError> {
        let out = std::process::Command::new("git").args(args).current_dir(dir).output().map_err(internal)?;
        if out.status.success() { Ok(()) } else { Err(err(StatusCode::UNPROCESSABLE_ENTITY, "workspace_unavailable", String::from_utf8_lossy(&out.stderr))) }
    };
    run(&["clone", "--no-tags", "--", remote, "."], cwd)?;
    if let Some(base) = &m.git_base {
        run(&["checkout", "--quiet", base], cwd)?;
    }
    Ok(())
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
        .route("/teleports/placement", put(set_placement))
        .route("/teleports/placement/:session_id", get(get_placement))
        .route("/teleports/placements", get(list_placements))
        .route("/teleports/export/:harness/:id", get(export_native))
        .route("/teleports/export/:harness/:id/diff", get(export_diff))
        .route("/teleports/to/:target_id/*rest", axum::routing::any(relay).layer(DefaultBodyLimit::max(MAX_FILE_BYTES as usize + 1024)))
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
        materialize(&cwd, &manifest)?;
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
    let (row, resume) = blocking(move || {
        let mut conn = db.connect().map_err(internal)?;
        ensure_tables(&conn).map_err(internal)?;
        let mut row = load(&conn, &user.user_id, "id", &id)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no such teleport"))?;
        match row.state.as_str() {
            // Retries after success, or while the resume is still running, change nothing.
            "active" | "resuming" => return Ok((row, false)),
            "cancelled" => return Err(err(StatusCode::CONFLICT, "teleport_cancelled", "this transfer was cancelled")),
            // Installed already; only the resume failed. Try it again.
            "resume_failed" => {
                conn.execute("UPDATE teleports SET state = 'resuming', last_error = NULL WHERE id = ?1", params![id]).map_err(internal)?;
                row.state = "resuming".into();
                return Ok((row, true));
            }
            _ => {}
        }
        if (0..row.manifest.files.len()).any(|i| !row.uploaded.contains(&i)) {
            return Err(err(StatusCode::CONFLICT, "teleport_rejected", "files are still missing"));
        }
        let cwd = row.cwd.clone().ok_or_else(|| internal("transfer has no workspace"))?;
        let native_index = row.manifest.files.iter().position(|f| f.kind == "native").ok_or_else(|| internal("no native file"))?;
        let target = native_target(&row.manifest, &cwd, &row.manifest.files[native_index].name);
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
        install(&dir.join(format!("{native_index}.part")), &target)?;
        // The transfer is only `active` once the resumed session has produced new output.
        tx.execute("UPDATE teleports SET state = 'resuming', activated_at = datetime('now') WHERE id = ?1", params![id]).map_err(internal)?;
        if let Err(e) = tx.commit() {
            let _ = std::fs::remove_file(&target);
            return Err(internal(e));
        }
        // The code the Mac had but git did not: applied on top of the clone, never over cloud edits.
        let mut report = apply_project(&dir, &row.manifest, FsPath::new(&cwd));
        report["snapshot"] = json!(snapshot(FsPath::new(&cwd)));
        let _ = conn.execute("UPDATE teleports SET project_report = ?1 WHERE id = ?2", params![report.to_string(), id]);
        row.report = Some(report);
        let _ = std::fs::remove_dir_all(&dir);
        row.state = "resuming".into();
        Ok((row, true))
    })
    .await?;
    if resume {
        tokio::spawn(run_resume(state.clone(), row.id.clone(), row.manifest.clone(), row.cwd.clone().unwrap_or_default()));
    }
    Ok(Json(view(&row)))
}

/// Ask this computer's gizzi to resume the native session (`--resume`, same model and
/// effort), send the handoff, then replay the queued prompts. It answers after the first
/// new output; only then does the transfer become `active`.
async fn run_resume(state: Arc<AppState>, id: String, m: Manifest, cwd: String) {
    let body = json!({
        "sessionId": m.session_id.clone().unwrap_or_else(|| m.native_id.clone()),
        "harness": m.harness,
        "nativeId": m.native_id,
        "cwd": cwd,
        "model": m.model,
        "effort": m.reasoning,
        "handoff": m.handoff,
        "queued": m.queued,
    });
    let client = crate::agent_session_routes::gizzi_client(&axum::http::HeaderMap::new());
    let url = format!("{}/v1/native-session/teleport-resume?directory={}", crate::agent_session_routes::gizzi_base(), urlencode(&cwd));
    let result = match client.post(url).json(&body).timeout(std::time::Duration::from_secs(180)).send().await {
        Ok(res) if res.status().is_success() => Ok(()),
        Ok(res) => Err(res.text().await.unwrap_or_default().chars().take(400).collect::<String>()),
        Err(e) => Err(e.to_string()),
    };
    let db = state.db.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(conn) = db.connect() {
            let _ = match result {
                Ok(()) => conn.execute("UPDATE teleports SET state = 'active', last_error = NULL WHERE id = ?1", params![id]),
                Err(msg) => conn.execute("UPDATE teleports SET state = 'resume_failed', last_error = ?2 WHERE id = ?1", params![id, msg]),
            };
        }
    })
    .await;
}

fn urlencode(v: &str) -> String {
    v.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Apply the `patch` and `untracked` files over the fresh clone. A hunk that does not fit and
/// an untracked file that already exists are conflicts: the cloud original stays, and the
/// count is reported.
fn apply_project(dir: &FsPath, m: &Manifest, cwd: &FsPath) -> Value {
    let (mut patch_conflicts, mut untracked_conflicts, mut applied) = (0u32, 0u32, 0u32);
    let mut note: Option<String> = None;
    for (i, f) in m.files.iter().enumerate() {
        let part = dir.join(format!("{i}.part"));
        match f.kind.as_str() {
            "patch" => {
                let out = std::process::Command::new("git")
                    .arg("-C").arg(cwd)
                    .args(["apply", "--reject", "--whitespace=nowarn"])
                    .arg(&part)
                    .output();
                if let Err(e) = &out {
                    note = Some(e.to_string());
                }
                // Rejected hunks land in .rej files: count and remove them, the original stays.
                patch_conflicts += remove_rejects(cwd);
                applied += 1;
            }
            "untracked" => {
                let tmp = dir.join(format!("{i}.untracked"));
                let _ = std::fs::create_dir_all(&tmp);
                let ok = std::process::Command::new("tar").arg("-xf").arg(&part).arg("-C").arg(&tmp).output().map(|o| o.status.success()).unwrap_or(false);
                if ok {
                    untracked_conflicts += copy_new(&tmp, &tmp, cwd);
                    applied += 1;
                } else {
                    note = Some("the untracked-files archive could not be read".into());
                }
            }
            _ => {}
        }
    }
    json!({ "applied": applied, "patch_conflicts": patch_conflicts, "untracked_conflicts": untracked_conflicts, "note": note })
}

/// A commit object of the workspace as it is now (not on any branch), so the way back can
/// diff against exactly what the Mac sent.
fn snapshot(cwd: &FsPath) -> Option<String> {
    let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(cwd).args(args).output().ok().filter(|o| o.status.success());
    git(&["add", "-A"])?;
    let tree = String::from_utf8(git(&["write-tree"])?.stdout).ok()?.trim().to_string();
    let commit = git(&["-c", "user.name=teleport", "-c", "user.email=teleport@localhost", "commit-tree", &tree, "-m", "teleport snapshot"]).and_then(|o| String::from_utf8(o.stdout).ok()).map(|c| c.trim().to_string());
    let _ = git(&["reset", "-q"]);
    commit.filter(|c| !c.is_empty())
}

fn remove_rejects(dir: &FsPath) -> u32 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            if p.is_dir() {
                n += remove_rejects(&p);
            } else if name.ends_with(".rej") && std::fs::remove_file(&p).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// Copy extracted files into the workspace without overwriting; returns how many already existed.
fn copy_new(root: &FsPath, dir: &FsPath, cwd: &FsPath) -> u32 {
    let mut conflicts = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if e.file_name() != ".git" {
                conflicts += copy_new(root, &p, cwd);
            }
            continue;
        }
        let Ok(rel) = p.strip_prefix(root) else { continue };
        if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            continue;
        }
        let dest = cwd.join(rel);
        if dest.exists() {
            conflicts += 1;
        } else {
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&p, &dest);
        }
    }
    conflicts
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlacementBody {
    session_id: String,
    /// A connected server's id; null = back on this Mac.
    target_id: Option<String>,
}

/// Flip where a session lives once a teleport has activated (or returned).
async fn set_placement(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<PlacementBody>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    blocking(move || {
        match body.target_id.as_deref() {
            Some(t) => {
                if crate::placement::owned_target(&db, &user.user_id, t).is_none() {
                    return Err(err(StatusCode::BAD_REQUEST, "teleport_rejected", "that server isn't connected and ready"));
                }
                crate::placement::record(&db, &body.session_id, t);
            }
            None => crate::placement::clear(&db, &body.session_id),
        }
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Where a session runs: `{ "place": "local" }` or `{ "place": "cloud", "targetId", "label" }`.
/// Drives the Local / Cloud badge on session rows and the Code session header.
async fn get_placement(State(state): State<Arc<AppState>>, Path(session_id): Path<String>) -> Json<Value> {
    Json(match crate::placement::session_target(&state.db, &session_id) {
        Some(t) => json!({ "place": "cloud", "targetId": t.id, "label": t.name }),
        None => json!({ "place": "local" }),
    })
}

/// Every session placed on a connected computer, for list badges (one call, not one per row).
async fn list_placements(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    blocking(move || {
        let conn = db.connect().map_err(internal)?;
        let mut stmt = conn
            .prepare(
                "SELECT p.session_id, t.id, COALESCE(t.name, 'My server')
                 FROM session_placements p JOIN remote_backend_targets t ON t.id = p.target_id
                 WHERE t.user_id = ?1 AND t.status = 'ready'",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map(params![user.user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .map_err(internal)?;
        let mut out = serde_json::Map::new();
        for row in rows.flatten() {
            out.insert(row.0, json!({ "targetId": row.1, "label": row.2 }));
        }
        Ok(Json(json!({ "placements": out })))
    })
    .await
}

/// Forward a call to a connected server's `/api/v1/<rest>` with its stored token, so the
/// Mac-side client never holds the server's credentials. Only the teleport paths pass.
async fn relay(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((target_id, rest)): Path<(String, String)>,
    method: Method,
    body: Bytes,
) -> Result<(StatusCode, Bytes), ApiError> {
    if !rest.starts_with("teleports") || rest.contains("..") {
        return Err(err(StatusCode::BAD_REQUEST, "teleport_rejected", "only teleport calls can be relayed"));
    }
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let tid = target_id.clone();
    let target = blocking(move || Ok(crate::placement::owned_target(&db, &uid, &tid))).await?;
    let target = target.ok_or_else(|| err(StatusCode::BAD_REQUEST, "teleport_rejected", "that server isn't connected and ready"))?;
    let mut req = crate::net_client::client()
        .request(method, format!("{}/api/v1/{rest}", target.gateway_url))
        .body(body.to_vec());
    if let Some(t) = &target.token {
        req = req.bearer_auth(t);
    }
    let res = req.header(reqwest::header::CONTENT_TYPE, "application/octet-stream").send().await.map_err(|e| {
        err(StatusCode::BAD_GATEWAY, "teleport_rejected", format!("{} is unreachable: {e}", target.name))
    })?;
    let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    Ok((status, res.bytes().await.map_err(internal)?))
}

/// Find a native session file this computer already has, by harness and id.
fn find_native(harness: &str, id: &str) -> Option<PathBuf> {
    if !valid_token(id) {
        return None;
    }
    let (root, flat) = match harness {
        "claude" => (std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".claude")).join("projects"), true),
        "codex" => (std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".codex")).join("sessions"), false),
        _ => return None,
    };
    fn walk(dir: &FsPath, id: &str, flat: bool, depth: u8) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if depth < 4 {
                    if let Some(found) = walk(&path, id, flat, depth + 1) {
                        return Some(found);
                    }
                }
            } else if name.ends_with(".jsonl") && (if flat { name == format!("{id}.jsonl") } else { name.contains(id) }) {
                return Some(path);
            }
        }
        None
    }
    walk(&root, id, flat, 0)
}

/// Cloud -> Mac: the native session file, byte for byte.
async fn export_native(Path((harness, id)): Path<(String, String)>) -> Result<(StatusCode, [(&'static str, String); 2], Bytes), ApiError> {
    blocking(move || {
        let path = find_native(&harness, &id).ok_or_else(|| err(StatusCode::NOT_FOUND, "not_found", "no native session file for that id"))?;
        let bytes = std::fs::read(&path).map_err(internal)?;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let sha = hex::encode(Sha256::digest(&bytes));
        Ok((StatusCode::OK, [("content-type", "application/octet-stream".to_string()), ("x-teleport-file", format!("{name};{sha}"))], Bytes::from(bytes)))
    })
    .await
}

#[derive(Deserialize)]
struct DiffQuery {
    /// Workspace folder name (as in the manifest) and the commit the Mac started from.
    workspace_name: String,
    base: String,
}

/// Cloud -> Mac: what changed in the workspace since the commit the Mac teleported from.
async fn export_diff(
    State(state): State<Arc<AppState>>,
    Path((harness, id)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<DiffQuery>,
) -> Result<(StatusCode, Bytes), ApiError> {
    if !valid_token(&q.workspace_name) || q.base.len() < 7 || q.base.len() > 64 || !q.base.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err(StatusCode::BAD_REQUEST, "teleport_rejected", "bad workspace_name or base"));
    }
    let cwd = state.data_dir.join("teleport-workspaces").join(&q.workspace_name);
    let db = state.db.clone();
    let key = format!("{harness}:{id}");
    blocking(move || {
        // Prefer the snapshot taken when the Mac's work was applied; fall back to the caller's base.
        let snap: Option<String> = db.connect().ok().and_then(|conn| {
            ensure_tables(&conn).ok()?;
            conn.query_row(
                "SELECT t.project_report FROM teleports t JOIN teleport_native_owners o ON o.teleport_id = t.id WHERE o.native_key = ?1",
                params![key],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .and_then(|r| serde_json::from_str::<Value>(&r).ok())
            .and_then(|v| v["snapshot"].as_str().map(str::to_string))
        });
        let q = DiffQuery { base: snap.unwrap_or(q.base), ..q };
        // Untracked files count: intent-to-add makes them show in the diff.
        let _ = std::process::Command::new("git").arg("-C").arg(&cwd).args(["add", "-N", "."]).output();
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .args(["diff", "--binary", &q.base])
            .output()
            .map_err(internal)?;
        if !out.status.success() {
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "teleport_rejected", String::from_utf8_lossy(&out.stderr)));
        }
        Ok((StatusCode::OK, Bytes::from(out.stdout)))
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
            session_id: None,
            direction: None,
            git_remote: None,
            git_base: None,
            files: vec![FileEntry { name: "x.jsonl".into(), size: 10, sha256: "a".repeat(64), kind: "native".into() }],
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
