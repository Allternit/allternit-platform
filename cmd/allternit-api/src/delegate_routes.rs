//! Project Delegate board: a project's to-do list you hand to its bots.
//!
//! A task waits with no bot until you drop it on one, or Auto does. Assigning
//! it creates a project thread for that bot (`thread_routes::create`) and the
//! coordinator runs it like any other project thread: kickoff turn, review,
//! Al's completion line, the wrap-up.
//!
//! Each bot works its delegated tasks one at a time. A new thread starts at
//! once when the bot has nothing running in the project; otherwise it is
//! `queued`, and [`pump`] starts it when the bot's running thread stops
//! (review, done, blocked, needs you, …). Plan threads that wait on other
//! threads (`bot_thread_deps`) are the coordinator's business, not ours:
//! `pump` only picks threads a task row points at.
//!
//! Numbers are estimates, never quotes. Time is the user's own estimate per
//! task. Cost is the average real cost of the bot's last finished task
//! threads from the usage ledger (`llm_usage_events`), `None` until it has
//! one; a bot on a vendor subscription averages 0, which is what it costs.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::coordinator_routes::{self, CoordinatorRuntime, GizziCoordinator, TeamBot};
use crate::db::DbHandle;
use crate::thread_routes::{self, CreateThreadBody, ThreadView};
use crate::AppState;

pub const MAX_TITLE: usize = 200;
pub const MAX_ESTIMATE_MIN: i64 = 24 * 60;
/// Finished threads the cost average looks back over.
const COST_SAMPLE: i64 = 20;

pub fn delegate_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/projects/:project_id/delegate", get(get_board))
        .route("/projects/:project_id/delegate/tasks", post(add_task))
        .route("/projects/:project_id/delegate/tasks/:task_id", delete(remove_task))
        .route("/projects/:project_id/delegate/tasks/:task_id/assign", post(assign_task))
        .route("/projects/:project_id/delegate/auto", put(set_auto))
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum DelegateError {
    ProjectNotFound,
    TaskNotFound,
    AlreadyAssigned,
    NotOnTeam,
    NoBots,
    Invalid(String),
    Runtime(String),
    Db,
}

impl From<rusqlite::Error> for DelegateError {
    fn from(e: rusqlite::Error) -> Self {
        warn!(error = %e, "delegate DB error");
        DelegateError::Db
    }
}

