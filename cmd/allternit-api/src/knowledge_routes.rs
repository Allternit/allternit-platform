//! Knowledge factory routes under `/api/v1/knowledge/...` (contract:
//! `handoffs/memory-raw-rag-knowledge-factory-2026-11-27/CONTRACT.md`, section
//! "Knowledge factory"; store: [`crate::knowledge_store`]).
//!
//! Every route is signed-in only (`get_user`, the same identity as the other
//! memory routes) and the scope is always the signed-in user: no route takes a
//! user id. Agents file flags freely; every proposal waits for the person's
//! approve, and nothing here activates a proposal by itself.
//!
//! - `POST /knowledge/flags` → `{flag}`
//! - `GET  /knowledge/flags?status=` → `{flags}`
//! - `POST /knowledge/flags/{id}/dismiss` → `{flag}`
//! - `GET  /knowledge/proposals?status=` → `{proposals}`
//! - `POST /knowledge/proposals` → `{proposal}`
//! - `POST /knowledge/proposals/{id}/eval` `{eval}` → `{proposal}` (only while `proposed`)
//! - `POST /knowledge/proposals/{id}/approve|reject|activated|rollback` → `{proposal}`

use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::get_user;
use crate::db::DbHandle;
use crate::knowledge_store::{self as store, Decision, NewFlag, NewProposal, Proposal, StoreError};
use crate::AppState;

type Reply = (StatusCode, Json<Value>);

pub fn knowledge_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/knowledge/flags", get(list_flags).post(create_flag))
        .route("/knowledge/flags/:id/dismiss", post(dismiss_flag))
        .route("/knowledge/proposals", get(list_proposals).post(create_proposal))
        .route("/knowledge/proposals/:id/eval", post(set_eval))
        .route("/knowledge/proposals/:id/approve", post(approve))
        .route("/knowledge/proposals/:id/reject", post(reject))
        .route("/knowledge/proposals/:id/activated", post(activated))
        .route("/knowledge/proposals/:id/rollback", post(rollback))
}

/// Body of `POST /knowledge/proposals/{id}/eval`.
#[derive(Debug, Deserialize)]
pub struct EvalBody {
    pub eval: Value,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn err(status: StatusCode, msg: impl Into<String>) -> Reply {
    (status, Json(json!({ "error": msg.into() })))
}

fn store_err(e: StoreError) -> Reply {
    match e {
        StoreError::Invalid(m) => err(StatusCode::BAD_REQUEST, m),
        StoreError::NotFound => err(StatusCode::NOT_FOUND, "Not found"),
        StoreError::Conflict(m) => err(StatusCode::CONFLICT, m),
        StoreError::Db(e) => {
            tracing::warn!("knowledge route database error: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "Knowledge request failed. Retry later.")
        }
    }
}

/// Same identity as the other `/memory` handlers; an empty id is refused.
fn user_id(headers: &HeaderMap) -> Result<String, Reply> {
    match get_user(headers) {
        Some(u) if !u.user_id.is_empty() => Ok(u.user_id),
        _ => Err(err(StatusCode::UNAUTHORIZED, "Unauthorized")),
    }
}

/// Run blocking store work off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> store::Result<T> + Send + 'static) -> Result<T, Reply> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(store_err(e)),
        Err(_) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Knowledge request failed. Retry later.")),
    }
}

fn status_filter(q: &ListQuery) -> Option<String> {
    q.status.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

// ── Approve ─────────────────────────────────────────────────────────────────

/// Drive path for an approved `domain` / `data_access` proposal, `None` for
/// other kinds. `target_ref` is a slug (the store validates it); the checks
/// here keep the path inside `knowledge/<kind>/` even if that ever changes.
fn knowledge_path(p: &Proposal) -> store::Result<Option<String>> {
    let dir = match p.target_kind.as_str() {
        "domain" => "knowledge/domain",
        "data_access" => "knowledge/data-access",
        _ => return Ok(None),
    };
    let r = p.target_ref.as_str();
    if r.is_empty() || r.starts_with('.') || r.contains("..") || !r.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
        return Err(StoreError::Invalid("target_ref is not a safe file name".into()));
    }
    let path = format!("{dir}/{r}.md");
    let ok = std::path::Path::new(&path).components().all(|c| matches!(c, std::path::Component::Normal(_)));
    if !ok || !path.starts_with(dir) {
        return Err(StoreError::Invalid("target_ref escapes the knowledge folder".into()));
    }
    Ok(Some(path))
}

