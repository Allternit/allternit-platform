//! Past chats (Primitives Wave D, D1 recall): gizzi's `thread_read`,
//! `thread_recent` and `thread_wait` tools.
//!
//! Unlike the bot thread tools (`thread_bot_tools`), these work in every
//! session the caller owns: a bot's chat, a plain chat, a Gizzi Code session.
//! Whose session it is comes from the session id
//! ([`agent_user_tools::resolve_owner`]), never from the model. Incognito
//! chats are never returned: a thread marked incognito, or a session in
//! `ephemeral_sessions`, answers 404 exactly like a missing one. Transcripts
//! stay in gizzi's local session store; this API only maps a thread to its
//! session and says where it lives.
//!
//! * `POST /bot-routing/thread/resolve` `{sessionId, thread}` (`thread` is a
//!   bot thread id, a session id, or `"current"`) → `{thread: {id, title,
//!   status, sessionId, local, computer?, summary?, lastActivityAt?}}`.
//!   `local` is this API's view (the session was opened through this
//!   runtime); gizzi still checks its own store before reading.
//! * `POST /bot-routing/thread/recent` `{sessionId, limit ≤20, before?, after?, project?}`
//!   → `{threads: [...]}`, newest first (the `thread_search` row shape).
//! * `POST /bot-routing/thread/wait` `{sessionId, targets ≤8 [{thread, afterCursor?}], timeoutS 0-600}`
//!   → `{woke, timedOut, statuses}`: the first target that finished a turn or
//!   now needs the user, read from `bot_events`. A cursor is the `seq` of the
//!   last event already seen; events at or before it never wake the caller
//!   again. With no cursor, a thread already waiting on the user wakes at
//!   once and any other only on a new event. `timeoutS` 0 is a snapshot.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_user_tools::{resolve_owner, Owner, Refusal};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::thread_routes;
use crate::AppState;

// ---- limits (one place) ----

pub const MAX_RECENT: usize = 20;
pub const DEFAULT_RECENT: usize = 10;
pub const MAX_WAIT_TARGETS: usize = 8;
pub const MAX_WAIT_SECONDS: u64 = 600;
/// How often a long wait re-reads the ledger.
const POLL_EVERY: Duration = Duration::from_millis(500);

/// Ledger types that mean "the user is needed now".
const NEEDS_USER_EVENTS: &[&str] = &["thread.needs_user", "thread.blocked", "approval.requested", "task.waiting_for_input", "task.waiting_for_approval"];
/// Ledger types that mean "a turn (or the run) ended".
const TURN_FINISHED_EVENTS: &[&str] = &[
    "thread.completed",
    "thread.idle",
    "thread.failed",
    "thread.declined",
    "turn.completed",
    "turn.failed",
    "run.completed",
    "run.failed",
    "run.ended",
    "routine.completed",
    "routine.failed",
];

pub fn thread_read_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/thread/resolve", post(resolve_route))
        .route("/bot-routing/thread/recent", post(recent_route))
        .route("/bot-routing/thread/wait", post(wait_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveBody {
    pub session_id: String,
    pub thread: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentBody {
    pub session_id: String,
    pub limit: Option<usize>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub project: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitTarget {
    pub thread: String,
    pub after_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitBody {
    pub session_id: String,
    pub targets: Vec<WaitTarget>,
    pub timeout_s: Option<u64>,
}

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into(), "code": code }))).into_response()
}

fn refusal((status, code, msg): Refusal) -> Response {
    err(status, code, msg)
}

/// Missing, someone else's and incognito all answer the same.
fn not_found() -> Refusal {
    (StatusCode::NOT_FOUND, "not_found", "No chat with that id.".into())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, Refusal> + Send + 'static) -> Result<T, Response> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(r)) => Err(refusal(r)),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, "failed", e.to_string())),
    }
}

fn db_err(e: rusqlite::Error) -> Refusal {
    (StatusCode::INTERNAL_SERVER_ERROR, "failed", e.to_string())
}

fn owner_of(db: &DbHandle, user_id: &str, session_id: &str) -> Result<Owner, Refusal> {
    let paired = crate::relay_auth::process_secret().paired_owner();
    resolve_owner(db, user_id, session_id, paired.as_deref())
}

