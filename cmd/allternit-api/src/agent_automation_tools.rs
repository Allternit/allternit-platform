//! Scheduled tasks an agent makes for its user (Primitives Wave A, the
//! `automation` tool): gizzi's tool (`runtime/primitives/automation.ts`)
//! checks and builds every request, and these five routes store and change
//! them as local routines that the local scheduler runs
//! (`routine_local_scheduler`).
//!
//! Like `agent_user_tools`, they work in every session the caller owns (a
//! bot's chat, a plain chat, a Gizzi Code session); whose session it is comes
//! from [`resolve_owner`], never from the model. Gizzi sends the user's token
//! or the internal service token.
//!
//! * `POST /bot-routing/automation/create` `{sessionId, kind, title, prompt, target, initiation, schedule, timezone, maxRuns?, until?, scheduleInput, automationRun?}`
//!   → `{automation, duplicate?}`.
//! * `POST /bot-routing/automation/list` `{sessionId}` → `{automations}`: every
//!   routine the session's owner has (made by an agent or in the app).
//! * `POST /bot-routing/automation/update` `{sessionId, id, title?, prompt?, kind?, target?, status?, schedule?, timezone?, maxRuns?, until?, scheduleInput?}`
//!   → `{automation}`. `maxRuns` / `until` null clear them.
//! * `POST /bot-routing/automation/delete` `{sessionId, id}` → `{deleted: true, id}`.
//! * `POST /bot-routing/automation/run` `{sessionId, id}` → `{id, status: started|queued}`.
//!
//! Errors are `{error, code}` (the runtime reads both).
//!
//! **Rules.** Always `execution_domain = 'local'` (the user's own machine or
//! cloud computer, never a company server). Only the owner's rows can be
//! listed, changed, run or deleted. A recurring task runs at most every
//! [`MIN_INTERVAL_MINUTES`]; a one-shot falls 1 minute to
//! [`MAX_ONE_SHOT_DAYS`] days ahead; one person has at most [`MAX_ACTIVE`]
//! active agent-made tasks. Asking again for a task that already exists
//! (same session, kind, prompt and schedule) returns it with
//! `duplicate: true`. A turn started by a scheduled run can't create one
//! (`automationRun` in the body, or a scheduled run going in the session):
//! refused, never asked.
//!
//! **Stored as.** `schedule_type` `once` (a UTC instant) or `cron` + its
//! `timezone`. `config = {jobType: "agent", prompt, kind, target,
//! delivery: {sessionId}, scheduleInput, until?}`; `metadata = {origin:
//! "agent", initiation, sessionId, sourceThreadId?}`. `agent_id` is the bot
//! when the session is a bot's chat (its turns run as the bot), else NULL:
//! `config.delivery.sessionId` is what runs it ([`crate::automation_routes::insert_routine`]).

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::agent_user_tools::{resolve_owner, Owner, Refusal};
use crate::auth::AuthUser;
use crate::automation_routes::{delivery_session, insert_routine, Routine};
use crate::routine_local_scheduler::{counted_runs, next_run};
use crate::AppState;

// ---- limits (one place; Eoj can change them; the runtime has the same numbers) ----

/// Shortest gap between two runs of a recurring task.
pub const MIN_INTERVAL_MINUTES: i64 = 15;
/// A one-shot runs at least this far ahead ...
pub const MIN_ONE_SHOT_SECS: i64 = 60;
/// ... and at most this many days ahead.
pub const MAX_ONE_SHOT_DAYS: i64 = 365;
/// Active agent-made tasks per person.
pub const MAX_ACTIVE: i64 = 25;
pub const MAX_TITLE_CHARS: usize = 80;
pub const MAX_PROMPT_CHARS: usize = 4000;
const MAX_LISTED: i64 = 200;

pub fn agent_automation_tools_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/bot-routing/automation/create", post(create_route))
        .route("/bot-routing/automation/list", post(list_route))
        .route("/bot-routing/automation/update", post(update_route))
        .route("/bot-routing/automation/delete", post(delete_route))
        .route("/bot-routing/automation/run", post(run_route))
}

// ---- bodies ----

/// `once`: an RFC 3339 instant. `cron`: 5 fields read in `timezone`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ScheduleBody {
    Once { at: String },
    Cron { expression: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBody {
    pub session_id: String,
    pub kind: String,
    #[serde(default)]
    pub title: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub initiation: Option<String>,
    pub schedule: ScheduleBody,
    pub timezone: String,
    #[serde(default)]
    pub max_runs: Option<i64>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub schedule_input: Option<Value>,
    /// The calling turn was started by a scheduled run (a runtime that knows sends it).
    #[serde(default)]
    pub automation_run: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBody {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdBody {
    pub session_id: String,
    pub id: String,
}

/// A field that can be absent (no change), null (clear) or a value.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBody {
    pub session_id: String,
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub schedule: Option<ScheduleBody>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub max_runs: Option<Option<i64>>,
    #[serde(default, deserialize_with = "nullable")]
    pub until: Option<Option<String>>,
    #[serde(default)]
    pub schedule_input: Option<Value>,
}

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into(), "code": code }))).into_response()
}

fn refuse(status: StatusCode, code: &'static str, message: impl Into<String>) -> Refusal {
    (status, code, message.into())
}

fn bad(code: &'static str, message: impl Into<String>) -> Refusal {
    refuse(StatusCode::BAD_REQUEST, code, message)
}

fn db_err(e: rusqlite::Error) -> Refusal {
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "failed", e.to_string())
}

