//! Plan computer hours and overage (Eoj, 2026-11-13). A child module of
//! `provisioning`, next to the credit computer lane it borrows its rate,
//! wallet and stop wording from.
//!
//! ## The rule
//!
//! A paid plan's included computer gets a number of running hours per UTC
//! calendar month: Plus 200 (`ALLTERNIT_PLUS_INCLUDED_HOURS`). Max and Ultra
//! have no number yet, so they are not metered here. Hours past the included
//! ones bill from the USD credit wallet at the credit computer running rate
//! ([`CreditDefaults::price_per_hour_usd`], $0.10/h), per minute, the same way
//! credit computers bill. At $0 the plan computer stops (status `suspended`,
//! `plan_overage_stopped_at` set). It is never deleted: the plan still pays
//! for it. It is released (back to `sleeping` on Orgo, `stopped` elsewhere)
//! when the next month starts, when credits are added, or when charging is
//! turned off.
//!
//! ## Flag
//!
//! Metering always runs. Charging and the $0 stop need `ALLTERNIT_PLAN_OVERAGE=1`.
//! Overage minutes metered while the flag is off are settled as waived, so
//! turning it on never charges for hours used before.
//!
//! ## Metering and charging
//!
//! Each tick (the credit computer task, every minute) works on
//! `plan_computer_hours`, one row per computer per month: running seconds
//! from `provisioned_instance_usage_sessions` clipped to `[month start, now
//! rounded down to the minute)`, overage = running - included, and whole
//! overage minutes not yet settled (`charged_minutes + waived_minutes`) are
//! charged or waived. The ledger row (`credit_transactions`, source
//! [`PLAN_OVERAGE_LEDGER_SOURCE`], key `plan_computer_overage:<id>:<YYYY-MM>:<cursor>`),
//! the wallet debit and the cursor move happen in one transaction under a
//! row lock, so repeated ticks, two replicas or a restart never charge a
//! minute twice. The month a tick has just left is closed first, so the
//! last minutes of a month are settled against that month.

use super::*;
use crate::services::customer_emails::{self, CreditNotice};
use chrono::{Datelike, TimeZone};

const ENV_PLUS_INCLUDED_HOURS: &str = "ALLTERNIT_PLUS_INCLUDED_HOURS";
const ENV_CHARGING: &str = "ALLTERNIT_PLAN_OVERAGE";
/// Ledger `source` of every plan computer overage debit.
pub const PLAN_OVERAGE_LEDGER_SOURCE: &str = "plan_computer_overage";
/// `provisioned_instance_usage_sessions.stop_reason` when the plan computer
/// stopped at $0 past its hours.
pub const PLAN_HOURS_STOP_REASON: &str = "plan_hours_exhausted";
/// Billing plan id of Plus (`billing_subscriptions.plan_id`, `provisioned_instances.plan_id`).
pub const PLUS_PLAN_ID: &str = "plus";
const WARN_24H: f64 = 24.0;
const WARN_2H: f64 = 2.0;

/// Included hours and the charging flag.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanHoursDefaults {
    /// Running hours a month included with Plus (default 200).
    pub plus_included_hours: i64,
    /// `ALLTERNIT_PLAN_OVERAGE=1`: charge overage and stop at $0.
    pub charging: bool,
}

impl Default for PlanHoursDefaults {
    fn default() -> Self {
        Self { plus_included_hours: 200, charging: false }
    }
}

impl PlanHoursDefaults {
    pub fn from_env() -> Self {
        Self {
            plus_included_hours: std::env::var(ENV_PLUS_INCLUDED_HOURS)
                .ok()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|v| *v >= 0)
                .unwrap_or(200),
            charging: matches!(
                std::env::var(ENV_CHARGING).ok().as_deref().map(str::trim),
                Some("1") | Some("true") | Some("on")
            ),
        }
    }

    /// Included running hours a month for a billing plan (`None` = not metered).
    pub fn included_hours(&self, plan_id: Option<&str>) -> Option<i64> {
        (plan_id == Some(PLUS_PLAN_ID)).then_some(self.plus_included_hours)
    }

    /// Billing plan ids that have included hours.
    fn metered_plans(&self) -> Vec<String> {
        vec![PLUS_PLAN_ID.to_string()]
    }
}

