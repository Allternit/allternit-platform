//! A bot steering its own thread (W3): gizzi's `thread_update`,
//! `thread_close` and `thread_search` tools.
//!
//! Which thread and whose it is come from the calling session
//! (`bot_routing::authorize`, the same check `message_agent` and
//! `suggest_task` use), never from the model. Gizzi sends the user's token or
//! the internal service token.
//!
//! * `POST /bot-routing/thread/update` `{sessionId, title?, goal?, needsYou?: {reason, urgency?}}`
//!   — rename the thread / restate its goal when the owner changes it, or put
//!   it in `needs_you` with a reason (urgency `urgent` reaches the owner in
//!   quiet hours). Same write as `PATCH /threads/:id`.
//! * `POST /bot-routing/thread/close` `{sessionId, outcome: done|dropped, note?}`
//!   — `done` resolves the task thread done with the note as its summary;
//!   `dropped` resolves it `declined` and archives it. Both run the close-out
//!   hook (`thread_closeout`). Standing threads (a main chat, a routine's home)
//!   don't close.
//! * `POST /bot-routing/thread/search` `{sessionId, query, limit?}` — the
//!   owner's threads, open, resolved and archived, matched on title, goal,
//!   summary and status line. Transcripts of archived threads stay readable.

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::bot_routing::{authorize, Caller};
use crate::thread_routes::{self, PatchThreadBody, ResolveBody};
use crate::AppState;

pub fn thread_bot_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/thread/update", post(update_route))
        .route("/bot-routing/thread/close", post(close_route))
        .route("/bot-routing/thread/search", post(search_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedsYou {
    pub reason: String,
    pub urgency: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBody {
    pub session_id: String,
    pub title: Option<String>,
    pub goal: Option<String>,
    pub needs_you: Option<NeedsYou>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseBody {
    pub session_id: String,
    pub outcome: String,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBody {
    pub session_id: String,
    pub query: String,
    pub limit: Option<usize>,
}

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn owner(caller: &Caller) -> AuthUser {
    AuthUser {
        user_id: caller.user_id.clone(),
        email: None,
        name: None,
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    }
}

async fn caller(state: &AppState, user: &AuthUser, session_id: &str) -> Result<Caller, Response> {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), session_id.to_string());
    match tokio::task::spawn_blocking(move || authorize(&db, &uid, &sid)).await {
        Ok(Ok(c)) => Ok(c),
        Ok(Err(e)) => Err(err(StatusCode::NOT_FOUND, e.message())),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, e.to_string())),
    }
}

/// The calling session's thread.
async fn own_thread(state: &AppState, user: &AuthUser, session_id: &str) -> Result<(Caller, thread_routes::ThreadView), Response> {
    let c = caller(state, user, session_id).await?;
    let Some(tid) = c.thread_id.clone() else {
        return Err(err(StatusCode::NOT_FOUND, "this chat isn't a thread yet"));
    };
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || thread_routes::load_view(&db, &tid)).await {
        Ok(Ok(Some(t))) => Ok((c, t)),
        _ => Err(err(StatusCode::NOT_FOUND, "thread not found")),
    }
}

fn clean(s: Option<String>, cap: usize) -> Option<String> {
    s.map(|v| v.trim().chars().take(cap).collect::<String>()).filter(|v| !v.is_empty())
}