impl DelegateError {
    fn status(&self) -> StatusCode {
        match self {
            DelegateError::ProjectNotFound | DelegateError::TaskNotFound => StatusCode::NOT_FOUND,
            DelegateError::AlreadyAssigned | DelegateError::NoBots => StatusCode::CONFLICT,
            DelegateError::NotOnTeam | DelegateError::Invalid(_) => StatusCode::BAD_REQUEST,
            DelegateError::Runtime(_) => StatusCode::BAD_GATEWAY,
            DelegateError::Db => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// What the user reads.
    pub fn message(&self) -> String {
        match self {
            DelegateError::ProjectNotFound => "Project not found.".into(),
            DelegateError::TaskNotFound => "That task is gone.".into(),
            DelegateError::AlreadyAssigned => "That task is already handed out.".into(),
            DelegateError::NotOnTeam => "That bot isn't on this project's team.".into(),
            DelegateError::NoBots => "Add a bot to this project to hand tasks to.".into(),
            DelegateError::Invalid(m) => m.clone(),
            DelegateError::Runtime(m) => format!("Couldn't start the bot's thread: {m}"),
            DelegateError::Db => "Something went wrong saving that. Try again.".into(),
        }
    }
}

impl IntoResponse for DelegateError {
    fn into_response(self) -> Response {
        (self.status(), Json(json!({ "error": self.message() }))).into_response()
    }
}

// ─── Persistence ────────────────────────────────────────────────────────────

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn project_title(db: &DbHandle, project_id: &str, user_id: &str) -> Result<String, DelegateError> {
    db.connect()?
        .query_row("SELECT title FROM cowork_projects WHERE id = ?1 AND user_id = ?2", params![project_id, user_id], |r| r.get(0))
        .optional()?
        .ok_or(DelegateError::ProjectNotFound)
}

pub fn auto_enabled(db: &DbHandle, project_id: &str, user_id: &str) -> bool {
    db.connect()
        .ok()
        .and_then(|c| {
            c.query_row(
                "SELECT auto FROM project_delegate_settings WHERE project_id = ?1 AND user_id = ?2",
                params![project_id, user_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .ok()
            .flatten()
        })
        .unwrap_or(0)
        == 1
}

#[derive(Debug, Clone)]
struct TaskRow {
    id: String,
    title: String,
    notes: Option<String>,
    estimate_min: Option<i64>,
    assigned_bot_id: Option<String>,
}

fn load_task(db: &DbHandle, project_id: &str, user_id: &str, task_id: &str) -> Result<TaskRow, DelegateError> {
    db.connect()?
        .query_row(
            "SELECT id, title, notes, estimate_min, assigned_bot_id FROM project_tasks
             WHERE id = ?1 AND project_id = ?2 AND user_id = ?3",
            params![task_id, project_id, user_id],
            |r| Ok(TaskRow { id: r.get(0)?, title: r.get(1)?, notes: r.get(2)?, estimate_min: r.get(3)?, assigned_bot_id: r.get(4)? }),
        )
        .optional()?
        .ok_or(DelegateError::TaskNotFound)
}

fn unassigned(db: &DbHandle, project_id: &str, user_id: &str) -> Result<Vec<TaskRow>, DelegateError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT id, title, notes, estimate_min, assigned_bot_id FROM project_tasks
         WHERE project_id = ?1 AND user_id = ?2 AND assigned_bot_id IS NULL ORDER BY position",
    )?;
    let rows = stmt
        .query_map(params![project_id, user_id], |r| {
            Ok(TaskRow { id: r.get(0)?, title: r.get(1)?, notes: r.get(2)?, estimate_min: r.get(3)?, assigned_bot_id: r.get(4)? })
        })?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// One open thread in a bot's lane: what it is running and what waits.
#[derive(Debug, Clone)]
struct LaneItem {
    thread_id: String,
    task_id: Option<String>,
    title: String,
    status: String,
    estimate_min: Option<i64>,
    started_at: Option<String>,
}

/// The bot's open task threads in this project, running first, then the
/// delegated queue in the order it will run.
fn lane(db: &DbHandle, project_id: &str, bot_id: &str) -> Result<Vec<LaneItem>, DelegateError> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT t.id, k.id, t.title, t.status, k.estimate_min, t.started_at
         FROM bot_threads t LEFT JOIN project_tasks k ON k.thread_id = t.id
         WHERE t.project_id = ?1 AND t.bot_id = ?2 AND t.kind = 'task' AND t.incognito = 0
           AND t.status IN ('planning', 'working', 'queued', 'blocked', 'needs_you', 'paused')
         ORDER BY CASE t.status WHEN 'working' THEN 0 WHEN 'planning' THEN 0 WHEN 'queued' THEN 2 ELSE 1 END,
                  COALESCE(k.assigned_at, t.created_at)",
    )?;
    let rows = stmt
        .query_map(params![project_id, bot_id], |r| {
            Ok(LaneItem { thread_id: r.get(0)?, task_id: r.get(1)?, title: r.get(2)?, status: r.get(3)?, estimate_min: r.get(4)?, started_at: r.get(5)? })
        })?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Average real cost (cents) of the bot's last finished task threads, and how
/// many threads it averaged over. `None` until one has usage on the ledger.
pub fn cost_per_task_cents(db: &DbHandle, user_id: &str, bot_id: &str) -> Option<(f64, i64)> {
    let conn = db.connect().ok()?;
    let (n, micro): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(c), 0) FROM (
                 SELECT SUM(COALESCE(e.recomputed_cost_microdollars, e.cost_microdollars)) AS c
                 FROM bot_threads t
                 JOIN bot_thread_sessions s ON s.thread_id = t.id
                 JOIN llm_usage_events e ON e.gizzi_session_id = s.session_id
                 WHERE t.bot_id = ?1 AND t.user_id = ?2 AND t.kind = 'task' AND t.status IN ('review', 'done')
                 GROUP BY t.id ORDER BY MAX(t.last_activity_at) DESC LIMIT ?3
             )",
            params![bot_id, user_id, COST_SAMPLE],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    (n > 0).then(|| (micro as f64 / 10_000.0 / n as f64, n))
}

/// Minutes left on a lane item: its estimate, less the time it has already
/// run (at least one minute while it runs). `None` without an estimate.
fn remaining_min(item: &LaneItem, at: chrono::DateTime<chrono::Utc>) -> Option<i64> {
    let est = item.estimate_min?;
    if !matches!(item.status.as_str(), "working" | "planning") {
        return Some(est);
    }
    let ran = item
        .started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|s| (at - s.with_timezone(&chrono::Utc)).num_minutes().max(0))
        .unwrap_or(0);
    Some((est - ran).max(1))
}

