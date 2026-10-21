//! Main-bot routing: one of a user's bots hands a task to another of the
//! SAME user's bots and gets the result back.
//!
//! The caller is gizzi's `message_agent` tool running in an app bot's chat
//! (bots live in this API's `agents` table, not in gizzi's `~/.gizzi/bots`
//! store). Which bot is calling comes from the calling session's metadata
//! (`isBot` + `botCanonicalFor` / `botThreadOf` / `agentId`), never from the
//! model's input; the target must be another bot with the same owner.
//!
//! Delivery is a normal task thread for the target bot (`created_by = "bot"`,
//! `parent_thread_id` = the caller's thread, same project), so the work shows
//! on the target bot's screen and in the project. A follow-up from the same
//! caller thread to the same bot continues that thread while it is open.
//! The kickoff turn runs at once; when it finishes inside the wait window the
//! reply goes straight back to the caller, otherwise the caller gets a
//! "started" handle and the reply is posted into the caller's session when it
//! lands.
//!
//! Routes (authenticated; gizzi sends the user's token or the internal
//! service token):
//! * `POST /bot-routing/roster`  `{sessionId}` → the caller and its teammates
//! * `POST /bot-routing/message` `{sessionId, target, message, waitSeconds?}`

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{AuthUser, INTERNAL_SERVICE_USER_ID};
use crate::db::DbHandle;
use crate::thread_routes::{self, CreateThreadBody, ThreadRuntime};
use crate::AppState;

/// How long the caller's tool call waits for the target's reply by default.
pub const DEFAULT_WAIT_SECS: u64 = 90;
/// Upper bound a caller can ask for (a tool call must not hang forever).
pub const MAX_WAIT_SECS: u64 = 300;
const SUMMARY_CAP: usize = 1200;

pub fn bot_routing_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/roster", post(roster_route))
        .route("/bot-routing/message", post(message_route))
}

// ─── Model ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RosterBot {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    pub title: Option<String>,
    pub tagline: Option<String>,
    pub description: Option<String>,
}

/// The bot whose chat is calling, and where that chat sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub bot_id: String,
    pub user_id: String,
    pub name: String,
    pub session_id: String,
    pub thread_id: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterBody {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageBody {
    pub session_id: String,
    pub target: String,
    pub message: String,
    pub wait_seconds: Option<u64>,
}

/// Refusals the tool shows the model as a plain sentence.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteError {
    /// The session isn't an app bot's chat (or isn't this user's).
    NotABotChat,
    BadRequest(String),
    /// The target bot is still working on the last hand-off from this chat.
    Busy(String),
    Failed(String),
}

impl RouteError {
    fn status(&self) -> StatusCode {
        match self {
            RouteError::NotABotChat => StatusCode::NOT_FOUND,
            RouteError::BadRequest(_) => StatusCode::BAD_REQUEST,
            RouteError::Busy(_) => StatusCode::CONFLICT,
            RouteError::Failed(_) => StatusCode::BAD_GATEWAY,
        }
    }
    pub fn message(&self) -> String {
        match self {
            RouteError::NotABotChat => "This chat isn't one of your bots' chats, so it can't hand work to other bots.".into(),
            RouteError::BadRequest(m) | RouteError::Busy(m) | RouteError::Failed(m) => m.clone(),
        }
    }
}

// ─── Caller and roster ──────────────────────────────────────────────────────

