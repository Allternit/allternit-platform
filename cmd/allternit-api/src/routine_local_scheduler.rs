//! Local routine scheduler: runs Automation Tasks routines whose
//! `execution_domain = 'local'` (migration V185).
//!
//! Local routines used to be stored and never run — `run_routine` answered
//! 501 "run via local scheduler" and nothing polled
//! `/automation/local-schedules`. The API now runs them itself: on the
//! desktop the API *is* the user's machine, so "local" means here. Cloud and
//! hybrid routines keep going to the gizzi cron daemon.
//!
//! * **Schedule.** `next_run_at` is computed from `schedule_type`:
//!   `interval` (`30m`, `1h`, `1d` …), `cron` (5-field, in this runtime's
//!   local time — the user's own zone on the desktop), `once` (RFC 3339 time,
//!   or immediately when empty). `manual` routines only run on demand, and
//!   `manual` + `config.trigger = "startup"` runs once per API boot.
//! * **Claim.** A tick advances `next_run_at` with a compare-and-set before
//!   running, so two ticks or two processes on one DB never double-fire.
//! * **Bot delivery.** A routine with an `agent_id` posts as a real turn in
//!   that bot's canonical thread (`agents.config.canonicalThreadId`, else
//!   the newest session tagged `botCanonicalFor`, else a new one that gets
//!   pinned), so its output lands where the user already reads the bot. The
//!   previous successful output is carried forward.
//! * **Monitor.** `config.monitor.command` runs through the normal tool path
//!   (`shell.exec`: permission policy + host-control gate). Unchanged output
//!   is a silent `skipped` run with no model turn.
//! * **Phone skill.** `config.replay = {"kind": "phone", "skill": name,
//!   "serial"?: phone serial, "inputs"?: {name: value}}` replays a saved
//!   phone skill (`phone_replay`) through the Desktop phone gateway with no
//!   model turn; only a step whose element moved re-decides, on the
//!   routine's bot's brain. The step report is the run output.
//! * **Record.** Every run is a `routine_runs` row (the Automation Tasks run
//!   history) and, for bots, a `routine.*` event on the bot ledger. The row
//!   is written `running` when the run starts and completed when it ends;
//!   its `metadata` keeps `sessionId`, `model` (the brain that ran it),
//!   `apps` (tools the turn called) and `steps` (its last todo list).
//! * **Live + Stop.** While a run is going, [`live_run`] names it and
//!   `GET /automation/routines/:id/live` reads its steps from the session.
//!   [`request_stop`] (`POST …/runs/:run_id/cancel`) aborts the turn; the run
//!   ends `cancelled` with its output so far and does not count as a failure.
//!   Rows left `running` by a runtime that quit are failed at the next boot.
//! * **Needs a bot.** A local routine with no `agent_id` is never claimed
//!   (it used to fail every run with "local routines need an agent"). The
//!   tick flags it `metadata.needsAgent = true` so the UI can list it under
//!   "Needs a bot"; assigning an agent (PUT `agent_id`) clears the flag.
//! * **Auto-pause.** `metadata.consecutiveFailures` counts failed runs in a
//!   row (reset by any good run). At [`AUTO_PAUSE_AFTER`] the routine is set
//!   to `paused`, `metadata.autoPaused` records why, and the bot ledger gets a
//!   `routine.paused` event, so a broken routine stops and the user is told.
//! * **Loops.** Local loops (`loops`, V285) run through the same [`execute`],
//!   with their runs in `loop_runs`. Differences: a loop with no agent runs as
//!   the owner's main bot; `config.model` overrides the brain; each loop keeps
//!   one session (`loops.session_id`, else `metadata.loopSessionId`) so a loop
//!   every five minutes doesn't fill the bot's main chat; `expires_at` is its
//!   stop rule (the loop model has no iteration cap), and past it the loop is
//!   `disabled`. Occurrences missed while the computer slept aren't recorded:
//!   a loop just runs again on waking. Each due loop runs in its own task.

use chrono::{DateTime, Offset, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{info, warn};

use crate::agent_session_routes::TurnDigest;
use crate::bot_event_routes::{append_event, ActorBody, AppendEventBody};
use crate::db::DbHandle;
use crate::AppState;

const PREVIOUS_OUTPUT_CAP: usize = 2 * 1024;
const MONITOR_OUTPUT_CAP: usize = 4 * 1024;
const RUN_OUTPUT_CAP: usize = 8 * 1024;
/// Failed runs in a row before a routine pauses itself.
pub const AUTO_PAUSE_AFTER: i64 = 3;
/// A run that takes longer than this is cut off and fails with `outcome_reason = timeout`.
pub const DEFAULT_TIMEOUT_SECS: i64 = 1800;
/// Most `skipped` rows written for the occurrences missed in one gap.
const MISSED_ROWS_CAP: usize = 30;
/// Longest a run may start after it was due before it counts as missed.
const MIN_GRACE_SECS: i64 = 300;
/// SQL predicate: the routine has an agent to run it.
const HAS_AGENT: &str = "agent_id IS NOT NULL AND TRIM(agent_id) != ''";
/// SQL expression: the row's metadata as a JSON object (NULL/garbage → {}).
const META: &str = "CASE WHEN json_valid(metadata) THEN metadata ELSE '{}' END";

// ─── Schedule ───────────────────────────────────────────────────────────────

/// Next run strictly after `after`, or `None` when the routine is not
/// tick-scheduled (manual, or a one-shot that already ran).
pub fn next_run(schedule_type: &str, expression: &str, after: DateTime<Utc>, first: bool) -> Option<DateTime<Utc>> {
    match schedule_type {
        "interval" => {
            let secs = crate::automation_routes::parse_interval_seconds(expression)?;
            (secs > 0).then(|| after + chrono::Duration::seconds(secs))
        }
        "cron" => {
            // cron_lite evaluates fields in UTC; shift so "0 9 * * *" means
            // 9:00 in this runtime's local zone.
            let offset = chrono::Local::now().offset().fix().local_minus_utc() as i64;
            let shifted = after + chrono::Duration::seconds(offset);
            crate::cron_lite::next_run_after(expression, shifted)
                .ok()
                .map(|t| t - chrono::Duration::seconds(offset))
        }
        "once" if first => Some(
            DateTime::parse_from_rfc3339(expression.trim())
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or(after),
        ),
        _ => None,
    }
}

// ─── Stored routine (the columns this scheduler needs) ─────────────────────

/// Which kind of task a [`LocalRoutine`] is: where its row and runs live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Routine,
    Loop,
}

impl TaskKind {
    fn table(self) -> &'static str {
        match self {
            TaskKind::Routine => "routines",
            TaskKind::Loop => "loops",
        }
    }

    /// Both tables have the same columns (V285).
    fn runs_table(self) -> &'static str {
        match self {
            TaskKind::Routine => "routine_runs",
            TaskKind::Loop => "loop_runs",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalRoutine {
    pub kind: TaskKind,
    pub id: String,
    pub user_id: String,
    pub agent_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub schedule_type: String,
    pub schedule_expression: String,
    pub config: Value,
    pub metadata: Value,
    pub next_run_at: Option<String>,
    /// Seconds a run may take before it is cut off (`routines.timeout_seconds`; default [`DEFAULT_TIMEOUT_SECS`]).
    pub timeout_seconds: Option<i64>,
    /// How late a scheduled run started, set by [`claim_due`] when it ran past its grace period.
    pub late_by_ms: Option<i64>,
}

const COLS: &str = "id, user_id, agent_id, name, description, schedule_type, schedule_expression, \
                    config, metadata, next_run_at, timeout_seconds";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalRoutine> {
    let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    Ok(LocalRoutine {
        kind: TaskKind::Routine,
        id: row.get(0)?,
        user_id: row.get(1)?,
        agent_id: row.get(2)?,
        name: row.get(3)?,
        description: row.get(4)?,
        schedule_type: row.get(5)?,
        schedule_expression: row.get(6)?,
        config: parse(row.get(7)?),
        metadata: parse(row.get(8)?),
        next_run_at: row.get(9)?,
        timeout_seconds: row.get(10)?,
        late_by_ms: None,
    })
}

/// `COLS` for `loops`: a loop has no timeout of its own, so the default applies.
const LOOP_COLS: &str = "id, user_id, agent_id, name, description, schedule_type, schedule_expression, \
                         config, metadata, next_run_at, NULL";

fn map_loop_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalRoutine> {
    map_row(row).map(|r| LocalRoutine { kind: TaskKind::Loop, ..r })
}

/// The owner's main bot, else their first: what a loop on the Default Model runs as.
fn default_bot(conn: &rusqlite::Connection, user_id: &str) -> Option<String> {
    const CONFIG: &str = "CASE WHEN json_valid(config) THEN config ELSE '{}' END";
    conn.query_row(
        &format!(
            "SELECT id FROM agents
             WHERE user_id = ?1 AND (COALESCE(is_bot, 0) = 1 OR json_extract({CONFIG}, '$.isBot') = 1)
             ORDER BY COALESCE(json_extract({CONFIG}, '$.botProfile.chiefOfStaff'), 0) DESC, created_at, id
             LIMIT 1"
        ),
        params![user_id],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn with_runner(conn: &rusqlite::Connection, mut r: LocalRoutine) -> LocalRoutine {
    if r.agent_id.as_deref().map_or(true, |a| a.trim().is_empty()) {
        r.agent_id = default_bot(conn, &r.user_id);
    }
    r
}

/// A loop ready to run: an agentless one is given the owner's main bot.
pub fn get_loop(db: &DbHandle, id: &str) -> rusqlite::Result<Option<LocalRoutine>> {
    let conn = db.connect()?;
    let found = conn
        .query_row(&format!("SELECT {LOOP_COLS} FROM loops WHERE id = ?1"), params![id], map_loop_row)
        .optional()?;
    Ok(found.map(|r| with_runner(&conn, r)))
}

pub fn get_routine(db: &DbHandle, id: &str) -> rusqlite::Result<Option<LocalRoutine>> {
    db.connect()?
        .query_row(&format!("SELECT {COLS} FROM routines WHERE id = ?1"), params![id], map_row)
        .optional()
}

/// Give every active local routine without a cursor its first run time.
fn backfill(db: &DbHandle, now: DateTime<Utc>) -> rusqlite::Result<()> {
    let conn = db.connect()?;
    flag_agentless(&conn)?;
    let pending: Vec<(String, String, String, Option<String>)> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT id, schedule_type, schedule_expression, last_run_at FROM routines
             WHERE execution_domain = 'local' AND status = 'active' AND next_run_at IS NULL
               AND {HAS_AGENT}
               AND schedule_type IN ('interval', 'cron', 'once')"
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.filter_map(Result::ok).collect()
    };
    for (id, kind, expr, last_run) in pending {
        // A one-shot that already ran stays unscheduled.
        if kind == "once" && last_run.is_some() {
            continue;
        }
        if let Some(next) = next_run(&kind, &expr, now, true) {
            conn.execute(
                "UPDATE routines SET next_run_at = ?2 WHERE id = ?1 AND next_run_at IS NULL",
                params![id, next.to_rfc3339()],
            )?;
        }
    }
    Ok(())
}

/// Mark local routines that have no agent as `metadata.needsAgent` (they are
/// never run until one is assigned) and drop their cursor so they don't sit
/// "due" in the UI.
pub fn flag_agentless(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    let flagged = conn.execute(
        &format!(
            "UPDATE routines SET metadata = json_set({META}, '$.needsAgent', json('true')), next_run_at = NULL
             WHERE execution_domain = 'local' AND NOT ({HAS_AGENT})
               AND (json_extract({META}, '$.needsAgent') IS NOT 1 OR next_run_at IS NOT NULL)"
        ),
        [],
    )?;
    sync_needs_bot_items(conn);
    Ok(flagged)
}

/// One `routine_needs_bot` attention item per active agentless local routine; closed
/// once the routine has a bot (or stops being active). A dismissed item is not
/// re-opened for the same routine.
fn sync_needs_bot_items(conn: &rusqlite::Connection) {
    use crate::netops::alerts;
    let missing: Vec<(String, String, String)> = (|| -> rusqlite::Result<_> {
        let mut st = conn.prepare(&format!(
            "SELECT r.id, r.user_id, r.name FROM routines r
             WHERE r.execution_domain = 'local' AND r.status = 'active' AND NOT ({HAS_AGENT})
               AND NOT EXISTS (SELECT 1 FROM attention_items a WHERE a.id = 'routine_needs_bot:' || r.id
                               AND (a.resolved_at IS NULL OR a.resolution = 'dismissed'))"
        ))?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.filter_map(Result::ok).collect())
    })()
    .unwrap_or_default();
    for (id, owner, name) in missing {
        alerts::open(conn, &alerts::Item {
            owner,
            kind: "routine_needs_bot".into(),
            severity: "warn".into(),
            source: "routine".into(),
            source_id: id,
            summary: format!("\"{name}\" has no bot to run it, so it never runs. Assign it to a bot."),
            detail: json!({ "routine": name }),
            ..Default::default()
        });
    }
    let fixed: Vec<(String, String)> = (|| -> rusqlite::Result<_> {
        let mut st = conn.prepare(&format!(
            "SELECT a.id, a.owner FROM attention_items a WHERE a.kind = 'routine_needs_bot' AND a.resolved_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM routines r WHERE r.id = a.source_id AND r.execution_domain = 'local'
                               AND r.status = 'active' AND NOT ({HAS_AGENT}))"
        ))?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.filter_map(Result::ok).collect())
    })()
    .unwrap_or_default();
    for (id, owner) in fixed {
        alerts::resolve(conn, &owner, &id, "fixed");
    }
}

