//! Credit computers (compute gate path C, CONTRACT-compute-gate-2026-11-13
//! section 6). A child module of `provisioning`: same service, registry,
//! state machine and metering sessions as paid and free computers.
//!
//! ## What it is
//!
//! A hosted computer (tier `credit`) bought with the USD credit wallet
//! (`user_credits`) instead of a subscription. One per account. Size Medium
//! (2 cpu / 8 GB / 50 GB, the contract placeholder). It lands on the Orgo host
//! by default (`ALLTERNIT_COMPUTE_PROVIDER_CREDIT`, default `orgo`) and sleeps
//! after 30 idle minutes like a paid Orgo computer.
//!
//! Creation is behind `ALLTERNIT_CREDIT_COMPUTERS=1` (the routes answer 404
//! otherwise). Billing always runs, so a computer made while the flag was on
//! never runs free after it is turned off.
//!
//! ## Billing
//!
//! [`start_credit_computer_task`] ticks every minute. Each tick debits, per
//! credit computer, the whole minutes in `[credit_billed_through, now rounded
//! down to the minute)`: running seconds (from the metering sessions) at
//! `ALLTERNIT_CREDIT_COMPUTER_PRICE_PER_HOUR_USD` (default 0.10) and the rest
//! at `ALLTERNIT_CREDIT_COMPUTER_STOPPED_PRICE_PER_HOUR_USD` (default 0.005).
//! The debit, its ledger row (`credit_transactions`, key
//! `credit_computer:<id>:<minute>`) and the cursor move happen in one
//! transaction under a row lock, so repeated ticks, two replicas or a restart
//! never charge a minute twice. A restart that misses ticks catches up on the
//! next one. The balance is clamped at 0 (a chargeback hold below 0 stays).
//!
//! ## Running out
//!
//! Runway = balance / running price. At 24 h and 2 h of runway a warning is
//! recorded once (cleared when a top-up lifts the runway back). At $0 the
//! computer stops and becomes `suspended` with `delete_after` = now + 14 days
//! (`ALLTERNIT_CREDIT_COMPUTER_RETENTION_DAYS`). Suspended computers are not
//! billed. Day 7 and day 13 warnings are recorded, then the computer is
//! deleted. Starting it again needs `ALLTERNIT_CREDIT_COMPUTER_MIN_START_USD`
//! (default 5) and resumes the same computer.
//!
//! Warning emails go through `customer_emails`: recorded as `queued` unless
//! `ALLTERNIT_CREDIT_COMPUTER_EMAILS=1` (and `ALLTERNIT_CUSTOMER_EMAILS=1`),
//! because the copy waits for Eoj's approval. Clients show the in-app notice
//! from `GET /api/v1/account/compute-status` (`computer.runway_hours`,
//! `computer.retention_until`).

use super::*;
use crate::services::customer_emails::{self, CreditNotice};

/// The one credit size the contract offers today.
pub const CREDIT_SIZE_ID: &str = "medium";

const ENV_ENABLED: &str = "ALLTERNIT_CREDIT_COMPUTERS";
const ENV_PRICE: &str = "ALLTERNIT_CREDIT_COMPUTER_PRICE_PER_HOUR_USD";
const ENV_STOPPED_PRICE: &str = "ALLTERNIT_CREDIT_COMPUTER_STOPPED_PRICE_PER_HOUR_USD";
const ENV_MIN_START: &str = "ALLTERNIT_CREDIT_COMPUTER_MIN_START_USD";
const ENV_RETENTION_DAYS: &str = "ALLTERNIT_CREDIT_COMPUTER_RETENTION_DAYS";
const ENV_CPU: &str = "ALLTERNIT_CREDIT_COMPUTER_CPU";
const ENV_MEMORY_MB: &str = "ALLTERNIT_CREDIT_COMPUTER_MEMORY_MB";
const ENV_DISK_GB: &str = "ALLTERNIT_CREDIT_COMPUTER_DISK_GB";
/// Ledger `source` of every credit computer debit.
pub const CREDIT_LEDGER_SOURCE: &str = "credit_computer";
/// Seconds between billing ticks.
const TICK_SECONDS: u64 = 60;
/// Low-runway warnings (hours of running time left).
const WARN_24H: f64 = 24.0;
const WARN_2H: f64 = 2.0;
/// Retention warnings, in days after the computer stopped at $0.
const DELETE_WARNING_DAYS: [i64; 2] = [7, 13];