fn to_response(result: Result<Value, Refusal>) -> Response {
    match result {
        Ok(v) => Json(v).into_response(),
        Err((status, code, msg)) => err(status, code, msg),
    }
}

async fn owner_for(state: &AppState, user: &AuthUser, session_id: &str) -> Result<Owner, Response> {
    let db = state.db.clone();
    let (uid, sid) = (user.user_id.clone(), session_id.to_string());
    let paired = crate::relay_auth::process_secret().paired_owner();
    match tokio::task::spawn_blocking(move || resolve_owner(&db, &uid, &sid, paired.as_deref())).await {
        Ok(Ok(o)) => Ok(o),
        Ok(Err((status, code, msg))) => Err(err(status, code, msg)),
        Err(e) => Err(err(StatusCode::BAD_GATEWAY, "failed", e.to_string())),
    }
}

// ---- checks ----

/// A schedule as stored: `schedule_type`, `schedule_expression`, its zone and first run.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub schedule_type: &'static str,
    pub expression: String,
    pub timezone: String,
    pub first: DateTime<Utc>,
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Check a schedule against the limits and turn it into what the scheduler stores.
pub fn check_schedule(schedule: &ScheduleBody, timezone: &str, now: DateTime<Utc>) -> Result<Checked, Refusal> {
    let zone = timezone.trim();
    if zone.parse::<chrono_tz::Tz>().is_err() {
        return Err(bad("bad_timezone", format!("\"{zone}\" is not an IANA time zone (for example \"America/Chicago\").")));
    }
    match schedule {
        ScheduleBody::Once { at } => {
            let at = DateTime::parse_from_rfc3339(at.trim())
                .map_err(|_| bad("bad_schedule", "schedule.at must be an RFC 3339 instant, such as 2026-11-18T15:00:00Z."))?
                .with_timezone(&Utc);
            let ahead = (at - now).num_seconds();
            if ahead < MIN_ONE_SHOT_SECS {
                return Err(bad("out_of_window", "That time is in the past or less than a minute away."));
            }
            if ahead > MAX_ONE_SHOT_DAYS * 24 * 3600 {
                return Err(bad("out_of_window", format!("One-time scheduled tasks can be at most {MAX_ONE_SHOT_DAYS} days ahead.")));
            }
            Ok(Checked { schedule_type: "once", expression: rfc3339(at), timezone: zone.to_string(), first: at })
        }
        ScheduleBody::Cron { expression } => {
            let expression = expression.split_whitespace().collect::<Vec<_>>().join(" ");
            if expression.split(' ').count() != 5 {
                return Err(bad("bad_cron", "cron must have 5 fields: minute hour day-of-month month day-of-week."));
            }
            let first = next_run("cron", &expression, now, true, Some(zone))
                .ok_or_else(|| bad("bad_cron", format!("\"{expression}\" is not a cron line this scheduler can run.")))?;
            if let Some(gap) = shortest_gap(&expression, zone, first) {
                if gap < chrono::Duration::minutes(MIN_INTERVAL_MINUTES) {
                    return Err(bad(
                        "too_frequent",
                        format!("Recurring scheduled tasks run at most every {MIN_INTERVAL_MINUTES} minutes; that one runs {} min apart.", gap.num_minutes()),
                    ));
                }
            }
            Ok(Checked { schedule_type: "cron", expression, timezone: zone.to_string(), first })
        }
    }
}

/// The shortest gap between runs of a cron line, looked at over its next
/// eight days (enough for any weekly pattern) or 500 runs.
fn shortest_gap(expression: &str, zone: &str, first: DateTime<Utc>) -> Option<chrono::Duration> {
    let horizon = first + chrono::Duration::days(8);
    let mut prev = first;
    let mut shortest: Option<chrono::Duration> = None;
    for _ in 0..500 {
        let Some(next) = next_run("cron", expression, prev, false, Some(zone)) else { break };
        let gap = next - prev;
        shortest = Some(shortest.map_or(gap, |s| s.min(gap)));
        if next > horizon {
            break;
        }
        prev = next;
    }
    shortest
}

