//! Free sleeping cloud computer (plan PLAN-cloud-computer-provisioning-2026-09-30,
//! decisions 16 and 17). A child module of `provisioning`, so it drives the
//! same service, backend registry and state machine.
//!
//! A free account gets one small container from the same image as paid
//! (`allternit-cloud-computer`: the Linux Desktop app in provisioned mode,
//! Chrome started only when a subscription task needs it, the CLI tools),
//! with smaller limits. It **sleeps** (`incus stop`, status
//! `sleeping`, costs only its disk) after `ALLTERNIT_FREE_IDLE_MINUTES` with no
//! activity and **wakes** (status `waking` → `running`) on:
//!
//! - the owner: `POST /api/v1/provisioned-instances/:id/wake`;
//! - a relay request or socket ticket to its runtime device
//!   (`runtime_relay::connect_or_wake_runtime`);
//! - a scheduled job: the runtime's own next due job
//!   (`POST /api/v1/runtime-devices/:id/activity {nextWakeAt}`) or a cloud
//!   `schedules` row owned by the user, `ALLTERNIT_FREE_WAKE_LEAD_SECONDS`
//!   before it is due.
//!
//! **Ready signal:** the runtime device holds a live relay connection
//! (`runtimeOnline: true` on the instance view). `running` alone means the
//! container started; the daemon connects a few seconds later.
//!
//! **Activity** (the idle clock, `last_activity_at`): user relay traffic
//! (requests, socket tickets, socket frames — `touch_runtime_activity`), a
//! runtime attaching to the relay, a wake, and the runtime's own
//! `POST .../activity {busy: true}` while it runs a job. Runtime heartbeats
//! deliberately do not count, or an idle daemon would never sleep.
//!
//! **Guards:** at most `ALLTERNIT_FREE_MAX_AWAKE_PER_HOST` awake free computers
//! per host (create and wake both check it), at most
//! `ALLTERNIT_FREE_MAX_WAKES_PER_HOUR` wakes per user per rolling hour (all
//! reasons), the root disk size is the disk cap, and free computers never get
//! the cancel snapshot: they are deleted after
//! `ALLTERNIT_FREE_DELETE_AFTER_DAYS` without the owner using them
//! (`last_owner_activity_at`; scheduled wakes do not count).
//!
//! **Upgrade:** when the account gets a paid computer, the free one is marked
//! `replaced_by` the paid one, put to sleep, and deleted
//! `ALLTERNIT_FREE_DELETE_AFTER_DAYS` later. It stays wakeable until then so
//! the runtime's data can be exported. It is not restored into the paid
//! computer: the paid computer is created fresh at the plan size.

use super::*;

const ENV_FREE_IMAGE: &str = "ALLTERNIT_FREE_IMAGE";
const ENV_FREE_CPU: &str = "ALLTERNIT_FREE_CPU";
const ENV_FREE_CPU_ALLOWANCE: &str = "ALLTERNIT_FREE_CPU_ALLOWANCE";
const ENV_FREE_CPU_PRIORITY: &str = "ALLTERNIT_FREE_CPU_PRIORITY";
const ENV_FREE_MEMORY_MB: &str = "ALLTERNIT_FREE_MEMORY_MB";
const ENV_FREE_DISK_GB: &str = "ALLTERNIT_FREE_DISK_GB";
const ENV_FREE_IDLE_MINUTES: &str = "ALLTERNIT_FREE_IDLE_MINUTES";
const ENV_FREE_MAX_AWAKE_PER_HOST: &str = "ALLTERNIT_FREE_MAX_AWAKE_PER_HOST";
const ENV_FREE_MAX_WAKES_PER_HOUR: &str = "ALLTERNIT_FREE_MAX_WAKES_PER_HOUR";
const ENV_FREE_DELETE_AFTER_DAYS: &str = "ALLTERNIT_FREE_DELETE_AFTER_DAYS";
const ENV_FREE_WAKE_LEAD_SECONDS: &str = "ALLTERNIT_FREE_WAKE_LEAD_SECONDS";
const ENV_FREE_SWEEP_SECONDS: &str = "ALLTERNIT_FREE_SWEEP_SECONDS";
const ENV_FREE_ENABLED: &str = "ALLTERNIT_FREE_COMPUTERS";