// ---- resolve ----

/// Where one thread's transcript lives, from the caller's point of view.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub id: String,
    pub bot_id: Option<String>,
    pub title: String,
    pub status: String,
    pub status_line: Option<String>,
    pub session_id: Option<String>,
    pub local: bool,
    pub computer: Option<String>,
    pub summary: Option<String>,
    pub last_activity_at: Option<String>,
}

impl Resolved {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "status": self.status,
            "sessionId": self.session_id,
            "local": self.local,
            "computer": self.computer,
            "summary": self.summary,
            "lastActivityAt": self.last_activity_at,
        })
    }
}

fn is_ephemeral(db: &DbHandle, session_id: &str) -> bool {
    // Fail closed: if the flag can't be read, treat the chat as incognito.
    db.is_session_ephemeral(session_id).unwrap_or(true)
}

/// The session's metadata says it was opened through this runtime, and on
/// which computer (when a client recorded one).
fn session_place(db: &DbHandle, session_id: &str) -> (bool, Option<String>) {
    match db.get_session_metadata(session_id).ok().flatten() {
        Some(bag) => {
            let computer = ["computerName", "computer", "deviceName"]
                .iter()
                .find_map(|k| bag[*k].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string));
            (true, computer)
        }
        None => (false, None),
    }
}

/// A bot thread the owner may read: never incognito, never someone else's.
fn owned_thread(db: &DbHandle, owner: &str, id: &str) -> Result<Option<thread_routes::ThreadView>, Refusal> {
    let t = thread_routes::load_view(db, id).map_err(db_err)?;
    Ok(t.filter(|t| !t.incognito && thread_user(db, &t.id).as_deref() == Some(owner)))
}

fn thread_user(db: &DbHandle, id: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    conn.query_row("SELECT user_id FROM bot_threads WHERE id = ?1", params![id], |r| r.get(0)).optional().ok().flatten()
}

fn from_view(db: &DbHandle, t: &thread_routes::ThreadView, session: Option<String>) -> Resolved {
    let session_id = session.or_else(|| t.current_session_id.clone()).or_else(|| latest_generation(db, &t.id));
    let (local, computer) = session_id.as_deref().map(|s| session_place(db, s)).unwrap_or((false, None));
    Resolved {
        id: t.id.clone(),
        bot_id: Some(t.bot_id.clone()),
        title: t.title.clone(),
        status: t.status.clone(),
        status_line: t.status_line.clone(),
        session_id,
        local,
        computer,
        summary: t.summary.clone(),
        last_activity_at: Some(t.last_activity_at.clone()),
    }
}

