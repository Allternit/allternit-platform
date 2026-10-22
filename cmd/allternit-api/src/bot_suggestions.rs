//! Bot-suggested tasks: a bot proposes work, the user says go ahead.
//!
//! The caller is gizzi's `suggest_task` tool running in an app bot's chat.
//! Which bot is suggesting comes from the calling session (as with
//! `message_agent`, see `bot_routing::authorize`), never from the model.
//!
//! A suggestion is a normal task thread for that bot in status `proposed`
//! (panel group `waiting`), with `created_by = "bot"` and `origin.channel =
//! "suggestion"`. Its session exists from the start and holds the suggestion
//! as a seeded note (no model turn), so every surface can open it like any
//! other thread. Nothing runs until the user answers:
//!
//! * `POST /threads/:id/approve` — the thread goes `working` and the kickoff
//!   turn runs in its session; when the turn ends it goes `review` with the
//!   reply as its summary, and the result is posted into the chat the
//!   suggestion came from.
//! * `POST /threads/:id/decline` — the thread resolves as `declined`.
//!
//! The owner's Inbox ("Needs you") lists open suggestions with the same two
//! answers (`inbox_needs`, kind `suggestion`).
//!
//! Route (authenticated; gizzi sends the user's token or the internal
//! service token): `POST /bot-routing/suggest` `{sessionId, title, objective, why?}`.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::AuthUser;
use crate::bot_routing::{authorize, Caller, GizziRouter, RouteError, RouteRuntime};
use crate::db::DbHandle;
use crate::thread_routes::{self, CreateThreadBody, ThreadView};
use crate::AppState;

/// Open suggestions one bot may have waiting at once (a bot can't flood the user).
pub const MAX_OPEN: i64 = 3;
const SUMMARY_CAP: usize = 1200;

