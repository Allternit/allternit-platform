//! Memory Drive API (`/api/v1/memory/drive/*`), mounted once behind the
//! normal auth middleware. Contract: docs/MEMORY_DRIVE_CONTRACT.md.
//! Every handler resolves the caller from `AuthUser` and checks drive access
//! server-side; personal drives are indexed for recall, shared drives are
//! read from git.
use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Extension, Json, Router,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::memory_drive::{DriveError, Entry, Operation};
use crate::memory_drive_scopes::{self as scopes, DriveRef};
use crate::memory_drive_service::{self as service, DriveRecord, ServiceError};
use crate::memory_dream::{self as dream, UndoError};
use crate::AppState;

pub fn memory_drive_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memory/drive/mounts", get(mounts))
        .route("/memory/drive/info", get(info))
        .route("/memory/drive/tree", get(tree))
        .route("/memory/drive/file", get(file))
        .route("/memory/drive/history", get(history))
        .route("/memory/drive/diff", get(diff))
        .route("/memory/drive/write", post(write))
        .route("/memory/drive/remember", post(remember))
        .route("/memory/drive/reindex", post(reindex))
        .route("/memory/drive/health", get(health))
        .route("/memory/drive/import", post(import))
        .route("/memory/drive/import-text", post(import_text))
        .route("/memory/drive/purge", post(purge))
        .route("/memory/drive/peers", get(list_peers))
        .route("/memory/drive/peers/:peer_id", axum::routing::put(save_peer))
        .route("/memory/drive/tokens", get(list_tokens).post(mint_token))
        .route("/memory/drive/tokens/:id", delete(revoke_token))
        .route("/memory/drive/settings", get(get_settings).put(put_settings))
        .route("/memory/drive/dreams", get(list_dreams))
        .route("/memory/drive/dreams/run", post(run_dream))
        .route("/memory/drive/dreams/:id/undo", post(undo_dream))
        .route("/memory/drive/questions", get(list_questions).post(ask_question))
        .route("/memory/drive/questions/:id/answer", post(answer_question))
        .route("/memory/drive/questions/:id/resolve", post(resolve_question))
        .layer(DefaultBodyLimit::max(3 * 1024 * 1024))
}

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn error(e: ServiceError) -> Response {
    match &e {
        ServiceError::NotFound | ServiceError::Drive(DriveError::NotFound(_)) => err(StatusCode::NOT_FOUND, e.to_string()),
        ServiceError::Forbidden => err(StatusCode::FORBIDDEN, e.to_string()),
        ServiceError::Drive(DriveError::Conflict { expected, actual }) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "Memory changed since you opened it. Reload and try again.",
                "expected_revision": expected, "actual_revision": actual,
            })),
        )
            .into_response(),
        ServiceError::IndexPending { revision } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": e.to_string(), "committed": true, "revision": revision, "index_dirty": true })),
        )
            .into_response(),
        ServiceError::Database(inner) => {
            tracing::warn!("memory drive db error: {inner}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "Memory storage is unavailable. Retry later.")
        }
        ServiceError::Drive(DriveError::Io(inner)) => {
            tracing::warn!("memory drive io error: {inner}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "Memory storage is unavailable. Retry later.")
        }
        ServiceError::Drive(DriveError::Git { .. }) => {
            tracing::warn!("memory drive git error: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "Memory storage is unavailable. Retry later.")
        }
        ServiceError::Drive(DriveError::OwnerMismatch) => err(StatusCode::NOT_FOUND, "Memory drive not found."),
        _ => err(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

type Ctx = (Arc<AppState>, AuthUser);

/// Run blocking drive work for an authenticated user.
async fn run(state: Arc<AppState>, user: AuthUser, f: impl FnOnce(&Ctx) -> service::Result<Value> + Send + 'static) -> Response {
    if user.user_id.is_empty() {
        return err(StatusCode::UNAUTHORIZED, "Sign in to use your memory drive.");
    }
    match tokio::task::spawn_blocking(move || f(&(state, user))).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => error(e),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "Memory request failed. Retry later."),
    }
}

fn open(ctx: &Ctx, drive: Option<&str>, write: bool) -> service::Result<(DriveRef, DriveRecord)> {
    let r = DriveRef::parse(drive, &ctx.1.user_id)?;
    let root = crate::memory_drive_writer::root_or(&ctx.0.db, &ctx.0.config.brains_dir())?;
    let d = scopes::open(&ctx.0.db, &root, &ctx.1.user_id, &r, write)?;
    Ok((r, d))
}

fn author(user: &AuthUser) -> String {
    user.name.clone().or_else(|| user.email.clone()).unwrap_or_else(|| "member".into())
}

#[derive(Default, Deserialize)]
struct DriveQuery {
    drive: Option<String>,
    revision: Option<String>,
    path: Option<String>,
    limit: Option<usize>,
    from: Option<String>,
    to: Option<String>,
}

async fn mounts(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    run(state, user, |(s, u)| Ok(json!({ "mounts": scopes::mounts(&s.db, &u.user_id)? }))).await
}

async fn info(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, headers: HeaderMap, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| {
        let (r, d) = open(ctx, q.drive.as_deref(), false)?;
        if d.kind == "personal" {
            service::repair_index(&ctx.0.db, &d.user_id)?;
        }
        let d = if d.kind == "personal" { service::resolve(&ctx.0.db, &d.user_id)? } else { d };
        let conn = ctx.0.db.connect()?;
        let pending = crate::memory_drive_writer::pending_count(&conn, &ctx.1.user_id)?;
        let tree = d.storage()?.snapshot(None)?;
        let bytes: usize = tree.files.values().map(String::len).sum();
        let usage = json!({ "bytes": bytes, "files": tree.files.len(),
            "max_bytes": crate::memory_drive::MAX_DRIVE_BYTES, "max_files": crate::memory_drive::MAX_FILES });
        if bytes * 10 >= crate::memory_drive::MAX_DRIVE_BYTES * 8 || tree.files.len() * 10 >= crate::memory_drive::MAX_FILES * 8 {
            crate::metrics::inc_memory_drive_event("near_limit");
        }
        Ok(json!({
            "usage": usage,
            "purged_ids": d.storage()?.purged_ids()?,
            "ref": r.label(), "kind": d.kind, "name": d.name, "brain_id": d.brain_id, "branch": d.branch,
            "revision": d.storage()?.head()?, "indexed_revision": d.indexed_revision,
            "index_dirty": d.kind == "personal" && d.dirty_revision.is_some(),
            "imported_at": d.imported_at, "pending_writes": pending,
            "clone_url": crate::brain_routes::clone_url_for(&headers, &d.brain_id),
            "access": scopes::access(&conn, &ctx.1.user_id, &d.kind, &d.scope_id)?,
        }))
    })
    .await
}

async fn tree(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, q.drive.as_deref(), false)?;
        let snapshot = d.storage()?.snapshot(q.revision.as_deref())?;
        let files: Vec<_> = snapshot.files.iter().map(|(p, c)| json!({ "path": p, "bytes": c.len() })).collect();
        Ok(json!({ "revision": snapshot.revision, "files": files }))
    })
    .await
}