fn is_running(status: &str) -> bool {
    matches!(status, "working" | "planning")
}

/// Work the bot still has to do: running or waiting its turn. A blocked or
/// paused thread waits on the user, not the bot, so it doesn't make it busy.
fn is_active(status: &str) -> bool {
    is_running(status) || status == "queued"
}

// ─── Queue ──────────────────────────────────────────────────────────────────

/// Start the bot's next delegated task when it has nothing running in the
/// project. Returns `(thread id, kickoff text)` for `start_threads`, or
/// nothing. The `queued → working` claim makes a second caller a no-op.
pub fn pump(db: &DbHandle, project_id: &str, bot_id: &str) -> Vec<(String, String)> {
    let Ok(conn) = db.connect() else { return vec![] };
    let running: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_threads WHERE project_id = ?1 AND bot_id = ?2 AND status IN ('planning', 'working')",
            params![project_id, bot_id],
            |r| r.get(0),
        )
        .unwrap_or(1);
    if running > 0 {
        return vec![];
    }
    let next: Option<String> = conn
        .query_row(
            "SELECT t.id FROM project_tasks k JOIN bot_threads t ON t.id = k.thread_id
             WHERE k.project_id = ?1 AND t.bot_id = ?2 AND t.status = 'queued'
             ORDER BY k.assigned_at, k.position LIMIT 1",
            params![project_id, bot_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(id) = next else { return vec![] };
    let claimed = conn
        .execute("UPDATE bot_threads SET status = 'working' WHERE id = ?1 AND status = 'queued'", params![id])
        .unwrap_or(0);
    if claimed != 1 {
        return vec![];
    }
    let title: String = conn
        .query_row("SELECT title FROM cowork_projects WHERE id = ?1", params![project_id], |r| r.get(0))
        .unwrap_or_default();
    match thread_routes::load_view(db, &id) {
        Ok(Some(t)) => vec![(id, coordinator_routes::kickoff_text(&title, &t))],
        _ => vec![],
    }
}

/// Every bot's queue that can move, across all projects: at startup, after
/// `interrupt_orphaned_turns` freed the bots whose turns the restart ended.
pub fn pump_all(db: &DbHandle) -> Vec<(String, String, String, Vec<(String, String)>)> {
    let Ok(conn) = db.connect() else { return vec![] };
    let lanes: Vec<(String, String, String)> = conn
        .prepare(
            "SELECT DISTINCT k.user_id, k.project_id, t.bot_id FROM project_tasks k JOIN bot_threads t ON t.id = k.thread_id
             WHERE t.status = 'queued'",
        )
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    lanes
        .into_iter()
        .filter_map(|(user, project, bot)| {
            let start = pump(db, &project, &bot);
            (!start.is_empty()).then_some((user, project, bot, start))
        })
        .collect()
}

/// Auto's pick: the team bot with the least estimated work left in this
/// project, then the fewest open threads, then team order. "Free first."
pub fn pick_bot(db: &DbHandle, project_id: &str, team: &[TeamBot]) -> Option<String> {
    let at = chrono::Utc::now();
    team.iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let items: Vec<LaneItem> = lane(db, project_id, &b.id).ok()?.into_iter().filter(|it| is_active(&it.status)).collect();
            let minutes: i64 = items.iter().filter_map(|it| remaining_min(it, at)).sum();
            Some(((minutes, items.len(), i), b.id.clone()))
        })
        .min_by_key(|(k, _)| *k)
        .map(|(_, id)| id)
}