pub fn bot_suggestions_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/suggest", post(suggest_route))
        .route("/threads/:id/approve", post(approve_route))
        .route("/threads/:id/decline", post(decline_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestBody {
    pub session_id: String,
    pub title: String,
    pub objective: String,
    #[serde(default)]
    pub why: Option<String>,
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let cut: String = s.chars().take(cap).collect();
    format!("{}…", cut.trim_end())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn note_text(caller: &Caller, title: &str, objective: &str, why: Option<&str>) -> String {
    let mut out = format!("[Suggested by {}] {}\n\n{}", caller.name, title, objective);
    if let Some(w) = why {
        out.push_str(&format!("\n\nWhy: {w}"));
    }
    out.push_str("\n\nNothing runs until you say go ahead.");
    out
}

/// Open a suggestion for the calling bot. A suggestion with the same title
/// that is still open is returned as it is (a retried tool call adds nothing).
pub async fn suggest<R: RouteRuntime>(db: &DbHandle, rt: &R, caller: &Caller, title: &str, objective: &str, why: Option<&str>) -> Result<Value, RouteError> {
    let title = truncate(title.trim(), 80);
    let objective = objective.trim();
    let why = why.map(str::trim).filter(|w| !w.is_empty());
    if title.is_empty() || objective.is_empty() {
        return Err(RouteError::BadRequest("Give the suggestion a short title and say what you would do.".into()));
    }
    let (same, open): (Option<String>, i64) = {
        let conn = db.connect().map_err(|e| RouteError::Failed(e.to_string()))?;
        let same = conn
            .query_row(
                "SELECT id FROM bot_threads WHERE bot_id = ?1 AND user_id = ?2 AND status = 'proposed' AND title = ?3 LIMIT 1",
                params![caller.bot_id, caller.user_id, title],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| RouteError::Failed(e.to_string()))?;
        let open = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_threads WHERE bot_id = ?1 AND user_id = ?2 AND status = 'proposed'",
                params![caller.bot_id, caller.user_id],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| RouteError::Failed(e.to_string()))?;
        (same, open)
    };
    if let Some(id) = same {
        return Ok(json!({ "status": "proposed", "threadId": id, "existing": true }));
    }
    if open >= MAX_OPEN {
        return Err(RouteError::Held(format!(
            "You already have {open} suggestions waiting for your user. Wait for an answer before suggesting more."
        )));
    }
    let body: CreateThreadBody = serde_json::from_value(json!({
        "botId": caller.bot_id,
        "title": title,
        "kind": "task",
        "objective": objective,
        "status": "proposed",
        "createdBy": "bot",
        "projectId": caller.project_id,
        "parentThreadId": caller.thread_id,
        "origin": {
            "channel": "suggestion",
            "why": why,
            "fromBotId": caller.bot_id,
            "fromBotName": caller.name,
            "fromThreadId": caller.thread_id,
            "fromSessionId": caller.session_id,
        },
    }))
    .map_err(|e| RouteError::Failed(e.to_string()))?;
    let t = thread_routes::create(db, rt, &caller.user_id, body).await.map_err(RouteError::Failed)?;
    if let Some(session) = t.current_session_id.as_deref() {
        if let Err(e) = rt.seed(session, &note_text(caller, &t.title, objective, why)).await {
            tracing::warn!(error = %e, thread = %t.id, "couldn't write the suggestion into its thread");
        }
    }
    if let Ok(conn) = db.connect() {
        let _ = conn.execute("UPDATE bot_threads SET status_line = ?2 WHERE id = ?1", params![t.id, why.map(|w| truncate(w, 140))]);
    }
    thread_routes::ledger(db, &caller.bot_id, &t.id, "thread.proposed", ("bot", &caller.bot_id), json!({ "title": t.title, "why": why }), t.current_session_id.as_deref());
    Ok(json!({ "status": "proposed", "threadId": t.id, "existing": false }))
}

/// The suggestion behind `id` when `user_id` owns it and it is still open.
fn open_suggestion(db: &DbHandle, user_id: &str, id: &str) -> Result<ThreadView, (StatusCode, String)> {
    let owner: Option<String> = db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT user_id FROM bot_threads WHERE id = ?1", params![id], |r| r.get(0)).optional().ok().flatten());
    if owner.as_deref() != Some(user_id) {
        return Err((StatusCode::NOT_FOUND, "thread not found".into()));
    }
    let t = thread_routes::load_view(db, id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "thread not found".into()))?;
    if t.status != "proposed" {
        return Err((StatusCode::CONFLICT, "this suggestion has already been answered".into()));
    }
    Ok(t)
}

fn set_status(db: &DbHandle, id: &str, status: &str, line: Option<&str>, summary: Option<&str>) {
    if let Ok(conn) = db.connect() {
        let ts = now();
        let _ = conn.execute(
            "UPDATE bot_threads SET status = ?2, status_line = ?3, summary = COALESCE(?4, summary),
                 started_at = CASE WHEN ?2 = 'working' AND started_at IS NULL THEN ?5 ELSE started_at END,
                 resolved_at = CASE WHEN ?2 IN ('done', 'failed', 'declined') THEN ?5 ELSE NULL END,
                 last_activity_at = ?5, updated_at = ?5
             WHERE id = ?1",
            params![id, status, line, summary, ts],
        );
    }
}

/// The chat a suggestion came from, when it was another session than the thread's own.
fn origin_session(t: &ThreadView, db: &DbHandle) -> Option<String> {
    let origin: Option<String> = db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT origin FROM bot_threads WHERE id = ?1", params![t.id], |r| r.get(0)).optional().ok().flatten())
        .flatten();
    origin
        .and_then(|o| serde_json::from_str::<Value>(&o).ok())
        .and_then(|o| o["fromSessionId"].as_str().map(str::to_string))
        .filter(|s| Some(s.as_str()) != t.current_session_id.as_deref())
}

/// Go ahead: start the work in the suggestion's thread. Returns the thread
/// as `working`; the turn runs in the background.
pub async fn approve<R: RouteRuntime>(db: &DbHandle, rt: Arc<R>, user_id: &str, id: &str) -> Result<ThreadView, (StatusCode, String)> {
    let t = open_suggestion(db, user_id, id)?;
    let session = t.current_session_id.clone().ok_or((StatusCode::CONFLICT, "the suggestion has no session".into()))?;
    set_status(db, id, "working", Some("Working on it"), None);
    thread_routes::ledger(db, &t.bot_id, id, "thread.started", ("user", user_id), json!({ "from": "proposed", "to": "working", "approved": true }), Some(&session));
    let kickoff = format!(
        "[Your user said go ahead with your suggestion] {}\n\n{}\n\nDo the work now, then reply with the result.",
        t.title,
        t.objective.as_deref().unwrap_or(&t.title)
    );
    let report_to = origin_session(&t, db);
    let (job_db, job_id, job_bot, job_title) = (db.clone(), id.to_string(), t.bot_id.clone(), t.title.clone());
    tokio::spawn(async move {
        let out = rt.send_turn(&session, &job_bot, &kickoff).await;
        match &out {
            Ok(reply) => set_status(&job_db, &job_id, "review", Some("Done, ready for your review"), Some(&truncate(reply, SUMMARY_CAP))),
            Err(e) => set_status(&job_db, &job_id, "blocked", Some(&truncate(e, 140)), None),
        }
        if let Some(to) = report_to {
            let note = match &out {
                Ok(reply) => format!("[Done: \"{job_title}\" (thread {job_id})]\n\n{}", truncate(reply, SUMMARY_CAP)),
                Err(e) => format!("[Couldn't finish \"{job_title}\" (thread {job_id})] {e}"),
            };
            if let Err(e) = rt.report(&to, &note).await {
                tracing::warn!(error = %e, thread = %job_id, "couldn't post the suggestion's result back");
            }
        }
    });
    thread_routes::load_view(db, id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "thread not found".into()))
}

