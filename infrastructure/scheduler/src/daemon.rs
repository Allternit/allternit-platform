//! Scheduler Daemon
//!
//! Main daemon that polls schedules and triggers runs.
//!
//! Every tick atomically *claims* the due rows it is going to fire
//! (`UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED) RETURNING …`,
//! stamping `claimed_by` / `claimed_at`), so any number of daemons — and the
//! in-process scheduler in `allternit-cloud-api`, which uses the same claim —
//! can poll one database and each due occurrence fires exactly once. A claim
//! is released when the row's `next_run_at` is advanced; a claim left behind
//! by a daemon that died mid-fire expires after `claim_ttl_secs`.
//!
//! `next_run_at` is computed with the cron fields read as wall-clock time in
//! the schedule's own `timezone` (chrono-tz), so `0 9 * * *` in
//! `America/New_York` fires at 13:00 UTC in summer and 14:00 UTC in winter.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Pool, Postgres};
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::misfire::{handle_misfire, MisfireAction};
use crate::scheduler::Schedule;
use crate::SchedulerConfig;

/// Columns selected/returned for a `Schedule`, normalised to the Rust types:
/// `misfire_policy` is the Postgres enum `misfirepolicy` (read as text), the
/// counters are `bigint`, and the nullable text/bool columns get defaults so a
/// sparse row never fails to decode.
pub(crate) const SCHEDULE_COLUMNS: &str = "id, \
     COALESCE(name, '') AS name, \
     description, \
     COALESCE(cron_expr, '') AS cron_expr, \
     natural_lang, \
     COALESCE(timezone, 'UTC') AS timezone, \
     COALESCE(job_template, 'null'::json) AS job_template, \
     COALESCE(enabled, FALSE) AS enabled, \
     COALESCE(misfire_policy::text, 'fire_once') AS misfire_policy, \
     last_run_at, \
     next_run_at, \
     COALESCE(run_count, 0)::bigint AS run_count, \
     COALESCE(misfire_count, 0)::bigint AS misfire_count, \
     owner_id, \
     tenant_id, \
     COALESCE(created_at, now()) AS created_at, \
     COALESCE(updated_at, now()) AS updated_at";

/// Atomically claim up to `$3` due schedules for daemon `$1` at time `$2`.
///
/// `$4` is the stale-claim cutoff: a row claimed before it (by a daemon that
/// died mid-fire) may be re-claimed. `FOR UPDATE SKIP LOCKED` makes two
/// concurrent claimers split the due set instead of both taking it.
pub(crate) fn claim_due_sql() -> String {
    format!(
        "UPDATE schedules \
         SET claimed_by = $1, claimed_at = $2 \
         WHERE id IN ( \
             SELECT id FROM schedules \
             WHERE enabled = TRUE \
               AND next_run_at IS NOT NULL \
               AND next_run_at <= $2 \
               AND (claimed_by IS NULL OR claimed_at IS NULL OR claimed_at < $4) \
             ORDER BY next_run_at ASC \
             LIMIT $3 \
             FOR UPDATE SKIP LOCKED \
         ) \
         RETURNING {SCHEDULE_COLUMNS}"
    )
}

/// Release a claim after a successful fire: advance the schedule and bump the
/// counters. Guarded by `claimed_by` so a daemon whose claim already expired
/// and was taken over cannot overwrite the new holder's state.
pub(crate) const RELEASE_AFTER_RUN_SQL: &str = "UPDATE schedules \
     SET last_run_at = $1, next_run_at = $2, run_count = COALESCE(run_count, 0) + 1, \
         misfire_count = COALESCE(misfire_count, 0) + $3, updated_at = $4, \
         claimed_by = NULL, claimed_at = NULL \
     WHERE id = $5 AND claimed_by = $6";

/// Release a claim without a successful fire (trigger failed, misfire
/// ignored, or the next run could not be computed).
pub(crate) const RELEASE_WITHOUT_RUN_SQL: &str = "UPDATE schedules \
     SET next_run_at = $1, misfire_count = COALESCE(misfire_count, 0) + $2, updated_at = $3, \
         claimed_by = NULL, claimed_at = NULL \
     WHERE id = $4 AND claimed_by = $5";

pub(crate) const INSERT_TRIGGER_SQL: &str = "INSERT INTO trigger_history \
     (id, schedule_id, run_id, triggered_at, success, error_message, metadata) \
     VALUES ($1, $2, $3, $4, $5, $6, $7)";