// Sizes and limits are Eoj-open (plan "Open for Eoj"); env overrides them.
// Decision 17: the same full image as paid (Desktop app + Chrome + CLI
// tools), with smaller limits that fit Chrome started on demand.
const DEFAULT_FREE_IMAGE: &str = "allternit-cloud-computer";
/// Eoj 2026-10-01: 2 vCPU (a wake reaches /health in ~20s; 1 vCPU took
/// ~40s) at low CPU priority, so paid computers win under contention.
const DEFAULT_FREE_CPU: i64 = 2;
/// Incus `limits.cpu.priority` (0–10; Incus default 10, which paid keeps).
const DEFAULT_FREE_CPU_PRIORITY: u8 = 2;
/// Optional Incus `limits.cpu.allowance` on top of `limits.cpu` (e.g.
/// "50ms/100ms" = half a core, hard quota). Empty = the whole vCPU.
const DEFAULT_FREE_CPU_ALLOWANCE: &str = "";
const DEFAULT_FREE_MEMORY_MB: i64 = 2048;
const DEFAULT_FREE_DISK_GB: i64 = 10;
const DEFAULT_FREE_IDLE_MINUTES: i64 = 15;
const DEFAULT_FREE_MAX_AWAKE_PER_HOST: i64 = 10;
const DEFAULT_FREE_MAX_WAKES_PER_HOUR: i64 = 12;
const DEFAULT_FREE_DELETE_AFTER_DAYS: i64 = 30;
const DEFAULT_FREE_WAKE_LEAD_SECONDS: i64 = 120;
const DEFAULT_FREE_SWEEP_SECONDS: u64 = 60;
/// A cloud schedule this far past due no longer wakes the computer (the
/// scheduler advances `next_run_at` after a run; a stale one must not keep
/// waking it).
const SCHEDULE_STALE_MINUTES: i64 = 10;

/// Free computer settings, snapshotted at service construction.
#[derive(Debug, Clone)]
pub struct FreeDefaults {
    /// `ALLTERNIT_FREE_COMPUTERS=1` turns on the create endpoint. Sleep,
    /// wake and retention always run, but only act on free rows.
    pub enabled: bool,
    pub image: String,
    pub cpu_cores: i64,
    pub cpu_allowance: Option<String>,
    /// `limits.cpu.priority`; `ALLTERNIT_FREE_CPU_PRIORITY` (empty = Incus default).
    pub cpu_priority: Option<u8>,
    pub memory_mb: i64,
    pub disk_gb: i64,
    pub idle: Duration,
    pub max_awake_per_host: i64,
    pub max_wakes_per_hour: i64,
    pub delete_after: Duration,
    pub wake_lead: Duration,
}

impl FreeDefaults {
    pub fn from_env() -> Self {
        Self {
            enabled: matches!(std::env::var(ENV_FREE_ENABLED).as_deref(), Ok("1") | Ok("true")),
            image: env_string(ENV_FREE_IMAGE, DEFAULT_FREE_IMAGE),
            cpu_cores: env_i64(ENV_FREE_CPU, DEFAULT_FREE_CPU).max(1),
            cpu_allowance: Some(env_string(ENV_FREE_CPU_ALLOWANCE, DEFAULT_FREE_CPU_ALLOWANCE))
                .filter(|value| !value.trim().is_empty()),
            cpu_priority: match std::env::var(ENV_FREE_CPU_PRIORITY) {
                Err(_) => Some(DEFAULT_FREE_CPU_PRIORITY),
                Ok(value) if value.trim().is_empty() => None,
                Ok(value) => Some(
                    value
                        .trim()
                        .parse::<u8>()
                        .map(|priority| priority.min(10))
                        .unwrap_or(DEFAULT_FREE_CPU_PRIORITY),
                ),
            },
            memory_mb: env_i64(ENV_FREE_MEMORY_MB, DEFAULT_FREE_MEMORY_MB),
            disk_gb: env_i64(ENV_FREE_DISK_GB, DEFAULT_FREE_DISK_GB),
            idle: Duration::minutes(env_i64(ENV_FREE_IDLE_MINUTES, DEFAULT_FREE_IDLE_MINUTES).max(1)),
            max_awake_per_host: env_i64(ENV_FREE_MAX_AWAKE_PER_HOST, DEFAULT_FREE_MAX_AWAKE_PER_HOST),
            max_wakes_per_hour: env_i64(ENV_FREE_MAX_WAKES_PER_HOUR, DEFAULT_FREE_MAX_WAKES_PER_HOUR),
            delete_after: Duration::days(
                env_i64(ENV_FREE_DELETE_AFTER_DAYS, DEFAULT_FREE_DELETE_AFTER_DAYS).max(1),
            ),
            wake_lead: Duration::seconds(
                env_i64(ENV_FREE_WAKE_LEAD_SECONDS, DEFAULT_FREE_WAKE_LEAD_SECONDS).max(0),
            ),
        }
    }