async fn update_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<UpdateBody>) -> Response {
    let (c, t) = match own_thread(&state, &user, &body.session_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let title = clean(body.title, 120);
    let goal = clean(body.goal, 2000);
    if title.is_some() && t.kind == "standing" {
        return err(StatusCode::CONFLICT, "this is a standing thread (a main chat or a routine's home); it keeps its name");
    }
    let (status, status_line, urgency) = match body.needs_you {
        Some(n) => {
            let reason = clean(Some(n.reason), 200);
            if reason.is_none() {
                return err(StatusCode::BAD_REQUEST, "needsYou.reason is required");
            }
            (Some("needs_you".to_string()), reason, Some(n.urgency.unwrap_or_else(|| "normal".into())))
        }
        None => (None, None, None),
    };
    if title.is_none() && goal.is_none() && status.is_none() {
        return err(StatusCode::BAD_REQUEST, "nothing to update: give title, goal or needsYou");
    }
    thread_routes::patch_thread(
        State(state),
        Extension(owner(&c)),
        Path(t.id),
        Json(PatchThreadBody { title, objective: goal, status, status_line, urgency, ..Default::default() }),
    )
    .await
}

async fn close_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<CloseBody>) -> Response {
    let (c, t) = match own_thread(&state, &user, &body.session_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if t.kind == "standing" {
        return err(StatusCode::CONFLICT, "this is a standing thread (a main chat or a routine's home); it doesn't close");
    }
    let note = clean(body.note, 2000);
    match body.outcome.as_str() {
        "done" => {
            thread_routes::resolve_thread(
                State(state),
                Extension(owner(&c)),
                Path(t.id),
                Json(ResolveBody { status: "done".into(), summary: note }),
            )
            .await
        }
        "dropped" => {
            let res = thread_routes::patch_thread(
                State(state.clone()),
                Extension(owner(&c)),
                Path(t.id.clone()),
                Json(PatchThreadBody {
                    status: Some("declined".into()),
                    summary: note.or_else(|| Some("Dropped at the owner's request.".into())),
                    ..Default::default()
                }),
            )
            .await;
            if !res.status().is_success() {
                return res;
            }
            match thread_routes::set_archived(&state.db, &t.id, true, ("bot", &c.bot_id)).await {
                Ok(t) => Json(json!({ "thread": t })).into_response(),
                Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
        }
        _ => err(StatusCode::BAD_REQUEST, "outcome must be done or dropped"),
    }
}

/// The owner's threads matching `query` (every word must match somewhere),
/// newest first, archived and resolved included. Incognito threads never match.
pub fn search(db: &crate::db::DbHandle, user_id: &str, query: &str, limit: usize) -> rusqlite::Result<Vec<Value>> {
    let conn = db.connect()?;
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| format!("%{}%", w.to_lowercase().replace('%', "").replace('_', "")))
        .filter(|w| w.len() > 2)
        .take(8)
        .collect();
    let mut sql = String::from(
        "SELECT t.id, t.bot_id, COALESCE(json_extract(a.config, '$.botProfile.displayName'), a.name), t.title, t.objective,
                t.status, t.summary, t.status_line, t.archived_at, t.current_session_id, t.last_activity_at, t.kind
         FROM bot_threads t LEFT JOIN agents a ON a.id = t.bot_id
         WHERE t.user_id = ?1 AND t.incognito = 0",
    );
    let mut args: Vec<String> = vec![user_id.to_string()];
    for w in &words {
        args.push(w.clone());
        let n = args.len();
        sql.push_str(&format!(
            " AND lower(COALESCE(t.title, '') || ' ' || COALESCE(t.objective, '') || ' ' || COALESCE(t.summary, '') || ' ' || COALESCE(t.status_line, '')) LIKE ?{n}"
        ));
    }
    sql.push_str(&format!(" ORDER BY t.last_activity_at DESC LIMIT {}", limit.clamp(1, 25)));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
        let status: String = r.get(5)?;
        let archived_at: Option<String> = r.get(8)?;
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "botId": r.get::<_, String>(1)?,
            "botName": r.get::<_, Option<String>>(2)?,
            "title": r.get::<_, String>(3)?,
            "goal": r.get::<_, Option<String>>(4)?,
            "closed": archived_at.is_some() || thread_routes::group_of(&status) == "resolved",
            "status": status,
            "summary": r.get::<_, Option<String>>(6)?,
            "statusLine": r.get::<_, Option<String>>(7)?,
            "archived": archived_at.is_some(),
            "archivedAt": archived_at,
            "currentSessionId": r.get::<_, Option<String>>(9)?,
            "lastActivityAt": r.get::<_, String>(10)?,
            "kind": r.get::<_, String>(11)?,
        }))
    })?;
    Ok(rows.filter_map(Result::ok).collect())
}

async fn search_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<SearchBody>) -> Response {
    let c = match caller(&state, &user, &body.session_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let db = state.db.clone();
    let (uid, q, limit) = (c.user_id.clone(), body.query.clone(), body.limit.unwrap_or(10));
    match tokio::task::spawn_blocking(move || search(&db, &uid, &q, limit)).await {
        Ok(Ok(threads)) => Json(json!({ "threads": threads })).into_response(),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "search failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[tokio::test]
    async fn search_finds_archived_threads_and_skips_incognito() {
        let dir = std::env::temp_dir().join(format!("allternit-thread-search-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1', 'user-a', 'ledger', 'm', 'p', 1, '{}')",
            [],
        )
        .unwrap();
        let ts = chrono::Utc::now().to_rfc3339();
        for (id, title, incognito, archived) in [("t1", "Venue booking", 0, Some(ts.clone())), ("t2", "Venue secret", 1, None), ("t3", "Taxes", 0, None)] {
            conn.execute(
                "INSERT INTO bot_threads (id, user_id, bot_id, kind, incognito, title, status, created_by, last_activity_at, created_at, updated_at, archived_at)
                 VALUES (?1, 'user-a', 'bot-1', 'task', ?3, ?2, 'done', 'user', ?4, ?4, ?4, ?5)",
                params![id, title, incognito, ts, archived],
            )
            .unwrap();
        }
        let hits = search(&state.db, "user-a", "venue", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["id"], "t1");
        assert_eq!(hits[0]["archived"], true);
        assert_eq!(hits[0]["closed"], true);
        assert!(search(&state.db, "user-b", "venue", 10).unwrap().is_empty());
    }
}