/// Body lines as the drive stores them: the drive allows only headings and
/// one-line provenance bullets, so each text line becomes one bullet.
fn body_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.replace('\t', " ").trim().to_string())
        .map(|l| {
            if l.starts_with('#') {
                l
            } else {
                l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).unwrap_or(&l).trim().to_string()
            }
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// A stored file turned back into body lines (entries -> their text).
fn stored_lines(file: &str) -> Vec<String> {
    let text: Vec<String> = file
        .lines()
        .map(|l| match (l.starts_with("- ") && !l.starts_with("- [["), crate::memory_drive::Entry::parse(l)) {
            (true, Ok(e)) => e.text,
            _ => l.to_string(),
        })
        .collect();
    body_lines(&text.join("\n"))
}

/// Render body lines as a drive file. Every entry carries `source`
/// (`knowledge:proposal/<id>`), `added` (the approval date) and `kind`: the
/// front-matter header, in the only form the drive's file format allows.
fn render_file(kind: &str, source: &str, key: &str, lines: &[String]) -> store::Result<String> {
    use sha2::{Digest, Sha256};
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with('#') {
            out.push_str(line);
        } else {
            let id = format!("kn-{}", &hex::encode(Sha256::digest(format!("{key}\0{i}").as_bytes()))[..24]);
            let entry = crate::memory_drive::Entry {
                id,
                text: line.clone(),
                source: source.to_string(),
                added: today.clone(),
                metadata: [("kind".to_string(), kind.to_string())].into_iter().collect(),
            };
            out.push_str(&entry.render().map_err(|e| drive_err(e.into()))?);
        }
        out.push('\n');
    }
    Ok(out)
}

fn drive_err(e: crate::memory_drive_service::ServiceError) -> StoreError {
    use crate::memory_drive::DriveError;
    use crate::memory_drive_service::ServiceError;
    match e {
        ServiceError::Drive(DriveError::Secret) => StoreError::Invalid("the text contains a possible credential; remove it before approving".into()),
        ServiceError::Drive(DriveError::Conflict { .. }) => StoreError::Conflict("the memory drive changed while saving; try again".into()),
        ServiceError::Drive(d @ (DriveError::Invalid { .. } | DriveError::Limit(_))) => StoreError::Invalid(d.to_string()),
        ServiceError::Provenance(m) => StoreError::Invalid(m),
        other => {
            tracing::warn!("knowledge drive write failed: {other}");
            StoreError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(other.to_string()))))
        }
    }
}

/// Write (`Some(lines)`) or remove (`None`) one knowledge file as ONE drive
/// commit. `expect` is checked against the current file text first: a
/// mismatch is a 409 and nothing is written.
fn commit_knowledge_file(
    db: &DbHandle, brains_dir: &Path, user_id: &str, path: &str, kind: &str, source: &str, key: &str,
    expect: &dyn Fn(Option<&Vec<String>>) -> store::Result<()>, lines: Option<Vec<String>>, message: &str,
) -> store::Result<()> {
    use crate::memory_drive::Operation;
    use crate::memory_drive_service as service;
    let content = match &lines {
        Some(l) => {
            let c = render_file(kind, source, key, l)?;
            crate::memory_drive::scan_secrets(&c).map_err(|e| drive_err(e.into()))?;
            Some(c)
        }
        None => None,
    };
    let root = crate::memory_drive_writer::root_or(db, brains_dir).map_err(drive_err)?;
    match service::resolve(db, user_id) {
        Ok(_) => {}
        Err(service::ServiceError::NotFound) => {
            service::provision(db, &root, user_id).map_err(drive_err)?;
        }
        Err(e) => return Err(drive_err(e)),
    }
    for attempt in 0..6u64 {
        let snap = service::read(db, user_id, None).map_err(drive_err)?;
        let head = snap.revision.clone().ok_or_else(|| StoreError::Conflict("memory drive is not ready".into()))?;
        let current = snap.files.get(path).map(|f| stored_lines(f));
        expect(current.as_ref())?;
        let op = match &content {
            Some(c) => Operation::SetFile { path: path.to_string(), content: c.clone() },
            None if current.is_none() => return Ok(()),
            None => Operation::DeleteFile { path: path.to_string() },
        };
        match service::apply_as(db, user_id, &head, &[op], message, "Knowledge via Allternit") {
            Ok(_) => return Ok(()),
            // The commit is saved; only the index refresh is pending, and the
            // next drive read repairs it.
            Err(service::ServiceError::IndexPending { .. }) => return Ok(()),
            Err(service::ServiceError::Drive(crate::memory_drive::DriveError::Conflict { .. })) if attempt < 5 => {
                std::thread::sleep(std::time::Duration::from_millis(15 * (attempt + 1)));
            }
            Err(e) => return Err(drive_err(e)),
        }
    }
    Err(StoreError::Conflict("the memory drive changed while saving; try again".into()))
}