    pub fn size(&self) -> ComputerSize {
        ComputerSize {
            cpu_cores: self.cpu_cores,
            memory_mb: self.memory_mb,
            disk_gb: self.disk_gb,
        }
    }
}

/// Why a free computer is being woken (`provisioned_instance_wakes.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeReason {
    /// `POST /api/v1/provisioned-instances/:id/wake`.
    Owner,
    /// A relay request / socket ticket to the sleeping runtime.
    Relay,
    /// A scheduled job is about to be due.
    Schedule,
}

impl WakeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            WakeReason::Owner => "owner",
            WakeReason::Relay => "relay",
            WakeReason::Schedule => "schedule",
        }
    }

    /// Owner and relay wakes are the owner using the computer; scheduled
    /// wakes are not (they must not hold off free retention).
    fn is_owner_use(self) -> bool {
        !matches!(self, WakeReason::Schedule)
    }
}

/// Result of [`ProvisioningService::wake`].
#[derive(Debug)]
pub struct WakeResult {
    pub view: InstanceView,
    /// True when this call issued the start (false: already awake/waking).
    pub woke: bool,
    /// The background start task, when one was spawned (tests await it).
    pub task: Option<tokio::task::JoinHandle<()>>,
}

/// Outcome of a relay wake-on-demand for a runtime device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionedWakeOutcome {
    /// The device is not a provisioned computer.
    NotProvisioned,
    /// The computer is running; its daemon may still be connecting.
    AlreadyActive,
    /// A start is in flight (just issued or already `waking`/`provisioning`).
    Waking,
    /// The computer exists but cannot be woken (paid and stopped, suspended,
    /// error).
    NotWakeable,
}

/// User-driven relay traffic for a provisioned computer's device: moves the
/// idle clock and the owner-use clock. Called from
/// `services::touch_runtime_activity` next to the hosted-runtime stamp.
pub async fn touch_provisioned_activity(db: &PgPool, device_id: &str) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE provisioned_instances
        SET last_activity_at = CURRENT_TIMESTAMP, last_owner_activity_at = CURRENT_TIMESTAMP
        WHERE device_id = $1 AND status IN ('provisioning', 'running', 'waking')
        "#,
    )
    .bind(device_id)
    .execute(db)
    .await?;
    Ok(())
}

/// The runtime behind `device_id` attached to the relay. A free computer that
/// was `waking` (or `sleeping`, started outside the wake path) is `running`
/// now, and gets a fresh idle window so it is not slept right after a wake.
pub async fn note_runtime_attached(db: &PgPool, device_id: &str) -> Result<(), ApiError> {
    let woke: Vec<String> = sqlx::query_scalar(
        r#"
        UPDATE provisioned_instances
        SET status = 'running',
            last_started_at = CASE WHEN status = 'running' THEN last_started_at ELSE CURRENT_TIMESTAMP END,
            last_activity_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE device_id = $1 AND tier = 'free' AND status IN ('running', 'waking', 'sleeping')
        RETURNING id
        "#,
    )
    .bind(device_id)
    .fetch_all(db)
    .await?;
    for id in woke {
        record_instance_started(db, &id).await?;
    }
    Ok(())
}

impl ProvisioningService {
    /// Ensure the account's free computer exists (idempotent: returns the
    /// live one, whatever its status). The caller checks the account is free.
    pub async fn create_free(&self, user_id: &str) -> Result<InstanceView, ApiError> {
        self.create_tier(user_id, None, TIER_FREE).await
    }