async fn file(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| {
        let path = q.path.clone().ok_or_else(|| ServiceError::Provenance("path is required".into()))?;
        crate::memory_drive::validate_path(&path)?;
        let (_, d) = open(ctx, q.drive.as_deref(), false)?;
        let mut snapshot = d.storage()?.snapshot(q.revision.as_deref())?;
        let content = snapshot.files.remove(&path).ok_or_else(|| DriveError::NotFound(path.clone()))?;
        Ok(json!({ "revision": snapshot.revision, "path": path, "content": content }))
    })
    .await
}

async fn history(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, q.drive.as_deref(), false)?;
        let commits = d.storage()?.history(q.path.as_deref(), q.limit.unwrap_or(25))?;
        Ok(json!({ "commits": commits }))
    })
    .await
}

async fn diff(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| {
        let (Some(from), Some(to)) = (q.from.clone(), q.to.clone()) else {
            return Err(ServiceError::Provenance("from and to are required".into()));
        };
        let (_, d) = open(ctx, q.drive.as_deref(), false)?;
        let patch = d.storage()?.diff(&from, &to, q.path.as_deref())?;
        Ok(json!({ "from": from, "to": to, "path": q.path, "diff": patch }))
    })
    .await
}

#[derive(Deserialize)]
struct WriteBody {
    drive: Option<String>,
    expected_revision: String,
    #[serde(default)]
    message: Option<String>,
    operations: Vec<Operation>,
}