/// Hand a task to a bot: claim the task, create the bot's thread for it in the
/// project, and start it if the bot is free. Returns the thread and what to
/// pass to `start_threads`.
pub async fn assign<R: CoordinatorRuntime>(
    db: &DbHandle,
    rt: &R,
    user_id: &str,
    project_id: &str,
    task_id: &str,
    bot_id: &str,
) -> Result<(ThreadView, Vec<(String, String)>), DelegateError> {
    project_title(db, project_id, user_id)?;
    let task = load_task(db, project_id, user_id, task_id)?;
    if task.assigned_bot_id.is_some() {
        return Err(DelegateError::AlreadyAssigned);
    }
    let team = coordinator_routes::load_team(db, project_id, user_id)?;
    if !team.iter().any(|b| b.id == bot_id) {
        return Err(DelegateError::NotOnTeam);
    }
    let claimed = db.connect()?.execute(
        "UPDATE project_tasks SET assigned_bot_id = ?2, assigned_at = ?3 WHERE id = ?1 AND assigned_bot_id IS NULL",
        params![task.id, bot_id, now()],
    )?;
    if claimed != 1 {
        return Err(DelegateError::AlreadyAssigned);
    }
    let body = CreateThreadBody {
        bot_id: bot_id.to_string(),
        title: task.title.clone(),
        project_id: Some(project_id.to_string()),
        parent_thread_id: None,
        kind: "task".into(),
        incognito: false,
        objective: Some(task.notes.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| task.title.clone())),
        success_criteria: None,
        status: Some("queued".into()),
        todo: vec![],
        created_by: Some("user".into()),
        origin: Some(json!({ "delegateTaskId": task.id })),
        session_id: None,
        depends_on: vec![],
    };
    let thread = match thread_routes::create(db, rt, user_id, body).await {
        Ok(t) => t,
        Err(e) => {
            // Put the task back so it can be handed out again.
            let _ = db.connect().map(|c| {
                c.execute("UPDATE project_tasks SET assigned_bot_id = NULL, assigned_at = NULL WHERE id = ?1", params![task.id])
            });
            return Err(DelegateError::Runtime(e));
        }
    };
    db.connect()?.execute("UPDATE project_tasks SET thread_id = ?2 WHERE id = ?1", params![task.id, thread.id])?;
    let start = pump(db, project_id, bot_id);
    let thread = thread_routes::load_view(db, &thread.id)?.unwrap_or(thread);
    Ok((thread, start))
}

/// Auto: hand every unassigned task out, free bot first. Stops at the first
/// failure and returns what started so far with the error.
pub async fn auto_assign_all<R: CoordinatorRuntime>(
    db: &DbHandle,
    rt: &R,
    user_id: &str,
    project_id: &str,
) -> (Vec<(String, String)>, Option<DelegateError>) {
    let mut start = Vec::new();
    let tasks = match unassigned(db, project_id, user_id) {
        Ok(t) => t,
        Err(e) => return (start, Some(e)),
    };
    for task in tasks {
        let team = match coordinator_routes::load_team(db, project_id, user_id) {
            Ok(t) => t,
            Err(e) => return (start, Some(e.into())),
        };
        let Some(bot) = pick_bot(db, project_id, &team) else { return (start, Some(DelegateError::NoBots)) };
        match assign(db, rt, user_id, project_id, &task.id, &bot).await {
            Ok((_, s)) => start.extend(s),
            Err(DelegateError::AlreadyAssigned) => {}
            Err(e) => return (start, Some(e)),
        }
    }
    (start, None)
}

// ─── Board ──────────────────────────────────────────────────────────────────