/// Hook run on approve, before the status changes, for `domain` and
/// `data_access` proposals: write `knowledge/domain/<ref>.md` or
/// `knowledge/data-access/<ref>.md` in the user's personal Memory Drive as one
/// commit `Knowledge: <title> (approved)`, so it shows in Memory Drive history
/// and can be undone there. A possible secret is a 400; a file that no longer
/// matches the proposal's `base` is a 409 (the reviewer sees the fresh text).
/// An error leaves the proposal `proposed` and its flags untouched.
///
/// The drive's `MEMORY.md` index lists the file like any topic file. Recall
/// reads its bullets as ordinary entries; step 17 adds raw search over them.
pub fn apply_approved(db: &DbHandle, brains_dir: &Path, user_id: &str, proposal: &Proposal) -> store::Result<()> {
    let Some(path) = knowledge_path(proposal)? else { return Ok(()) };
    crate::memory_drive::scan_secrets(&proposal.body).map_err(|e| drive_err(e.into()))?;
    let lines = body_lines(&proposal.body);
    if lines.is_empty() {
        return Err(StoreError::Invalid("the proposal body is empty".into()));
    }
    let base = proposal.base.as_deref().map(body_lines).filter(|b| !b.is_empty());
    let expect = |current: Option<&Vec<String>>| -> store::Result<()> {
        let ok = match (current, &base) {
            (None, None) => true,
            (Some(c), Some(b)) => c == b,
            // No base but a file exists: only fine when it is already this text.
            (Some(c), None) => *c == lines,
            (None, Some(_)) => false,
        };
        if ok { Ok(()) } else { Err(StoreError::Conflict("the file changed since this proposal was written; review the current text".into())) }
    };
    commit_knowledge_file(
        db, brains_dir, user_id, &path, &proposal.target_kind,
        &format!("knowledge:proposal/{}", proposal.id), &proposal.id, &expect, Some(lines.clone()),
        &format!("Knowledge: {} (approved)", one_line(&proposal.title)),
    )
}

/// Undo [`apply_approved`] as a new commit: restore the `base` text, or remove
/// the file when there was none. If the file was edited after approval it is a
/// 409 and the edit is kept.
pub fn rollback_applied(db: &DbHandle, brains_dir: &Path, user_id: &str, proposal: &Proposal) -> store::Result<()> {
    let Some(path) = knowledge_path(proposal)? else { return Ok(()) };
    let applied = body_lines(&proposal.body);
    let restore = proposal.base.as_deref().map(body_lines).filter(|b| !b.is_empty());
    let expect = |current: Option<&Vec<String>>| -> store::Result<()> {
        match current {
            Some(c) if *c == applied => Ok(()),
            // Already rolled back by hand.
            None if restore.is_none() => Ok(()),
            Some(c) if Some(c) == restore.as_ref() => Ok(()),
            _ => Err(StoreError::Conflict("the file was edited after approval; undo it in Memory Drive history".into())),
        }
    };
    commit_knowledge_file(
        db, brains_dir, user_id, &path, &proposal.target_kind,
        &format!("knowledge:rollback/{}", proposal.id), &format!("{}-rollback", proposal.id), &expect, restore.clone(),
        &format!("Knowledge: {} (rolled back)", one_line(&proposal.title)),
    )
}