async fn write(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<WriteBody>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, b.drive.as_deref(), true)?;
        let message = b.message.as_deref().map(str::trim).filter(|m| !m.is_empty() && m.len() <= 200 && !m.contains('\n')).unwrap_or("Edit memory");
        let r = scopes::apply_for(&ctx.0.db, &ctx.1.user_id, &d, &b.expected_revision, &b.operations, message)?;
        Ok(json!({ "revision": r.revision, "changed": r.changed }))
    })
    .await
}

#[derive(Deserialize)]
struct RememberBody {
    drive: Option<String>,
    text: String,
    source: Option<String>,
    session: Option<String>,
    memory_type: Option<String>,
    path: Option<String>,
}

async fn remember(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<RememberBody>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, b.drive.as_deref(), true)?;
        if let Some(p) = &b.path {
            crate::memory_drive::validate_path(p)?;
        }
        if d.kind == "personal" {
            let fact = crate::memory_drive_writer::NewFact {
                text: b.text.clone(),
                memory_type: b.memory_type.clone(),
                session: b.session.clone(),
                source: b.source.clone(),
                path: b.path.clone(),
                ..Default::default()
            };
            let out = crate::memory_drive_writer::commit_facts(&ctx.0.db, &ctx.1.user_id, &[fact], &[], "Remember")?
                .ok_or(ServiceError::NotFound)?;
            let Some(Some(fact_id)) = out.fact_ids.first().cloned() else {
                return Err(ServiceError::Provenance("Nothing saved: it is empty, already remembered, or looks like a credential.".into()));
            };
            let conn = ctx.0.db.connect()?;
            let (path, entry_id): (String, String) = conn.query_row(
                "SELECT file_path,entry_id FROM memory_drive_entries WHERE fact_id=?1",
                params![fact_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            return Ok(json!({ "revision": out.revision, "fact_id": fact_id, "path": path, "entry_id": entry_id }));
        }
        // Shared drive: a plain spec entry, no personal index.
        let mut entry = Entry {
            id: crate::memory_drive_writer::new_entry_id(),
            text: b.text.split_whitespace().collect::<Vec<_>>().join(" "),
            source: b.source.clone().unwrap_or_else(|| match &b.session {
                Some(s) => format!("/?session={s}"),
                None => format!("allternit:drive/{}", d.id),
            }),
            added: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            metadata: Default::default(),
        };
        if let Some(t) = &b.memory_type {
            entry.metadata.insert("memory_type".into(), t.clone());
        }
        entry.metadata.insert("author".into(), author(&ctx.1).replace([';', '[', ']', '\\'], ""));
        entry.render()?;
        let path = b.path.clone().unwrap_or_else(|| crate::memory_drive_writer::topic_for(b.memory_type.as_deref()).into());
        let head = d.storage()?.head()?.ok_or(DriveError::Uninitialized)?;
        let r = scopes::apply_for(&ctx.0.db, &ctx.1.user_id, &d, &head, &[Operation::UpsertEntry { path: path.clone(), entry: entry.clone() }], "Remember")?;
        Ok(json!({ "revision": r.revision, "path": path, "entry_id": entry.id }))
    })
    .await
}

async fn reindex(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    run(state, user, |ctx| {
        open(ctx, None, true)?;
        crate::memory_drive_writer::retry_pending(&ctx.0.db, &ctx.1.user_id);
        let d = service::reindex(&ctx.0.db, &ctx.1.user_id)?;
        Ok(json!({ "indexed_revision": d.indexed_revision, "index_dirty": d.dirty_revision.is_some() }))
    })
    .await
}

async fn health(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    run(state, user, |ctx| {
        let (_, d) = open(ctx, None, false)?;
        crate::memory_drive_writer::retry_pending(&ctx.0.db, &ctx.1.user_id);
        service::repair_index(&ctx.0.db, &ctx.1.user_id)?;
        let d = service::resolve(&ctx.0.db, &d.user_id)?;
        let pending = crate::memory_drive_writer::pending_count(&ctx.0.db.connect()?, &ctx.1.user_id)?;
        Ok(json!({
            "revision": d.storage()?.head()?, "indexed_revision": d.indexed_revision,
            "index_dirty": d.dirty_revision.is_some(), "imported_at": d.imported_at, "pending_writes": pending,
        }))
    })
    .await
}

#[derive(Default, Deserialize)]
struct ImportBody {
    #[serde(default)]
    apply: bool,
    expected_revision: Option<String>,
}