    /// True when the user holds an active or trialing paid subscription.
    pub async fn has_active_subscription(&self, user_id: &str) -> Result<bool, ApiError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM billing_subscriptions WHERE user_id = $1 AND status IN ('active', 'trialing')",
        )
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;
        Ok(count > 0)
    }

    /// Wake a free computer. `user_id` scopes the lookup to the owner (the
    /// wake endpoint); `None` is the system (relay / scheduled) path.
    ///
    /// Idempotent: a `running`, `waking` or `provisioning` computer is
    /// returned as-is (`woke: false`) and counts no wake. A `sleeping` one
    /// is claimed `waking` under the host row lock — after the per-user
    /// hourly wake limit (429) and the per-host awake cap (503) — and its
    /// start runs in the background. The start task moves it to `running`;
    /// the runtime attaching to the relay is the ready signal.
    pub async fn wake(
        self: &Arc<Self>,
        instance_id: &str,
        user_id: Option<&str>,
        reason: WakeReason,
    ) -> Result<WakeResult, ApiError> {
        let row = self.fetch_row(instance_id, user_id).await?;
        if !row.is_free() {
            return Err(ApiError::BadRequest(
                "Only free computers sleep; start a paid computer with /start".to_string(),
            ));
        }
        match row.status.as_str() {
            "sleeping" => {}
            "running" | "waking" | "provisioning" => {
                if reason.is_owner_use() {
                    sqlx::query(
                        "UPDATE provisioned_instances SET last_activity_at = CURRENT_TIMESTAMP, last_owner_activity_at = CURRENT_TIMESTAMP WHERE id = $1",
                    )
                    .bind(&row.id)
                    .execute(&self.db)
                    .await?;
                }
                return Ok(WakeResult {
                    view: self.fetch_row(&row.id, None).await?.into(),
                    woke: false,
                    task: None,
                });
            }
            other => {
                return Err(ApiError::BadRequest(format!(
                    "Free computer cannot wake from status '{other}'"
                )))
            }
        }
        let Some(host_id) = row.host_id.clone() else {
            return Err(ApiError::ServiceUnavailable(
                "Free computer has no fleet host".to_string(),
            ));
        };

        let now = Utc::now();
        let mut transaction = self.db.begin().await?;
        // Serializes the awake-cap check per host.
        sqlx::query("SELECT id FROM provisioned_hosts WHERE id = $1 FOR UPDATE")
            .bind(&host_id)
            .execute(&mut *transaction)
            .await?;
        let wakes_last_hour: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instance_wakes WHERE user_id = $1 AND created_at > $2",
        )
        .bind(&row.user_id)
        .bind(now - Duration::hours(1))
        .fetch_one(&mut *transaction)
        .await?;
        if wakes_last_hour >= self.free.max_wakes_per_hour {
            transaction.rollback().await?;
            return Err(ApiError::TooManyRequests(format!(
                "Free computer wake limit reached ({} per hour); try again later",
                self.free.max_wakes_per_hour
            )));
        }
        let awake: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM provisioned_instances
            WHERE tier = 'free' AND host_id = $1 AND status IN ('provisioning', 'running', 'waking')
            "#,
        )
        .bind(&host_id)
        .fetch_one(&mut *transaction)
        .await?;
        if awake >= self.free.max_awake_per_host {
            transaction.rollback().await?;
            return Err(ApiError::ServiceUnavailable(
                "Free computers are at capacity right now; retry in a few minutes".to_string(),
            ));
        }
        let claimed = sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = 'waking', wake_count = wake_count + 1, last_woken_at = $2,
                last_activity_at = $2,
                last_owner_activity_at = CASE WHEN $3 THEN $2 ELSE last_owner_activity_at END,
                next_wake_at = CASE WHEN next_wake_at <= $4 THEN NULL ELSE next_wake_at END,
                error_message = NULL, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND status = 'sleeping'
            "#,
        )
        .bind(&row.id)
        .bind(now)
        .bind(reason.is_owner_use())
        .bind(now + self.free.wake_lead)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if claimed != 1 {
            // A concurrent wake won the claim.
            transaction.rollback().await?;
            return Ok(WakeResult {
                view: self.fetch_row(&row.id, None).await?.into(),
                woke: false,
                task: None,
            });
        }
        sqlx::query(
            "INSERT INTO provisioned_instance_wakes (id, instance_id, user_id, reason, created_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(format!("piw_{}", Uuid::new_v4().simple()))
        .bind(&row.id)
        .bind(&row.user_id)
        .bind(reason.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        tracing::info!(id = %row.id, reason = reason.as_str(), "free computer waking");

        let service = Arc::clone(self);
        let task_row = row.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = service.finish_wake(&task_row).await {
                tracing::warn!(id = %task_row.id, %error, "free computer wake failed");
            }
        });
        Ok(WakeResult {
            view: self.fetch_row(&row.id, None).await?.into(),
            woke: true,
            task: Some(task),
        })
    }

    /// Background half of a wake: start the container. Success → `running`
    /// (the relay attach then makes the runtime online). A vanished
    /// container → `error` ("missing"). Any other failure puts the row back
    /// to `sleeping` with the reason, so a later wake retries.
    async fn finish_wake(&self, row: &InstanceRow) -> Result<(), ApiError> {
        let backend = self.backend_for_row(row).await?;
        match backend.start(&row.incus_name).await {
            Ok(()) => {
                let moved = sqlx::query(
                    r#"
                    UPDATE provisioned_instances
                    SET status = 'running', last_started_at = CURRENT_TIMESTAMP,
                        last_activity_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
                    WHERE id = $1 AND status = 'waking'
                    "#,
                )
                .bind(&row.id)
                .execute(&self.db)
                .await?
                .rows_affected();
                if moved == 1 {
                    record_instance_started(&self.db, &row.id).await?;
                }
                Ok(())
            }
            Err(ProvisionError::NotFound(_)) => self.mark_missing(row).await,
            Err(error) => {
                sqlx::query(
                    "UPDATE provisioned_instances SET status = 'sleeping', error_message = $2, updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND status = 'waking'",
                )
                .bind(&row.id)
                .bind(format!("wake failed: {error}"))
                .execute(&self.db)
                .await?;
                Err(error.to_api_error())
            }
        }
    }

    /// Relay wake-on-demand for a runtime device (see module docs).
    pub async fn wake_for_device(
        self: &Arc<Self>,
        device_id: &str,
    ) -> Result<ProvisionedWakeOutcome, ApiError> {
        let row = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE device_id = $1 AND status <> 'deleted' ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(device_id)
        .fetch_optional(&self.db)
        .await?;
        let Some(row) = row else {
            return Ok(ProvisionedWakeOutcome::NotProvisioned);
        };
        Ok(match row.status.as_str() {
            "running" => ProvisionedWakeOutcome::AlreadyActive,
            "waking" | "provisioning" => ProvisionedWakeOutcome::Waking,
            "sleeping" if row.is_free() => {
                self.wake(&row.id, None, WakeReason::Relay).await?;
                ProvisionedWakeOutcome::Waking
            }
            _ => ProvisionedWakeOutcome::NotWakeable,
        })
    }

    /// Put a running free computer to sleep: backend stop, `sleeping`,
    /// metering interval closed with `reason`.
    pub(crate) async fn sleep_row(&self, row: &InstanceRow, reason: &str) -> Result<(), ApiError> {
        let backend = self.backend_for_row(row).await?;
        match backend.stop(&row.incus_name).await {
            Ok(()) => {}
            Err(ProvisionError::NotFound(_)) => return self.mark_missing(row).await,
            Err(error) => return Err(error.to_api_error()),
        }
        sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = 'sleeping', sleep_count = sleep_count + 1,
                last_stopped_at = CURRENT_TIMESTAMP, last_slept_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND status = 'running'
            "#,
        )
        .bind(&row.id)
        .execute(&self.db)
        .await?;
        record_instance_stopped(&self.db, &row.id, reason).await?;
        tracing::info!(id = %row.id, %reason, "free computer asleep");
        Ok(())
    }

    /// The runtime reports activity: `busy` (it is running a job, keep it
    /// awake) and/or its next scheduled job (`next_wake_at`: `Some(None)`
    /// clears it). Authenticated by the device credential in the route.
    pub async fn record_runtime_activity(
        &self,
        device_id: &str,
        busy: bool,
        next_wake_at: Option<Option<DateTime<Utc>>>,
    ) -> Result<Option<InstanceView>, ApiError> {
        let row = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE device_id = $1 AND status <> 'deleted' ORDER BY created_at DESC LIMIT 1"
        ))
        .bind(device_id)
        .fetch_optional(&self.db)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        if busy {
            sqlx::query(
                "UPDATE provisioned_instances SET last_activity_at = CURRENT_TIMESTAMP WHERE id = $1 AND status IN ('provisioning', 'running', 'waking')",
            )
            .bind(&row.id)
            .execute(&self.db)
            .await?;
        }
        if let Some(next_wake_at) = next_wake_at {
            sqlx::query("UPDATE provisioned_instances SET next_wake_at = $2 WHERE id = $1")
                .bind(&row.id)
                .bind(next_wake_at)
                .execute(&self.db)
                .await?;
        }
        Ok(Some(self.fetch_row(&row.id, None).await?.into()))
    }

    /// Upgrade (decision 16): mark the account's live free computer replaced
    /// by `paid_instance_id`, put it to sleep, and schedule its deletion.
    pub(crate) async fn retire_free_on_upgrade(
        &self,
        user_id: &str,
        paid_instance_id: &str,
    ) -> Result<(), ApiError> {
        let Some(row) = self.live_row_for(user_id, None, TIER_FREE).await? else {
            return Ok(());
        };
        if row.status == "running" {
            if let Err(error) = self.sleep_row(&row, "upgraded").await {
                tracing::warn!(id = %row.id, %error, "upgrade: free computer not slept; idle sweep will");
            }
        }
        sqlx::query(
            "UPDATE provisioned_instances SET replaced_by = $2, delete_after = $3, updated_at = CURRENT_TIMESTAMP WHERE id = $1",
        )
        .bind(&row.id)
        .bind(paid_instance_id)
        .bind(Utc::now() + self.free.delete_after)
        .execute(&self.db)
        .await?;
        tracing::info!(id = %row.id, %paid_instance_id, "free computer replaced by paid computer");
        Ok(())
    }

    /// One pass of the free-computer sweep (every
    /// `ALLTERNIT_FREE_SWEEP_SECONDS`): sleep idle ones, wake ones with a
    /// scheduled job about to be due, delete abandoned ones.
    pub async fn sweep_free(self: &Arc<Self>, now: DateTime<Utc>) -> Result<(), ApiError> {
        // 1. Idle → sleep (not when a scheduled job is about to be due).
        let idle = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE tier = 'free' AND status = 'running' \
               AND COALESCE(last_activity_at, last_started_at, created_at) <= $1 \
               AND (next_wake_at IS NULL OR next_wake_at > $2)"
        ))
        .bind(now - self.free.idle)
        .bind(now + self.free.wake_lead)
        .fetch_all(&self.db)
        .await?;
        for row in idle {
            if let Err(error) = self.sleep_row(&row, "idle").await {
                tracing::warn!(id = %row.id, %error, "free sweep: idle sleep failed");
            }
        }

        // 2. Scheduled job due soon → wake.
        let due = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances p \
             WHERE tier = 'free' AND status = 'sleeping' \
               AND (next_wake_at <= $1 \
                    OR EXISTS (SELECT 1 FROM schedules s \
                               WHERE s.owner_id = p.user_id AND s.enabled = TRUE \
                                 AND s.next_run_at <= $1 AND s.next_run_at > $2))"
        ))
        .bind(now + self.free.wake_lead)
        .bind(now - Duration::minutes(SCHEDULE_STALE_MINUTES))
        .fetch_all(&self.db)
        .await?;
        for row in due {
            match self.wake(&row.id, None, WakeReason::Schedule).await {
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(id = %row.id, %error, "free sweep: scheduled wake refused");
                }
            }
        }

        // 3. Retention: no snapshot, just delete — after N days without the
        //    owner, or N days after an upgrade replaced it.
        let abandoned = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE tier = 'free' AND status <> 'deleted' \
               AND ((replaced_by IS NULL AND COALESCE(last_owner_activity_at, created_at) <= $1) \
                    OR (replaced_by IS NOT NULL AND delete_after <= $2))"
        ))
        .bind(now - self.free.delete_after)
        .bind(now)
        .fetch_all(&self.db)
        .await?;
        for row in abandoned {
            if let Err(error) = self.delete(&row.id, &row.user_id).await {
                tracing::warn!(id = %row.id, %error, "free sweep: retention delete failed");
            } else {
                tracing::info!(id = %row.id, "free sweep: unused free computer deleted (no snapshot)");
            }
        }
        Ok(())
    }
}

/// Free computer sweep task: idle sleep, scheduled wake, retention.
/// Interval `ALLTERNIT_FREE_SWEEP_SECONDS` (default 60, floor 15).
pub fn start_free_computer_task(state: Arc<crate::ApiState>) {
    let interval_seconds = std::env::var(ENV_FREE_SWEEP_SECONDS)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value >= 15)
        .unwrap_or(DEFAULT_FREE_SWEEP_SECONDS);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(interval_seconds, "Free computer sweep task started");
        loop {
            interval.tick().await;
            if let Err(error) = state.provisioning_service.sweep_free(Utc::now()).await {
                tracing::error!("Free computer sweep failed: {}", error);
            }
        }
    });
}
