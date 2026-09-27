//! Durable bot threads (`bot_threads`, `bot_thread_sessions`,
//! `bot_thread_deps`, `project_bots`; migration V186). Spec:
//! `docs/architecture/BOT_THREAD_PARITY_SPEC.md` P3.1–P3.4.
//!
//! A thread is a unit of work owned by a bot. It outlives any one model
//! context: each context window is a *generation* backed by one gizzi
//! session. When a generation fills up (or the user switches to a model with
//! a smaller window) the client reports usage, the server answers
//! `shouldHandoff`, and `POST /threads/:id/handoff` writes the checkpoint,
//! ends the generation, and seeds a fresh session with the checkpoint (gizzi
//! `noReply`, so no model turn). The UI draws each generation boundary as the
//! "rip" divider.
//!
//! * **Kinds.** `standing` threads are long-lived (a bot's main chat, a
//!   routine's home) and never resolve; `task` threads have an objective and
//!   resolve done/failed.
//! * **Status** uses the spec grammar (queued · planning · working · blocked ·
//!   needs_you · review · done · failed · paused · idle); the server derives
//!   the panel `group` (waiting / working / queued / idle / resolved), so no
//!   surface infers state from transcript text.
//! * **Sync.** Existing bot chats (gizzi sessions tagged `botThreadOf` /
//!   `botCanonicalFor`) become threads the first time a list is read —
//!   generation 1, no big-bang migration.
//! * **Events.** Lifecycle changes land on the bot ledger (`bot_events`) with
//!   `thread_id`: thread.created / started / blocked / needs_user /
//!   checkpointed / completed / failed / paused / resumed.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::bot_event_routes::{append_event, verify_bot_ownership, ActorBody, AppendEventBody};
use crate::db::DbHandle;
use crate::AppState;

/// Hand off to a fresh generation at this share of the model's window.
pub const HANDOFF_FRACTION: f64 = 0.70;

pub const STATUSES: &[&str] = &[
    "queued", "planning", "working", "blocked", "needs_you", "review", "done", "failed", "paused", "idle",
];

/// Panel group for a status (Threads panel sections).
pub fn group_of(status: &str) -> &'static str {
    match status {
        "needs_you" | "blocked" | "review" => "waiting",
        "working" | "planning" => "working",
        "queued" => "queued",
        "done" | "failed" => "resolved",
        _ => "idle",
    }
}

fn event_for_status(status: &str) -> &'static str {
    match status {
        "working" | "planning" => "thread.started",
        "blocked" => "thread.blocked",
        "needs_you" | "review" => "thread.needs_user",
        "done" => "thread.completed",
        "failed" => "thread.failed",
        "paused" => "thread.paused",
        "queued" => "thread.queued",
        _ => "thread.idle",
    }
}

pub fn thread_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/threads", get(list_threads).post(create_thread))
        .route("/threads/by-session/:session_id", get(thread_by_session))
        .route("/threads/:id", get(get_thread).patch(patch_thread))
        .route("/threads/:id/resolve", post(resolve_thread))
        .route("/threads/:id/usage", post(report_usage))
        .route("/threads/:id/handoff", post(handoff))
        .route("/threads/:id/deps", put(set_deps))
        .route("/threads/:id/events", get(thread_events))
        .route("/projects/:project_id/bots", get(get_team).put(put_team))
        .route("/bot-projects", get(bot_projects))
}