/// Same words, any spacing or case: what counts as "the same prompt".
fn normalize_prompt(prompt: &str) -> String {
    prompt.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn check_prompt(prompt: &str) -> Result<String, Refusal> {
    let text = prompt.trim();
    if text.is_empty() {
        return Err(bad("bad_prompt", "prompt is empty: say what to do when it runs."));
    }
    if text.chars().count() > MAX_PROMPT_CHARS {
        return Err(bad("bad_prompt", format!("prompt is longer than {MAX_PROMPT_CHARS} characters.")));
    }
    Ok(text.to_string())
}

fn title_for(title: Option<&str>, prompt: &str) -> Result<String, Refusal> {
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        if t.chars().count() > MAX_TITLE_CHARS {
            return Err(bad("bad_title", format!("title is longer than {MAX_TITLE_CHARS} characters.")));
        }
        return Ok(t.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    let first = prompt.lines().next().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    Ok(if first.chars().count() > MAX_TITLE_CHARS {
        format!("{}…", first.chars().take(MAX_TITLE_CHARS - 1).collect::<String>().trim_end())
    } else {
        first
    })
}

fn check_choice(value: Option<&str>, allowed: &[&'static str], field: &str) -> Result<&'static str, Refusal> {
    let v = value.map(str::trim).unwrap_or(allowed[0]);
    allowed.iter().find(|a| **a == v).copied().ok_or_else(|| bad("bad_field", format!("{field} must be one of {}.", allowed.join(", "))))
}

fn check_until(until: &str, now: DateTime<Utc>) -> Result<String, Refusal> {
    let at = DateTime::parse_from_rfc3339(until.trim()).map_err(|_| bad("bad_until", "until must be an RFC 3339 instant."))?.with_timezone(&Utc);
    if at <= now {
        return Err(bad("out_of_window", "until is already past."));
    }
    Ok(rfc3339(at))
}

fn check_max_runs(n: i64) -> Result<i64, Refusal> {
    if (1..=100_000).contains(&n) {
        Ok(n)
    } else {
        Err(bad("bad_max_runs", "maxRuns must be from 1 to 100000."))
    }
}

/// A truthy `automationRun` (the runtime's own marker of a scheduled turn).
fn from_run(marker: Option<&Value>) -> bool {
    match marker {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

const AUTOMATION_RUN_REFUSAL: &str =
    "This run was started by a scheduled task, and a scheduled task can't create scheduled tasks. Tell the user in your reply instead.";

// ---- rows ----

const ROUTINE_COLS: &str = "id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, name, description, status, schedule_type, schedule_expression, timezone, execution_domain, config, tags, metadata, max_runs, timeout_seconds, max_retries, created_at, updated_at, next_run_at, last_run_at";

fn read_routine(row: &rusqlite::Row<'_>) -> rusqlite::Result<Routine> {
    let json = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(Routine {
        id: row.get(0)?,
        user_id: row.get(1)?,
        workspace_id: row.get(2)?,
        agent_id: row.get(3)?,
        goal_id: row.get(4)?,
        gizzi_job_id: row.get(5)?,
        name: row.get(6)?,
        description: row.get(7)?,
        status: row.get(8)?,
        schedule_type: row.get(9)?,
        schedule_expression: row.get(10)?,
        timezone: row.get(11)?,
        execution_domain: row.get(12)?,
        config: json(row.get(13)?).unwrap_or_else(|| json!({})),
        tags: row.get::<_, Option<String>>(14)?.and_then(|s| serde_json::from_str(&s).ok()),
        metadata: json(row.get(15)?),
        max_runs: row.get(16)?,
        timeout_seconds: row.get(17)?,
        max_retries: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
        next_run_at: row.get(21)?,
        last_run_at: row.get(22)?,
    })
}

fn owned(conn: &rusqlite::Connection, owner: &str, id: &str) -> Result<Routine, Refusal> {
    conn.query_row(&format!("SELECT {ROUTINE_COLS} FROM routines WHERE id = ?1 AND user_id = ?2"), params![id.trim(), owner], read_routine)
        .optional()
        .map_err(db_err)?
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND, "not_found", format!("There's no scheduled task with id {} (use action list).", id.trim())))
}

fn meta_str<'a>(r: &'a Routine, key: &str) -> Option<&'a str> {
    r.metadata.as_ref().and_then(|m| m.get(key)).and_then(Value::as_str)
}

fn agent_made(r: &Routine) -> bool {
    meta_str(r, "origin") == Some("agent")
}

fn active_agent_made(conn: &rusqlite::Connection, owner: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM routines WHERE user_id = ?1 AND status = 'active'
           AND json_extract(CASE WHEN json_valid(metadata) THEN metadata ELSE '{}' END, '$.origin') = 'agent'",
        params![owner],
        |r| r.get(0),
    )
}

fn zone_of(r: &Routine) -> String {
    r.timezone.as_deref().map(str::trim).filter(|z| z.parse::<chrono_tz::Tz>().is_ok()).unwrap_or("UTC").to_string()
}

/// The runtime's `AutomationInfo` for a routine (agent-made or made in the app).
pub fn info(conn: &rusqlite::Connection, r: &Routine, now: DateTime<Utc>) -> Value {
    let zone = zone_of(r);
    let schedule = match r.schedule_type.as_str() {
        "once" => json!({ "type": "once", "at": r.schedule_expression }),
        "cron" => json!({ "type": "cron", "expression": r.schedule_expression }),
        // Made in the app: no cron line, so say what it is.
        "interval" => json!({ "type": "cron", "expression": format!("@every {}", r.schedule_expression) }),
        other => json!({ "type": "cron", "expression": format!("@{other}") }),
    };
    let status = match r.status.as_str() {
        s @ ("active" | "paused" | "completed") => s,
        _ => "paused",
    };
    let next = if status == "active" {
        r.next_run_at.clone().or_else(|| {
            let fresh = !(r.schedule_type == "once" && r.last_run_at.is_some());
            fresh.then(|| next_run(&r.schedule_type, &r.schedule_expression, now, true, r.timezone.as_deref())).flatten().map(rfc3339)
        })
    } else {
        None
    };
    let kind = r.config.get("kind").and_then(Value::as_str).filter(|k| *k == "heartbeat").unwrap_or("message");
    let target = r.config.get("target").and_then(Value::as_str).filter(|t| *t == "new_session").unwrap_or("this_thread");
    let mut out = json!({
        "id": r.id,
        "title": r.name,
        "kind": kind,
        "target": target,
        "status": status,
        "schedule": schedule,
        "timezone": zone,
        "nextRunAt": next,
        "lastRunAt": r.last_run_at,
        "lastRunStatus": meta_str(r, "lastRunStatus"),
        "maxRuns": r.max_runs,
        "runCount": counted_runs(conn, &r.id),
        "until": r.config.get("until").and_then(Value::as_str),
        "sessionId": delivery_session(&r.config),
        "origin": if agent_made(r) { "agent" } else { "user" },
        "initiation": meta_str(r, "initiation"),
        "createdAt": r.created_at,
    });
    if let Some(agent) = r.agent_id.as_deref().filter(|a| !a.trim().is_empty()) {
        out["agentId"] = json!(agent);
    }
    out
}

// ---- create ----

/// Store a new agent-made task. `in_run`: a scheduled run is going in the
/// calling session (its turn is the caller). `Ok((info, duplicate))`.
pub fn create(conn: &rusqlite::Connection, owner: &Owner, body: &CreateBody, in_run: bool, now: DateTime<Utc>) -> Result<(Value, bool), Refusal> {
    if in_run || from_run(body.automation_run.as_ref()) {
        return Err(refuse(StatusCode::FORBIDDEN, "automation_run", AUTOMATION_RUN_REFUSAL));
    }
    let session = body.session_id.trim();
    let kind = check_choice(Some(body.kind.as_str()), &["message", "heartbeat"], "kind")?;
    let target = check_choice(body.target.as_deref(), &["this_thread", "new_session"], "target")?;
    let initiation = check_choice(body.initiation.as_deref(), &["user_request", "own_followup"], "initiation")?;
    let prompt = check_prompt(&body.prompt)?;
    let title = title_for(body.title.as_deref(), &prompt)?;
    let checked = check_schedule(&body.schedule, &body.timezone, now)?;
    let max_runs = body.max_runs.map(check_max_runs).transpose()?;
    let until = body.until.as_deref().map(|u| check_until(u, now)).transpose()?;

    // The same task asked for again: hand back the one there is.
    let wanted = normalize_prompt(&prompt);
    let candidates: Vec<Routine> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ROUTINE_COLS} FROM routines
                 WHERE user_id = ?1 AND status IN ('active', 'paused') AND schedule_type = ?2
                   AND json_extract(config, '$.delivery.sessionId') = ?3
                   AND COALESCE(json_extract(config, '$.kind'), 'message') = ?4"
            ))
            .map_err(db_err)?;
        let rows = stmt.query_map(params![owner.user_id, checked.schedule_type, session, kind], read_routine).map_err(db_err)?;
        rows.filter_map(Result::ok).collect()
    };
    let same_time = |r: &Routine| match checked.schedule_type {
        // A one-shot from delay_minutes lands a few seconds apart on each ask.
        "once" => DateTime::parse_from_rfc3339(&r.schedule_expression).is_ok_and(|t| (t.with_timezone(&Utc) - checked.first).num_seconds().abs() < 60),
        _ => r.schedule_expression == checked.expression && zone_of(r) == checked.timezone,
    };
    if let Some(existing) = candidates
        .iter()
        .find(|r| same_time(r) && r.config.get("prompt").and_then(Value::as_str).map(normalize_prompt).as_deref() == Some(wanted.as_str()))
    {
        return Ok((info(conn, existing, now), true));
    }

    if active_agent_made(conn, &owner.user_id).map_err(db_err)? >= MAX_ACTIVE {
        return Err(refuse(
            StatusCode::CONFLICT,
            "too_many",
            format!("There are already {MAX_ACTIVE} active scheduled tasks. Delete or pause one first (use action list)."),
        ));
    }

    let mut config = json!({
        "jobType": "agent",
        "prompt": prompt,
        "kind": kind,
        "target": target,
        "delivery": { "sessionId": session },
        "scheduleInput": body.schedule_input.clone().unwrap_or_else(|| json!({})),
    });
    if let Some(u) = &until {
        config["until"] = json!(u);
    }
    let mut metadata = json!({ "origin": "agent", "initiation": initiation, "sessionId": session });
    // Made in a bot thread: closing the thread stops it (thread_closeout).
    if let Some(thread) = crate::thread_closeout::thread_for_session(conn, session) {
        metadata["sourceThreadId"] = json!(thread);
    }
    let stamp = now.to_rfc3339();
    let routine = Routine {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: owner.user_id.clone(),
        workspace_id: None,
        agent_id: owner.bot.as_ref().map(|c| c.bot_id.clone()),
        goal_id: None,
        gizzi_job_id: None,
        name: title,
        description: None,
        status: "active".into(),
        schedule_type: checked.schedule_type.into(),
        schedule_expression: checked.expression.clone(),
        timezone: Some(checked.timezone.clone()),
        // Never a company server: this machine, or the user's cloud computer.
        execution_domain: "local".into(),
        config,
        tags: Some(vec!["automation".into(), format!("automation:{kind}")]),
        metadata: Some(metadata),
        max_runs: max_runs.map(|n| n as i32),
        timeout_seconds: None,
        max_retries: None,
        created_at: stamp.clone(),
        updated_at: stamp,
        next_run_at: Some(checked.first.to_rfc3339()),
        last_run_at: None,
    };
    insert_routine(conn, &routine).map_err(db_err)?;
    Ok((info(conn, &routine, now), false))
}