/// Claim due routines by advancing their cursor first (compare-and-set).
fn claim_due(db: &DbHandle, now: DateTime<Utc>) -> rusqlite::Result<Vec<LocalRoutine>> {
    let conn = db.connect()?;
    let due: Vec<LocalRoutine> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM routines
             WHERE execution_domain = 'local' AND status = 'active' AND {HAS_AGENT}
               AND next_run_at IS NOT NULL AND next_run_at <= ?1
               AND NOT {closed}
             ORDER BY next_run_at",
            closed = crate::thread_closeout::TIED_TO_CLOSED_THREAD
        ))?;
        let rows = stmt.query_map(params![now.to_rfc3339()], map_row)?;
        rows.filter_map(Result::ok).collect()
    };
    let mut claimed = Vec::new();
    for r in due {
        let next = next_run(&r.schedule_type, &r.schedule_expression, now, false).map(|t| t.to_rfc3339());
        let changed = conn.execute(
            "UPDATE routines SET next_run_at = ?2 WHERE id = ?1 AND next_run_at = ?3",
            params![r.id, next, r.next_run_at],
        )?;
        if changed == 1 {
            let mut r = r;
            if let Some(due_at) = r.next_run_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc)) {
                let late = now - due_at;
                let interval = next_run(&r.schedule_type, &r.schedule_expression, due_at, false).map_or(0, |t| (t - due_at).num_seconds());
                let grace = chrono::Duration::seconds(MIN_GRACE_SECS.max(interval / 10));
                if late > grace {
                    r.late_by_ms = Some(late.num_milliseconds());
                    record_missed(&conn, &r, due_at, now);
                }
            }
            claimed.push(r);
        }
    }
    Ok(claimed)
}

/// Stop rule: an active local loop past its `expires_at` is disabled. Returns how many.
fn expire_loops(db: &DbHandle, now: DateTime<Utc>) -> rusqlite::Result<usize> {
    let conn = db.connect()?;
    let dated: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, expires_at FROM loops
             WHERE execution_domain = 'local' AND status = 'active' AND expires_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.filter_map(Result::ok).collect()
    };
    let mut expired = 0;
    for (id, at) in dated {
        // Parsed, not compared as text: the offset can differ.
        let Ok(at) = DateTime::parse_from_rfc3339(at.trim()) else { continue };
        if at.with_timezone(&Utc) <= now {
            expired += conn.execute(
                &format!(
                    "UPDATE loops SET status = 'disabled', next_run_at = NULL,
                         metadata = json_set({META}, '$.expiredAt', ?2)
                     WHERE id = ?1 AND status = 'active'"
                ),
                params![id, now.to_rfc3339()],
            )?;
        }
    }
    Ok(expired)
}

/// Give every active local loop without a cursor its first run time.
fn backfill_loops(db: &DbHandle, now: DateTime<Utc>) -> rusqlite::Result<()> {
    let conn = db.connect()?;
    let pending: Vec<(String, String, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, schedule_type, schedule_expression, last_run_at FROM loops
             WHERE execution_domain = 'local' AND status = 'active' AND next_run_at IS NULL
               AND schedule_type IN ('interval', 'cron', 'once')",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.filter_map(Result::ok).collect()
    };
    for (id, kind, expr, last_run) in pending {
        if kind == "once" && last_run.is_some() {
            continue;
        }
        if let Some(next) = next_run(&kind, &expr, now, true) {
            conn.execute(
                "UPDATE loops SET next_run_at = ?2 WHERE id = ?1 AND next_run_at IS NULL",
                params![id, next.to_rfc3339()],
            )?;
        }
    }
    Ok(())
}

/// Claim due loops by advancing their cursor first (compare-and-set), so two
/// ticks never both run one, and a restart picks up from the stored cursor.
fn claim_due_loops(db: &DbHandle, now: DateTime<Utc>) -> rusqlite::Result<Vec<LocalRoutine>> {
    let conn = db.connect()?;
    let due: Vec<LocalRoutine> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT {LOOP_COLS} FROM loops
             WHERE execution_domain = 'local' AND status = 'active'
               AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY next_run_at"
        ))?;
        let rows = stmt.query_map(params![now.to_rfc3339()], map_loop_row)?;
        rows.filter_map(Result::ok).collect()
    };
    let mut claimed = Vec::new();
    for r in due {
        let next = next_run(&r.schedule_type, &r.schedule_expression, now, false).map(|t| t.to_rfc3339());
        let changed = conn.execute(
            "UPDATE loops SET next_run_at = ?2 WHERE id = ?1 AND next_run_at = ?3",
            params![r.id, next, r.next_run_at],
        )?;
        if changed == 1 {
            claimed.push(with_runner(&conn, r));
        }
    }
    Ok(claimed)
}

/// Stop rules, then first run times, then the claim: one loop tick's bookkeeping.
fn prepare_loops(db: &DbHandle, now: DateTime<Utc>) -> Vec<LocalRoutine> {
    if let Err(e) = expire_loops(db, now) {
        warn!(error = %e, "local loops: expiry failed");
    }
    if let Err(e) = backfill_loops(db, now) {
        warn!(error = %e, "local loops: backfill failed");
    }
    claim_due_loops(db, now).unwrap_or_else(|e| {
        warn!(error = %e, "local loops: claim failed");
        Vec::new()
    })
}

/// A scheduled run started later than its grace period. Every occurrence skipped
/// before the one that is about to run gets a `skipped` row
/// (`outcome_reason = missed_runtime_offline`, at most [`MISSED_ROWS_CAP`]), and a
/// `routine_missed` attention item tells the owner.
fn record_missed(conn: &rusqlite::Connection, r: &LocalRoutine, due_at: DateTime<Utc>, now: DateTime<Utc>) {
    let mut occurrences = Vec::new();
    let mut t = due_at;
    while t <= now && occurrences.len() <= MISSED_ROWS_CAP {
        occurrences.push(t);
        match next_run(&r.schedule_type, &r.schedule_expression, t, false) {
            Some(n) => t = n,
            None => break,
        }
    }
    // The last occurrence is the run that follows; with a long gap the list is cut at the cap.
    let missed = if occurrences.len() > MISSED_ROWS_CAP { MISSED_ROWS_CAP } else { occurrences.len().saturating_sub(1) };
    for at in &occurrences[..missed] {
        let wrote = conn.execute(
            "INSERT INTO routine_runs (id, routine_id, status, scheduled_at, started_at, finished_at, duration_ms,
                 output, error, triggered_by, metadata, outcome_reason)
             VALUES (?1, ?2, 'skipped', ?3, ?3, ?4, 0, 'missed', NULL, 'schedule', '{}', 'missed_runtime_offline')",
            params![uuid::Uuid::new_v4().to_string(), r.id, at.to_rfc3339(), now.to_rfc3339()],
        );
        if let Err(e) = wrote {
            warn!(routine = %r.id, error = %e, "could not record a missed run");
        }
    }
    let late_min = r.late_by_ms.unwrap_or(0) / 60_000;
    let summary = if missed > 0 {
        format!("\"{}\" missed {missed} run{} while this computer was off or asleep.", r.name, if missed == 1 { "" } else { "s" })
    } else {
        format!("\"{}\" started {late_min} min late: this computer was off or asleep when it was due.", r.name)
    };
    crate::netops::alerts::open(conn, &crate::netops::alerts::Item {
        owner: r.user_id.clone(),
        kind: "routine_missed".into(),
        severity: "warn".into(),
        bot_id: r.agent_id.clone(),
        source: "routine".into(),
        source_id: r.id.clone(),
        summary,
        detail: json!({ "routine": r.name, "dueAt": due_at.to_rfc3339(), "lateByMs": r.late_by_ms, "missedRuns": missed }),
        ..Default::default()
    });
}

