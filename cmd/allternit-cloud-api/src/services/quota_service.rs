//! Runtime quota enforcement for pairing, devices, relay sockets, and bandwidth.
//!
//! Quotas are keyed by Clerk user id. The service creates a `user_runtime_quotas`
//! row lazily from the default `plan_tiers` row. Admins can override individual
//! quota fields without changing the tier.

use crate::error::ApiError;
use chrono::{Datelike, Timelike, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

/// Effective quota values for a single user.
#[derive(Debug, Clone)]
pub struct UserQuota {
    pub user_id: String,
    pub plan_tier_id: String,
    pub max_active_devices: i64,
    pub max_pairings_per_day: i64,
    pub max_relay_sockets: i64,
    pub max_relay_mb_per_day: i64,
    pub max_hosted_runtime_hours_monthly: i64,
    pub can_create_hosted_runtime: bool,
    pub max_hosted_runtimes: i64,
    pub max_hosted_runtime_memory_mb: i64,
    pub hard_spend_cap_usd: Option<f64>,
}

/// Service that reads and enforces user-level runtime quotas.
#[derive(Debug, Clone)]
pub struct QuotaService {
    db: PgPool,
    default_tier_id: String,
}

impl QuotaService {
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            default_tier_id: std::env::var("DEFAULT_PLAN_TIER")
                .unwrap_or_else(|_| "free".to_string()),
        }
    }

    /// Ensure a quota row exists for the user, creating it from the default tier
    /// if necessary. Returns the effective quota.
    pub async fn ensure_quota(&self, user_id: &str) -> Result<UserQuota, ApiError> {
        if let Some(quota) = self.get_quota(user_id).await? {
            return Ok(quota);
        }
        self.create_quota_from_tier(user_id, &self.default_tier_id)
            .await
    }

    /// Get the current effective quota for a user, if one exists.
    pub async fn get_quota(&self, user_id: &str) -> Result<Option<UserQuota>, ApiError> {
        let row = sqlx::query_as::<_, UserQuotaRow>(
            r#"
            SELECT
                q.user_id,
                q.plan_tier_id,
                q.max_active_devices,
                q.max_pairings_per_day,
                q.max_relay_sockets,
                q.max_relay_mb_per_day,
                q.max_hosted_runtime_hours_monthly,
                q.can_create_hosted_runtime,
                q.max_hosted_runtimes,
                q.max_hosted_runtime_memory_mb,
                q.hard_spend_cap_usd
            FROM user_runtime_quotas q
            WHERE q.user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?;
        Ok(row.map(UserQuota::from))
    }

    /// Create a quota row for a user by copying the named tier.
    async fn create_quota_from_tier(
        &self,
        user_id: &str,
        tier_id: &str,
    ) -> Result<UserQuota, ApiError> {
        sqlx::query(
            r#"
            INSERT INTO user_runtime_quotas (
                user_id, plan_tier_id,
                max_active_devices, max_pairings_per_day, max_relay_sockets,
                max_relay_mb_per_day, max_hosted_runtime_hours_monthly,
                can_create_hosted_runtime, max_hosted_runtimes,
                max_hosted_runtime_memory_mb, hard_spend_cap_usd
            )
            SELECT $1, id,
                max_active_devices, max_pairings_per_day, max_relay_sockets,
                max_relay_mb_per_day, max_hosted_runtime_hours_monthly,
                can_create_hosted_runtime, max_hosted_runtimes,
                max_hosted_runtime_memory_mb, hard_spend_cap_usd
            FROM plan_tiers
            WHERE id = $2
            ON CONFLICT(user_id) DO UPDATE SET
                plan_tier_id = excluded.plan_tier_id,
                max_active_devices = excluded.max_active_devices,
                max_pairings_per_day = excluded.max_pairings_per_day,
                max_relay_sockets = excluded.max_relay_sockets,
                max_relay_mb_per_day = excluded.max_relay_mb_per_day,
                max_hosted_runtime_hours_monthly = excluded.max_hosted_runtime_hours_monthly,
                can_create_hosted_runtime = excluded.can_create_hosted_runtime,
                max_hosted_runtimes = excluded.max_hosted_runtimes,
                max_hosted_runtime_memory_mb = excluded.max_hosted_runtime_memory_mb,
                hard_spend_cap_usd = excluded.hard_spend_cap_usd
            "#,
        )
        .bind(user_id)
        .bind(tier_id)
        .execute(&self.db)
        .await?;

        self.get_quota(user_id)
            .await?
            .ok_or_else(|| ApiError::Internal("Failed to initialize user quota".to_string()))
    }

    /// Enforce the active runtime device cap. Call inside a transaction around
    /// the device insert for correctness.
    pub async fn check_active_device_cap(
        &self,
        user_id: &str,
        quota: &UserQuota,
        hostname: Option<&str>,
        platform: Option<&str>,
    ) -> Result<(), ApiError> {
        // The cap counts machines, not sign-ins: the CLI and Desktop on the
        // same Mac are one runtime. A device's machine is its hostname +
        // platform (both sent by every pairing client); a device without a
        // hostname counts as its own machine.
        let machine = machine_key(hostname, platform);
        let (machines, this_machine_active): (i64, bool) = sqlx::query_as(
            r#"
            SELECT
                COUNT(DISTINCT COALESCE(
                    LOWER(NULLIF(hostname, '')) || '|' || COALESCE(platform, ''),
                    'device:' || id
                )),
                COALESCE(BOOL_OR(
                    $2::TEXT IS NOT NULL
                    AND LOWER(NULLIF(hostname, '')) || '|' || COALESCE(platform, '') = $2
                ), FALSE)
            FROM runtime_devices
            WHERE user_id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(user_id)
        .bind(machine.as_deref())
        .fetch_one(&self.db)
        .await?;

        if this_machine_active {
            return Ok(());
        }
        if machines >= quota.max_active_devices {
            return Err(ApiError::Forbidden(format!(
                "Active machine limit reached ({}/{}). Revoke a runtime on another machine or upgrade your plan.",
                machines, quota.max_active_devices
            )));
        }
        Ok(())
    }

    /// Record a pairing creation and enforce the daily pairing cap.
    pub async fn record_pairing_created(
        &self,
        user_id: &str,
        quota: &UserQuota,
    ) -> Result<(), ApiError> {
        let mut tx = self.db.begin().await?;
        match Self::record_pairing_created_in_tx(&mut tx, user_id, quota).await {
            Ok(()) => {
                tx.commit().await?;
                Ok(())
            }
            Err(error) => {
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    /// Same as [`Self::record_pairing_created`], but inside an existing
    /// transaction so a later rollback (failed device insert, consumed
    /// pairing, etc.) does not burn a daily slot.
    ///
    /// Increments only when the current count is under the cap. Hitting the
    /// cap returns 403 without changing the stored count.
    pub async fn record_pairing_created_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        user_id: &str,
        quota: &UserQuota,
    ) -> Result<(), ApiError> {
        let today = Utc::now().date_naive();
        let id = Uuid::new_v4().to_string();

        sqlx::query(
            r#"
            INSERT INTO user_pairing_usage (id, user_id, usage_date, pairings_created)
            VALUES ($1, $2, $3, 0)
            ON CONFLICT (user_id, usage_date) DO NOTHING
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(today)
        .execute(&mut **tx)
        .await?;

        let created: Option<i64> = sqlx::query_scalar(
            r#"
            UPDATE user_pairing_usage
            SET pairings_created = pairings_created + 1,
                updated_at = CURRENT_TIMESTAMP
            WHERE user_id = $1 AND usage_date = $2 AND pairings_created < $3
            RETURNING pairings_created
            "#,
        )
        .bind(user_id)
        .bind(today)
        .bind(quota.max_pairings_per_day)
        .fetch_optional(&mut **tx)
        .await?;

        if created.is_some() {
            return Ok(());
        }

        let current: i64 = sqlx::query_scalar(
            "SELECT pairings_created FROM user_pairing_usage WHERE user_id = $1 AND usage_date = $2",
        )
        .bind(user_id)
        .bind(today)
        .fetch_one(&mut **tx)
        .await?;
        Err(ApiError::Forbidden(format!(
            "Daily pairing limit reached ({}/{}). Try again tomorrow or upgrade your plan.",
            current, quota.max_pairings_per_day
        )))
    }

    /// Record a pairing approval. Used to distinguish created vs approved counts.
    pub async fn record_pairing_approved(&self, user_id: &str) -> Result<(), ApiError> {
        let today = Utc::now().date_naive();
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO user_pairing_usage (id, user_id, usage_date, pairings_approved)
            VALUES ($1, $2, $3, 1)
            ON CONFLICT(user_id, usage_date) DO UPDATE SET
                pairings_approved = user_pairing_usage.pairings_approved + 1,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(today)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Count currently open relay sockets across all of a user's runtimes.
    pub async fn count_open_relay_sockets(&self, user_id: &str) -> Result<i64, ApiError> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM runtime_devices d
            JOIN runtime_relay_sockets s ON s.runtime_id = d.id
            WHERE d.user_id = $1 AND s.closed_at IS NULL
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;
        Ok(count)
    }

    /// Enforce the daily relay socket cap and the concurrent socket cap.
    pub async fn check_relay_socket_allowed(
        &self,
        user_id: &str,
        quota: &UserQuota,
    ) -> Result<(), ApiError> {
        let today = Utc::now().date_naive();
        let id = Uuid::new_v4().to_string();

        let opened: i64 = sqlx::query_scalar(
            "SELECT sockets_opened FROM user_relay_usage WHERE user_id = $1 AND usage_date = $2",
        )
        .bind(user_id)
        .bind(today)
        .fetch_optional(&self.db)
        .await?
        .unwrap_or(0);

        if opened >= quota.max_relay_sockets {
            return Err(ApiError::Forbidden(format!(
                "Daily relay socket limit reached ({}/{}). Upgrade your plan for more.",
                opened, quota.max_relay_sockets
            )));
        }

        sqlx::query(
            r#"
            INSERT INTO user_relay_usage (id, user_id, usage_date, sockets_opened)
            VALUES ($1, $2, $3, 1)
            ON CONFLICT(user_id, usage_date) DO UPDATE SET
                sockets_opened = user_relay_usage.sockets_opened + 1,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(today)
        .execute(&self.db)
        .await?;

        let concurrent = self.count_open_relay_sockets(user_id).await?;
        if concurrent >= quota.max_relay_sockets {
            return Err(ApiError::Forbidden(format!(
                "Concurrent relay socket limit reached ({}/{}). Close another socket or upgrade your plan.",
                concurrent, quota.max_relay_sockets
            )));
        }
        Ok(())
    }

    /// Record that a relay socket opened. Returns the socket record id so the
    /// caller can close it later.
    pub async fn open_relay_socket(
        &self,
        runtime_id: &str,
        socket_path: &str,
    ) -> Result<String, ApiError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO runtime_relay_sockets (id, runtime_id, socket_path) VALUES ($1, $2, $3)",
        )
        .bind(&id)
        .bind(runtime_id)
        .bind(socket_path)
        .execute(&self.db)
        .await?;
        Ok(id)
    }

    /// Record that a relay socket closed and add its egress to usage.
    pub async fn close_relay_socket(
        &self,
        socket_id: &str,
        egress_bytes: i64,
    ) -> Result<(), ApiError> {
        sqlx::query(
            "UPDATE runtime_relay_sockets SET closed_at = CURRENT_TIMESTAMP, egress_bytes = $1 WHERE id = $2",
        )
        .bind(egress_bytes)
        .bind(socket_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Record egress bytes for a user and enforce the daily bandwidth cap.
    pub async fn record_relay_egress(
        &self,
        user_id: &str,
        quota: &UserQuota,
        bytes: i64,
    ) -> Result<(), ApiError> {
        if bytes <= 0 {
            return Ok(());
        }
        let today = Utc::now().date_naive();
        let id = Uuid::new_v4().to_string();

        sqlx::query(
            r#"
            INSERT INTO user_relay_usage (id, user_id, usage_date, egress_bytes)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT(user_id, usage_date) DO UPDATE SET
                egress_bytes = user_relay_usage.egress_bytes + excluded.egress_bytes,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&id)
        .bind(user_id)
        .bind(today)
        .bind(bytes)
        .execute(&self.db)
        .await?;

        let total_bytes: i64 = sqlx::query_scalar(
            "SELECT egress_bytes FROM user_relay_usage WHERE user_id = $1 AND usage_date = $2",
        )
        .bind(user_id)
        .bind(today)
        .fetch_one(&self.db)
        .await?;

        let mb = total_bytes / (1024 * 1024);
        if mb > quota.max_relay_mb_per_day as i64 {
            return Err(ApiError::Forbidden(format!(
                "Daily relay bandwidth limit reached ({} MB / {} MB). Upgrade your plan for more.",
                mb, quota.max_relay_mb_per_day
            )));
        }
        Ok(())
    }

    /// Check whether the user has hit their hard spend cap.
    ///
    /// Prepaid credits take precedence over the plan spend cap: closed usage
    /// sessions already deducted their cost from the balance, so the only
    /// unbilled spend is what open sessions have accrued so far — block when
    /// that would overrun the remaining balance. Users without a credits row
    /// fall back to the plan's hard cap against month spend (run_costs plus
    /// relay egress plus hosted usage).
    pub async fn check_spend_cap(&self, user_id: &str, quota: &UserQuota) -> Result<(), ApiError> {
        let credit_balance: Option<f64> = sqlx::query_scalar(
            "SELECT balance_usd FROM user_credits WHERE user_id = $1"
        )
        .bind(user_id)
        .fetch_optional(&self.db)
        .await
        .unwrap_or(None);

        if let Some(credits) = credit_balance {
            let open_accrued = crate::services::open_session_accrued_cost(&self.db, user_id)
                .await
                .unwrap_or(0.0);
            if credits - open_accrued <= 0.0 {
                return Err(ApiError::Forbidden(format!(
                    "Credit balance exhausted (${:.2} remaining, ${:.2} accrued by running sessions). Add credits to continue.",
                    credits, open_accrued
                )));
            }
            debug!(
                user_id = %user_id,
                credits = %credits,
                open_accrued = %open_accrued,
                "Credit balance check passed"
            );
            return Ok(());
        }

        let cap = match quota.hard_spend_cap_usd {
            Some(cap) if cap > 0.0 => cap,
            _ => return Ok(()),
        };

        let now = Utc::now();
        let month_start = now
            .with_day(1)
            .unwrap_or(now)
            .with_hour(0)
            .unwrap_or(now)
            .with_minute(0)
            .unwrap_or(now)
            .with_second(0)
            .unwrap_or(now);

        let run_cost: f64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(SUM(rc.total_cost), 0)
            FROM run_costs rc
            JOIN runs r ON rc.run_id = r.id
            WHERE r.owner_id = $1 AND rc.started_at >= $2
            "#,
        )
        .bind(user_id)
        .bind(month_start)
        .fetch_one(&self.db)
        .await
        .unwrap_or(0.0);

        let relay_egress_mb: i64 = sqlx::query_scalar(
            r#"
            SELECT (COALESCE(SUM(u.egress_bytes), 0) / (1024 * 1024))::BIGINT
            FROM user_relay_usage u
            WHERE u.user_id = $1 AND u.usage_date >= $2
            "#,
        )
        .bind(user_id)
        .bind(month_start.date_naive())
        .fetch_one(&self.db)
        .await
        .unwrap_or(0);

        // Relay bandwidth is not directly billed today, but we include a nominal
        // $0.02/MB placeholder so a runaway socket cannot silently spend nothing.
        let relay_cost = relay_egress_mb as f64 * 0.02;
        let hosted_cost = crate::services::hosted_usage_summary(&self.db, user_id)
            .await
            .map(|summary| summary.estimated_cost_usd)
            .unwrap_or(0.0);
        let current = run_cost + relay_cost + hosted_cost;

        if current >= cap {
            return Err(ApiError::Forbidden(format!(
                "Monthly spend cap reached (${:.2} / ${:.2}). Contact support to raise the cap.",
                current, cap
            )));
        }
        debug!(
            user_id = %user_id,
            current = %current,
            cap = %cap,
            "Spend cap check passed"
        );
        Ok(())
    }

    /// True if the quota allows creating hosted runtimes.
    pub fn can_create_hosted_runtime(&self, quota: &UserQuota) -> bool {
        quota.can_create_hosted_runtime
    }

    /// Enforce the paid entitlement, per-user instance count, machine-size
    /// ceiling, and monthly runtime-hour allowance before provisioning.
    pub async fn check_hosted_runtime_creation(
        &self,
        user_id: &str,
        quota: &UserQuota,
        requested_memory_mb: i64,
    ) -> Result<(), ApiError> {
        if !quota.can_create_hosted_runtime || quota.max_hosted_runtimes <= 0 {
            return Err(ApiError::Forbidden(
                "Your plan does not include hosted runtimes. Upgrade to a hosted compute plan."
                    .to_string(),
            ));
        }
        if requested_memory_mb <= 0
            || requested_memory_mb > quota.max_hosted_runtime_memory_mb
            || !matches!(requested_memory_mb, 512 | 1024 | 2048)
        {
            return Err(ApiError::Forbidden(format!(
                "Your {} plan allows hosted runtimes up to {} MB.",
                quota.plan_tier_id, quota.max_hosted_runtime_memory_mb
            )));
        }

        let active_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM hosted_runtime_instances
            WHERE user_id = $1 AND status NOT IN ('destroying', 'destroyed')
            "#,
        )
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;
        if active_count >= quota.max_hosted_runtimes {
            return Err(ApiError::Forbidden(format!(
                "Hosted runtime limit reached ({}/{}). Destroy an existing runtime or upgrade your plan.",
                active_count, quota.max_hosted_runtimes
            )));
        }

        self.check_hosted_runtime_hours(user_id, quota).await
    }

    /// Prevent a stopped machine from starting once its monthly allowance is
    /// exhausted. A value of zero means the tier has no hosted entitlement,
    /// not unlimited usage.
    pub async fn check_hosted_runtime_hours(
        &self,
        user_id: &str,
        quota: &UserQuota,
    ) -> Result<(), ApiError> {
        if quota.max_hosted_runtime_hours_monthly <= 0 {
            return Err(ApiError::Forbidden(
                "Your plan has no hosted runtime hours available.".to_string(),
            ));
        }
        let usage = crate::services::hosted_usage_summary(&self.db, user_id).await?;
        let limit_seconds = quota.max_hosted_runtime_hours_monthly * 3600;
        if usage.total_seconds >= limit_seconds {
            return Err(ApiError::Forbidden(format!(
                "Monthly hosted runtime allowance reached ({} hours).",
                quota.max_hosted_runtime_hours_monthly
            )));
        }
        Ok(())
    }
}

#[derive(Debug, sqlx::FromRow)]
struct UserQuotaRow {
    user_id: String,
    plan_tier_id: String,
    max_active_devices: i64,
    max_pairings_per_day: i64,
    max_relay_sockets: i64,
    max_relay_mb_per_day: i64,
    max_hosted_runtime_hours_monthly: i64,
    can_create_hosted_runtime: bool,
    max_hosted_runtimes: i64,
    max_hosted_runtime_memory_mb: i64,
    #[sqlx(default)]
    hard_spend_cap_usd: Option<f64>,
}

impl From<UserQuotaRow> for UserQuota {
    fn from(row: UserQuotaRow) -> Self {
        Self {
            user_id: row.user_id,
            plan_tier_id: row.plan_tier_id,
            max_active_devices: row.max_active_devices,
            max_pairings_per_day: row.max_pairings_per_day,
            max_relay_sockets: row.max_relay_sockets,
            max_relay_mb_per_day: row.max_relay_mb_per_day,
            max_hosted_runtime_hours_monthly: row.max_hosted_runtime_hours_monthly,
            can_create_hosted_runtime: row.can_create_hosted_runtime,
            max_hosted_runtimes: row.max_hosted_runtimes,
            max_hosted_runtime_memory_mb: row.max_hosted_runtime_memory_mb,
            hard_spend_cap_usd: row.hard_spend_cap_usd,
        }
    }
}

/// Helper to share a quota service reference.
pub type SharedQuotaService = Arc<QuotaService>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal schema for the credit-balance branch of check_spend_cap:
    /// user_credits plus the open-session accrued-cost query's table.
    async fn test_pool() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS user_credits CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE user_credits (
                user_id TEXT PRIMARY KEY,
                balance_usd DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (balance_usd >= 0),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS hosted_runtime_usage_sessions CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE hosted_runtime_usage_sessions (
                id TEXT PRIMARY KEY,
                hosted_instance_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                ended_at TIMESTAMPTZ,
                duration_seconds BIGINT,
                cost_per_hour DOUBLE PRECISION NOT NULL DEFAULT 0,
                estimated_cost_usd DOUBLE PRECISION,
                stop_reason TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn quota_for(user_id: &str) -> UserQuota {
        UserQuota {
            user_id: user_id.to_string(),
            plan_tier_id: "pro".to_string(),
            max_active_devices: 5,
            max_pairings_per_day: 50,
            max_relay_sockets: 20,
            max_relay_mb_per_day: 5000,
            max_hosted_runtime_hours_monthly: 100,
            can_create_hosted_runtime: true,
            max_hosted_runtimes: 1,
            max_hosted_runtime_memory_mb: 1024,
            hard_spend_cap_usd: Some(100.0),
        }
    }

    #[tokio::test]
    async fn credit_user_is_blocked_when_open_sessions_overrun_the_balance() {
        let pool = test_pool().await;
        let quota_service = QuotaService::new(pool.clone());
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 10.0)")
            .execute(&pool)
            .await
            .unwrap();
        // Open session accruing $12 so far (2h at $6/h): more than the $10 balance.
        sqlx::query(
            "INSERT INTO hosted_runtime_usage_sessions (id, hosted_instance_id, user_id, started_at, cost_per_hour) VALUES ('s_1', 'hr_1', 'user_1', NOW() - INTERVAL '2 hours', 6.0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let result = quota_service
            .check_spend_cap("user_1", &quota_for("user_1"))
            .await;
        assert!(
            matches!(result, Err(ApiError::Forbidden(_))),
            "open session accruing more than the balance must block: {result:?}"
        );
    }

    #[tokio::test]
    async fn credit_user_with_no_open_sessions_is_allowed() {
        let pool = test_pool().await;
        let quota_service = QuotaService::new(pool.clone());
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 10.0)")
            .execute(&pool)
            .await
            .unwrap();
        // A closed session must not count: its cost already came out of the
        // balance when it closed.
        sqlx::query(
            "INSERT INTO hosted_runtime_usage_sessions (id, hosted_instance_id, user_id, started_at, ended_at, duration_seconds, cost_per_hour, estimated_cost_usd) VALUES ('s_1', 'hr_1', 'user_1', NOW() - INTERVAL '3 hours', NOW() - INTERVAL '1 hour', 7200, 6.0, 12.0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        quota_service
            .check_spend_cap("user_1", &quota_for("user_1"))
            .await
            .expect("balance 10 with no open sessions must be allowed");
    }

    #[tokio::test]
    async fn credit_user_at_exactly_zero_remaining_is_blocked() {
        let pool = test_pool().await;
        let quota_service = QuotaService::new(pool.clone());
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_1', 0.0)")
            .execute(&pool)
            .await
            .unwrap();

        let result = quota_service
            .check_spend_cap("user_1", &quota_for("user_1"))
            .await;
        assert!(
            matches!(result, Err(ApiError::Forbidden(_))),
            "a drained balance must block even with nothing running: {result:?}"
        );
    }

    async fn pairing_usage_pool() -> PgPool {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS user_pairing_usage CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE user_pairing_usage (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                usage_date DATE NOT NULL,
                pairings_created BIGINT NOT NULL DEFAULT 0,
                pairings_approved BIGINT NOT NULL DEFAULT 0,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE (user_id, usage_date)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn pairing_quota(user_id: &str, max_pairings_per_day: i64) -> UserQuota {
        let mut quota = quota_for(user_id);
        quota.max_pairings_per_day = max_pairings_per_day;
        quota
    }

    #[tokio::test]
    async fn the_runtime_cap_counts_machines_not_sign_ins() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS runtime_devices CASCADE").execute(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE runtime_devices (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, hostname TEXT, platform TEXT, revoked_at TIMESTAMPTZ)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Desktop on this Mac holds the one slot.
        sqlx::query("INSERT INTO runtime_devices (id, user_id, hostname, platform) VALUES ('rt_desktop', 'user_1', 'Joes-MacBook', 'darwin-arm64')")
            .execute(&pool)
            .await
            .unwrap();
        let service = QuotaService::new(pool.clone());
        let mut quota = quota_for("user_1");
        quota.max_active_devices = 1;

        // The CLI on the same Mac is the same machine.
        service
            .check_active_device_cap("user_1", &quota, Some("joes-macbook"), Some("darwin-arm64"))
            .await
            .expect("same machine shares the slot");
        // Another machine is over the cap.
        let other = service
            .check_active_device_cap("user_1", &quota, Some("vps-1"), Some("linux-x64"))
            .await;
        assert!(matches!(other, Err(ApiError::Forbidden(ref m)) if m.contains("1/1")), "{other:?}");
        // No hostname: its own machine.
        assert!(service.check_active_device_cap("user_1", &quota, None, None).await.is_err());
        // Two sign-ins on one Mac still count once.
        sqlx::query("INSERT INTO runtime_devices (id, user_id, hostname, platform) VALUES ('rt_cli', 'user_1', 'Joes-MacBook', 'darwin-arm64')")
            .execute(&pool)
            .await
            .unwrap();
        quota.max_active_devices = 2;
        service
            .check_active_device_cap("user_1", &quota, Some("vps-1"), Some("linux-x64"))
            .await
            .expect("second machine fits a 2-machine plan");
    }

    #[tokio::test]
    async fn pairing_quota_counts_only_successful_increments() {
        let pool = pairing_usage_pool().await;
        let quota_service = QuotaService::new(pool.clone());
        let quota = pairing_quota("user_1", 2);

        quota_service
            .record_pairing_created("user_1", &quota)
            .await
            .expect("first pairing under cap");
        quota_service
            .record_pairing_created("user_1", &quota)
            .await
            .expect("second pairing at cap");

        let denied = quota_service.record_pairing_created("user_1", &quota).await;
        assert!(
            matches!(denied, Err(ApiError::Forbidden(ref msg)) if msg.contains("2/2")),
            "third pairing must 403 at cap: {denied:?}"
        );

        let stored: i64 = sqlx::query_scalar(
            "SELECT pairings_created FROM user_pairing_usage WHERE user_id = 'user_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored, 2, "a rejected increment must not raise the stored count");
    }

    #[tokio::test]
    async fn pairing_quota_rolls_back_with_the_outer_transaction() {
        let pool = pairing_usage_pool().await;
        let quota = pairing_quota("user_1", 5);

        let mut tx = pool.begin().await.unwrap();
        QuotaService::record_pairing_created_in_tx(&mut tx, "user_1", &quota)
            .await
            .expect("increment inside txn");
        tx.rollback().await.unwrap();

        let stored: Option<i64> = sqlx::query_scalar(
            "SELECT pairings_created FROM user_pairing_usage WHERE user_id = 'user_1'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(
            stored.unwrap_or(0) == 0,
            "a rolled-back handshake must not consume a daily pairing slot, stored={stored:?}"
        );
    }
}

/// The machine a pairing comes from, as the cap query keys it
/// (`lower(hostname)|platform`), or None without a hostname.
pub(crate) fn machine_key(hostname: Option<&str>, platform: Option<&str>) -> Option<String> {
    let host = hostname.map(str::trim).filter(|h| !h.is_empty())?;
    Some(format!("{}|{}", host.to_lowercase(), platform.unwrap_or("")))
}

#[cfg(test)]
mod machine_key_tests {
    use super::machine_key;

    #[test]
    fn same_mac_is_one_machine_whatever_the_case() {
        assert_eq!(machine_key(Some("Joes-MacBook"), Some("darwin-arm64")), machine_key(Some("joes-macbook"), Some("darwin-arm64")));
        assert_ne!(machine_key(Some("a"), Some("darwin-arm64")), machine_key(Some("b"), Some("darwin-arm64")));
        assert_eq!(machine_key(Some("  "), Some("darwin-arm64")), None);
        assert_eq!(machine_key(None, None), None);
    }
}