// ---- list ----

pub fn list(conn: &rusqlite::Connection, owner: &Owner, now: DateTime<Utc>) -> Result<Value, Refusal> {
    let mut stmt = conn
        .prepare(&format!("SELECT {ROUTINE_COLS} FROM routines WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2"))
        .map_err(db_err)?;
    let rows: Vec<Routine> = stmt.query_map(params![owner.user_id, MAX_LISTED], read_routine).map_err(db_err)?.filter_map(Result::ok).collect();
    Ok(json!({ "automations": rows.iter().map(|r| info(conn, r, now)).collect::<Vec<_>>() }))
}

// ---- update ----

pub fn update(conn: &rusqlite::Connection, owner: &Owner, body: &UpdateBody, now: DateTime<Utc>) -> Result<Value, Refusal> {
    let mut r = owned(conn, &owner.user_id, &body.id)?;
    if r.execution_domain != "local" || r.gizzi_job_id.is_some() {
        return Err(refuse(StatusCode::CONFLICT, "cloud_routine", "That scheduled task runs on the always-on scheduler; change it in the app."));
    }
    let delivers = delivery_session(&r.config).is_some();
    if (body.kind.is_some() || body.target.is_some()) && !delivers {
        return Err(bad("bad_field", "kind and target apply only to scheduled tasks made in a conversation."));
    }
    let mut changed = false;
    if let Some(prompt) = &body.prompt {
        r.config["prompt"] = json!(check_prompt(prompt)?);
        changed = true;
    }
    if let Some(title) = &body.title {
        let prompt = r.config.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
        r.name = title_for(Some(title.as_str()), &prompt)?;
        changed = true;
    }
    if let Some(kind) = &body.kind {
        r.config["kind"] = json!(check_choice(Some(kind.as_str()), &["message", "heartbeat"], "kind")?);
        changed = true;
    }
    if let Some(target) = &body.target {
        r.config["target"] = json!(check_choice(Some(target.as_str()), &["this_thread", "new_session"], "target")?);
        changed = true;
    }
    let was = r.status.clone();
    let mut rearm = false;
    if let Some(schedule) = &body.schedule {
        let zone = body.timezone.as_deref().ok_or_else(|| bad("bad_timezone", "A new schedule needs its timezone."))?;
        let checked = check_schedule(schedule, zone, now)?;
        r.schedule_type = checked.schedule_type.into();
        r.schedule_expression = checked.expression;
        r.timezone = Some(checked.timezone);
        r.next_run_at = Some(checked.first.to_rfc3339());
        r.last_run_at = None;
        if let Some(input) = &body.schedule_input {
            r.config["scheduleInput"] = input.clone();
        }
        // A new schedule replaces the old end (COUNT/UNTIL, or none).
        match body.max_runs.clone().flatten() {
            Some(n) => r.max_runs = Some(check_max_runs(n)? as i32),
            None => r.max_runs = None,
        }
        match body.until.clone().flatten() {
            Some(u) => r.config["until"] = json!(check_until(&u, now)?),
            None => {
                if let Some(m) = r.config.as_object_mut() {
                    m.remove("until");
                }
            }
        }
        // A new schedule turns a finished task back on, unless it is being paused.
        if r.status == "completed" && body.status.is_none() {
            r.status = "active".into();
        }
        changed = true;
    } else if body.timezone.is_some() {
        return Err(bad("bad_timezone", "To change the time zone, send the schedule again together with the new timezone."));
    }
    if let Some(status) = &body.status {
        let status = check_choice(Some(status.as_str()), &["active", "paused"], "status")?;
        if status == "active" && was == "completed" && body.schedule.is_none() {
            return Err(refuse(StatusCode::CONFLICT, "nothing_due", "Nothing more is due on that schedule; send a new schedule to turn it back on."));
        }
        if status != r.status {
            r.status = status.into();
            if body.schedule.is_none() {
                // The scheduler recomputes the cursor from the schedule.
                r.next_run_at = None;
            }
        }
        rearm = status == "active";
        changed = true;
    }
    if !changed {
        return Err(bad("nothing_to_change", "update needs something to change: title, prompt, kind, target, schedule or status."));
    }
    if r.status == "active" && was != "active" && agent_made(&r) && active_agent_made(conn, &owner.user_id).map_err(db_err)? >= MAX_ACTIVE {
        return Err(refuse(
            StatusCode::CONFLICT,
            "too_many",
            format!("There are already {MAX_ACTIVE} active scheduled tasks. Delete or pause one first (use action list)."),
        ));
    }
    if r.status != "active" {
        r.next_run_at = None;
    }
    let stamp = now.to_rfc3339();
    conn.execute(
        "UPDATE routines SET name = ?3, status = ?4, schedule_type = ?5, schedule_expression = ?6, timezone = ?7,
             config = ?8, max_runs = ?9, next_run_at = ?10, last_run_at = ?11, updated_at = ?12
         WHERE id = ?1 AND user_id = ?2",
        params![
            r.id,
            owner.user_id,
            r.name,
            r.status,
            r.schedule_type,
            r.schedule_expression,
            r.timezone,
            r.config.to_string(),
            r.max_runs,
            r.next_run_at,
            r.last_run_at,
            stamp,
        ],
    )
    .map_err(db_err)?;
    if rearm || body.schedule.is_some() {
        // Resumed or rescheduled: a fresh set of tries, and no stale "finished" mark.
        conn.execute(
            "UPDATE routines SET metadata = json_remove(CASE WHEN json_valid(metadata) THEN metadata ELSE '{}' END,
                 '$.needsAgent', '$.consecutiveFailures', '$.autoPaused', '$.completed')
             WHERE id = ?1 AND user_id = ?2",
            params![r.id, owner.user_id],
        )
        .map_err(db_err)?;
    }
    let fresh = owned(conn, &owner.user_id, &r.id)?;
    Ok(json!({ "automation": info(conn, &fresh, now) }))
}