/// First instant of the UTC calendar month holding `ts`.
pub fn month_start(ts: DateTime<Utc>) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(ts.year(), ts.month(), 1, 0, 0, 0).single().unwrap_or(ts)
}

/// First instant of the next UTC calendar month.
pub fn next_month_start(ts: DateTime<Utc>) -> DateTime<Utc> {
    let (year, month) = if ts.month() == 12 { (ts.year() + 1, 1) } else { (ts.year(), ts.month() + 1) };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single().unwrap_or(ts)
}

/// Result of one metering pass over one plan computer (current month).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanHoursTick {
    pub instance_id: String,
    pub user_id: String,
    pub status: String,
    pub period_start: DateTime<Utc>,
    pub running_seconds: i64,
    pub included_seconds: i64,
    /// Overage minutes newly charged by this pass (both months).
    pub charged_minutes: i64,
    pub charged_usd: f64,
    pub balance_usd: f64,
}

impl PlanHoursTick {
    pub fn overage_seconds(&self) -> i64 {
        (self.running_seconds - self.included_seconds).max(0)
    }
}

/// Hours of the plan computer this month, for compute-status.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanHoursUsage {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub included_hours: i64,
    pub used_hours: f64,
    pub overage_hours: f64,
    pub overage_price_per_hour_usd: f64,
    pub overage_charged_usd: f64,
    /// `ALLTERNIT_PLAN_OVERAGE` is on (overage bills from credits).
    pub overage_billing: bool,
    /// Stopped at $0 past the included hours.
    pub stopped_for_hours: bool,
    /// When a computer stopped for hours can run again without credits.
    pub resumes_at: Option<DateTime<Utc>>,
}

/// Running seconds of the instance in `[from, to)`.
async fn running_seconds_between<'e, E>(
    executor: E,
    instance_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<i64, ApiError>
where
    E: sqlx::PgExecutor<'e>,
{
    if to <= from {
        return Ok(0);
    }
    Ok(sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(GREATEST(0, EXTRACT(EPOCH FROM (LEAST(COALESCE(ended_at, $3), $3) - GREATEST(started_at, $2))))), 0)::BIGINT
        FROM provisioned_instance_usage_sessions
        WHERE instance_id = $1 AND started_at < $3 AND COALESCE(ended_at, $3) > $2
        "#,
    )
    .bind(instance_id)
    .bind(from)
    .bind(to)
    .fetch_one(executor)
    .await?)
}

/// What one settle of one month did.
struct Settled {
    running_seconds: i64,
    charged_minutes: i64,
    charged_usd: f64,
    debited: bool,
}

impl ProvisioningService {
    /// The plan hours settings.
    pub fn plan_hours_defaults(&self) -> &PlanHoursDefaults {
        &self.plan_hours
    }