fn latest_generation(db: &DbHandle, thread_id: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    conn.query_row(
        "SELECT session_id FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1",
        params![thread_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Map `thread` (a bot thread id, a session id, or `"current"`) to the
/// session that holds its transcript. `caller` is the calling session and its
/// already-checked owner.
pub fn resolve(db: &DbHandle, caller_session: &str, caller: &Owner, thread: &str, paired_owner: Option<&str>) -> Result<Resolved, Refusal> {
    let thread = thread.trim();
    if thread.is_empty() || thread.len() > 200 {
        return Err((StatusCode::BAD_REQUEST, "bad_thread", "thread must be an id or \"current\"".into()));
    }
    if thread == "current" || thread == caller_session {
        if let Some(tid) = caller.bot.as_ref().and_then(|b| b.thread_id.clone()) {
            if let Some(t) = thread_routes::load_view(db, &tid).map_err(db_err)? {
                // The caller's own chat, incognito or not: it is already in its context.
                return Ok(from_view(db, &t, Some(caller_session.to_string())));
            }
        }
        let (_, computer) = session_place(db, caller_session);
        return Ok(Resolved {
            id: caller_session.to_string(),
            bot_id: None,
            title: "This chat".into(),
            status: "active".into(),
            status_line: None,
            session_id: Some(caller_session.to_string()),
            local: true,
            computer,
            summary: None,
            last_activity_at: None,
        });
    }
    // A bot thread by id.
    if let Some(t) = owned_thread(db, &caller.user_id, thread)? {
        return Ok(from_view(db, &t, None));
    }
    // A session that is one generation of a bot thread.
    let conn = db.connect().map_err(db_err)?;
    let gen_thread: Option<String> = conn
        .query_row("SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1", params![thread], |r| r.get(0))
        .optional()
        .map_err(db_err)?;
    drop(conn);
    if let Some(tid) = gen_thread {
        if is_ephemeral(db, thread) {
            return Err(not_found());
        }
        return match owned_thread(db, &caller.user_id, &tid)? {
            Some(t) => Ok(from_view(db, &t, Some(thread.to_string()))),
            None => Err(not_found()),
        };
    }
    // Any other session: a plain chat or a Gizzi Code session on this runtime.
    if !thread.starts_with("ses") || is_ephemeral(db, thread) {
        return Err(not_found());
    }
    let owner = resolve_owner(db, &caller.user_id, thread, paired_owner).map_err(|_| not_found())?;
    if owner.user_id != caller.user_id {
        return Err(not_found());
    }
    let (_, computer) = session_place(db, thread);
    let title = db
        .get_session_metadata(thread)
        .ok()
        .flatten()
        .and_then(|b| b["title"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Chat".into());
    Ok(Resolved {
        id: thread.to_string(),
        bot_id: None,
        title,
        status: "unknown".into(),
        status_line: None,
        session_id: Some(thread.to_string()),
        // Sessions that aren't bot threads live in the store of the runtime
        // this API serves; gizzi confirms before reading.
        local: true,
        computer,
        summary: None,
        last_activity_at: None,
    })
}

async fn resolve_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<ResolveBody>) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    match blocking(move || {
        let paired = crate::relay_auth::process_secret().paired_owner();
        let owner = resolve_owner(&db, &uid, &body.session_id, paired.as_deref())?;
        resolve(&db, &body.session_id, &owner, &body.thread, paired.as_deref())
    })
    .await
    {
        Ok(r) => Json(json!({ "thread": r.to_json() })).into_response(),
        Err(r) => r,
    }
}

// ---- recent ----

fn parse_time(s: Option<&str>, field: &str) -> Result<Option<String>, Refusal> {
    match s.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => chrono::DateTime::parse_from_rfc3339(v)
            .map(|t| Some(t.with_timezone(&chrono::Utc).to_rfc3339()))
            .map_err(|_| (StatusCode::BAD_REQUEST, "bad_time", format!("{field} must be an RFC 3339 time"))),
    }
}

/// The owner's most recent threads, newest first. Incognito threads never appear.
pub fn recent(db: &DbHandle, owner: &str, body: &RecentBody) -> Result<Vec<Value>, Refusal> {
    let limit = body.limit.unwrap_or(DEFAULT_RECENT).clamp(1, MAX_RECENT);
    let before = parse_time(body.before.as_deref(), "before")?;
    let after = parse_time(body.after.as_deref(), "after")?;
    let project = body.project.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_string);
    let conn = db.connect().map_err(db_err)?;
    let mut stmt = conn
        .prepare(
            "SELECT t.id, t.bot_id, COALESCE(json_extract(a.config, '$.botProfile.displayName'), a.name), t.title, t.objective,
                    t.status, t.summary, t.status_line, t.archived_at, t.current_session_id, t.last_activity_at, t.kind, t.project_id
             FROM bot_threads t LEFT JOIN agents a ON a.id = t.bot_id
             WHERE t.user_id = ?1 AND t.incognito = 0
               AND (?2 IS NULL OR julianday(t.last_activity_at) < julianday(?2))
               AND (?3 IS NULL OR julianday(t.last_activity_at) > julianday(?3))
               AND (?4 IS NULL OR t.project_id = ?4)
             ORDER BY julianday(t.last_activity_at) DESC, t.id DESC LIMIT ?5",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![owner, before, after, project, limit as i64], |r| {
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
                "projectId": r.get::<_, Option<String>>(12)?,
            }))
        })
        .map_err(db_err)?;
    rows.collect::<rusqlite::Result<Vec<Value>>>().map_err(db_err)
}

async fn recent_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<RecentBody>) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    match blocking(move || {
        let owner = owner_of(&db, &uid, &body.session_id)?;
        recent(&db, &owner.user_id, &body)
    })
    .await
    {
        Ok(threads) => Json(json!({ "threads": threads })).into_response(),
        Err(r) => r,
    }
}