async fn import(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, body: Option<Json<ImportBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    run(state, user, move |ctx| {
        let owner = &ctx.1.user_id;
        if !body.apply {
            // Dry run: no provisioning, no writes.
            return Ok(json!({ "dry_run": true, "plan": service::import_plan(&ctx.0.db, owner)? }));
        }
        open(ctx, None, true)?;
        let expected = body.expected_revision.ok_or_else(|| ServiceError::Provenance("apply requires expected_revision from drive info".into()))?;
        let r = service::import_apply(&ctx.0.db, owner, &expected)?;
        Ok(json!({ "dry_run": false, "revision": r.revision, "changed": r.changed }))
    })
    .await
}

// ─── Tokens ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MintBody {
    drive: Option<String>,
    label: Option<String>,
    access: Option<String>,
}

async fn list_tokens(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, q.drive.as_deref(), false)?;
        let conn = ctx.0.db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id,name,access,created_at,last_used_at FROM git_tokens WHERE user_id=?1 AND brain_id=?2 ORDER BY created_at DESC",
        )?;
        let tokens = stmt
            .query_map(params![ctx.1.user_id, d.brain_id], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "label": r.get::<_, Option<String>>(1)?, "access": r.get::<_, String>(2)?,
                    "created_at": r.get::<_, Option<String>>(3)?, "last_used_at": r.get::<_, Option<String>>(4)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({ "tokens": tokens }))
    })
    .await
}

async fn mint_token(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, headers: HeaderMap, Json(b): Json<MintBody>) -> Response {
    run(state, user, move |ctx| {
        let access = match b.access.as_deref().unwrap_or("read") {
            "read" => "read",
            "write" => "write",
            _ => return Err(ServiceError::Provenance("access must be read or write".into())),
        };
        let (_, d) = open(ctx, b.drive.as_deref(), access == "write")?;
        let label = b
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| l.chars().filter(|c| !c.is_control()).take(80).collect::<String>())
            .unwrap_or_else(|| format!("Memory Drive {access}"));
        let conn = ctx.0.db.connect()?;
        let (id, token) = crate::brain_routes::create_scoped_git_token(&conn, &ctx.1.user_id, Some(&label), &d.brain_id, access)?;
        Ok(json!({
            "id": id, "token": token, "username": "x-access-token", "access": access, "label": label,
            "clone_url": crate::brain_routes::clone_url_for(&headers, &d.brain_id),
            "note": "Copy this token now. It is not shown again.",
        }))
    })
    .await
}

async fn revoke_token(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    run(state, user, move |ctx| {
        let n = ctx.0.db.connect()?.execute(
            "DELETE FROM git_tokens WHERE id=?1 AND user_id=?2 AND brain_id IS NOT NULL AND brain_id IN (SELECT brain_id FROM memory_drives)",
            params![id, ctx.1.user_id],
        )?;
        if n == 0 {
            return Err(ServiceError::NotFound);
        }
        Ok(json!({ "revoked": true }))
    })
    .await
}

// ─── Dreams ─────────────────────────────────────────────────────────────────

async fn get_settings(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    run(state, user, |ctx| {
        open(ctx, None, false)?;
        let conn = ctx.0.db.connect()?;
        Ok(json!({ "dreaming_enabled": dream::dreaming_enabled(&conn, &ctx.1.user_id)?, "timezone": dream::timezone(&conn, &ctx.1.user_id)? }))
    })
    .await
}

#[derive(Deserialize)]
struct SettingsBody {
    dreaming_enabled: Option<bool>,
    timezone: Option<String>,
}

async fn put_settings(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<SettingsBody>) -> Response {
    run(state, user, move |ctx| {
        open(ctx, None, true)?;
        if let Some(on) = b.dreaming_enabled {
            dream::set_dreaming(&ctx.0.db, &ctx.1.user_id, on)?;
        }
        if let Some(tz) = b.timezone.as_deref().filter(|t| !t.is_empty()) {
            dream::set_timezone(&ctx.0.db, &ctx.1.user_id, tz)?;
        }
        let conn = ctx.0.db.connect()?;
        Ok(json!({ "dreaming_enabled": dream::dreaming_enabled(&conn, &ctx.1.user_id)?, "timezone": dream::timezone(&conn, &ctx.1.user_id)? }))
    })
    .await
}

async fn list_dreams(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| Ok(json!({ "dreams": dream::list(&ctx.0.db, &ctx.1.user_id, q.limit.unwrap_or(30))? }))).await
}