// ---- delete ----

/// Delete the owner's row. `Ok(Some(job))` when a cloud daemon job goes with it.
pub fn delete(conn: &rusqlite::Connection, owner: &Owner, id: &str) -> Result<Option<String>, Refusal> {
    let r = owned(conn, &owner.user_id, id)?;
    conn.execute("DELETE FROM routines WHERE id = ?1 AND user_id = ?2", params![r.id, owner.user_id]).map_err(db_err)?;
    Ok(r.gizzi_job_id)
}

// ---- routes ----

async fn create_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<CreateBody>) -> Response {
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let in_run = crate::routine_local_scheduler::session_running_automation(body.session_id.trim());
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, Refusal> {
        let conn = db.connect().map_err(db_err)?;
        let (automation, duplicate) = create(&conn, &owner, &body, in_run, Utc::now())?;
        Ok(if duplicate { json!({ "automation": automation, "duplicate": true }) } else { json!({ "automation": automation }) })
    })
    .await
    .unwrap_or_else(|e| Err(refuse(StatusCode::BAD_GATEWAY, "failed", e.to_string())));
    to_response(result)
}

async fn list_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<SessionBody>) -> Response {
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, Refusal> { list(&db.connect().map_err(db_err)?, &owner, Utc::now()) })
        .await
        .unwrap_or_else(|e| Err(refuse(StatusCode::BAD_GATEWAY, "failed", e.to_string())));
    to_response(result)
}