// ---- wait ----

/// One watched thread, resolved and owner-checked.
#[derive(Debug, Clone)]
pub struct Watched {
    pub thread_id: String,
    pub bot_id: String,
    /// Events at or before this `seq` never wake the caller.
    pub after: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wake {
    pub index: usize,
    pub seq: i64,
    pub reason: &'static str,
    pub occurred_at: String,
}

fn reason_for(event_type: &str) -> Option<&'static str> {
    if NEEDS_USER_EVENTS.contains(&event_type) {
        Some("needs_user")
    } else if TURN_FINISHED_EVENTS.contains(&event_type) {
        Some("turn_finished")
    } else {
        None
    }
}

fn parse_cursor(c: Option<&str>) -> Result<Option<i64>, Refusal> {
    match c.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => v
            .parse::<i64>()
            .map(Some)
            .map_err(|_| (StatusCode::BAD_REQUEST, "bad_cursor", "afterCursor must be a cursor from an earlier thread_wait".into())),
    }
}

fn max_seq(conn: &rusqlite::Connection, w: &Watched) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM bot_events WHERE bot_id = ?1 AND thread_id = ?2",
        params![w.bot_id, w.thread_id],
        |r| r.get(0),
    )
}

/// Resolve each target to a bot thread the owner may watch. Targets without a
/// cursor start at the ledger's current end, so only new events wake them;
/// the returned flags mark the ones already waiting on the user.
pub fn watch(db: &DbHandle, caller_session: &str, caller: &Owner, targets: &[WaitTarget], paired_owner: Option<&str>) -> Result<Vec<(Watched, bool)>, Refusal> {
    if targets.is_empty() || targets.len() > MAX_WAIT_TARGETS {
        return Err((StatusCode::BAD_REQUEST, "bad_targets", format!("give 1 to {MAX_WAIT_TARGETS} targets")));
    }
    let mut out = Vec::with_capacity(targets.len());
    for t in targets {
        let r = resolve(db, caller_session, caller, &t.thread, paired_owner)?;
        let Some(bot_id) = r.bot_id.clone() else {
            return Err((StatusCode::BAD_REQUEST, "not_watchable", format!("{} isn't a bot thread, so there is nothing to wait for", t.thread)));
        };
        let cursor = parse_cursor(t.after_cursor.as_deref())?;
        let mut w = Watched { thread_id: r.id.clone(), bot_id, after: cursor };
        let waiting_now = cursor.is_none() && thread_routes::group_of(&r.status) == "waiting";
        if cursor.is_none() {
            let conn = db.connect().map_err(db_err)?;
            w.after = Some(max_seq(&conn, &w).map_err(db_err)?);
        }
        out.push((w, waiting_now));
    }
    Ok(out)
}