fn bot_name_and_owner(conn: &rusqlite::Connection, bot_id: &str) -> Option<(String, String)> {
    conn.query_row(
        "SELECT user_id, COALESCE(json_extract(config, '$.botProfile.displayName'), name) FROM agents
         WHERE id = ?1 AND (is_bot = 1 OR json_extract(config, '$.isBot') = 1)",
        params![bot_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()
    .ok()
    .flatten()
    .map(|(owner, name)| (name, owner))
}

/// The app bot behind `session_id`, or `None` when the session is not a
/// bot's own chat (a regular chat, a group room, a subagent).
pub fn caller_for_session(db: &DbHandle, session_id: &str) -> Option<Caller> {
    let bag = db.get_session_metadata(session_id).ok().flatten()?;
    if bag["isGroupChat"].as_bool() == Some(true) {
        return None;
    }
    let s = |k: &str| bag[k].as_str().filter(|v| !v.is_empty()).map(str::to_string);
    let bot_id = s("botCanonicalFor")
        .or_else(|| s("botThreadOf"))
        .or_else(|| if bag["isBot"].as_bool() == Some(true) { s("agentId") } else { None })?;
    let conn = db.connect().ok()?;
    let (name, user_id) = bot_name_and_owner(&conn, &bot_id)?;
    drop(conn);
    // A main chat the thread model hasn't adopted yet becomes its standing thread,
    // so the hand-off has a parent to hang under.
    let _ = thread_routes::sync_user_threads(db, &user_id);
    let conn = db.connect().ok()?;
    let thread: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT t.id, t.project_id FROM bot_thread_sessions s JOIN bot_threads t ON t.id = s.thread_id
             WHERE s.session_id = ?1 AND t.bot_id = ?2 ORDER BY s.generation DESC LIMIT 1",
            params![session_id, bot_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .ok()
        .flatten()
        .or_else(|| {
            let id = s("threadId")?;
            conn.query_row("SELECT id, project_id FROM bot_threads WHERE id = ?1 AND bot_id = ?2", params![id, bot_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .ok()
            .flatten()
        });
    let (thread_id, project_id) = match thread {
        Some((t, p)) => (Some(t), p.or_else(|| s("projectId"))),
        None => (None, s("projectId")),
    };
    Some(Caller { bot_id, user_id, name, session_id: session_id.to_string(), thread_id, project_id })
}

/// The user's bots other than `exclude`, oldest first.
pub fn roster(db: &DbHandle, user_id: &str, exclude: &str) -> Vec<RosterBot> {
    let Ok(conn) = db.connect() else { return vec![] };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, COALESCE(json_extract(config, '$.botProfile.displayName'), name),
                json_extract(config, '$.botProfile.handle'), json_extract(config, '$.botProfile.title'),
                json_extract(config, '$.botProfile.tagline'), description
         FROM agents
         WHERE user_id = ?1 AND id != ?2 AND (is_bot = 1 OR json_extract(config, '$.isBot') = 1)
         ORDER BY created_at, id",
    ) else {
        return vec![];
    };
    let blank = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let bots: Vec<RosterBot> = stmt.query_map(params![user_id, exclude], |r| {
        Ok(RosterBot {
            id: r.get(0)?,
            name: r.get(1)?,
            handle: blank(r.get(2)?),
            title: blank(r.get(3)?),
            tagline: blank(r.get(4)?),
            description: blank(r.get(5)?),
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default();
    bots
}

fn norm(s: &str) -> String {
    s.trim().trim_start_matches('@').trim().to_lowercase()
}

/// Resolve a target by id, handle or name (case-insensitive, `@` optional).
/// Unknown and ambiguous targets are errors that name the bots to pick from.
pub fn match_target<'a>(target: &str, bots: &'a [RosterBot]) -> Result<&'a RosterBot, String> {
    let want = norm(target);
    if want.is_empty() {
        return Err("Say which bot to hand this to (its name).".into());
    }
    if let Some(b) = bots.iter().find(|b| b.id == target.trim()) {
        return Ok(b);
    }
    let hits: Vec<&RosterBot> = bots
        .iter()
        .filter(|b| norm(&b.name) == want || b.handle.as_deref().map(norm).as_deref() == Some(want.as_str()))
        .collect();
    match hits.len() {
        1 => Ok(hits[0]),
        0 => {
            let known = bots.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ");
            Err(format!("There's no bot called '{}'. Your bots: {}.", target.trim(), if known.is_empty() { "(none yet)" } else { &known }))
        }
        _ => Err(format!(
            "'{}' matches more than one bot ({}). Use one of their ids: {}.",
            target.trim(),
            hits.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", "),
            hits.iter().map(|b| b.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// One line per teammate: what the model reads to pick who owns the work.
pub fn roster_text(bots: &[RosterBot]) -> String {
    if bots.is_empty() {
        return "You have no other bots yet.".into();
    }
    bots.iter()
        .map(|b| {
            let mut line = format!("- {}", b.name);
            if let Some(h) = &b.handle {
                line.push_str(&format!(" (@{h})"));
            }
            if let Some(t) = &b.title {
                line.push_str(&format!(", {t}"));
            }
            let about = [b.tagline.as_deref(), b.description.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ");
            if !about.is_empty() {
                line.push_str(&format!(": {}", truncate(&about, 200)));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let cut: String = s.chars().take(cap).collect();
    format!("{}…", cut.trim_end())
}

/// The caller, checked against the authenticated user. The internal service
/// (gizzi with the shared token) acts for whoever owns the session's bot.
pub fn authorize(db: &DbHandle, user_id: &str, session_id: &str) -> Result<Caller, RouteError> {
    let caller = caller_for_session(db, session_id).ok_or(RouteError::NotABotChat)?;
    if user_id != INTERNAL_SERVICE_USER_ID && caller.user_id != user_id {
        return Err(RouteError::NotABotChat);
    }
    Ok(caller)
}

// ─── Delivery ───────────────────────────────────────────────────────────────

/// What routing needs beyond thread creation. Production calls gizzi.
pub trait RouteRuntime: ThreadRuntime + Send + Sync + 'static {
    /// Run a turn in the target's thread session; returns its reply.
    fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> impl Future<Output = Result<String, String>> + Send;
    /// Post a late result into the caller's chat (no reply is started).
    fn report(&self, session_id: &str, text: &str) -> impl Future<Output = Result<(), String>> + Send;
}

pub struct GizziRouter {
    pub db: DbHandle,
}

impl ThreadRuntime for GizziRouter {
    async fn create_session(&self, bot_id: &str, bot_name: &str, title: &str, canonical: bool, thread_id: &str) -> Result<String, String> {
        thread_routes::GizziRuntime { db: self.db.clone() }.create_session(bot_id, bot_name, title, canonical, thread_id).await
    }
    async fn seed(&self, session_id: &str, text: &str) -> Result<(), String> {
        crate::agent_session_routes::seed_session_message(&self.db, session_id, text).await
    }
    async fn restrict(&self, session_id: &str, rules: Value) -> Result<(), String> {
        crate::agent_session_routes::restrict_session(session_id, rules).await
    }
    async fn handoff(&self, session_id: &str, reason: &str, context: &str, baton: Option<Value>) -> Result<(String, Value), String> {
        thread_routes::GizziRuntime { db: self.db.clone() }.handoff(session_id, reason, context, baton).await
    }
}

impl RouteRuntime for GizziRouter {
    async fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> Result<String, String> {
        crate::agent_session_routes::send_bot_turn(&self.db, session_id, bot_id, text).await
    }
    async fn report(&self, session_id: &str, text: &str) -> Result<(), String> {
        crate::agent_session_routes::seed_session_message(&self.db, session_id, text).await
    }
}

fn set_status(db: &DbHandle, thread_id: &str, status: &str, status_line: Option<&str>, summary: Option<&str>) {
    if let Ok(conn) = db.connect() {
        let ts = chrono::Utc::now().to_rfc3339();
        let _ = conn.execute(
            "UPDATE bot_threads SET status = ?2, status_line = COALESCE(?3, status_line), summary = COALESCE(?4, summary),
                 started_at = CASE WHEN ?2 = 'working' AND started_at IS NULL THEN ?5 ELSE started_at END,
                 last_activity_at = ?5, updated_at = ?5
             WHERE id = ?1",
            params![thread_id, status, status_line, summary, ts],
        );
    }
}

/// The conversation key: one open thread per (caller thread or chat, target).
fn channel_key(caller: &Caller, target_id: &str) -> String {
    format!("bot:{}:{}", caller.thread_id.as_deref().unwrap_or(&caller.session_id), target_id)
}

fn kickoff_text(caller: &Caller, message: &str) -> String {
    format!(
        "[handed to you by {}] {}\n\nDo the work, then reply with the result. Your reply goes back to {}.",
        caller.name,
        message.trim(),
        caller.name
    )
}

fn title_of(message: &str) -> String {
    let line = message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Handed-off task");
    truncate(line, 80)
}

/// Start (or continue) the target's thread and run the turn. Returns the
/// tool result: `status` is `done` (with `reply`), `started` (still running;
/// the reply will be posted into the caller's chat) or `failed`.
pub async fn route<R: RouteRuntime>(db: &DbHandle, rt: Arc<R>, caller: &Caller, target: &str, message: &str, wait: Duration) -> Result<Value, RouteError> {
    let text = message.trim();
    if text.is_empty() {
        return Err(RouteError::BadRequest("The message is empty. Say what the other bot should do.".into()));
    }
    let bots = roster(db, &caller.user_id, &caller.bot_id);
    let bot = match_target(target, &bots).map_err(RouteError::BadRequest)?.clone();
    let key = channel_key(caller, &bot.id);

    let open: Option<(String, String, Option<String>)> = db.connect().ok().and_then(|c| {
        c.query_row(
            "SELECT id, status, current_session_id FROM bot_threads
             WHERE bot_id = ?1 AND user_id = ?2 AND json_extract(origin, '$.channelKey') = ?3
               AND status NOT IN ('done', 'failed')
             ORDER BY updated_at DESC LIMIT 1",
            params![bot.id, caller.user_id, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()
        .flatten()
    });
    let (thread_id, session_id, resumed) = match open {
        Some((id, status, _)) if status == "working" => {
            return Err(RouteError::Busy(format!(
                "{} is still working on your last hand-off (thread {id}). Its result will come back to this chat.",
                bot.name
            )));
        }
        Some((id, _, Some(session))) => (id, session, true),
        _ => {
            let body: CreateThreadBody = serde_json::from_value(json!({
                "botId": bot.id,
                "title": title_of(text),
                "kind": "task",
                "objective": text,
                "createdBy": "bot",
                "projectId": caller.project_id,
                "parentThreadId": caller.thread_id,
                "origin": {
                    "channel": "bot",
                    "channelKey": key,
                    "fromBotId": caller.bot_id,
                    "fromBotName": caller.name,
                    "fromThreadId": caller.thread_id,
                    "fromSessionId": caller.session_id,
                },
            }))
            .map_err(|e| RouteError::Failed(e.to_string()))?;
            let t = thread_routes::create(db, rt.as_ref(), &caller.user_id, body).await.map_err(RouteError::Failed)?;
            let session = t.current_session_id.clone().ok_or_else(|| RouteError::Failed("the thread has no session".into()))?;
            (t.id, session, false)
        }
    };

    set_status(db, &thread_id, "working", Some(&format!("Working on it for {}", caller.name)), None);
    let kickoff = kickoff_text(caller, text);
    let (job_db, job_rt, job_caller, job_bot, job_thread, job_session) =
        (db.clone(), rt.clone(), caller.clone(), bot.clone(), thread_id.clone(), session_id.clone());
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
    tokio::spawn(async move {
        let out = job_rt.send_turn(&job_session, &job_bot.id, &kickoff).await;
        match &out {
            Ok(reply) => set_status(&job_db, &job_thread, "review", Some(&format!("Reported back to {}", job_caller.name)), Some(&truncate(reply, SUMMARY_CAP))),
            Err(e) => set_status(&job_db, &job_thread, "blocked", Some(&truncate(e, 140)), None),
        }
        // The caller stopped waiting: post the result into its chat instead.
        if let Err(out) = tx.send(out) {
            let note = match out {
                Ok(reply) => format!("[Report from {} on \"{}\" (thread {})]\n\n{}", job_bot.name, kickoff_title(&job_db, &job_thread), job_thread, reply),
                Err(e) => format!("[{} couldn't finish the hand-off (thread {})] {}", job_bot.name, job_thread, e),
            };
            if let Err(e) = job_rt.report(&job_caller.session_id, &note).await {
                tracing::warn!(error = %e, thread = %job_thread, "bot routing: couldn't post the late result to the caller");
            }
        }
    });

    let base = json!({
        "threadId": thread_id,
        "sessionId": session_id,
        "resumed": resumed,
        "bot": { "id": bot.id, "name": bot.name },
    });
    let mut out = base;
    match tokio::time::timeout(wait, rx).await {
        Ok(Ok(Ok(reply))) => {
            out["status"] = json!("done");
            out["reply"] = json!(reply);
        }
        Ok(Ok(Err(e))) => {
            out["status"] = json!("failed");
            out["error"] = json!(e);
        }
        Ok(Err(_)) => {
            out["status"] = json!("failed");
            out["error"] = json!("the turn was dropped");
        }
        Err(_) => {
            out["status"] = json!("started");
            out["note"] = json!(format!("{} is still working. The result will be posted in this chat when it's ready.", bot.name));
        }
    }
    Ok(out)
}

fn kickoff_title(db: &DbHandle, thread_id: &str) -> String {
    db.connect()
        .ok()
        .and_then(|c| c.query_row("SELECT title FROM bot_threads WHERE id = ?1", params![thread_id], |r| r.get::<_, String>(0)).ok())
        .unwrap_or_default()
}

// ─── Handlers ───────────────────────────────────────────────────────────────

fn refuse(e: RouteError) -> Response {
    (e.status(), Json(json!({ "error": e.message() }))).into_response()
}

async fn roster_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<RosterBody>) -> Response {
    let db = state.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        let caller = authorize(&db, &user.user_id, &body.session_id)?;
        let bots = roster(&db, &caller.user_id, &caller.bot_id);
        Ok::<_, RouteError>(json!({
            "caller": { "id": caller.bot_id, "name": caller.name, "threadId": caller.thread_id, "projectId": caller.project_id },
            "bots": bots,
            "text": roster_text(&bots),
        }))
    })
    .await;
    match res {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => refuse(e),
        Err(e) => refuse(RouteError::Failed(e.to_string())),
    }
}

async fn message_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<MessageBody>) -> Response {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), body.session_id.clone());
    let caller = match tokio::task::spawn_blocking(move || authorize(&db, &uid, &sid)).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => return refuse(e),
        Err(e) => return refuse(RouteError::Failed(e.to_string())),
    };
    let wait = Duration::from_secs(body.wait_seconds.unwrap_or(DEFAULT_WAIT_SECS).min(MAX_WAIT_SECS));
    let rt = Arc::new(GizziRouter { db: state.db.clone() });
    match route(&state.db, rt, &caller, &body.target, &body.message, wait).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => refuse(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRt {
        created: Mutex<Vec<String>>,
        turns: Mutex<Vec<(String, String, String)>>,
        reports: Mutex<Vec<(String, String)>>,
        /// When set, turns wait this long before replying.
        delay: Option<Duration>,
        fail: bool,
    }

    impl ThreadRuntime for FakeRt {
        async fn create_session(&self, _b: &str, _n: &str, title: &str, _c: bool, _t: &str) -> Result<String, String> {
            let mut c = self.created.lock().unwrap();
            c.push(title.to_string());
            Ok(format!("routed-sess-{}", c.len()))
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Ok((format!("{s}-next"), json!({})))
        }
    }

    impl RouteRuntime for FakeRt {
        async fn send_turn(&self, s: &str, b: &str, t: &str) -> Result<String, String> {
            self.turns.lock().unwrap().push((s.into(), b.into(), t.into()));
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            if self.fail {
                return Err("model unavailable".into());
            }
            Ok(format!("Done: {}", t.lines().next().unwrap_or_default()))
        }
        async fn report(&self, s: &str, t: &str) -> Result<(), String> {
            self.reports.lock().unwrap().push((s.into(), t.into()));
            Ok(())
        }
    }

    async fn setup() -> DbHandle {
        let dir = std::env::temp_dir().join(format!("allternit-bot-routing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let conn = state.db.connect().unwrap();
        for (id, user, name, cfg, desc) in [
            ("main", "user-a", "a", r#"{"botProfile":{"displayName":"A://","title":"Chief of staff"}}"#, "Routes work"),
            ("scout", "user-a", "scout", r#"{"botProfile":{"displayName":"Scout","handle":"scout-v1","tagline":"Finds leads"}}"#, "Researches companies"),
            ("ledger", "user-a", "ledger", r#"{"botProfile":{"displayName":"Ledger","title":"Bookkeeper"}}"#, ""),
            ("other", "user-b", "other", r#"{"botProfile":{"displayName":"Mallory"}}"#, ""),
        ] {
            conn.execute(
                "INSERT INTO agents (id, user_id, name, description, model, provider, is_bot, config) VALUES (?1, ?2, ?3, ?4, 'm', 'p', 1, ?5)",
                params![id, user, name, desc, cfg],
            )
            .unwrap();
        }
        // A plain (non-bot) agent of the same user is never on the roster.
        conn.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('plain', 'user-a', 'plain', 'm', 'p', 0, '{}')", [])
            .unwrap();
        state.db.set_session_metadata("s-main", &json!({"isBot": true, "agentId": "main", "botCanonicalFor": "main", "projectId": "proj-1"})).unwrap();
        state.db.set_session_metadata("s-plain", &json!({"sessionMode": "agent"})).unwrap();
        state.db.set_session_metadata("s-group", &json!({"isGroupChat": true, "botThreadOf": "main"})).unwrap();
        state.db.set_session_metadata("s-other", &json!({"isBot": true, "botCanonicalFor": "other"})).unwrap();
        state.db.clone()
    }

    #[tokio::test]
    async fn caller_comes_from_session_metadata_and_gets_a_parent_thread() {
        let db = setup().await;
        let c = caller_for_session(&db, "s-main").unwrap();
        assert_eq!((c.bot_id.as_str(), c.user_id.as_str(), c.name.as_str()), ("main", "user-a", "A://"));
        assert!(c.thread_id.is_some(), "the main chat is adopted as its standing thread");
        assert_eq!(c.project_id.as_deref(), Some("proj-1"));
        assert!(caller_for_session(&db, "s-plain").is_none());
        assert!(caller_for_session(&db, "s-group").is_none());
        assert!(caller_for_session(&db, "nope").is_none());
    }

    #[tokio::test]
    async fn roster_is_the_same_users_other_bots_with_their_jobs() {
        let db = setup().await;
        let bots = roster(&db, "user-a", "main");
        let mut ids = bots.iter().map(|b| b.id.as_str()).collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["ledger", "scout"], "other user's bots, plain agents and the caller are left out");
        let text = roster_text(&bots);
        assert!(text.contains("- Scout (@scout-v1): Finds leads Researches companies"), "{text}");
        assert!(text.contains("- Ledger, Bookkeeper"), "{text}");
        assert!(!text.contains("Mallory"));
    }

    #[test]
    fn targets_resolve_by_id_handle_or_name_and_ambiguity_lists_choices() {
        let bot = |id: &str, name: &str, handle: Option<&str>| RosterBot {
            id: id.into(),
            name: name.into(),
            handle: handle.map(str::to_string),
            title: None,
            tagline: None,
            description: None,
        };
        let bots = vec![bot("b1", "Scout", Some("scout-v1")), bot("b2", "Ledger", None), bot("b3", "ledger", None)];
        assert_eq!(match_target("scout", &bots).unwrap().id, "b1");
        assert_eq!(match_target("@Scout-V1", &bots).unwrap().id, "b1");
        assert_eq!(match_target("b2", &bots).unwrap().id, "b2");
        let amb = match_target("LEDGER", &bots).unwrap_err();
        assert!(amb.contains("b2") && amb.contains("b3"), "{amb}");
        let unknown = match_target("Nobody", &bots).unwrap_err();
        assert!(unknown.contains("Scout, Ledger, ledger"), "{unknown}");
    }

    #[tokio::test]
    async fn ownership_never_crosses_users() {
        let db = setup().await;
        assert!(authorize(&db, "user-a", "s-main").is_ok());
        assert!(authorize(&db, INTERNAL_SERVICE_USER_ID, "s-main").is_ok());
        assert_eq!(authorize(&db, "user-b", "s-main").unwrap_err(), RouteError::NotABotChat);
        // A's main bot can't reach user B's bot, even by id.
        let caller = authorize(&db, "user-a", "s-main").unwrap();
        let rt = Arc::new(FakeRt::default());
        let err = route(&db, rt.clone(), &caller, "other", "steal", Duration::from_secs(1)).await.unwrap_err();
        assert!(matches!(err, RouteError::BadRequest(_)), "{err:?}");
        assert!(rt.turns.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn hand_off_creates_a_child_thread_in_the_project_and_returns_the_reply() {
        let db = setup().await;
        let caller = caller_for_session(&db, "s-main").unwrap();
        let rt = Arc::new(FakeRt::default());
        let out = route(&db, rt.clone(), &caller, "Scout", "Find 3 bakeries in Saint Paul", Duration::from_secs(5)).await.unwrap();
        assert_eq!(out["status"], "done");
        assert!(out["reply"].as_str().unwrap().contains("[handed to you by A://] Find 3 bakeries"));
        let t = thread_routes::load_view(&db, out["threadId"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(t.bot_id, "scout");
        assert_eq!(t.parent_thread_id, caller.thread_id);
        assert_eq!(t.project_id.as_deref(), Some("proj-1"));
        assert_eq!(t.created_by, "bot");
        assert_eq!(t.status, "review");
        assert!(t.summary.unwrap().starts_with("Done:"));

        // A follow-up to the same bot from the same chat continues that thread.
        let again = route(&db, rt.clone(), &caller, "scout", "Now their phone numbers", Duration::from_secs(5)).await.unwrap();
        assert_eq!(again["threadId"], out["threadId"]);
        assert_eq!(again["resumed"], true);
        assert_eq!(rt.created.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn long_work_returns_a_handle_and_reports_back_later() {
        let db = setup().await;
        let caller = caller_for_session(&db, "s-main").unwrap();
        let rt = Arc::new(FakeRt { delay: Some(Duration::from_millis(300)), ..Default::default() });
        let out = route(&db, rt.clone(), &caller, "ledger", "Reconcile September", Duration::from_millis(20)).await.unwrap();
        assert_eq!(out["status"], "started");
        let busy = route(&db, rt.clone(), &caller, "ledger", "and October", Duration::from_millis(20)).await.unwrap_err();
        assert!(matches!(busy, RouteError::Busy(_)));
        tokio::time::sleep(Duration::from_millis(600)).await;
        let reports = rt.reports.lock().unwrap().clone();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].0, "s-main");
        assert!(reports[0].1.starts_with("[Report from Ledger"), "{}", reports[0].1);
    }

    #[tokio::test]
    async fn a_failed_turn_blocks_the_thread_and_says_why() {
        let db = setup().await;
        let caller = caller_for_session(&db, "s-main").unwrap();
        let rt = Arc::new(FakeRt { fail: true, ..Default::default() });
        let out = route(&db, rt, &caller, "scout", "Find leads", Duration::from_secs(5)).await.unwrap();
        assert_eq!(out["status"], "failed");
        let t = thread_routes::load_view(&db, out["threadId"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(t.status, "blocked");
    }
}