// ─── Delivery ───────────────────────────────────────────────────────────────

/// How a routine reaches the bot. Production drives gizzi and the tool path;
/// tests substitute a recorder.
pub trait RoutineDriver: Send + Sync {
    fn session_exists(&self, session_id: &str) -> impl Future<Output = bool> + Send;
    fn create_session(&self, bot_id: &str, bot_name: &str) -> impl Future<Output = Result<String, String>> + Send;
    fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> impl Future<Output = Result<String, String>> + Send;
    /// `send_turn` with `"provider/model"` in place of the bot's own model (a loop's `config.model`).
    fn send_turn_on_model(&self, session_id: &str, bot_id: &str, text: &str, model: &str) -> impl Future<Output = Result<String, String>> + Send;
    fn run_monitor(&self, user_id: &str, command: &str) -> impl Future<Output = Result<String, String>> + Send;
    /// The session a run goes to: the routine's own standing thread, a fresh
    /// generation per run (P4.4). `None` → the bot's main thread.
    fn routine_session(
        &self,
        r: &LocalRoutine,
        agent_id: &str,
        instruction: &str,
    ) -> impl Future<Output = Option<Result<String, String>>> + Send;
    /// `config.replay = {kind: "phone", skill, serial?, inputs?}`: replay a
    /// saved phone skill with no model turn (`phone_replay`). `Ok` is the
    /// run report as JSON text; a diverged or failed replay is `Err`.
    fn run_phone_skill(
        &self,
        r: &LocalRoutine,
        skill: &str,
        serial: Option<&str>,
        inputs: &[(String, String)],
    ) -> impl Future<Output = Result<String, String>> + Send {
        let _ = (r, skill, serial, inputs);
        std::future::ready(Err("phone skills can't run on this driver".to_string()))
    }
    /// What the session's newest turn did so far (text, tools, todo steps, brain).
    fn turn_digest(&self, session_id: &str) -> impl Future<Output = Option<TurnDigest>> + Send {
        let _ = session_id;
        std::future::ready(None)
    }
    /// Stop the turn running in `session_id` (a user Stop).
    fn abort_session(&self, session_id: &str) -> impl Future<Output = Result<(), String>> + Send {
        let _ = session_id;
        std::future::ready(Ok(()))
    }
}

pub struct GizziDriver {
    pub state: Arc<AppState>,
}

impl RoutineDriver for GizziDriver {
    async fn session_exists(&self, session_id: &str) -> bool {
        crate::agent_session_routes::bot_session_exists(&self.state.db, session_id).await
    }

    async fn create_session(&self, bot_id: &str, bot_name: &str) -> Result<String, String> {
        crate::agent_session_routes::create_bot_session(&self.state.db, bot_id, bot_name).await
    }

    async fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> Result<String, String> {
        crate::agent_session_routes::send_bot_turn(&self.state.db, session_id, bot_id, text).await
    }

    async fn send_turn_on_model(&self, session_id: &str, bot_id: &str, text: &str, model: &str) -> Result<String, String> {
        crate::agent_session_routes::send_bot_turn_on_model(&self.state.db, session_id, bot_id, text, model).await
    }

    async fn routine_session(&self, r: &LocalRoutine, agent_id: &str, instruction: &str) -> Option<Result<String, String>> {
        let rt = crate::thread_routes::GizziRuntime { db: self.state.db.clone() };
        Some(
            crate::thread_routes::routine_generation(&self.state.db, &rt, &r.user_id, agent_id, &r.id, &r.name, instruction)
                .await,
        )
    }

    async fn run_phone_skill(&self, r: &LocalRoutine, skill: &str, serial: Option<&str>, inputs: &[(String, String)]) -> Result<String, String> {
        // A moved step re-decides on the routine's bot's brain.
        let model = crate::phone_replay::brain(&self.state.db, r.agent_id.as_deref(), None);
        let report = crate::phone_replay::run_skill(&self.state.data_dir, skill, serial, inputs, model, "routine", None).await?;
        crate::phone_runs::record_run_completed(&self.state.db, &report, "routine");
        if report.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(phone_skill_summary(skill, &report))
        } else {
            Err(report.get("error").and_then(Value::as_str).unwrap_or("the phone skill didn't finish").to_string())
        }
    }

    async fn turn_digest(&self, session_id: &str) -> Option<TurnDigest> {
        crate::agent_session_routes::latest_turn_digest(&self.state.db, session_id).await
    }

    async fn abort_session(&self, session_id: &str) -> Result<(), String> {
        crate::agent_session_routes::abort_automation_session(&self.state.db, session_id).await
    }

    async fn run_monitor(&self, user_id: &str, command: &str) -> Result<String, String> {
        use crate::permission_policy::{evaluate, PermissionAction};
        match evaluate(self.state.config.active_permission_policy().as_ref(), "shell.exec", None, None) {
            PermissionAction::Deny => return Err("shell.exec is denied by the permission policy".into()),
            PermissionAction::Ask => {
                return Err("shell.exec needs approval under the current policy; a scheduled monitor can't ask".into())
            }
            PermissionAction::Allow => {}
        }
        let request = crate::tool_routes::ExecuteToolRequest {
            tool: "shell.exec".to_string(),
            args: json!({ "command": command }),
            timeout: Some(60),
            ..Default::default()
        };
        let result = crate::tool_routes::execute_tool_internal(&self.state, &request, user_id, None).await?;
        if result.get("exit_code").and_then(Value::as_i64).unwrap_or(0) != 0 {
            let stderr = result.get("stderr").and_then(Value::as_str).unwrap_or("");
            return Err(format!("monitor command failed: {}", truncate(stderr, 512)));
        }
        Ok(result.get("stdout").and_then(Value::as_str).unwrap_or("").to_string())
    }
}

/// The run output for a replayed phone skill: one line per step.
fn phone_skill_summary(skill: &str, report: &Value) -> String {
    let steps = report.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = format!(
        "Phone skill \"{skill}\" ran {} step{} with {} model call{} (run {}).",
        steps.len(),
        if steps.len() == 1 { "" } else { "s" },
        report.get("model_calls").and_then(Value::as_u64).unwrap_or(0),
        if report.get("model_calls").and_then(Value::as_u64) == Some(1) { "" } else { "s" },
        report.get("run_id").and_then(Value::as_str).unwrap_or("?"),
    );
    for s in steps {
        out.push_str(&format!(
            "\n- {}{}",
            s.get("label").and_then(Value::as_str).unwrap_or("step"),
            s.get("via").and_then(Value::as_str).map(|v| format!(" (found by {v})")).unwrap_or_default()
        ));
    }
    out
}

/// `config.replay` when it names a phone skill: (skill, serial, inputs).
fn phone_replay_of(config: &Value) -> Option<(String, Option<String>, Vec<(String, String)>)> {
    let replay = config.get("replay")?;
    if replay.get("kind").and_then(Value::as_str) != Some("phone") {
        return None;
    }
    let skill = replay.get("skill").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())?.to_string();
    let serial = replay.get("serial").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let inputs = replay.get("inputs").and_then(Value::as_object).map(crate::phone_replay::inputs_of).unwrap_or_default();
    Some((skill, serial, inputs))
}

fn truncate(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// FNV-1a 32-bit hex, identical to the web `fnv1aHex`.
pub fn fnv1a_hex(input: &str) -> String {
    let mut hash: u32 = 0x811c9dc5;
    for unit in input.encode_utf16() {
        hash ^= unit as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    format!("{hash:08x}")
}

/// The routine's instruction: explicit config first, then the Automation
/// form's description, then its name.
fn instruction_of(r: &LocalRoutine) -> String {
    ["prompt", "instruction", "message"]
        .iter()
        .find_map(|k| r.config.get(*k).and_then(Value::as_str).filter(|s| !s.trim().is_empty()))
        .map(str::to_string)
        .or_else(|| r.description.clone().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| r.name.clone())
}

struct BotInfo {
    name: String,
    is_bot: bool,
    pinned_thread: Option<String>,
}

fn bot_info(db: &DbHandle, agent_id: &str) -> Option<BotInfo> {
    let conn = db.connect().ok()?;
    // The bot profile lives in config.botProfile (no dedicated column).
    let (name, is_bot, config): (String, i64, Option<String>) = conn
        .query_row(
            "SELECT name, COALESCE(is_bot, 0), config FROM agents WHERE id = ?1",
            params![agent_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()?;
    let config: Value = config.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let display = config
        .pointer("/botProfile/displayName")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or(name);
    Some(BotInfo {
        name: display,
        is_bot: is_bot == 1 || config.get("isBot").and_then(Value::as_bool).unwrap_or(false),
        pinned_thread: config.get("canonicalThreadId").and_then(Value::as_str).map(str::to_string),
    })
}

/// The bot's canonical thread: its pin, else the newest session tagged
/// `botCanonicalFor`, else a new session that becomes the pin.
async fn resolve_thread<D: RoutineDriver>(db: &DbHandle, driver: &D, bot_id: &str, bot: &BotInfo) -> Result<String, String> {
    if let Some(pin) = bot.pinned_thread.as_deref() {
        if driver.session_exists(pin).await {
            return Ok(pin.to_string());
        }
    }
    let tagged: Option<String> = db.connect().ok().and_then(|conn| {
        conn.query_row(
            "SELECT session_id FROM session_metadata
             WHERE json_extract(metadata, '$.botCanonicalFor') = ?1 ORDER BY rowid DESC LIMIT 1",
            params![bot_id],
            |r| r.get(0),
        )
        .ok()
    });
    let session = match tagged {
        Some(id) if driver.session_exists(&id).await => id,
        _ => driver.create_session(bot_id, &bot.name).await?,
    };
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "UPDATE agents SET config = json_set(COALESCE(config, '{}'), '$.canonicalThreadId', ?2) WHERE id = ?1",
            params![bot_id, session],
        );
    }
    Ok(session)
}

/// The session a loop runs in: `loops.session_id`, else the one it was given
/// before (`metadata.loopSessionId`), else a new one it keeps.
async fn loop_session<D: RoutineDriver>(db: &DbHandle, driver: &D, r: &LocalRoutine, agent_id: &str, bot: &BotInfo) -> Result<String, String> {
    let own: Option<String> = db.connect().ok().and_then(|conn| {
        conn.query_row("SELECT session_id FROM loops WHERE id = ?1", params![r.id], |row| row.get(0)).ok().flatten()
    });
    let kept = r.metadata.get("loopSessionId").and_then(Value::as_str).map(str::to_string);
    for session in own.into_iter().chain(kept) {
        if driver.session_exists(&session).await {
            return Ok(session);
        }
    }
    let session = driver.create_session(agent_id, &format!("{}: {}", bot.name, r.name)).await?;
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            &format!("UPDATE loops SET metadata = json_set({META}, '$.loopSessionId', ?2) WHERE id = ?1"),
            params![r.id, session],
        );
    }
    Ok(session)
}