async fn run_dream(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    if user.user_id.is_empty() {
        return err(StatusCode::UNAUTHORIZED, "Sign in to use your memory drive.");
    }
    let owner = user.user_id.clone();
    let db = state.db.clone();
    let fallback = state.config.brains_dir();
    let provisioned = tokio::task::spawn_blocking({
        let (db, owner) = (db.clone(), owner.clone());
        move || {
            let root = crate::memory_drive_writer::root_or(&db, &fallback)?;
            scopes::open(&db, &root, &owner, &DriveRef::personal(&owner), true)
        }
    })
    .await;
    match provisioned {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return error(e),
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "Memory request failed. Retry later."),
    }
    let tz = db.connect().ok().and_then(|c| dream::timezone(&c, &owner).ok().flatten());
    let now = chrono::Utc::now();
    let today = match tz.as_deref().and_then(|t| t.parse::<chrono_tz::Tz>().ok()) {
        Some(z) => now.with_timezone(&z).date_naive(),
        None => chrono::Local::now().date_naive(),
    };
    match dream::run(db.clone(), owner.clone(), today, dream::gizzi_completer(owner.clone())).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => {
            // Already ran today (or running): return today's row.
            let date = today.format("%Y-%m-%d").to_string();
            match dream::list(&db, &owner, 30) {
                Ok(rows) => match rows.into_iter().find(|r| r.date == date) {
                    Some(r) => Json(r).into_response(),
                    None => err(StatusCode::CONFLICT, "A Dream is already running. Try again in a few minutes."),
                },
                Err(e) => error(e),
            }
        }
        Err(e) => error(e),
    }
}

async fn undo_dream(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    if user.user_id.is_empty() {
        return err(StatusCode::UNAUTHORIZED, "Sign in to use your memory drive.");
    }
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || dream::undo(&db, &user.user_id, &id)).await {
        Ok(Ok(row)) => Json(json!({ "undone": true, "revision": row.undo_revision, "dream": row })).into_response(),
        Ok(Err(UndoError::Conflict(paths))) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "Some lines this Dream changed were edited later. Undo them by hand in these files.", "conflicts": paths })),
        )
            .into_response(),
        Ok(Err(UndoError::NotUndoable(m))) => err(StatusCode::BAD_REQUEST, m),
        Ok(Err(UndoError::Service(e))) => error(e),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "Memory request failed. Retry later."),
    }
}

// ─── Questions board ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct QuestionBody {
    drive: Option<String>,
    text: Option<String>,
    source: Option<String>,
}

async fn list_questions(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<DriveQuery>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, q.drive.as_deref(), false)?;
        let (revision, questions) = scopes::questions(&d)?;
        Ok(json!({ "revision": revision, "questions": questions }))
    })
    .await
}

fn text_of(b: &QuestionBody) -> service::Result<String> {
    b.text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ServiceError::Provenance("text is required".into()))
}

async fn ask_question(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<QuestionBody>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, b.drive.as_deref(), true)?;
        let (revision, entry) = scopes::ask(&ctx.0.db, &ctx.1.user_id, &author(&ctx.1), &d, &text_of(&b)?, b.source.as_deref())?;
        Ok(json!({ "revision": revision, "question": entry }))
    })
    .await
}

async fn answer_question(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<QuestionBody>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, b.drive.as_deref(), true)?;
        let (revision, entry) = scopes::answer(&ctx.0.db, &ctx.1.user_id, &author(&ctx.1), &d, &id, &text_of(&b)?, b.source.as_deref())?;
        Ok(json!({ "revision": revision, "answer": entry }))
    })
    .await
}

async fn resolve_question(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>, Json(b): Json<QuestionBody>) -> Response {
    run(state, user, move |ctx| {
        let (_, d) = open(ctx, b.drive.as_deref(), true)?;
        Ok(json!({ "revision": scopes::resolve(&ctx.0.db, &ctx.1.user_id, &d, &id)? }))
    })
    .await
}

#[derive(Deserialize)]
struct TextImportBody {
    text: String,
    from: String,
    #[serde(default)]
    apply: bool,
}

/// Import memories pasted or uploaded from another assistant. Dry run
/// unless `apply` is true; apply is one commit.
async fn import_text(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<TextImportBody>) -> Response {
    run(state, user, move |ctx| {
        if b.text.len() > 512 * 1024 {
            return Err(ServiceError::Provenance("That file is too large. Import up to 512 KB at a time.".into()));
        }
        open(ctx, None, b.apply)?;
        if !b.apply {
            let plan = crate::memory_drive_text_import::plan(&ctx.0.db, &ctx.1.user_id, &b.text, &b.from)?;
            return Ok(json!({ "dry_run": true, "plan": plan }));
        }
        let (revision, imported) = crate::memory_drive_text_import::apply(&ctx.0.db, &ctx.1.user_id, &b.text, &b.from)?;
        Ok(json!({ "dry_run": false, "revision": revision, "imported": imported }))
    })
    .await
}