    /// One pass over every metered plan computer: meter, charge, then stop
    /// at $0 or record low-balance warnings. Returns what was metered.
    pub async fn bill_plan_computer_hours(&self, now: DateTime<Utc>) -> Result<Vec<PlanHoursTick>, ApiError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM provisioned_instances \
             WHERE tier = 'paid' AND plan_id = ANY($1) \
               AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking') \
             ORDER BY id",
        )
        .bind(self.plan_hours.metered_plans())
        .fetch_all(&self.db)
        .await?;
        let mut ticks = Vec::with_capacity(ids.len());
        for id in ids {
            let tick = match self.meter_plan_hours(&id, now).await {
                Ok(Some(tick)) => tick,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%id, %error, "plan hours: computer not metered this pass");
                    continue;
                }
            };
            if let Err(error) = self.after_plan_hours_tick(&tick, now).await {
                tracing::warn!(id = %tick.instance_id, %error, "plan hours: follow-up failed; retried next pass");
            }
            ticks.push(tick);
        }
        Ok(ticks)
    }

    /// Meter (and, with the flag, charge) one plan computer: the month the
    /// clock just left if it is still open, then the current month.
    pub(crate) async fn meter_plan_hours(
        &self,
        instance_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<PlanHoursTick>, ApiError> {
        let to = credit::floor_minute(now);
        let period_start = month_start(to);
        let mut transaction = self.db.begin().await?;
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT user_id, status, plan_id FROM provisioned_instances WHERE id = $1 AND tier = 'paid' FOR UPDATE",
        )
        .bind(instance_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((user_id, status, plan_id)) = row else {
            transaction.rollback().await?;
            return Ok(None);
        };
        let Some(included_hours) = self.plan_hours.included_hours(plan_id.as_deref()) else {
            transaction.rollback().await?;
            return Ok(None);
        };
        let included_seconds = included_hours.saturating_mul(3600);

        // Close the previous month if its last minutes are not settled yet.
        let previous: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT period_start FROM plan_computer_hours \
             WHERE instance_id = $1 AND period_start < $2 AND metered_through < $2 \
             ORDER BY period_start DESC LIMIT 1",
        )
        .bind(instance_id)
        .bind(period_start)
        .fetch_optional(&mut *transaction)
        .await?;
        let mut charged_minutes = 0;
        let mut charged_usd = 0.0;
        let mut debited = false;
        if let Some(previous_start) = previous {
            let closed = self
                .settle_month(&mut transaction, instance_id, &user_id, plan_id.as_deref(), previous_start, next_month_start(previous_start), included_seconds, now)
                .await?;
            charged_minutes += closed.charged_minutes;
            charged_usd += closed.charged_usd;
            debited |= closed.debited;
        }
        let current = self
            .settle_month(&mut transaction, instance_id, &user_id, plan_id.as_deref(), period_start, to, included_seconds, now)
            .await?;
        charged_minutes += current.charged_minutes;
        charged_usd += current.charged_usd;
        debited |= current.debited;
        let balance_usd = credit::wallet_balance(&mut *transaction, &user_id).await?;
        transaction.commit().await?;
        if debited {
            crate::services::auto_topup::spawn_check(self.db.clone(), user_id.clone(), balance_usd);
        }
        Ok(Some(PlanHoursTick {
            instance_id: instance_id.to_string(),
            user_id,
            status,
            period_start,
            running_seconds: current.running_seconds,
            included_seconds,
            charged_minutes,
            charged_usd,
            balance_usd,
        }))
    }

    /// Meter one month of one computer up to `to` (clamped to the month)
    /// and settle its new overage minutes. Runs inside the caller's
    /// transaction, which holds the instance row lock.
    #[allow(clippy::too_many_arguments)]
    async fn settle_month(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        instance_id: &str,
        user_id: &str,
        plan_id: Option<&str>,
        period_start: DateTime<Utc>,
        to: DateTime<Utc>,
        included_seconds: i64,
        now: DateTime<Utc>,
    ) -> Result<Settled, ApiError> {
        let to = to.min(next_month_start(period_start)).max(period_start);
        sqlx::query(
            r#"
            INSERT INTO plan_computer_hours (instance_id, period_start, user_id, plan_id, included_seconds, metered_through)
            VALUES ($1, $2, $3, $4, $5, $2)
            ON CONFLICT (instance_id, period_start) DO NOTHING
            "#,
        )
        .bind(instance_id)
        .bind(period_start)
        .bind(user_id)
        .bind(plan_id)
        .bind(included_seconds)
        .execute(&mut **transaction)
        .await?;
        let (charged_before, waived_before): (i64, i64) = sqlx::query_as(
            "SELECT charged_minutes, waived_minutes FROM plan_computer_hours \
             WHERE instance_id = $1 AND period_start = $2 FOR UPDATE",
        )
        .bind(instance_id)
        .bind(period_start)
        .fetch_one(&mut **transaction)
        .await?;
        let running_seconds = running_seconds_between(&mut **transaction, instance_id, period_start, to).await?;
        let overage_minutes = (running_seconds - included_seconds).max(0) / 60;
        let settled_before = charged_before + waived_before;
        let new_minutes = (overage_minutes - settled_before).max(0);
        let (mut charged, mut waived, mut charge_usd, mut debited) = (0_i64, 0_i64, 0.0_f64, false);
        if new_minutes > 0 {
            if self.plan_hours.charging {
                charged = new_minutes;
                let charge = new_minutes as f64 * self.credit.price_per_hour_usd / 60.0;
                if charge > 0.0 {
                    let inserted = sqlx::query(
                        r#"
                        INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
                        VALUES ($1, $2, $3, $4, $5, $6)
                        ON CONFLICT (transaction_id) DO NOTHING
                        "#,
                    )
                    .bind(Uuid::new_v4().to_string())
                    .bind(user_id)
                    .bind(-charge)
                    .bind(format!(
                        "{PLAN_OVERAGE_LEDGER_SOURCE}:{instance_id}:{}:{}",
                        period_start.format("%Y-%m"),
                        settled_before + new_minutes
                    ))
                    .bind(PLAN_OVERAGE_LEDGER_SOURCE)
                    .bind(now)
                    .execute(&mut **transaction)
                    .await?;
                    if inserted.rows_affected() == 1 {
                        charge_usd = charge;
                        // Clamped at zero; a chargeback hold below zero stays (as credit computers).
                        let updated = sqlx::query(
                            "UPDATE user_credits SET balance_usd = GREATEST(LEAST(balance_usd, 0), balance_usd - $1) WHERE user_id = $2",
                        )
                        .bind(charge)
                        .bind(user_id)
                        .execute(&mut **transaction)
                        .await?;
                        debited = updated.rows_affected() > 0;
                    }
                }
            } else {
                waived = new_minutes;
            }
        }
        sqlx::query(
            r#"
            UPDATE plan_computer_hours
            SET running_seconds = $3, metered_through = GREATEST(metered_through, $4), included_seconds = $5,
                charged_minutes = charged_minutes + $6, waived_minutes = waived_minutes + $7,
                charged_usd = charged_usd + $8, updated_at = CURRENT_TIMESTAMP
            WHERE instance_id = $1 AND period_start = $2
            "#,
        )
        .bind(instance_id)
        .bind(period_start)
        .bind(running_seconds)
        .bind(to)
        .bind(included_seconds)
        .bind(charged)
        .bind(waived)
        .bind(charge_usd)
        .execute(&mut **transaction)
        .await?;
        Ok(Settled { running_seconds, charged_minutes: charged, charged_usd: charge_usd, debited })
    }

    /// After a pass, with charging on and the month past its hours: stop at
    /// $0, or record the low-balance warnings (same thresholds as credit
    /// computers, re-armed after a top-up and every month).
    async fn after_plan_hours_tick(&self, tick: &PlanHoursTick, now: DateTime<Utc>) -> Result<(), ApiError> {
        if !self.plan_hours.charging || tick.overage_seconds() == 0 {
            return Ok(());
        }
        if tick.balance_usd <= 0.0 {
            let row = self.fetch_row(&tick.instance_id, None).await?;
            return self.stop_plan_for_hours(&row, now).await;
        }
        let Some(runway) = self.credit.runway_hours(tick.balance_usd) else {
            return Ok(());
        };
        let warned: (Option<DateTime<Utc>>, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT warned_24h_at, warned_2h_at FROM plan_computer_hours WHERE instance_id = $1 AND period_start = $2",
        )
        .bind(&tick.instance_id)
        .bind(tick.period_start)
        .fetch_one(&self.db)
        .await?;
        let set_warned = |sql: &'static str| {
            sqlx::query(sql).bind(&tick.instance_id).bind(tick.period_start).bind(now)
        };
        if runway > WARN_24H {
            // A top-up lifted the runway back: arm the warnings again.
            if warned.0.is_some() || warned.1.is_some() {
                sqlx::query("UPDATE plan_computer_hours SET warned_24h_at = NULL, warned_2h_at = NULL WHERE instance_id = $1 AND period_start = $2")
                    .bind(&tick.instance_id)
                    .bind(tick.period_start)
                    .execute(&self.db)
                    .await?;
            }
            return Ok(());
        }
        let notice = if runway <= WARN_2H && warned.1.is_none() {
            set_warned("UPDATE plan_computer_hours SET warned_2h_at = $3, warned_24h_at = COALESCE(warned_24h_at, $3) WHERE instance_id = $1 AND period_start = $2")
                .execute(&self.db)
                .await?;
            Some(CreditNotice::Low2h)
        } else if runway > WARN_2H && warned.0.is_none() {
            set_warned("UPDATE plan_computer_hours SET warned_24h_at = $3 WHERE instance_id = $1 AND period_start = $2")
                .execute(&self.db)
                .await?;
            Some(CreditNotice::Low24h)
        } else {
            None
        };
        if let Some(notice) = notice {
            let reference = format!("plan:{}:{}", tick.instance_id, now.timestamp());
            customer_emails::notify_credit(&self.db, &tick.user_id, notice, &reference).await;
        }
        Ok(())
    }

    /// Past the included hours with the wallet at $0: stop the plan
    /// computer, mark it `suspended` with `plan_overage_stopped_at`, close
    /// metering. Nothing is deleted and no deletion is scheduled.
    pub(crate) async fn stop_plan_for_hours(&self, row: &InstanceRow, now: DateTime<Utc>) -> Result<(), ApiError> {
        if row.tier != TIER_PAID || !matches!(row.status.as_str(), "provisioning" | "running" | "stopped" | "sleeping" | "waking") {
            return Ok(());
        }
        if matches!(row.status.as_str(), "running" | "waking" | "provisioning") {
            let backend = self.backend_for_row(row).await?;
            match backend.stop(&row.incus_name).await {
                Ok(()) => {}
                Err(ProvisionError::NotFound(_)) => return self.mark_missing(row).await,
                Err(error) => return Err(error.to_api_error()),
            }
        }
        let updated = sqlx::query(
            r#"
            UPDATE provisioned_instances
            SET status = 'suspended', plan_overage_stopped_at = $2, last_stopped_at = $2,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND tier = 'paid' AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking')
            "#,
        )
        .bind(&row.id)
        .bind(now)
        .execute(&self.db)
        .await?;
        record_instance_stopped(&self.db, &row.id, PLAN_HOURS_STOP_REASON).await?;
        if updated.rows_affected() == 1 {
            let resumes_at = next_month_start(now);
            tracing::info!(id = %row.id, %resumes_at, "plan computer past its hours stopped at $0");
            customer_emails::notify_credit(
                &self.db,
                &row.user_id,
                CreditNotice::PlanHoursStopped { resumes_at },
                &format!("{}:{}", row.id, now.timestamp()),
            )
            .await;
        }
        Ok(())
    }

    /// Release plan computers stopped for hours when the month they stopped
    /// in is over, the wallet has credits again, or charging is off. Only
    /// while the subscription is active and not cancelled: a lapsed plan is
    /// the billing webhook's to resume. Released computers go back to
    /// `sleeping` on Orgo (agent entry wakes them) or `stopped` (`/start`).
    /// Also clears the marker from rows the webhook already resumed.
    pub async fn release_plan_hours_stops(&self, now: DateTime<Utc>) -> Result<Vec<String>, ApiError> {
        sqlx::query(
            "UPDATE provisioned_instances SET plan_overage_stopped_at = NULL \
             WHERE plan_overage_stopped_at IS NOT NULL AND status <> 'suspended'",
        )
        .execute(&self.db)
        .await?;
        let rows: Vec<(String, String, DateTime<Utc>)> = sqlx::query_as(
            r#"
            SELECT i.id, i.user_id, i.plan_overage_stopped_at
            FROM provisioned_instances i
            WHERE i.tier = 'paid' AND i.status = 'suspended' AND i.plan_overage_stopped_at IS NOT NULL
              AND i.cancelled_at IS NULL
              AND EXISTS (
                  SELECT 1 FROM billing_subscriptions s
                  WHERE s.stripe_subscription_id = i.subscription_id AND s.status IN ('active', 'trialing')
              )
            ORDER BY i.id
            "#,
        )
        .fetch_all(&self.db)
        .await?;
        let mut released = Vec::new();
        for (id, user_id, stopped_at) in rows {
            let new_month = month_start(now) > stopped_at;
            let funded = credit::wallet_balance(&self.db, &user_id).await? > 0.0;
            if !(new_month || funded || !self.plan_hours.charging) {
                continue;
            }
            let row = self.fetch_row(&id, None).await?;
            let sleeps = match self.backend_for_row(&row).await {
                Ok(backend) => backend.provider() == ORGO_SCHEME,
                Err(error) => {
                    tracing::warn!(%id, %error, "plan hours: release skipped; backend unknown");
                    continue;
                }
            };
            let updated = sqlx::query(
                r#"
                UPDATE provisioned_instances
                SET status = $2, plan_overage_stopped_at = NULL, updated_at = CURRENT_TIMESTAMP
                WHERE id = $1 AND status = 'suspended' AND plan_overage_stopped_at IS NOT NULL AND cancelled_at IS NULL
                "#,
            )
            .bind(&id)
            .bind(if sleeps { "sleeping" } else { "stopped" })
            .execute(&self.db)
            .await?;
            if updated.rows_affected() == 1 {
                tracing::info!(%id, new_month, funded, "plan computer released after an hours stop");
                released.push(id);
            }
        }
        Ok(released)
    }
}

