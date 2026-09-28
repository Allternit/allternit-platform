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
//! * **Record.** Every run is a `routine_runs` row (the Automation Tasks run
//!   history) and, for bots, a `routine.*` event on the bot ledger.

use chrono::{DateTime, Offset, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{info, warn};

use crate::bot_event_routes::{append_event, ActorBody, AppendEventBody};
use crate::db::DbHandle;
use crate::AppState;

const PREVIOUS_OUTPUT_CAP: usize = 2 * 1024;
const MONITOR_OUTPUT_CAP: usize = 4 * 1024;
const RUN_OUTPUT_CAP: usize = 8 * 1024;

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

#[derive(Debug, Clone)]
pub struct LocalRoutine {
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
}

const COLS: &str = "id, user_id, agent_id, name, description, schedule_type, schedule_expression, \
                    config, metadata, next_run_at";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalRoutine> {
    let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    Ok(LocalRoutine {
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
    })
}

pub fn get_routine(db: &DbHandle, id: &str) -> rusqlite::Result<Option<LocalRoutine>> {
    db.connect()?
        .query_row(&format!("SELECT {COLS} FROM routines WHERE id = ?1"), params![id], map_row)
        .optional()
}

/// Give every active local routine without a cursor its first run time.
fn backfill(db: &DbHandle, now: DateTime<Utc>) -> rusqlite::Result<()> {
    let conn = db.connect()?;
    let pending: Vec<(String, String, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, schedule_type, schedule_expression, last_run_at FROM routines
             WHERE execution_domain = 'local' AND status = 'active' AND next_run_at IS NULL
               AND schedule_type IN ('interval', 'cron', 'once')",
        )?;
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

/// Claim due routines by advancing their cursor first (compare-and-set).
fn claim_due(db: &DbHandle, now: DateTime<Utc>) -> rusqlite::Result<Vec<LocalRoutine>> {
    let conn = db.connect()?;
    let due: Vec<LocalRoutine> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM routines
             WHERE execution_domain = 'local' AND status = 'active'
               AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY next_run_at"
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
            claimed.push(r);
        }
    }
    Ok(claimed)
}

// ─── Delivery ───────────────────────────────────────────────────────────────

/// How a routine reaches the bot. Production drives gizzi and the tool path;
/// tests substitute a recorder.
pub trait RoutineDriver: Send + Sync {
    fn session_exists(&self, session_id: &str) -> impl Future<Output = bool> + Send;
    fn create_session(&self, bot_id: &str, bot_name: &str) -> impl Future<Output = Result<String, String>> + Send;
    fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> impl Future<Output = Result<String, String>> + Send;
    fn run_monitor(&self, user_id: &str, command: &str) -> impl Future<Output = Result<String, String>> + Send;
    /// The session a run goes to: the routine's own standing thread, a fresh
    /// generation per run (P4.4). `None` → the bot's main thread.
    fn routine_session(
        &self,
        r: &LocalRoutine,
        agent_id: &str,
        instruction: &str,
    ) -> impl Future<Output = Option<Result<String, String>>> + Send;
}

pub struct GizziDriver {
    pub state: Arc<AppState>,
}

impl RoutineDriver for GizziDriver {
    async fn session_exists(&self, session_id: &str) -> bool {
        crate::agent_session_routes::bot_session_exists(session_id).await
    }

    async fn create_session(&self, bot_id: &str, bot_name: &str) -> Result<String, String> {
        crate::agent_session_routes::create_bot_session(&self.state.db, bot_id, bot_name).await
    }

    async fn send_turn(&self, session_id: &str, bot_id: &str, text: &str) -> Result<String, String> {
        crate::agent_session_routes::send_bot_turn(&self.state.db, session_id, bot_id, text).await
    }