#[derive(Deserialize)]
struct PurgeBody {
    drive: Option<String>,
    expected_revision: String,
    entry_ids: Vec<String>,
    /// Must be exactly "delete forever".
    confirm: String,
}

/// Permanently delete entries from the drive and its whole history.
async fn purge(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<PurgeBody>) -> Response {
    run(state, user, move |ctx| {
        if b.confirm != "delete forever" {
            return Err(ServiceError::Provenance("Confirm with \"delete forever\" to remove memories from history.".into()));
        }
        if b.entry_ids.is_empty() || b.entry_ids.len() > 200 {
            return Err(ServiceError::Provenance("Choose 1 to 200 memories to delete.".into()));
        }
        let (_, d) = open(ctx, b.drive.as_deref(), true)?;
        let revision = scopes::purge(&ctx.0.db, &ctx.1.user_id, &d, &b.expected_revision, &b.entry_ids)?;
        Ok(json!({ "revision": revision, "purged": b.entry_ids.len() }))
    })
    .await
}

/// Sync state with the owner's other computers (personal drive).
async fn list_peers(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    run(state, user, |ctx| {
        let (_, d) = open(ctx, None, false)?;
        let conn = ctx.0.db.connect()?;
        let mut st = conn.prepare(
            "SELECT peer_id,peer_name,local_revision,peer_revision,last_sync_at,last_error FROM memory_drive_peers WHERE drive_id=?1 ORDER BY peer_id",
        )?;
        let peers = st
            .query_map(params![d.id], |r| {
                Ok(json!({ "peer_id": r.get::<_, String>(0)?, "peer_name": r.get::<_, Option<String>>(1)?,
                    "local_revision": r.get::<_, Option<String>>(2)?, "peer_revision": r.get::<_, Option<String>>(3)?,
                    "last_sync_at": r.get::<_, Option<String>>(4)?, "last_error": r.get::<_, Option<String>>(5)? }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({ "peers": peers }))
    })
    .await
}

#[derive(Deserialize)]
struct PeerBody {
    peer_name: Option<String>,
    local_revision: Option<String>,
    peer_revision: Option<String>,
    last_error: Option<String>,
}

async fn save_peer(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(peer_id): Path<String>, Json(b): Json<PeerBody>) -> Response {
    run(state, user, move |ctx| {
        if peer_id.is_empty() || peer_id.len() > 128 || !peer_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
            return Err(ServiceError::Provenance("invalid computer id".into()));
        }
        for rev in [&b.local_revision, &b.peer_revision].into_iter().flatten() {
            crate::memory_drive::validate_oid(rev)?;
        }
        let (_, d) = open(ctx, None, true)?;
        let name: Option<String> = b.peer_name.as_deref().map(|n| n.chars().filter(|c| !c.is_control()).take(80).collect());
        let err: Option<String> = b.last_error.as_deref().map(|n| n.chars().filter(|c| !c.is_control()).take(300).collect());
        let synced = b.last_error.is_none() && b.local_revision.is_some() && b.peer_revision.is_some();
        ctx.0.db.connect()?.execute(
            "INSERT INTO memory_drive_peers(drive_id,peer_id,peer_name,local_revision,peer_revision,last_sync_at,last_error)
             VALUES(?1,?2,?3,?4,?5,CASE WHEN ?7 THEN CURRENT_TIMESTAMP END,?6)
             ON CONFLICT(drive_id,peer_id) DO UPDATE SET peer_name=COALESCE(excluded.peer_name,peer_name),
               local_revision=CASE WHEN ?7 THEN excluded.local_revision ELSE local_revision END,
               peer_revision=CASE WHEN ?7 THEN excluded.peer_revision ELSE peer_revision END,
               last_sync_at=CASE WHEN ?7 THEN CURRENT_TIMESTAMP ELSE last_sync_at END,
               last_error=excluded.last_error",
            params![d.id, peer_id, name, b.local_revision, b.peer_revision, err, synced],
        )?;
        Ok(json!({ "saved": true }))
    })
    .await
}
