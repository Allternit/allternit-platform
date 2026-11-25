//! Island bar API (`/api/v1/bar/*`; Island bar + Team map, lane 15; PLAN.md
//! §2.2, §2.6, D9, D11). The person picks an app and a tool by type-ahead (no
//! model call), fills the blanks, sees a review card, then runs it. A
//! `bar_jobs` row (V295) is the unit from the review card on (D9).
//!
//! - `GET /bar/apps?q=&bot_id=` → `{apps: [{app: {kind, ref, name, icon}, level, tool_count, runnable, not_runnable_reason}], partial}`:
//!   the caller's connections (`mcp_connectors`) that list tools. `level` is the
//!   actor's: `full` for the person on their own connection; a bot's live grant
//!   (`app` when its owner never set levels; `null` when it has no grant).
//! - `GET /bar/apps/:kind/:ref/tools?q=` → `{app, tools: [{name, title, has_ui, task_capable, read_only}]}`
//!   from the pinned tool list (`mcp_server_routes::check_pins`, as the proxy's
//!   `tools/list`): a new or changed tool stays out until the owner approves it.
//! - `POST /bar/intent {app: {kind, ref}, tool, bot_id?}` →
//!   `{intent_id, app, tool: {name, title}, sentence, blanks, source}` (`bar_blanks`).
//!   `intent_id` is stateless (app + tool); review checks everything again.
//! - `GET /bar/recents?app=&tool=&field=` → `{recents: [...]}`;
//!   `DELETE /bar/recents?app=&tool=&field=` → `{deleted}`. `app` is `<kind>:<ref>`
//!   or a bare connector id. Recents are written on run only
//!   (`record_recent_values`, called by lane 17's run).
//! - `POST /bar/review {intent_id, args, bot_id?}` → `{job_id, state, level,
//!   needs_confirmation, rule: {id, text}|null, approver: {user_id, name, is_you}, lines}`,
//!   job state `review`. Level, rule and approver come from
//!   `team_rules::preview_call` (the gate's own helpers, read only: no ask is
//!   opened here; the run opens it). `needs_confirmation` = the install mode
//!   asks for this tool, or a rule matched whose approver is the caller.
//! - `GET /bar/jobs?state=active|all|<state>` → `{jobs}`; `GET /bar/jobs/:id` → `{job}`.
//!
//! - `POST /bar/jobs/:id/run` → `{job}` (lane 17). Only the job's person, from
//!   `review`. The team rule gate (`team_rules::gate_call`, the proxy's own
//!   gate, nothing spent) decides: a rule routed to someone else opens (or
//!   reuses) the routed ask, ledgers `approval.requested` for the approver (the
//!   cloud push every approval uses), and the job waits in `waiting_approval`;
//!   otherwise the job is `working` and the engine calls the tool in session
//!   `bar:<job_id>` (`agent_session_routes::engine_tool_call`, lane 16) in the
//!   background with `_meta {allternit/jobId, allternit/approved, allternit/approvalId?}`.
//!   The answer is the receipt (`result_json = {structured, text}`), then
//!   `completed` or `failed`. Recents are written on run. No engine → 409 `no_runtime`.
//! - Approval: the approver decides the routed ask on its own route
//!   (`cowork_routes::decide_approval`), which calls [`on_routed_decision`]:
//!   approved → `working` and the call runs with `allternit/approvalId`;
//!   rejected or withdrawn → `failed` "not approved"; past 24 h → `failed`
//!   "not approved in time" (also found on read by [`expire_overdue`]).
//! - `GET /bar/jobs/:id/events` (SSE, requester or approver): the job first as
//!   `{type: bar_job, job}`, then the engine's MCP events for session
//!   `bar:<job_id>` unchanged (`{type: mcp_task|mcp_elicitation, session_id, part}`,
//!   `{type: mcp_elicitation_resolved, session_id, request_id, action}`) and
//!   `{type: bar_job, job}` on every state change. Ends after a finished job.
//! - `POST /bar/jobs/:id/cancel` → `{job}`: cancels the running MCP task (the
//!   engine's cancel route, then the call is dropped), withdraws a pending routed
//!   ask, and sets `cancelled`. 409 `job_finished` once finished.
//! - A finished job (completed, failed, cancelled) keeps no secrets: the
//!   arguments' secret fields (`bar_blanks::is_secret_field`: `format: password`,
//!   `writeOnly`, secret names; from the pinned schema) are replaced by `••••`.
//!   `job_json` masks secret-named fields while the job is still active.
//!
//! A 0-bot account works: with no `bot_id` the person is the actor.
//!
//! App kinds (lane 15b): `connector` (the caller's connections), `vendor`
//! (cloud `vendor_apps`: its tools are its connection's pinned tools named in
//! its allow-list or opening its `ui://` resource; level ≤ the install's
//! `granted_tier`, never above `app`; runs through its connection's proxy entry,
//! as the Apps+MCP vendor app UI does) and `artifact` (tools = the Apps+MCP
//! mirror tools; listed with `runnable: false` because they need the open app's
//! signed binding). Vendor and artifact apps come from the owner's cloud shelf
//! (`GET /api/v1/apps` + `GET /api/v1/vendor-apps`, signed); `/bar/apps` rows
//! carry `runnable` and `not_runnable_reason`, and `partial: ["app_shelf"]`
//! when the shelf could not be read. Intent, review and run answer 409
//! `app_not_runnable` for an app that cannot run here; 503
//! `app_shelf_unavailable` when the shelf cannot be read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::BoxStream;
use futures::StreamExt;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::account_sync::{AccountCloud, SignedAccountCloud};
use crate::auth::AuthUser;
use crate::bar_blanks::{self, BlankType, RecentValue};
use crate::db::DbHandle;
use crate::mcp_apps::{self, AppsError, Connector};
use crate::team_routes;
use crate::team_rules::{self, Decision};
use crate::team_store::{AppKind, ApprovalRule, GrantLevel};
use crate::AppState;

/// Answer when no Allternit runtime can run the job (lane 17's run path).
pub const NO_RUNTIME: &str = "no_runtime";
const LIST_BUDGET: Duration = Duration::from_secs(8);
const RECENTS_PER_FIELD: i64 = 8;
const JOBS_LIMIT: i64 = 100;
const INTENT_PREFIX: &str = "bin_";
const NOW_SQL: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

pub fn bar_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bar/apps", get(apps_route))
        .route("/bar/apps/:kind/:app_ref/tools", get(tools_route))
        .route("/bar/intent", post(intent_route))
        .route("/bar/recents", get(recents_get).delete(recents_delete))
        .route("/bar/review", post(review_route))
        .route("/bar/jobs", get(jobs_route))
        .route("/bar/jobs/:id", get(job_route))
        .route("/bar/jobs/:id/run", post(run_route))
        .route("/bar/jobs/:id/events", get(events_route))
        .route("/bar/jobs/:id/cancel", post(cancel_route))
}

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    let message: String = message.into();
    (status, Json(json!({ "error": message, "code": code }))).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    warn!(error = %e, "bar routes: internal error");
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal", "something went wrong; try again")
}

/// 409 `no_runtime`: nothing can run the job right now. Clients show the
/// connect kit state.
pub fn no_runtime() -> Response {
    err(StatusCode::CONFLICT, NO_RUNTIME, "No Allternit computer is reachable to run this. Connect one and try again.")
}

async fn with_db<T: Send + 'static>(
    state: &Arc<AppState>,
    f: impl FnOnce(&Connection) -> Result<T, Response> + Send + 'static,
) -> Result<T, Response> {
    with_handle(&state.db, f).await
}

async fn with_handle<T: Send + 'static>(
    db: &DbHandle,
    f: impl FnOnce(&Connection) -> Result<T, Response> + Send + 'static,
) -> Result<T, Response> {
    let db = db.clone();
    match tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(internal)?;
        f(&conn)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => Err(internal(e)),
    }
}

// ── Jobs (bar_jobs) ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Review,
    WaitingApproval,
    Working,
    InputRequired,
    Completed,
    Failed,
    Cancelled,
}

impl JobState {
    pub const ACTIVE: [JobState; 3] = [JobState::WaitingApproval, JobState::Working, JobState::InputRequired];

    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Review => "review",
            JobState::WaitingApproval => "waiting_approval",
            JobState::Working => "working",
            JobState::InputRequired => "input_required",
            JobState::Completed => "completed",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            JobState::Review,
            JobState::WaitingApproval,
            JobState::Working,
            JobState::InputRequired,
            JobState::Completed,
            JobState::Failed,
            JobState::Cancelled,
        ]
        .into_iter()
        .find(|st| st.as_str() == s)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, JobState::Completed | JobState::Failed | JobState::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BarJob {
    pub id: String,
    pub user_id: String,
    pub bot_id: Option<String>,
    pub app_kind: AppKind,
    pub app_ref: String,
    pub tool: String,
    pub args: Value,
    pub level: Option<GrantLevel>,
    pub rule_id: Option<String>,
    pub approver_user_id: Option<String>,
    pub approval_id: Option<String>,
    pub task_id: Option<String>,
    pub session_id: String,
    pub state: JobState,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Joined from the app's own table (connector name); not a column.
    pub app_name: Option<String>,
}

const JOB_COLS: &str = "j.id, j.user_id, j.bot_id, j.app_kind, j.app_ref, j.tool, j.args_json, j.level, j.rule_id, \
     j.approver_user_id, j.approval_id, j.task_id, j.session_id, j.state, j.result_json, j.error, j.created_at, \
     j.updated_at, j.started_at, j.finished_at, c.name";
const JOB_FROM: &str = "bar_jobs j LEFT JOIN mcp_connectors c ON j.app_kind = 'connector' AND c.id = j.app_ref";

fn bad_col(idx: usize, s: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, format!("unexpected value {s:?}").into())
}

fn job_from_row(r: &Row<'_>) -> rusqlite::Result<BarJob> {
    let kind: String = r.get(3)?;
    let level: Option<String> = r.get(7)?;
    let state: String = r.get(13)?;
    let args: String = r.get(6)?;
    let result: Option<String> = r.get(14)?;
    Ok(BarJob {
        id: r.get(0)?,
        user_id: r.get(1)?,
        bot_id: r.get(2)?,
        app_kind: AppKind::parse(&kind).ok_or_else(|| bad_col(3, &kind))?,
        app_ref: r.get(4)?,
        tool: r.get(5)?,
        args: serde_json::from_str(&args).unwrap_or_else(|_| json!({})),
        level: level.as_deref().and_then(GrantLevel::parse),
        rule_id: r.get(8)?,
        approver_user_id: r.get(9)?,
        approval_id: r.get(10)?,
        task_id: r.get(11)?,
        session_id: r.get(12)?,
        state: JobState::parse(&state).ok_or_else(|| bad_col(13, &state))?,
        result: result.and_then(|s| serde_json::from_str(&s).ok()),
        error: r.get(15)?,
        created_at: r.get(16)?,
        updated_at: r.get(17)?,
        started_at: r.get(18)?,
        finished_at: r.get(19)?,
        app_name: r.get(20)?,
    })
}

/// The job as every surface reads it (`GET /bar/jobs*`, lane 17's `bar_job` event).
pub fn job_json(job: &BarJob) -> Value {
    json!({
        "id": job.id,
        "user_id": job.user_id,
        "bot_id": job.bot_id,
        "app": { "kind": job.app_kind.as_str(), "ref": job.app_ref, "name": job.app_name },
        "tool": job.tool,
        "title": crate::mcp_server_routes::tool_label(&job.tool),
        // Secret-named fields never leave the runtime (a finished job's are already redacted).
        "args": redact_secrets(&job.args, &Value::Null),
        "level": job.level.map(GrantLevel::as_str),
        "rule_id": job.rule_id,
        "approver_user_id": job.approver_user_id,
        "approval_id": job.approval_id,
        "task_id": job.task_id,
        "session_id": job.session_id,
        "state": job.state.as_str(),
        "result": job.result,
        "error": job.error,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "started_at": job.started_at,
        "finished_at": job.finished_at,
    })
}

pub struct NewJob<'a> {
    pub user_id: &'a str,
    pub bot_id: Option<&'a str>,
    pub app_kind: AppKind,
    pub app_ref: &'a str,
    pub tool: &'a str,
    pub args: &'a Value,
    pub level: Option<GrantLevel>,
    pub rule_id: Option<&'a str>,
    pub approver_user_id: Option<&'a str>,
}

/// Insert a job in state `review`. Returns its id (`bjb_…`); its session is `bar:<id>`.
pub fn insert_job(conn: &Connection, job: &NewJob<'_>) -> rusqlite::Result<String> {
    let id = format!("bjb_{}", uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO bar_jobs (id, user_id, bot_id, app_kind, app_ref, tool, args_json, level, rule_id,
                               approver_user_id, session_id, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'review')",
        params![
            id,
            job.user_id,
            job.bot_id,
            job.app_kind.as_str(),
            job.app_ref,
            job.tool,
            job.args.to_string(),
            job.level.map(GrantLevel::as_str),
            job.rule_id,
            job.approver_user_id,
            format!("bar:{id}"),
        ],
    )?;
    Ok(id)
}

pub fn get_job(conn: &Connection, id: &str) -> rusqlite::Result<Option<BarJob>> {
    conn.query_row(&format!("SELECT {JOB_COLS} FROM {JOB_FROM} WHERE j.id = ?1"), params![id], job_from_row).optional()
}

/// The caller's jobs in `states` (all states when empty), newest change first.
pub fn list_jobs(conn: &Connection, user_id: &str, states: &[JobState]) -> rusqlite::Result<Vec<BarJob>> {
    let filter = if states.is_empty() {
        String::new()
    } else {
        format!(" AND j.state IN ({})", states.iter().map(|s| format!("'{}'", s.as_str())).collect::<Vec<_>>().join(","))
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {JOB_COLS} FROM {JOB_FROM} WHERE j.user_id = ?1{filter} ORDER BY j.updated_at DESC, j.id LIMIT {JOBS_LIMIT}"
    ))?;
    let rows = stmt.query_map(params![user_id], job_from_row)?.collect();
    rows
}