fn previous_output(db: &DbHandle, r: &LocalRoutine) -> Option<String> {
    db.connect().ok()?.query_row(
        &format!(
            "SELECT output FROM {} WHERE routine_id = ?1 AND status = 'succeeded' AND output IS NOT NULL
             ORDER BY started_at DESC LIMIT 1",
            r.kind.runs_table()
        ),
        params![r.id],
        |r| r.get(0),
    )
    .ok()
}

/// Routine delivery text. gizzi recognises the `[routine: <label>]` prefix
/// (`runtime/bots/bot-routines.ts` ROUTINE_MARKER_PREFIX) and keeps the turn
/// quiet — no first-response preamble — the same as CLI bot routines.
pub fn routine_turn(label: &str, body: &str) -> String {
    // Bot routines are named "[bot:<name>] <title>"; the marker carries the title.
    let label = match (label.starts_with("[bot:"), label.find(']')) {
        (true, Some(end)) => label[end + 1..].trim(),
        _ => label.trim(),
    };
    let label = if label.is_empty() { "routine" } else { label };
    format!("[routine: {label}] {body}")
}

pub enum Trigger {
    Schedule,
    Startup,
    Manual { user_id: String },
}

impl Trigger {
    fn as_str(&self) -> &'static str {
        match self {
            Trigger::Schedule => "schedule",
            Trigger::Startup => "startup",
            Trigger::Manual { .. } => "manual",
        }
    }
}

/// Outcome of one run, as stored in `routine_runs.status`.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub status: &'static str,
    pub output: Option<String>,
    pub error: Option<String>,
    pub session_id: Option<String>,
    /// This failure was the [`AUTO_PAUSE_AFTER`]th in a row and paused the routine.
    pub auto_paused: bool,
    /// Why a run did not succeed (`routine_runs.outcome_reason`), see [`classify`].
    pub reason: Option<&'static str>,
}

/// `routine_runs.outcome_reason` for a failed run, from its error text.
pub fn classify(error: &str) -> &'static str {
    let e = error.to_lowercase();
    if e.contains("timed out") {
        "timeout"
    } else if e.contains("has no bot") || e.contains("no longer exists") || e.contains("needs an agent") {
        "no_bot"
    } else if e.contains("lane_blocked") || e.contains("auth_required") || e.contains("challenge") || e.contains("sign in") || e.contains("signed out") {
        "lane_blocked"
    } else if e.contains("quota") || e.contains("rate_limit") || e.contains("rate limit") || e.contains("429") || e.contains("credit") || e.contains("billing") {
        "quota"
    } else if e.contains("restart") || e.contains("aborted") || e.contains("shutting down") {
        "runtime_restart"
    } else {
        "model_error"
    }
}

/// User-facing reason stored in `metadata.autoPaused.reason`.
pub fn auto_pause_reason() -> String {
    format!("Paused after {AUTO_PAUSE_AFTER} failed runs in a row. Fix the cause, then resume it.")
}

/// Update the failure streak after a run; pauses the routine at
/// [`AUTO_PAUSE_AFTER`]. Returns true when this run paused it.
fn record_streak(conn: &rusqlite::Connection, kind: TaskKind, id: &str, status: &str, error: Option<&str>, at: &str) -> bool {
    let table = kind.table();
    // A stopped run says nothing about whether the routine works.
    if status == "cancelled" {
        return false;
    }
    if status != "failed" {
        let _ = conn.execute(
            &format!(
                "UPDATE {table} SET metadata = json_remove({META}, '$.consecutiveFailures', '$.lastRunError')
                 WHERE id = ?1"
            ),
            params![id],
        );
        return false;
    }
    let _ = conn.execute(
        &format!(
            "UPDATE {table} SET metadata = json_set({META},
                 '$.consecutiveFailures', COALESCE(json_extract({META}, '$.consecutiveFailures'), 0) + 1,
                 '$.lastRunError', ?2)
             WHERE id = ?1"
        ),
        params![id, error],
    );
    // Only an active routine pauses itself (a manual run of a paused one
    // just records the failure).
    let paused = conn
        .execute(
            &format!(
                "UPDATE {table} SET status = 'paused', next_run_at = NULL,
                     metadata = json_set({META}, '$.autoPaused',
                         json_object('at', ?2, 'reason', ?3, 'lastError', ?4))
                 WHERE id = ?1 AND status = 'active'
                   AND COALESCE(json_extract({META}, '$.consecutiveFailures'), 0) >= ?5"
            ),
            params![id, at, auto_pause_reason(), error, AUTO_PAUSE_AFTER],
        )
        .unwrap_or(0);
    paused == 1
}

/// The run's outcome could not be stored: keep it as a `dead_letter` attention item so it is never lost.
fn dead_letter(db: &DbHandle, r: &LocalRoutine, outcome: &RunOutcome, run_id: &str, why: String) {
    warn!(routine = %r.id, error = %why, "could not record a routine run");
    let item = crate::netops::alerts::Item {
        owner: r.user_id.clone(),
        kind: "dead_letter".into(),
        severity: "warn".into(),
        bot_id: r.agent_id.clone(),
        source: "routine".into(),
        source_id: run_id.to_string(),
        summary: format!("A run of \"{}\" finished ({}) but its record could not be saved.", r.name, outcome.status),
        detail: json!({ "routineId": r.id, "status": outcome.status, "error": outcome.error, "output": outcome.output.as_deref().map(|o| truncate(o, 1024)), "reason": outcome.reason, "storeError": why }),
        ..Default::default()
    };
    // A fresh connection: the one that failed may be the problem.
    if let Ok(c) = db.connect() {
        crate::netops::alerts::open(&c, &item);
    }
}

/// Tell the owner about a failed run, or close the open problems after a good one.
/// Every failure opens `routine_failed` (also when the routine has no bot); an
/// auto-pause escalates it to critical. A good run resolves it, and resolves
/// `routine_missed` when it started on time.
fn alert_for_outcome(conn: &rusqlite::Connection, r: &LocalRoutine, outcome: &RunOutcome, run_id: &str, trigger: &Trigger) {
    use crate::netops::alerts;
    if outcome.status == "cancelled" {
        return;
    }
    if outcome.status != "failed" {
        alerts::resolve_source(conn, &r.user_id, "routine_failed", &r.id, "fixed");
        if r.late_by_ms.is_none() {
            alerts::resolve_source(conn, &r.user_id, "routine_missed", &r.id, "fixed");
        }
        return;
    }
    let error = outcome.error.as_deref().unwrap_or("it failed");
    let flagged_critical = r.metadata.get("critical").and_then(Value::as_bool).unwrap_or(false);
    let (severity, summary) = if outcome.auto_paused {
        ("critical", format!("\"{}\" was paused after {AUTO_PAUSE_AFTER} failed runs in a row: {}", r.name, truncate(error, 160)))
    } else {
        (if flagged_critical { "critical" } else { "warn" }, format!("\"{}\" failed: {}", r.name, truncate(error, 160)))
    };
    alerts::open(conn, &alerts::Item {
        owner: r.user_id.clone(),
        kind: "routine_failed".into(),
        severity: severity.into(),
        bot_id: r.agent_id.clone(),
        source: if r.kind == TaskKind::Loop { "loop".into() } else { "routine".into() },
        source_id: r.id.clone(),
        summary,
        detail: json!({ "routine": r.name, "runId": run_id, "reason": outcome.reason, "error": error, "trigger": trigger.as_str(), "autoPaused": outcome.auto_paused }),
        // The retry action runs routines.
        retry: (r.kind == TaskKind::Routine).then(|| json!({ "routineId": r.id })),
        ..Default::default()
    });
    let _ = conn.execute(
        &format!("UPDATE {} SET alerted_at = ?2 WHERE id = ?1", r.kind.runs_table()),
        params![run_id, Utc::now().to_rfc3339()],
    );
}

// ─── Live runs ──────────────────────────────────────────────────────────────

/// A run in progress: its `routine_runs` row, the session it runs in once
/// known, and the signal a user Stop sends.
struct LiveRun {
    run_id: String,
    session_id: Option<String>,
    stop: Arc<tokio::sync::Notify>,
}

fn live_runs() -> &'static Mutex<HashMap<String, LiveRun>> {
    static MAP: OnceLock<Mutex<HashMap<String, LiveRun>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn with_live<T>(f: impl FnOnce(&mut HashMap<String, LiveRun>) -> T) -> T {
    f(&mut live_runs().lock().unwrap_or_else(|e| e.into_inner()))
}

/// The run in progress for a routine: `(run_id, session_id)`.
pub fn live_run(routine_id: &str) -> Option<(String, Option<String>)> {
    with_live(|m| m.get(routine_id).map(|l| (l.run_id.clone(), l.session_id.clone())))
}

/// Ask the routine's run in progress to stop. False when `run_id` is not
/// running here. The run ends `cancelled` and keeps its output so far.
pub fn request_stop(routine_id: &str, run_id: &str) -> bool {
    with_live(|m| match m.get(routine_id) {
        Some(l) if l.run_id == run_id => {
            l.stop.notify_one();
            true
        }
        _ => false,
    })
}

/// Unregisters the live run when `execute` returns (or is dropped).
struct LiveGuard(String);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        with_live(|m| m.remove(&self.0));
    }
}

/// Error text a stopped run's work ends with.
const STOPPED: &str = "stopped by user";

/// Runs left `running` by a runtime that stopped mid-run (crash, quit) can
/// never finish: mark them failed. Cloud runs (`gizzi_run_id`) are the
/// daemon's to settle.
pub fn settle_orphaned_runs(db: &DbHandle) -> rusqlite::Result<usize> {
    let now = Utc::now().to_rfc3339();
    let conn = db.connect()?;
    let mut settled = 0;
    for table in ["routine_runs", "loop_runs"] {
        settled += conn.execute(
            &format!(
                "UPDATE {table} SET status = 'failed', finished_at = ?1, outcome_reason = 'runtime_restart',
                     error = 'The runtime stopped while this run was going.'
                 WHERE status = 'running' AND gizzi_run_id IS NULL"
            ),
            params![now],
        )?;
    }
    Ok(settled)
}