/// Not now: the suggestion resolves as `declined`.
pub fn decline(db: &DbHandle, user_id: &str, id: &str) -> Result<ThreadView, (StatusCode, String)> {
    let t = open_suggestion(db, user_id, id)?;
    set_status(db, id, "declined", Some("You said not now"), None);
    thread_routes::ledger(db, &t.bot_id, id, "thread.declined", ("user", user_id), json!({ "from": "proposed", "to": "declined" }), t.current_session_id.as_deref());
    thread_routes::load_view(db, id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "thread not found".into()))
}

// ─── Handlers ───────────────────────────────────────────────────────────────

fn answer(r: Result<ThreadView, (StatusCode, String)>) -> Response {
    match r {
        Ok(t) => Json(json!({ "thread": t })).into_response(),
        Err((status, e)) => (status, Json(json!({ "error": e }))).into_response(),
    }
}

async fn suggest_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<SuggestBody>) -> Response {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), body.session_id.clone());
    let caller = match tokio::task::spawn_blocking(move || authorize(&db, &uid, &sid)).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => return (StatusCode::NOT_FOUND, Json(json!({ "error": e.message() }))).into_response(),
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response(),
    };
    let rt = GizziRouter { db: state.db.clone() };
    match suggest(&state.db, &rt, &caller, &body.title, &body.objective, body.why.as_deref()).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            let status = match e {
                RouteError::BadRequest(_) => StatusCode::BAD_REQUEST,
                RouteError::Held(_) => StatusCode::TOO_MANY_REQUESTS,
                RouteError::NotABotChat => StatusCode::NOT_FOUND,
                _ => StatusCode::BAD_GATEWAY,
            };
            (status, Json(json!({ "error": e.message() }))).into_response()
        }
    }
}

async fn approve_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let rt = Arc::new(GizziRouter { db: state.db.clone() });
    answer(approve(&state.db, rt, &user.user_id, &id).await)
}