/// Fields lane 17 sets as a job moves on; `None` leaves the column as it is.
#[derive(Debug, Default)]
pub struct JobUpdate {
    pub approval_id: Option<String>,
    pub task_id: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// Move a job to `state`. Leaving `review` stamps `started_at`; a terminal
/// state stamps `finished_at`. A finished job never changes again: `false`.
pub fn set_job_state(conn: &Connection, id: &str, state: JobState, update: &JobUpdate) -> rusqlite::Result<bool> {
    let n = conn.execute(
        &format!(
            "UPDATE bar_jobs SET state = ?2, updated_at = {NOW_SQL},
                 started_at = CASE WHEN started_at IS NULL AND ?2 <> 'review' THEN {NOW_SQL} ELSE started_at END,
                 finished_at = CASE WHEN ?2 IN ('completed', 'failed', 'cancelled') THEN COALESCE(finished_at, {NOW_SQL}) ELSE finished_at END,
                 approval_id = COALESCE(?3, approval_id), task_id = COALESCE(?4, task_id),
                 result_json = COALESCE(?5, result_json), error = COALESCE(?6, error)
             WHERE id = ?1 AND state NOT IN ('completed', 'failed', 'cancelled')"
        ),
        params![
            id,
            state.as_str(),
            update.approval_id,
            update.task_id,
            update.result.as_ref().map(Value::to_string),
            update.error,
        ],
    )?;
    Ok(n > 0)
}

// ── Recent values (bar_recent_values) ────────────────────────────────────────

/// Keep the run's blank values as recents (lane 17 calls this on run, never
/// earlier). Only top-level flat fields of `input_schema` that are not secret
/// (`bar_blanks::recordable_fields`: no `format: password`, no `writeOnly`, no
/// secret-named field) and whose value is one line of text. No schema, no
/// recents. Keeps the 8 newest per field. Returns how many values were kept.
pub fn record_recent_values(conn: &Connection, job: &BarJob, input_schema: &Value) -> rusqlite::Result<usize> {
    let blanks = bar_blanks::blanks_for_schema(input_schema, "", None).blanks;
    let mut kept = 0;
    for field in bar_blanks::recordable_fields(input_schema) {
        let value = &job.args[field.as_str()];
        let Some(text) = bar_blanks::display_value(value) else { continue };
        // An enum value is shown with its option label.
        let label = blanks
            .iter()
            .find(|b| b.key == field)
            .and_then(|b| b.options.as_ref())
            .and_then(|opts| opts.iter().find(|o| &o.value == value))
            .map(|o| o.label.clone())
            .unwrap_or(text);
        conn.execute(
            &format!(
                "INSERT INTO bar_recent_values (user_id, app_kind, app_ref, tool, field, value_json, label, used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, {NOW_SQL})
                 ON CONFLICT (user_id, app_kind, app_ref, tool, field, value_json)
                 DO UPDATE SET used_at = excluded.used_at, label = excluded.label"
            ),
            params![job.user_id, job.app_kind.as_str(), job.app_ref, job.tool, field, value.to_string(), label],
        )?;
        conn.execute(
            "DELETE FROM bar_recent_values
             WHERE user_id = ?1 AND app_kind = ?2 AND app_ref = ?3 AND tool = ?4 AND field = ?5
               AND rowid NOT IN (
                   SELECT rowid FROM bar_recent_values
                   WHERE user_id = ?1 AND app_kind = ?2 AND app_ref = ?3 AND tool = ?4 AND field = ?5
                   ORDER BY used_at DESC, rowid DESC LIMIT ?6)",
            params![job.user_id, job.app_kind.as_str(), job.app_ref, job.tool, field, RECENTS_PER_FIELD],
        )?;
        kept += 1;
    }
    Ok(kept)
}

/// Recents per field for one tool, newest first, at most 8 per field.
fn recents_for_tool(
    conn: &Connection,
    user_id: &str,
    kind: AppKind,
    app_ref: &str,
    tool: &str,
) -> rusqlite::Result<Vec<(String, RecentValue)>> {
    let mut stmt = conn.prepare(
        "SELECT field, value_json, label, used_at FROM bar_recent_values
         WHERE user_id = ?1 AND app_kind = ?2 AND app_ref = ?3 AND tool = ?4
         ORDER BY used_at DESC, rowid DESC",
    )?;
    let rows = stmt.query_map(params![user_id, kind.as_str(), app_ref, tool], |r| {
        let raw: String = r.get(1)?;
        let value: Value = serde_json::from_str(&raw).unwrap_or(Value::String(raw));
        let label: Option<String> = r.get(2)?;
        Ok((
            r.get::<_, String>(0)?,
            RecentValue {
                label: label.or_else(|| bar_blanks::display_value(&value)).unwrap_or_default(),
                value,
                used_at: r.get(3)?,
            },
        ))
    })?;
    rows.collect()
}

/// `?app=` as `<kind>:<ref>` or a bare connector id.
fn parse_app_param(app: &str) -> Option<(AppKind, String)> {
    match app.split_once(':') {
        Some((kind, r)) if AppKind::parse(kind).is_some() && !r.is_empty() => Some((AppKind::parse(kind)?, r.to_string())),
        _ if !app.trim().is_empty() => Some((AppKind::Connector, app.trim().to_string())),
        _ => None,
    }
}

#[derive(Debug, Deserialize, Default)]
struct RecentsQuery {
    app: Option<String>,
    tool: Option<String>,
    field: Option<String>,
}

/// `WHERE` for the recents routes: the caller, then each filter given.
fn recents_filter(user_id: &str, q: &RecentsQuery) -> Result<(String, Vec<String>), Response> {
    let mut sql = "user_id = ?1".to_string();
    let mut args = vec![user_id.to_string()];
    if let Some(app) = q.app.as_deref().filter(|a| !a.trim().is_empty()) {
        let (kind, r) = parse_app_param(app).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_request", "app must be <kind>:<ref>"))?;
        args.push(kind.as_str().to_string());
        sql.push_str(&format!(" AND app_kind = ?{}", args.len()));
        args.push(r);
        sql.push_str(&format!(" AND app_ref = ?{}", args.len()));
    }
    for (col, v) in [("tool", &q.tool), ("field", &q.field)] {
        if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
            args.push(v.to_string());
            sql.push_str(&format!(" AND {col} = ?{}", args.len()));
        }
    }
    Ok((sql, args))
}

async fn recents_get(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<RecentsQuery>) -> Response {
    let (filter, args) = match recents_filter(&user.user_id, &q) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let listed = with_db(&state, move |conn| {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT app_kind, app_ref, tool, field, value_json, label, used_at FROM bar_recent_values
                 WHERE {filter} ORDER BY used_at DESC, rowid DESC LIMIT 200"
            ))
            .map_err(internal)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                let raw: String = r.get(4)?;
                Ok(json!({
                    "app": { "kind": r.get::<_, String>(0)?, "ref": r.get::<_, String>(1)? },
                    "tool": r.get::<_, String>(2)?,
                    "field": r.get::<_, String>(3)?,
                    "value": serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw)),
                    "label": r.get::<_, Option<String>>(5)?,
                    "used_at": r.get::<_, String>(6)?,
                }))
            })
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(internal)?;
        Ok(rows)
    })
    .await;
    match listed {
        Ok(recents) => Json(json!({ "recents": recents })).into_response(),
        Err(r) => r,
    }
}

async fn recents_delete(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<RecentsQuery>) -> Response {
    let (filter, args) = match recents_filter(&user.user_id, &q) {
        Ok(f) => f,
        Err(r) => return r,
    };
    match with_db(&state, move |conn| {
        conn.execute(&format!("DELETE FROM bar_recent_values WHERE {filter}"), rusqlite::params_from_iter(args.iter()))
            .map_err(internal)
    })
    .await
    {
        Ok(n) => Json(json!({ "deleted": n })).into_response(),
        Err(r) => r,
    }
}

// ── Apps and tools ───────────────────────────────────────────────────────────

fn bot_owned(conn: &Connection, user_id: &str, bot_id: &str) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1 AND user_id = ?2)", params![bot_id, user_id], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n != 0)
}

/// `None` = the person acts. A bot must be the caller's own.
async fn check_bot(state: &Arc<AppState>, user_id: &str, bot_id: Option<&str>) -> Result<Option<String>, Response> {
    let Some(bot) = bot_id.map(str::trim).filter(|b| !b.is_empty()).map(str::to_string) else {
        return Ok(None);
    };
    let (uid, b) = (user_id.to_string(), bot.clone());
    let owned = with_db(state, move |conn| bot_owned(conn, &uid, &b).map_err(internal)).await?;
    if owned {
        Ok(Some(bot))
    } else {
        Err(err(StatusCode::NOT_FOUND, "bot_not_found", "bot not found"))
    }
}

/// The actor's level on an app (see the module docs). `None` = a bot with no grant.
fn actor_level(bot_id: Option<&str>, preview: &team_rules::Preview) -> Option<GrantLevel> {
    match bot_id {
        None => Some(GrantLevel::Full),
        Some(_) if preview.denied == Some(Decision::DenyNoGrant) => None,
        Some(_) => Some(preview.level.unwrap_or(GrantLevel::App)),
    }
}

fn unsupported_kind(kind: &str) -> Response {
    err(StatusCode::NOT_FOUND, "app_kind_unsupported", format!("the bar does not serve '{kind}' apps yet"))
}

// ── Vendor and artifact apps (the owner's cloud shelf) ──────────────────────

/// The owner's vendor app records (`{apps: [{id, connector_id, ui_resource,
/// tool_allowlist, …}]}`, cloud-api `routes/apps.rs`). Signed like the shelf.
pub const CLOUD_VENDOR_APPS_PATH: &str = "/api/v1/vendor-apps";
/// `partial[]` entry on `/bar/apps` when the cloud shelf could not be read.
pub const PARTIAL_APP_SHELF: &str = "app_shelf";
/// Why an artifact app's tools are listed but not run. Its tools (the Apps+MCP
/// mirror tools `app_storage_*` / `app_ai_complete`) run on the MCP edge and
/// only with the signed app binding the open app hands out
/// (`mcp_edge_apps.rs`); the runtime has no route that mints one.
pub const ARTIFACT_NOT_RUNNABLE: &str =
    "An artifact app's tools run inside the open app, which signs each call; this computer cannot make that call yet. Open the app to use them.";

/// A vendor or artifact app from the owner's cloud shelf (`GET /api/v1/apps`),
/// joined with its vendor record (`GET /api/v1/vendor-apps`).
#[derive(Debug, Clone, PartialEq)]
pub struct CloudApp {
    pub kind: AppKind,
    pub app_ref: String,
    pub name: String,
    pub icon: Option<String>,
    /// The most the person may do with it: the install's `granted_tier`, a
    /// vendor app never above `app` (`team_routes::install_ceiling`). `None` = not installed.
    pub ceiling: Option<GrantLevel>,
    /// Vendor apps: the connection the app's tools run through.
    pub connector_id: Option<String>,
    /// Vendor apps: the app's own `ui://` resource.
    pub ui_resource: Option<String>,
    /// Vendor apps: tool names the app may call.
    pub tool_allowlist: Vec<String>,
}

/// Read the shelf for `user_id`. The signed cloud client answers for this
/// computer's paired owner only, so anyone else gets `Err` (fail closed).
pub async fn read_cloud_apps(cloud: &dyn AccountCloud, paired_owner: Option<&str>, user_id: &str) -> Result<Vec<CloudApp>, String> {
    if paired_owner != Some(user_id) {
        return Err("this computer reads apps for its own owner only".into());
    }
    let shelf = team_routes::fetch_shelf(cloud).await?;
    let records: Vec<Value> = if shelf.iter().any(|a| a.kind == AppKind::Vendor) {
        let (status, body) = cloud.send("GET", CLOUD_VENDOR_APPS_PATH, Vec::new()).await?;
        if !(200..300).contains(&status) {
            return Err(format!("cloud answered {status}"));
        }
        let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        v["apps"].as_array().cloned().ok_or("cloud answer has no apps[]")?
    } else {
        Vec::new()
    };
    Ok(shelf
        .iter()
        .map(|a| {
            let rec = records.iter().find(|r| a.kind == AppKind::Vendor && r["id"].as_str() == Some(a.app_ref.as_str()));
            let text = |key: &str| rec.and_then(|r| r[key].as_str()).map(str::to_string);
            CloudApp {
                kind: a.kind,
                app_ref: a.app_ref.clone(),
                name: a.name.clone().unwrap_or_else(|| a.app_ref.clone()),
                icon: a.icon.clone(),
                ceiling: team_routes::install_ceiling(a),
                connector_id: text("connector_id"),
                ui_resource: text("ui_resource"),
                tool_allowlist: rec
                    .and_then(|r| r["tool_allowlist"].as_array())
                    .map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
            }
        })
        .collect())
}

/// A vendor app's tools are its connection's tools that it may call: named in
/// its allow-list, or opening its own `ui://` resource.
pub fn vendor_tool_allowed(app: &CloudApp, tool: &Value) -> bool {
    let name = tool["name"].as_str().unwrap_or_default();
    app.tool_allowlist.iter().any(|t| t == name)
        || app.ui_resource.as_deref().is_some_and(|u| mcp_apps::ui_resource_uri(tool).as_deref() == Some(u))
}

/// An artifact app's tools: the Apps+MCP mirror tools, without the `app` and
/// `binding` fields the open app fills in itself.
pub fn artifact_tools() -> Vec<Value> {
    let scope = json!({ "type": "string", "enum": ["user", "shared"], "default": "user" });
    let key = json!({ "type": "string" });
    let tool = |name: &str, title: &str, read_only: bool, properties: Value, required: Vec<&str>| {
        json!({
            "name": name,
            "title": title,
            "annotations": { "readOnlyHint": read_only },
            "inputSchema": { "type": "object", "properties": properties, "required": required },
        })
    };
    vec![
        tool("app_storage_get", "Read a stored value", true, json!({ "scope": scope, "key": key }), vec!["key"]),
        tool("app_storage_set", "Store a value", false, json!({ "scope": scope, "key": key, "value": { "type": "string" } }), vec!["key", "value"]),
        tool("app_storage_delete", "Delete a stored value", false, json!({ "scope": scope, "key": key }), vec!["key"]),
        tool("app_storage_list", "List stored keys", true, json!({ "scope": scope, "prefix": { "type": "string" } }), vec![]),
        tool(
            "app_ai_complete",
            "Ask the app's AI",
            false,
            json!({ "prompt": { "type": "string" }, "system": { "type": "string" }, "max_tokens": { "type": "integer" } }),
            vec!["prompt"],
        ),
    ]
}