async fn update_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<UpdateBody>) -> Response {
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<Value, Refusal> { update(&db.connect().map_err(db_err)?, &owner, &body, Utc::now()) })
        .await
        .unwrap_or_else(|e| Err(refuse(StatusCode::BAD_GATEWAY, "failed", e.to_string())));
    to_response(result)
}

async fn delete_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<IdBody>) -> Response {
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let db = state.db.clone();
    let id = body.id.trim().to_string();
    let deleted = {
        let id = id.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<String>, Refusal> { delete(&db.connect().map_err(db_err)?, &owner, &id) })
            .await
            .unwrap_or_else(|e| Err(refuse(StatusCode::BAD_GATEWAY, "failed", e.to_string())))
    };
    match deleted {
        Ok(job) => {
            if let Some(job) = job {
                if let Err(e) = crate::automation_routes::cron_daemon_delete_job(&crate::net_client::client(), &job).await {
                    tracing::warn!("failed to delete daemon routine job: {}", e);
                }
            }
            Json(json!({ "deleted": true, "id": id })).into_response()
        }
        Err((status, code, msg)) => err(status, code, msg),
    }
}

/// `queued` when the run posts into the calling thread, whose turn (the one
/// asking) is still going: it runs as soon as that turn ends.
pub fn run_status(r: &crate::routine_local_scheduler::LocalRoutine, calling_session: &str) -> &'static str {
    let this_thread = r.automation_kind() == "message" && r.config.get("target").and_then(Value::as_str) != Some("new_session");
    if this_thread && r.delivery_session() == Some(calling_session.trim()) {
        "queued"
    } else {
        "started"
    }
}