fn one_line(title: &str) -> String {
    let t: String = title.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>')).take(120).collect();
    let t = t.trim();
    if t.is_empty() { "untitled".into() } else { t.to_string() }
}

/// Approve a proposal for `user_id`: `domain` / `data_access` run
/// [`apply_approved`] first; then the status becomes `approved` and linked
/// flags become `resolved`. A `skill` proposal only becomes `approved`; the
/// Gizzi client activates it and reports back with `/activated`.
pub fn approve_proposal(db: &DbHandle, brains_dir: &Path, user_id: &str, id: &str) -> store::Result<Proposal> {
    let p = store::get_proposal(db, user_id, id)?.ok_or(StoreError::NotFound)?;
    if p.status != "proposed" {
        return Err(StoreError::Conflict(format!("proposal is {}", p.status)));
    }
    if matches!(p.target_kind.as_str(), "domain" | "data_access") {
        apply_approved(db, brains_dir, user_id, &p)?;
    }
    store::update_proposal_status(db, user_id, id, Decision::Approve)
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn create_flag(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<NewFlag>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    match blocking(move || store::create_flag(&db, &user, &req)).await {
        Ok(flag) => (StatusCode::OK, Json(json!({ "flag": flag }))),
        Err(e) => e,
    }
}

async fn list_flags(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    let status = status_filter(&q);
    let limit = q.limit.unwrap_or(50);
    match blocking(move || store::list_flags(&db, &user, status.as_deref(), limit)).await {
        Ok(flags) => (StatusCode::OK, Json(json!({ "flags": flags }))),
        Err(e) => e,
    }
}

async fn dismiss_flag(State(state): State<Arc<AppState>>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    match blocking(move || store::dismiss_flag(&db, &user, &id)).await {
        Ok(flag) => (StatusCode::OK, Json(json!({ "flag": flag }))),
        Err(e) => e,
    }
}

async fn list_proposals(State(state): State<Arc<AppState>>, headers: HeaderMap, Query(q): Query<ListQuery>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    let status = status_filter(&q);
    let limit = q.limit.unwrap_or(50);
    match blocking(move || store::list_proposals(&db, &user, status.as_deref(), limit)).await {
        Ok(proposals) => (StatusCode::OK, Json(json!({ "proposals": proposals }))),
        Err(e) => e,
    }
}

async fn create_proposal(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(req): Json<NewProposal>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    match blocking(move || store::create_proposal(&db, &user, &req)).await {
        Ok(p) => (StatusCode::OK, Json(json!({ "proposal": p }))),
        Err(e) => e,
    }
}

async fn set_eval(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(req): Json<EvalBody>,
) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    match blocking(move || store::set_proposal_eval(&db, &user, &id, &req.eval)).await {
        Ok(p) => (StatusCode::OK, Json(json!({ "proposal": p }))),
        Err(e) => e,
    }
}

async fn approve(State(state): State<Arc<AppState>>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    let brains_dir = state.config.brains_dir();
    match blocking(move || approve_proposal(&db, &brains_dir, &user, &id)).await {
        Ok(p) => (StatusCode::OK, Json(json!({ "proposal": p }))),
        Err(e) => e,
    }
}

async fn decide(state: Arc<AppState>, headers: HeaderMap, id: String, decision: Decision) -> Reply {
    let user = match user_id(&headers) { Ok(u) => u, Err(e) => return e };
    let db = state.db.clone();
    let brains_dir = state.config.brains_dir();
    let rolling_back = matches!(decision, Decision::Rollback);
    match blocking(move || {
        if rolling_back {
            if let Some(p) = store::get_proposal(&db, &user, &id)? {
                if matches!(p.status.as_str(), "approved" | "active") {
                    rollback_applied(&db, &brains_dir, &user, &p)?;
                }
            }
        }
        store::update_proposal_status(&db, &user, &id, decision)
    }).await {
        Ok(p) => (StatusCode::OK, Json(json!({ "proposal": p }))),
        Err(e) => e,
    }
}

