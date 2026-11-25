//! Island bar API (`/api/v1/bar/*`; Island bar + Team map, lane 15; PLAN.md
//! §2.2, §2.6, D9, D11). The person picks an app and a tool by type-ahead (no
//! model call), fills the blanks, sees a review card, then runs it. A
//! `bar_jobs` row (V295) is the unit from the review card on (D9).
//!
//! - `GET /bar/apps?q=&bot_id=` → `{apps: [{app: {kind, ref, name, icon}, level, tool_count}]}`:
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
//! Run, events and cancel are lane 17. A 0-bot account works: with no `bot_id`
//! the person is the actor. Only `connector` apps are served for now; vendor and
//! artifact apps answer 404 `app_kind_unsupported`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::warn;

use crate::auth::AuthUser;
use crate::bar_blanks::{self, BlankType, RecentValue};
use crate::mcp_apps::{self, AppsError, Connector};
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
    let db = state.db.clone();
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
        "args": job.args,
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

/// One pinned tool by name. 404 `tool_not_found` when it is not listed or is held.
pub(crate) async fn pinned_tool(state: &Arc<AppState>, user_id: &str, connector: &Connector, tool: &str) -> Result<Value, Response> {
    let tools = pinned_tools(state, user_id, connector).await.map_err(IntoResponse::into_response)?;
    tools
        .into_iter()
        .find(|t| t["name"].as_str() == Some(tool))
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "tool_not_found", format!("{} has no tool '{tool}' you can use", connector.name)))
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

    let apps: Vec<Value> = connectors
        .iter()
        .zip(known)
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|((c, (_, level)), n)| {
            json!({
                "app": { "kind": "connector", "ref": c.id, "name": c.name, "icon": Value::Null },
                "level": level.map(GrantLevel::as_str),
                "tool_count": n,
            })
        })
        .collect();
    Json(json!({ "apps": apps })).into_response()
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
    if AppKind::parse(&kind) != Some(AppKind::Connector) {
        return unsupported_kind(&kind);
    }
    let connector = match load_connector(&state, &user.user_id, &app_ref).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let tools = match pinned_tools(&state, &user.user_id, &connector).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let query = q.q.unwrap_or_default();
    let cards: Vec<Value> = tools
        .iter()
        .filter(|t| matches_q(&query, &[t["name"].as_str().unwrap_or_default(), bar_blanks::tool_title(t).as_str()]))
        .map(tool_card)
        .collect();
    Json(json!({
        "app": { "kind": "connector", "ref": connector.id, "name": connector.name },
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
    if AppKind::parse(&body.app.kind) != Some(AppKind::Connector) {
        return unsupported_kind(&body.app.kind);
    }
    let connector = match load_connector(&state, &user.user_id, &body.app.app_ref).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let tool = match pinned_tool(&state, &user.user_id, &connector, &body.tool).await {
        Ok(t) => t,
        Err(r) => return r,
    };
    let mut built = bar_blanks::blanks_for_tool(&tool);
    let (uid, cid, tname) = (user.user_id.clone(), connector.id.clone(), body.tool.clone());
    let recents = match with_db(&state, move |conn| {
        recents_for_tool(conn, &uid, AppKind::Connector, &cid, &tname).map_err(internal)
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
        "intent_id": encode_intent(AppKind::Connector, &connector.id, &body.tool),
        "app": { "kind": "connector", "ref": connector.id, "name": connector.name },
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
    let level = actor_level(input.bot_id, &preview);
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
    Ok(json!({
        "job_id": job_id,
        "state": JobState::Review.as_str(),
        "level": level.map(GrantLevel::as_str),
        "needs_confirmation": needs_confirmation,
        "rule": chosen.map(|(r, _)| json!({ "id": r.id, "text": team_rules::rule_text(r, input.app_name) })),
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
    if kind != AppKind::Connector {
        return unsupported_kind(kind.as_str());
    }
    let connector = match load_connector(&state, &user.user_id, &app_ref).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let tool = match pinned_tool(&state, &user.user_id, &connector, &tool_name).await {
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
                app_kind: AppKind::Connector,
                app_ref: &connector.id,
                app_name: &connector.name,
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
    match with_db(&state, move |conn| list_jobs(conn, &uid, &states).map_err(internal)).await {
        Ok(jobs) => Json(json!({ "jobs": jobs.iter().map(job_json).collect::<Vec<_>>() })).into_response(),
        Err(r) => r,
    }
}

/// The requester sees their job; the approver it waits on sees it too.
async fn job_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(id): Path<String>) -> Response {
    let uid = user.user_id.clone();
    match with_db(&state, move |conn| get_job(conn, &id).map_err(internal)).await {
        Ok(Some(job)) if job.user_id == uid || job.approver_user_id.as_deref() == Some(uid.as_str()) => {
            Json(json!({ "job": job_json(&job) })).into_response()
        }
        Ok(_) => err(StatusCode::NOT_FOUND, "job_not_found", "job not found"),
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
}