async fn run_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<IdBody>) -> Response {
    let owner = match owner_for(&state, &user, &body.session_id).await {
        Ok(o) => o,
        Err(r) => return r,
    };
    let routine = match crate::routine_local_scheduler::get_routine(&state.db, body.id.trim()) {
        Ok(Some(r)) if r.user_id == owner.user_id => r,
        Ok(_) => return err(StatusCode::NOT_FOUND, "not_found", format!("There's no scheduled task with id {} (use action list).", body.id.trim())),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, "failed", e.to_string()),
    };
    let local: bool = state
        .db
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT execution_domain = 'local' AND gizzi_job_id IS NULL FROM routines WHERE id = ?1", params![routine.id], |r| r.get(0)).ok())
        .unwrap_or(false);
    if !local {
        return err(StatusCode::CONFLICT, "cloud_routine", "That scheduled task runs on the always-on scheduler; run it from the app.");
    }
    if !routine.runnable() {
        return err(StatusCode::CONFLICT, "agent_required", crate::automation_routes::AGENT_REQUIRED_MESSAGE);
    }
    let Some(guard) = crate::routine_local_scheduler::FlightGuard::acquire(&routine.id) else {
        return err(StatusCode::CONFLICT, "already_running", "This scheduled task is already running.");
    };
    let status = run_status(&routine, &body.session_id);
    let id = routine.id.clone();
    let user_id = owner.user_id.clone();
    let state = state.clone();
    tokio::spawn(async move {
        let _guard = guard;
        let driver = crate::routine_local_scheduler::GizziDriver { state: state.clone() };
        crate::routine_local_scheduler::execute(&state.db, &driver, &routine, crate::routine_local_scheduler::Trigger::Manual { user_id }).await;
    });
    Json(json!({ "id": id, "status": status })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routine_local_scheduler::{parse_verdict, Verdict};

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-automation-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await
    }

    fn me(user: &str) -> Owner {
        Owner { user_id: user.into(), bot: None }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-17T15:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn body(session: &str, prompt: &str, schedule: ScheduleBody) -> CreateBody {
        CreateBody {
            session_id: session.into(),
            kind: "message".into(),
            title: None,
            prompt: prompt.into(),
            target: None,
            initiation: None,
            schedule,
            timezone: "America/Chicago".into(),
            max_runs: None,
            until: None,
            schedule_input: Some(json!({ "at": "2026-11-18T09:00" })),
            automation_run: None,
        }
    }

    fn at(s: &str) -> ScheduleBody {
        ScheduleBody::Once { at: s.into() }
    }

    fn cron(s: &str) -> ScheduleBody {
        ScheduleBody::Cron { expression: s.into() }
    }

    #[tokio::test]
    async fn a_task_made_in_a_plain_chat_is_a_local_routine_with_no_bot_that_posts_into_that_session() {
        let st = setup().await;
        let conn = st.db.connect().unwrap();
        let (a, duplicate) = create(&conn, &me("user-a"), &body("ses_plain", "Remind me to call the bank", at("2026-11-18T15:00:00Z")), false, now()).unwrap();
        assert!(!duplicate);
        assert_eq!((a["origin"].as_str(), a["sessionId"].as_str(), a["status"].as_str()), (Some("agent"), Some("ses_plain"), Some("active")));
        assert_eq!(a["schedule"], json!({ "type": "once", "at": "2026-11-18T15:00:00Z" }));
        assert_eq!((a["nextRunAt"].as_str(), a["title"].as_str()), (Some("2026-11-18T15:00:00+00:00"), Some("Remind me to call the bank")));
        let (agent, domain, delivery, origin, initiation): (Option<String>, String, String, String, String) = conn
            .query_row(
                "SELECT agent_id, execution_domain, json_extract(config, '$.delivery.sessionId'), json_extract(metadata, '$.origin'), json_extract(metadata, '$.initiation') FROM routines WHERE id = ?1",
                [a["id"].as_str().unwrap()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((agent, domain.as_str(), delivery.as_str(), origin.as_str(), initiation.as_str()), (None, "local", "ses_plain", "agent", "user_request"));
        // The tick neither flags it "Needs a bot" nor drops its cursor.
        assert_eq!(crate::routine_local_scheduler::flag_agentless(&conn).unwrap(), 0);
        let open: i64 = conn.query_row("SELECT count(*) FROM attention_items WHERE kind = 'routine_needs_bot'", [], |r| r.get(0)).unwrap();
        assert_eq!(open, 0);
    }

    #[tokio::test]
    async fn only_the_owner_lists_changes_runs_or_deletes_a_task() {
        let st = setup().await;
        let conn = st.db.connect().unwrap();
        let (a, _) = create(&conn, &me("user-a"), &body("ses_a", "Check the deploy", cron("0 9 * * 1-5")), false, now()).unwrap();
        let id = a["id"].as_str().unwrap().to_string();
        assert_eq!(list(&conn, &me("user-b"), now()).unwrap()["automations"].as_array().unwrap().len(), 0);
        let pause = UpdateBody { session_id: "ses_b".into(), id: id.clone(), status: Some("paused".into()), ..Default::default() };
        assert_eq!(update(&conn, &me("user-b"), &pause, now()).unwrap_err().1, "not_found");
        assert_eq!(delete(&conn, &me("user-b"), &id).unwrap_err().1, "not_found");
        // The owner sees app-made routines too, and can pause and delete.
        conn.execute(
            "INSERT INTO routines (id, user_id, name, status, schedule_type, schedule_expression, execution_domain, config)
             VALUES ('app-1', 'user-a', 'Morning brief', 'active', 'interval', '1h', 'local', '{}')",
            [],
        )
        .unwrap();
        let listed = list(&conn, &me("user-a"), now()).unwrap();
        let origins: Vec<&str> = listed["automations"].as_array().unwrap().iter().map(|a| a["origin"].as_str().unwrap()).collect();
        assert_eq!(origins.len(), 2);
        assert!(origins.contains(&"user") && origins.contains(&"agent"));
        let paused = update(&conn, &me("user-a"), &pause, now()).unwrap();
        assert_eq!((paused["automation"]["status"].as_str(), paused["automation"]["nextRunAt"].as_str()), (Some("paused"), None));
        assert_eq!(delete(&conn, &me("user-a"), &id).unwrap(), None);
        assert_eq!(list(&conn, &me("user-a"), now()).unwrap()["automations"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn recurring_tasks_run_at_most_every_fifteen_minutes() {
        let n = now();
        assert_eq!(check_schedule(&cron("*/5 * * * *"), "UTC", n).unwrap_err().1, "too_frequent");
        // Two runs ten minutes apart once a day is still too close.
        assert_eq!(check_schedule(&cron("0,10 9 * * *"), "Europe/Paris", n).unwrap_err().1, "too_frequent");
        assert_eq!(check_schedule(&cron("*/15 * * * *"), "UTC", n).unwrap().schedule_type, "cron");
        assert_eq!(check_schedule(&cron("0 9 * * 1"), "America/Chicago", n).unwrap().expression, "0 9 * * 1");
        assert_eq!(check_schedule(&cron("0 9 * *"), "UTC", n).unwrap_err().1, "bad_cron");
        assert_eq!(check_schedule(&cron("0 9 * * *"), "Mars/Base", n).unwrap_err().1, "bad_timezone");
        // One-shots: 1 minute to 365 days ahead.
        assert_eq!(check_schedule(&at("2026-11-17T15:00:30Z"), "UTC", n).unwrap_err().1, "out_of_window");
        assert_eq!(check_schedule(&at("2027-12-01T15:00:00Z"), "UTC", n).unwrap_err().1, "out_of_window");
        assert_eq!(check_schedule(&at("2026-11-17T09:30:00-06:00"), "America/Chicago", n).unwrap().expression, "2026-11-17T15:30:00Z");
    }

    #[tokio::test]
    async fn the_twenty_sixth_active_agent_task_is_refused() {
        let st = setup().await;
        let conn = st.db.connect().unwrap();
        for i in 0..MAX_ACTIVE {
            create(&conn, &me("user-a"), &body("ses_a", &format!("Task {i}"), cron("0 9 * * *")), false, now()).unwrap();
        }
        let refused = create(&conn, &me("user-a"), &body("ses_a", "One more", cron("0 9 * * *")), false, now()).unwrap_err();
        assert_eq!((refused.0, refused.1), (StatusCode::CONFLICT, "too_many"));
        // Another person has their own cap; a paused task frees a slot.
        assert!(create(&conn, &me("user-b"), &body("ses_b", "Theirs", cron("0 9 * * *")), false, now()).is_ok());
        conn.execute("UPDATE routines SET status = 'paused' WHERE id = (SELECT id FROM routines WHERE user_id = 'user-a' LIMIT 1)", []).unwrap();
        assert!(create(&conn, &me("user-a"), &body("ses_a", "One more", cron("0 9 * * *")), false, now()).is_ok());
    }

    #[tokio::test]
    async fn asking_again_for_the_same_task_returns_the_one_there_is() {
        let st = setup().await;
        let conn = st.db.connect().unwrap();
        let (first, _) = create(&conn, &me("user-a"), &body("ses_a", "Check   the build", cron("0 9 * * *")), false, now()).unwrap();
        let (again, duplicate) = create(&conn, &me("user-a"), &body("ses_a", "check the build", cron("0  9 * * *")), false, now()).unwrap();
        assert!(duplicate);
        assert_eq!(again["id"], first["id"]);
        // A one-shot from delay_minutes asked twice lands seconds apart: still the same.
        let (once, _) = create(&conn, &me("user-a"), &body("ses_a", "Stretch", at("2026-11-17T16:00:00Z")), false, now()).unwrap();
        let (twice, dup) = create(&conn, &me("user-a"), &body("ses_a", "Stretch", at("2026-11-17T16:00:20Z")), false, now()).unwrap();
        assert!(dup);
        assert_eq!(twice["id"], once["id"]);
        // Another thread, kind or time is a new task.
        assert!(!create(&conn, &me("user-a"), &body("ses_b", "check the build", cron("0 9 * * *")), false, now()).unwrap().1);
        let mut heartbeat = body("ses_a", "check the build", cron("0 9 * * *"));
        heartbeat.kind = "heartbeat".into();
        assert!(!create(&conn, &me("user-a"), &heartbeat, false, now()).unwrap().1);
        assert!(!create(&conn, &me("user-a"), &body("ses_a", "check the build", cron("0 10 * * *")), false, now()).unwrap().1);
        let n: i64 = conn.query_row("SELECT count(*) FROM routines", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 5);
    }

    #[tokio::test]
    async fn a_run_started_by_a_scheduled_task_cannot_create_one() {
        let st = setup().await;
        let conn = st.db.connect().unwrap();
        let mut marked = body("ses_a", "Make another", cron("0 9 * * *"));
        marked.automation_run = Some(json!({ "id": "r-1", "runId": "run-1" }));
        let refused = create(&conn, &me("user-a"), &marked, false, now()).unwrap_err();
        assert_eq!((refused.0, refused.1), (StatusCode::FORBIDDEN, "automation_run"));
        // A scheduled run going in the calling session is refused the same way.
        let refused = create(&conn, &me("user-a"), &body("ses_a", "Make another", cron("0 9 * * *")), true, now()).unwrap_err();
        assert_eq!(refused.1, "automation_run");
        let n: i64 = conn.query_row("SELECT count(*) FROM routines", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        // An explicit false / null is no marker.
        let mut unmarked = body("ses_a", "Fine", cron("0 9 * * *"));
        unmarked.automation_run = Some(Value::Bool(false));
        assert!(create(&conn, &me("user-a"), &unmarked, false, now()).is_ok());
    }

    #[tokio::test]
    async fn a_new_schedule_turns_a_finished_task_back_on_and_resume_alone_does_not() {
        let st = setup().await;
        let conn = st.db.connect().unwrap();
        let (a, _) = create(&conn, &me("user-a"), &body("ses_a", "Ping", cron("0 9 * * *")), false, now()).unwrap();
        let id = a["id"].as_str().unwrap().to_string();
        assert!(crate::routine_local_scheduler::complete(&conn, crate::routine_local_scheduler::TaskKind::Routine, &id, "max_runs", now()));
        let resume = UpdateBody { session_id: "ses_a".into(), id: id.clone(), status: Some("active".into()), ..Default::default() };
        assert_eq!(update(&conn, &me("user-a"), &resume, now()).unwrap_err().1, "nothing_due");
        let resched = UpdateBody {
            session_id: "ses_a".into(),
            id: id.clone(),
            schedule: Some(cron("30 8 * * *")),
            timezone: Some("America/Chicago".into()),
            max_runs: Some(Some(3)),
            ..Default::default()
        };
        let v = update(&conn, &me("user-a"), &resched, now()).unwrap();
        assert_eq!((v["automation"]["status"].as_str(), v["automation"]["maxRuns"].as_i64()), (Some("active"), Some(3)));
        assert_eq!(v["automation"]["schedule"]["expression"], "30 8 * * *");
        let completed: Option<String> = conn.query_row("SELECT json_extract(metadata, '$.completed') FROM routines WHERE id = ?1", [&id], |r| r.get(0)).unwrap();
        assert_eq!(completed, None);
    }

    #[derive(Deserialize)]
    struct Vector {
        note: String,
        input: String,
        expected: Verdict,
    }

    #[test]
    fn parse_verdict_passes_every_shared_vector() {
        let vectors: Vec<Vector> = serde_json::from_str(include_str!("../../../contracts/automation/heartbeat-verdict.json")).unwrap();
        assert!(vectors.len() >= 25);
        for v in vectors {
            assert_eq!(parse_verdict(&v.input), v.expected, "{}: {:?}", v.note, v.input);
        }
    }

    #[test]
    fn heartbeat_instructions_are_the_shared_text() {
        let prompt = crate::routine_local_scheduler::heartbeat_prompt("  Is PR #42 merged?  ");
        assert!(prompt.starts_with("Is PR #42 merged?\n\n---\nThis is a scheduled check (a heartbeat)."));
        assert!(prompt.ends_with("so do not start work that needs approval."));
    }
}