pub fn board(db: &DbHandle, project_id: &str, user_id: &str) -> Result<Value, DelegateError> {
    project_title(db, project_id, user_id)?;
    let at = chrono::Utc::now();
    let tasks: Vec<Value> = unassigned(db, project_id, user_id)?
        .into_iter()
        .map(|t| json!({ "id": t.id, "title": t.title, "notes": t.notes, "estimateMin": t.estimate_min }))
        .collect();
    let team = coordinator_routes::load_team(db, project_id, user_id)?;
    let mut queued_cost = 0.0;
    let mut cost_known = false;
    let mut bots = Vec::with_capacity(team.len());
    for b in &team {
        let items = lane(db, project_id, &b.id)?;
        let cost = cost_per_task_cents(db, user_id, &b.id);
        let delegated = items.iter().filter(|i| i.task_id.is_some() && is_active(&i.status)).count();
        if let Some((c, _)) = cost {
            queued_cost += c * delegated as f64;
            cost_known = cost_known || delegated > 0;
        }
        let busy: i64 = items.iter().filter(|i| is_active(&i.status)).filter_map(|i| remaining_min(i, at)).sum();
        let unknown = items.iter().filter(|i| is_active(&i.status) && i.estimate_min.is_none()).count();
        bots.push(json!({
            "id": b.id,
            "name": b.name,
            "running": items.iter().any(|i| is_running(&i.status)),
            "costPerTaskCents": cost.map(|(c, _)| (c * 100.0).round() / 100.0),
            "costSample": cost.map(|(_, n)| n).unwrap_or(0),
            "busyMinutes": busy,
            "unestimated": unknown,
            "queue": items.iter().map(|i| json!({
                "threadId": i.thread_id,
                "taskId": i.task_id,
                "title": i.title,
                "status": i.status,
                "estimateMin": i.estimate_min,
            })).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({
        "projectId": project_id,
        "auto": auto_enabled(db, project_id, user_id),
        "tasks": tasks,
        "bots": bots,
        "queuedCostCents": cost_known.then(|| (queued_cost * 100.0).round() / 100.0),
        "now": at.to_rfc3339(),
    }))
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// Run `start_threads` in the background; the caller never waits on a model.
fn spawn_start(state: &Arc<AppState>, user_id: &str, project_id: &str, start: Vec<(String, String)>) {
    if start.is_empty() {
        return;
    }
    let st = state.clone();
    let uid = user_id.to_string();
    let pid = project_id.to_string();
    tokio::spawn(async move {
        let rt = GizziCoordinator { state: st.clone() };
        coordinator_routes::start_threads(&st.db, &rt, &uid, &pid, start).await;
    });
}

async fn get_board(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(project_id): Path<String>) -> Response {
    match board(&state.db, &project_id, &user.user_id) {
        Ok(b) => Json(b).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddTaskBody {
    pub title: String,
    pub notes: Option<String>,
    pub estimate_min: Option<i64>,
}

pub fn insert_task(db: &DbHandle, project_id: &str, user_id: &str, body: &AddTaskBody) -> Result<String, DelegateError> {
    project_title(db, project_id, user_id)?;
    let title = body.title.trim();
    if title.is_empty() {
        return Err(DelegateError::Invalid("Give the task a title.".into()));
    }
    if title.chars().count() > MAX_TITLE {
        return Err(DelegateError::Invalid(format!("Keep the title under {MAX_TITLE} characters.")));
    }
    if let Some(m) = body.estimate_min {
        if !(1..=MAX_ESTIMATE_MIN).contains(&m) {
            return Err(DelegateError::Invalid("Pick an estimate between 1 minute and 24 hours.".into()));
        }
    }
    let conn = db.connect()?;
    let position: f64 = conn.query_row(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM project_tasks WHERE project_id = ?1 AND user_id = ?2",
        params![project_id, user_id],
        |r| r.get(0),
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO project_tasks (id, user_id, project_id, title, notes, estimate_min, position, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, user_id, project_id, title, body.notes.as_deref().map(str::trim).filter(|n| !n.is_empty()), body.estimate_min, position, now()],
    )?;
    Ok(id)
}

async fn add_task(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(project_id): Path<String>,
    Json(body): Json<AddTaskBody>,
) -> Response {
    let id = match insert_task(&state.db, &project_id, &user.user_id, &body) {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let mut warning = None;
    if auto_enabled(&state.db, &project_id, &user.user_id) {
        let rt = GizziCoordinator { state: state.clone() };
        let pick = coordinator_routes::load_team(&state.db, &project_id, &user.user_id)
            .ok()
            .and_then(|team| pick_bot(&state.db, &project_id, &team));
        match pick {
            Some(bot) => match assign(&state.db, &rt, &user.user_id, &project_id, &id, &bot).await {
                Ok((_, start)) => spawn_start(&state, &user.user_id, &project_id, start),
                Err(e) => warning = Some(e.message()),
            },
            None => warning = Some(DelegateError::NoBots.message()),
        }
    }
    match board(&state.db, &project_id, &user.user_id) {
        Ok(mut b) => {
            b["taskId"] = json!(id);
            if let Some(w) = warning {
                b["autoError"] = json!(w);
            }
            (StatusCode::CREATED, Json(b)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn remove_task(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((project_id, task_id)): Path<(String, String)>,
) -> Response {
    let res = (|| -> Result<Value, DelegateError> {
        let task = load_task(&state.db, &project_id, &user.user_id, &task_id)?;
        if task.assigned_bot_id.is_some() {
            return Err(DelegateError::AlreadyAssigned);
        }
        state.db.connect()?.execute("DELETE FROM project_tasks WHERE id = ?1 AND assigned_bot_id IS NULL", params![task.id])?;
        board(&state.db, &project_id, &user.user_id)
    })();
    match res {
        Ok(b) => Json(b).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignBody {
    pub bot_id: String,
}

async fn assign_task(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((project_id, task_id)): Path<(String, String)>,
    Json(body): Json<AssignBody>,
) -> Response {
    let rt = GizziCoordinator { state: state.clone() };
    match assign(&state.db, &rt, &user.user_id, &project_id, &task_id, &body.bot_id).await {
        Ok((thread, start)) => {
            spawn_start(&state, &user.user_id, &project_id, start);
            match board(&state.db, &project_id, &user.user_id) {
                Ok(mut b) => {
                    b["thread"] = json!(thread);
                    Json(b).into_response()
                }
                Err(e) => e.into_response(),
            }
        }
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct AutoBody {
    pub auto: bool,
}

async fn set_auto(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(project_id): Path<String>,
    Json(body): Json<AutoBody>,
) -> Response {
    if let Err(e) = project_title(&state.db, &project_id, &user.user_id) {
        return e.into_response();
    }
    let saved = state.db.connect().and_then(|c| {
        c.execute(
            "INSERT INTO project_delegate_settings (project_id, user_id, auto, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (project_id, user_id) DO UPDATE SET auto = excluded.auto, updated_at = excluded.updated_at",
            params![project_id, user.user_id, body.auto as i64, now()],
        )
    });
    if saved.is_err() {
        return DelegateError::Db.into_response();
    }
    let mut warning = None;
    if body.auto {
        let rt = GizziCoordinator { state: state.clone() };
        let (start, e) = auto_assign_all(&state.db, &rt, &user.user_id, &project_id).await;
        spawn_start(&state, &user.user_id, &project_id, start);
        warning = e.map(|e| e.message());
    }
    match board(&state.db, &project_id, &user.user_id) {
        Ok(mut b) => {
            if let Some(w) = warning {
                b["autoError"] = json!(w);
            }
            Json(b).into_response()
        }
        Err(e) => e.into_response(),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread_routes::ThreadRuntime;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        sessions: Mutex<usize>,
        turns: Mutex<Vec<String>>,
    }

    impl ThreadRuntime for Fake {
        async fn create_session(&self, _b: &str, _n: &str, _t: &str, _c: bool, _id: &str) -> Result<String, String> {
            let mut n = self.sessions.lock().unwrap();
            *n += 1;
            Ok(format!("sess-{n}"))
        }
        async fn seed(&self, _s: &str, _t: &str) -> Result<(), String> {
            Ok(())
        }
        async fn handoff(&self, _s: &str, _r: &str, _c: &str, _b: Option<Value>) -> Result<(String, Value), String> {
            Err("no handoff".into())
        }
    }

    impl CoordinatorRuntime for Fake {
        async fn plan(&self, _s: &str, _p: &str, _m: Option<(String, String)>) -> Option<String> {
            None
        }
        async fn send_turn(&self, s: &str, _b: &str, _t: &str) -> Result<String, String> {
            self.turns.lock().unwrap().push(s.into());
            Ok("Done.".into())
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-delegate-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let conn = state.db.connect().unwrap();
        for (id, name) in [("scout", "Scout"), ("forge", "Forge")] {
            conn.execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'u', ?2, 'm', 'p', 1, ?3)",
                params![id, name.to_lowercase(), json!({"isBot": true, "botProfile": {"displayName": name}}).to_string()],
            )
            .unwrap();
            conn.execute("INSERT INTO project_bots (project_id, bot_id, added_at) VALUES ('p1', ?1, ?2)", params![id, now()]).unwrap();
        }
        conn.execute("INSERT INTO cowork_projects (id, user_id, title) VALUES ('p1', 'u', 'Launch')", []).unwrap();
        state
    }

    fn add(state: &AppState, title: &str, est: Option<i64>) -> String {
        insert_task(&state.db, "p1", "u", &AddTaskBody { title: title.into(), notes: None, estimate_min: est }).unwrap()
    }

    fn status(state: &AppState, id: &str) -> String {
        thread_routes::load_view(&state.db, id).unwrap().unwrap().status
    }

    #[tokio::test]
    async fn a_free_bot_starts_at_once_and_a_busy_bot_queues() {
        let state = setup("queue").await;
        let rt = Fake::default();
        let a = add(&state, "Fix flaky test", Some(20));
        let b = add(&state, "Write notes", Some(10));
        let (t1, s1) = assign(&state.db, &rt, "u", "p1", &a, "scout").await.unwrap();
        assert_eq!(s1.len(), 1, "scout was free: the first task starts");
        assert_eq!(t1.status, "working");
        assert_eq!(t1.project_id.as_deref(), Some("p1"));
        let (t2, s2) = assign(&state.db, &rt, "u", "p1", &b, "scout").await.unwrap();
        assert!(s2.is_empty(), "scout is busy: the second task waits");
        assert_eq!(t2.status, "queued");
        // The running one stops (review): the queue moves.
        state.db.connect().unwrap().execute("UPDATE bot_threads SET status = 'review' WHERE id = ?1", params![t1.id]).unwrap();
        let next = pump(&state.db, "p1", "scout");
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].0, t2.id);
        assert_eq!(status(&state, &t2.id), "working");
        assert!(pump(&state.db, "p1", "scout").is_empty(), "claim makes a second pump a no-op");
    }

    #[tokio::test]
    async fn a_bot_runs_its_lane_through_on_its_own() {
        let state = setup("lane").await;
        let rt = Fake::default();
        let a = add(&state, "First", None);
        let b = add(&state, "Second", None);
        let (t1, start) = assign(&state.db, &rt, "u", "p1", &a, "scout").await.unwrap();
        let (t2, none) = assign(&state.db, &rt, "u", "p1", &b, "scout").await.unwrap();
        assert!(none.is_empty());
        // The first kickoff finishes (review) and the coordinator's next wave is the bot's next task.
        coordinator_routes::start_threads(&state.db, &rt, "u", "p1", start).await;
        let turns = rt.turns.lock().unwrap().clone();
        assert_eq!(turns, vec![t1.current_session_id.clone().unwrap(), t2.current_session_id.clone().unwrap()], "in order, one at a time");
        assert_eq!(status(&state, &t1.id), "review");
        assert_eq!(status(&state, &t2.id), "review");
    }

    #[tokio::test]
    async fn a_thread_waiting_on_you_does_not_make_the_bot_busy() {
        let state = setup("blocked").await;
        let rt = Fake::default();
        let a = add(&state, "Needs a decision", Some(30));
        let (t, _) = assign(&state.db, &rt, "u", "p1", &a, "scout").await.unwrap();
        state.db.connect().unwrap().execute("UPDATE bot_threads SET status = 'blocked' WHERE id = ?1", params![t.id]).unwrap();
        let b = board(&state.db, "p1", "u").unwrap();
        let scout = b["bots"].as_array().unwrap().iter().find(|x| x["id"] == "scout").unwrap();
        assert_eq!(scout["queue"].as_array().unwrap().len(), 1, "still shown in its lane");
        assert_eq!(scout["busyMinutes"], 0);
        assert_eq!(scout["running"], false);
        let team = coordinator_routes::load_team(&state.db, "p1", "u").unwrap();
        assert_eq!(pick_bot(&state.db, "p1", &team).as_deref(), Some("scout"), "free again for Auto (team order breaks the tie)");
    }

    #[tokio::test]
    async fn a_task_is_assigned_once_and_only_to_the_team() {
        let state = setup("once").await;
        let rt = Fake::default();
        let a = add(&state, "Update pricing page", None);
        assert_eq!(assign(&state.db, &rt, "u", "p1", &a, "stranger").await.unwrap_err(), DelegateError::NotOnTeam);
        assign(&state.db, &rt, "u", "p1", &a, "forge").await.unwrap();
        assert_eq!(assign(&state.db, &rt, "u", "p1", &a, "scout").await.unwrap_err(), DelegateError::AlreadyAssigned);
        assert_eq!(assign(&state.db, &rt, "other", "p1", &a, "scout").await.unwrap_err(), DelegateError::ProjectNotFound);
    }

    #[tokio::test]
    async fn auto_hands_out_free_bot_first() {
        let state = setup("auto").await;
        let rt = Fake::default();
        let first = add(&state, "Long job", Some(60));
        assign(&state.db, &rt, "u", "p1", &first, "scout").await.unwrap();
        add(&state, "Short job", Some(10));
        add(&state, "Another", Some(10));
        let (start, err) = auto_assign_all(&state.db, &rt, "u", "p1").await;
        assert!(err.is_none());
        assert_eq!(start.len(), 1, "forge was free and starts; the next goes behind it");
        let b = board(&state.db, "p1", "u").unwrap();
        assert_eq!(b["tasks"].as_array().unwrap().len(), 0, "everything is handed out");
        let forge = b["bots"].as_array().unwrap().iter().find(|x| x["id"] == "forge").unwrap();
        assert_eq!(forge["queue"].as_array().unwrap().len(), 2, "both short jobs went to the bot with less work");
        assert_eq!(forge["busyMinutes"], 20);
    }

    #[tokio::test]
    async fn the_board_reports_estimates_and_real_cost_only() {
        let state = setup("board").await;
        let rt = Fake::default();
        let a = add(&state, "Summarize churn calls", Some(15));
        let b = board(&state.db, "p1", "u").unwrap();
        assert_eq!(b["tasks"][0]["estimateMin"], 15);
        assert!(b["bots"][0]["costPerTaskCents"].is_null(), "no history, no number");
        assert!(b["queuedCostCents"].is_null());
        let (t, _) = assign(&state.db, &rt, "u", "p1", &a, "scout").await.unwrap();
        let conn = state.db.connect().unwrap();
        conn.execute("UPDATE bot_threads SET status = 'done' WHERE id = ?1", params![t.id]).unwrap();
        conn.execute(
            "INSERT INTO llm_usage_events (id, status, cost_microdollars, gizzi_session_id) VALUES ('e1', 'ok', 120000, ?1)",
            params![t.current_session_id],
        )
        .unwrap();
        let (cents, n) = cost_per_task_cents(&state.db, "u", "scout").unwrap();
        assert_eq!(n, 1);
        assert!((cents - 12.0).abs() < 1e-9, "120000 µ$ = 12 cents, got {cents}");
    }

    #[tokio::test]
    async fn bad_tasks_are_refused() {
        let state = setup("bad").await;
        let body = |t: &str, e: Option<i64>| AddTaskBody { title: t.into(), notes: None, estimate_min: e };
        assert!(matches!(insert_task(&state.db, "p1", "u", &body("  ", None)), Err(DelegateError::Invalid(_))));
        assert!(matches!(insert_task(&state.db, "p1", "u", &body("x", Some(0))), Err(DelegateError::Invalid(_))));
        assert_eq!(insert_task(&state.db, "nope", "u", &body("x", None)), Err(DelegateError::ProjectNotFound));
    }
}