/// The earliest wake event after each target's cursor, across all targets.
pub fn first_wake(db: &DbHandle, watched: &[Watched]) -> rusqlite::Result<Option<Wake>> {
    let conn = db.connect()?;
    let types: Vec<&str> = NEEDS_USER_EVENTS.iter().chain(TURN_FINISHED_EVENTS).copied().collect();
    let placeholders = (0..types.len()).map(|i| format!("?{}", i + 4)).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT seq, event_type, occurred_at FROM bot_events
         WHERE bot_id = ?1 AND thread_id = ?2 AND seq > ?3 AND event_type IN ({placeholders})
         ORDER BY seq ASC LIMIT 1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut best: Option<Wake> = None;
    for (index, w) in watched.iter().enumerate() {
        use rusqlite::types::Value as Sql;
        let mut args: Vec<Sql> = vec![Sql::Text(w.bot_id.clone()), Sql::Text(w.thread_id.clone()), Sql::Integer(w.after.unwrap_or(0))];
        args.extend(types.iter().map(|t| Sql::Text(t.to_string())));
        let hit: Option<(i64, String, String)> = stmt
            .query_row(rusqlite::params_from_iter(args.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        if let Some((seq, ty, at)) = hit {
            let Some(reason) = reason_for(&ty) else { continue };
            let earlier = best.as_ref().map(|b| at < b.occurred_at).unwrap_or(true);
            if earlier {
                best = Some(Wake { index, seq, reason, occurred_at: at });
            }
        }
    }
    Ok(best)
}

/// Each target's latest status line, with the cursor to pass back.
pub fn statuses(db: &DbHandle, watched: &[Watched], wake: Option<&Wake>) -> rusqlite::Result<Vec<Value>> {
    let conn = db.connect()?;
    let mut out = Vec::with_capacity(watched.len());
    for (i, w) in watched.iter().enumerate() {
        let row: Option<(String, String, Option<String>)> = conn
            .query_row("SELECT title, status, status_line FROM bot_threads WHERE id = ?1", params![w.thread_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        let (title, status, status_line) = row.unwrap_or_else(|| ("(gone)".into(), "unknown".into(), None));
        // The woken thread's cursor stops at its waking event, so a later one
        // still wakes the next call; the rest move to the ledger's end.
        let cursor = match wake {
            Some(k) if k.index == i => k.seq,
            _ => max_seq(&conn, w)?.max(w.after.unwrap_or(0)),
        };
        out.push(json!({
            "thread": w.thread_id,
            "title": title,
            "status": status,
            "statusLine": status_line,
            "cursor": cursor.to_string(),
        }));
    }
    Ok(out)
}

/// Wait up to `timeout` for the first target to wake (0 = one look).
pub async fn wait(db: DbHandle, watched: Vec<(Watched, bool)>, timeout: Duration) -> Result<Value, Refusal> {
    let deadline = Instant::now() + timeout;
    let already = watched.iter().position(|(_, waiting)| *waiting);
    let watched: Vec<Watched> = watched.into_iter().map(|(w, _)| w).collect();
    let mut wake: Option<Wake> = None;
    if let Some(index) = already {
        let w = watched[index].clone();
        let seq = {
            let d = db.clone();
            tokio::task::spawn_blocking(move || d.connect().and_then(|c| max_seq(&c, &w)))
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, "failed", e.to_string()))?
                .map_err(db_err)?
        };
        wake = Some(Wake { index, seq, reason: "needs_user", occurred_at: String::new() });
    }
    while wake.is_none() {
        let (d, ws) = (db.clone(), watched.clone());
        wake = tokio::task::spawn_blocking(move || first_wake(&d, &ws))
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, "failed", e.to_string()))?
            .map_err(db_err)?;
        if wake.is_some() || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL_EVERY.min(deadline.saturating_duration_since(Instant::now()))).await;
    }
    let (d, ws, k) = (db.clone(), watched.clone(), wake.clone());
    let lines = tokio::task::spawn_blocking(move || statuses(&d, &ws, k.as_ref()))
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, "failed", e.to_string()))?
        .map_err(db_err)?;
    let woke = wake.as_ref().map(|k| {
        let mut v = lines[k.index].clone();
        v["reason"] = json!(k.reason);
        v
    });
    Ok(json!({ "woke": woke, "timedOut": woke.is_none(), "statuses": lines }))
}