/// Why a shelf app cannot run from the bar (`None` = it can). Vendor apps need
/// an install and their connection on this computer (`connector_here`).
pub fn cloud_app_not_runnable(app: &CloudApp, connector_here: bool) -> Option<String> {
    match app.kind {
        AppKind::Artifact => Some(ARTIFACT_NOT_RUNNABLE.to_string()),
        AppKind::Vendor if app.ceiling.is_none() => Some(format!("Install {} first.", app.name)),
        AppKind::Vendor if app.connector_id.is_none() => Some(format!("{} has no connection recorded.", app.name)),
        AppKind::Vendor if !connector_here => Some(format!("{}'s connection is not on this computer.", app.name)),
        _ => None,
    }
}

/// An app the bar serves, resolved for one caller.
#[derive(Clone)]
pub struct BarApp {
    pub kind: AppKind,
    pub app_ref: String,
    pub name: String,
    pub icon: Option<String>,
    /// The connection the tools run through (connector and runnable vendor apps).
    pub connector: Option<Connector>,
    /// Vendor and artifact apps: the shelf row.
    pub cloud: Option<CloudApp>,
    /// Why it cannot run here; `None` = runnable.
    pub not_runnable: Option<String>,
}

impl BarApp {
    pub fn app_json(&self) -> Value {
        json!({ "kind": self.kind.as_str(), "ref": self.app_ref, "name": self.name, "icon": self.icon })
    }

    /// The install's cap on the caller's level (`None` = no cap: a connection).
    pub fn ceiling(&self) -> Option<GrantLevel> {
        self.cloud.as_ref().and_then(|c| c.ceiling)
    }
}

fn paired_owner() -> Option<String> {
    crate::relay_auth::process_secret().paired_owner()
}

fn app_not_found() -> Response {
    err(StatusCode::NOT_FOUND, "app_not_found", "app not found")
}

fn app_not_runnable(why: &str) -> Response {
    err(StatusCode::CONFLICT, "app_not_runnable", why)
}

/// Resolve `(kind, ref)` for the caller: a connection from the registry; a
/// vendor or artifact app from the cloud shelf (503 `app_shelf_unavailable`
/// when it cannot be read).
async fn resolve_app(state: &Arc<AppState>, user_id: &str, kind: AppKind, app_ref: &str) -> Result<BarApp, Response> {
    if kind == AppKind::Connector {
        let c = load_connector(state, user_id, app_ref).await?;
        return Ok(BarApp {
            kind,
            app_ref: c.id.clone(),
            name: c.name.clone(),
            icon: None,
            connector: Some(c),
            cloud: None,
            not_runnable: None,
        });
    }
    let apps = read_cloud_apps(&SignedAccountCloud, paired_owner().as_deref(), user_id)
        .await
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, "app_shelf_unavailable", format!("Could not read your apps: {e}")))?;
    let app = apps.into_iter().find(|a| a.kind == kind && a.app_ref == app_ref).ok_or_else(app_not_found)?;
    let connector = match (kind, app.connector_id.as_deref()) {
        (AppKind::Vendor, Some(cid)) => {
            mcp_apps::load_connector(state, user_id, cid, mcp_apps::allow_private_hosts()).await.map_err(IntoResponse::into_response)?
        }
        _ => None,
    };
    Ok(BarApp {
        kind,
        app_ref: app.app_ref.clone(),
        name: app.name.clone(),
        icon: app.icon.clone(),
        not_runnable: cloud_app_not_runnable(&app, connector.is_some()),
        connector,
        cloud: Some(app),
    })
}

/// The app's tools the caller may use.
async fn app_tools(state: &Arc<AppState>, user_id: &str, app: &BarApp) -> Result<Vec<Value>, Response> {
    if app.kind == AppKind::Artifact {
        return Ok(artifact_tools());
    }
    let Some(connector) = &app.connector else {
        return Ok(Vec::new());
    };
    let tools = pinned_tools(state, user_id, connector).await.map_err(IntoResponse::into_response)?;
    Ok(match &app.cloud {
        Some(vendor) => tools.into_iter().filter(|t| vendor_tool_allowed(vendor, t)).collect(),
        None => tools,
    })
}

/// One of the app's tools by name. 404 `tool_not_found` otherwise.
async fn app_tool(state: &Arc<AppState>, user_id: &str, app: &BarApp, tool: &str) -> Result<Value, Response> {
    app_tools(state, user_id, app)
        .await?
        .into_iter()
        .find(|t| t["name"].as_str() == Some(tool))
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "tool_not_found", format!("{} has no tool '{tool}' you can use", app.name)))
}

/// The caller's level on an app: the actor's level, capped by the install.
fn capped_level(level: Option<GrantLevel>, ceiling: Option<GrantLevel>) -> Option<GrantLevel> {
    match (level, ceiling) {
        (Some(l), Some(c)) => Some(l.min(c)),
        (l, _) => l,
    }
}

async fn load_connector(state: &Arc<AppState>, user_id: &str, connector_id: &str) -> Result<Connector, Response> {
    match mcp_apps::load_connector(state, user_id, connector_id, mcp_apps::allow_private_hosts()).await {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err(err(StatusCode::NOT_FOUND, "connector_not_found", "connector not found")),
        Err(e) => Err(e.into_response()),
    }
}

/// The connector's tools the caller may use: listed live, visible to the model,
/// and pinned (`mcp_server_routes::check_pins`, the proxy's own `tools/list`
/// filter; a server's first listing pins what it lists).
pub(crate) async fn pinned_tools(state: &Arc<AppState>, user_id: &str, connector: &Connector) -> Result<Vec<Value>, AppsError> {
    let allow_private = mcp_apps::allow_private_hosts();
    let listing = async {
        let client = match mcp_apps::open_session(connector, allow_private).await {
            // A managed token the server stopped accepting: re-grant once (as the proxy does).
            Err(e) if e.code == "connector_unauthorized" && connector.is_managed() && !connector.auth_failed() => {
                let mut fresh = connector.clone();
                mcp_apps::regrant_managed(state, &mut fresh, allow_private).await;
                mcp_apps::open_session(&fresh, allow_private).await?
            }
            other => other?,
        };
        let tools = mcp_apps::list_all_tools(&client).await;
        mcp_apps::close_session(client).await;
        tools
    };
    let tools = tokio::time::timeout(LIST_BUDGET, listing)
        .await
        .map_err(|_| AppsError::new(StatusCode::GATEWAY_TIMEOUT, "app_timeout", format!("{} did not answer in time", connector.name)))??;
    let visible: Vec<Value> = tools.into_iter().filter(mcp_apps::tool_visible_to_model).collect();
    let (db, uid, cid) = (state.db.clone(), user_id.to_string(), connector.id.clone());
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| AppsError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))?;
        let allowed = crate::mcp_server_routes::check_pins(&conn, &uid, &cid, &visible).0;
        Ok::<Vec<Value>, AppsError>(
            visible
                .into_iter()
                .filter(|t| t["name"].as_str().is_some_and(|n| allowed.iter().any(|a| a == n)))
                .collect::<Vec<Value>>(),
        )
    })
    .await
    .map_err(|e| AppsError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))?
}

fn tool_card(tool: &Value) -> Value {
    json!({
        "name": tool["name"],
        "title": bar_blanks::tool_title(tool),
        "has_ui": mcp_apps::ui_resource_uri(tool).is_some_and(|u| u.starts_with("ui://")),
        "task_capable": matches!(tool["execution"]["taskSupport"].as_str(), Some("optional" | "required")),
        "read_only": tool["annotations"]["readOnlyHint"] == true,
    })
}

fn matches_q(q: &str, hay: &[&str]) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || hay.iter().any(|h| h.to_lowercase().contains(&q))
}

#[derive(Debug, Deserialize, Default)]
struct AppsQuery {
    q: Option<String>,
    bot_id: Option<String>,
}

async fn apps_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<AppsQuery>) -> Response {
    let bot = match check_bot(&state, &user.user_id, q.bot_id.as_deref()).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let query = q.q.unwrap_or_default();
    let connectors: Vec<Connector> = mcp_apps::load_user_connectors(&state, &user.user_id, mcp_apps::allow_private_hosts())
        .await
        .into_iter()
        .filter(|c| matches_q(&query, &[c.name.as_str(), c.name_id.as_str()]))
        .collect();

    // Tool count from the pins; a connector never listed yet is listed now (which pins it).
    let (uid, ids, b) = (user.user_id.clone(), connectors.iter().map(|c| c.id.clone()).collect::<Vec<_>>(), bot.clone());
    let known = match with_db(&state, move |conn| {
        ids.iter()
            .map(|id| -> Result<(i64, Option<GrantLevel>), Response> {
                let pinned: i64 = conn
                    .query_row("SELECT COUNT(*) FROM mcp_tool_pins WHERE owner = ?1 AND server_id = ?2", params![uid, id], |r| r.get(0))
                    .map_err(internal)?;
                let preview = team_rules::preview_call(conn, &uid, b.as_deref(), AppKind::Connector, id, "", true, &Value::Null)
                    .map_err(internal)?;
                Ok((pinned, actor_level(b.as_deref(), &preview)))
            })
            .collect::<Result<Vec<_>, Response>>()
    })
    .await
    {
        Ok(k) => k,
        Err(r) => return r,
    };
    let counts = futures::future::join_all(connectors.iter().zip(&known).map(|(c, (pinned, _))| {
        let state = state.clone();
        let uid = user.user_id.clone();
        async move {
            if *pinned > 0 {
                return *pinned as usize;
            }
            pinned_tools(&state, &uid, c).await.map(|t| t.len()).unwrap_or(0)
        }
    }))
    .await;

    let mut apps: Vec<Value> = connectors
        .iter()
        .zip(known)
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|((c, (_, level)), n)| {
            json!({
                "app": { "kind": "connector", "ref": c.id, "name": c.name, "icon": Value::Null },
                "level": level.map(GrantLevel::as_str),
                "tool_count": n,
                "runnable": true,
                "not_runnable_reason": Value::Null,
            })
        })
        .collect();

    // Vendor and artifact apps from the owner's cloud shelf.
    let mut partial: Vec<&str> = vec![];
    match read_cloud_apps(&SignedAccountCloud, paired_owner().as_deref(), &user.user_id).await {
        Ok(shelf) => {
            let shelf: Vec<CloudApp> = shelf.into_iter().filter(|a| matches_q(&query, &[a.name.as_str(), a.app_ref.as_str()])).collect();
            let rows = futures::future::join_all(shelf.iter().map(|a| cloud_app_row(&state, &user.user_id, bot.as_deref(), a))).await;
            apps.extend(rows.into_iter().flatten());
        }
        Err(e) => {
            warn!(error = %e, "bar apps: cloud shelf not read");
            partial.push(PARTIAL_APP_SHELF);
        }
    }
    Json(json!({ "apps": apps, "partial": partial })).into_response()
}

/// One `/bar/apps` row for a shelf app. A vendor app's tools are listed live
/// (its connection's pinned tools in its allow-list); an artifact app lists the
/// mirror tools but is not runnable. Not-runnable apps stay listed with the reason.
async fn cloud_app_row(state: &Arc<AppState>, user_id: &str, bot: Option<&str>, a: &CloudApp) -> Option<Value> {
    let connector = match (a.kind, a.connector_id.as_deref()) {
        (AppKind::Vendor, Some(cid)) => mcp_apps::load_connector(state, user_id, cid, mcp_apps::allow_private_hosts()).await.ok().flatten(),
        _ => None,
    };
    let bar_app = BarApp {
        kind: a.kind,
        app_ref: a.app_ref.clone(),
        name: a.name.clone(),
        icon: a.icon.clone(),
        not_runnable: cloud_app_not_runnable(a, connector.is_some()),
        connector,
        cloud: Some(a.clone()),
    };
    let tool_count = match a.kind {
        AppKind::Artifact => artifact_tools().len(),
        _ if bar_app.not_runnable.is_some() => 0,
        _ => app_tools(state, user_id, &bar_app).await.map(|t| t.len()).unwrap_or(0),
    };
    // A runnable vendor app the caller can use none of the tools of is not a chip.
    if bar_app.not_runnable.is_none() && tool_count == 0 {
        return None;
    }
    let (uid, b, kind, app_ref) = (user_id.to_string(), bot.map(str::to_string), a.kind, a.app_ref.clone());
    let level = with_db(state, move |conn| {
        let preview = team_rules::preview_call(conn, &uid, b.as_deref(), kind, &app_ref, "", true, &Value::Null).map_err(internal)?;
        Ok(actor_level(b.as_deref(), &preview))
    })
    .await
    .ok()?;
    Some(json!({
        "app": bar_app.app_json(),
        "level": a.ceiling.and(capped_level(level, a.ceiling)).map(GrantLevel::as_str),
        "tool_count": tool_count,
        "runnable": bar_app.not_runnable.is_none(),
        "not_runnable_reason": bar_app.not_runnable,
    }))
}

#[derive(Debug, Deserialize, Default)]
struct ToolsQuery {
    q: Option<String>,
}

async fn tools_route(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((kind, app_ref)): Path<(String, String)>,
    Query(q): Query<ToolsQuery>,
) -> Response {
    let Some(kind) = AppKind::parse(&kind) else {
        return unsupported_kind(&kind);
    };
    let app = match resolve_app(&state, &user.user_id, kind, &app_ref).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let tools = match app_tools(&state, &user.user_id, &app).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let query = q.q.unwrap_or_default();
    let cards: Vec<Value> = tools
        .iter()
        .filter(|t| matches_q(&query, &[t["name"].as_str().unwrap_or_default(), bar_blanks::tool_title(t).as_str()]))
        .map(tool_card)
        .collect();
    Json(json!({
        "app": app.app_json(),
        "runnable": app.not_runnable.is_none(),
        "not_runnable_reason": app.not_runnable,
        "tools": cards,
    }))
    .into_response()
}