    async fn routine_session(&self, r: &LocalRoutine, agent_id: &str, instruction: &str) -> Option<Result<String, String>> {
        let rt = crate::thread_routes::GizziRuntime { db: self.state.db.clone() };
        Some(
            crate::thread_routes::routine_generation(&self.state.db, &rt, &r.user_id, agent_id, &r.id, &r.name, instruction)
                .await,
        )
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

fn previous_output(db: &DbHandle, routine_id: &str) -> Option<String> {
    db.connect().ok()?.query_row(
        "SELECT output FROM routine_runs WHERE routine_id = ?1 AND status = 'succeeded' AND output IS NOT NULL
         ORDER BY started_at DESC LIMIT 1",
        params![routine_id],
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
}

/// Run one routine end to end and record it. Never panics on a bad run —
/// the failure is the recorded result.
pub async fn execute<D: RoutineDriver>(db: &DbHandle, driver: &D, r: &LocalRoutine, trigger: Trigger) -> RunOutcome {
    let started = Utc::now();
    let mut monitor_hash: Option<String> = None;
    let mut session_used: Option<String> = None;
    let bot = r.agent_id.as_deref().and_then(|id| bot_info(db, id));

    let result: Result<(String, &'static str), String> = async {
        let agent_id = r.agent_id.as_deref().ok_or("local routines need an agent to run them")?;
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
            if let Some(prev) = previous_output(db, &r.id) {
                instruction = format!("{instruction}\n\nPrevious run output:\n{}", truncate(&prev, PREVIOUS_OUTPUT_CAP));
            }
            routine_turn(&r.name, &instruction)
        };
        let session = match driver.routine_session(r, agent_id, &instruction_of(r)).await {
            Some(result) => result?,
            None => resolve_thread(db, driver, agent_id, bot).await?,
        };
        session_used = Some(session.clone());
        let reply = driver.send_turn(&session, agent_id, &text).await?;
        Ok((reply, "succeeded"))
    }
    .await;

    let outcome = match result {
        Ok((out, status)) => RunOutcome { status, output: Some(truncate(&out, RUN_OUTPUT_CAP)), error: None, session_id: session_used },
        Err(e) => RunOutcome { status: "failed", output: None, error: Some(truncate(&e, RUN_OUTPUT_CAP)), session_id: session_used },
    };
    let finished = Utc::now();

    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "INSERT INTO routine_runs (id, routine_id, status, scheduled_at, started_at, finished_at, duration_ms,
                 output, error, triggered_by, metadata)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                uuid::Uuid::new_v4().to_string(),
                r.id,
                outcome.status,
                started.to_rfc3339(),
                finished.to_rfc3339(),
                (finished - started).num_milliseconds(),
                outcome.output,
                outcome.error,
                trigger.as_str(),
                json!({ "sessionId": outcome.session_id }).to_string(),
            ],
        );
        let _ = conn.execute(
            "UPDATE routines SET last_run_at = ?2,
                 metadata = json_set(
                     CASE WHEN ?3 IS NULL THEN COALESCE(metadata, '{}')
                          ELSE json_set(COALESCE(metadata, '{}'), '$.lastMonitorHash', ?3) END,
                     '$.lastRunStatus', ?4)
             WHERE id = ?1",
            params![r.id, started.to_rfc3339(), monitor_hash, outcome.status],
        );
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
        }
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

pub async fn run_startup<D: RoutineDriver>(db: &DbHandle, driver: &D) -> usize {
    let routines: Vec<LocalRoutine> = (|| -> rusqlite::Result<Vec<LocalRoutine>> {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM routines WHERE execution_domain = 'local' AND status = 'active'
               AND json_extract(config, '$.trigger') = 'startup'"
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
        creates: AtomicUsize,
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
            if self.fail {
                return Err("provider_rate_limit".into());
            }
            self.turns.lock().unwrap().push((s.to_string(), t.to_string()));
            Ok(format!("done: {}", t.lines().next().unwrap_or("")))
        }
        async fn run_monitor(&self, _u: &str, _c: &str) -> Result<String, String> {
            Ok(self.monitor.lock().unwrap().clone())
        }
        async fn routine_session(&self, _r: &LocalRoutine, _a: &str, _i: &str) -> Option<Result<String, String>> {
            None
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
            .execute("UPDATE routines SET next_run_at = '2000-01-01T00:00:00+00:00' WHERE id = ?1", params![id])
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
        let conn = state.db.connect().unwrap();
        let err: String = conn
            .query_row("SELECT error FROM routine_runs WHERE routine_id = 'r4' LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert!(err.contains("need an agent"));
    }

    #[tokio::test]
    async fn startup_routines_run_at_boot() {
        let state = setup("startup").await;
        insert(&state, "r5", "manual", "", json!({"trigger": "startup"}), Some("bot-1"));
        let d = Fake::default();
        assert_eq!(run_startup(&state.db, &d).await, 1);
        assert_eq!(runs(&state, "r5"), vec!["succeeded"]);
    }
}