/// Whether accounts may create credit computers (`ALLTERNIT_CREDIT_COMPUTERS`).
pub fn credit_computers_enabled() -> bool {
    matches!(
        std::env::var(ENV_ENABLED).ok().as_deref().map(str::trim),
        Some("1") | Some("true")
    )
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(default)
}

/// Size, prices, minimum start and retention of credit computers.
#[derive(Debug, Clone, PartialEq)]
pub struct CreditDefaults {
    pub cpu_cores: i64,
    pub memory_mb: i64,
    pub disk_gb: i64,
    pub price_per_hour_usd: f64,
    pub stopped_price_per_hour_usd: f64,
    pub min_start_usd: f64,
    pub retention: Duration,
}

impl Default for CreditDefaults {
    /// The contract placeholders (PENDING Eoj).
    fn default() -> Self {
        Self {
            cpu_cores: 2,
            memory_mb: 8192,
            disk_gb: 50,
            price_per_hour_usd: 0.10,
            stopped_price_per_hour_usd: 0.005,
            min_start_usd: 5.0,
            retention: Duration::days(14),
        }
    }
}

impl CreditDefaults {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            cpu_cores: env_i64(ENV_CPU, defaults.cpu_cores).max(1),
            memory_mb: env_i64(ENV_MEMORY_MB, defaults.memory_mb).max(1024),
            disk_gb: env_i64(ENV_DISK_GB, defaults.disk_gb).max(10),
            price_per_hour_usd: env_f64(ENV_PRICE, defaults.price_per_hour_usd),
            stopped_price_per_hour_usd: env_f64(ENV_STOPPED_PRICE, defaults.stopped_price_per_hour_usd),
            min_start_usd: env_f64(ENV_MIN_START, defaults.min_start_usd),
            retention: Duration::days(env_i64(ENV_RETENTION_DAYS, 14).max(1)),
        }
    }

    pub fn size(&self) -> ComputerSize {
        ComputerSize {
            cpu_cores: self.cpu_cores,
            memory_mb: self.memory_mb,
            disk_gb: self.disk_gb,
        }
    }

    /// Hours of running time the balance buys (`None` when running is free).
    pub fn runway_hours(&self, balance_usd: f64) -> Option<f64> {
        (self.price_per_hour_usd > 0.0).then(|| balance_usd.max(0.0) / self.price_per_hour_usd)
    }

    /// Charge for a billing window of `window_seconds` of which
    /// `running_seconds` ran (the rest is billed at the stopped rate).
    pub fn window_charge_usd(&self, window_seconds: i64, running_seconds: i64) -> f64 {
        let window = window_seconds.max(0) as f64;
        let running = (running_seconds.max(0) as f64).min(window);
        (running * self.price_per_hour_usd + (window - running) * self.stopped_price_per_hour_usd) / 3600.0
    }
}

/// `ts` rounded down to the whole minute.
pub(crate) fn floor_minute(ts: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(ts.timestamp() - ts.timestamp().rem_euclid(60), 0).unwrap_or(ts)
}

/// Result of one billing pass over one credit computer.
#[derive(Debug, Clone, PartialEq)]
pub struct CreditTick {
    pub instance_id: String,
    pub user_id: String,
    /// Whole minutes billed by this pass (0 = nothing new).
    pub minutes: i64,
    pub charged_usd: f64,
    /// Wallet balance after the pass.
    pub balance_usd: f64,
}

/// Outcome of `POST /api/v1/provisioned-instances/credit`.
#[derive(Debug)]
pub enum CreditStart {
    Started(InstanceView),
    /// Balance under `min_start_usd`: nothing was created or started.
    InsufficientCredits { balance_usd: f64 },
}

/// The account's wallet balance (0 without a row).
pub(crate) async fn wallet_balance<'e, E>(executor: E, user_id: &str) -> Result<f64, ApiError>
where
    E: sqlx::PgExecutor<'e>,
{
    let balance: Option<f64> = sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(executor)
        .await?;
    Ok(balance.unwrap_or(0.0))
}

impl ProvisioningService {
    /// The account's live credit computer, if any.
    pub async fn live_credit_row(&self, user_id: &str) -> Result<Option<InstanceRow>, ApiError> {
        self.live_row_for(user_id, None, TIER_CREDIT).await
    }

