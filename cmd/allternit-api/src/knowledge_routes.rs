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
        .route("/knowledge/proposals/:id/approve", post(approve))
        .route("/knowledge/proposals/:id/reject", post(reject))
        .route("/knowledge/proposals/:id/activated", post(activated))
        .route("/knowledge/proposals/:id/rollback", post(rollback))
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

/// Hook run on approve, before the status changes, for `domain` and
/// `data_access` proposals. **Step 16** fills it in: write
/// `knowledge/domain/<target_ref>.md` or `knowledge/data-access/<target_ref>.md`
/// in the user's personal Memory Drive as one commit (via
/// `memory_drive_writer::root_or(db, brains_dir)` and the drive service). An
/// error here leaves the proposal `proposed` and its flags untouched.
/// In this step it does nothing and returns `Ok`.
pub fn apply_approved(_db: &DbHandle, _brains_dir: &Path, _user_id: &str, _proposal: &Proposal) -> store::Result<()> {
    Ok(())
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
    match blocking(move || store::update_proposal_status(&db, &user, &id, decision)).await {
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
}