/// This month's hours of `instance_id` (live, to `now`), or `None` when it is
/// not a plan computer with included hours. Read by compute-status.
pub async fn plan_hours_usage(
    db: &PgPool,
    instance_id: &str,
    defaults: &PlanHoursDefaults,
    price_per_hour_usd: f64,
    now: DateTime<Utc>,
) -> Result<Option<PlanHoursUsage>, ApiError> {
    let row: Option<(String, Option<String>, String, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT tier, plan_id, status, plan_overage_stopped_at FROM provisioned_instances WHERE id = $1",
    )
    .bind(instance_id)
    .fetch_optional(db)
    .await?;
    let Some((tier, plan_id, status, stopped_at)) = row else {
        return Ok(None);
    };
    if tier != TIER_PAID {
        return Ok(None);
    }
    let Some(included_hours) = defaults.included_hours(plan_id.as_deref()) else {
        return Ok(None);
    };
    let period_start = month_start(now);
    let period_end = next_month_start(now);
    let running = running_seconds_between(db, instance_id, period_start, now).await?;
    let charged: Option<f64> = sqlx::query_scalar(
        "SELECT charged_usd FROM plan_computer_hours WHERE instance_id = $1 AND period_start = $2",
    )
    .bind(instance_id)
    .bind(period_start)
    .fetch_optional(db)
    .await?;
    let used_hours = running as f64 / 3600.0;
    let stopped_for_hours = status == "suspended" && stopped_at.is_some();
    Ok(Some(PlanHoursUsage {
        period_start,
        period_end,
        included_hours,
        used_hours,
        overage_hours: (used_hours - included_hours as f64).max(0.0),
        overage_price_per_hour_usd: price_per_hour_usd,
        overage_charged_usd: charged.unwrap_or(0.0),
        overage_billing: defaults.charging,
        stopped_for_hours,
        resumes_at: stopped_for_hours.then(|| next_month_start(stopped_at.unwrap_or(now))),
    }))
}
