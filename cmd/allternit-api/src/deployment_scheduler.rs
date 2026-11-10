//! Deployment scheduler daemon.
//!
//! `/beta/deployments` stores agent + cron bindings with `next_run_at`, but
//! nothing executed them when cron came due — runs stayed `running` forever
//! unless a client manually triggered or patched. This daemon closes that
//! gap: it polls `beta_deployments` for active rows whose `next_run_at` is
//! due, creates a `beta_deployment_runs` row (`triggered_by = 'scheduler'`),
//! enqueues the deployment-tied `beta_work_tasks` row external workers
//! execute, and advances `next_run_at`.
//!
//! Claiming is restart-safe: the advance is a conditional
//! `UPDATE … WHERE id = ? AND next_run_at = ?expected`. If it affects 0 rows,
//! another scheduler instance (or an interleaved PATCH) already claimed or
//! rescheduled the row, and this tick skips it. The claim and the run/task
//! inserts happen in one SQLite transaction, so a crash mid-tick cannot
//! advance `next_run_at` without creating the run.
//!
//! Overdue policy is **fire-once**: a deployment that was due while the
//! server was down fires exactly one run on catch-up, never a burst of
//! missed occurrences. The next occurrence is computed from the DUE time
//! (not the tick time) so ordinary tick latency does not drift the
//! schedule; only when that next occurrence is itself already in the past
//! (multiple missed occurrences) does the scheduler fall back to the next
//! occurrence after now — still firing just the single catch-up run.
//!
//! The scheduler only CREATES runs. Terminal status (succeeded/failed/
//! cancelled) remains the worker's job via the existing
//! `PATCH /beta/deployments/:id/runs/:run_id` path.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde_json::json;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::db::DbHandle;
use crate::AppState;

/// Env var overriding the poll interval, in seconds.
pub const INTERVAL_ENV: &str = "DEPLOYMENT_SCHEDULER_INTERVAL_SECS";
/// Default poll interval when the env var is unset or unparseable.
pub const DEFAULT_INTERVAL_SECS: u64 = 15;

/// Shared, cheaply-cloneable scheduler counters surfaced in
/// `GET /api/v1/monitor/system`.
pub struct DeploymentSchedulerState {
    /// Unix seconds of the last successful tick; 0 = never ticked.
    last_tick_at: AtomicI64,
    /// Total runs created by the scheduler process lifetime.
    runs_fired_total: AtomicU64,
}

impl DeploymentSchedulerState {
    pub fn new() -> Self {
        Self {
            last_tick_at: AtomicI64::new(0),
            runs_fired_total: AtomicU64::new(0),
        }
    }

    pub fn record_tick(&self) {
        self.last_tick_at
            .store(Utc::now().timestamp(), Ordering::Relaxed);
    }

    pub fn record_fired(&self, count: u64) {
        self.runs_fired_total.fetch_add(count, Ordering::Relaxed);
    }

    /// `{last_tick_at, runs_fired_total}` as a monitor-ready JSON value.
    pub fn snapshot(&self) -> serde_json::Value {
        let last_tick = self.last_tick_at.load(Ordering::Relaxed);
        json!({
            "last_tick_at": match last_tick {
                0 => serde_json::Value::Null,
                secs => DateTime::<Utc>::from_timestamp(secs, 0)
                    .map(|dt| json!(dt.to_rfc3339()))
                    .unwrap_or_else(|| json!(secs.to_string())),
            },
            "runs_fired_total": self.runs_fired_total.load(Ordering::Relaxed),
        })
    }
}

impl Default for DeploymentSchedulerState {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a poll-interval value. Unparseable or absurd values (0 or huge)
/// fall back to the default.
fn parse_interval_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|s| s.parse::<u64>().ok())
        .filter(|v| (1..=3600).contains(v))
        .unwrap_or(DEFAULT_INTERVAL_SECS)
}

/// Poll interval from [`INTERVAL_ENV`], defaulting to
/// [`DEFAULT_INTERVAL_SECS`].
pub fn interval_from_env() -> Duration {
    Duration::from_secs(parse_interval_secs(std::env::var(INTERVAL_ENV).ok().as_deref()))
}

/// One run created by a scheduler tick.
#[derive(Debug, Clone)]
pub struct FiredRun {
    pub deployment_id: String,
    pub run_id: String,
    pub task_id: String,
    pub user_id: String,
    pub next_run_at: DateTime<Utc>,
}

