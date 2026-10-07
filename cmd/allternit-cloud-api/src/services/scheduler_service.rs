//! Scheduler Service
//!
//! Background service that runs the job scheduler within the API server.
//! Polls schedules from the database and triggers runs when due.
//! Now with multi-region awareness for intelligent run placement.

use crate::db::cowork_models::Schedule;
use crate::db::models::Region;
use crate::ApiState;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Scheduler configuration
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
    /// Max schedules to process per tick
    pub max_schedules_per_tick: i64,
    /// Misfire threshold in seconds
    pub misfire_threshold_secs: i64,
    /// Misfire policy: ignore, fire_once, fire_all
    pub misfire_policy: MisfirePolicy,
    /// Default region for runs without explicit region preference
    pub default_region: Option<String>,
    /// Whether multi-region scheduling is enabled
    pub multi_region_enabled: bool,
    /// User location for proximity-based selection (latitude, longitude)
    pub user_location: Option<(f64, f64)>,
    /// Cost optimization mode
    pub cost_optimization: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 60,
            max_schedules_per_tick: 100,
            misfire_threshold_secs: 300, // 5 minutes
            misfire_policy: MisfirePolicy::FireOnce,
            default_region: std::env::var("DEFAULT_REGION").ok(),
            multi_region_enabled: std::env::var("MULTI_REGION_ENABLED")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true),
            user_location: None,
            cost_optimization: std::env::var("COST_OPTIMIZATION")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(false),
        }
    }
}

impl SchedulerConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            poll_interval_secs: std::env::var("SCHEDULER_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            max_schedules_per_tick: std::env::var("SCHEDULER_MAX_PER_TICK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            misfire_threshold_secs: std::env::var("SCHEDULER_MISFIRE_THRESHOLD_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            misfire_policy: std::env::var("SCHEDULER_MISFIRE_POLICY")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(MisfirePolicy::FireOnce),
            default_region: std::env::var("DEFAULT_REGION").ok(),
            multi_region_enabled: std::env::var("MULTI_REGION_ENABLED")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(true),
            user_location: parse_location_env(),
            cost_optimization: std::env::var("COST_OPTIMIZATION")
                .ok()
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(false),
        }
    }
}

/// Parse USER_LOCATION env var (format: "lat,lon")
fn parse_location_env() -> Option<(f64, f64)> {
    std::env::var("USER_LOCATION").ok().and_then(|s| {
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() == 2 {
            let lat = parts[0].trim().parse::<f64>().ok()?;
            let lon = parts[1].trim().parse::<f64>().ok()?;
            Some((lat, lon))
        } else {
            None
        }
    })
}

/// Misfire policy for missed schedules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MisfirePolicy {
    Ignore,
    FireOnce,
    FireAll,
}

impl std::str::FromStr for MisfirePolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ignore" => Ok(MisfirePolicy::Ignore),
            "fire_once" => Ok(MisfirePolicy::FireOnce),
            "fire_all" => Ok(MisfirePolicy::FireAll),
            _ => Err(format!("Unknown misfire policy: {}", s)),
        }
    }
}

/// Misfire action
enum MisfireAction {
    Ignore,
    FireOnce,
    FireAll,
}

/// Scheduler service that runs as a background task
pub struct SchedulerService {
    config: SchedulerConfig,
    /// Identity written to `schedules.claimed_by` (see `claim_due_sql`).
    instance_id: String,
}

/// How long a claim on a due schedule is honoured before another poller may
/// take it over (covers a process that died mid-fire). Firing here is a DB
/// insert, so this only has to outlast one tick's worth of inserts.
const CLAIM_TTL_SECS: i64 = 600;

/// Columns of `cowork_models::Schedule`.
const SCHEDULE_COLUMNS: &str = "id, name, description, cron_expr, natural_lang, timezone, \
     job_template, enabled, misfire_policy, last_run_at, next_run_at, run_count, misfire_count, \
     owner_id, tenant_id, region_id, created_at, updated_at";