// ── Intent ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AppRef {
    kind: String,
    #[serde(rename = "ref")]
    app_ref: String,
}

#[derive(Debug, Deserialize)]
struct IntentBody {
    app: AppRef,
    tool: String,
    bot_id: Option<String>,
}

pub fn encode_intent(kind: AppKind, app_ref: &str, tool: &str) -> String {
    format!("{INTENT_PREFIX}{}", hex::encode(json!([kind.as_str(), app_ref, tool]).to_string()))
}

pub fn decode_intent(id: &str) -> Option<(AppKind, String, String)> {
    let bytes = hex::decode(id.strip_prefix(INTENT_PREFIX)?).ok()?;
    let (kind, app_ref, tool): (String, String, String) = serde_json::from_slice(&bytes).ok()?;
    Some((AppKind::parse(&kind)?, app_ref, tool))
}

async fn intent_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<IntentBody>) -> Response {
    if let Err(r) = check_bot(&state, &user.user_id, body.bot_id.as_deref()).await {
        return r;
    }
    let Some(kind) = AppKind::parse(&body.app.kind) else {
        return unsupported_kind(&body.app.kind);
    };
    let app = match resolve_app(&state, &user.user_id, kind, &body.app.app_ref).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Some(why) = &app.not_runnable {
        return app_not_runnable(why);
    }
    let tool = match app_tool(&state, &user.user_id, &app, &body.tool).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let mut built = bar_blanks::blanks_for_tool(&tool);
    let (uid, aref, tname) = (user.user_id.clone(), app.app_ref.clone(), body.tool.clone());
    let recents = match with_db(&state, move |conn| {
        recents_for_tool(conn, &uid, kind, &aref, &tname).map_err(internal)
    })
    .await
    {
        Ok(r) => r,
        Err(r) => return r,
    };
    for blank in built.blanks.iter_mut().filter(|b| !b.secret && b.kind != BlankType::Details) {
        blank.recent = recents
            .iter()
            .filter(|(field, _)| *field == blank.key)
            .map(|(_, v)| v.clone())
            .take(RECENTS_PER_FIELD as usize)
            .collect();
    }
    Json(json!({
        "intent_id": encode_intent(kind, &app.app_ref, &body.tool),
        "app": app.app_json(),
        "tool": { "name": body.tool, "title": bar_blanks::tool_title(&tool) },
        "sentence": built.sentence,
        "blanks": built.blanks,
        "source": built.source,
    }))
    .into_response()
}

// ── Review ───────────────────────────────────────────────────────────────────

/// Everything the review step needs once the tool has been listed.
pub struct ReviewInput<'a> {
    pub user_id: &'a str,
    pub bot_id: Option<&'a str>,
    pub app_kind: AppKind,
    pub app_ref: &'a str,
    pub app_name: &'a str,
    /// The install's cap on the caller's level (vendor/artifact apps; `None` = no cap).
    pub ceiling: Option<GrantLevel>,
    /// The pinned tool as the connector lists it.
    pub tool: &'a Value,
    pub args: &'a Value,
    /// The install mode asks before this tool (`mcp_user_proxy::requires_confirmation`).
    pub install_confirms: bool,
}

fn display_name(conn: &Connection, user_id: &str) -> rusqlite::Result<Option<String>> {
    Ok(conn
        .query_row("SELECT COALESCE(NULLIF(TRIM(name), ''), email) FROM users WHERE id = ?1", params![user_id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// The review card's lines: one per filled blank, in blank order. Secret
/// values are masked; an enum shows its option label; `details` counts fields.
fn review_lines(tool: &Value, args: &Value) -> Vec<Value> {
    bar_blanks::blanks_for_tool(tool)
        .blanks
        .iter()
        .filter_map(|b| {
            let value = if b.kind == BlankType::Details {
                let n = b.fields.iter().flatten().filter(|f| !args[f.as_str()].is_null()).count();
                (n > 0).then(|| if n == 1 { "1 field".to_string() } else { format!("{n} fields") })?
            } else {
                let v = &args[b.key.as_str()];
                if v.is_null() {
                    return None;
                }
                if b.secret {
                    "••••".to_string()
                } else {
                    b.options
                        .iter()
                        .flatten()
                        .find(|o| &o.value == v)
                        .map(|o| o.label.clone())
                        .or_else(|| bar_blanks::display_value(v))
                        .unwrap_or_else(|| v.to_string())
                }
            };
            Some(json!({ "key": b.key, "label": b.label, "value": value }))
        })
        .collect()
}

/// Required blanks the arguments leave empty (labels).
fn missing_required(tool: &Value, args: &Value) -> Vec<String> {
    bar_blanks::blanks_for_tool(tool)
        .blanks
        .iter()
        .filter(|b| b.required)
        .filter(|b| match b.kind {
            BlankType::Details => b.fields.iter().flatten().all(|f| args[f.as_str()].is_null()),
            _ => match &args[b.key.as_str()] {
                Value::Null => true,
                Value::String(s) => s.trim().is_empty(),
                _ => false,
            },
        })
        .map(|b| b.label.clone())
        .collect()
}

/// The review step: level, rule and approver (via `team_rules::preview_call`),
/// then a `bar_jobs` row in state `review`. Read-only on the rule side: the ask
/// is opened by the run (lane 17), not here.
pub fn review_job(conn: &Connection, input: &ReviewInput<'_>) -> Result<Value, Response> {
    let tool_name = input.tool["name"].as_str().unwrap_or_default();
    let read_only = input.tool["annotations"]["readOnlyHint"] == true;
    let missing = missing_required(input.tool, input.args);
    if !missing.is_empty() {
        let body = json!({
            "error": format!("Fill in {} first.", missing.join(", ")),
            "code": "missing_fields",
            "fields": missing,
        });
        return Err((StatusCode::BAD_REQUEST, Json(body)).into_response());
    }
    let preview = team_rules::preview_call(
        conn,
        input.user_id,
        input.bot_id,
        input.app_kind,
        input.app_ref,
        tool_name,
        read_only,
        input.args,
    )
    .map_err(internal)?;
    match preview.denied {
        Some(Decision::DenyNoGrant) => {
            return Err(err(
                StatusCode::FORBIDDEN,
                "no_grant",
                format!("This bot has no access to {}. Its owner can give it a level on the Team map.", input.app_name),
            ))
        }
        Some(Decision::DenyReadOnly) => {
            return Err(err(
                StatusCode::FORBIDDEN,
                "level_read_only",
                format!("This bot can only read from {} (Browser level), and '{tool_name}' makes changes.", input.app_name),
            ))
        }
        _ => {}
    }
    let level = capped_level(actor_level(input.bot_id, &preview), input.ceiling);
    if level == Some(GrantLevel::Browser) && !read_only {
        return Err(err(
            StatusCode::FORBIDDEN,
            "level_read_only",
            format!("{} is at Browser level here, so it can only read, and '{tool_name}' makes changes.", input.app_name),
        ));
    }
    // The ask the run will open first: someone else's rule; else the caller's own.
    let chosen: Option<&(ApprovalRule, String)> =
        preview.matches.iter().find(|(_, a)| a != input.user_id).or_else(|| preview.matches.first());
    let approver = chosen.map(|(_, a)| a.as_str()).unwrap_or(input.user_id);
    let is_you = approver == input.user_id;
    let needs_confirmation = input.install_confirms || (chosen.is_some() && is_you);
    let job_id = insert_job(
        conn,
        &NewJob {
            user_id: input.user_id,
            bot_id: input.bot_id,
            app_kind: input.app_kind,
            app_ref: input.app_ref,
            tool: tool_name,
            args: input.args,
            level,
            rule_id: chosen.map(|(r, _)| r.id.as_str()),
            approver_user_id: chosen.map(|(_, a)| a.as_str()),
        },
    )
    .map_err(internal)?;
    let name = display_name(conn, approver).map_err(internal)?;
    let rule = match chosen {
        Some((r, a)) => json!({ "id": r.id, "text": team_rules::rule_text(conn, r, input.app_name, Some(a.as_str())).map_err(internal)? }),
        None => Value::Null,
    };
    Ok(json!({
        "job_id": job_id,
        "state": JobState::Review.as_str(),
        "level": level.map(GrantLevel::as_str),
        "needs_confirmation": needs_confirmation,
        "rule": rule,
        "approver": { "user_id": approver, "name": name, "is_you": is_you },
        "lines": review_lines(input.tool, input.args),
    }))
}

#[derive(Debug, Deserialize)]
struct ReviewBody {
    intent_id: String,
    #[serde(default)]
    args: Option<Value>,
    bot_id: Option<String>,
}

async fn review_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<ReviewBody>) -> Response {
    let Some((kind, app_ref, tool_name)) = decode_intent(&body.intent_id) else {
        return err(StatusCode::BAD_REQUEST, "bad_intent", "intent_id is not one this server made");
    };
    let args = match body.args {
        None | Some(Value::Null) => json!({}),
        Some(v @ Value::Object(_)) => v,
        Some(_) => return err(StatusCode::BAD_REQUEST, "bad_request", "args must be an object"),
    };
    let bot = match check_bot(&state, &user.user_id, body.bot_id.as_deref()).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let app = match resolve_app(&state, &user.user_id, kind, &app_ref).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Some(connector) = app.connector.clone().filter(|_| app.not_runnable.is_none()) else {
        return app_not_runnable(app.not_runnable.as_deref().unwrap_or("This app cannot run from the bar."));
    };
    let tool = match app_tool(&state, &user.user_id, &app, &tool_name).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let modes = crate::mcp_user_proxy::load_install_modes(&state, &user.user_id).await;
    let install_confirms =
        crate::mcp_user_proxy::requires_confirmation(crate::mcp_user_proxy::mode_for(&modes, &connector), &tool);
    let uid = user.user_id.clone();
    let reviewed = with_db(&state, move |conn| {
        review_job(
            conn,
            &ReviewInput {
                user_id: &uid,
                bot_id: bot.as_deref(),
                app_kind: kind,
                app_ref: &app.app_ref,
                app_name: &app.name,
                ceiling: app.ceiling(),
                tool: &tool,
                args: &args,
                install_confirms,
            },
        )
    })
    .await;
    match reviewed {
        Ok(v) => Json(v).into_response(),
        Err(r) => r,
    }
}

// ── Job reads ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct JobsQuery {
    state: Option<String>,
}

async fn jobs_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<JobsQuery>) -> Response {
    let states: Vec<JobState> = match q.state.as_deref().map(str::trim).unwrap_or("active") {
        "" | "active" => JobState::ACTIVE.to_vec(),
        "all" => Vec::new(),
        other => match JobState::parse(other) {
            Some(s) => vec![s],
            None => return err(StatusCode::BAD_REQUEST, "bad_request", "state must be active, all, or a job state"),
        },
    };
    let uid = user.user_id.clone();
    match with_db(&state, move |conn| {
        expire_overdue(conn, &uid).map_err(internal)?;
        list_jobs(conn, &uid, &states).map_err(internal)
    })
    .await
    {
        Ok(jobs) => Json(json!({ "jobs": jobs.iter().map(job_json).collect::<Vec<_>>() })).into_response(),
        Err(r) => r,
    }
}

/// The requester sees their job; the approver it waits on sees it too.
async fn job_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let uid = user.user_id.clone();
    let reader = uid.clone();
    match with_db(&state, move |conn| {
        expire_overdue(conn, &reader).map_err(internal)?;
        get_job(conn, &id).map_err(internal)
    })
    .await
    {
        Ok(Some(job)) if job.user_id == uid || job.approver_user_id.as_deref() == Some(uid.as_str()) => {
            Json(json!({ "job": job_json(&job) })).into_response()
        }
        Ok(_) => err(StatusCode::NOT_FOUND, "job_not_found", "job not found"),
        Err(r) => r,
    }
}

// ── Run, events, cancel (lane 17) ────────────────────────────────────────────

/// What a finished job's secret argument values become.
pub const REDACTED: &str = "••••";
const NOT_APPROVED: &str = "not approved";
const NOT_APPROVED_IN_TIME: &str = "not approved in time";
/// A cancel looks the tool up for its schema at most this long (names alone after).
const CANCEL_LIST_BUDGET: Duration = Duration::from_secs(3);

/// What the bar needs from the engine. [`GizziEngine`] in the product; tests fake it.
#[async_trait::async_trait]
pub trait BarEngine: Send + Sync {
    /// An engine answers (`/v1/global/health`).
    async fn reachable(&self) -> bool;
    /// The engine's direct tool call (lane 16): `{sessionID, server, tool, args, meta, …}` →
    /// `{sessionID, toolCallID, result}`. Blocks until the tool finishes. `Err((status, body))`.
    async fn call_tool(&self, payload: Value) -> Result<Value, (u16, String)>;
    /// The engine's MCP task cancel route. `true` when it took it.
    async fn cancel_task(&self, task_id: &str) -> bool;
    /// Every session's MCP events (`agent_session_routes::mcp_bus_event` shapes); `None` when
    /// the engine stream cannot be opened.
    async fn mcp_events(&self) -> Option<BoxStream<'static, Value>>;
}

/// The local gizzi engine, through `agent_session_routes`' shared helpers.
pub struct GizziEngine {
    state: Arc<AppState>,
}

impl GizziEngine {
    pub fn shared(state: &Arc<AppState>) -> Arc<dyn BarEngine> {
        Arc::new(GizziEngine { state: state.clone() })
    }

    fn client() -> reqwest::Client {
        // No caller headers: every bar call and its event stream reach the same engine instance.
        crate::agent_session_routes::gizzi_client(&axum::http::HeaderMap::new())
    }
}

#[async_trait::async_trait]
impl BarEngine for GizziEngine {
    async fn reachable(&self) -> bool {
        crate::health::HealthState::gizzi_healthy(&self.state).await
    }