async fn decline_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    match tokio::task::spawn_blocking(move || decline(&db, &uid, &id)).await {
        Ok(r) => answer(r),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread_routes::ThreadRuntime;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct FakeRt {
        seeds: Mutex<Vec<(String, String)>>,
        turns: Mutex<Vec<(String, String)>>,
        reports: Mutex<Vec<(String, String)>>,
    }

    impl ThreadRuntime for FakeRt {
        async fn create_session(&self, _b: &str, _n: &str, _title: &str, _c: bool, t: &str) -> Result<String, String> {
            Ok(format!("sess-{t}"))
        }
        async fn seed(&self, s: &str, t: &str) -> Result<(), String> {
            self.seeds.lock().unwrap().push((s.into(), t.into()));
            Ok(())
        }
        async fn handoff(&self, s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Ok((format!("{s}-next"), json!({})))
        }
    }

    impl RouteRuntime for FakeRt {
        async fn send_turn(&self, s: &str, _b: &str, t: &str) -> Result<String, String> {
            self.turns.lock().unwrap().push((s.into(), t.into()));
            Ok("Drafted the three posts.".into())
        }
        async fn report(&self, s: &str, t: &str) -> Result<(), String> {
            self.reports.lock().unwrap().push((s.into(), t.into()));
            Ok(())
        }
    }

    async fn setup() -> DbHandle {
        let dir = std::env::temp_dir().join(format!("allternit-bot-suggest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('duet', 'user-a', 'duet', 'm', 'p', 1, '{\"botProfile\":{\"displayName\":\"Duet\"}}')",
                [],
            )
            .unwrap();
        state.db.set_session_metadata("s-duet", &json!({"isBot": true, "agentId": "duet", "botCanonicalFor": "duet"})).unwrap();
        state.db.clone()
    }

    #[tokio::test]
    async fn suggestion_waits_then_runs_on_go_ahead_and_reports_back() {
        let db = setup().await;
        let rt = Arc::new(FakeRt::default());
        let caller = crate::bot_routing::caller_for_session(&db, "s-duet").unwrap();
        let v = suggest(&db, rt.as_ref(), &caller, "Draft launch posts", "Write three posts for Friday's launch.", Some("Launch is Friday")).await.unwrap();
        let id = v["threadId"].as_str().unwrap().to_string();
        let t = thread_routes::load_view(&db, &id).unwrap().unwrap();
        assert_eq!((t.status.as_str(), t.group, t.created_by.as_str()), ("proposed", "waiting", "bot"));
        assert!(rt.turns.lock().unwrap().is_empty(), "nothing runs before the go-ahead");
        assert!(rt.seeds.lock().unwrap()[0].1.contains("[Suggested by Duet] Draft launch posts"));

        // A retried call returns the same suggestion.
        let again = suggest(&db, rt.as_ref(), &caller, "Draft launch posts", "same", None).await.unwrap();
        assert_eq!((again["threadId"].as_str(), again["existing"].as_bool()), (Some(id.as_str()), Some(true)));

        assert_eq!(approve(&db, rt.clone(), "user-b", &id).await.unwrap_err().0, StatusCode::NOT_FOUND, "another user can't approve it");
        let t = approve(&db, rt.clone(), "user-a", &id).await.unwrap();
        assert_eq!(t.status, "working");
        for _ in 0..50 {
            if thread_routes::load_view(&db, &id).unwrap().unwrap().status == "review" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let t = thread_routes::load_view(&db, &id).unwrap().unwrap();
        assert_eq!((t.status.as_str(), t.summary.as_deref()), ("review", Some("Drafted the three posts.")));
        assert!(rt.turns.lock().unwrap()[0].1.contains("go ahead with your suggestion"));
        assert_eq!(rt.reports.lock().unwrap()[0].0, "s-duet", "the result goes back to the chat it came from");
        assert_eq!(approve(&db, rt.clone(), "user-a", &id).await.unwrap_err().0, StatusCode::CONFLICT);
    }

    #[test]
    fn routes_merge_with_threads_and_routing_without_overlap() {
        // axum panics at startup on an overlapping route.
        let _ = thread_routes::thread_router().merge(crate::bot_routing::bot_routing_router()).merge(bot_suggestions_router());
    }

    #[tokio::test]
    async fn not_now_declines_and_open_suggestions_are_capped() {
        let db = setup().await;
        let rt = FakeRt::default();
        let caller = crate::bot_routing::caller_for_session(&db, "s-duet").unwrap();
        let mut ids = vec![];
        for i in 0..MAX_OPEN {
            ids.push(suggest(&db, &rt, &caller, &format!("Idea {i}"), "Do it", None).await.unwrap()["threadId"].as_str().unwrap().to_string());
        }
        assert!(matches!(suggest(&db, &rt, &caller, "One more", "Do it", None).await, Err(RouteError::Held(_))));
        let t = decline(&db, "user-a", &ids[0]).unwrap();
        assert_eq!((t.status.as_str(), t.group), ("declined", "resolved"));
        assert!(t.resolved_at.is_some());
        assert!(suggest(&db, &rt, &caller, "One more", "Do it", None).await.is_ok(), "declining frees a slot");
    }
}