    /// Create or start the account's credit computer. Needs at least
    /// `min_start_usd` in the wallet, also to resume a suspended one. A
    /// computer already running, starting or waking is returned as-is.
    pub async fn start_credit(&self, user_id: &str) -> Result<CreditStart, ApiError> {
        let existing = self.live_credit_row(user_id).await?;
        if let Some(row) = &existing {
            if matches!(row.status.as_str(), "running" | "provisioning" | "waking") {
                return Ok(CreditStart::Started(self.view_of(row.clone()).await));
            }
        }
        let balance_usd = wallet_balance(&self.db, user_id).await?;
        if balance_usd < self.credit.min_start_usd {
            return Ok(CreditStart::InsufficientCredits { balance_usd });
        }
        let view = match existing {
            Some(row) if matches!(row.status.as_str(), "stopped" | "sleeping") => {
                self.start(&row.id, user_id).await?
            }
            // None: a new computer. Suspended: create_tier resumes it.
            _ => self.create_tier(user_id, None, TIER_CREDIT).await?,
        };
        // Billing starts now: a resumed computer is not charged for the days
        // it sat suspended, and its warnings start over.
        sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET credit_billed_through = GREATEST(COALESCE(credit_billed_through, $2), $2),
                credit_warned_24h_at = NULL, credit_warned_2h_at = NULL, credit_delete_warned = 0
            WHERE id = $1
            "#,
        )
        .bind(&view.id)
        .bind(floor_minute(Utc::now()))
        .execute(&self.db)
        .await?;
        Ok(CreditStart::Started(view))
    }

    /// Stop the account's credit computer (the stopped rate applies).
    /// `None` = the account has no credit computer. Idempotent: a computer
    /// that is already stopped, asleep or suspended is returned as-is.
    pub async fn stop_credit(&self, user_id: &str) -> Result<Option<InstanceView>, ApiError> {
        let Some(row) = self.live_credit_row(user_id).await? else {
            return Ok(None);
        };
        if matches!(row.status.as_str(), "stopped" | "sleeping" | "suspended") {
            return Ok(Some(self.view_of(row).await));
        }
        self.stop(&row.id, user_id).await.map(Some)
    }

    /// One billing pass over every billable credit computer, then the
    /// follow-ups: stop at $0, low-runway warnings. Returns what was billed.
    pub async fn bill_credit_computers(&self, now: DateTime<Utc>) -> Result<Vec<CreditTick>, ApiError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM provisioned_instances \
             WHERE tier = 'credit' AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking') \
             ORDER BY id",
        )
        .fetch_all(&self.db)
        .await?;
        let mut ticks = Vec::with_capacity(ids.len());
        for id in ids {
            let tick = match self.bill_credit_minutes(&id, now).await {
                Ok(Some(tick)) => tick,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%id, %error, "credit billing: computer not billed this pass");
                    continue;
                }
            };
            if let Err(error) = self.after_credit_tick(&tick, now).await {
                tracing::warn!(id = %tick.instance_id, %error, "credit billing: follow-up failed; retried next pass");
            }
            ticks.push(tick);
        }
        Ok(ticks)
    }

    /// Debit the unbilled whole minutes of one credit computer (see the
    /// module docs). `None` when the row is no longer billable.
    pub(crate) async fn bill_credit_minutes(
        &self,
        instance_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<CreditTick>, ApiError> {
        let to = floor_minute(now);
        let mut transaction = self.db.begin().await?;
        let row: Option<(String, String, Option<DateTime<Utc>>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT user_id, status, credit_billed_through, created_at FROM provisioned_instances \
             WHERE id = $1 AND tier = 'credit' FOR UPDATE",
        )
        .bind(instance_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((user_id, status, billed_through, created_at)) = row else {
            transaction.rollback().await?;
            return Ok(None);
        };
        if !matches!(status.as_str(), "provisioning" | "running" | "stopped" | "sleeping" | "waking") {
            transaction.rollback().await?;
            return Ok(None);
        }
        let from = billed_through.unwrap_or_else(|| floor_minute(created_at));
        if to <= from {
            let balance_usd = wallet_balance(&mut *transaction, &user_id).await?;
            transaction.commit().await?;
            return Ok(Some(CreditTick {
                instance_id: instance_id.to_string(),
                user_id,
                minutes: 0,
                charged_usd: 0.0,
                balance_usd,
            }));
        }
        let running_seconds: i64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(SUM(GREATEST(0, EXTRACT(EPOCH FROM (LEAST(COALESCE(ended_at, $3), $3) - GREATEST(started_at, $2))))), 0)::BIGINT
            FROM provisioned_instance_usage_sessions
            WHERE instance_id = $1 AND started_at < $3 AND COALESCE(ended_at, $3) > $2
            "#,
        )
        .bind(instance_id)
        .bind(from)
        .bind(to)
        .fetch_one(&mut *transaction)
        .await?;
        let window_seconds = (to - from).num_seconds();
        let charge = self.credit.window_charge_usd(window_seconds, running_seconds);
        let mut debited = false;
        if charge > 0.0 {
            let inserted = sqlx::query(
                r#"
                INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (transaction_id) DO NOTHING
                "#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&user_id)
            .bind(-charge)
            .bind(format!("{CREDIT_LEDGER_SOURCE}:{instance_id}:{}", to.timestamp() / 60))
            .bind(CREDIT_LEDGER_SOURCE)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
            if inserted.rows_affected() == 1 {
                // Clamp at zero; a balance already below zero (a chargeback
                // hold) stays where it is, as in cost_service.
                let updated = sqlx::query(
                    "UPDATE user_credits SET balance_usd = GREATEST(LEAST(balance_usd, 0), balance_usd - $1) WHERE user_id = $2",
                )
                .bind(charge)
                .bind(&user_id)
                .execute(&mut *transaction)
                .await?;
                debited = updated.rows_affected() > 0;
            }
        }
        sqlx::query("UPDATE provisioned_instances SET credit_billed_through = $2 WHERE id = $1")
            .bind(instance_id)
            .bind(to)
            .execute(&mut *transaction)
            .await?;
        let balance_usd = wallet_balance(&mut *transaction, &user_id).await?;
        transaction.commit().await?;
        if debited {
            crate::services::auto_topup::spawn_check(self.db.clone(), user_id.clone(), balance_usd);
        }
        Ok(Some(CreditTick {
            instance_id: instance_id.to_string(),
            user_id,
            minutes: window_seconds / 60,
            charged_usd: if debited { charge } else { 0.0 },
            balance_usd,
        }))
    }

    /// After a pass: stop at $0, or record the low-runway warnings.
    async fn after_credit_tick(&self, tick: &CreditTick, now: DateTime<Utc>) -> Result<(), ApiError> {
        if tick.balance_usd <= 0.0 {
            let row = self.fetch_row(&tick.instance_id, None).await?;
            return self.suspend_credit_row(&row, now).await;
        }
        let Some(runway) = self.credit.runway_hours(tick.balance_usd) else {
            return Ok(());
        };
        let warned: (Option<DateTime<Utc>>, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT credit_warned_24h_at, credit_warned_2h_at FROM provisioned_instances WHERE id = $1",
        )
        .bind(&tick.instance_id)
        .fetch_one(&self.db)
        .await?;
        // A top-up lifted the runway back: arm the warnings again.
        if runway > WARN_24H && (warned.0.is_some() || warned.1.is_some()) {
            sqlx::query(
                "UPDATE provisioned_instances SET credit_warned_24h_at = NULL, credit_warned_2h_at = NULL WHERE id = $1",
            )
            .bind(&tick.instance_id)
            .execute(&self.db)
            .await?;
            return Ok(());
        }
        if runway > WARN_2H && warned.1.is_some() {
            sqlx::query("UPDATE provisioned_instances SET credit_warned_2h_at = NULL WHERE id = $1")
                .bind(&tick.instance_id)
                .execute(&self.db)
                .await?;
        }
        let notice = if runway <= WARN_2H && warned.1.is_none() {
            // The 2 h warning also covers the 24 h one when both cross at once.
            sqlx::query(
                "UPDATE provisioned_instances SET credit_warned_2h_at = $2, credit_warned_24h_at = COALESCE(credit_warned_24h_at, $2) WHERE id = $1",
            )
            .bind(&tick.instance_id)
            .bind(now)
            .execute(&self.db)
            .await?;
            Some(CreditNotice::Low2h)
        } else if runway <= WARN_24H && runway > WARN_2H && warned.0.is_none() {
            sqlx::query("UPDATE provisioned_instances SET credit_warned_24h_at = $2 WHERE id = $1")
                .bind(&tick.instance_id)
                .bind(now)
                .execute(&self.db)
                .await?;
            Some(CreditNotice::Low24h)
        } else {
            None
        };
        if let Some(notice) = notice {
            let reference = format!("{}:{}", tick.instance_id, now.timestamp());
            customer_emails::notify_credit(&self.db, &tick.user_id, notice, &reference).await;
        }
        Ok(())
    }

    /// The wallet hit $0: stop the computer, mark it `suspended`, schedule
    /// its deletion after the retention window, close metering.
    pub(crate) async fn suspend_credit_row(&self, row: &InstanceRow, now: DateTime<Utc>) -> Result<(), ApiError> {
        if row.tier != TIER_CREDIT || matches!(row.status.as_str(), "suspended" | "deleted" | "error") {
            return Ok(());
        }
        // Stopped and sleeping computers are already off on the backend.
        if matches!(row.status.as_str(), "running" | "waking" | "provisioning") {
            let backend = self.backend_for_row(row).await?;
            match backend.stop(&row.incus_name).await {
                Ok(()) => {}
                Err(ProvisionError::NotFound(_)) => return self.mark_missing(row).await,
                Err(error) => return Err(error.to_api_error()),
            }
        }
        let delete_after = now + self.credit.retention;
        let updated = sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = 'suspended', cancelled_at = $2, delete_after = $3,
                last_stopped_at = $2, credit_delete_warned = 0, updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking')
            "#,
        )
        .bind(&row.id)
        .bind(now)
        .bind(delete_after)
        .execute(&self.db)
        .await?;
        record_instance_stopped(&self.db, &row.id, "credits_exhausted").await?;
        if updated.rows_affected() == 1 {
            tracing::info!(id = %row.id, %delete_after, "credit computer stopped at $0");
            customer_emails::notify_credit(
                &self.db,
                &row.user_id,
                CreditNotice::Stopped { delete_after },
                &format!("{}:{}", row.id, now.timestamp()),
            )
            .await;
        }
        Ok(())
    }

    /// Suspended credit computers: day 7 and day 13 warnings, then deletion
    /// once `delete_after` passes.
    pub async fn sweep_credit_retention(&self, now: DateTime<Utc>) -> Result<(), ApiError> {
        let rows: Vec<(String, String, DateTime<Utc>, Option<DateTime<Utc>>, i32)> = sqlx::query_as(
            "SELECT id, user_id, delete_after, cancelled_at, credit_delete_warned FROM provisioned_instances \
             WHERE tier = 'credit' AND status = 'suspended' AND delete_after IS NOT NULL",
        )
        .fetch_all(&self.db)
        .await?;
        for (id, user_id, delete_after, cancelled_at, warned) in rows {
            if delete_after <= now {
                let row = self.fetch_row(&id, None).await?;
                if let Err(error) = self.delete_credit_row(&row).await {
                    tracing::warn!(%id, %error, "credit retention: delete failed; retried next pass");
                }
                continue;
            }
            let stopped_at = cancelled_at.unwrap_or(delete_after - self.credit.retention);
            let due = DELETE_WARNING_DAYS
                .iter()
                .copied()
                .filter(|day| i64::from(warned) < *day && now >= stopped_at + Duration::days(*day))
                .max();
            if let Some(day) = due {
                sqlx::query("UPDATE provisioned_instances SET credit_delete_warned = $2 WHERE id = $1")
                    .bind(&id)
                    .bind(day as i32)
                    .execute(&self.db)
                    .await?;
                customer_emails::notify_credit(
                    &self.db,
                    &user_id,
                    CreditNotice::DeleteWarning { delete_after },
                    &format!("{id}:{}:d{day}", stopped_at.timestamp()),
                )
                .await;
            }
        }
        Ok(())
    }

    async fn delete_credit_row(&self, row: &InstanceRow) -> Result<(), ApiError> {
        if row.host_id.is_some() {
            let backend = self.backend_for_row(row).await?;
            match backend.delete(&row.incus_name).await {
                Ok(()) | Err(ProvisionError::NotFound(_)) => {}
                Err(error) => return Err(error.to_api_error()),
            }
        }
        let updated = sqlx::query(
            "UPDATE provisioned_instances SET status = 'deleted', deleted_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND status = 'suspended'",
        )
        .bind(&row.id)
        .execute(&self.db)
        .await?;
        if updated.rows_affected() == 1 {
            if let Some(host_id) = &row.host_id {
                self.release_allocation(host_id, row.allocation()).await;
            }
            if let Some(device_id) = &row.device_id {
                revoke_device(&self.db, device_id).await?;
            }
            tracing::info!(id = %row.id, "credit retention: stopped computer deleted");
        }
        Ok(())
    }
}

/// Per-minute billing and the retention sweep. Always on: computers made
/// while `ALLTERNIT_CREDIT_COMPUTERS` was on keep being billed after it is off.
pub fn start_credit_computer_task(state: Arc<crate::ApiState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(TICK_SECONDS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(interval_seconds = TICK_SECONDS, "Credit computer billing task started");
        loop {
            interval.tick().await;
            let now = Utc::now();
            if let Err(error) = state.provisioning_service.bill_credit_computers(now).await {
                tracing::error!("Credit computer billing failed: {}", error);
            }
            if let Err(error) = state.provisioning_service.sweep_credit_retention(now).await {
                tracing::error!("Credit computer retention sweep failed: {}", error);
            }
        }
    });
}