/// Run one routine end to end and record it. Never panics on a bad run —
/// the failure is the recorded result. The `routine_runs` row is written
/// `running` when the run starts (so the UI can show it live and stop it)
/// and completed when it ends.
pub async fn execute<D: RoutineDriver>(db: &DbHandle, driver: &D, r: &LocalRoutine, trigger: Trigger) -> RunOutcome {
    let started = Utc::now();
    let mut monitor_hash: Option<String> = None;
    let bot = r.agent_id.as_deref().and_then(|id| bot_info(db, id));

    let runs_table = r.kind.runs_table();
    let run_id = uuid::Uuid::new_v4().to_string();
    let opened = db
        .connect()
        .and_then(|conn| {
            conn.execute(
                &format!(
                    "INSERT INTO {runs_table} (id, routine_id, status, scheduled_at, started_at, triggered_by, metadata, late_by_ms)
                     VALUES (?1, ?2, 'running', ?3, ?3, ?4, '{{}}', ?5)"
                ),
                params![run_id, r.id, started.to_rfc3339(), trigger.as_str(), r.late_by_ms],
            )
        })
        .is_ok();
    let stop = Arc::new(tokio::sync::Notify::new());
    with_live(|m| m.insert(r.id.clone(), LiveRun { run_id: run_id.clone(), session_id: None, stop: stop.clone() }));
    let _live = LiveGuard(r.id.clone());
    let note_session = |session: &str| {
        with_live(|m| {
            if let Some(l) = m.get_mut(&r.id) {
                l.session_id = Some(session.to_string());
            }
        });
        if let Ok(conn) = db.connect() {
            let _ = conn.execute(
                &format!("UPDATE {runs_table} SET metadata = json_set(COALESCE(metadata, '{{}}'), '$.sessionId', ?2) WHERE id = ?1"),
                params![run_id, session],
            );
        }
    };

    let timeout_secs = r.timeout_seconds.filter(|t| *t > 0).unwrap_or(DEFAULT_TIMEOUT_SECS);
    let work = async {
        // A phone skill replays with no model turn and needs no bot thread.
        if let Some((skill, serial, inputs)) = phone_replay_of(&r.config) {
            let out = driver.run_phone_skill(r, &skill, serial.as_deref(), &inputs).await?;
            return Ok((out, "succeeded"));
        }
        let agent_id = r.agent_id.as_deref().ok_or("this routine has no bot; assign it to a bot to run it")?;
        let bot = bot.as_ref().ok_or("the routine's agent no longer exists")?;
        let text = if let Some(command) = r.config.pointer("/monitor/command").and_then(Value::as_str) {
            let out = truncate(&driver.run_monitor(&r.user_id, command).await?, MONITOR_OUTPUT_CAP);
            let hash = fnv1a_hex(&out);
            if r.metadata.get("lastMonitorHash").and_then(Value::as_str) == Some(hash.as_str()) {
                return Ok(("no change".to_string(), "skipped"));
            }
            monitor_hash = Some(hash);
            routine_turn(&r.name, &out)
        } else {
            let mut instruction = instruction_of(r);
            if let Some(prev) = previous_output(db, r) {
                instruction = format!("{instruction}\n\nPrevious run output:\n{}", truncate(&prev, PREVIOUS_OUTPUT_CAP));
            }
            routine_turn(&r.name, &instruction)
        };
        let session = if r.kind == TaskKind::Loop {
            loop_session(db, driver, r, agent_id, bot).await?
        } else {
            match driver.routine_session(r, agent_id, &instruction_of(r)).await {
                Some(result) => result?,
                None => resolve_thread(db, driver, agent_id, bot).await?,
            }
        };
        note_session(&session);
        // A loop's `config.model`, when it's the "provider/model" a brain needs.
        let model = r.config.get("model").and_then(Value::as_str).filter(|m| r.kind == TaskKind::Loop && m.contains('/'));
        let reply = match model {
            Some(model) => driver.send_turn_on_model(&session, agent_id, &text, model).await?,
            None => driver.send_turn(&session, agent_id, &text).await?,
        };
        Ok((reply, "succeeded"))
    };
    let timed = async {
        match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs as u64), work).await {
            Ok(res) => res,
            Err(_) => Err(format!(
                "timed out after {} (this routine's limit)",
                if timeout_secs % 60 == 0 { format!("{} min", timeout_secs / 60) } else { format!("{timeout_secs} s") }
            )),
        }
    };
    let result: Result<(String, &'static str), String> = tokio::select! {
        res = timed => res,
        _ = stop.notified() => Err(STOPPED.to_string()),
    };
    let session_used = live_run(&r.id).and_then(|(_, s)| s);
    let stopped = result.as_ref().err().is_some_and(|e| e == STOPPED);
    if stopped {
        if let Some(session) = session_used.as_deref() {
            if let Err(e) = driver.abort_session(session).await {
                warn!(routine = %r.id, error = %e, "could not stop the routine's turn");
            }
        }
    }
    let digest = match session_used.as_deref() {
        Some(session) => driver.turn_digest(session).await,
        None => None,
    };

    let outcome = match result {
        Ok((out, status)) => RunOutcome {
            status,
            output: Some(truncate(&out, RUN_OUTPUT_CAP)),
            error: None,
            session_id: session_used,
            auto_paused: false,
            reason: None,
        },
        Err(_) if stopped => RunOutcome {
            status: "cancelled",
            output: digest.as_ref().map(|d| d.text.clone()).filter(|t| !t.is_empty()).map(|t| truncate(&t, RUN_OUTPUT_CAP)),
            error: None,
            session_id: session_used,
            auto_paused: false,
            reason: Some("stopped"),
        },
        Err(e) => RunOutcome {
            status: "failed",
            output: None,
            reason: Some(classify(&e)),
            error: Some(truncate(&e, RUN_OUTPUT_CAP)),
            session_id: session_used,
            auto_paused: false,
        },
    };
    let mut outcome = outcome;
    let finished = Utc::now();
    let run_meta = {
        let mut m = json!({ "sessionId": outcome.session_id });
        if let Some(d) = &digest {
            m["apps"] = json!(d.tools);
            if !d.steps.is_empty() {
                m["steps"] = json!(d.steps);
            }
            if let Some(model) = &d.model {
                m["model"] = json!(model);
            }
        }
        m
    };

    if let Ok(conn) = db.connect() {
        let stored = if opened {
            conn.execute(
                &format!(
                    "UPDATE {runs_table} SET status = ?2, finished_at = ?3, duration_ms = ?4, output = ?5, error = ?6,
                         metadata = ?7, outcome_reason = ?8
                     WHERE id = ?1"
                ),
                params![
                    run_id,
                    outcome.status,
                    finished.to_rfc3339(),
                    (finished - started).num_milliseconds(),
                    outcome.output,
                    outcome.error,
                    run_meta.to_string(),
                    outcome.reason,
                ],
            )
            .and_then(|n| if n == 1 { Ok(n) } else { Err(rusqlite::Error::QueryReturnedNoRows) })
        } else {
            conn.execute(
                &format!(
                    "INSERT INTO {runs_table} (id, routine_id, status, scheduled_at, started_at, finished_at, duration_ms,
                         output, error, triggered_by, metadata, outcome_reason, late_by_ms)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ),
                params![
                    run_id,
                    r.id,
                    outcome.status,
                    started.to_rfc3339(),
                    finished.to_rfc3339(),
                    (finished - started).num_milliseconds(),
                    outcome.output,
                    outcome.error,
                    trigger.as_str(),
                    run_meta.to_string(),
                    outcome.reason,
                    r.late_by_ms,
                ],
            )
        };
        if let Err(e) = stored {
            dead_letter(db, r, &outcome, &run_id, e.to_string());
        }
        let _ = conn.execute(
            &format!(
                "UPDATE {} SET last_run_at = ?2,
                     metadata = json_set(
                         CASE WHEN ?3 IS NULL THEN COALESCE(metadata, '{{}}')
                              ELSE json_set(COALESCE(metadata, '{{}}'), '$.lastMonitorHash', ?3) END,
                         '$.lastRunStatus', ?4)
                 WHERE id = ?1",
                r.kind.table()
            ),
            params![r.id, started.to_rfc3339(), monitor_hash, outcome.status],
        );
        outcome.auto_paused =
            record_streak(&conn, r.kind, &r.id, outcome.status, outcome.error.as_deref(), &finished.to_rfc3339());
        alert_for_outcome(&conn, r, &outcome, &run_id, &trigger);
    } else {
        dead_letter(db, r, &outcome, &run_id, "database unavailable".into());
    }

    if let (Some(agent_id), Some(bot)) = (r.agent_id.as_deref(), bot.as_ref()) {
        if bot.is_bot {
            let (actor_type, actor_id) = match &trigger {
                Trigger::Manual { user_id } => ("user", user_id.clone()),
                _ => ("routine", r.id.clone()),
            };
            let event = AppendEventBody {
                event_type: format!("routine.{}", match outcome.status { "succeeded" => "completed", s => s }),
                actor: ActorBody { r#type: actor_type.to_string(), id: actor_id },
                payload: json!({
                    "routineId": r.id,
                    "title": r.name,
                    "trigger": trigger.as_str(),
                    "output": outcome.output.as_deref().map(|o| truncate(o, 1024)),
                    "error": outcome.error,
                    "durationMs": (finished - started).num_milliseconds(),
                }),
                occurred_at: None,
                session_id: outcome.session_id.clone(),
                goal_id: None,
                wih_id: None,
                task_id: None,
                run_id: None,
                idempotency_key: Some(format!("{}:{}", r.id, started.timestamp_millis())),
            };
            if let Err(e) = append_event(db, agent_id, &event, &finished.to_rfc3339()) {
                warn!(routine = %r.id, error = %e, "failed to ledger routine run");
            }
            if outcome.auto_paused {
                let paused = AppendEventBody {
                    event_type: "routine.paused".to_string(),
                    actor: ActorBody { r#type: "routine".to_string(), id: r.id.clone() },
                    payload: json!({
                        "routineId": r.id,
                        "title": r.name,
                        "auto": true,
                        "consecutiveFailures": AUTO_PAUSE_AFTER,
                        "reason": auto_pause_reason(),
                        "error": outcome.error,
                    }),
                    occurred_at: None,
                    session_id: outcome.session_id.clone(),
                    goal_id: None,
                    wih_id: None,
                    task_id: None,
                    run_id: None,
                    idempotency_key: Some(format!("{}:paused:{}", r.id, started.timestamp_millis())),
                };
                if let Err(e) = append_event(db, agent_id, &paused, &finished.to_rfc3339()) {
                    warn!(routine = %r.id, error = %e, "failed to ledger routine auto-pause");
                }
            }
        }
    }
    if outcome.auto_paused {
        warn!(routine = %r.id, "routine paused after {AUTO_PAUSE_AFTER} failed runs in a row");
    }
    outcome
}

// ─── Tick ───────────────────────────────────────────────────────────────────

fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Holds a routine id for the duration of a run (tick vs Run now).
pub struct FlightGuard(String);

impl FlightGuard {
    pub fn acquire(id: &str) -> Option<Self> {
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        set.insert(id.to_string()).then(|| FlightGuard(id.to_string()))
    }
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        in_flight().lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

pub async fn run_due<D: RoutineDriver>(db: &DbHandle, driver: &D, now: DateTime<Utc>) -> usize {
    if let Err(e) = backfill(db, now) {
        warn!(error = %e, "local routines: backfill failed");
    }
    let claimed = match claim_due(db, now) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "local routines: claim failed");
            return 0;
        }
    };
    let mut fired = 0;
    for r in claimed {
        let Some(_guard) = FlightGuard::acquire(&r.id) else { continue };
        execute(db, driver, &r, Trigger::Schedule).await;
        fired += 1;
    }
    fired
}