async fn reject(State(state): State<Arc<AppState>>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Reply {
    decide(state, headers, id, Decision::Reject).await
}

async fn activated(State(state): State<Arc<AppState>>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Reply {
    decide(state, headers, id, Decision::Activated).await
}

async fn rollback(State(state): State<Arc<AppState>>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Reply {
    decide(state, headers, id, Decision::Rollback).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn call(app: &Router, method: &str, uri: &str, who: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        if let Some(who) = who {
            req = req.header("x-allternit-user-id", who);
        }
        let req = req.body(body.map_or(Body::empty(), |b| Body::from(b.to_string()))).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn knowledge_routes_flag_propose_approve_round_trip() {
        let dir = std::env::temp_dir().join(format!("allternit-knowledge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        crate::memory_drive_writer::configure_root(&st.db, &dir.join("brains")).unwrap();
        // Mounted the way main.rs mounts the v1 routes.
        let app: Router = Router::new().nest("/api/v1", knowledge_router()).with_state(st.clone());

        // Signed out is refused.
        assert_eq!(call(&app, "GET", "/api/v1/knowledge/flags", None, None).await.0, StatusCode::UNAUTHORIZED);

        let (s, v) = call(&app, "POST", "/api/v1/knowledge/flags", Some("user-a"), Some(json!({
            "kind": "missing", "target_kind": "data_access", "target_ref": "github",
            "evidence": "No guide for listing PRs.", "session_id": "ses_9", "turn_id": "t_3"
        }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let flag_id = v["flag"]["id"].as_str().unwrap().to_string();
        assert_eq!((v["flag"]["status"].clone(), v["flag"]["session_id"].clone()), (json!("open"), json!("ses_9")));

        // A bad kind is a 400 and writes nothing.
        let (s, _) = call(&app, "POST", "/api/v1/knowledge/flags", Some("user-a"), Some(json!({
            "kind": "bogus", "target_kind": "skill", "evidence": "x"
        }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        let (_, v) = call(&app, "GET", "/api/v1/knowledge/flags?status=open", Some("user-a"), None).await;
        assert_eq!(v["flags"].as_array().unwrap().len(), 1);
        let (_, v) = call(&app, "GET", "/api/v1/knowledge/flags", Some("user-b"), None).await;
        assert_eq!(v["flags"], json!([]), "user B sees none of A's flags");
        assert_eq!(call(&app, "GET", "/api/v1/knowledge/flags?status=nope", Some("user-a"), None).await.0, StatusCode::BAD_REQUEST);

        let (s, v) = call(&app, "POST", "/api/v1/knowledge/proposals", Some("user-a"), Some(json!({
            "target_kind": "data_access", "target_ref": "github", "title": "GitHub guide",
            "body": "# GitHub\n`gh pr list`", "evidence": [{ "flag_id": flag_id }], "origin": "curator"
        }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let pid = v["proposal"]["id"].as_str().unwrap().to_string();
        assert_eq!(v["proposal"]["evidence"], json!([{ "flag_id": flag_id }]));
        assert_eq!(v["proposal"]["status"], json!("proposed"));

        // Skill growth attaches its evaluation after proposing.
        let eval_uri = format!("/api/v1/knowledge/proposals/{pid}/eval");
        let (s, v) = call(&app, "POST", &eval_uri, Some("user-a"), Some(json!({ "eval": { "score": 0.9, "report": "ok" } }))).await;
        assert_eq!((s, v["proposal"]["eval"]["score"].clone()), (StatusCode::OK, json!(0.9)), "{v}");
        assert_eq!(call(&app, "POST", &eval_uri, Some("user-b"), Some(json!({ "eval": {} }))).await.0, StatusCode::NOT_FOUND);

        // User B can neither see nor approve it.
        let (_, v) = call(&app, "GET", "/api/v1/knowledge/proposals", Some("user-b"), None).await;
        assert_eq!(v["proposals"], json!([]));
        let approve = format!("/api/v1/knowledge/proposals/{pid}/approve");
        assert_eq!(call(&app, "POST", &approve, Some("user-b"), None).await.0, StatusCode::NOT_FOUND);

        let (s, v) = call(&app, "POST", &approve, Some("user-a"), None).await;
        assert_eq!((s, v["proposal"]["status"].clone()), (StatusCode::OK, json!("approved")), "{v}");
        let (_, v) = call(&app, "GET", "/api/v1/knowledge/flags?status=resolved", Some("user-a"), None).await;
        assert_eq!(v["flags"][0]["id"], json!(flag_id));
        assert_eq!(v["flags"][0]["proposal_id"], json!(pid));
        // A second approve is a conflict.
        assert_eq!(call(&app, "POST", &approve, Some("user-a"), None).await.0, StatusCode::CONFLICT);

        let (s, v) = call(&app, "POST", &format!("/api/v1/knowledge/proposals/{pid}/rollback"), Some("user-a"), None).await;
        assert_eq!((s, v["proposal"]["status"].clone()), (StatusCode::OK, json!("rolled_back")));
        let stored: String = st.db.connect().unwrap()
            .query_row("SELECT status FROM knowledge_proposals WHERE id = ?1", [&pid], |r| r.get(0)).unwrap();
        assert_eq!(stored, "rolled_back");
    }

    // ── Step 16: approve writes the file into the Memory Drive ──────────────

    async fn setup() -> (Arc<AppState>, Router) {
        let dir = std::env::temp_dir().join(format!("allternit-knowledge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        crate::memory_drive_writer::configure_root(&st.db, &dir.join("brains")).unwrap();
        let app: Router = Router::new().nest("/api/v1", knowledge_router()).with_state(st.clone());
        (st, app)
    }

    async fn propose(app: &Router, kind: &str, r: &str, body: &str, base: Option<&str>) -> String {
        let (s, v) = call(app, "POST", "/api/v1/knowledge/proposals", Some("user-a"), Some(json!({
            "target_kind": kind, "target_ref": r, "title": "GitHub guide", "body": body,
            "base": base, "evidence": [], "origin": "curator"
        }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["proposal"]["id"].as_str().unwrap().to_string()
    }

    fn drive_file(st: &Arc<AppState>, path: &str) -> Option<String> {
        let snap = crate::memory_drive_service::read(&st.db, "user-a", None).unwrap();
        snap.files.get(path).cloned()
    }

    fn history(st: &Arc<AppState>) -> Vec<String> {
        let drive = crate::memory_drive_service::resolve(&st.db, "user-a").unwrap();
        drive.storage().unwrap().history(None, 20).unwrap().into_iter().map(|c| c.message).collect()
    }

    fn approve_uri(id: &str, verb: &str) -> String { format!("/api/v1/knowledge/proposals/{id}/{verb}") }

    #[tokio::test]
    async fn approving_a_data_access_proposal_commits_the_file_to_the_drive() {
        let (st, app) = setup().await;
        let id = propose(&app, "data_access", "github", "# GitHub\n- `gh pr list` lists PRs\nUse `--state open`.", None).await;
        let (s, v) = call(&app, "POST", &approve_uri(&id, "approve"), Some("user-a"), None).await;
        assert_eq!((s, v["proposal"]["status"].clone()), (StatusCode::OK, json!("approved")), "{v}");

        let file = drive_file(&st, "knowledge/data-access/github.md").expect("file written");
        assert!(file.starts_with("# GitHub\n"), "{file}");
        assert!(file.contains("- `gh pr list` lists PRs [source: knowledge:proposal/"), "{file}");
        assert!(file.contains("kind: data_access"), "{file}");
        assert_eq!(stored_lines(&file), vec!["# GitHub", "`gh pr list` lists PRs", "Use `--state open`."]);
        let index = drive_file(&st, "MEMORY.md").unwrap();
        assert!(index.contains("[[knowledge/data-access/github]]"), "{index}");
        let msgs = history(&st);
        assert_eq!(msgs.iter().filter(|m| m.starts_with("Knowledge:")).count(), 1, "{msgs:?}");
        assert_eq!(msgs[0], "Knowledge: GitHub guide (approved)");

        // A domain proposal goes to its own folder.
        let d = propose(&app, "domain", "billing", "Invoices are monthly.", None).await;
        assert_eq!(call(&app, "POST", &approve_uri(&d, "approve"), Some("user-a"), None).await.0, StatusCode::OK);
        assert!(drive_file(&st, "knowledge/domain/billing.md").is_some());
    }

    #[tokio::test]
    async fn rejecting_a_proposal_writes_nothing() {
        let (st, app) = setup().await;
        let id = propose(&app, "domain", "billing", "Invoices are monthly.", None).await;
        let (s, _) = call(&app, "POST", &approve_uri(&id, "reject"), Some("user-a"), None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(crate::memory_drive_service::resolve(&st.db, "user-a").is_err()
            || drive_file(&st, "knowledge/domain/billing.md").is_none());
        assert!(!history(&st).iter().any(|m| m.starts_with("Knowledge:")));
    }

    #[tokio::test]
    async fn a_stale_base_is_a_409_and_the_file_is_untouched() {
        let (st, app) = setup().await;
        let first = propose(&app, "domain", "billing", "Invoices are monthly.", None).await;
        assert_eq!(call(&app, "POST", &approve_uri(&first, "approve"), Some("user-a"), None).await.0, StatusCode::OK);
        let before = drive_file(&st, "knowledge/domain/billing.md").unwrap();

        // The proposal was written against different text than the file now holds.
        let stale = propose(&app, "domain", "billing", "Invoices are weekly.", Some("Invoices are yearly.")).await;
        let (s, v) = call(&app, "POST", &approve_uri(&stale, "approve"), Some("user-a"), None).await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(drive_file(&st, "knowledge/domain/billing.md").unwrap(), before);
        let (_, v) = call(&app, "GET", "/api/v1/knowledge/proposals?status=proposed", Some("user-a"), None).await;
        assert_eq!(v["proposals"].as_array().unwrap().len(), 1, "the proposal stays proposed");

        // A matching base goes through.
        let ok = propose(&app, "domain", "billing", "Invoices are weekly.", Some("Invoices are monthly.")).await;
        assert_eq!(call(&app, "POST", &approve_uri(&ok, "approve"), Some("user-a"), None).await.0, StatusCode::OK);
        assert_eq!(stored_lines(&drive_file(&st, "knowledge/domain/billing.md").unwrap()), vec!["Invoices are weekly."]);
    }

    #[tokio::test]
    async fn rollback_restores_the_previous_text_or_removes_the_file() {
        let (st, app) = setup().await;
        let first = propose(&app, "domain", "billing", "Invoices are monthly.", None).await;
        call(&app, "POST", &approve_uri(&first, "approve"), Some("user-a"), None).await;
        let second = propose(&app, "domain", "billing", "Invoices are weekly.", Some("Invoices are monthly.")).await;
        call(&app, "POST", &approve_uri(&second, "approve"), Some("user-a"), None).await;

        let (s, v) = call(&app, "POST", &approve_uri(&second, "rollback"), Some("user-a"), None).await;
        assert_eq!((s, v["proposal"]["status"].clone()), (StatusCode::OK, json!("rolled_back")), "{v}");
        assert_eq!(stored_lines(&drive_file(&st, "knowledge/domain/billing.md").unwrap()), vec!["Invoices are monthly."]);
        assert_eq!(history(&st)[0], "Knowledge: GitHub guide (rolled back)");

        // No base: rollback removes the file.
        let (s, _) = call(&app, "POST", &approve_uri(&first, "rollback"), Some("user-a"), None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(drive_file(&st, "knowledge/domain/billing.md").is_none());
    }

    #[tokio::test]
    async fn a_secret_in_the_body_is_refused() {
        let (st, app) = setup().await;
        let id = propose(&app, "data_access", "github", "Use token: ghp_abcdefgh12345678 for the API.", None).await;
        let (s, _) = call(&app, "POST", &approve_uri(&id, "approve"), Some("user-a"), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(crate::memory_drive_service::resolve(&st.db, "user-a").is_err()
            || drive_file(&st, "knowledge/data-access/github.md").is_none());
        let (_, v) = call(&app, "GET", "/api/v1/knowledge/proposals?status=proposed", Some("user-a"), None).await;
        assert_eq!(v["proposals"].as_array().unwrap().len(), 1);
    }
}