/// Next fire time for `schedule` strictly after `after`, with the cron fields
/// evaluated in the schedule's timezone. An unknown timezone falls back to UTC
/// (logged) — the same fallback the cloud-api schedule routes use when a row
/// is created, so both sides agree on the instant.
pub(crate) fn next_run_after(schedule: &Schedule, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>> {
    let expr = allternit_cron_parser::parse(&schedule.cron_expr)
        .map(|p| p.expression)
        .unwrap_or_else(|_| schedule.cron_expr.clone());
    match allternit_cron_parser::next_occurrence_in_tz(&expr, &schedule.timezone, after) {
        Ok(next) => Ok(next),
        Err(allternit_cron_parser::ParseError::InvalidFormat(msg)) if msg.starts_with("unknown timezone") => {
            warn!(schedule = %schedule.id, timezone = %schedule.timezone, "unknown schedule timezone; evaluating cron in UTC");
            allternit_cron_parser::next_occurrence_in_tz(&expr, "UTC", after)
                .map_err(|e| anyhow::anyhow!("Failed to calculate next occurrence: {e}"))
        }
        Err(e) => Err(anyhow::anyhow!("Failed to calculate next occurrence: {e}")),
    }
}

/// Stable-per-process identity written to `claimed_by`.
fn instance_id() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "host".to_string());
    format!("{host}:{}:{}", std::process::id(), &Uuid::new_v4().simple().to_string()[..8])
}

/// Scheduler daemon
pub struct SchedulerDaemon {
    config: SchedulerConfig,
    db: Pool<Postgres>,
    /// HTTP client for API calls
    http_client: reqwest::Client,
    /// Identity written to `schedules.claimed_by`.
    instance_id: String,
}

impl SchedulerDaemon {
    /// Create a new scheduler daemon
    pub async fn new(config: SchedulerConfig) -> Result<Self> {
        info!("Connecting to database");

        let db = PgPool::connect(&config.database_url)
            .await
            .context("Failed to connect to database")?;

        Self::with_pool(config, db)
    }

    /// Create a daemon over an existing pool.
    pub fn with_pool(config: SchedulerConfig, db: PgPool) -> Result<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        let instance_id = instance_id();
        info!(instance = %instance_id, "scheduler instance id");