// ─── Wire types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TodoItem {
    pub text: String,
    /// done | active | pending
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationView {
    pub generation: i64,
    pub session_id: String,
    pub model: Option<String>,
    pub context_window: Option<i64>,
    pub tokens_used: i64,
    pub reason: String,
    pub checkpoint_summary: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub id: String,
    pub bot_id: String,
    pub project_id: Option<String>,
    pub parent_thread_id: Option<String>,
    pub kind: String,
    pub incognito: bool,
    pub title: String,
    pub objective: Option<String>,
    pub success_criteria: Option<String>,
    pub status: String,
    pub group: &'static str,
    pub status_line: Option<String>,
    pub todo: Vec<TodoItem>,
    /// `[done, total]` from the todo list — the row's k/n ring.
    pub progress: [usize; 2],
    pub summary: Option<String>,
    pub checkpoint: Option<Value>,
    pub current_session_id: Option<String>,
    pub generation: i64,
    /// Share of the current generation's context window in use (0–1).
    pub context_used: Option<f64>,
    pub depends_on: Vec<String>,
    pub created_by: String,
    pub last_activity_at: String,
    pub resolved_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateThreadBody {
    pub bot_id: String,
    pub title: String,
    pub project_id: Option<String>,
    pub parent_thread_id: Option<String>,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub incognito: bool,
    pub objective: Option<String>,
    pub success_criteria: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub todo: Vec<TodoItem>,
    pub created_by: Option<String>,
    pub origin: Option<Value>,
    /// Adopt an existing chat session as generation 1 instead of creating one.
    pub session_id: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

fn default_kind() -> String {
    "task".into()
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchThreadBody {
    pub title: Option<String>,
    pub objective: Option<String>,
    pub success_criteria: Option<String>,
    pub status: Option<String>,
    pub status_line: Option<String>,
    pub todo: Option<Vec<TodoItem>>,
    pub summary: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveBody {
    #[serde(default = "default_done")]
    pub status: String,
    pub summary: Option<String>,
}

fn default_done() -> String {
    "done".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBody {
    pub tokens_used: i64,
    pub context_window: Option<i64>,
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffBody {
    /// Checkpoint the next generation starts from. Empty → the server writes
    /// it from the current window's transcript (automatic handoff).
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub open_items: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// budget | model_switch | routine_run | manual
    #[serde(default = "default_reason")]
    pub reason: String,
    pub model: Option<String>,
    pub context_window: Option<i64>,
}

fn default_reason() -> String {
    "manual".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepsBody {
    pub depends_on: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamBody {
    pub bot_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub project_id: Option<String>,
    pub bot_id: Option<String>,
    #[serde(default)]
    pub include_resolved: bool,
    #[serde(default)]
    pub include_incognito: bool,
}

// ─── Persistence ────────────────────────────────────────────────────────────

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

const COLS: &str = "t.id, t.bot_id, t.project_id, t.parent_thread_id, t.kind, t.incognito, t.title, t.objective, \
     t.success_criteria, t.status, t.status_line, t.todo, t.summary, t.checkpoint, t.current_session_id, \
     t.created_by, t.last_activity_at, t.resolved_at, t.created_at, t.user_id";

struct Stored {
    view: ThreadView,
    user_id: String,
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<Stored> {
    let todo: Vec<TodoItem> = row
        .get::<_, Option<String>>(11)?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let done = todo.iter().filter(|t| t.state == "done").count();
    let status: String = row.get(9)?;
    Ok(Stored {
        view: ThreadView {
            id: row.get(0)?,
            bot_id: row.get(1)?,
            project_id: row.get(2)?,
            parent_thread_id: row.get(3)?,
            kind: row.get(4)?,
            incognito: row.get::<_, i64>(5)? != 0,
            title: row.get(6)?,
            objective: row.get(7)?,
            success_criteria: row.get(8)?,
            group: group_of(&status),
            status,
            status_line: row.get(10)?,
            progress: [done, todo.len()],
            todo,
            summary: row.get(12)?,
            checkpoint: row.get::<_, Option<String>>(13)?.and_then(|s| serde_json::from_str(&s).ok()),
            current_session_id: row.get(14)?,
            generation: 0,
            context_used: None,
            depends_on: vec![],
            created_by: row.get(15)?,
            last_activity_at: row.get(16)?,
            resolved_at: row.get(17)?,
            created_at: row.get(18)?,
        },
        user_id: row.get(19)?,
    })
}

/// Fill generation, context use and dependencies.
fn enrich(conn: &rusqlite::Connection, v: &mut ThreadView) -> rusqlite::Result<()> {
    let gen: Option<(i64, i64, Option<i64>)> = conn
        .query_row(
            "SELECT generation, tokens_used, context_window FROM bot_thread_sessions
             WHERE thread_id = ?1 ORDER BY generation DESC LIMIT 1",
            params![v.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((g, used, window)) = gen {
        v.generation = g;
        v.context_used = window.filter(|w| *w > 0).map(|w| (used as f64 / w as f64).min(1.0));
    }
    let mut stmt = conn.prepare("SELECT depends_on FROM bot_thread_deps WHERE thread_id = ?1 ORDER BY depends_on")?;
    v.depends_on = stmt.query_map(params![v.id], |r| r.get(0))?.filter_map(Result::ok).collect();
    Ok(())
}

fn load(db: &DbHandle, id: &str) -> rusqlite::Result<Option<Stored>> {
    let conn = db.connect()?;
    let stored = conn
        .query_row(&format!("SELECT {COLS} FROM bot_threads t WHERE t.id = ?1"), params![id], map_row)
        .optional()?;
    match stored {
        Some(mut s) => {
            enrich(&conn, &mut s.view)?;
            Ok(Some(s))
        }
        None => Ok(None),
    }
}

/// Load a thread view by id (no owner check — callers already scoped it).
pub fn load_view(db: &DbHandle, id: &str) -> rusqlite::Result<Option<ThreadView>> {
    Ok(load(db, id)?.map(|s| s.view))
}

fn generations(db: &DbHandle, id: &str) -> rusqlite::Result<Vec<GenerationView>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT generation, session_id, model, context_window, tokens_used, reason, checkpoint_summary,
                started_at, ended_at
         FROM bot_thread_sessions WHERE thread_id = ?1 ORDER BY generation",
    )?;
    let rows = stmt.query_map(params![id], |r| {
        Ok(GenerationView {
            generation: r.get(0)?,
            session_id: r.get(1)?,
            model: r.get(2)?,
            context_window: r.get(3)?,
            tokens_used: r.get(4)?,
            reason: r.get(5)?,
            checkpoint_summary: r.get(6)?,
            started_at: r.get(7)?,
            ended_at: r.get(8)?,
        })
    })?;
    rows.collect()
}

/// Record a lifecycle event on the bot ledger, tagged with the thread.
fn ledger(db: &DbHandle, bot_id: &str, thread_id: &str, event_type: &str, actor: (&str, &str), payload: Value, session_id: Option<&str>) {
    let key = format!("{thread_id}:{event_type}:{}", uuid::Uuid::new_v4());
    let body = AppendEventBody {
        event_type: event_type.to_string(),
        actor: ActorBody { r#type: actor.0.to_string(), id: actor.1.to_string() },
        payload,
        occurred_at: None,
        session_id: session_id.map(str::to_string),
        goal_id: None,
        wih_id: None,
        task_id: None,
        run_id: None,
        idempotency_key: Some(key.clone()),
    };
    match append_event(db, bot_id, &body, &now()) {
        Ok(_) => {
            if let Ok(conn) = db.connect() {
                let _ = conn.execute(
                    "UPDATE bot_events SET thread_id = ?1 WHERE bot_id = ?2 AND idempotency_key = ?3",
                    params![thread_id, bot_id, key],
                );
            }
        }
        Err(e) => warn!(thread = %thread_id, error = %e, "failed to ledger thread event"),
    }
}

/// Existing bot chats become threads (generation 1) the first time they are
/// listed. Idempotent: sessions already attached to a thread are skipped.
pub fn sync_user_threads(db: &DbHandle, user_id: &str) -> rusqlite::Result<usize> {
    let conn = db.connect()?;
    let orphans: Vec<(String, String, bool, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT m.session_id,
                    COALESCE(json_extract(m.metadata, '$.botCanonicalFor'), json_extract(m.metadata, '$.botThreadOf')),
                    json_extract(m.metadata, '$.botCanonicalFor') IS NOT NULL,
                    json_extract(m.metadata, '$.projectId')
             FROM session_metadata m
             JOIN agents a ON a.id = COALESCE(json_extract(m.metadata, '$.botCanonicalFor'), json_extract(m.metadata, '$.botThreadOf'))
             WHERE a.user_id = ?1
               AND COALESCE(json_extract(m.metadata, '$.isGroupChat'), 0) = 0
               AND NOT EXISTS (SELECT 1 FROM bot_thread_sessions s WHERE s.session_id = m.session_id)",
        )?;
        let rows = stmt.query_map(params![user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)))?;
        rows.filter_map(Result::ok).collect()
    };
    let ts = now();
    for (session_id, bot_id, canonical, project_id) in &orphans {
        let id = uuid::Uuid::new_v4().to_string();
        let (kind, title) = if *canonical { ("standing", "Main chat") } else { ("task", "Thread") };
        conn.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, project_id, kind, title, status, current_session_id,
                 created_by, last_activity_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'idle', ?7, 'import', ?8, ?8, ?8)",
            params![id, user_id, bot_id, project_id, kind, title, session_id, ts],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO bot_thread_sessions (thread_id, generation, session_id, reason, started_at)
             VALUES (?1, 1, ?2, 'start', ?3)",
            params![id, session_id, ts],
        )?;
    }
    Ok(orphans.len())
}

fn list(db: &DbHandle, user_id: &str, q: &ListQuery) -> rusqlite::Result<Vec<ThreadView>> {
    let conn = db.connect()?;
    let mut sql = format!("SELECT {COLS} FROM bot_threads t WHERE t.user_id = ?1");
    let mut args: Vec<String> = vec![user_id.to_string()];
    if let Some(p) = &q.project_id {
        args.push(p.clone());
        sql.push_str(&format!(" AND t.project_id = ?{}", args.len()));
    }
    if let Some(b) = &q.bot_id {
        args.push(b.clone());
        sql.push_str(&format!(" AND t.bot_id = ?{}", args.len()));
    }
    if !q.include_resolved {
        sql.push_str(" AND t.status NOT IN ('done', 'failed')");
    }
    if !q.include_incognito {
        sql.push_str(" AND t.incognito = 0");
    }
    sql.push_str(" ORDER BY t.last_activity_at DESC LIMIT 500");
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<Stored> = stmt
        .query_map(rusqlite::params_from_iter(args.iter()), map_row)?
        .filter_map(Result::ok)
        .collect();
    let mut out = Vec::with_capacity(rows.len());
    for mut s in rows {
        enrich(&conn, &mut s.view)?;
        out.push(s.view);
    }
    Ok(out)
}

/// Reject a dependency that would make a cycle (A waits on B waits on A).
fn creates_cycle(conn: &rusqlite::Connection, thread_id: &str, depends_on: &[String]) -> rusqlite::Result<bool> {
    let mut frontier: Vec<String> = depends_on.to_vec();
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = frontier.pop() {
        if next == thread_id {
            return Ok(true);
        }
        if !seen.insert(next.clone()) {
            continue;
        }
        let mut stmt = conn.prepare("SELECT depends_on FROM bot_thread_deps WHERE thread_id = ?1")?;
        let ups: Vec<String> = stmt.query_map(params![next], |r| r.get(0))?.filter_map(Result::ok).collect();
        frontier.extend(ups);
    }
    Ok(false)
}

// ─── Runtime seam ───────────────────────────────────────────────────────────

/// gizzi operations a thread needs. Production calls gizzi; tests record.
pub trait ThreadRuntime: Send + Sync {
    fn create_session(
        &self,
        bot_id: &str,
        bot_name: &str,
        title: &str,
        canonical: bool,
        thread_id: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
    fn seed(&self, session_id: &str, text: &str) -> impl Future<Output = Result<(), String>> + Send;
    /// Model-written checkpoint for a session: `{summary, decisions[],
    /// openItems[], artifacts[]}`. `None` → the caller falls back.
    fn summarize(&self, _session_id: &str, _objective: &str) -> impl Future<Output = Option<Value>> + Send {
        async { None }
    }
}

pub const CHECKPOINT_SYSTEM: &str = "You write the checkpoint a bot needs to continue its work in a fresh context. \
Reply with ONE JSON object and nothing else: {\"summary\":\"<where things stand, 2-5 sentences>\",\"decisions\":[\"<decisions made>\"],\"openItems\":[\"<what is still open>\"],\"artifacts\":[\"<files, links or outputs produced>\"]}. \
Keep facts, numbers and names exactly as they appear. No advice, no filler.";

/// Parse the model's checkpoint JSON (tolerates code fences).
pub fn parse_checkpoint(raw: &str) -> Option<Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let v: Value = serde_json::from_str(&raw[start..=end]).ok()?;
    v.get("summary").and_then(Value::as_str).filter(|s| !s.trim().is_empty())?;
    Some(v)
}

pub struct GizziRuntime {
    pub db: DbHandle,
}

impl ThreadRuntime for GizziRuntime {
    async fn create_session(&self, bot_id: &str, bot_name: &str, title: &str, canonical: bool, thread_id: &str) -> Result<String, String> {
        crate::agent_session_routes::create_bot_thread_session(&self.db, bot_id, bot_name, title, canonical, Some(thread_id)).await
    }
    async fn seed(&self, session_id: &str, text: &str) -> Result<(), String> {
        crate::agent_session_routes::seed_session_message(session_id, text).await
    }
    async fn summarize(&self, session_id: &str, objective: &str) -> Option<Value> {
        let transcript = crate::agent_session_routes::session_transcript(session_id, 24_000).await.ok()?;
        if transcript.trim().is_empty() {
            return None;
        }
        let prompt = format!("Objective: {objective}\n\nConversation so far:\n{transcript}");
        // Write the checkpoint on the thread's own bot model, not the
        // platform default (which may be an unconfigured provider).
        let model: Option<(String, String)> = self.db.connect().ok().and_then(|c| {
            c.query_row(
                "SELECT a.provider, a.model FROM bot_thread_sessions s
                 JOIN bot_threads t ON t.id = s.thread_id JOIN agents a ON a.id = t.bot_id
                 WHERE s.session_id = ?1",
                params![session_id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .ok()
            .and_then(|(p, m)| p.zip(m).filter(|(p, m)| !p.is_empty() && !m.is_empty()))
        });
        let raw = crate::gizzi_completion::complete_ephemeral(&prompt, Some(CHECKPOINT_SYSTEM), model.as_ref()).await?;
        parse_checkpoint(&raw)
    }
}

fn bot_name(db: &DbHandle, bot_id: &str) -> String {
    db.connect()
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT COALESCE(json_extract(config, '$.botProfile.displayName'), name) FROM agents WHERE id = ?1",
                params![bot_id],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .unwrap_or_else(|| "Bot".into())
}

/// Create a thread; generation 1 adopts `session_id` or gets a new session.
pub async fn create<R: ThreadRuntime>(db: &DbHandle, rt: &R, user_id: &str, body: CreateThreadBody) -> Result<ThreadView, String> {
    if body.title.trim().is_empty() {
        return Err("title is required".into());
    }
    if body.kind != "standing" && body.kind != "task" {
        return Err("kind must be standing or task".into());
    }
    let status = body.status.clone().unwrap_or_else(|| "idle".into());
    if !STATUSES.contains(&status.as_str()) {
        return Err(format!("unknown status `{status}`"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let session_id = match body.session_id.clone() {
        Some(s) => s,
        None => rt.create_session(&body.bot_id, &bot_name(db, &body.bot_id), body.title.trim(), false, &id).await?,
    };
    let ts = now();
    {
        let conn = db.connect().map_err(|e| e.to_string())?;
        if creates_cycle(&conn, &id, &body.depends_on).map_err(|e| e.to_string())? {
            return Err("dependency cycle".into());
        }
        conn.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, project_id, parent_thread_id, kind, incognito, title,
                 objective, success_criteria, status, todo, current_session_id, created_by, origin,
                 started_at, last_activity_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17, ?17)",
            params![
                id,
                user_id,
                body.bot_id,
                body.project_id,
                body.parent_thread_id,
                body.kind,
                body.incognito as i64,
                body.title.trim(),
                body.objective,
                body.success_criteria,
                status,
                serde_json::to_string(&body.todo).ok(),
                session_id,
                body.created_by.clone().unwrap_or_else(|| "user".into()),
                body.origin.as_ref().map(|o| o.to_string()),
                (group_of(&status) == "working").then(|| ts.clone()),
                ts,
            ],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO bot_thread_sessions (thread_id, generation, session_id, reason, started_at) VALUES (?1, 1, ?2, 'start', ?3)",
            params![id, session_id, ts],
        )
        .map_err(|e| e.to_string())?;
        for dep in &body.depends_on {
            conn.execute("INSERT OR IGNORE INTO bot_thread_deps (thread_id, depends_on) VALUES (?1, ?2)", params![id, dep])
                .map_err(|e| e.to_string())?;
        }
        if let Some(p) = &body.project_id {
            conn.execute(
                "INSERT OR IGNORE INTO project_bots (project_id, bot_id, added_at) VALUES (?1, ?2, ?3)",
                params![p, body.bot_id, ts],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if !body.incognito {
        ledger(
            db,
            &body.bot_id,
            &id,
            "thread.created",
            ("user", user_id),
            json!({ "title": body.title.trim(), "kind": body.kind, "projectId": body.project_id, "createdBy": body.created_by }),
            Some(&session_id),
        );
    }
    load(db, &id).map_err(|e| e.to_string())?.map(|s| s.view).ok_or_else(|| "thread vanished".into())
}

/// Write the checkpoint, end the current generation, start the next one.
pub async fn do_handoff<R: ThreadRuntime>(db: &DbHandle, rt: &R, user_id: &str, id: &str, mut body: HandoffBody) -> Result<ThreadView, String> {
    let stored = load(db, id).map_err(|e| e.to_string())?.ok_or("thread not found")?;
    let t = stored.view;
    if body.summary.trim().is_empty() {
        // Automatic handoff: the model writes the checkpoint from this
        // window's transcript; without a model, carry the thread's own state.
        let objective = t.objective.clone().unwrap_or_else(|| t.title.clone());
        let written = match t.current_session_id.as_deref() {
            Some(sid) => rt.summarize(sid, &objective).await,
            None => None,
        };
        let list = |v: &Value, k: &str| -> Vec<String> {
            v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        match written {
            Some(cp) => {
                body.summary = cp["summary"].as_str().unwrap_or_default().to_string();
                if body.decisions.is_empty() { body.decisions = list(&cp, "decisions"); }
                if body.open_items.is_empty() { body.open_items = list(&cp, "openItems"); }
                if body.artifacts.is_empty() { body.artifacts = list(&cp, "artifacts"); }
            }
            None => {
                body.summary = t
                    .summary
                    .clone()
                    .or_else(|| t.status_line.clone())
                    .unwrap_or_else(|| format!("Continuing \"{}\" in a fresh context.", t.title));
            }
        }
    }
    let canonical = db
        .connect()
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT json_extract(config, '$.canonicalThreadId') FROM agents WHERE id = ?1",
                params![t.bot_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
        })
        .map_or(false, |pin| Some(pin) == t.current_session_id);
    let next_gen = t.generation.max(1) + 1;
    let session = rt.create_session(&t.bot_id, &bot_name(db, &t.bot_id), &t.title, canonical, &t.id).await?;

    let mut checkpoint = format!("[checkpoint: generation {}] Continuing this thread in a fresh context.\n\n", next_gen - 1);
    if let Some(o) = t.objective.as_deref() {
        checkpoint.push_str(&format!("Objective: {o}\n"));
    }
    if let Some(c) = t.success_criteria.as_deref() {
        checkpoint.push_str(&format!("Done when: {c}\n"));
    }
    checkpoint.push_str(&format!("\nWhere things stand:\n{}\n", body.summary.trim()));
    for (label, items) in [("Decisions", &body.decisions), ("Open items", &body.open_items), ("Artifacts", &body.artifacts)] {
        if !items.is_empty() {
            checkpoint.push_str(&format!("\n{label}:\n"));
            for i in items {
                checkpoint.push_str(&format!("- {i}\n"));
            }
        }
    }
    if !t.todo.is_empty() {
        checkpoint.push_str("\nPlan:\n");
        for item in &t.todo {
            let mark = match item.state.as_str() { "done" => "[x]", "active" => "[~]", _ => "[ ]" };
            checkpoint.push_str(&format!("{mark} {}\n", item.text));
        }
    }
    rt.seed(&session, &checkpoint).await?;

    let ts = now();
    let carried = json!({
        "summary": body.summary.trim(),
        "decisions": body.decisions,
        "openItems": body.open_items,
        "artifacts": body.artifacts,
        "generation": next_gen - 1,
    });
    {
        let conn = db.connect().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE bot_thread_sessions SET ended_at = ?3, checkpoint_summary = ?4 WHERE thread_id = ?1 AND generation = ?2",
            params![t.id, next_gen - 1, ts, body.summary.trim()],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO bot_thread_sessions (thread_id, generation, session_id, model, context_window, reason, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![t.id, next_gen, session, body.model, body.context_window, body.reason, ts],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE bot_threads SET current_session_id = ?2, checkpoint = ?3, last_activity_at = ?4, updated_at = ?4 WHERE id = ?1",
            params![t.id, session, carried.to_string(), ts],
        )
        .map_err(|e| e.to_string())?;
        if canonical {
            conn.execute(
                "UPDATE agents SET config = json_set(COALESCE(config, '{}'), '$.canonicalThreadId', ?2) WHERE id = ?1",
                params![t.bot_id, session],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if !t.incognito {
        ledger(
            db,
            &t.bot_id,
            &t.id,
            "thread.checkpointed",
            ("user", user_id),
            json!({ "generation": next_gen, "reason": body.reason, "carried": carried }),
            Some(&session),
        );
    }
    load(db, id).map_err(|e| e.to_string())?.map(|s| s.view).ok_or_else(|| "thread vanished".into())
}

// ─── Handlers ───────────────────────────────────────────────────────────────

fn err(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> rusqlite::Result<T> + Send + 'static) -> Result<T, Response> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            warn!(error = %e, "thread DB error");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
        Err(e) => {
            warn!(error = %e, "thread DB task panicked");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "database error"))
        }
    }
}

/// Load a thread the caller owns.
async fn owned(state: &AppState, user: &AuthUser, id: &str) -> Result<ThreadView, Response> {
    let db = state.db.clone();
    let id = id.to_string();
    match blocking(move || load(&db, &id)).await? {
        Some(s) if s.user_id == user.user_id => Ok(s.view),
        _ => Err(err(StatusCode::NOT_FOUND, "thread not found")),
    }
}

async fn list_threads(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    match blocking(move || {
        sync_user_threads(&db, &uid)?;
        list(&db, &uid, &q)
    })
    .await
    {
        Ok(threads) => Json(json!({ "threads": threads })).into_response(),
        Err(r) => r,
    }
}

async fn create_thread(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateThreadBody>,
) -> Response {
    if !verify_bot_ownership(&state, &user.user_id, &body.bot_id).await {
        return err(StatusCode::FORBIDDEN, "bot not found or access denied");
    }
    let rt = GizziRuntime { db: state.db.clone() };
    match create(&state.db, &rt, &user.user_id, body).await {
        Ok(t) => (StatusCode::CREATED, Json(json!({ "thread": t }))).into_response(),
        Err(e) if e.contains("required") || e.contains("unknown") || e.contains("kind") || e.contains("cycle") => {
            err(StatusCode::BAD_REQUEST, e)
        }
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

async fn get_thread(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let t = match owned(&state, &user, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let db = state.db.clone();
    match blocking(move || generations(&db, &id)).await {
        Ok(gens) => Json(json!({ "thread": t, "generations": gens })).into_response(),
        Err(r) => r,
    }
}

async fn thread_by_session(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(session_id): Path<String>,
) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let found = blocking(move || {
        sync_user_threads(&db, &uid)?;
        db.connect()?
            .query_row(
                "SELECT thread_id FROM bot_thread_sessions WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
    })
    .await;
    match found {
        Ok(Some(id)) => match owned(&state, &user, &id).await {
            Ok(t) => Json(json!({ "thread": t })).into_response(),
            Err(r) => r,
        },
        Ok(None) => err(StatusCode::NOT_FOUND, "no thread for that session"),
        Err(r) => r,
    }
}

async fn patch_thread(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<PatchThreadBody>,
) -> Response {
    let before = match owned(&state, &user, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if let Some(s) = body.status.as_deref() {
        if !STATUSES.contains(&s) {
            return err(StatusCode::BAD_REQUEST, format!("unknown status `{s}`"));
        }
    }
    let db = state.db.clone();
    let tid = id.clone();
    let status_changed = body.status.as_deref().filter(|s| *s != before.status).map(str::to_string);
    let res = blocking(move || {
        let ts = now();
        let conn = db.connect()?;
        conn.execute(
            "UPDATE bot_threads SET
                 title = COALESCE(?2, title), objective = COALESCE(?3, objective),
                 success_criteria = COALESCE(?4, success_criteria), status = COALESCE(?5, status),
                 status_line = COALESCE(?6, status_line), todo = COALESCE(?7, todo),
                 summary = COALESCE(?8, summary), project_id = COALESCE(?9, project_id),
                 started_at = CASE WHEN ?5 IN ('working', 'planning') AND started_at IS NULL THEN ?10 ELSE started_at END,
                 resolved_at = CASE WHEN ?5 IN ('done', 'failed') THEN ?10
                                    WHEN ?5 IS NOT NULL THEN NULL ELSE resolved_at END,
                 last_activity_at = ?10, updated_at = ?10
             WHERE id = ?1",
            params![
                tid,
                body.title,
                body.objective,
                body.success_criteria,
                body.status,
                body.status_line,
                body.todo.as_ref().and_then(|t| serde_json::to_string(t).ok()),
                body.summary,
                body.project_id,
                ts
            ],
        )?;
        load(&db, &tid)
    })
    .await;
    match res {
        Ok(Some(s)) => {
            // A thread reaching review/done may unblock queued dependents.
            if matches!(s.view.status.as_str(), "review" | "done") {
                if let Some(project_id) = s.view.project_id.clone() {
                    let ready = crate::coordinator_routes::ready_dependents(&state.db, &project_id, &s.view.id);
                    if !ready.is_empty() {
                        let st = state.clone();
                        let uid = user.user_id.clone();
                        tokio::spawn(async move {
                            let rt = crate::coordinator_routes::GizziCoordinator { state: st.clone() };
                            crate::coordinator_routes::start_threads(&st.db, &rt, &uid, &project_id, ready).await;
                        });
                    }
                }
            }
            if let (Some(status), false) = (status_changed, s.view.incognito) {
                let event = if before.group == "resolved" && s.view.group != "resolved" { "thread.resumed" } else { event_for_status(&status) };
                ledger(
                    &state.db,
                    &s.view.bot_id,
                    &s.view.id,
                    event,
                    ("user", &user.user_id),
                    json!({ "from": before.status, "to": status, "statusLine": s.view.status_line }),
                    s.view.current_session_id.as_deref(),
                );
            }
            Json(json!({ "thread": s.view })).into_response()
        }
        Ok(None) => err(StatusCode::NOT_FOUND, "thread not found"),
        Err(r) => r,
    }
}

async fn resolve_thread(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<ResolveBody>,
) -> Response {
    if body.status != "done" && body.status != "failed" {
        return err(StatusCode::BAD_REQUEST, "status must be done or failed");
    }
    let t = match owned(&state, &user, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    if t.kind == "standing" {
        return err(StatusCode::CONFLICT, "standing threads don't resolve; pause them instead");
    }
    patch_thread(
        State(state),
        Extension(user),
        Path(id),
        Json(PatchThreadBody { status: Some(body.status), summary: body.summary, ..Default::default() }),
    )
    .await
}

async fn report_usage(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UsageBody>,
) -> Response {
    let t = match owned(&state, &user, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let db = state.db.clone();
    let res = blocking(move || {
        let conn = db.connect()?;
        let (stored_window, stored_model): (Option<i64>, Option<String>) = conn.query_row(
            "SELECT context_window, model FROM bot_thread_sessions WHERE thread_id = ?1 AND generation = ?2",
            params![t.id, t.generation.max(1)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        conn.execute(
            "UPDATE bot_thread_sessions SET tokens_used = ?3,
                 context_window = COALESCE(?4, context_window), model = COALESCE(?5, model)
             WHERE thread_id = ?1 AND generation = ?2",
            params![t.id, t.generation.max(1), body.tokens_used, body.context_window, body.model],
        )?;
        let window = body.context_window.or(stored_window);
        let fraction = window.filter(|w| *w > 0).map(|w| body.tokens_used as f64 / w as f64);
        let switched_down = matches!((stored_window, body.context_window), (Some(old), Some(new)) if new < old)
            && body.model.is_some()
            && body.model != stored_model;
        let should = fraction.map_or(false, |f| f >= HANDOFF_FRACTION);
        let reason = if switched_down && should { Some("model_switch") } else if should { Some("budget") } else { None };
        Ok(json!({ "contextUsed": fraction, "shouldHandoff": should, "reason": reason, "threshold": HANDOFF_FRACTION }))
    })
    .await;
    match res {
        Ok(v) => Json(v).into_response(),
        Err(r) => r,
    }
}

async fn handoff(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<HandoffBody>,
) -> Response {
    if let Err(r) = owned(&state, &user, &id).await {
        return r;
    }
    let rt = GizziRuntime { db: state.db.clone() };
    match do_handoff(&state.db, &rt, &user.user_id, &id, body).await {
        Ok(t) => Json(json!({ "thread": t })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, e),
    }
}

async fn set_deps(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<DepsBody>,
) -> Response {
    if let Err(r) = owned(&state, &user, &id).await {
        return r;
    }
    let db = state.db.clone();
    let tid = id.clone();
    let res = blocking(move || {
        let conn = db.connect()?;
        conn.execute("DELETE FROM bot_thread_deps WHERE thread_id = ?1", params![tid])?;
        if creates_cycle(&conn, &tid, &body.depends_on)? {
            return Ok(None);
        }
        for dep in &body.depends_on {
            conn.execute("INSERT OR IGNORE INTO bot_thread_deps (thread_id, depends_on) VALUES (?1, ?2)", params![tid, dep])?;
        }
        load(&db, &tid)
    })
    .await;
    match res {
        Ok(Some(s)) => Json(json!({ "thread": s.view })).into_response(),
        Ok(None) => err(StatusCode::BAD_REQUEST, "dependency cycle"),
        Err(r) => r,
    }
}

/// Bot projects for the Bots launch: every project with a bot team (or
/// tagged `metadata.kind = "bots"`), its team, and live thread counts per
/// panel group — "Cloud pricing launch · 1 waiting · 1 working".
pub fn bot_project_overview(db: &DbHandle, user_id: &str) -> rusqlite::Result<Vec<Value>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT p.id, p.title, p.description, p.metadata, p.updated_at,
                (SELECT MAX(t.last_activity_at) FROM bot_threads t WHERE t.project_id = p.id AND t.incognito = 0)
         FROM cowork_projects p
         WHERE p.user_id = ?1
           AND (EXISTS (SELECT 1 FROM project_bots b WHERE b.project_id = p.id)
                OR json_extract(p.metadata, '$.kind') = 'bots')",
    )?;
    let projects: Vec<(String, String, Option<String>, Option<String>, Option<String>, Option<String>)> = stmt
        .query_map(params![user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .filter_map(Result::ok)
        .collect();
    let mut out = Vec::with_capacity(projects.len());
    for (id, title, description, metadata, updated, last_thread) in projects {
        let mut team_stmt = conn.prepare("SELECT bot_id FROM project_bots WHERE project_id = ?1 ORDER BY added_at")?;
        let team: Vec<String> = team_stmt.query_map(params![id], |r| r.get(0))?.filter_map(Result::ok).collect();
        let mut counts = json!({ "waiting": 0, "working": 0, "queued": 0, "idle": 0, "resolved": 0 });
        let mut c_stmt = conn.prepare("SELECT status, COUNT(*) FROM bot_threads WHERE project_id = ?1 AND incognito = 0 GROUP BY status")?;
        for row in c_stmt.query_map(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (status, n) = row?;
            let g = group_of(&status);
            counts[g] = json!(counts[g].as_i64().unwrap_or(0) + n);
        }
        let meta: Value = metadata.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or(Value::Null);
        let activity = match (last_thread, updated) {
            (Some(a), Some(b)) => Some(if a > b { a } else { b }),
            (a, b) => a.or(b),
        };
        out.push(json!({
            "id": id,
            "title": title,
            "description": description,
            "botIds": team,
            "counts": counts,
            "lastActivityAt": activity,
            "archived": meta.get("archived").and_then(Value::as_bool).unwrap_or(false),
            "favorite": meta.get("favorite").and_then(Value::as_bool).unwrap_or(false),
        }));
    }
    out.sort_by(|a, b| b["lastActivityAt"].as_str().cmp(&a["lastActivityAt"].as_str()));
    Ok(out)
}

async fn bot_projects(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    match blocking(move || bot_project_overview(&db, &uid)).await {
        Ok(projects) => Json(json!({ "projects": projects })).into_response(),
        Err(r) => r,
    }
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub limit: Option<i64>,
}

/// The thread's activity (its `thread.*`, `routine.*` and other ledger
/// events), newest first — the inspector's Activity tab.
async fn thread_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> Response {
    let t = match owned(&state, &user, &id).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let db = state.db.clone();
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let res = blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, seq, event_type, actor_type, actor_id, payload, session_id, occurred_at
             FROM bot_events WHERE bot_id = ?1 AND thread_id = ?2 ORDER BY seq DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![t.bot_id, t.id, limit], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "sequence": r.get::<_, i64>(1)?,
                "type": r.get::<_, String>(2)?,
                "actor": { "type": r.get::<_, String>(3)?, "id": r.get::<_, String>(4)? },
                "payload": r.get::<_, String>(5).ok().and_then(|p| serde_json::from_str::<Value>(&p).ok()).unwrap_or(Value::Null),
                "sessionId": r.get::<_, Option<String>>(6)?,
                "occurredAt": r.get::<_, String>(7)?,
            }))
        })?;
        rows.collect::<rusqlite::Result<Vec<Value>>>()
    })
    .await;
    match res {
        Ok(events) => Json(json!({ "events": events })).into_response(),
        Err(r) => r,
    }
}

async fn get_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(project_id): Path<String>,
) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let res = blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT p.bot_id FROM project_bots p JOIN agents a ON a.id = p.bot_id
             WHERE p.project_id = ?1 AND a.user_id = ?2 ORDER BY p.added_at",
        )?;
        let ids: Vec<String> = stmt.query_map(params![project_id, uid], |r| r.get(0))?.filter_map(Result::ok).collect();
        Ok(ids)
    })
    .await;
    match res {
        Ok(ids) => Json(json!({ "botIds": ids })).into_response(),
        Err(r) => r,
    }
}

async fn put_team(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(project_id): Path<String>,
    Json(body): Json<TeamBody>,
) -> Response {
    for bot in &body.bot_ids {
        if !verify_bot_ownership(&state, &user.user_id, bot).await {
            return err(StatusCode::FORBIDDEN, format!("bot {bot} not found or access denied"));
        }
    }
    let db = state.db.clone();
    let res = blocking(move || {
        let conn = db.connect()?;
        conn.execute("DELETE FROM project_bots WHERE project_id = ?1", params![project_id])?;
        let ts = now();
        for bot in &body.bot_ids {
            conn.execute(
                "INSERT OR IGNORE INTO project_bots (project_id, bot_id, added_at) VALUES (?1, ?2, ?3)",
                params![project_id, bot, ts],
            )?;
        }
        Ok(body.bot_ids)
    })
    .await;
    match res {
        Ok(ids) => Json(json!({ "botIds": ids })).into_response(),
        Err(r) => r,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::sync::Mutex;
    use tower::ServiceExt;

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    #[derive(Default)]
    struct FakeRt {
        created: Mutex<Vec<(String, bool)>>,
        seeded: Mutex<Vec<(String, String)>>,
        checkpoint: Option<Value>,
        prefix: &'static str,
    }

    impl ThreadRuntime for FakeRt {
        async fn create_session(&self, _b: &str, _n: &str, title: &str, canonical: bool, _t: &str) -> Result<String, String> {
            let mut c = self.created.lock().unwrap();
            c.push((title.to_string(), canonical));
            Ok(format!("{}sess-{}", self.prefix, c.len()))
        }
        async fn seed(&self, s: &str, text: &str) -> Result<(), String> {
            self.seeded.lock().unwrap().push((s.to_string(), text.to_string()));
            Ok(())
        }
        async fn summarize(&self, _s: &str, _o: &str) -> Option<Value> {
            self.checkpoint.clone()
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-threads-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config)
             VALUES ('bot-1', 'user-a', 'ledger', 'm', 'p', 1, '{\"botProfile\":{\"displayName\":\"Ledger\"}}')",
            [],
        )
        .unwrap();
        state
    }

    fn body(title: &str) -> CreateThreadBody {
        serde_json::from_value(json!({ "botId": "bot-1", "title": title })).unwrap()
    }

    async fn call(state: &Arc<AppState>, method: &str, uri: &str, u: &str, b: Option<Value>) -> (StatusCode, Value) {
        let app = thread_router().with_state(state.clone());
        let mut req = Request::builder().method(method).uri(uri).extension(user(u));
        let body = match b {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = app.oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[test]
    fn groups_follow_the_status_grammar() {
        assert_eq!(group_of("needs_you"), "waiting");
        assert_eq!(group_of("blocked"), "waiting");
        assert_eq!(group_of("working"), "working");
        assert_eq!(group_of("queued"), "queued");
        assert_eq!(group_of("failed"), "resolved");
        assert_eq!(group_of("paused"), "idle");
    }

    #[tokio::test]
    async fn create_patch_and_progress_ring() {
        let state = setup("create").await;
        let rt = FakeRt::default();
        let mut b = body("GPU unit economics");
        b.project_id = Some("proj-1".into());
        b.todo = vec![
            TodoItem { text: "Pull pricing".into(), state: "done".into() },
            TodoItem { text: "Model costs".into(), state: "active".into() },
            TodoItem { text: "Price it".into(), state: "pending".into() },
        ];
        let t = create(&state.db, &rt, "user-a", b).await.unwrap();
        assert_eq!(t.progress, [1, 3]);
        assert_eq!(t.generation, 1);
        assert_eq!(t.current_session_id.as_deref(), Some("sess-1"));
        assert_eq!(rt.created.lock().unwrap()[0], ("GPU unit economics".to_string(), false));

        let (s, v) = call(&state, "PATCH", &format!("/threads/{}", t.id), "user-a", Some(json!({"status": "needs_you", "statusLine": "Pick a margin"}))).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["thread"]["group"], "waiting");

        let (_, list) = call(&state, "GET", "/threads?projectId=proj-1", "user-a", None).await;
        assert_eq!(list["threads"].as_array().unwrap().len(), 1);
        let (_, team) = call(&state, "GET", "/projects/proj-1/bots", "user-a", None).await;
        assert_eq!(team["botIds"], json!(["bot-1"]));

        let (s, _) = call(&state, "GET", &format!("/threads/{}", t.id), "user-b", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "other users can't read it");

        let conn = state.db.connect().unwrap();
        let events: Vec<String> = conn
            .prepare("SELECT event_type FROM bot_events WHERE thread_id = ?1 ORDER BY seq")
            .unwrap()
            .query_map(params![t.id], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(events, vec!["thread.created", "thread.needs_user"]);
        let (s, ev) = call(&state, "GET", &format!("/threads/{}/events", t.id), "user-a", None).await;
        assert_eq!(s, StatusCode::OK);
        let types: Vec<&str> = ev["events"].as_array().unwrap().iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(types, vec!["thread.needs_user", "thread.created"]);
    }

    #[tokio::test]
    async fn usage_signals_handoff_and_handoff_starts_a_seeded_generation() {
        let state = setup("handoff").await;
        let rt = FakeRt::default();
        let mut b = body("Pricing engine");
        b.objective = Some("Wire pricing into checkout".into());
        let t = create(&state.db, &rt, "user-a", b).await.unwrap();

        let (_, u) = call(&state, "POST", &format!("/threads/{}/usage", t.id), "user-a", Some(json!({"tokensUsed": 100000, "contextWindow": 200000, "model": "sonnet"}))).await;
        assert_eq!(u["shouldHandoff"], false);
        let (_, u) = call(&state, "POST", &format!("/threads/{}/usage", t.id), "user-a", Some(json!({"tokensUsed": 142000, "contextWindow": 200000}))).await;
        assert_eq!(u["shouldHandoff"], true);
        assert_eq!(u["reason"], "budget");

        let next = do_handoff(
            &state.db,
            &rt,
            "user-a",
            &t.id,
            serde_json::from_value(json!({"summary": "Checkout calls the new price table", "decisions": ["35% margin"], "openItems": ["Annual plan discount"], "reason": "budget"})).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(next.generation, 2);
        assert_eq!(next.current_session_id.as_deref(), Some("sess-2"));
        assert_eq!(next.checkpoint.as_ref().unwrap()["decisions"], json!(["35% margin"]));
        let seeded = rt.seeded.lock().unwrap().clone();
        assert_eq!(seeded[0].0, "sess-2");
        assert!(seeded[0].1.starts_with("[checkpoint: generation 1]"));
        assert!(seeded[0].1.contains("Objective: Wire pricing into checkout"));
        assert!(seeded[0].1.contains("- 35% margin"));

        let gens = generations(&state.db, &t.id).unwrap();
        assert_eq!(gens.len(), 2);
        assert_eq!(gens[0].tokens_used, 142000);
        assert!(gens[0].ended_at.is_some());
        assert_eq!(gens[0].checkpoint_summary.as_deref(), Some("Checkout calls the new price table"));
    }

    #[tokio::test]
    async fn automatic_handoff_writes_its_own_checkpoint() {
        let state = setup("auto").await;
        let rt = FakeRt {
            checkpoint: parse_checkpoint("```json\n{\"summary\":\"H100 all-in is $1.94/hr\",\"decisions\":[\"35% margin\"],\"openItems\":[\"Annual discount\"],\"artifacts\":[\"pricing.xlsx\"]}\n```"),
            ..Default::default()
        };
        let t = create(&state.db, &rt, "user-a", body("Economics")).await.unwrap();
        let auto: HandoffBody = serde_json::from_value(json!({"reason": "budget"})).unwrap();
        let next = do_handoff(&state.db, &rt, "user-a", &t.id, auto).await.unwrap();
        assert_eq!(next.checkpoint.as_ref().unwrap()["summary"], "H100 all-in is $1.94/hr");
        assert_eq!(next.checkpoint.as_ref().unwrap()["artifacts"], json!(["pricing.xlsx"]));
        assert!(rt.seeded.lock().unwrap()[0].1.contains("- 35% margin"));

        // No model: carry the thread's own state instead of failing.
        let bare = FakeRt { prefix: "b-", ..Default::default() };
        let t2 = create(&state.db, &bare, "user-a", body("Engine")).await.unwrap();
        state.db.connect().unwrap().execute("UPDATE bot_threads SET status_line = 'Wiring checkout' WHERE id = ?1", params![t2.id]).unwrap();
        let auto: HandoffBody = serde_json::from_value(json!({"reason": "model_switch"})).unwrap();
        let next2 = do_handoff(&state.db, &bare, "user-a", &t2.id, auto).await.unwrap();
        assert_eq!(next2.checkpoint.as_ref().unwrap()["summary"], "Wiring checkout");
        assert!(parse_checkpoint("{\"summary\": \"\"}").is_none());
    }

    #[tokio::test]
    async fn existing_bot_chats_become_threads_once() {
        let state = setup("sync").await;
        state.db.set_session_metadata("s-main", &json!({"isBot": true, "botCanonicalFor": "bot-1"})).unwrap();
        state.db.set_session_metadata("s-side", &json!({"isBot": true, "botThreadOf": "bot-1"})).unwrap();
        state.db.set_session_metadata("s-group", &json!({"isGroupChat": true, "botThreadOf": "bot-1"})).unwrap();
        assert_eq!(sync_user_threads(&state.db, "user-a").unwrap(), 2);
        assert_eq!(sync_user_threads(&state.db, "user-a").unwrap(), 0);
        let (_, list) = call(&state, "GET", "/threads?botId=bot-1", "user-a", None).await;
        let kinds: Vec<&str> = list["threads"].as_array().unwrap().iter().map(|t| t["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"standing") && kinds.contains(&"task"));
        let (s, by) = call(&state, "GET", "/threads/by-session/s-main", "user-a", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(by["thread"]["kind"], "standing");
    }

    #[tokio::test]
    async fn dependencies_reject_cycles_and_standing_threads_do_not_resolve() {
        let state = setup("deps").await;
        let rt = FakeRt::default();
        let a = create(&state.db, &rt, "user-a", body("Economics")).await.unwrap();
        let mut bb = body("Engine");
        bb.depends_on = vec![a.id.clone()];
        let b = create(&state.db, &rt, "user-a", bb).await.unwrap();
        assert_eq!(b.depends_on, vec![a.id.clone()]);
        let (s, _) = call(&state, "PUT", &format!("/threads/{}/deps", a.id), "user-a", Some(json!({"dependsOn": [b.id]}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        let mut sb = body("Main chat");
        sb.kind = "standing".into();
        let st = create(&state.db, &rt, "user-a", sb).await.unwrap();
        let (s, _) = call(&state, "POST", &format!("/threads/{}/resolve", st.id), "user-a", Some(json!({"status": "done"}))).await;
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, v) = call(&state, "POST", &format!("/threads/{}/resolve", a.id), "user-a", Some(json!({"status": "done", "summary": "H100 at $1.94/hr all-in"}))).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["thread"]["group"], "resolved");
        let (_, open) = call(&state, "GET", "/threads", "user-a", None).await;
        assert!(open["threads"].as_array().unwrap().iter().all(|t| t["id"] != json!(a.id)), "resolved hidden by default");
    }

    #[tokio::test]
    async fn bot_project_overview_counts_threads_by_group() {
        let state = setup("overview").await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO cowork_projects (id, user_id, title, metadata) VALUES
                   ('p1', 'user-a', 'Cloud pricing launch', NULL),
                   ('p2', 'user-a', 'Plain chat project', NULL),
                   ('p3', 'user-a', 'Empty bots project', '{\"kind\":\"bots\"}')",
                [],
            )
            .unwrap();
        let rt = FakeRt::default();
        for (title, status) in [("Economics", "needs_you"), ("Research", "working"), ("Page", "queued"), ("SDK", "done")] {
            let mut b = body(title);
            b.project_id = Some("p1".into());
            b.status = Some(status.into());
            create(&state.db, &rt, "user-a", b).await.unwrap();
        }
        let (s, v) = call(&state, "GET", "/bot-projects", "user-a", None).await;
        assert_eq!(s, StatusCode::OK);
        let projects = v["projects"].as_array().unwrap();
        let titles: Vec<&str> = projects.iter().map(|p| p["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"Cloud pricing launch") && titles.contains(&"Empty bots project"));
        assert!(!titles.contains(&"Plain chat project"), "projects without a bot team stay out");
        let p1 = projects.iter().find(|p| p["id"] == "p1").unwrap();
        assert_eq!(p1["botIds"], json!(["bot-1"]));
        assert_eq!(p1["counts"], json!({"waiting": 1, "working": 1, "queued": 1, "idle": 0, "resolved": 1}));
    }

    #[tokio::test]
    async fn incognito_threads_are_hidden_and_leave_no_ledger_trail() {
        let state = setup("incognito").await;
        let rt = FakeRt::default();
        let mut b = body("Quick question");
        b.incognito = true;
        let t = create(&state.db, &rt, "user-a", b).await.unwrap();
        let (_, list) = call(&state, "GET", "/threads", "user-a", None).await;
        assert!(list["threads"].as_array().unwrap().is_empty());
        let n: i64 = state.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE thread_id = ?1", params![t.id], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }
}