/// One loop tick, each due loop in turn. Production runs them side by side
/// ([`tick_loops`]); a loop already running is skipped, never overlapped.
pub async fn run_due_loops<D: RoutineDriver>(db: &DbHandle, driver: &D, now: DateTime<Utc>) -> usize {
    let mut fired = 0;
    for r in prepare_loops(db, now) {
        let Some(_guard) = FlightGuard::acquire(&r.id) else { continue };
        execute(db, driver, &r, Trigger::Schedule).await;
        fired += 1;
    }
    fired
}

/// One loop tick: each due loop in its own task, so a long one doesn't hold up the rest.
fn tick_loops(state: &Arc<AppState>, now: DateTime<Utc>) {
    for r in prepare_loops(&state.db, now) {
        let Some(guard) = FlightGuard::acquire(&r.id) else { continue };
        let state = state.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let driver = GizziDriver { state: state.clone() };
            execute(&state.db, &driver, &r, Trigger::Schedule).await;
        });
    }
}

pub async fn run_startup<D: RoutineDriver>(db: &DbHandle, driver: &D) -> usize {
    let routines: Vec<LocalRoutine> = (|| -> rusqlite::Result<Vec<LocalRoutine>> {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM routines WHERE execution_domain = 'local' AND status = 'active'
               AND {HAS_AGENT} AND json_extract(config, '$.trigger') = 'startup'"
        ))?;
        let rows = stmt.query_map([], map_row)?;
        Ok(rows.filter_map(Result::ok).collect())
    })()
    .unwrap_or_default();
    let mut fired = 0;
    for r in routines {
        let Some(_guard) = FlightGuard::acquire(&r.id) else { continue };
        execute(db, driver, &r, Trigger::Startup).await;
        fired += 1;
    }
    fired
}