/// Atomically claim up to `$3` schedules due at `$2` for poller `$1`; `$4` is
/// the stale-claim cutoff. Same claim as the standalone `allternit-scheduler`
/// daemon (infrastructure/scheduler/src/daemon.rs), so any mix of pollers on
/// one database fires each due occurrence once. Migration 018 adds the columns.
fn claim_due_sql() -> String {
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

/// Region selection criteria
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionSelectionCriteria {
    /// Select region with most available capacity
    Capacity,
    /// Select region closest to user
    Proximity,
    /// Select cheapest region
    Cost,
    /// Balanced approach (capacity + proximity)
    Balanced,
}

impl Default for RegionSelectionCriteria {
    fn default() -> Self {
        RegionSelectionCriteria::Balanced
    }
}

impl SchedulerService {
    /// Create a new scheduler service
    pub fn new(config: SchedulerConfig) -> Self {
        let host = std::env::var("HOSTNAME")
            .ok()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "cloud-api".to_string());
        let instance_id = format!(
            "{host}:{}:{}",
            std::process::id(),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        Self {
            config,
            instance_id,
        }
    }

    /// Start the scheduler background task
    pub fn start(self, state: Arc<ApiState>) {
        tokio::spawn(async move {
            let mut poll_interval = interval(Duration::from_secs(self.config.poll_interval_secs));

            info!(
                "Scheduler service started (poll_interval: {}s, multi_region: {})",
                self.config.poll_interval_secs, self.config.multi_region_enabled
            );

            loop {
                poll_interval.tick().await;

                if let Err(e) = self.tick(&state).await {
                    error!("Error during scheduler tick: {}", e);
                }
            }
        });
    }

    /// Run one poll tick: claim due schedules and fire them.
    pub async fn run_once(&self, state: &Arc<ApiState>) -> anyhow::Result<()> {
        self.tick(state).await
    }

    /// Single tick - check schedules and trigger due runs
    async fn tick(&self, state: &Arc<ApiState>) -> anyhow::Result<()> {
        let now = Utc::now();
        debug!("Scheduler tick at {}", now);

        // Claim the enabled schedules that are due. A claimed row is ours
        // alone until it is released, so a second API replica (or the
        // standalone daemon) polling the same database never fires it too.
        let due_schedules = self.claim_due_schedules(state, now).await?;

        if !due_schedules.is_empty() {
            info!("Claimed {} due schedules", due_schedules.len());
        }

        // Misfires are claimed rows whose due time is past the threshold; they
        // go through the misfire policy instead of a plain fire. (Previously a
        // separate unclaimed misfire query could fire the same row again.)
        let misfire_cutoff = now - chrono::Duration::seconds(self.config.misfire_threshold_secs);
        for schedule in due_schedules {
            let result = match schedule.next_run_at {
                Some(due) if due < misfire_cutoff => self.process_misfire(state, &schedule).await,
                _ => self.process_schedule(state, &schedule, 0).await,
            };
            if let Err(e) = result {
                error!("Failed to process schedule {}: {}", schedule.id, e);
            }
        }

        Ok(())
    }

    /// Atomically claim the schedules due at `now`.
    async fn claim_due_schedules(
        &self,
        state: &Arc<ApiState>,
        now: chrono::DateTime<Utc>,
    ) -> anyhow::Result<Vec<Schedule>> {
        let stale_before = now - chrono::Duration::seconds(CLAIM_TTL_SECS);
        let schedules = sqlx::query_as::<_, Schedule>(&claim_due_sql())
            .bind(&self.instance_id)
            .bind(now)
            .bind(self.config.max_schedules_per_tick)
            .bind(stale_before)
            .fetch_all(&state.db)
            .await?;

        Ok(schedules)
    }

    /// Fire a claimed schedule once and release the claim. `misfired` (0 or
    /// 1) is added to `misfire_count` in the same update. Every path releases
    /// the claim, so an error never leaves the row stuck until the TTL.
    async fn process_schedule(
        &self,
        state: &Arc<ApiState>,
        schedule: &Schedule,
        misfired: i64,
    ) -> anyhow::Result<()> {
        info!("Processing schedule: {} ({})", schedule.id, schedule.name);

        let now = Utc::now();

        // Calculate next run time
        let next_run = match self.calculate_next_run(schedule).await {
            Ok(next) => next,
            Err(e) => {
                // Unparseable cron: stop firing rather than re-claiming the
                // row every tick.
                error!("Schedule {} has an invalid cron expression; disabling future runs: {}", schedule.id, e);
                return self.update_schedule_next_run(state, schedule, None, misfired).await;
            }
        };

        // Determine region for this run
        let region_id = if self.config.multi_region_enabled {
            self.select_region_for_schedule(state, schedule).await
        } else {
            Ok(self.config.default_region.clone())
        };

        if let Ok(Some(ref r)) = region_id {
            debug!("Selected region '{}' for schedule {}", r, schedule.id);
        }

        // Trigger the run directly in the database
        let trigger_result = match region_id {
            Ok(region_id) => self.trigger_run(state, schedule, region_id).await,
            Err(e) => Err(e),
        };

        match trigger_result {
            Ok(run_id) => {
                info!("Triggered run {} for schedule {}", run_id, schedule.id);

                // Update schedule status
                self.update_schedule_after_run(state, schedule, now, next_run, misfired)
                    .await?;
            }
            Err(e) => {
                error!("Failed to trigger run for schedule {}: {}", schedule.id, e);

                // Still update next_run_at to prevent infinite retries
                self.update_schedule_next_run(state, schedule, next_run, misfired)
                    .await?;
            }
        }

        Ok(())
    }

    /// A claimed schedule whose due time is older than the misfire threshold.
    async fn process_misfire(&self, state: &Arc<ApiState>, schedule: &Schedule) -> anyhow::Result<()> {
        warn!("Misfired schedule: {} ({})", schedule.id, schedule.name);

        match self.handle_misfire(schedule) {
            MisfireAction::Ignore => {
                let next_run = self.calculate_next_run(schedule).await.unwrap_or(None);
                self.update_schedule_next_run(state, schedule, next_run, 1).await
            }
            MisfireAction::FireOnce | MisfireAction::FireAll => {
                // FireAll catch-up of every missed occurrence is not
                // implemented; one run is fired.
                self.process_schedule(state, schedule, 1).await
            }
        }
    }

    /// Select appropriate region for a schedule
    async fn select_region_for_schedule(
        &self,
        state: &Arc<ApiState>,
        schedule: &Schedule,
    ) -> anyhow::Result<Option<String>> {
        // If schedule has explicit region preference, use it (if active)
        if let Some(ref region_id) = schedule.region_id {
            let is_active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM regions WHERE id = $1 AND active = TRUE)",
            )
            .bind(region_id)
            .fetch_one(&state.db)
            .await?;

            if is_active {
                return Ok(Some(region_id.clone()));
            } else {
                warn!(
                    "Schedule {} preferred region '{}' is inactive, selecting alternative",
                    schedule.id, region_id
                );
            }
        }

        // Otherwise, auto-select based on criteria
        self.select_best_region(state).await
    }

    /// Select the best region based on capacity, proximity, and cost
    async fn select_best_region(&self, state: &Arc<ApiState>) -> anyhow::Result<Option<String>> {
        // Get all active regions with capacity info
        let regions = sqlx::query_as::<_, Region>(
            r#"
            SELECT 
                r.id, r.name, r.provider, r.endpoint, r.capacity, r.active,
                r.cost_factor, r.location_lat, r.location_lon, r.metadata,
                r.created_at, r.updated_at
            FROM regions r
            WHERE r.active = TRUE
            "#,
        )
        .fetch_all(&state.db)
        .await?;

        if regions.is_empty() {
            warn!("No active regions available for scheduling");
            return Ok(self.config.default_region.clone());
        }

        // Get capacity info for each region
        let mut candidates = Vec::new();
        for region in &regions {
            let capacity_info: Option<(i32, i32)> = sqlx::query_as(
                "SELECT COALESCE(current_runs, 0), COALESCE(queued_runs, 0) FROM region_capacity WHERE region_id = $1"
            )
            .bind(&region.id)
            .fetch_optional(&state.db)
            .await?;

            let (current, queued) = capacity_info.unwrap_or((0, 0));
            let available = region.capacity - current - queued;

            if available > 0 {
                candidates.push((region.clone(), available));
            }
        }

        if candidates.is_empty() {
            warn!("All regions at capacity, using default region");
            return Ok(self.config.default_region.clone());
        }

        // Select based on criteria
        let selected = if self.config.cost_optimization {
            // Select cheapest region with capacity
            candidates
                .into_iter()
                .min_by(|a, b| {
                    a.0.cost_factor
                        .partial_cmp(&b.0.cost_factor)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(r, _)| r.id)
        } else if let Some((user_lat, user_lon)) = self.config.user_location {
            // Select closest region with capacity
            let closest = candidates
                .iter()
                .filter_map(|(r, available)| {
                    r.location_lat.and_then(|lat| {
                        r.location_lon.map(|lon| {
                            let dist = haversine_distance(user_lat, user_lon, lat, lon);
                            (r.clone(), *available, dist)
                        })
                    })
                })
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(r, _, _)| r.id);

            closest.or_else(|| {
                // Fallback to capacity if no location data
                candidates
                    .into_iter()
                    .max_by_key(|(_, available)| *available)
                    .map(|(r, _)| r.id)
            })
        } else {
            // Select region with most available capacity
            candidates
                .into_iter()
                .max_by_key(|(_, available)| *available)
                .map(|(r, _)| r.id)
        };

        Ok(selected.or_else(|| self.config.default_region.clone()))
    }

    /// Calculate next run time for a schedule, with the cron fields read as
    /// wall-clock time in the schedule's timezone. Shares the schedule
    /// routes' parser so classic 5-field cron (`0 9 * * *`) works here too:
    /// `cron::Schedule` alone requires a seconds field and rejected it.
    async fn calculate_next_run(
        &self,
        schedule: &Schedule,
    ) -> anyhow::Result<Option<chrono::DateTime<Utc>>> {
        crate::routes::schedules::calculate_next_run(&schedule.cron_expr, &schedule.timezone)
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("Invalid cron expression '{}'", schedule.cron_expr))
    }
    /// Trigger a run for a schedule - creates run directly in database
    async fn trigger_run(
        &self,
        state: &Arc<ApiState>,
        schedule: &Schedule,
        region_id: Option<String>,
    ) -> anyhow::Result<String> {
        let run_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        // Create run directly in database instead of via HTTP API
        sqlx::query(
            r#"
            INSERT INTO runs (
                id, name, description, mode, status, step_cursor, total_steps, completed_steps,
                config, owner_id, tenant_id, runtime_id, runtime_type, schedule_id, region_id,
                created_at, updated_at, started_at, completed_at, error_message, error_details
            ) VALUES ($1, $2, $3, $4::runmode, $5::runstatus, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21::json)
            "#,
        )
        .bind(&run_id)
        .bind(format!("Scheduled: {}", schedule.name))
        .bind(&schedule.description)
        .bind("remote")
        .bind("queued") // Start as queued, will be picked up by runtime
        .bind(None::<String>)
        .bind(None::<i32>)
        .bind(0i32)
        .bind(&schedule.job_template)
        .bind(&schedule.owner_id)
        .bind(&schedule.tenant_id)
        .bind(None::<String>) // runtime_id - will be assigned by orchestrator
        .bind(None::<String>) // runtime_type
        .bind(&schedule.id)
        .bind(&region_id) // region_id for multi-region scheduling
        .bind(now)
        .bind(now)
        .bind(None::<chrono::DateTime<Utc>>)
        .bind(None::<chrono::DateTime<Utc>>)
        .bind(None::<String>)
        .bind(None::<String>)
        .execute(&state.db)
        .await?;

        // Initialize cost tracking for the run
        // Use defaults - in a full implementation these would come from
        // schedule configuration or target pool settings
        let provider = "hetzner";
        let region = region_id.as_deref().unwrap_or("fsn1");
        let instance_type = "cx11";

        let _ = crate::services::init_run_cost_tracking(
            &state.db,
            &run_id,
            provider,
            region,
            instance_type,
        )
        .await;

        // Emit run created event
        let _ = crate::services::EventStore::append(
            state.event_store.as_ref(),
            &run_id,
            crate::db::cowork_models::EventType::RunCreated,
            serde_json::json!({
                "schedule_id": schedule.id,
                "schedule_name": schedule.name,
                "triggered_at": now,
                "region_id": region_id,
            }),
        )
        .await;

        Ok(run_id)
    }

    /// Update schedule after successful run
    async fn update_schedule_after_run(
        &self,
        state: &Arc<ApiState>,
        schedule: &Schedule,
        last_run: chrono::DateTime<Utc>,
        next_run: Option<chrono::DateTime<Utc>>,
        misfired: i64,
    ) -> anyhow::Result<()> {
        // Releases the claim; guarded by claimed_by so a poller whose claim
        // expired and was taken over cannot overwrite the new holder.
        sqlx::query(
            r#"
            UPDATE schedules
            SET last_run_at = $1, next_run_at = $2, run_count = COALESCE(run_count, 0) + 1,
                misfire_count = COALESCE(misfire_count, 0) + $3, updated_at = $4,
                claimed_by = NULL, claimed_at = NULL
            WHERE id = $5 AND claimed_by = $6
            "#,
        )
        .bind(last_run)
        .bind(next_run)
        .bind(misfired)
        .bind(Utc::now())
        .bind(&schedule.id)
        .bind(&self.instance_id)
        .execute(&state.db)
        .await?;

        Ok(())
    }

    /// Update just the next_run_at field
    async fn update_schedule_next_run(
        &self,
        state: &Arc<ApiState>,
        schedule: &Schedule,
        next_run: Option<chrono::DateTime<Utc>>,
        misfired: i64,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE schedules SET next_run_at = $1, misfire_count = COALESCE(misfire_count, 0) + $2, \
             updated_at = $3, claimed_by = NULL, claimed_at = NULL \
             WHERE id = $4 AND claimed_by = $5",
        )
        .bind(next_run)
        .bind(misfired)
        .bind(Utc::now())
        .bind(&schedule.id)
        .bind(&self.instance_id)
        .execute(&state.db)
        .await?;

        Ok(())
    }

    /// Handle misfire based on policy
    fn handle_misfire(&self, _schedule: &Schedule) -> MisfireAction {
        // Use config misfire policy
        match self.config.misfire_policy {
            MisfirePolicy::Ignore => MisfireAction::Ignore,
            MisfirePolicy::FireOnce => MisfireAction::FireOnce,
            MisfirePolicy::FireAll => {
                // Calculate how many runs were missed (simplified)
                MisfireAction::FireAll
            }
        }
    }
}

/// Calculate Haversine distance between two points (in km)
fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371.0; // Earth's radius in km

    let lat1_rad = lat1.to_radians();
    let lat2_rad = lat2.to_radians();
    let delta_lat = (lat2 - lat1).to_radians();
    let delta_lon = (lon2 - lon1).to_radians();

    let a = (delta_lat / 2.0).sin().powi(2)
        + lat1_rad.cos() * lat2_rad.cos() * (delta_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

    R * c
}

/// Initialize and start the scheduler service
pub fn start_scheduler_service(state: Arc<ApiState>, config: SchedulerConfig) {
    let service = SchedulerService::new(config);
    service.start(state);
}