    async fn call_tool(&self, payload: Value) -> Result<Value, (u16, String)> {
        crate::agent_session_routes::engine_tool_call(&Self::client(), &payload).await
    }

    async fn cancel_task(&self, task_id: &str) -> bool {
        crate::agent_session_routes::engine_cancel_task(&Self::client(), task_id).await.is_ok()
    }

    async fn mcp_events(&self) -> Option<BoxStream<'static, Value>> {
        crate::agent_session_routes::engine_mcp_events(&Self::client()).await.ok()
    }
}

// ── Job events (`{type: bar_job, job}`) ──────────────────────────────────────

fn job_events_tx() -> &'static tokio::sync::broadcast::Sender<Value> {
    static TX: OnceLock<tokio::sync::broadcast::Sender<Value>> = OnceLock::new();
    TX.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// Every job's state changes, as `{type: bar_job, job}`.
pub fn subscribe_jobs() -> tokio::sync::broadcast::Receiver<Value> {
    job_events_tx().subscribe()
}

fn bar_job_event(job: &BarJob) -> Value {
    json!({ "type": "bar_job", "job": job_json(job) })
}

/// Tell watchers (call after the change is committed).
fn announce(job: &BarJob) {
    let _ = job_events_tx().send(bar_job_event(job));
}

/// The events one job's watcher gets: `first` (the job as read), then the
/// engine's MCP events for `bar:<job_id>` and the job's `bar_job` changes, until
/// a `bar_job` shows it finished. Engine events go first when both are ready.
pub fn job_event_stream(
    first: BarJob,
    engine: Option<BoxStream<'static, Value>>,
    mut jobs: tokio::sync::broadcast::Receiver<Value>,
) -> impl futures::Stream<Item = Value> + Send {
    async_stream::stream! {
        let finished = first.state.is_terminal();
        let (job_id, session) = (first.id.clone(), first.session_id.clone());
        yield bar_job_event(&first);
        if finished {
            return;
        }
        let mut engine = engine;
        loop {
            let next_engine = async {
                match engine.as_mut() {
                    Some(s) => s.next().await,
                    None => std::future::pending().await,
                }
            };
            let step = tokio::select! {
                biased;
                ev = next_engine => Err(ev),
                job = jobs.recv() => Ok(job),
            };
            match step {
                Err(Some(ev)) => {
                    if ev["session_id"].as_str() == Some(session.as_str()) {
                        yield ev;
                    }
                }
                Err(None) => engine = None,
                Ok(Ok(ev)) => {
                    if ev["job"]["id"].as_str() == Some(job_id.as_str()) {
                        let done = ev["job"]["state"].as_str().and_then(JobState::parse).is_some_and(JobState::is_terminal);
                        yield ev;
                        if done {
                            return;
                        }
                    }
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return,
            }
        }
    }
}

// ── Secrets ──────────────────────────────────────────────────────────────────

/// `args` with every secret field's value replaced by [`REDACTED`], at any
/// depth: marked secret by `schema` (`format: password`, `writeOnly`) or named
/// like one (`bar_blanks::is_secret_field`, the rule recents use). With a null
/// schema, names alone decide.
pub fn redact_secrets(args: &Value, schema: &Value) -> Value {
    match args {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let prop = &schema["properties"][k.as_str()];
                    let out = if v.is_null() {
                        Value::Null
                    } else if bar_blanks::is_secret_field(k, prop) {
                        json!(REDACTED)
                    } else {
                        redact_secrets(v, prop)
                    };
                    (k.clone(), out)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| redact_secrets(v, &schema["items"])).collect()),
        other => other.clone(),
    }
}

// ── State moves ──────────────────────────────────────────────────────────────

/// Move an active job on and read it back. `None` when it had already finished.
fn move_job(conn: &Connection, id: &str, state: JobState, update: &JobUpdate) -> rusqlite::Result<Option<BarJob>> {
    if !set_job_state(conn, id, state, update)? {
        return Ok(None);
    }
    get_job(conn, id)
}

/// Finish a job (completed, failed or cancelled) and redact its secrets
/// (`schema` = the pinned tool's `inputSchema`; null = names alone). `None`
/// when it had already finished. The caller announces it after commit.
pub fn finish_job(conn: &Connection, id: &str, state: JobState, update: &JobUpdate, schema: &Value) -> rusqlite::Result<Option<BarJob>> {
    debug_assert!(state.is_terminal());
    let Some(mut job) = move_job(conn, id, state, update)? else {
        return Ok(None);
    };
    let redacted = redact_secrets(&job.args, schema);
    if redacted != job.args {
        conn.execute("UPDATE bar_jobs SET args_json = ?2 WHERE id = ?1", params![id, redacted.to_string()])?;
        job.args = redacted;
    }
    Ok(Some(job))
}

/// Waiting jobs whose routed ask outlived its 24 hours (D20) fail "not
/// approved in time". `user_id` = the requester or the approver reading.
pub fn expire_overdue(conn: &Connection, user_id: &str) -> rusqlite::Result<Vec<BarJob>> {
    let ids: Vec<String> = conn
        .prepare(&format!(
            "SELECT j.id FROM bar_jobs j JOIN cowork_approvals a ON a.id = j.approval_id
             WHERE j.state = 'waiting_approval' AND (j.user_id = ?1 OR j.approver_user_id = ?1)
               AND a.created_at < datetime('now', '-{} hours')",
            team_rules::ASK_TTL_HOURS
        ))?
        .query_map(params![user_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut expired = Vec::new();
    for id in ids {
        let update = JobUpdate { error: Some(NOT_APPROVED_IN_TIME.into()), ..Default::default() };
        if let Some(job) = finish_job(conn, &id, JobState::Failed, &update, &Value::Null)? {
            announce(&job);
            expired.push(job);
        }
    }
    Ok(expired)
}

fn job_not_found() -> Response {
    err(StatusCode::NOT_FOUND, "job_not_found", "job not found")
}

/// The caller's own job (only the person who made it runs or cancels it).
fn own_job(conn: &Connection, user_id: &str, job_id: &str) -> Result<BarJob, Response> {
    match get_job(conn, job_id).map_err(internal)? {
        Some(job) if job.user_id == user_id => Ok(job),
        _ => Err(job_not_found()),
    }
}

fn not_in_review(job: &BarJob) -> Response {
    err(StatusCode::CONFLICT, "job_not_in_review", format!("This job is {} and can't be run again.", job.state.as_str()))
}

/// Before anything is listed or opened: the caller's job, still in `review`,
/// and an engine to run it. 409 `no_runtime` otherwise.
pub async fn check_runnable(db: &DbHandle, engine: &dyn BarEngine, user_id: &str, job_id: &str) -> Result<BarJob, Response> {
    let (uid, id) = (user_id.to_string(), job_id.to_string());
    let job = with_handle(db, move |conn| own_job(conn, &uid, &id)).await?;
    if job.state != JobState::Review {
        return Err(not_in_review(&job));
    }
    if !engine.reachable().await {
        return Err(no_runtime());
    }
    Ok(job)
}

// ── Run ──────────────────────────────────────────────────────────────────────

/// The tool as the run sees it.
pub struct RunInput<'a> {
    pub user_id: &'a str,
    pub job_id: &'a str,
    pub app_name: &'a str,
    pub tool_read_only: bool,
    /// The pinned tool's `inputSchema` (recents and redaction).
    pub input_schema: &'a Value,
}

#[derive(Debug)]
pub enum RunStart {
    /// A rule routed it to someone else: the ask is open as `job.approval_id`.
    Waiting(BarJob),
    /// `working`: the caller starts the engine call.
    Call(BarJob),
}

/// The run's database step, in one transaction: the job is still `review`,
/// the gate decides (nothing spent: the proxy's gate checks and spends on the
/// real call), recents are kept, and the job moves on. Announced after commit.
pub fn begin_run(conn: &Connection, input: &RunInput<'_>) -> Result<RunStart, Response> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(internal)?;
    let job = own_job(&tx, input.user_id, input.job_id)?;
    if job.state != JobState::Review {
        return Err(not_in_review(&job));
    }
    let decision = team_rules::gate_call(
        &tx,
        &team_rules::GateCall {
            user_id: &job.user_id,
            bot_id: job.bot_id.as_deref(),
            app_kind: job.app_kind,
            app_ref: &job.app_ref,
            app_name: input.app_name,
            tool: &job.tool,
            tool_read_only: input.tool_read_only,
            args: &job.args,
            approval_ids: &[],
            job_id: Some(job.id.as_str()),
            // Pressing Run on the review card is the person's own confirmation.
            self_confirmed: true,
            held_for_confirmation: true,
        },
    )
    .map_err(internal)?;
    record_recent_values(&tx, &job, input.input_schema).map_err(internal)?;
    let started = match decision {
        Decision::NeedsOwner { rule, approver_user_id, approval_id, .. } => {
            tx.execute(
                "UPDATE bar_jobs SET approver_user_id = ?2, rule_id = ?3 WHERE id = ?1",
                params![job.id, approver_user_id, rule.id],
            )
            .map_err(internal)?;
            // The approval push every ask uses: owner ledger → cloud `approval.requested`.
            crate::runtime_events::record_user_event(
                &tx,
                &approver_user_id,
                "approval.requested",
                None,
                &json!({ "approvalId": approval_id, "source": team_rules::ROUTED_SOURCE, "jobId": job.id }),
                Some(&format!("approval:{approval_id}:requested")),
            )
            .map_err(internal)?;
            let update = JobUpdate { approval_id: Some(approval_id), ..Default::default() };
            let job = move_job(&tx, &job.id, JobState::WaitingApproval, &update).map_err(internal)?.ok_or_else(job_not_found)?;
            RunStart::Waiting(job)
        }
        Decision::DenyNoGrant => {
            return Err(err(
                StatusCode::FORBIDDEN,
                "no_grant",
                format!("This bot has no access to {}. Its owner can give it a level on the Team map.", input.app_name),
            ))
        }
        Decision::DenyReadOnly => {
            return Err(err(
                StatusCode::FORBIDDEN,
                "level_read_only",
                format!("This bot can only read from {} (Browser level), and '{}' makes changes.", input.app_name, job.tool),
            ))
        }
        _ => RunStart::Call(move_job(&tx, &job.id, JobState::Working, &JobUpdate::default()).map_err(internal)?.ok_or_else(job_not_found)?),
    };
    tx.commit().map_err(internal)?;
    match &started {
        RunStart::Waiting(job) | RunStart::Call(job) => announce(job),
    }
    Ok(started)
}

/// The engine call for a connector tool: the per-user proxy server, the
/// namespaced tool, and the proxy registration for `bar:<job_id>`.
pub struct PreparedCall {
    pub server: String,
    pub tool: String,
    pub app_name: String,
    pub tool_read_only: bool,
    pub input_schema: Value,
    /// The `mcpProxy` entry a turn sends (`mcp_user_proxy::proxy_registration`).
    pub proxy: Option<Value>,
}

/// One running engine call.
pub struct RunCall {
    pub job_id: String,
    pub payload: Value,
    pub input_schema: Value,
}

impl PreparedCall {
    pub fn for_job(&self, job: &BarJob) -> RunCall {
        RunCall { job_id: job.id.clone(), payload: call_payload(job, &self.server, &self.tool, self.proxy.clone()), input_schema: self.input_schema.clone() }
    }
}

/// The engine's direct-call body for a job (`POST /v1/mcp/tool/call`).
pub fn call_payload(job: &BarJob, server: &str, tool: &str, proxy: Option<Value>) -> Value {
    let mut meta = serde_json::Map::new();
    meta.insert(team_rules::JOB_ID_META_KEY.into(), json!(job.id));
    // The person confirmed on the review card (the install mode's ask).
    meta.insert(crate::mcp_user_proxy::APPROVED_META_KEY.into(), json!(true));
    if let Some(a) = &job.approval_id {
        meta.insert(team_rules::APPROVAL_ID_META_KEY.into(), json!(a));
    }
    let mut payload = json!({
        "sessionID": job.session_id,
        "server": server,
        "tool": tool,
        "args": job.args,
        "meta": meta,
        "toolCallID": format!("bar_{}", job.id),
    });
    if let Some(proxy) = proxy {
        payload["mcpProxy"] = proxy;
    }
    payload
}

/// List the job's tool and build its engine call (connector and vendor apps:
/// a vendor app's call goes through its connection's proxy entry).
async fn prepare_call(state: &Arc<AppState>, job: &BarJob) -> Result<PreparedCall, Response> {
    let app = resolve_app(state, &job.user_id, job.app_kind, &job.app_ref).await?;
    let Some(connector) = app.connector.clone().filter(|_| app.not_runnable.is_none()) else {
        return Err(app_not_runnable(app.not_runnable.as_deref().unwrap_or("This app cannot run from the bar.")));
    };
    // A vendor app's tool runs through its own connection, inside its allow-list.
    let tool = app_tool(state, &job.user_id, &app, &job.tool).await?;
    let connectors = mcp_apps::load_user_connectors(state, &job.user_id, mcp_apps::allow_private_hosts()).await;
    let prefixes = crate::mcp_user_proxy::connector_prefixes(&connectors);
    let prefix = connectors
        .iter()
        .position(|c| c.id == connector.id)
        .and_then(|i| prefixes.get(i).cloned())
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "connector_not_found", "connector not found"))?;
    Ok(PreparedCall {
        server: crate::mcp_user_proxy::PROXY_SERVER_NAME.to_string(),
        tool: crate::mcp_user_proxy::namespaced(&prefix, &job.tool),
        app_name: app.name.clone(),
        tool_read_only: tool["annotations"]["readOnlyHint"] == true,
        input_schema: tool["inputSchema"].clone(),
        proxy: crate::mcp_user_proxy::proxy_registration(state, &job.user_id, &job.session_id, job.bot_id.as_deref()).await,
    })
}

