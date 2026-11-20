//! Sleep cycles (step 5.1): the worker leases one `daydream`/`nap` unit at a
//! time from the queue (`queue.rs`), opens a `sleep_cycles` row and runs the
//! stages in one fixed order:
//!
//! | # | stage | runs when | spends |
//! |---|---|---|---|
//! | 1 | `capture` | always (marks the user's queued capture units done) | no |
//! | 2 | `consolidation` | always (`memory_consolidation::run_for_user`) | no (S1 is local) |
//! | 2b | `fit` | `strengths` stage on (`sleep::fit::fit_user`) | no |
//! | 3 | `dream` | Dream is on for the drive and today's Dream is not finished | yes |
//! | 4 | `curation` | a bot's weekly curation is due | yes |
//! | 5 | `pages` | no-op slot: waits for gate G1 and step 4.2 | - |
//! | 6 | `checks` | `checks` stage on (`sleep::checks::run_stage`) | no |
//! | 7 | `sign` | no-op slot: waits for step 4.1 (manifest) | - |
//!
//! A daydream runs stages 1-2 only. `sleep_cycles.stage` follows the stage
//! that is running; `report_json` lists each stage's outcome.
//!
//! - **Clean stop.** Between stages the unit's `stop_requested` mark (set by
//!   `resume`) is re-read. When set, the cycle ends `stopped` after the stage
//!   that was running and the remaining stages become a `held` unit with
//!   `resume_from_stage`; the session's next signal re-queues it.
//! - **Per-cycle stop rule.** No new stage starts after `cycle_max_stages`
//!   stages or `cycle_max_minutes`; the rest is held the same way.
//! - **Daily spend cap.** A spending stage runs only when `daily_spend_cap_usd`
//!   is above 0 and today's spend plus its estimate fits under it; otherwise
//!   it is recorded `skipped: spend_cap`. Non-spending stages always run. The
//!   estimate only gates the run; the spend recorded afterwards is the real
//!   cost from the usage ledger ([`ledger_spend_usd`]).
//! - **Failure.** An error ends the cycle `failed` and counts one attempt on
//!   the unit (`queue::fail`); after `unit_max_attempts` it is deferred.
//! - **Sleep debt.** New memories since the last finished nap over
//!   `debt_scale`. At or above `debt_pause_threshold` (orchestrator on) the
//!   consolidation decay write pauses (see [`decay_paused`]).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use super::config::{self, Resolved};
use super::queue::{self, Unit};
use super::trigger::KIND_DAYDREAM;
use super::types::Stage;
use crate::db::DbHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Capture,
    Consolidation,
    Fit,
    Dream,
    Rem,
    Curation,
    Pages,
    Checks,
    Sign,
}

impl Step {
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Capture => "capture",
            Step::Consolidation => "consolidation",
            Step::Fit => "fit",
            Step::Dream => "dream",
            Step::Rem => "rem",
            Step::Curation => "curation",
            Step::Pages => "pages",
            Step::Checks => "checks",
            Step::Sign => "sign",
        }
    }
}

/// A nap's fixed stage order.
pub const NAP: [Step; 9] = [
    Step::Capture,
    Step::Consolidation,
    Step::Fit,
    Step::Dream,
    Step::Rem,
    Step::Curation,
    Step::Pages,
    Step::Checks,
    Step::Sign,
];
/// A daydream is the light pass: capture and consolidation only.
pub const DAYDREAM: [Step; 2] = [Step::Capture, Step::Consolidation];

pub fn plan_for(kind: &str) -> &'static [Step] {
    if kind == KIND_DAYDREAM { &DAYDREAM } else { &NAP }
}