        Ok(Self {
            config,
            db,
            http_client,
            instance_id,
        })
    }

    /// Run the daemon forever
    pub async fn run(&self) -> Result<()> {
        let mut poll_interval = interval(Duration::from_secs(self.config.poll_interval_secs));

        info!("Scheduler daemon started");

        loop {
            poll_interval.tick().await;

            if let Err(e) = self.tick().await {
                error!("Error during scheduler tick: {}", e);
            }
        }
    }

    /// Run a single tick (for testing)
    pub async fn run_once(&self) -> Result<()> {
        self.tick().await
    }

    /// Single tick - claim due schedules and fire each exactly once.
    async fn tick(&self) -> Result<()> {
        let now = Utc::now();
        debug!("Scheduler tick at {}", now);

        let claimed = self.claim_due_schedules(now).await?;

        if !claimed.is_empty() {
            info!("Claimed {} due schedules", claimed.len());
        }

        let misfire_cutoff = now - chrono::Duration::seconds(self.config.misfire_threshold_secs);
        for schedule in claimed {
            let result = match schedule.next_run_at {
                Some(due) if due < misfire_cutoff => self.process_misfire(&schedule).await,
                _ => self.process_schedule(&schedule, 0).await,
            };
            if let Err(e) = result {
                error!("Failed to process schedule {}: {}", schedule.id, e);
            }
        }

        Ok(())
    }

    /// Atomically claim the schedules that are due at `now`.
    async fn claim_due_schedules(&self, now: DateTime<Utc>) -> Result<Vec<Schedule>> {
        let stale_before = now - chrono::Duration::seconds(self.config.claim_ttl_secs);
        let schedules = sqlx::query_as::<_, Schedule>(&claim_due_sql())
            .bind(&self.instance_id)
            .bind(now)
            .bind(self.config.max_schedules_per_tick as i64)
            .bind(stale_before)
            .fetch_all(&self.db)
            .await
            .context("Failed to claim due schedules")?;

        Ok(schedules)
    }

    /// Fire a claimed schedule once and release the claim.
    /// `misfired` (0 or 1) is added to `misfire_count` in the same update.
    async fn process_schedule(&self, schedule: &Schedule, misfired: i64) -> Result<()> {
        info!("Processing schedule: {} ({})", schedule.id, schedule.name);

        let now = Utc::now();

        let next_run = match next_run_after(schedule, now) {
            Ok(next) => next,
            Err(e) => {
                // Unparseable cron: stop firing (next_run_at = NULL) rather
                // than re-claiming the row every tick.
                error!("Schedule {} has an invalid cron expression; disabling future runs: {}", schedule.id, e);
                self.release_without_run(schedule, None, misfired).await?;
                return Ok(());
            }
        };

        match self.trigger_run(schedule).await {
            Ok(run_id) => {
                info!("Triggered run {} for schedule {}", run_id, schedule.id);
                self.release_after_run(schedule, now, next_run, misfired).await?;
                self.record_trigger(schedule, Some(&run_id), None).await;
            }
            Err(e) => {
                error!("Failed to trigger run for schedule {}: {}", schedule.id, e);
                // Still advance next_run_at to prevent infinite retries.
                self.release_without_run(schedule, next_run, misfired).await?;
                self.record_trigger(schedule, None, Some(&e.to_string())).await;
            }
        }

        Ok(())
    }

    /// A claimed schedule whose due time is older than the misfire threshold.
    async fn process_misfire(&self, schedule: &Schedule) -> Result<()> {
        warn!("Misfired schedule: {} ({})", schedule.id, schedule.name);

        match handle_misfire(schedule, self.config.misfire_policy) {
            MisfireAction::Ignore => {
                let next_run = next_run_after(schedule, Utc::now()).unwrap_or(None);
                self.release_without_run(schedule, next_run, 1).await
            }
            MisfireAction::FireOnce => self.process_schedule(schedule, 1).await,
            MisfireAction::FireAll { count } => {
                // Catch-up of every missed occurrence is not implemented; one
                // run is fired and the gap is logged.
                warn!("Schedule {} missed ~{} runs; firing once", schedule.id, count);
                self.process_schedule(schedule, 1).await
            }
        }
    }

    /// Trigger a run for a schedule
    async fn trigger_run(&self, schedule: &Schedule) -> Result<String> {
        match self.config.execution_mode {
            crate::ExecutionMode::Api => self.trigger_run_api(schedule).await,
            crate::ExecutionMode::Local => self.trigger_run_local(schedule).await,
        }
    }

    /// Trigger a run via the control plane API (cloud mode).
    async fn trigger_run_api(&self, schedule: &Schedule) -> Result<String> {
        let run_id = uuid::Uuid::new_v4().to_string();

        // Build API request to create run
        let url = format!("{}/api/v1/runs", self.config.api_url);

        let body = serde_json::json!({
            "name": format!("Scheduled: {}", schedule.name),
            "description": schedule.description,
            "mode": "remote",
            "config": schedule.job_template,
            "auto_start": true,
            "schedule_id": schedule.id,
        });

        let mut request = self.http_client.post(&url).json(&body);

        // Add API key if configured
        if let Some(api_key) = &self.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .context("Failed to send trigger request")?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("API error: {}", error_text));
        }

        let result: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse API response")?;

        let created_run_id = result["id"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or(run_id);

        Ok(created_run_id)
    }

    /// Execute the scheduled job directly on this machine (local mode).
    async fn trigger_run_local(&self, schedule: &Schedule) -> Result<String> {
        let run_id = uuid::Uuid::new_v4().to_string();

        #[derive(Debug, serde::Deserialize)]
        struct LocalJobConfig {
            command: String,
            #[serde(default)]
            args: Vec<String>,
            #[serde(default)]
            env: std::collections::HashMap<String, String>,
            working_dir: Option<String>,
            timeout_seconds: Option<u64>,
        }

        let job: LocalJobConfig = serde_json::from_value(schedule.job_template.0.clone())
            .context("Failed to parse job_template as local job config")?;

        info!(
            "[local] Executing schedule {}: {} {:?}",
            schedule.id, job.command, job.args
        );

        let mut cmd = tokio::process::Command::new(&job.command);
        cmd.args(&job.args);

        for (key, value) in &job.env {
            cmd.env(key, value);
        }

        if let Some(dir) = &job.working_dir {
            cmd.current_dir(dir);
        }

        let timeout = job.timeout_seconds.unwrap_or(300);

        let status = tokio::time::timeout(Duration::from_secs(timeout), cmd.status())
            .await
            .context("Local job timed out")?
            .context("Failed to spawn local job")?;

        if !status.success() {
            return Err(anyhow::anyhow!(
                "Local job exited with code {:?}",
                status.code()
            ));
        }

        Ok(run_id)
    }

    async fn release_after_run(
        &self,
        schedule: &Schedule,
        last_run: DateTime<Utc>,
        next_run: Option<DateTime<Utc>>,
        misfired: i64,
    ) -> Result<()> {
        let updated = sqlx::query(RELEASE_AFTER_RUN_SQL)
            .bind(last_run)
            .bind(next_run)
            .bind(misfired)
            .bind(Utc::now())
            .bind(&schedule.id)
            .bind(&self.instance_id)
            .execute(&self.db)
            .await
            .context("Failed to update schedule")?;
        if updated.rows_affected() == 0 {
            warn!("Schedule {} claim was lost before release (claim TTL too short?)", schedule.id);
        }
        Ok(())
    }

    async fn release_without_run(
        &self,
        schedule: &Schedule,
        next_run: Option<DateTime<Utc>>,
        misfired: i64,
    ) -> Result<()> {
        sqlx::query(RELEASE_WITHOUT_RUN_SQL)
            .bind(next_run)
            .bind(misfired)
            .bind(Utc::now())
            .bind(&schedule.id)
            .bind(&self.instance_id)
            .execute(&self.db)
            .await
            .context("Failed to update schedule next_run")?;
        Ok(())
    }

    /// Record a schedule trigger for auditing (best effort).
    async fn record_trigger(&self, schedule: &Schedule, run_id: Option<&str>, error: Option<&str>) {
        let success = error.is_none();
        let result = sqlx::query(INSERT_TRIGGER_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(&schedule.id)
            .bind(run_id)
            .bind(Utc::now())
            .bind(success)
            .bind(error)
            .bind(Some(format!("{{\"claimed_by\":\"{}\"}}", self.instance_id)))
            .execute(&self.db)
            .await;

        match result {
            Ok(_) if success => debug!("Recorded trigger: schedule={}, run_id={:?}", schedule.id, run_id),
            Ok(_) => warn!("Recorded failed trigger: schedule={}", schedule.id),
            Err(e) => warn!("Failed to write trigger_history for schedule {}: {}", schedule.id, e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn schedule(cron: &str, tz: &str) -> Schedule {
        Schedule {
            id: "s1".into(),
            name: "t".into(),
            description: None,
            cron_expr: cron.into(),
            natural_lang: None,
            timezone: tz.into(),
            job_template: sqlx::types::Json(serde_json::json!({})),
            enabled: true,
            misfire_policy: "fire_once".into(),
            last_run_at: None,
            next_run_at: None,
            run_count: 0,
            misfire_count: 0,
            owner_id: None,
            tenant_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Every statement this crate sends (all of them live in the constants
    /// above) uses Postgres `$n` placeholders; a MySQL/SQLite `?` placeholder
    /// is rejected by Postgres at parse time.
    #[test]
    fn sql_uses_postgres_placeholders_only() {
        for sql in [
            claim_due_sql(),
            RELEASE_AFTER_RUN_SQL.to_string(),
            RELEASE_WITHOUT_RUN_SQL.to_string(),
            INSERT_TRIGGER_SQL.to_string(),
        ] {
            assert!(!sql.contains('?'), "`?` placeholder in: {sql}");
            assert!(sql.contains("$1"), "no $1 in: {sql}");
        }
    }

    #[test]
    fn claim_sql_is_atomic_and_skip_locked() {
        let sql = claim_due_sql();
        assert!(sql.starts_with("UPDATE schedules"));
        assert!(sql.contains("SET claimed_by = $1, claimed_at = $2"));
        assert!(sql.contains("FOR UPDATE SKIP LOCKED"));
        assert!(sql.contains("next_run_at <= $2"));
        assert!(sql.contains("LIMIT $3"));
        assert!(sql.contains("claimed_at < $4"));
        assert!(sql.contains("RETURNING id,"));
        // The enum column is read as text so it decodes into String.
        assert!(sql.contains("misfire_policy::text"));
        // Releases only touch rows this daemon still holds.
        assert!(RELEASE_AFTER_RUN_SQL.contains("claimed_by = $6"));
        assert!(RELEASE_WITHOUT_RUN_SQL.contains("claimed_by = $5"));
    }

    #[test]
    fn next_run_honors_schedule_timezone_across_dst() {
        let s = schedule("0 9 * * *", "America/New_York");
        let summer = Utc.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap();
        assert_eq!(
            next_run_after(&s, summer).unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 6, 15, 13, 0, 0).unwrap())
        );
        let winter = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
        assert_eq!(
            next_run_after(&s, winter).unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap())
        );
    }

    #[test]
    fn next_run_accepts_natural_language_and_falls_back_to_utc() {
        let at = Utc.with_ymd_and_hms(2026, 6, 15, 0, 0, 0).unwrap();
        let s = schedule("daily at 9am", "Not/AZone");
        assert_eq!(
            next_run_after(&s, at).unwrap(),
            Some(Utc.with_ymd_and_hms(2026, 6, 15, 9, 0, 0).unwrap())
        );
        assert!(next_run_after(&schedule("garbage", "UTC"), at).is_err());
    }
}

#[cfg(test)]
#[path = "daemon_pg_tests.rs"]
mod pg_tests;