/// The receipt: `structuredContent` first, the text parts second.
pub fn receipt(result: &Value) -> Value {
    let text: Vec<&str> = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["type"] == "text")
        .filter_map(|c| c["text"].as_str())
        .collect();
    json!({
        "structured": result.get("structuredContent").cloned().unwrap_or(Value::Null),
        "text": if text.is_empty() { Value::Null } else { json!(text.join("\n")) },
    })
}

fn engine_error_text(status: u16, body: &str) -> String {
    if status == 502 {
        return "No Allternit computer is reachable to run this.".into();
    }
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    parsed["message"]
        .as_str()
        .or(parsed["error"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| body.chars().take(500).collect())
}

/// Store the engine's answer on the job (receipt, task id, error) and finish it.
pub fn settle(conn: &Connection, job_id: &str, outcome: Result<Value, (u16, String)>, schema: &Value) -> rusqlite::Result<Option<BarJob>> {
    let (state, update) = match outcome {
        Ok(answer) => {
            let result = answer.get("result").cloned().unwrap_or(Value::Null);
            let failed = result["isError"] == true;
            let receipt = receipt(&result);
            let error = failed.then(|| receipt["text"].as_str().unwrap_or("The tool reported an error.").to_string());
            let update = JobUpdate {
                task_id: answer["toolCallID"].as_str().map(|t| format!("direct:{t}")),
                result: Some(receipt),
                error,
                ..Default::default()
            };
            (if failed { JobState::Failed } else { JobState::Completed }, update)
        }
        Err((status, body)) => (JobState::Failed, JobUpdate { error: Some(engine_error_text(status, &body)), ..Default::default() }),
    };
    finish_job(conn, job_id, state, &update, schema)
}

/// An engine event for the job's session while its call runs: the task id
/// (for cancel) and `input_required` while an elicitation form is open.
fn note_engine_event(conn: &Connection, job_id: &str, ev: &Value) -> rusqlite::Result<Option<BarJob>> {
    match ev["type"].as_str() {
        Some("mcp_task") => {
            if let Some(task) = ev["part"]["taskId"].as_str() {
                conn.execute(
                    "UPDATE bar_jobs SET task_id = ?2 WHERE id = ?1 AND task_id IS NULL AND state IN ('working', 'input_required')",
                    params![job_id, task],
                )?;
            }
            Ok(None)
        }
        Some(kind @ ("mcp_elicitation" | "mcp_elicitation_resolved")) => {
            let (from, to) = if kind == "mcp_elicitation" {
                (JobState::Working, JobState::InputRequired)
            } else {
                (JobState::InputRequired, JobState::Working)
            };
            match get_job(conn, job_id)? {
                Some(job) if job.state == from => move_job(conn, job_id, to, &JobUpdate::default()),
                _ => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

type Running = HashMap<String, (futures::future::AbortHandle, Value)>;

fn running() -> &'static Mutex<Running> {
    static RUNNING: OnceLock<Mutex<Running>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn blocking<T: Send + 'static>(db: &DbHandle, f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static) -> Option<T> {
    let db = db.clone();
    match tokio::task::spawn_blocking(move || db.connect().and_then(|conn| f(&conn))).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            warn!(error = %e, "bar job: database write failed");
            None
        }
        Err(e) => {
            warn!(error = %e, "bar job: database task failed");
            None
        }
    }
}

/// Run the engine call in the background (D10: the engine makes it; the run
/// route does not wait). Cancel drops it through its abort handle.
pub fn spawn_call(db: DbHandle, engine: Arc<dyn BarEngine>, call: RunCall) -> tokio::task::JoinHandle<()> {
    let (abort, registration) = futures::future::AbortHandle::new_pair();
    let job_id = call.job_id.clone();
    running().lock().unwrap_or_else(|p| p.into_inner()).insert(job_id.clone(), (abort, call.input_schema.clone()));
    tokio::spawn(async move {
        let _ = futures::future::Abortable::new(drive_call(db, engine, call), registration).await;
        running().lock().unwrap_or_else(|p| p.into_inner()).remove(&job_id);
    })
}

async fn drive_call(db: DbHandle, engine: Arc<dyn BarEngine>, call: RunCall) {
    let session = call.payload["sessionID"].as_str().unwrap_or_default().to_string();
    // Opened before the call so its first task part is seen.
    let mut events = engine.mcp_events().await;
    let reply = engine.call_tool(call.payload.clone());
    tokio::pin!(reply);
    let outcome = loop {
        let step = {
            let next_event = async {
                match events.as_mut() {
                    Some(s) => s.next().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                out = &mut reply => Ok(out),
                ev = next_event => Err(ev),
            }
        };
        match step {
            Ok(out) => break out,
            Err(None) => events = None,
            Err(Some(ev)) if ev["session_id"].as_str() == Some(session.as_str()) => {
                let id = call.job_id.clone();
                if let Some(Some(job)) = blocking(&db, move |conn| note_engine_event(conn, &id, &ev)).await {
                    announce(&job);
                }
            }
            Err(Some(_)) => {}
        }
    };
    let (id, schema) = (call.job_id.clone(), call.input_schema.clone());
    if let Some(Some(job)) = blocking(&db, move |conn| settle(conn, &id, outcome, &schema)).await {
        announce(&job);
    }
}

pub async fn run_job(state: &Arc<AppState>, engine: Arc<dyn BarEngine>, user_id: &str, job_id: &str) -> Response {
    let job = match check_runnable(&state.db, engine.as_ref(), user_id, job_id).await {
        Ok(j) => j,
        Err(r) => return r,
    };
    let prepared = match prepare_call(state, &job).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let (uid, id, app_name, schema, read_only) =
        (user_id.to_string(), job_id.to_string(), prepared.app_name.clone(), prepared.input_schema.clone(), prepared.tool_read_only);
    let started = with_db(state, move |conn| {
        begin_run(conn, &RunInput { user_id: &uid, job_id: &id, app_name: &app_name, tool_read_only: read_only, input_schema: &schema })
    })
    .await;
    match started {
        Ok(RunStart::Waiting(job)) => Json(json!({ "job": job_json(&job) })).into_response(),
        Ok(RunStart::Call(job)) => {
            spawn_call(state.db.clone(), engine, prepared.for_job(&job));
            Json(json!({ "job": job_json(&job) })).into_response()
        }
        Err(r) => r,
    }
}

async fn run_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let engine = GizziEngine::shared(&state);
    run_job(&state, engine, &user.user_id, &id).await
}

// ── Approval decided ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum Resume {
    /// Not a waiting bar job's ask, or the ask is still open.
    Nothing,
    /// Declined, withdrawn or too late: the job failed.
    Finished(BarJob),
    /// Approved: the job is `working` with the approval id; run it.
    Run(BarJob),
}

/// The waiting bar job a routed ask belongs to.
fn waiting_job_for(conn: &Connection, approval_id: &str) -> rusqlite::Result<Option<BarJob>> {
    let job_id: Option<String> = conn
        .query_row(
            "SELECT job_id FROM cowork_approvals WHERE id = ?1 AND source = ?2",
            params![approval_id, team_rules::ROUTED_SOURCE],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(job_id) = job_id else { return Ok(None) };
    Ok(get_job(conn, &job_id)?
        .filter(|j| j.state == JobState::WaitingApproval && j.approval_id.as_deref() == Some(approval_id)))
}

/// After the routed ask was decided: approved (within 24 h) → `working`;
/// rejected or withdrawn → `failed` "not approved"; approved too late →
/// `failed` "not approved in time". `schema` redacts a failed job's secrets.
pub fn resume_decision(conn: &Connection, approval_id: &str, schema: &Value) -> rusqlite::Result<Resume> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let Some(job) = waiting_job_for(&tx, approval_id)? else { return Ok(Resume::Nothing) };
    let (dismissed, decision, late): (i64, Option<String>, bool) = tx.query_row(
        &format!(
            "SELECT dismissed, decision, created_at < datetime('now', '-{} hours') FROM cowork_approvals WHERE id = ?1",
            team_rules::ASK_TTL_HOURS
        ),
        params![approval_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if dismissed == 0 {
        return Ok(Resume::Nothing);
    }
    let resumed = match (decision.as_deref(), late) {
        (Some("approved"), false) => move_job(&tx, &job.id, JobState::Working, &JobUpdate::default())?.map(Resume::Run),
        (Some("approved"), true) => {
            let update = JobUpdate { error: Some(NOT_APPROVED_IN_TIME.into()), ..Default::default() };
            finish_job(&tx, &job.id, JobState::Failed, &update, schema)?.map(Resume::Finished)
        }
        _ => {
            let update = JobUpdate { error: Some(NOT_APPROVED.into()), ..Default::default() };
            finish_job(&tx, &job.id, JobState::Failed, &update, schema)?.map(Resume::Finished)
        }
    };
    tx.commit()?;
    let resumed = resumed.unwrap_or(Resume::Nothing);
    if let Resume::Run(job) | Resume::Finished(job) = &resumed {
        announce(job);
    }
    Ok(resumed)
}

/// Hook for `cowork_routes::decide_approval`: a decided routed ask resumes or
/// fails the bar job waiting on it (in the background; a no-op otherwise).
pub fn on_routed_decision(state: &Arc<AppState>, approval_id: &str) {
    let (state, approval_id) = (state.clone(), approval_id.to_string());
    tokio::spawn(async move {
        let aid = approval_id.clone();
        let Some(Some(job)) = blocking(&state.db, move |conn| waiting_job_for(conn, &aid)).await else { return };
        // The tool as listed now: its schema (redaction) and its engine call.
        let prepared = prepare_call(&state, &job).await;
        let schema = prepared.as_ref().map(|p| p.input_schema.clone()).unwrap_or(Value::Null);
        let aid = approval_id.clone();
        let Some(Resume::Run(job)) = blocking(&state.db, move |conn| resume_decision(conn, &aid, &schema)).await else { return };
        match prepared {
            Ok(p) => {
                spawn_call(state.db.clone(), GizziEngine::shared(&state), p.for_job(&job));
            }
            Err(_) => {
                let id = job.id.clone();
                let update = JobUpdate { error: Some(format!("The app could not be reached to run '{}'.", job.tool)), ..Default::default() };
                if let Some(Some(job)) = blocking(&state.db, move |conn| finish_job(conn, &id, JobState::Failed, &update, &Value::Null)).await {
                    announce(&job);
                }
            }
        }
    });
}

// ── Events ───────────────────────────────────────────────────────────────────

async fn events_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    // Subscribe before reading the job, so no change falls between the two.
    let jobs = subscribe_jobs();
    let uid = user.user_id.clone();
    let reader = uid.clone();
    let job = match with_db(&state, move |conn| {
        expire_overdue(conn, &reader).map_err(internal)?;
        get_job(conn, &id).map_err(internal)
    })
    .await
    {
        Ok(Some(job)) if job.user_id == uid || job.approver_user_id.as_deref() == Some(uid.as_str()) => job,
        Ok(_) => return job_not_found(),
        Err(r) => return r,
    };
    let engine = if job.state.is_terminal() { None } else { GizziEngine::shared(&state).mcp_events().await };
    let stream = job_event_stream(job, engine, jobs).map(|v| Ok::<_, std::convert::Infallible>(Event::default().data(v.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

// ── Cancel ───────────────────────────────────────────────────────────────────

/// Cancel a job: finish it `cancelled` first (so the call's own finish cannot
/// overwrite it), withdraw a pending routed ask, then cancel the MCP task
/// through the engine's cancel route and drop the call. `schema` redacts
/// secrets when the job is not running here (a running job carries its own).
pub async fn cancel_job(db: &DbHandle, engine: &dyn BarEngine, user_id: &str, job_id: &str, schema: Value) -> Result<BarJob, Response> {
    let run = running().lock().unwrap_or_else(|p| p.into_inner()).remove(job_id);
    let schema = run.as_ref().map(|(_, s)| s.clone()).unwrap_or(schema);
    let (uid, id) = (user_id.to_string(), job_id.to_string());
    let cancelled = with_handle(db, move |conn| {
        let job = own_job(conn, &uid, &id)?;
        if job.state.is_terminal() {
            return Err(err(StatusCode::CONFLICT, "job_finished", format!("This job is already {}.", job.state.as_str())));
        }
        let done = finish_job(conn, &id, JobState::Cancelled, &JobUpdate::default(), &schema)
            .map_err(internal)?
            .ok_or_else(|| err(StatusCode::CONFLICT, "job_finished", "This job has already finished."))?;
        if job.state == JobState::WaitingApproval {
            if let Some(ask) = &job.approval_id {
                crate::cowork_routes::withdraw_routed_ask(conn, ask, &job.user_id).map_err(internal)?;
            }
        }
        Ok(done)
    })
    .await;
    let job = match cancelled {
        Ok(job) => job,
        Err(r) => {
            // Not cancelled here: a running call keeps its handle.
            if let Some(entry) = run {
                running().lock().unwrap_or_else(|p| p.into_inner()).insert(job_id.to_string(), entry);
            }
            return Err(r);
        }
    };
    announce(&job);
    if let Some(task) = job.task_id.as_deref() {
        if !engine.cancel_task(task).await {
            warn!(job_id, "bar cancel: the engine did not take the task cancel; dropping the call");
        }
    }
    if let Some((abort, _)) = run {
        abort.abort();
    }
    Ok(job)
}

async fn cancel_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let (uid, jid) = (user.user_id.clone(), id.clone());
    let job = match with_db(&state, move |conn| own_job(conn, &uid, &jid)).await {
        Ok(j) => j,
        Err(r) => return r,
    };
    // A job that is not running here is redacted from a fresh listing (names alone if it is slow).
    let running_here = running().lock().unwrap_or_else(|p| p.into_inner()).contains_key(&id);
    let schema = if running_here || job.state.is_terminal() || job.app_kind == AppKind::Artifact {
        Value::Null
    } else {
        let lookup = async {
            let app = resolve_app(&state, &job.user_id, job.app_kind, &job.app_ref).await.ok()?;
            app_tool(&state, &job.user_id, &app, &job.tool).await.ok().map(|t| t["inputSchema"].clone())
        };
        tokio::time::timeout(CANCEL_LIST_BUDGET, lookup).await.ok().flatten().unwrap_or(Value::Null)
    };
    let engine = GizziEngine::shared(&state);
    match cancel_job(&state.db, engine.as_ref(), &user.user_id, &id, schema).await {
        Ok(job) => Json(json!({ "job": job_json(&job) })).into_response(),
        Err(r) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use crate::team_store::{self, ApproverKind, RuleInput, RuleOp, ScopeKind};

    fn db() -> Connection {
        DbHandle::new_memory().expect("migrated db").connect().expect("connect")
    }

    /// Org `org-1` owned by `boss`; `ann` reports to `mgr`.
    fn seed(conn: &Connection) {
        conn.execute("INSERT INTO organizations (id, name) VALUES ('org-1', 'Org')", []).unwrap();
        for (u, role) in [("boss", "owner"), ("mgr", "member"), ("ann", "member")] {
            conn.execute(
                "INSERT INTO users (id, email, name, organization_id) VALUES (?1, ?1 || '@x.test', ?1, 'org-1')",
                params![u],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO organization_members (id, organization_id, user_id, role) VALUES (?1, 'org-1', ?2, ?3)",
                params![format!("m-{u}"), u, role],
            )
            .unwrap();
        }
        team_store::set_reports_to(conn, "org-1", "ann", Some("mgr")).unwrap();
    }

    /// "'pay' in Stripe when amount is over 100 USD needs approval from <approver>".
    fn pay_rule(conn: &Connection, approver: ApproverKind) -> ApprovalRule {
        team_store::create_rule(
            conn,
            &RuleInput {
                org_id: Some("org-1".into()),
                owner_user_id: "boss".into(),
                scope_kind: ScopeKind::Org,
                scope_ref: None,
                app_kind: Some(AppKind::Connector),
                app_ref: Some("conn-1".into()),
                tool: "pay".into(),
                field: Some("amount".into()),
                op: RuleOp::Gt,
                value: Some(json!(100)),
                unit_label: Some("USD".into()),
                approver_kind: approver,
                approver_user_id: None,
                enabled: true,
            },
            "boss",
        )
        .unwrap()
    }

    fn pay_tool() -> Value {
        json!({
            "name": "pay",
            "inputSchema": { "type": "object", "required": ["amount", "to"], "properties": {
                "amount": { "type": "number" }, "to": { "type": "string" }, "pin": { "type": "string" }
            }}
        })
    }

    fn review(conn: &Connection, user: &str, args: Value) -> Option<Value> {
        review_job(
            conn,
            &ReviewInput {
                user_id: user,
                bot_id: None,
                app_kind: AppKind::Connector,
                app_ref: "conn-1",
                app_name: "Stripe",
                ceiling: None,
                tool: &pay_tool(),
                args: &args,
                install_confirms: false,
            },
        )
        .ok()
    }

    #[test]
    fn review_names_the_approver_from_a_matching_rule() {
        let conn = db();
        seed(&conn);
        let rule = pay_rule(&conn, ApproverKind::Manager);
        let out = review(&conn, "ann", json!({ "amount": 250, "to": "acme", "pin": "1234" })).unwrap();
        assert_eq!(out["approver"], json!({ "user_id": "mgr", "name": "mgr", "is_you": false }));
        assert_eq!(out["rule"]["id"], json!(rule.id));
        assert!(out["rule"]["text"].as_str().unwrap().contains("over 100 USD"));
        assert_eq!(out["level"], json!("full"));
        assert_eq!(out["needs_confirmation"], json!(false));
        // The secret blank is masked on the card.
        assert!(out["lines"].as_array().unwrap().contains(&json!({ "key": "pin", "label": "Pin", "value": "••••" })));
        let job = get_job(&conn, out["job_id"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!((job.state, job.approver_user_id.as_deref(), job.rule_id.as_deref()), (JobState::Review, Some("mgr"), Some(rule.id.as_str())));
        assert_eq!(job.session_id, format!("bar:{}", job.id));
        // Review opens no ask: the run does.
        let asks: i64 = conn.query_row("SELECT COUNT(*) FROM cowork_approvals", [], |r| r.get(0)).unwrap();
        assert_eq!(asks, 0);

        // Under the threshold no rule matches: the caller approves, nothing to confirm.
        let small = review(&conn, "ann", json!({ "amount": 50, "to": "acme" })).unwrap();
        assert_eq!(small["rule"], Value::Null);
        assert_eq!(small["approver"]["is_you"], json!(true));
    }

    #[test]
    fn review_says_is_you_when_the_caller_is_the_approver() {
        let conn = db();
        seed(&conn);
        pay_rule(&conn, ApproverKind::Owner); // a person's owner = the org owner
        let out = review(&conn, "boss", json!({ "amount": 500, "to": "acme" })).unwrap();
        assert_eq!(out["approver"], json!({ "user_id": "boss", "name": "boss", "is_you": true }));
        assert!(out["rule"]["id"].is_string());
        assert_eq!(out["needs_confirmation"], json!(true));
    }

    struct ShelfCloud {
        shelf: Value,
        vendors: Value,
    }
    #[async_trait::async_trait]
    impl AccountCloud for ShelfCloud {
        async fn send(&self, m: &str, p: &str, _b: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
            assert_eq!(m, "GET");
            let body = match p {
                team_routes::CLOUD_APPS_PATH => &self.shelf,
                CLOUD_VENDOR_APPS_PATH => &self.vendors,
                other => panic!("unexpected path {other}"),
            };
            Ok((200, body.to_string().into_bytes()))
        }
    }

    fn shelf_cloud() -> ShelfCloud {
        ShelfCloud {
            shelf: json!({
                "yours": [{ "kind": "artifact", "ref": "art1", "name": "Quote builder", "icon": "🧮", "provenance": "you",
                            "tier": "full", "granted_tier": "app", "installed": true, "pinned": false }],
                "servers": [{ "kind": "vendor", "ref": "vap1", "name": "Linear", "icon": "https://x/linear.png", "provenance": "server",
                              "tier": "app", "granted_tier": "app", "installed": true, "pinned": false }],
            }),
            vendors: json!({ "apps": [{ "id": "vap1", "connector_id": "conn-lin", "server_url": "https://mcp.linear.app",
                                        "ui_resource": "ui://linear/board", "name": "Linear", "icon": null,
                                        "tool_allowlist": ["create_issue"], "tier": "app" }] }),
        }
    }

    #[tokio::test]
    async fn vendor_and_artifact_apps_come_from_the_owners_shelf() {
        let apps = read_cloud_apps(&shelf_cloud(), Some("ann"), "ann").await.unwrap();
        let vendor = apps.iter().find(|a| a.kind == AppKind::Vendor).unwrap();
        assert_eq!(
            (vendor.connector_id.as_deref(), vendor.ui_resource.as_deref(), vendor.tool_allowlist.clone()),
            (Some("conn-lin"), Some("ui://linear/board"), vec!["create_issue".to_string()])
        );
        assert_eq!((vendor.name.as_str(), vendor.icon.as_deref(), vendor.ceiling), ("Linear", Some("https://x/linear.png"), Some(GrantLevel::App)));
        let artifact = apps.iter().find(|a| a.kind == AppKind::Artifact).unwrap();
        assert_eq!((artifact.icon.as_deref(), artifact.ceiling), (Some("🧮"), Some(GrantLevel::App)));

        // A vendor app runs when installed and its connection is here; an artifact app never does (yet).
        assert_eq!(cloud_app_not_runnable(vendor, true), None);
        assert_eq!(cloud_app_not_runnable(vendor, false).as_deref(), Some("Linear's connection is not on this computer."));
        let uninstalled = CloudApp { ceiling: None, ..vendor.clone() };
        assert_eq!(cloud_app_not_runnable(&uninstalled, true).as_deref(), Some("Install Linear first."));
        assert_eq!(cloud_app_not_runnable(artifact, true).as_deref(), Some(ARTIFACT_NOT_RUNNABLE));

        // The signed client answers for the paired owner only: anyone else fails closed.
        assert!(read_cloud_apps(&shelf_cloud(), Some("boss"), "ann").await.is_err());
        assert!(read_cloud_apps(&shelf_cloud(), None, "ann").await.is_err());
    }

    #[test]
    fn a_vendor_apps_tools_are_its_allow_list_and_its_own_ui_tools() {
        let app = CloudApp {
            kind: AppKind::Vendor,
            app_ref: "vap1".into(),
            name: "Linear".into(),
            icon: None,
            ceiling: Some(GrantLevel::App),
            connector_id: Some("conn-lin".into()),
            ui_resource: Some("ui://linear/board".into()),
            tool_allowlist: vec!["create_issue".into()],
        };
        assert!(vendor_tool_allowed(&app, &json!({ "name": "create_issue" })));
        assert!(vendor_tool_allowed(&app, &json!({ "name": "show_board", "_meta": { "ui": { "resourceUri": "ui://linear/board" } } })));
        assert!(!vendor_tool_allowed(&app, &json!({ "name": "delete_project" })));
        assert!(!vendor_tool_allowed(&app, &json!({ "name": "other_view", "_meta": { "ui": { "resourceUri": "ui://linear/other" } } })));

        // An artifact app's tools are the mirror tools, without the app/binding fields.
        let tools = artifact_tools();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["app_storage_get", "app_storage_set", "app_storage_delete", "app_storage_list", "app_ai_complete"]);
        let set = bar_blanks::blanks_for_tool(&tools[1]);
        let keys: Vec<&str> = set.blanks.iter().map(|b| b.key.as_str()).collect();
        assert!(keys.contains(&"key") && keys.contains(&"value") && !keys.contains(&"binding"), "{keys:?}");
    }

    #[test]
    fn review_caps_a_vendor_app_at_its_install_level() {
        let conn = db();
        seed(&conn);
        let (tool, args) = (pay_tool(), json!({ "amount": 5, "to": "acme" }));
        let input = |ceiling| ReviewInput {
            user_id: "ann",
            bot_id: None,
            app_kind: AppKind::Vendor,
            app_ref: "vap1",
            app_name: "Linear",
            ceiling,
            tool: &tool,
            args: &args,
            install_confirms: false,
        };
        let refused = review_job(&conn, &input(Some(GrantLevel::Browser))).unwrap_err();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let ok = review_job(&conn, &input(Some(GrantLevel::App))).unwrap();
        assert_eq!(ok["level"], json!("app"));
        let job = get_job(&conn, ok["job_id"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!((job.app_kind, job.app_ref.as_str()), (AppKind::Vendor, "vap1"));
    }

    #[test]
    fn review_refuses_missing_required_fields() {
        let conn = db();
        seed(&conn);
        assert!(review(&conn, "ann", json!({ "amount": 5 })).is_none());
        let jobs: i64 = conn.query_row("SELECT COUNT(*) FROM bar_jobs", [], |r| r.get(0)).unwrap();
        assert_eq!(jobs, 0);
    }

    #[test]
    fn recent_values_skip_secrets_and_keep_eight_per_field() {
        let conn = db();
        let schema = json!({ "type": "object", "properties": {
            "to": { "type": "string" },
            "password": { "type": "string" },
            "api_key": { "type": "string" },
            "otp": { "type": "string" },
            "code": { "type": "string", "format": "password" },
            "hidden": { "type": "string", "writeOnly": true },
            "card_number": { "type": "string" },
            "meta": { "type": "object" }
        }});
        let mut last = None;
        for i in 0..10 {
            let args = json!({
                "to": format!("v{i}"), "password": "p", "api_key": "k", "otp": "1", "code": "c",
                "hidden": "h", "card_number": "4242", "meta": { "x": 1 }
            });
            let id = insert_job(
                &conn,
                &NewJob {
                    user_id: "ann",
                    bot_id: None,
                    app_kind: AppKind::Connector,
                    app_ref: "conn-1",
                    tool: "send",
                    args: &args,
                    level: None,
                    rule_id: None,
                    approver_user_id: None,
                },
            )
            .unwrap();
            let job = get_job(&conn, &id).unwrap().unwrap();
            assert_eq!(record_recent_values(&conn, &job, &schema).unwrap(), 1);
            last = Some(job);
        }
        let fields: Vec<String> = conn
            .prepare("SELECT DISTINCT field FROM bar_recent_values")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(fields, vec!["to".to_string()]);
        let kept = recents_for_tool(&conn, "ann", AppKind::Connector, "conn-1", "send").unwrap();
        assert_eq!(kept.len(), 8);
        assert_eq!(kept[0].1.value, json!("v9"));
        assert!(!kept.iter().any(|(_, v)| v.value == json!("v0") || v.value == json!("v1")));
        // No schema, no recents.
        assert_eq!(record_recent_values(&conn, &last.unwrap(), &Value::Null).unwrap(), 0);
    }

    #[test]
    fn job_state_moves_and_finished_jobs_stay_finished() {
        let conn = db();
        let id = insert_job(
            &conn,
            &NewJob {
                user_id: "ann",
                bot_id: None,
                app_kind: AppKind::Connector,
                app_ref: "conn-1",
                tool: "send",
                args: &json!({}),
                level: Some(GrantLevel::Full),
                rule_id: None,
                approver_user_id: None,
            },
        )
        .unwrap();
        assert!(list_jobs(&conn, "ann", &JobState::ACTIVE).unwrap().is_empty());
        assert!(set_job_state(&conn, &id, JobState::Working, &JobUpdate { task_id: Some("conn-1:t1".into()), ..Default::default() }).unwrap());
        let active = list_jobs(&conn, "ann", &JobState::ACTIVE).unwrap();
        assert_eq!(active.len(), 1);
        assert!(active[0].started_at.is_some());
        assert!(set_job_state(&conn, &id, JobState::Completed, &JobUpdate { result: Some(json!({ "ok": true })), ..Default::default() }).unwrap());
        assert!(!set_job_state(&conn, &id, JobState::Working, &JobUpdate::default()).unwrap());
        let job = get_job(&conn, &id).unwrap().unwrap();
        assert_eq!((job.state, job.task_id.as_deref(), job.result), (JobState::Completed, Some("conn-1:t1"), Some(json!({ "ok": true }))));
        assert!(job.finished_at.is_some());
    }

    #[test]
    fn intent_ids_round_trip() {
        let id = encode_intent(AppKind::Connector, "conn-1", "pay");
        assert_eq!(decode_intent(&id), Some((AppKind::Connector, "conn-1".to_string(), "pay".to_string())));
        assert_eq!(decode_intent("bin_zz"), None);
        assert_eq!(decode_intent("conn-1"), None);
    }

    // ── Lane 17: run, approval, events, cancel, redaction ───────────────────

    /// The engine edge (lane 16's relay + gizzi's event stream), faked.
    struct FakeEngine {
        up: bool,
        events: Vec<Value>,
        /// `None`: the call never returns (a long task).
        reply: Mutex<Option<Result<Value, (u16, String)>>>,
        calls: Mutex<Vec<Value>>,
        cancels: Mutex<Vec<String>>,
    }

    impl FakeEngine {
        fn new(up: bool, events: Vec<Value>, reply: Option<Result<Value, (u16, String)>>) -> Arc<FakeEngine> {
            Arc::new(FakeEngine { up, events, reply: Mutex::new(reply), calls: Mutex::default(), cancels: Mutex::default() })
        }
    }

    #[async_trait::async_trait]
    impl BarEngine for FakeEngine {
        async fn reachable(&self) -> bool {
            self.up
        }
        async fn call_tool(&self, payload: Value) -> Result<Value, (u16, String)> {
            self.calls.lock().unwrap().push(payload);
            let reply = self.reply.lock().unwrap().take();
            match reply {
                Some(r) => {
                    // Long enough for the session's events to be read first.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    r
                }
                None => std::future::pending().await,
            }
        }
        async fn cancel_task(&self, task_id: &str) -> bool {
            self.cancels.lock().unwrap().push(task_id.to_string());
            true
        }
        async fn mcp_events(&self) -> Option<BoxStream<'static, Value>> {
            Some(futures::stream::iter(self.events.clone()).chain(futures::stream::pending()).boxed())
        }
    }

    fn pay_schema() -> Value {
        pay_tool()["inputSchema"].clone()
    }

    fn run_input<'a>(user: &'a str, job_id: &'a str, schema: &'a Value) -> RunInput<'a> {
        RunInput { user_id: user, job_id, app_name: "Stripe", tool_read_only: false, input_schema: schema }
    }

    fn stripe_call() -> PreparedCall {
        PreparedCall {
            server: crate::mcp_user_proxy::PROXY_SERVER_NAME.into(),
            tool: "stripe__pay".into(),
            app_name: "Stripe".into(),
            tool_read_only: false,
            input_schema: pay_schema(),
            proxy: None,
        }
    }

    fn task_event(session: &str, task: &str, status: &str) -> Value {
        json!({ "type": "mcp_task", "session_id": session, "part": { "type": "mcp-task", "taskId": task, "status": status } })
    }

    #[tokio::test]
    async fn run_with_a_rule_waits_then_approval_runs_it_to_completed_with_bar_session_events() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        seed(&conn);
        pay_rule(&conn, ApproverKind::Manager);
        let reviewed = review(&conn, "ann", json!({ "amount": 250, "to": "acme", "pin": "1234" })).unwrap();
        let id = reviewed["job_id"].as_str().unwrap().to_string();
        let session = format!("bar:{id}");
        let engine = FakeEngine::new(
            true,
            vec![
                task_event("bar:bjb_someone_else", "direct:x", "working"),
                task_event(&session, "direct:bar_1", "working"),
                task_event(&session, "direct:bar_1", "completed"),
            ],
            Some(Ok(json!({
                "sessionID": session,
                "toolCallID": "bar_1",
                "result": { "content": [{ "type": "text", "text": "Paid 250 to acme" }], "structuredContent": { "payment": "pi_1" } },
            }))),
        );
        let jobs = subscribe_jobs();
        let first = check_runnable(&db, engine.as_ref(), "ann", &id).await.unwrap();
        let schema = pay_schema();

        // Run: the manager's rule routes it; the ask opens for mgr and the job waits.
        let RunStart::Waiting(waiting) = begin_run(&conn, &run_input("ann", &id, &schema)).unwrap() else {
            panic!("expected waiting_approval")
        };
        assert_eq!(waiting.state, JobState::WaitingApproval);
        let ask = waiting.approval_id.clone().expect("ask id");
        let (approver, job_ref): (String, String) = conn
            .query_row("SELECT approver_user_id, job_id FROM cowork_approvals WHERE id = ?1", params![ask], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((approver.as_str(), job_ref.as_str()), ("mgr", id.as_str()));
        // The approver's push: `approval.requested` on their ledger.
        let pushed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM runtime_user_events WHERE user_id = 'mgr' AND event_type = 'approval.requested'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pushed, 1);
        // Recents are kept on run, never the secret pin; the job's JSON masks the pin while it waits.
        let kept = recents_for_tool(&conn, "ann", AppKind::Connector, "conn-1", "pay").unwrap();
        assert!(kept.iter().any(|(f, v)| f == "to" && v.value == json!("acme")));
        assert!(!kept.iter().any(|(f, _)| f == "pin"));
        assert_eq!(job_json(&waiting)["args"]["pin"], json!(REDACTED));
        // A second press is refused.
        let again = begin_run(&conn, &run_input("ann", &id, &schema)).unwrap_err();
        assert_eq!(again.status(), StatusCode::CONFLICT);

        // The manager approves (the decide route's write); the job resumes with the approval id.
        conn.execute(
            "UPDATE cowork_approvals SET dismissed = 1, decision = 'approved', decided_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![ask],
        )
        .unwrap();
        let Resume::Run(working) = resume_decision(&conn, &ask, &schema).unwrap() else { panic!("expected the job to resume") };
        assert_eq!((working.state, working.approval_id.as_deref()), (JobState::Working, Some(ask.as_str())));
        let call = stripe_call().for_job(&working);
        assert_eq!(call.payload["sessionID"], json!(session));
        assert_eq!(call.payload["meta"]["allternit/approvalId"], json!(ask));
        assert_eq!(call.payload["meta"]["allternit/jobId"], json!(id));
        assert_eq!(call.payload["args"]["pin"], json!("1234"), "the call carries the real value");
        spawn_call(db.clone(), engine.clone(), call).await.unwrap();

        let done = get_job(&conn, &id).unwrap().unwrap();
        assert_eq!(done.state, JobState::Completed);
        assert_eq!(done.result, Some(json!({ "structured": { "payment": "pi_1" }, "text": "Paid 250 to acme" })));
        assert_eq!(done.task_id.as_deref(), Some("direct:bar_1"));
        assert_eq!((done.args["pin"].clone(), done.args["to"].clone()), (json!(REDACTED), json!("acme")));
        assert_eq!(engine.calls.lock().unwrap().len(), 1);

        // The job's event stream: the job first, only its own session's MCP events, every state change, then it ends.
        let events: Vec<Value> = job_event_stream(first, engine.mcp_events().await, jobs).collect().await;
        let sessions: Vec<&Value> = events.iter().filter(|e| e["type"] == "mcp_task").map(|e| &e["session_id"]).collect();
        assert_eq!(sessions, vec![&json!(session), &json!(session)]);
        let states: Vec<&str> =
            events.iter().filter(|e| e["type"] == "bar_job").map(|e| e["job"]["state"].as_str().unwrap()).collect();
        assert_eq!(states, vec!["review", "waiting_approval", "working", "completed"]);
    }

    #[tokio::test]
    async fn a_declined_ask_fails_the_job_not_approved() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        seed(&conn);
        pay_rule(&conn, ApproverKind::Manager);
        let id = review(&conn, "ann", json!({ "amount": 250, "to": "acme", "pin": "1234" })).unwrap()["job_id"].as_str().unwrap().to_string();
        let schema = pay_schema();
        let RunStart::Waiting(waiting) = begin_run(&conn, &run_input("ann", &id, &schema)).unwrap() else { panic!("expected waiting") };
        let ask = waiting.approval_id.unwrap();
        // Still open: nothing happens.
        assert!(matches!(resume_decision(&conn, &ask, &schema).unwrap(), Resume::Nothing));
        conn.execute("UPDATE cowork_approvals SET dismissed = 1, decision = 'rejected' WHERE id = ?1", params![ask]).unwrap();
        let Resume::Finished(failed) = resume_decision(&conn, &ask, &schema).unwrap() else { panic!("expected failed") };
        assert_eq!((failed.state, failed.error.as_deref()), (JobState::Failed, Some("not approved")));
        assert_eq!(failed.args["pin"], json!(REDACTED));
    }

    #[tokio::test]
    async fn cancel_moves_running_and_waiting_jobs_to_cancelled() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        seed(&conn);
        pay_rule(&conn, ApproverKind::Manager);
        let schema = pay_schema();

        // A running call (under the rule's threshold, so it runs at once) that never returns.
        let id = review(&conn, "ann", json!({ "amount": 50, "to": "acme", "pin": "1234" })).unwrap()["job_id"].as_str().unwrap().to_string();
        let engine = FakeEngine::new(true, vec![task_event(&format!("bar:{id}"), "conn-1:t9", "working")], None);
        let RunStart::Call(working) = begin_run(&conn, &run_input("ann", &id, &schema)).unwrap() else { panic!("expected working") };
        let handle = spawn_call(db.clone(), engine.clone(), stripe_call().for_job(&working));
        for _ in 0..100 {
            if get_job(&conn, &id).unwrap().unwrap().task_id.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Only the job's person may cancel it.
        assert_eq!(cancel_job(&db, engine.as_ref(), "mgr", &id, Value::Null).await.unwrap_err().status(), StatusCode::NOT_FOUND);
        let cancelled = cancel_job(&db, engine.as_ref(), "ann", &id, Value::Null).await.unwrap();
        assert_eq!(cancelled.state, JobState::Cancelled);
        assert_eq!(cancelled.args["pin"], json!(REDACTED));
        assert_eq!(*engine.cancels.lock().unwrap(), vec!["conn-1:t9".to_string()]);
        tokio::time::timeout(Duration::from_secs(2), handle).await.expect("the call was dropped").unwrap();
        assert_eq!(get_job(&conn, &id).unwrap().unwrap().state, JobState::Cancelled);
        let twice = cancel_job(&db, engine.as_ref(), "ann", &id, Value::Null).await.unwrap_err();
        assert_eq!(twice.status(), StatusCode::CONFLICT);

        // A job waiting on the manager: cancel withdraws the ask.
        let id = review(&conn, "ann", json!({ "amount": 250, "to": "acme" })).unwrap()["job_id"].as_str().unwrap().to_string();
        let RunStart::Waiting(waiting) = begin_run(&conn, &run_input("ann", &id, &schema)).unwrap() else { panic!("expected waiting") };
        let ask = waiting.approval_id.unwrap();
        let cancelled = cancel_job(&db, engine.as_ref(), "ann", &id, Value::Null).await.unwrap();
        assert_eq!(cancelled.state, JobState::Cancelled);
        let (dismissed, decision): (i64, Option<String>) = conn
            .query_row("SELECT dismissed, decision FROM cowork_approvals WHERE id = ?1", params![ask], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((dismissed, decision), (1, None));
        assert!(matches!(resume_decision(&conn, &ask, &schema).unwrap(), Resume::Nothing));
    }

    #[test]
    fn secrets_are_redacted_when_a_job_finishes() {
        let conn = db();
        let schema = json!({ "type": "object", "properties": {
            "to": { "type": "string" },
            "code": { "type": "string", "format": "password" },
            "hidden": { "type": "string", "writeOnly": true },
            "api_key": { "type": "string" },
            "card": { "type": "object", "properties": { "number": { "type": "string", "writeOnly": true }, "holder": { "type": "string" } } },
            "notes": { "type": "string" }
        }});
        let args = json!({
            "to": "acme", "code": "c0de", "hidden": "h", "api_key": "sk_live", "notes": null,
            "card": { "number": "4242", "holder": "Ann", "cvv": "123" }
        });
        let id = insert_job(
            &conn,
            &NewJob {
                user_id: "ann",
                bot_id: None,
                app_kind: AppKind::Connector,
                app_ref: "conn-1",
                tool: "send",
                args: &args,
                level: None,
                rule_id: None,
                approver_user_id: None,
            },
        )
        .unwrap();
        assert!(set_job_state(&conn, &id, JobState::Working, &JobUpdate::default()).unwrap());
        // Still running: the stored arguments are intact (the call needs them).
        assert_eq!(get_job(&conn, &id).unwrap().unwrap().args, args);

        let failed = settle(&conn, &id, Err((400, r#"{"error":"tool_call_failed","message":"card declined"}"#.into())), &schema)
            .unwrap()
            .unwrap();
        assert_eq!((failed.state, failed.error.as_deref()), (JobState::Failed, Some("card declined")));
        let stored = get_job(&conn, &id).unwrap().unwrap().args;
        assert_eq!(
            stored,
            json!({
                "to": "acme", "code": REDACTED, "hidden": REDACTED, "api_key": REDACTED, "notes": null,
                "card": { "number": REDACTED, "holder": "Ann", "cvv": REDACTED }
            })
        );
        // A finished job stays as it is.
        assert!(settle(&conn, &id, Ok(json!({ "result": {} })), &schema).unwrap().is_none());
    }

    #[tokio::test]
    async fn no_runtime_answers_409_and_leaves_the_job_in_review() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        seed(&conn);
        let id = review(&conn, "ann", json!({ "amount": 5, "to": "acme" })).unwrap()["job_id"].as_str().unwrap().to_string();
        let engine = FakeEngine::new(false, Vec::new(), None);
        let refused = check_runnable(&db, engine.as_ref(), "ann", &id).await.unwrap_err();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(refused.into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(body["code"], json!(NO_RUNTIME));
        assert_eq!(get_job(&conn, &id).unwrap().unwrap().state, JobState::Review);
        assert!(engine.calls.lock().unwrap().is_empty());
        // Someone else's job is not found, whatever the engine.
        assert_eq!(check_runnable(&db, engine.as_ref(), "mgr", &id).await.unwrap_err().status(), StatusCode::NOT_FOUND);
    }
}