/// Boot pass for startup routines, then a 30s tick. Called from `main`.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let driver = GizziDriver { state: state.clone() };
        match settle_orphaned_runs(&state.db) {
            Ok(n) if n > 0 => info!(settled = n, "routine runs left running by the last runtime marked failed"),
            Err(e) => warn!(error = %e, "could not settle orphaned routine runs"),
            _ => {}
        }
        // Loops tick on their own: a routine's run mustn't delay them. Started
        // after the settle above, which can't tell a new run from an orphan.
        let loops_state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                tick_loops(&loops_state, Utc::now());
            }
        });
        let fired = run_startup(&state.db, &driver).await;
        if fired > 0 {
            info!(fired, "local startup routines ran");
        }
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let fired = run_due(&state.db, &driver, Utc::now()).await;
            if fired > 0 {
                info!(fired, "local routines ran");
            }
        }
    });
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        sessions: Mutex<Vec<String>>,
        turns: Mutex<Vec<(String, String)>>,
        monitor: Mutex<String>,
        fail: bool,
        /// Milliseconds `send_turn` takes (the timeout test).
        delay_ms: u64,
        creates: AtomicUsize,
        /// Sessions a Stop aborted.
        aborts: Mutex<Vec<String>>,
        /// Models a turn was sent on.
        models: Mutex<Vec<String>>,
    }

    impl RoutineDriver for Fake {
        async fn session_exists(&self, id: &str) -> bool {
            self.sessions.lock().unwrap().iter().any(|s| s == id)
        }
        async fn create_session(&self, _b: &str, _n: &str) -> Result<String, String> {
            let id = format!("sess-{}", self.creates.fetch_add(1, Ordering::SeqCst));
            self.sessions.lock().unwrap().push(id.clone());
            Ok(id)
        }
        async fn send_turn(&self, s: &str, _b: &str, t: &str) -> Result<String, String> {
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            if self.fail {
                return Err("provider_rate_limit".into());
            }
            self.turns.lock().unwrap().push((s.to_string(), t.to_string()));
            Ok(format!("done: {}", t.lines().next().unwrap_or("")))
        }
        async fn send_turn_on_model(&self, s: &str, b: &str, t: &str, model: &str) -> Result<String, String> {
            self.models.lock().unwrap().push(model.to_string());
            self.send_turn(s, b, t).await
        }
        async fn run_monitor(&self, _u: &str, _c: &str) -> Result<String, String> {
            Ok(self.monitor.lock().unwrap().clone())
        }
        async fn routine_session(&self, _r: &LocalRoutine, _a: &str, _i: &str) -> Option<Result<String, String>> {
            None
        }
        async fn turn_digest(&self, _s: &str) -> Option<TurnDigest> {
            Some(TurnDigest {
                text: "Read 3 issues so far".into(),
                tools: vec!["linear_list_issues".into(), "slack_post".into()],
                steps: vec![json!({ "content": "Read issues", "status": "completed" })],
                model: Some("anthropic/claude-sonnet-5-5".into()),
            })
        }
        async fn abort_session(&self, s: &str) -> Result<(), String> {
            self.aborts.lock().unwrap().push(s.to_string());
            Ok(())
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-local-routines-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config)
                 VALUES ('bot-1', 'user-a', 'ledger', 'm', 'p', 1, '{\"botProfile\":{\"displayName\":\"Ledger\"}}')",
                [],
            )
            .unwrap();
        state
    }

    fn insert(state: &AppState, id: &str, kind: &str, expr: &str, config: Value, agent: Option<&str>) {
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO routines (id, user_id, agent_id, name, description, status, schedule_type,
                     schedule_expression, execution_domain, config)
                 VALUES (?1, 'user-a', ?2, ?3, 'Summarize the close', 'active', ?4, ?5, 'local', ?6)",
                params![id, agent, format!("Routine {id}"), kind, expr, config.to_string()],
            )
            .unwrap();
    }

    fn force_due(state: &AppState, id: &str) {
        state
            .db
            .connect()
            .unwrap()
            .execute("UPDATE routines SET next_run_at = ?2 WHERE id = ?1", params![id, (Utc::now() - chrono::Duration::minutes(2)).to_rfc3339()])
            .unwrap();
    }

    fn runs(state: &AppState, id: &str) -> Vec<String> {
        let conn = state.db.connect().unwrap();
        let mut stmt = conn.prepare("SELECT status FROM routine_runs WHERE routine_id = ?1 ORDER BY started_at").unwrap();
        stmt.query_map(params![id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    fn events(state: &AppState) -> Vec<String> {
        let conn = state.db.connect().unwrap();
        let mut stmt = conn.prepare("SELECT event_type FROM bot_events WHERE bot_id = 'bot-1' ORDER BY seq").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    fn insert_loop(state: &AppState, id: &str, expr: &str, config: Value, agent: Option<&str>, expires: Option<String>) {
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO loops (id, user_id, agent_id, name, description, status, schedule_type,
                     schedule_expression, execution_domain, config, expires_at)
                 VALUES (?1, 'user-a', ?2, ?3, 'Watch the queue', 'active', 'interval', ?4, 'local', ?5, ?6)",
                params![id, agent, format!("Loop {id}"), expr, config.to_string(), expires],
            )
            .unwrap();
    }

    fn force_loop_due(state: &AppState, id: &str) {
        state
            .db
            .connect()
            .unwrap()
            .execute("UPDATE loops SET next_run_at = ?2 WHERE id = ?1", params![id, (Utc::now() - chrono::Duration::minutes(2)).to_rfc3339()])
            .unwrap();
    }

    fn loop_runs(state: &AppState, id: &str) -> Vec<String> {
        let conn = state.db.connect().unwrap();
        let mut stmt = conn.prepare("SELECT status FROM loop_runs WHERE routine_id = ?1 ORDER BY started_at").unwrap();
        stmt.query_map(params![id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    fn loop_status(state: &AppState, id: &str) -> String {
        state.db.connect().unwrap().query_row("SELECT status FROM loops WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
    }

    #[tokio::test]
    async fn a_new_loop_waits_for_its_first_due_time_and_then_fires_once() {
        let state = setup("loop-due").await;
        let fake = Fake::default();
        insert_loop(&state, "l-due", "5m", json!({ "prompt": "Check it" }), Some("bot-1"), None);

        assert_eq!(run_due_loops(&state.db, &fake, Utc::now()).await, 0, "the first tick only sets the cursor");
        force_loop_due(&state, "l-due");
        let now = Utc::now();
        assert_eq!(run_due_loops(&state.db, &fake, now).await, 1);
        // The cursor moved before the run, so a second tick or a restart doesn't repeat it.
        assert_eq!(run_due_loops(&state.db, &fake, now).await, 0);
        assert_eq!(loop_runs(&state, "l-due"), vec!["succeeded"]);
        let next: Option<String> = state.db.connect().unwrap().query_row("SELECT next_run_at FROM loops WHERE id = 'l-due'", [], |r| r.get(0)).unwrap();
        assert!(DateTime::parse_from_rfc3339(&next.unwrap()).unwrap().with_timezone(&Utc) > now);
    }

    #[tokio::test]
    async fn a_paused_loop_does_not_fire() {
        let state = setup("loop-paused").await;
        let fake = Fake::default();
        insert_loop(&state, "l-paused", "5m", json!({ "prompt": "Check it" }), Some("bot-1"), None);
        force_loop_due(&state, "l-paused");
        state.db.connect().unwrap().execute("UPDATE loops SET status = 'paused' WHERE id = 'l-paused'", []).unwrap();
        assert_eq!(run_due_loops(&state.db, &fake, Utc::now()).await, 0);
        assert!(loop_runs(&state, "l-paused").is_empty());
    }

    #[tokio::test]
    async fn a_loop_past_expires_at_is_disabled_and_does_not_fire() {
        let state = setup("loop-expired").await;
        let fake = Fake::default();
        let past = (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
        insert_loop(&state, "l-expired", "5m", json!({ "prompt": "Check it" }), Some("bot-1"), Some(past));
        force_loop_due(&state, "l-expired");
        assert_eq!(run_due_loops(&state.db, &fake, Utc::now()).await, 0);
        assert_eq!(loop_status(&state, "l-expired"), "disabled");
    }

    #[tokio::test]
    async fn a_running_loop_is_not_overlapped() {
        let state = setup("loop-overlap").await;
        let fake = Fake::default();
        insert_loop(&state, "l-overlap", "5m", json!({ "prompt": "Check it" }), Some("bot-1"), None);
        force_loop_due(&state, "l-overlap");
        let _running = FlightGuard::acquire("l-overlap").unwrap();
        assert_eq!(run_due_loops(&state.db, &fake, Utc::now()).await, 0);
        assert!(loop_runs(&state, "l-overlap").is_empty());
    }

    #[tokio::test]
    async fn a_loop_with_no_agent_runs_as_the_main_bot_on_its_own_model_in_its_own_session() {
        let state = setup("loop-model").await;
        let fake = Fake::default();
        insert_loop(&state, "l-model", "5m", json!({ "prompt": "Check it", "model": "openrouter/x/y" }), None, None);
        for _ in 0..2 {
            force_loop_due(&state, "l-model");
            assert_eq!(run_due_loops(&state.db, &fake, Utc::now()).await, 1);
        }
        assert_eq!(*fake.models.lock().unwrap(), vec!["openrouter/x/y", "openrouter/x/y"]);
        let turns = fake.turns.lock().unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].0, turns[1].0, "both runs use the loop's one session");
        assert_eq!(fake.creates.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn schedule_math() {
        let t = DateTime::parse_from_rfc3339("2026-09-27T10:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(next_run("interval", "30m", t, false), Some(t + chrono::Duration::minutes(30)));
        assert_eq!(next_run("interval", "bogus", t, false), None);
        assert_eq!(next_run("manual", "", t, true), None);
        assert_eq!(next_run("once", "", t, true), Some(t));
        assert_eq!(next_run("once", "", t, false), None);
        let later = next_run("cron", "0 9 * * *", t, false).unwrap();
        assert!(later > t && later - t <= chrono::Duration::days(1));
        assert_eq!(fnv1a_hex("a"), "e40c292c");
        assert_eq!(routine_turn("[bot:Ledger] Month close", "x"), "[routine: Month close] x");
        assert_eq!(routine_turn("Weekly report", "x"), "[routine: Weekly report] x");
    }

    #[tokio::test]
    async fn a_routine_tied_to_a_closed_thread_is_never_claimed() {
        let state = setup("closed-thread").await;
        let ts = Utc::now().to_rfc3339();
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO bot_threads (id, user_id, bot_id, kind, title, status, last_activity_at, created_at, updated_at, archived_at)
                 VALUES ('th-1', 'user-a', 'bot-1', 'task', 'Venue', 'working', ?1, ?1, ?1, ?1)",
                params![ts],
            )
            .unwrap();
        insert(&state, "r1", "interval", "1h", json!({}), Some("bot-1"));
        state
            .db
            .connect()
            .unwrap()
            .execute("UPDATE routines SET metadata = '{\"sourceThreadId\":\"th-1\"}' WHERE id = 'r1'", [])
            .unwrap();
        let d = Fake::default();
        run_due(&state.db, &d, Utc::now()).await;
        force_due(&state, "r1");
        assert_eq!(run_due(&state.db, &d, Utc::now()).await, 0, "archived thread: the reminder doesn't fire");
        assert!(runs(&state, "r1").is_empty());
    }

    #[tokio::test]
    async fn due_routine_runs_once_into_the_bots_thread_and_pins_it() {
        let state = setup("tick").await;
        insert(&state, "r1", "interval", "1h", json!({}), Some("bot-1"));
        let now = Utc::now();
        let d = Fake::default();

        // First tick only schedules it (an hour out).
        assert_eq!(run_due(&state.db, &d, now).await, 0);
        force_due(&state, "r1");
        assert_eq!(run_due(&state.db, &d, now).await, 1);
        assert_eq!(run_due(&state.db, &d, now).await, 0, "claim must advance the cursor");

        let turns = d.turns.lock().unwrap().clone();
        assert_eq!(turns[0].0, "sess-0");
        assert_eq!(turns[0].1, "[routine: Routine r1] Summarize the close");
        assert_eq!(bot_info(&state.db, "bot-1").unwrap().pinned_thread.as_deref(), Some("sess-0"));
        assert_eq!(runs(&state, "r1"), vec!["succeeded"]);
        assert_eq!(events(&state), vec!["routine.completed"]);

        force_due(&state, "r1");
        run_due(&state.db, &d, now).await;
        let turns = d.turns.lock().unwrap().clone();
        assert_eq!(turns[1].0, "sess-0", "reuses the pin");
        assert!(turns[1].1.contains("Previous run output:\ndone: [routine: Routine r1]"));
        assert_eq!(d.creates.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn once_runs_then_stops_and_failures_are_recorded() {
        let state = setup("once").await;
        insert(&state, "r2", "once", "", json!({}), Some("bot-1"));
        let d = Fake { fail: true, ..Default::default() };
        let now = Utc::now();
        assert_eq!(run_due(&state.db, &d, now).await, 1);
        assert_eq!(run_due(&state.db, &d, now + chrono::Duration::hours(2)).await, 0);
        assert_eq!(runs(&state, "r2"), vec!["failed"]);
        let status: String = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT json_extract(metadata, '$.lastRunStatus') FROM routines WHERE id = 'r2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(events(&state), vec!["routine.failed"]);
    }

    #[tokio::test]
    async fn monitor_is_silent_when_output_unchanged_and_agentless_routines_fail_clearly() {
        let state = setup("monitor").await;
        insert(&state, "r3", "interval", "1h", json!({"monitor": {"command": "df -h"}}), Some("bot-1"));
        insert(&state, "r4", "interval", "1h", json!({}), None);
        let d = Fake::default();
        *d.monitor.lock().unwrap() = "91% used".into();
        let now = Utc::now();
        for _ in 0..2 {
            force_due(&state, "r3");
            force_due(&state, "r4");
            run_due(&state.db, &d, now).await;
        }
        assert_eq!(runs(&state, "r3"), vec!["succeeded", "skipped"]);
        assert_eq!(d.turns.lock().unwrap().len(), 1);
        assert!(d.turns.lock().unwrap()[0].1.ends_with("91% used"));
        // An agentless routine is never run (no failed run every tick); it is
        // flagged for the "Needs a bot" list and its cursor cleared.
        assert!(runs(&state, "r4").is_empty());
        assert_eq!(meta(&state, "r4", "$.needsAgent"), Some(1));
        let next: Option<String> = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT next_run_at FROM routines WHERE id = 'r4'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(next, None);
    }

    fn meta(state: &AppState, id: &str, path: &str) -> Option<i64> {
        state
            .db
            .connect()
            .unwrap()
            .query_row(&format!("SELECT json_extract(metadata, '{path}') FROM routines WHERE id = ?1"), params![id], |r| {
                r.get(0)
            })
            .unwrap()
    }

    fn status(state: &AppState, id: &str) -> String {
        state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT status FROM routines WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap()
    }

    #[tokio::test]
    async fn three_failures_in_a_row_pause_the_routine_and_say_so() {
        let state = setup("autopause").await;
        insert(&state, "r6", "interval", "1h", json!({}), Some("bot-1"));
        let failing = Fake { fail: true, ..Default::default() };
        let now = Utc::now();
        for i in 1..=2 {
            force_due(&state, "r6");
            run_due(&state.db, &failing, now).await;
            assert_eq!(meta(&state, "r6", "$.consecutiveFailures"), Some(i));
            assert_eq!(status(&state, "r6"), "active");
        }
        // A good run resets the streak.
        force_due(&state, "r6");
        run_due(&state.db, &Fake::default(), now).await;
        assert_eq!(meta(&state, "r6", "$.consecutiveFailures"), None);

        for _ in 0..3 {
            force_due(&state, "r6");
            run_due(&state.db, &failing, now).await;
        }
        assert_eq!(status(&state, "r6"), "paused");
        let conn = state.db.connect().unwrap();
        let (reason, last_error): (String, String) = conn
            .query_row(
                "SELECT json_extract(metadata, '$.autoPaused.reason'), json_extract(metadata, '$.autoPaused.lastError')
                 FROM routines WHERE id = 'r6'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(reason.contains("3 failed runs"));
        assert_eq!(last_error, "provider_rate_limit");
        let ev = events(&state);
        assert_eq!(ev.last().map(String::as_str), Some("routine.paused"));
        assert_eq!(ev.iter().filter(|e| *e == "routine.failed").count(), 5);

        // Paused: the tick no longer runs it.
        force_due(&state, "r6");
        assert_eq!(run_due(&state.db, &failing, now).await, 0);
    }

    #[tokio::test]
    async fn startup_routines_run_at_boot() {
        let state = setup("startup").await;
        insert(&state, "r5", "manual", "", json!({"trigger": "startup"}), Some("bot-1"));
        let d = Fake::default();
        assert_eq!(run_startup(&state.db, &d).await, 1);
        assert_eq!(runs(&state, "r5"), vec!["succeeded"]);
    }

    fn attention(state: &AppState, id: &str) -> Option<(String, String, Option<String>)> {
        state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT severity, kind, resolved_at FROM attention_items WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .unwrap()
    }

    #[test]
    fn errors_classify_into_outcome_reasons() {
        for (e, want) in [
            ("timed out after 30 min (this routine's limit)", "timeout"),
            ("this routine has no bot; assign it to a bot to run it", "no_bot"),
            ("the routine's agent no longer exists", "no_bot"),
            ("LANE_BLOCKED: challenge presented", "lane_blocked"),
            ("AUTH_REQUIRED", "lane_blocked"),
            ("provider_rate_limit", "quota"),
            ("The operation was aborted.", "runtime_restart"),
            ("500 internal error from the model", "model_error"),
        ] {
            assert_eq!(classify(e), want, "{e}");
        }
    }

    #[tokio::test]
    async fn a_run_past_its_timeout_fails_with_the_reason_and_alerts() {
        let state = setup("timeout").await;
        insert(&state, "rt", "interval", "1h", json!({}), Some("bot-1"));
        state.db.connect().unwrap().execute("UPDATE routines SET timeout_seconds = 1 WHERE id = 'rt'", []).unwrap();
        let r = get_routine(&state.db, "rt").unwrap().unwrap();
        let d = Fake { delay_ms: 5000, ..Default::default() };
        let started = std::time::Instant::now();
        let out = execute(&state.db, &d, &r, Trigger::Schedule).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        assert_eq!((out.status, out.reason), ("failed", Some("timeout")));
        let reason: String = state.db.connect().unwrap().query_row("SELECT outcome_reason FROM routine_runs WHERE routine_id = 'rt'", [], |r| r.get(0)).unwrap();
        assert_eq!(reason, "timeout");
        assert_eq!(attention(&state, "routine_failed:rt").map(|a| a.0), Some("warn".into()));
    }

    fn run_row(state: &AppState, id: &str) -> (String, Option<String>, Option<String>, Value) {
        state.db.connect().unwrap().query_row(
            "SELECT status, output, outcome_reason, metadata FROM routine_runs WHERE routine_id = ?1 ORDER BY started_at DESC LIMIT 1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null))),
        ).unwrap()
    }

    #[tokio::test]
    async fn a_run_is_running_while_it_goes_and_stop_keeps_its_partial_output() {
        let state = setup("stop").await;
        insert(&state, "rs", "interval", "1h", json!({}), Some("bot-1"));
        let r = get_routine(&state.db, "rs").unwrap().unwrap();
        let d = Arc::new(Fake { delay_ms: 5000, ..Default::default() });
        let task = {
            let (db, d, r) = (state.db.clone(), d.clone(), r.clone());
            tokio::spawn(async move { execute(&db, &*d, &r, Trigger::Schedule).await })
        };
        let (run_id, session) = loop {
            if let Some((run, Some(session))) = live_run("rs") {
                break (run, session);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(run_row(&state, "rs").0, "running");
        assert_eq!(run_row(&state, "rs").3["sessionId"], json!(session));
        assert!(!request_stop("rs", "some-other-run"));
        assert!(request_stop("rs", &run_id));
        let out = task.await.unwrap();
        assert_eq!((out.status, out.reason), ("cancelled", Some("stopped")));
        assert_eq!(*d.aborts.lock().unwrap(), vec![session]);
        let (status, output, reason, meta) = run_row(&state, "rs");
        assert_eq!((status.as_str(), output.as_deref(), reason.as_deref()), ("cancelled", Some("Read 3 issues so far"), Some("stopped")));
        assert_eq!(meta["apps"], json!(["linear_list_issues", "slack_post"]));
        assert_eq!(meta["model"], json!("anthropic/claude-sonnet-5-5"));
        assert!(live_run("rs").is_none());
        // A stop is not a failure: no streak, no alert.
        assert_eq!(meta_i(&state, "rs"), None);
        assert!(attention(&state, "routine_failed:rs").is_none());
        assert!(events(&state).contains(&"routine.cancelled".to_string()));
    }

    fn meta_i(state: &AppState, id: &str) -> Option<i64> {
        meta(state, id, "$.consecutiveFailures")
    }

    #[tokio::test]
    async fn a_finished_run_records_its_brain_apps_and_steps() {
        let state = setup("digest").await;
        insert(&state, "rg", "interval", "1h", json!({}), Some("bot-1"));
        let r = get_routine(&state.db, "rg").unwrap().unwrap();
        let out = execute(&state.db, &Fake::default(), &r, Trigger::Schedule).await;
        assert_eq!(out.status, "succeeded");
        let (status, _, _, meta) = run_row(&state, "rg");
        assert_eq!(status, "succeeded");
        assert_eq!(meta["steps"][0]["content"], json!("Read issues"));
        assert_eq!(meta["apps"][1], json!("slack_post"));
        let n: i64 = state.db.connect().unwrap().query_row("SELECT COUNT(*) FROM routine_runs WHERE routine_id = 'rg'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "the running row is completed in place, not duplicated");
    }

    #[tokio::test]
    async fn runs_left_running_by_a_dead_runtime_fail_at_boot() {
        let state = setup("orphan").await;
        insert(&state, "ro", "interval", "1h", json!({}), Some("bot-1"));
        let c = state.db.connect().unwrap();
        c.execute("INSERT INTO routine_runs (id, routine_id, status, scheduled_at, started_at) VALUES ('x', 'ro', 'running', '2026-10-24T09:00:00Z', '2026-10-24T09:00:00Z')", []).unwrap();
        c.execute("INSERT INTO routine_runs (id, routine_id, gizzi_run_id, status, scheduled_at) VALUES ('y', 'ro', 'g1', 'running', '2026-10-24T09:00:00Z')", []).unwrap();
        assert_eq!(settle_orphaned_runs(&state.db).unwrap(), 1);
        let st: String = c.query_row("SELECT status FROM routine_runs WHERE id = 'x'", [], |r| r.get(0)).unwrap();
        let cloud: String = c.query_row("SELECT status FROM routine_runs WHERE id = 'y'", [], |r| r.get(0)).unwrap();
        assert_eq!((st.as_str(), cloud.as_str()), ("failed", "running"));
    }

    #[tokio::test]
    async fn a_failure_opens_routine_failed_with_retry_and_the_next_success_resolves_it() {
        let state = setup("alert").await;
        insert(&state, "ra", "interval", "1h", json!({}), Some("bot-1"));
        let r = get_routine(&state.db, "ra").unwrap().unwrap();
        let bad = Fake { fail: true, ..Default::default() };
        execute(&state.db, &bad, &r, Trigger::Schedule).await;
        let (sev, kind, resolved) = attention(&state, "routine_failed:ra").expect("opened");
        assert_eq!((sev.as_str(), kind.as_str(), resolved), ("warn", "routine_failed", None));
        let c = state.db.connect().unwrap();
        let (retry, reason, alerted): (String, String, Option<String>) = c
            .query_row("SELECT (SELECT retry_json FROM attention_items WHERE id = 'routine_failed:ra'), outcome_reason, alerted_at FROM routine_runs WHERE routine_id = 'ra'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((retry.as_str(), reason.as_str()), (r#"{"routineId":"ra"}"#, "quota"));
        assert!(alerted.is_some());
        let opened: i64 = c.query_row("SELECT COUNT(*) FROM runtime_user_events WHERE event_type = 'attention.opened'", [], |r| r.get(0)).unwrap();
        assert_eq!(opened, 1);
        // Three failures in a row pause it and raise the item to critical.
        execute(&state.db, &bad, &r, Trigger::Schedule).await;
        execute(&state.db, &bad, &r, Trigger::Schedule).await;
        assert_eq!(attention(&state, "routine_failed:ra").map(|a| a.0), Some("critical".into()));
        execute(&state.db, &Fake::default(), &r, Trigger::Manual { user_id: "user-a".into() }).await;
        assert!(attention(&state, "routine_failed:ra").unwrap().2.is_some(), "a good run resolves it");
    }

    #[tokio::test]
    async fn a_gap_writes_missed_rows_and_one_alert() {
        let state = setup("missed").await;
        insert(&state, "rm", "interval", "1d", json!({}), Some("bot-1"));
        let now = Utc::now();
        // Due 51 h ago: occurrences at -51 h, -27 h and -3 h are past; the last one is the run that follows.
        state.db.connect().unwrap().execute("UPDATE routines SET next_run_at = ?2 WHERE id = ?1", params!["rm", (now - chrono::Duration::hours(51)).to_rfc3339()]).unwrap();
        let d = Fake::default();
        assert_eq!(run_due(&state.db, &d, now).await, 1);
        assert_eq!(runs(&state, "rm"), vec!["skipped", "skipped", "succeeded"]);
        let c = state.db.connect().unwrap();
        let reasons: i64 = c.query_row("SELECT COUNT(*) FROM routine_runs WHERE outcome_reason = 'missed_runtime_offline'", [], |r| r.get(0)).unwrap();
        let late: Option<i64> = c.query_row("SELECT late_by_ms FROM routine_runs WHERE status = 'succeeded'", [], |r| r.get(0)).unwrap();
        assert_eq!(reasons, 2);
        assert!(late.unwrap() >= 50 * 3600 * 1000);
        let (sev, kind, resolved) = attention(&state, "routine_missed:rm").expect("opened");
        assert_eq!((sev.as_str(), kind.as_str(), resolved), ("warn", "routine_missed", None));
    }

    #[tokio::test]
    async fn an_agentless_routine_opens_needs_bot_and_assigning_a_bot_closes_it() {
        let state = setup("needsbot").await;
        insert(&state, "rn", "interval", "1h", json!({}), None);
        let d = Fake::default();
        assert_eq!(run_due(&state.db, &d, Utc::now()).await, 0);
        assert_eq!(attention(&state, "routine_needs_bot:rn").map(|a| a.2), Some(None));
        // Another tick does not add a second open (or a second push).
        run_due(&state.db, &d, Utc::now()).await;
        let c = state.db.connect().unwrap();
        let opened: i64 = c.query_row("SELECT COUNT(*) FROM runtime_user_events WHERE event_type = 'attention.opened'", [], |r| r.get(0)).unwrap();
        assert_eq!(opened, 1);
        c.execute("UPDATE routines SET agent_id = 'bot-1' WHERE id = 'rn'", []).unwrap();
        run_due(&state.db, &d, Utc::now()).await;
        assert!(attention(&state, "routine_needs_bot:rn").unwrap().2.is_some());
    }

    #[tokio::test]
    async fn a_run_record_that_cannot_be_saved_becomes_a_dead_letter() {
        let state = setup("dead").await;
        insert(&state, "rd", "interval", "1h", json!({}), Some("bot-1"));
        let r = get_routine(&state.db, "rd").unwrap().unwrap();
        state.db.connect().unwrap().execute_batch("ALTER TABLE routine_runs RENAME TO routine_runs_gone").unwrap();
        let out = execute(&state.db, &Fake::default(), &r, Trigger::Schedule).await;
        assert_eq!(out.status, "succeeded");
        let c = state.db.connect().unwrap();
        let (kind, detail): (String, String) = c.query_row("SELECT kind, detail_json FROM attention_items WHERE kind = 'dead_letter'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(kind, "dead_letter");
        assert!(detail.contains("\"routineId\":\"rd\"") && detail.contains("storeError"), "{detail}");
    }
}