#[derive(Debug, Clone, Serialize)]
pub struct StepRecord {
    pub step: Step,
    /// `done` or `skipped`.
    pub status: &'static str,
    pub note: Option<String>,
    pub spend_usd: f64,
    pub detail: Value,
    /// When the stage began and ended (`sleep_cycles` time format). The
    /// report reads the usage ledger over this window for the real spend.
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl StepRecord {
    fn done(step: Step, detail: Value) -> Self {
        StepRecord { step, status: "done", note: None, spend_usd: 0.0, detail, started_at: None, finished_at: None }
    }
    fn skipped(step: Step, note: impl Into<String>) -> Self {
        StepRecord { step, status: "skipped", note: Some(note.into()), spend_usd: 0.0, detail: Value::Null, started_at: None, finished_at: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CycleReport {
    pub cycle_id: Option<String>,
    pub unit_id: String,
    pub user_id: String,
    pub kind: String,
    /// `finished`, `stopped`, `failed`, or `cancelled` (no cycle row: the
    /// orchestrator stage was off when the unit came up).
    pub state: &'static str,
    pub steps: Vec<StepRecord>,
    pub spend_usd: f64,
    pub stop_reason: Option<String>,
    pub resume_from_stage: Option<&'static str>,
    pub held_unit: Option<String>,
    pub error: Option<String>,
}

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The jobs the cycle runs that belong to other modules. The live
/// implementation calls them for real; tests replace only these edges (the
/// model-calling jobs), the queue and cycle rows stay real.
#[async_trait]
pub trait Jobs: Send + Sync {
    async fn consolidate(&self, user: &str) -> Result<Value, String>;
    /// The local date to Dream for, when the user's Dream is due today.
    fn dream_due(&self, conn: &Connection, user: &str, now: DateTime<Utc>) -> Result<Option<NaiveDate>, String>;
    async fn dream(&self, user: &str, date: NaiveDate) -> Result<Value, String>;
    /// Bots of this user whose weekly curation is due.
    fn curation_due(&self, conn: &Connection, user: &str, now: DateTime<Utc>) -> Result<Vec<String>, String>;
    async fn curate(&self, user: &str, bots: Vec<String>) -> Result<Value, String>;
}

/// A Dream run as the cycle records it. The Dream id and its two commits are
/// what `report.rs` needs to undo it.
pub fn dream_detail(date: NaiveDate, row: Option<crate::memory_dream::DreamRow>) -> Value {
    match row {
        Some(r) => json!({
            "date": date.to_string(), "status": r.status, "dream_id": r.id,
            "revision": r.revision, "base_revision": r.base_revision,
            "summary": r.summary, "error": r.error,
        }),
        None => json!({ "date": date.to_string(), "status": null }),
    }
}

/// The real jobs.
pub struct LiveJobs {
    pub state: Arc<crate::AppState>,
}

#[async_trait]
impl Jobs for LiveJobs {
    async fn consolidate(&self, user: &str) -> Result<Value, String> {
        let client = crate::memory_relations::S1Client::from_env();
        let live = crate::memory_consolidation::relation_bank_live();
        let r = crate::memory_consolidation::run_for_user(&self.state.db, &client, user, live).await?;
        Ok(serde_json::to_value(r).unwrap_or(Value::Null))
    }

    fn dream_due(&self, conn: &Connection, user: &str, now: DateTime<Utc>) -> Result<Option<NaiveDate>, String> {
        crate::memory_dream::due_date(conn, user, now).map_err(|e| e.to_string())
    }

    async fn dream(&self, user: &str, date: NaiveDate) -> Result<Value, String> {
        let row = crate::memory_dream::run(self.state.db.clone(), user.to_string(), date, crate::memory_dream::gizzi_completer(user.to_string()))
            .await
            .map_err(|e| e.to_string())?;
        // A failed Dream keeps its own bounded retry (attempts per drive and
        // date); the cycle reports it and the next due cycle retries. The Dream
        // id and its commits are what `report.rs` needs to undo it.
        Ok(dream_detail(date, row))
    }

    fn curation_due(&self, conn: &Connection, user: &str, now: DateTime<Utc>) -> Result<Vec<String>, String> {
        Ok(crate::memory_curation::due_bots(conn, Some(user), now).into_iter().map(|(bot, _)| bot).collect())
    }

    async fn curate(&self, user: &str, bots: Vec<String>) -> Result<Value, String> {
        let provider = crate::internal_batch::enabled().then(|| crate::internal_batch::provider_from_config(&self.state.config));
        let n = bots.len();
        let due = bots.into_iter().map(|b| (b, user.to_string())).collect();
        crate::memory_curation::curate_due(&self.state.db, due, provider.as_deref()).await;
        Ok(json!({ "bots": n }))
    }
}

// ── Debt ────────────────────────────────────────────────────────────────────

/// Sleep debt: memories added since the user's last finished nap (at most the
/// last 7 days), over `debt_scale`.
pub fn debt(conn: &Connection, cfg: &Resolved, user: &str, now: DateTime<Utc>) -> rusqlite::Result<f64> {
    let floor = queue::ts(now - Duration::days(7));
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_facts WHERE user_id = ?1 AND valid_until IS NULL
           AND datetime(valid_from) > datetime(MAX(?2, COALESCE((SELECT MAX(finished_at) FROM sleep_cycles
               WHERE user_id = ?1 AND kind = 'nap' AND state = 'finished'), '')))",
        params![user, floor],
        |r| r.get(0),
    )?;
    Ok(n as f64 / cfg.number("debt_scale").max(1.0))
}

/// While the debt is at or above `debt_pause_threshold` (and the user's
/// `orchestrator` stage is on), memory decay pauses: consolidation skips its
/// decay write. Retrieval strength (`strength.rs`) is computed on read from
/// time and has no decay write to skip.
pub fn decay_paused(conn: &Connection, cfg: &Resolved, user: &str, now: DateTime<Utc>) -> bool {
    cfg.stage_on(Stage::Orchestrator)
        && debt(conn, cfg, user, now).map(|d| d >= cfg.number("debt_pause_threshold")).unwrap_or(false)
}

/// Write one Sleep event on the memory activity stream. Best effort: a
/// failed write is dropped and never reaches the caller.
pub fn emit(conn: &Connection, user: &str, kind: &'static str, title: String, extra: Value, session: Option<String>, run: Option<String>) {
    let _ = crate::memory_activity::record(
        conn,
        user,
        crate::memory_activity::Activity { kind, title, extra: Some(extra), session, run, ..Default::default() },
    );
}

/// The chat session an event belongs to: none for clock and manual units.
fn event_session(unit: &Unit) -> Option<String> {
    Some(unit.session().to_string()).filter(|s| !s.is_empty() && s != queue::SESSION_CLOCK && s != queue::SESSION_MANUAL)
}

/// Emit `sleep.debt.changed` when the debt has crossed `debt_pause_threshold`
/// (up or down) since the last such event; otherwise write nothing. The last
/// band is read from the user's newest `sleep.debt.changed` row (no row means
/// "below"), so no extra state is stored. Called at a cycle's start and end,
/// never on a read of `GET status`.
pub fn emit_debt_if_crossed(conn: &Connection, cfg: &Resolved, user: &str, now: DateTime<Utc>) {
    let Ok(d) = debt(conn, cfg, user, now) else { return };
    let threshold = cfg.number("debt_pause_threshold");
    let paused = d >= threshold;
    let last: Option<bool> = conn
        .query_row(
            "SELECT detail FROM memory_activity WHERE user_id = ?1 AND kind = ?2 ORDER BY id DESC LIMIT 1",
            params![user, super::types::events::DEBT_CHANGED],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|j| serde_json::from_str::<Value>(&j).ok())
        .and_then(|v| v["decay_paused"].as_bool());
    if last.unwrap_or(false) == paused {
        return;
    }
    let title = if paused { "Memories are waiting for a nap" } else { "Sleep debt is back under the threshold" };
    emit(
        conn,
        user,
        super::types::events::DEBT_CHANGED,
        title.into(),
        json!({ "debt": d, "threshold": threshold, "decay_paused": paused }),
        None,
        None,
    );
}

// ── Cycle rows ──────────────────────────────────────────────────────────────

fn spend_today(conn: &Connection, user: &str, now: DateTime<Utc>) -> rusqlite::Result<f64> {
    let day = now.format("%Y-%m-%d 00:00:00").to_string();
    conn.query_row(
        "SELECT COALESCE(SUM(spend_usd), 0.0) FROM sleep_cycles WHERE user_id = ?1 AND started_at >= ?2",
        params![user, day],
        |r| r.get(0),
    )
}

fn open_cycle(conn: &Connection, unit: &Unit, now: DateTime<Utc>) -> rusqlite::Result<String> {
    let id = format!("cyc_{}", uuid::Uuid::new_v4().simple());
    let session = Some(unit.session()).filter(|s| !s.is_empty());
    conn.execute(
        "INSERT INTO sleep_cycles (id, user_id, session_id, trigger, kind, state, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6)",
        params![id, unit.user_id, session, unit.trigger(), unit.kind, queue::ts(now)],
    )?;
    conn.execute("UPDATE sleep_units SET cycle_id = ?2 WHERE id = ?1", params![unit.id, id])?;
    Ok(id)
}

fn close_cycle(conn: &Connection, report: &CycleReport, now: DateTime<Utc>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sleep_cycles SET state = ?2, spend_usd = ?3, report_json = ?4, error = ?5, finished_at = ?6 WHERE id = ?1",
        params![report.cycle_id, report.state, report.spend_usd, json!(report).to_string(), report.error, queue::ts(now)],
    )?;
    Ok(())
}

fn can_spend(cfg: &Resolved, spent_today: f64, estimate: f64) -> bool {
    let cap = cfg.daily_spend_cap_usd();
    cap > 0.0 && spent_today + estimate <= cap
}

/// Real model spend in USD that the usage ledger (`llm_usage_events`) holds
/// for `user` in `[from, to)`: the `memory` surface, which is what Dream's
/// `gizzi_completer` (`memory_dream.rs`) and curation's `curate_bot`
/// (`memory_curation.rs`) scope their calls to, tenant = user. S1 decision
/// rows carry no spend and are left out. Ledger rows are written as the model
/// call finishes and can land after the stage returns, so `report.rs` reads
/// this again at report time and records any late spend on the cycle.
pub fn ledger_spend_usd(conn: &Connection, user: &str, from: &str, to: &str) -> rusqlite::Result<f64> {
    let micro: i64 = conn.query_row(
        "SELECT COALESCE(SUM(COALESCE(recomputed_cost_microdollars, cost_microdollars)), 0)
           FROM llm_usage_events
          WHERE user_id = ?1 AND surface = 'memory' AND COALESCE(tier, '') != 'S1'
            AND status != 'in_progress' AND created_at >= ?2 AND created_at < ?3",
        params![user, from, to],
        |r| r.get(0),
    )?;
    Ok(micro as f64 / 1_000_000.0)
}

fn sql(e: rusqlite::Error) -> String {
    e.to_string()
}

#[allow(clippy::too_many_arguments)]
async fn run_step(db: &DbHandle, jobs: &dyn Jobs, cfg: &Resolved, unit: &Unit, cycle_id: &str, step: Step, now: DateTime<Utc>, spent_today: f64) -> Result<StepRecord, String> {
    let user = unit.user_id.as_str();
    match step {
        Step::Capture => {
            let conn = db.connect().map_err(sql)?;
            let captured = queue::finish_captures(&conn, user, cycle_id, now).map_err(sql)?;
            let units: Vec<Value> = captured.into_iter().map(|(id, what)| json!({ "unit": id, "capture": what })).collect();
            Ok(StepRecord::done(step, json!({ "captured": units })))
        }
        Step::Consolidation => Ok(StepRecord::done(step, jobs.consolidate(user).await?)),
        Step::Fit => {
            if !cfg.stage_on(Stage::Strengths) {
                return Ok(StepRecord::skipped(step, "stage_off: strengths"));
            }
            let conn = db.connect().map_err(sql)?;
            let r = super::fit::fit_user(&conn, user, now).map_err(|e| e.to_string())?;
            Ok(StepRecord::done(step, json!({
                "events": r.events, "fitted": r.fitted, "kept_owner": r.kept_owner, "skipped_reason": r.skipped_reason,
            })))
        }
        Step::Dream => {
            let due = {
                let conn = db.connect().map_err(sql)?;
                jobs.dream_due(&conn, user, now)?
            };
            let Some(date) = due else { return Ok(StepRecord::skipped(step, "not_due")) };
            let estimate = cfg.number("dream_cost_estimate_usd");
            if !can_spend(cfg, spent_today, estimate) {
                return Ok(StepRecord::skipped(step, "spend_cap"));
            }
            // The estimate above only gates the run. The real cost is read from
            // the usage ledger by `run_once` once the stage returns (see
            // [`ledger_spend_usd`]).
            Ok(StepRecord::done(step, jobs.dream(user, date).await?))
        }
        Step::Rem => {
            let conn = db.connect().map_err(sql)?;
            match super::rem::run_stage(&conn, user, cycle_id, now).map_err(sql)? {
                super::rem::Outcome::Off => Ok(StepRecord::skipped(step, "stage_off: rem")),
                super::rem::Outcome::AutoOff => Ok(StepRecord::skipped(step, "rem_auto_off")),
                super::rem::Outcome::Ran(detail) => Ok(StepRecord::done(step, detail)),
            }
        }
        Step::Curation => {
            let bots = {
                let conn = db.connect().map_err(sql)?;
                jobs.curation_due(&conn, user, now)?
            };
            if bots.is_empty() {
                return Ok(StepRecord::skipped(step, "not_due"));
            }
            let estimate = cfg.number("curation_cost_estimate_usd");
            if !can_spend(cfg, spent_today, estimate) {
                return Ok(StepRecord::skipped(step, "spend_cap"));
            }
            Ok(StepRecord::done(step, jobs.curate(user, bots).await?))
        }
        Step::Pages => Ok(StepRecord::skipped(step, "waits for G1/4.2")),
        Step::Checks => {
            if !cfg.stage_on(Stage::Checks) {
                return Ok(StepRecord::skipped(step, "stage_off: checks"));
            }
            let conn = db.connect().map_err(sql)?;
            match super::checks::run_stage(&conn, user, cycle_id, now).map_err(sql)? {
                Some(detail) => Ok(StepRecord::done(step, detail)),
                None => Ok(StepRecord::skipped(step, "stage_off: checks")),
            }
        }
        Step::Sign => Ok(StepRecord::skipped(step, "waits for step 4.1")),
    }
}

/// Leases and runs one unit as a cycle. `Ok(None)` when nothing is runnable.
pub async fn run_once(db: &DbHandle, jobs: &dyn Jobs, worker: &str, clock: &Clock) -> Result<Option<CycleReport>, String> {
    let now = clock();
    let (unit, cfg, cycle_id, spent_before) = {
        let conn = db.connect().map_err(sql)?;
        let global = config::load(&conn, "").map_err(sql)?;
        let lease_s = global.number("unit_lease_s") as i64;
        let max = global.number("unit_max_attempts") as i64;
        let Some(unit) = queue::claim(&conn, worker, now, lease_s, max).map_err(sql)? else { return Ok(None) };
        let cfg = config::load(&conn, &unit.user_id).map_err(sql)?;
        if !cfg.stage_on(Stage::Orchestrator) {
            queue::cancel(&conn, &unit.id, "orchestrator stage is off").map_err(sql)?;
            return Ok(Some(CycleReport {
                cycle_id: None, unit_id: unit.id.clone(), user_id: unit.user_id.clone(), kind: unit.kind.clone(),
                state: "cancelled", steps: vec![], spend_usd: 0.0, stop_reason: None, resume_from_stage: None,
                held_unit: None, error: Some("orchestrator stage is off".into()),
            }));
        }
        // A new cycle: units deferred in an earlier one get their next try.
        queue::release_deferred(&conn, &unit.user_id).map_err(sql)?;
        let cycle_id = open_cycle(&conn, &unit, now).map_err(sql)?;
        emit(&conn, &unit.user_id, super::types::events::CYCLE_STARTED, format!("Sleep started ({})", unit.kind),
            json!({ "cycle": cycle_id, "unit": unit.id, "kind": unit.kind, "trigger": unit.trigger() }),
            event_session(&unit), Some(cycle_id.clone()));
        emit_debt_if_crossed(&conn, &cfg, &unit.user_id, now);
        let spent = spend_today(&conn, &unit.user_id, now).map_err(sql)?;
        (unit, cfg, cycle_id, spent)
    };
    let lease_s = cfg.number("unit_lease_s") as i64;
    let max_attempts = cfg.number("unit_max_attempts") as i64;
    let max_steps = cfg.number("cycle_max_stages").max(1.0) as usize;
    let max_minutes = cfg.number("cycle_max_minutes").max(1.0) as i64;
    let plan = plan_for(&unit.kind);
    let start = unit.resume_from().and_then(|s| plan.iter().position(|p| p.as_str() == s)).unwrap_or(0);
    let mut report = CycleReport {
        cycle_id: Some(cycle_id.clone()), unit_id: unit.id.clone(), user_id: unit.user_id.clone(), kind: unit.kind.clone(),
        state: "finished", steps: vec![], spend_usd: 0.0, stop_reason: None, resume_from_stage: None, held_unit: None, error: None,
    };

    let started = now;
    for step in plan.iter().skip(start).copied() {
        let now = clock();
        {
            let conn = db.connect().map_err(sql)?;
            let stop = if queue::stop_requested(&conn, &unit.id).map_err(sql)? {
                Some("resume")
            } else if report.steps.len() >= max_steps {
                Some("stop_rule: cycle_max_stages")
            } else if !report.steps.is_empty() && now - started >= Duration::minutes(max_minutes) {
                Some("stop_rule: cycle_max_minutes")
            } else {
                None
            };
            if let Some(reason) = stop {
                report.state = "stopped";
                report.stop_reason = Some(reason.into());
                report.resume_from_stage = Some(step.as_str());
                report.held_unit = Some(queue::hold_rest(&conn, &unit, step.as_str(), reason).map_err(sql)?);
                close_cycle(&conn, &report, now).map_err(sql)?;
                queue::done(&conn, &unit.id, &cycle_id, report.spend_usd, now).map_err(sql)?;
                finish_events(&conn, &cfg, &unit, &report, now);
                return Ok(Some(report));
            }
            conn.execute("UPDATE sleep_cycles SET stage = ?2 WHERE id = ?1", params![cycle_id, step.as_str()]).map_err(sql)?;
            queue::renew(&conn, &unit.id, worker, now, lease_s).map_err(sql)?;
        }
        match run_step(db, jobs, &cfg, &unit, &cycle_id, step, now, spent_before + report.spend_usd).await {
            Ok(mut rec) => {
                let end = clock();
                rec.started_at = Some(queue::ts(now));
                rec.finished_at = Some(queue::ts(end));
                if rec.status == "done" && matches!(step, Step::Dream | Step::Curation) {
                    let conn = db.connect().map_err(sql)?;
                    // Up to the next second so rows stamped in the stage's last second count.
                    rec.spend_usd = ledger_spend_usd(&conn, &unit.user_id, &queue::ts(now), &queue::ts(end + Duration::seconds(1))).map_err(sql)?;
                }
                report.spend_usd += rec.spend_usd;
                let (status, note, spend) = (rec.status, rec.note.clone(), rec.spend_usd);
                report.steps.push(rec);
                let conn = db.connect().map_err(sql)?;
                conn.execute("UPDATE sleep_cycles SET spend_usd = ?2 WHERE id = ?1", params![cycle_id, report.spend_usd]).map_err(sql)?;
                emit(&conn, &unit.user_id, super::types::events::CYCLE_STAGE, format!("Sleep stage {}: {status}", step.as_str()),
                    json!({ "cycle": cycle_id, "stage": step.as_str(), "status": status, "note": note, "spend_usd": spend }),
                    event_session(&unit), Some(cycle_id.clone()));
            }
            Err(e) => {
                tracing::warn!(user = %unit.user_id, step = step.as_str(), error = %e, "sleep cycle stage failed");
                report.state = "failed";
                report.error = Some(format!("{}: {e}", step.as_str()));
                let conn = db.connect().map_err(sql)?;
                close_cycle(&conn, &report, clock()).map_err(sql)?;
                queue::fail(&conn, &unit.id, report.error.as_deref().unwrap_or(""), max_attempts, clock()).map_err(sql)?;
                emit(&conn, &unit.user_id, super::types::events::CYCLE_STAGE, format!("Sleep stage {}: failed", step.as_str()),
                    json!({ "cycle": cycle_id, "stage": step.as_str(), "status": "failed", "note": e, "spend_usd": 0.0 }),
                    event_session(&unit), Some(cycle_id.clone()));
                finish_events(&conn, &cfg, &unit, &report, clock());
                return Ok(Some(report));
            }
        }
    }
    let now = clock();
    let conn = db.connect().map_err(sql)?;
    close_cycle(&conn, &report, now).map_err(sql)?;
    queue::done(&conn, &unit.id, &cycle_id, report.spend_usd, now).map_err(sql)?;
    finish_events(&conn, &cfg, &unit, &report, now);
    Ok(Some(report))
}

/// `sleep.cycle.finished` (any end: finished, stopped or failed), then the
/// debt check.
fn finish_events(conn: &Connection, cfg: &Resolved, unit: &Unit, report: &CycleReport, now: DateTime<Utc>) {
    let done = report.steps.iter().filter(|s| s.status == "done").count();
    emit(conn, &unit.user_id, super::types::events::CYCLE_FINISHED, format!("Sleep {} ({})", report.state, unit.kind),
        json!({ "cycle": report.cycle_id, "kind": unit.kind, "state": report.state, "spend_usd": report.spend_usd,
                "stages": report.steps.len(), "stages_done": done, "stop_reason": report.stop_reason, "error": report.error }),
        event_session(&unit), report.cycle_id.clone());
    emit_debt_if_crossed(conn, cfg, &unit.user_id, now);
}

// ── Worker ──────────────────────────────────────────────────────────────────

/// One background task (spawned in main.rs next to the old clocks): drains
/// the queue one unit at a time, then waits `worker_poll_s` (global
/// `sleep_config` row, default 30 s). `ALLTERNIT_SLEEP_WORKER=0` disables.
/// Nothing is queued unless a user turns the `orchestrator` stage on.
pub fn spawn(state: Arc<crate::AppState>) {
    if std::env::var("ALLTERNIT_SLEEP_WORKER").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    tokio::spawn(async move {
        let worker = format!("sleep-{}", uuid::Uuid::new_v4().simple());
        let jobs = LiveJobs { state: state.clone() };
        let clock: Clock = Arc::new(Utc::now);
        loop {
            loop {
                match run_once(&state.db, &jobs, &worker, &clock).await {
                    // A failure backs off until the next poll.
                    Ok(Some(r)) if r.state == "failed" => break,
                    Ok(Some(r)) => tracing::info!(user = %r.user_id, kind = %r.kind, state = r.state, spend = r.spend_usd, "sleep cycle"),
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("sleep worker: {e}");
                        break;
                    }
                }
            }
            let poll = state
                .db
                .connect()
                .ok()
                .and_then(|c| config::load(&c, "").ok())
                .map(|c| c.number("worker_poll_s"))
                .unwrap_or(30.0);
            tokio::time::sleep(std::time::Duration::from_secs(poll.max(5.0) as u64)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sleep::config::Group;
    use crate::sleep::trigger;
    use crate::sleep::types::SignalEvent;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    fn db() -> DbHandle {
        let dir = std::env::temp_dir().join(format!("sleep-cycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        DbHandle::new(dir.join("t.db")).unwrap()
    }

    fn clock() -> Clock {
        let t = super::super::strength::parse_ts("2026-11-19 12:00:00").unwrap();
        Arc::new(move || t)
    }

    /// Sleep on for `user` with `stages`, and a daily cap.
    fn turn_on(db: &DbHandle, user: &str, stages: &[&str], cap: f64) {
        let mut values = vec![("enabled".to_string(), json!(true)), ("daily_spend_cap_usd".to_string(), json!(cap))];
        values.extend(stages.iter().map(|s| (format!("stage.{s}"), json!(true))));
        config::set_owner(&db.connect().unwrap(), user, Group::Setting, &values).unwrap();
    }

    fn signal(db: &DbHandle, user: &str, session: &str, event: SignalEvent) -> trigger::Outcome {
        let conn = db.connect().unwrap();
        let cfg = config::load(&conn, user).unwrap();
        trigger::handle(&conn, &cfg, user, session, event)
    }

    fn unit_state(db: &DbHandle, id: &str) -> (String, i64) {
        db.connect().unwrap().query_row("SELECT state, attempts FROM sleep_units WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    fn steps(r: &CycleReport) -> Vec<(&'static str, &'static str)> {
        r.steps.iter().map(|s| (s.step.as_str(), s.status)).collect()
    }

    /// Stubs only the jobs that call models (consolidation's S1, Dream,
    /// curation); every job is due.
    #[derive(Default)]
    struct Stub {
        calls: Mutex<Vec<String>>,
        fail_consolidation: AtomicBool,
        /// (db, user, session): send a `resume` signal while consolidation runs, once.
        resume_during: Mutex<Option<(DbHandle, String, String)>>,
        /// (db, user, micro-dollars): the model spend Dream's call writes to the usage ledger.
        dream_spend: Mutex<Option<(DbHandle, String, i64)>>,
    }

    /// One usage-ledger row for the `memory` surface, stamped at the test clock.
    fn ledger_row(db: &DbHandle, user: &str, micro: i64) {
        db.connect().unwrap().execute(
            "INSERT INTO llm_usage_events (id, user_id, surface, cost_microdollars, status, created_at)
             VALUES (?1, ?2, 'memory', ?3, 'ok', '2026-11-19 12:00:00')",
            params![uuid::Uuid::new_v4().to_string(), user, micro],
        ).unwrap();
    }

    #[async_trait]
    impl Jobs for Stub {
        async fn consolidate(&self, _user: &str) -> Result<Value, String> {
            self.calls.lock().unwrap().push("consolidate".into());
            if let Some((db, user, session)) = self.resume_during.lock().unwrap().take() {
                signal(&db, &user, &session, SignalEvent::Resume);
            }
            if self.fail_consolidation.load(Ordering::SeqCst) {
                return Err("consolidation broke".into());
            }
            Ok(json!({ "merged": 0 }))
        }
        fn dream_due(&self, _conn: &Connection, _user: &str, now: DateTime<Utc>) -> Result<Option<NaiveDate>, String> {
            Ok(Some(now.date_naive()))
        }
        async fn dream(&self, _user: &str, _date: NaiveDate) -> Result<Value, String> {
            self.calls.lock().unwrap().push("dream".into());
            if let Some((db, user, micro)) = self.dream_spend.lock().unwrap().take() {
                ledger_row(&db, &user, micro);
            }
            Ok(json!({ "status": "applied" }))
        }
        fn curation_due(&self, _conn: &Connection, _user: &str, _now: DateTime<Utc>) -> Result<Vec<String>, String> {
            Ok(vec!["bot-1".into()])
        }
        async fn curate(&self, _user: &str, bots: Vec<String>) -> Result<Value, String> {
            self.calls.lock().unwrap().push("curate".into());
            Ok(json!({ "bots": bots.len() }))
        }
    }

    #[tokio::test]
    async fn the_stage_order_holds_when_every_job_is_due() {
        let db = db();
        turn_on(&db, "u1", &["orchestrator", "strengths"], 10.0);
        let unit = signal(&db, "u1", "s1", SignalEvent::SessionEnd).queued[0].clone();
        let stub = Stub::default();
        *stub.dream_spend.lock().unwrap() = Some((db.clone(), "u1".into(), 40_000));
        // Another user's memory spend in the same second is not this cycle's.
        ledger_row(&db, "someone-else", 900_000);

        let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
        assert_eq!(r.state, "finished", "{r:?}");
        assert_eq!(steps(&r), vec![
            ("capture", "done"), ("consolidation", "done"), ("fit", "done"), ("dream", "done"),
            ("rem", "skipped"), ("curation", "done"), ("pages", "skipped"), ("checks", "skipped"), ("sign", "skipped"),
        ]);
        assert_eq!(r.steps[6].note.as_deref(), Some("waits for G1/4.2"));
        assert_eq!(*stub.calls.lock().unwrap(), vec!["consolidate", "dream", "curate"]);
        let conn = db.connect().unwrap();
        let (state, stage, spend): (String, String, f64) = conn
            .query_row("SELECT state, stage, spend_usd FROM sleep_cycles WHERE id = ?1", [r.cycle_id.as_deref().unwrap()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((state.as_str(), stage.as_str()), ("finished", "sign"));
        // The real cost from the usage ledger (Dream wrote 0.04; curation wrote
        // nothing); the 0.05 estimates only gate the cap.
        assert!((spend - 0.04).abs() < 1e-9, "dream's ledger cost: {spend}");
        assert!((r.steps[3].spend_usd - 0.04).abs() < 1e-9 && r.steps[5].spend_usd == 0.0);
        assert_eq!(unit_state(&db, &unit).0, "done");
        // Nothing else to run.
        assert!(run_once(&db, &stub, "w1", &clock()).await.unwrap().is_none());
    }

    fn kinds(db: &DbHandle) -> Vec<(String, Option<String>)> {
        let conn = db.connect().unwrap();
        crate::memory_activity::list(&conn, "u1", &crate::memory_activity::Filter { after: Some(0), limit: Some(200), ..Default::default() })
            .unwrap().into_iter().map(|r| (r.kind, r.detail["stage"].as_str().map(String::from))).collect()
    }

    #[tokio::test]
    async fn a_nap_writes_started_one_stage_event_per_stage_and_finished_in_order() {
        let db = db();
        turn_on(&db, "u1", &["orchestrator", "strengths"], 10.0);
        signal(&db, "u1", "s1", SignalEvent::SessionEnd);
        let r = run_once(&db, &Stub::default(), "w1", &clock()).await.unwrap().unwrap();
        assert_eq!(r.state, "finished", "{r:?}");
        let rows: Vec<_> = kinds(&db).into_iter().filter(|(k, _)| k != "sleep.debt.changed").collect();
        let mut want = vec![("sleep.cycle.started".to_string(), None)];
        want.extend(r.steps.iter().map(|s| ("sleep.cycle.stage".to_string(), Some(s.step.as_str().to_string()))));
        want.push(("sleep.cycle.finished".to_string(), None));
        assert_eq!(rows, want);
    }

    #[test]
    fn debt_changed_is_written_only_when_the_threshold_is_crossed() {
        let db = db();
        let conn = db.connect().unwrap();
        for (k, v) in [("enabled", "true"), ("debt_scale", "1")] {
            conn.execute("INSERT INTO sleep_config (user_id, key, value_json, source) VALUES ('u1', ?1, ?2, 'owner')", [k, v]).unwrap();
        }
        let cfg = config::load(&conn, "u1").unwrap();
        let now = clock()();
        let count = |c: &Connection| -> i64 { c.query_row("SELECT COUNT(*) FROM memory_activity WHERE kind = 'sleep.debt.changed'", [], |r| r.get(0)).unwrap() };
        // Below the threshold: nothing, however often it is checked.
        emit_debt_if_crossed(&conn, &cfg, "u1", now);
        emit_debt_if_crossed(&conn, &cfg, "u1", now);
        assert_eq!(count(&conn), 0);
        // One new memory crosses up (threshold 1.0, scale 1): one event; repeats add none.
        conn.execute("INSERT INTO memory_facts (id, user_id, fact, valid_from) VALUES ('f1', 'u1', 'x', datetime('2026-11-19 11:00:00'))", []).unwrap();
        emit_debt_if_crossed(&conn, &cfg, "u1", now);
        emit_debt_if_crossed(&conn, &cfg, "u1", now);
        assert_eq!(count(&conn), 1);
        // A finished nap brings it back down: one more event, with decay_paused false.
        conn.execute("INSERT INTO sleep_cycles (id, user_id, trigger, kind, state, started_at, finished_at) VALUES ('c1', 'u1', 'idle', 'nap', 'finished', '2026-11-19 11:30:00', '2026-11-19 11:40:00')", []).unwrap();
        emit_debt_if_crossed(&conn, &cfg, "u1", now);
        emit_debt_if_crossed(&conn, &cfg, "u1", now);
        assert_eq!(count(&conn), 2);
        let last: String = conn.query_row("SELECT detail FROM memory_activity WHERE kind = 'sleep.debt.changed' ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&last).unwrap()["decay_paused"], false);
    }

    #[tokio::test]
    async fn three_failures_move_a_unit_to_the_next_cycle() {
        let db = db();
        turn_on(&db, "u1", &["orchestrator"], 0.0);
        let unit = signal(&db, "u1", "s1", SignalEvent::SessionEnd).queued[0].clone();
        let stub = Stub::default();
        stub.fail_consolidation.store(true, Ordering::SeqCst);

        for (n, want) in [(1, "queued"), (2, "queued"), (3, "deferred")] {
            let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
            assert_eq!(r.state, "failed");
            assert_eq!(unit_state(&db, &unit), (want.to_string(), n));
        }
        // Deferred: not retried in this cycle.
        assert!(run_once(&db, &stub, "w1", &clock()).await.unwrap().is_none());

        // The next cycle (another unit) releases it, and it runs after that one.
        stub.fail_consolidation.store(false, Ordering::SeqCst);
        let next = signal(&db, "u1", "s2", SignalEvent::SessionEnd).queued[0].clone();
        let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
        assert_eq!((r.unit_id.as_str(), r.state), (next.as_str(), "finished"));
        assert_eq!(unit_state(&db, &unit), ("queued".to_string(), 0));
        let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
        assert_eq!((r.unit_id.as_str(), r.state), (unit.as_str(), "finished"));
    }

    #[tokio::test]
    async fn resume_stops_the_cycle_after_the_current_stage_and_the_next_idle_continues_from_the_next_stage() {
        let db = db();
        turn_on(&db, "u1", &["orchestrator"], 10.0);
        let unit = signal(&db, "u1", "s1", SignalEvent::Idle).queued[0].clone();
        let stub = Stub::default();
        // The user comes back while consolidation runs.
        *stub.resume_during.lock().unwrap() = Some((db.clone(), "u1".into(), "s1".into()));

        let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
        assert_eq!((r.state, r.stop_reason.as_deref(), r.resume_from_stage), ("stopped", Some("resume"), Some("fit")));
        assert_eq!(steps(&r), vec![("capture", "done"), ("consolidation", "done")]);
        let held = r.held_unit.clone().unwrap();
        assert_eq!(unit_state(&db, &unit).0, "done");
        assert_eq!(unit_state(&db, &held).0, "held");
        // Held work waits for the session's next signal.
        assert!(run_once(&db, &stub, "w1", &clock()).await.unwrap().is_none());

        // The next idle continues that unit instead of starting over.
        assert_eq!(signal(&db, "u1", "s1", SignalEvent::Idle).queued, vec![held.clone()]);
        let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
        assert_eq!(r.unit_id, held);
        assert_eq!(r.state, "finished");
        assert_eq!(steps(&r).iter().map(|s| s.0).collect::<Vec<_>>(), vec!["fit", "dream", "rem", "curation", "pages", "checks", "sign"]);
        assert_eq!(*stub.calls.lock().unwrap(), vec!["consolidate", "dream", "curate"]);
    }

    #[tokio::test]
    async fn the_spend_cap_stops_new_spending_stages() {
        let db = db();
        // Cap 0 (the default): no model spending; consolidation still runs.
        turn_on(&db, "u1", &["orchestrator"], 0.0);
        signal(&db, "u1", "s1", SignalEvent::SessionEnd);
        let stub = Stub::default();
        let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
        assert_eq!(r.state, "finished");
        let by: Vec<_> = r.steps.iter().map(|s| (s.step.as_str(), s.status, s.note.clone())).collect();
        assert_eq!(by[1], ("consolidation", "done", None));
        assert_eq!(by[3], ("dream", "skipped", Some("spend_cap".to_string())));
        assert_eq!(by[5], ("curation", "skipped", Some("spend_cap".to_string())));
        assert_eq!(*stub.calls.lock().unwrap(), vec!["consolidate"]);
        assert_eq!(r.spend_usd, 0.0);

        // A cap that fits one estimate: Dream runs, curation would pass the cap.
        turn_on(&db, "u2", &["orchestrator"], 0.05);
        signal(&db, "u2", "s1", SignalEvent::SessionEnd);
        let stub = Stub::default();
        *stub.dream_spend.lock().unwrap() = Some((db.clone(), "u2".into(), 50_000));
        let r = run_once(&db, &stub, "w1", &clock()).await.unwrap().unwrap();
        assert_eq!((r.steps[3].status, r.steps[5].note.as_deref()), ("done", Some("spend_cap")));
        assert_eq!(*stub.calls.lock().unwrap(), vec!["consolidate", "dream"]);
    }

    #[tokio::test]
    async fn orchestrator_off_the_old_clocks_behave_as_today() {
        let db = db();
        {
            let conn = db.connect().unwrap();
            for (id, user) in [("f1", "u-off"), ("f2", "u-on")] {
                conn.execute("INSERT INTO memory_facts (id, user_id, fact, confidence) VALUES (?1, ?2, 'Lives in Austin.', 0.9)", [id, user]).unwrap();
            }
        }
        turn_on(&db, "u-on", &["orchestrator"], 0.0);
        let mut client = crate::memory_relations::S1Client::new("http://127.0.0.1:9");
        client.enabled = false;

        assert_eq!(crate::memory_consolidation::tick_once(&db, &client, false, 50).await, (1, 1));
        let conn = db.connect().unwrap();
        let runs: Vec<String> = conn
            .prepare("SELECT user_id FROM memory_consolidation_runs").unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(runs, vec!["u-off".to_string()], "the off user is consolidated by the clock as before");
        let units: Vec<(String, String, String)> = conn
            .prepare("SELECT user_id, stage, json_extract(payload_json, '$.session_id') FROM sleep_units").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(units, vec![("u-on".to_string(), "nap".to_string(), "clock".to_string())]);
        // A second tick does not queue a second nap.
        assert_eq!(crate::memory_consolidation::tick_once(&db, &client, false, 50).await, (0, 1));
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM sleep_units", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }
}