struct DueDeployment {
    id: String,
    user_id: String,
    agent_id: Option<String>,
    cron: String,
    next_run_at: String,
}

/// Advance `next_run_at` past `due_at`: normally the first occurrence after
/// the DUE time (no per-tick drift); if multiple occurrences were missed and
/// that is still in the past, the first occurrence after `now` (fire-once).
fn next_after_due(cron: &str, due_at: DateTime<Utc>, now: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let next = crate::cron_lite::next_run_after(cron, due_at)?;
    if next <= now {
        crate::cron_lite::next_run_after(cron, now)
    } else {
        Ok(next)
    }
}

/// One scheduler tick: claim every due deployment and create its run + work
/// task. Runs synchronously on the calling connection — the spawned loop
/// calls this via `spawn_blocking`. Returns the runs fired this tick.
pub fn run_tick(db: &DbHandle, now: DateTime<Utc>) -> Result<Vec<FiredRun>, rusqlite::Error> {
    let mut conn = db.connect()?;
    let tx = conn.transaction()?;
    let mut stmt = tx.prepare(
        "SELECT id, user_id, agent_id, cron, next_run_at FROM beta_deployments
         WHERE status = 'active' AND next_run_at IS NOT NULL AND next_run_at <= ?1
         ORDER BY next_run_at",
    )?;
    let due = stmt
        .query_map(params![now.to_rfc3339()], |row| {
            Ok(DueDeployment {
                id: row.get(0)?,
                user_id: row.get(1)?,
                agent_id: row.get(2)?,
                cron: row.get(3)?,
                next_run_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<DueDeployment>, _>>()?;
    drop(stmt);

    let mut fired = Vec::new();
    for deployment in due {
        let due_at = match DateTime::parse_from_rfc3339(&deployment.next_run_at) {
            Ok(parsed) => parsed.with_timezone(&Utc),
            Err(e) => {
                warn!(
                    deployment_id = %deployment.id,
                    next_run_at = %deployment.next_run_at,
                    "skipping deployment with unparseable next_run_at: {e}"
                );
                continue;
            }
        };
        let next = match next_after_due(&deployment.cron, due_at, now) {
            Ok(next) => next,
            Err(e) => {
                warn!(
                    deployment_id = %deployment.id,
                    cron = %deployment.cron,
                    "skipping deployment with unparseable cron: {e}"
                );
                continue;
            }
        };
        // Restart-safe claim: only advance if the row still holds the due
        // time we selected. 0 rows = another scheduler instance claimed it
        // (or a PATCH rescheduled it) — skip.
        let claimed = tx.execute(
            "UPDATE beta_deployments SET
                next_run_at = ?1, last_run_at = ?2, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?3 AND next_run_at = ?4",
            params![
                next.to_rfc3339(),
                now.to_rfc3339(),
                deployment.id,
                deployment.next_run_at
            ],
        )?;
        if claimed == 0 {
            info!(
                deployment_id = %deployment.id,
                "deployment run skipped: claimed by another scheduler instance"
            );
            continue;
        }
        let (run_id, task_id) = crate::beta_deployment_routes::insert_deployment_run_tx(
            &tx,
            &deployment.id,
            &deployment.user_id,
            deployment.agent_id.as_deref(),
            "scheduler",
        )?;
        fired.push(FiredRun {
            deployment_id: deployment.id,
            run_id,
            task_id,
            user_id: deployment.user_id,
            next_run_at: next,
        });
    }
    tx.commit()?;
    Ok(fired)
}

/// Spawn the scheduler loop, following the same interval + broadcast
/// shutdown pattern as the other background loops in `main`.
pub fn spawn_deployment_scheduler(
    state: Arc<AppState>,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let period = interval_from_env();
    info!("Starting deployment scheduler (interval {period:?})");
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                _ = interval.tick() => {
                    let db = state.db.clone();
                    match tokio::task::spawn_blocking(move || run_tick(&db, Utc::now())).await {
                        Ok(Ok(fired)) => {
                            state.deployment_scheduler.record_tick();
                            if !fired.is_empty() {
                                state.deployment_scheduler.record_fired(fired.len() as u64);
                                for run in &fired {
                                    info!(
                                        deployment_id = %run.deployment_id,
                                        run_id = %run.run_id,
                                        next_run_at = %run.next_run_at,
                                        "deployment scheduler fired run"
                                    );
                                }
                                // Fire-and-forget webhooks for the new runs.
                                // Org scope resolves from the deployment
                                // owner's users row; no org → no delivery.
                                let state = state.clone();
                                let fired = fired.clone();
                                tokio::spawn(async move {
                                    for run in fired {
                                        let db = state.db.clone();
                                        let user_id = run.user_id.clone();
                                        let org = tokio::task::spawn_blocking(move || {
                                            let conn = db.connect().ok()?;
                                            conn.query_row(
                                                "SELECT organization_id FROM users WHERE id = ?1",
                                                rusqlite::params![user_id],
                                                |row| row.get::<_, Option<String>>(0),
                                            )
                                            .ok()
                                            .flatten()
                                        })
                                        .await
                                        .ok()
                                        .flatten();
                                        crate::webhook_subscription_routes::deliver_registered_event(
                                            state.clone(),
                                            org.as_deref(),
                                            crate::webhook_subscription_routes::events::DEPLOYMENT_RUN_CREATED,
                                            serde_json::json!({
                                                "deployment_id": run.deployment_id,
                                                "run_id": run.run_id,
                                                "triggered_by": "scheduler",
                                            }),
                                        )
                                        .await;
                                    }
                                });
                            }
                        }
                        Ok(Err(e)) => warn!("deployment scheduler tick failed: {e}"),
                        Err(e) => warn!("deployment scheduler task panicked: {e}"),
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Timelike};

    fn db() -> DbHandle {
        DbHandle::new_memory().expect("test db")
    }

    fn insert_deployment(
        db: &DbHandle,
        id: &str,
        cron: &str,
        next_run_at: DateTime<Utc>,
        status: &str,
    ) {
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO beta_deployments (id, user_id, agent_id, cron, next_run_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                "user-a",
                format!("agent-{id}"),
                cron,
                next_run_at.to_rfc3339(),
                status
            ],
        )
        .unwrap();
    }

    fn count_rows(db: &DbHandle, table: &str) -> i64 {
        let conn = db.connect().unwrap();
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn parses_interval_with_default_and_clamps() {
        assert_eq!(parse_interval_secs(Some("30")), 30);
        assert_eq!(parse_interval_secs(None), DEFAULT_INTERVAL_SECS);
        assert_eq!(parse_interval_secs(Some("junk")), DEFAULT_INTERVAL_SECS);
        assert_eq!(parse_interval_secs(Some("0")), DEFAULT_INTERVAL_SECS);
        assert_eq!(parse_interval_secs(Some("99999")), DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn due_deployment_fires_run_and_enqueues_task() {
        let db = db();
        let now = Utc::now();
        insert_deployment(&db, "d1", "* * * * *", now - Duration::seconds(30), "active");

        let fired = run_tick(&db, now).unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].deployment_id, "d1");
        assert!(fired[0].next_run_at > now);

        let conn = db.connect().unwrap();
        let (status, triggered_by): (String, Option<String>) = conn
            .query_row(
                "SELECT status, triggered_by FROM beta_deployment_runs WHERE id = ?1",
                params![fired[0].run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "running");
        assert_eq!(triggered_by.as_deref(), Some("scheduler"));

        let (task_deployment, payload): (String, String) = conn
            .query_row(
                "SELECT deployment_id, payload FROM beta_work_tasks WHERE id = ?1",
                params![fired[0].task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(task_deployment, "d1");
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["deployment_run_id"], json!(fired[0].run_id));
        assert_eq!(payload["agent_id"], json!("agent-d1"));

        // next_run_at advanced to the newly computed future occurrence.
        let next_run_at: String = conn
            .query_row(
                "SELECT next_run_at FROM beta_deployments WHERE id = 'd1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(next_run_at, fired[0].next_run_at.to_rfc3339());
    }

    #[test]
    fn claim_race_fires_exactly_one_run() {
        let db = db();
        let now = Utc::now();
        insert_deployment(&db, "d1", "* * * * *", now - Duration::seconds(10), "active");

        // First tick claims and fires.
        let first = run_tick(&db, now).unwrap();
        assert_eq!(first.len(), 1);
        // A second tick for the same due window finds nothing to claim.
        // Anchor the re-tick to the claimed row's next_run_at (not wall
        // clock): with a once-per-minute cron, `now + 5s` can cross the next
        // minute boundary when the test starts in the last seconds of a
        // minute, firing a second run and flaking the test.
        let before_next = first[0].next_run_at - Duration::seconds(1);
        let second = run_tick(&db, before_next).unwrap();
        assert!(second.is_empty());
        assert_eq!(count_rows(&db, "beta_deployment_runs"), 1);
        assert_eq!(count_rows(&db, "beta_work_tasks"), 1);
    }

    #[test]
    fn claim_guard_loses_when_next_run_at_moves() {
        let db = db();
        let now = Utc::now();
        insert_deployment(&db, "d1", "* * * * *", now - Duration::seconds(10), "active");

        // Simulate another scheduler instance / PATCH advancing the row
        // between this tick's SELECT and its claim.
        let conn = db.connect().unwrap();
        conn.execute(
            "UPDATE beta_deployments SET next_run_at = ?1 WHERE id = 'd1'",
            params![(now + Duration::hours(1)).to_rfc3339()],
        )
        .unwrap();
        drop(conn);

        let fired = run_tick(&db, now).unwrap();
        assert!(fired.is_empty());
        assert_eq!(count_rows(&db, "beta_deployment_runs"), 0);
    }

    #[test]
    fn not_due_deployment_does_not_fire() {
        let db = db();
        let now = Utc::now();
        insert_deployment(&db, "d1", "* * * * *", now + Duration::minutes(5), "active");

        let fired = run_tick(&db, now).unwrap();
        assert!(fired.is_empty());
        assert_eq!(count_rows(&db, "beta_deployment_runs"), 0);
        assert_eq!(count_rows(&db, "beta_work_tasks"), 0);
    }

    #[test]
    fn overdue_catch_up_fires_once_not_a_burst() {
        let db = db();
        // Mid-hour, so the +20 s follow-up tick never crosses the next hourly
        // occurrence (at hh:59:50 it would, and correctly fire).
        let now = Utc::now().date_naive().and_hms_opt(12, 30, 0).unwrap().and_utc();
        // Three missed hourly occurrences while the server was down.
        insert_deployment(&db, "d1", "0 * * * *", now - Duration::hours(3), "active");

        let fired = run_tick(&db, now).unwrap();
        assert_eq!(fired.len(), 1, "catch-up fires exactly one run");
        assert!(fired[0].next_run_at > now, "next occurrence is in the future");
        // A follow-up tick does not fire the still-missed occurrences.
        let follow_up = run_tick(&db, now + Duration::seconds(20)).unwrap();
        assert!(follow_up.is_empty());
        assert_eq!(count_rows(&db, "beta_deployment_runs"), 1);
    }

    #[test]
    fn next_occurrence_computed_after_due_time_not_now() {
        let db = db();
        let now = Utc::now();
        // Anchor the due time to the top of the current minute so exactly one
        // occurrence was missed regardless of the wall-clock phase.
        let due = now - Duration::seconds(i64::from(now.second()));
        insert_deployment(&db, "d1", "* * * * *", due, "active");

        let fired = run_tick(&db, now).unwrap();
        assert_eq!(fired.len(), 1);
        // The next occurrence must be the minute after the DUE time —
        // computing it after `now` would push it a full minute later and let
        // per-tick latency drift the schedule.
        let expected = crate::cron_lite::next_run_after("* * * * *", due).unwrap();
        assert_eq!(fired[0].next_run_at, expected);
        assert!(fired[0].next_run_at > now);
        assert!(
            fired[0].next_run_at <= now + Duration::seconds(75),
            "next stays anchored to the due time, not the tick time"
        );
    }

    #[test]
    fn paused_and_archived_deployments_never_fire() {
        let db = db();
        let now = Utc::now();
        insert_deployment(&db, "paused", "* * * * *", now - Duration::minutes(2), "paused");
        insert_deployment(&db, "archived", "* * * * *", now - Duration::minutes(2), "archived");

        let fired = run_tick(&db, now).unwrap();
        assert!(fired.is_empty());
        assert_eq!(count_rows(&db, "beta_deployment_runs"), 0);
    }
}