async fn wait_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<WaitBody>) -> Response {
    let timeout = Duration::from_secs(body.timeout_s.unwrap_or(0).min(MAX_WAIT_SECONDS));
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let watched = match blocking(move || {
        let paired = crate::relay_auth::process_secret().paired_owner();
        let owner = resolve_owner(&db, &uid, &body.session_id, paired.as_deref())?;
        watch(&db, &body.session_id, &owner, &body.targets, paired.as_deref())
    })
    .await
    {
        Ok(w) => w,
        Err(r) => return r,
    };
    match wait(state.db.clone(), watched, timeout).await {
        Ok(v) => Json(v).into_response(),
        Err(r) => refusal(r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-thread-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1', 'user-a', 'ledger', 'm', 'p', 1, '{}')",
            [],
        )
        .unwrap();
        state
    }

    fn thread(db: &DbHandle, id: &str, title: &str, incognito: i64, status: &str, at: &str, project: Option<&str>) {
        db.connect()
            .unwrap()
            .execute(
                "INSERT INTO bot_threads (id, user_id, bot_id, project_id, kind, incognito, title, status, created_by, last_activity_at, created_at, updated_at)
                 VALUES (?1, 'user-a', 'bot-1', ?6, 'task', ?3, ?2, ?4, 'user', ?5, ?5, ?5)",
                params![id, title, incognito, status, at, project],
            )
            .unwrap();
    }

    fn event(db: &DbHandle, thread: &str, seq: i64, ty: &str, at: &str) {
        db.connect()
            .unwrap()
            .execute(
                "INSERT INTO bot_events (id, bot_id, seq, event_type, actor_type, actor_id, payload, occurred_at, thread_id)
                 VALUES (?1, 'bot-1', ?2, ?3, 'bot', 'bot-1', '{}', ?4, ?5)",
                params![format!("ev-{seq}"), seq, ty, at, thread],
            )
            .unwrap();
    }

    fn owner() -> Owner {
        Owner { user_id: "user-a".into(), bot: None }
    }

    fn target(thread: &str, after: Option<&str>) -> WaitTarget {
        WaitTarget { thread: thread.into(), after_cursor: after.map(str::to_string) }
    }

    #[tokio::test]
    async fn recent_is_newest_first_filtered_and_skips_incognito() {
        let s = setup().await;
        thread(&s.db, "t1", "Old", 0, "done", "2026-11-01T10:00:00+00:00", Some("p1"));
        thread(&s.db, "t2", "Secret", 1, "idle", "2026-11-03T10:00:00+00:00", Some("p1"));
        thread(&s.db, "t3", "New", 0, "idle", "2026-11-05T10:00:00.250+00:00", None);
        let all = recent(&s.db, "user-a", &RecentBody { limit: Some(99), ..Default::default() }).unwrap();
        let ids: Vec<_> = all.iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["t3", "t1"]);
        let before = recent(&s.db, "user-a", &RecentBody { before: Some("2026-11-05T10:00:00Z".into()), ..Default::default() }).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0]["id"], "t1");
        let after = recent(&s.db, "user-a", &RecentBody { after: Some("2026-11-02T00:00:00Z".into()), ..Default::default() }).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["id"], "t3");
        let proj = recent(&s.db, "user-a", &RecentBody { project: Some("p1".into()), ..Default::default() }).unwrap();
        assert_eq!(proj.len(), 1);
        assert!(recent(&s.db, "user-b", &RecentBody::default()).unwrap().is_empty());
        assert_eq!(recent(&s.db, "user-a", &RecentBody { before: Some("yesterday".into()), ..Default::default() }).unwrap_err().1, "bad_time");
    }

    #[tokio::test]
    async fn resolve_hides_incognito_and_other_owners() {
        let s = setup().await;
        thread(&s.db, "t1", "Venue", 0, "idle", "2026-11-01T10:00:00+00:00", None);
        thread(&s.db, "t2", "Secret", 1, "idle", "2026-11-01T10:00:00+00:00", None);
        let r = resolve(&s.db, "ses_caller", &owner(), "t1", None).unwrap();
        assert_eq!(r.title, "Venue");
        assert_eq!(r.bot_id.as_deref(), Some("bot-1"));
        assert_eq!(resolve(&s.db, "ses_caller", &owner(), "t2", None).unwrap_err().1, "not_found");
        let other = Owner { user_id: "user-b".into(), bot: None };
        assert_eq!(resolve(&s.db, "ses_caller", &other, "t1", None).unwrap_err().1, "not_found");
        s.db.set_session_ephemeral("ses_incognito").unwrap();
        assert_eq!(resolve(&s.db, "ses_caller", &owner(), "ses_incognito", None).unwrap_err().1, "not_found");
        let cur = resolve(&s.db, "ses_caller", &owner(), "current", None).unwrap();
        assert_eq!(cur.session_id.as_deref(), Some("ses_caller"));
        assert!(cur.local);
    }

    #[tokio::test]
    async fn wait_timeout_zero_is_a_snapshot() {
        let s = setup().await;
        thread(&s.db, "t1", "Build", 0, "working", "2026-11-01T10:00:00+00:00", None);
        event(&s.db, "t1", 1, "thread.started", "2026-11-01T10:00:00+00:00");
        let w = watch(&s.db, "ses_caller", &owner(), &[target("t1", None)], None).unwrap();
        let started = Instant::now();
        let v = wait(s.db.clone(), w, Duration::ZERO).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(2), "timeout 0 returns at once, not after a wait");
        assert!(v["woke"].is_null());
        assert_eq!(v["timedOut"], true);
        assert_eq!(v["statuses"][0]["status"], "working");
        assert_eq!(v["statuses"][0]["cursor"], "1");
    }

    #[tokio::test]
    async fn wait_wakes_on_the_first_of_n_and_the_cursor_stops_redelivery() {
        let s = setup().await;
        thread(&s.db, "t1", "A", 0, "working", "2026-11-01T10:00:00+00:00", None);
        thread(&s.db, "t2", "B", 0, "working", "2026-11-01T10:00:00+00:00", None);
        thread(&s.db, "t3", "C", 0, "working", "2026-11-01T10:00:00+00:00", None);
        event(&s.db, "t1", 1, "thread.started", "2026-11-01T10:00:00+00:00");
        let w = watch(&s.db, "ses_caller", &owner(), &[target("t1", None), target("t2", None), target("t3", None)], None).unwrap();
        // t3 finishes before t2 needs the user; t1 only logs progress.
        let db = s.db.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            event(&db, "t1", 2, "run.progress", "2026-11-01T10:01:00+00:00");
            event(&db, "t3", 3, "turn.completed", "2026-11-01T10:01:01+00:00");
            event(&db, "t2", 4, "thread.needs_user", "2026-11-01T10:01:02+00:00");
        });
        let v = wait(s.db.clone(), w, Duration::from_secs(5)).await.unwrap();
        assert_eq!(v["woke"]["thread"], "t3");
        assert_eq!(v["woke"]["reason"], "turn_finished");
        assert_eq!(v["timedOut"], false);
        let cursors: Vec<String> = v["statuses"].as_array().unwrap().iter().map(|s| s["cursor"].as_str().unwrap().to_string()).collect();
        assert_eq!(cursors[2], "3");
        // Passing the cursors back: t3's event is not delivered again; t2's still wakes.
        let again = vec![target("t1", Some(&cursors[0])), target("t2", Some("0")), target("t3", Some(&cursors[2]))];
        let w = watch(&s.db, "ses_caller", &owner(), &again, None).unwrap();
        let v = wait(s.db.clone(), w, Duration::ZERO).await.unwrap();
        assert_eq!(v["woke"]["thread"], "t2");
        assert_eq!(v["woke"]["reason"], "needs_user");
        let past_all = vec![target("t2", Some("4")), target("t3", Some("3"))];
        let w = watch(&s.db, "ses_caller", &owner(), &past_all, None).unwrap();
        assert!(wait(s.db.clone(), w, Duration::ZERO).await.unwrap()["woke"].is_null());
    }

    #[tokio::test]
    async fn wait_without_cursor_wakes_at_once_for_a_waiting_thread_and_refuses_incognito() {
        let s = setup().await;
        thread(&s.db, "t1", "Ask", 0, "needs_you", "2026-11-01T10:00:00+00:00", None);
        thread(&s.db, "t2", "Secret", 1, "needs_you", "2026-11-01T10:00:00+00:00", None);
        event(&s.db, "t1", 7, "thread.needs_user", "2026-11-01T10:00:00+00:00");
        let w = watch(&s.db, "ses_caller", &owner(), &[target("t1", None)], None).unwrap();
        let v = wait(s.db.clone(), w, Duration::ZERO).await.unwrap();
        assert_eq!(v["woke"]["reason"], "needs_user");
        assert_eq!(v["woke"]["cursor"], "7");
        // With that cursor, the same ask does not wake the caller again.
        let w = watch(&s.db, "ses_caller", &owner(), &[target("t1", Some("7"))], None).unwrap();
        assert!(wait(s.db.clone(), w, Duration::ZERO).await.unwrap()["woke"].is_null());
        assert_eq!(watch(&s.db, "ses_caller", &owner(), &[target("t2", None)], None).unwrap_err().1, "not_found");
        let nine: Vec<_> = (0..9).map(|_| target("t1", None)).collect();
        assert_eq!(watch(&s.db, "ses_caller", &owner(), &nine, None).unwrap_err().1, "bad_targets");
    }
}
