//! Cost tracking service
//!
//! Manages cost calculation, tracking, and budget monitoring for cloud runs.
//! Provides functionality to track costs per run, per user/tenant, and
//! send alerts when budget thresholds are exceeded.

use crate::error::ApiError;
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time::interval;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Cost rates for a specific provider/region/instance combination
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct CostRate {
    pub provider: String,
    pub region: String,
    pub instance_type: String,
    pub cost_per_hour: f64,
    pub storage_cost_per_gb_month: f64,
    pub transfer_cost_per_gb: f64,
    pub currency: String,
    pub effective_from: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Cost record for a specific run
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct RunCost {
    pub id: String,
    pub run_id: String,
    pub instance_cost: f64,
    pub storage_cost: f64,
    pub transfer_cost: f64,
    pub total_cost: f64,
    pub provider: Option<String>,
    pub region: Option<String>,
    pub instance_type: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<i64>,
    pub storage_gb: Option<f64>,
    pub transfer_gb: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Summary of costs for a run (for API responses)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunCostSummary {
    pub run_id: String,
    pub instance_cost: f64,
    pub storage_cost: f64,
    pub transfer_cost: f64,
    pub total_cost: f64,
    pub provider: Option<String>,
    pub region: Option<String>,
    pub instance_type: Option<String>,
    pub duration_seconds: Option<i64>,
    pub currency: String,
}

/// User cost budget and tracking
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct UserCostBudget {
    pub user_id: String,
    pub monthly_budget: f64,
    pub current_month_cost: f64,
    pub alert_threshold: f64,
    pub last_alert_at: Option<DateTime<Utc>>,
    pub alert_enabled: bool,
    pub currency: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Cost alert record
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct CostAlert {
    pub id: String,
    pub user_id: String,
    pub alert_type: AlertType,
    pub threshold_percent: Option<f64>,
    pub current_cost: f64,
    pub budget_amount: f64,
    pub message: String,
    pub sent_at: DateTime<Utc>,
}

/// Type of cost alert
///
/// Stored in `cost_alerts.alert_type`, a `text` column (there is no Postgres
/// enum type); without `type_name = "text"` sqlx binds it as a type named
/// `alerttype` and every alert insert failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    Threshold,
    OverBudget,
    ProjectedOver,
}

impl std::fmt::Display for AlertType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertType::Threshold => write!(f, "threshold"),
            AlertType::OverBudget => write!(f, "over_budget"),
            AlertType::ProjectedOver => write!(f, "projected_over"),
        }
    }
}

/// Cost summary for a user (aggregated)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCostSummary {
    pub user_id: String,
    pub current_month_cost: f64,
    pub monthly_budget: f64,
    pub budget_utilization_percent: f64,
    pub currency: String,
    pub run_count: i64,
    pub total_duration_seconds: i64,
}

/// Cost breakdown by provider/region
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct CostBreakdown {
    pub provider: String,
    pub region: String,
    pub instance_type: String,
    pub total_cost: f64,
    pub run_count: i64,
    pub total_duration_hours: f64,
}

/// Request to update user budget
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UpdateBudgetRequest {
    pub monthly_budget: Option<f64>,
    pub alert_threshold: Option<f64>,
    pub alert_enabled: Option<bool>,
}

/// Request to create/update cost rate
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SetCostRateRequest {
    pub provider: String,
    pub region: String,
    pub instance_type: String,
    pub cost_per_hour: f64,
    pub storage_cost_per_gb_month: Option<f64>,
    pub transfer_cost_per_gb: Option<f64>,
    pub currency: Option<String>,
}

/// `run_costs` dimensions a breakdown can group by (task G9).
pub const BREAKDOWN_GROUP_BY: [&str; 3] = ["provider", "region", "instance_type"];

/// Resolve a `YYYY-MM` month filter to `[start, end)` bounds. `None` means
/// the current calendar month with an open upper bound (month-to-date,
/// matching the pre-filter behavior).
fn month_bounds(month: &Option<String>) -> Result<(chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>), ApiError> {
    use chrono::{Datelike, TimeZone};
    match month {
        Some(raw) => {
            let start = chrono::NaiveDate::parse_from_str(&format!("{raw}-01"), "%Y-%m-%d")
                .map_err(|_| {
                    ApiError::BadRequest(format!(
                        "`month` must be YYYY-MM (got `{raw}`)."
                    ))
                })?;
            let start = chrono::Utc
                .from_utc_datetime(&start.and_hms_opt(0, 0, 0).unwrap());
            let end = if start.month() == 12 {
                chrono::Utc
                    .with_ymd_and_hms(start.year() + 1, 1, 1, 0, 0, 0)
                    .unwrap()
            } else {
                chrono::Utc
                    .with_ymd_and_hms(start.year(), start.month() + 1, 1, 0, 0, 0)
                    .unwrap()
            };
            Ok((start, Some(end)))
        }
        None => {
            let now = chrono::Utc::now();
            let start = now
                .with_day(1)
                .unwrap_or(now)
                .with_hour(0)
                .unwrap_or(now)
                .with_minute(0)
                .unwrap_or(now);
            Ok((start, None))
        }
    }
}

/// Cost service trait - defines the interface for cost tracking
#[async_trait]
pub trait CostService: Send + Sync {
    /// Initialize cost tracking for a run when it starts
    async fn init_run_cost(
        &self,
        run_id: &str,
        provider: &str,
        region: &str,
        instance_type: &str,
    ) -> Result<RunCost, ApiError>;

    /// Finalize cost tracking when a run ends
    async fn finalize_run_cost(
        &self,
        run_id: &str,
        storage_gb: Option<f64>,
        transfer_gb: Option<f64>,
    ) -> Result<RunCost, ApiError>;

    /// Update costs for a running instance (periodic update)
    async fn update_running_cost(&self, run_id: &str) -> Result<RunCost, ApiError>;

    /// Get cost for a specific run
    async fn get_run_cost(&self, run_id: &str) -> Result<RunCost, ApiError>;

    /// Get or initialize user budget
    async fn get_or_init_user_budget(&self, user_id: &str) -> Result<UserCostBudget, ApiError>;

    /// Update user budget settings
    async fn update_user_budget(
        &self,
        user_id: &str,
        request: UpdateBudgetRequest,
    ) -> Result<UserCostBudget, ApiError>;

    /// Get cost summary for a user. `month` (YYYY-MM) filters to that
    /// calendar month; `None` means the current month to date. Scoping
    /// follows the crate tenant pattern: a run is visible when
    /// `runs.tenant_id` (set from the authenticated user at creation) or the
    /// legacy `runs.owner_id` matches.
    async fn get_user_cost_summary(
        &self,
        user_id: &str,
        month: Option<String>,
    ) -> Result<UserCostSummary, ApiError>;

    /// Get cost breakdown for a user, honoring the same `month` filter as
    /// the summary. `group_by` (provider | region | instance_type) collapses
    /// the grouping to a single dimension — these are the existing
    /// `run_costs` dimensions; untagged spend stays grouped under "unknown".
    /// `None` keeps the legacy three-dimension grouping.
    async fn get_user_cost_breakdown(
        &self,
        user_id: &str,
        month: Option<String>,
        group_by: Option<String>,
    ) -> Result<Vec<CostBreakdown>, ApiError>;

    /// Check and send budget alerts for a user
    async fn check_budget_alerts(&self, user_id: &str) -> Result<Vec<CostAlert>, ApiError>;

    /// Get cost rate for a provider/region/instance
    async fn get_cost_rate(
        &self,
        provider: &str,
        region: &str,
        instance_type: &str,
    ) -> Result<CostRate, ApiError>;

    /// Set or update cost rate
    async fn set_cost_rate(&self, request: SetCostRateRequest) -> Result<CostRate, ApiError>;

    /// List all cost rates
    async fn list_cost_rates(&self) -> Result<Vec<CostRate>, ApiError>;

    /// Recalculate all costs for current month (admin operation)
    async fn recalculate_monthly_costs(&self) -> Result<(), ApiError>;

    /// Update current month costs for all users (called periodically)
    async fn update_monthly_costs(&self) -> Result<(), ApiError>;

    /// Get the current prepaid credit balance for a user.
    async fn get_credit_balance(&self, user_id: &str) -> Result<f64, ApiError>;

    /// Add prepaid credits to a user's balance (idempotent by transaction_id).
    async fn add_credits(
        &self,
        user_id: &str,
        amount_usd: f64,
        transaction_id: &str,
        source: &str,
    ) -> Result<f64, ApiError>;

    /// Deduct credits from a user's balance. Returns the new balance.
    /// Errors if the balance is insufficient.
    async fn deduct_credits(&self, user_id: &str, amount_usd: f64) -> Result<f64, ApiError>;

    /// Deduct credits from a user's balance for an arbitrary billable usage
    /// reference. `transaction_id` = `ref_id`, so a replayed reference is a
    /// no-op; the balance clamps at zero. Users without a `user_credits` row
    /// (plan-cap enforcement) are a no-op success. Returns the balance.
    async fn deduct_credits_for_usage(
        &self,
        user_id: &str,
        ref_id: &str,
        amount_usd: f64,
        source: &str,
    ) -> Result<f64, ApiError>;

    /// Deduct the finalized cost of one hosted runtime usage session.
    ///
    /// Atomic and idempotent: a single DB transaction writes a negative
    /// `credit_transactions` ledger row keyed by `hosted-session-{session_id}`
    /// (ON CONFLICT DO NOTHING — a replay is a no-op) and clamps the balance
    /// at zero. Users without a `user_credits` row (plan-cap enforcement) are
    /// a no-op success. Returns the resulting balance.
    async fn deduct_credits_for_session(
        &self,
        user_id: &str,
        session_id: &str,
        amount_usd: f64,
    ) -> Result<f64, ApiError>;

    /// Grant one subscription plan's monthly credits with a rollover cap.
    ///
    /// Atomic and idempotent: a single DB transaction writes a
    /// `credit_transactions` ledger row keyed by `stripe-invoice-{invoice_id}`
    /// (ON CONFLICT DO NOTHING — a Stripe retry is a no-op) and applies the
    /// rollover math `LEAST(current_balance, rollover_cap) + monthly_credits`
    /// to the balance. Users without a `user_credits` row start fresh with the
    /// full monthly grant (the cap only clips rolled-over remainder). Returns
    /// the resulting balance.
    async fn grant_subscription_credits(
        &self,
        user_id: &str,
        invoice_id: &str,
        amount_usd: f64,
        rollover_cap_usd: f64,
    ) -> Result<f64, ApiError>;

    /// Debit credits for an explicit wallet transfer (e.g. moving balance to
    /// an organization's fabric compute ledger).
    ///
    /// Atomic and idempotent: `transaction_id` keys the ledger row (ON
    /// CONFLICT DO NOTHING — a replayed transfer is a no-op). Unlike usage
    /// deductions this does NOT clamp at zero: the transfer is refused with
    /// `PaymentRequired` when the balance cannot cover it, so money is never
    /// created or destroyed by the bridge. Returns the resulting balance.
    async fn debit_credits_for_transfer(
        &self,
        user_id: &str,
        amount_usd: f64,
        transaction_id: &str,
        source: &str,
    ) -> Result<f64, ApiError>;
}

/// Implementation of CostService using SQLite
pub struct CostServiceImpl {
    db: PgPool,
}

impl CostServiceImpl {
    /// Create a new CostServiceImpl
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Create from an existing pool reference
    pub fn from_arc(db: Arc<PgPool>) -> Self {
        Self { db: (*db).clone() }
    }

    /// Calculate instance cost based on duration and rate
    fn calculate_instance_cost(&self, duration_seconds: i64, cost_per_hour: f64) -> f64 {
        let hours = duration_seconds as f64 / 3600.0;
        hours * cost_per_hour
    }

    /// Calculate storage cost based on usage and duration
    fn calculate_storage_cost(
        &self,
        storage_gb: f64,
        duration_seconds: i64,
        cost_per_gb_month: f64,
    ) -> f64 {
        // Convert duration to fraction of month (approximate 30 days)
        let month_fraction = duration_seconds as f64 / (30.0 * 24.0 * 3600.0);
        storage_gb * month_fraction * cost_per_gb_month
    }

    /// Calculate transfer cost
    fn calculate_transfer_cost(&self, transfer_gb: f64, cost_per_gb: f64) -> f64 {
        transfer_gb * cost_per_gb
    }

    /// Send alert notification (placeholder for actual notification logic)
    async fn send_alert(&self, alert: &CostAlert) -> Result<(), ApiError> {
        // TODO: Integrate with notification service (email, webhook, etc.)
        info!(
            "Cost alert for user {}: {} - {}",
            alert.user_id, alert.alert_type, alert.message
        );
        Ok(())
    }
}

#[async_trait]
impl CostService for CostServiceImpl {
    async fn init_run_cost(
        &self,
        run_id: &str,
        provider: &str,
        region: &str,
        instance_type: &str,
    ) -> Result<RunCost, ApiError> {
        let cost_id = Uuid::new_v4().to_string();
        let now = Utc::now();

        // Check if cost record already exists for this run
        let existing: Option<RunCost> = sqlx::query_as("SELECT * FROM run_costs WHERE run_id = $1")
            .bind(run_id)
            .fetch_optional(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

        if let Some(cost) = existing {
            // Update existing record
            let updated = sqlx::query_as::<_, RunCost>(
                r#"
                UPDATE run_costs 
                SET provider = $1, region = $2, instance_type = $3, started_at = $4, updated_at = $5
                WHERE id = $6
                RETURNING *
                "#,
            )
            .bind(provider)
            .bind(region)
            .bind(instance_type)
            .bind(now)
            .bind(now)
            .bind(cost.id)
            .fetch_one(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

            return Ok(updated);
        }

        // Create new cost record
        let cost = sqlx::query_as::<_, RunCost>(
            r#"
            INSERT INTO run_costs (
                id, run_id, instance_cost, storage_cost, transfer_cost, total_cost,
                provider, region, instance_type, started_at, ended_at, duration_seconds,
                storage_gb, transfer_gb, created_at, updated_at
            ) VALUES ($1, $2, 0, 0, 0, 0, $3, $4, $5, $6, NULL, NULL, NULL, NULL, $7, $8)
            RETURNING *
            "#,
        )
        .bind(&cost_id)
        .bind(run_id)
        .bind(provider)
        .bind(region)
        .bind(instance_type)
        .bind(now)
        .bind(now)
        .bind(now)
        .fetch_one(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        info!(
            "Initialized cost tracking for run {} with {} {} in {}",
            run_id, provider, instance_type, region
        );

        Ok(cost)
    }

    async fn finalize_run_cost(
        &self,
        run_id: &str,
        storage_gb: Option<f64>,
        transfer_gb: Option<f64>,
    ) -> Result<RunCost, ApiError> {
        let now = Utc::now();

        // Get the cost record
        let cost: RunCost = sqlx::query_as("SELECT * FROM run_costs WHERE run_id = $1")
            .bind(run_id)
            .fetch_optional(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?
            .ok_or_else(|| {
                ApiError::NotFound(format!("Cost record not found for run: {}", run_id))
            })?;

        // Get cost rate
        let rate_result = if let (Some(provider), Some(region), Some(instance_type)) =
            (&cost.provider, &cost.region, &cost.instance_type)
        {
            self.get_cost_rate(provider, region, instance_type)
                .await
                .ok()
        } else {
            None
        };

        // Calculate duration
        let started_at = cost.started_at.unwrap_or(now);
        let duration_seconds = (now - started_at).num_seconds();

        // Calculate costs
        let instance_cost = if let Some(ref rate) = rate_result {
            self.calculate_instance_cost(duration_seconds, rate.cost_per_hour)
        } else {
            cost.instance_cost
        };

        let storage_cost = if let (Some(storage), Some(ref rate)) = (storage_gb, &rate_result) {
            self.calculate_storage_cost(storage, duration_seconds, rate.storage_cost_per_gb_month)
        } else {
            cost.storage_cost
        };

        let transfer_cost = if let (Some(transfer), Some(ref rate)) = (transfer_gb, &rate_result) {
            self.calculate_transfer_cost(transfer, rate.transfer_cost_per_gb)
        } else {
            cost.transfer_cost
        };

        let total_cost = instance_cost + storage_cost + transfer_cost;

        // Update the cost record
        let updated = sqlx::query_as::<_, RunCost>(
            r#"
            UPDATE run_costs 
            SET instance_cost = $1, storage_cost = $2, transfer_cost = $3, total_cost = $4,
                ended_at = $5, duration_seconds = $6, storage_gb = $7, transfer_gb = $8, updated_at = $9
            WHERE run_id = $10
            RETURNING *
            "#,
        )
        .bind(instance_cost)
        .bind(storage_cost)
        .bind(transfer_cost)
        .bind(total_cost)
        .bind(now)
        .bind(duration_seconds)
        .bind(storage_gb)
        .bind(transfer_gb)
        .bind(now)
        .bind(run_id)
        .fetch_one(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        // Update user monthly costs
        self.update_monthly_costs().await?;

        info!(
            "Finalized cost tracking for run {}: total_cost=${:.4}",
            run_id, total_cost
        );

        Ok(updated)
    }

    async fn update_running_cost(&self, run_id: &str) -> Result<RunCost, ApiError> {
        let now = Utc::now();

        // Get the cost record
        let cost: RunCost = sqlx::query_as("SELECT * FROM run_costs WHERE run_id = $1")
            .bind(run_id)
            .fetch_optional(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?
            .ok_or_else(|| {
                ApiError::NotFound(format!("Cost record not found for run: {}", run_id))
            })?;

        // Only update if started and not ended
        if cost.started_at.is_none() || cost.ended_at.is_some() {
            return Ok(cost);
        }

        // Get cost rate
        let rate_result = if let (Some(provider), Some(region), Some(instance_type)) =
            (&cost.provider, &cost.region, &cost.instance_type)
        {
            self.get_cost_rate(provider, region, instance_type)
                .await
                .ok()
        } else {
            None
        };

        // Calculate current duration
        let started_at = cost.started_at.unwrap();
        let duration_seconds = (now - started_at).num_seconds();

        // Calculate current costs
        let instance_cost = if let Some(ref rate) = rate_result {
            self.calculate_instance_cost(duration_seconds, rate.cost_per_hour)
        } else {
            cost.instance_cost
        };

        let storage_cost = if let (Some(storage), Some(ref rate)) = (cost.storage_gb, &rate_result)
        {
            self.calculate_storage_cost(storage, duration_seconds, rate.storage_cost_per_gb_month)
        } else {
            cost.storage_cost
        };

        let total_cost = instance_cost + storage_cost + cost.transfer_cost;

        // Update the cost record
        let updated = sqlx::query_as::<_, RunCost>(
            r#"
            UPDATE run_costs 
            SET instance_cost = $1, storage_cost = $2, total_cost = $3, duration_seconds = $4, updated_at = $5
            WHERE run_id = $6
            RETURNING *
            "#
        )
        .bind(instance_cost)
        .bind(storage_cost)
        .bind(total_cost)
        .bind(duration_seconds)
        .bind(now)
        .bind(run_id)
        .fetch_one(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        Ok(updated)
    }

    async fn get_run_cost(&self, run_id: &str) -> Result<RunCost, ApiError> {
        let cost: Option<RunCost> = sqlx::query_as("SELECT * FROM run_costs WHERE run_id = $1")
            .bind(run_id)
            .fetch_optional(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

        cost.ok_or_else(|| ApiError::NotFound(format!("Cost record not found for run: {}", run_id)))
    }

    async fn get_or_init_user_budget(&self, user_id: &str) -> Result<UserCostBudget, ApiError> {
        // Try to get existing budget
        let budget: Option<UserCostBudget> =
            sqlx::query_as("SELECT * FROM user_cost_budgets WHERE user_id = $1")
                .bind(user_id)
                .fetch_optional(&self.db)
                .await
                .map_err(ApiError::DatabaseError)?;

        if let Some(budget) = budget {
            return Ok(budget);
        }

        // Create default budget
        let now = Utc::now();
        let budget = sqlx::query_as::<_, UserCostBudget>(
            r#"
            INSERT INTO user_cost_budgets (
                user_id, monthly_budget, current_month_cost, alert_threshold, 
                last_alert_at, alert_enabled, currency, created_at, updated_at
            ) VALUES ($1, 100.0, 0.0, 80.0, NULL, true, 'USD', $2, $3)
            RETURNING *
            "#,
        )
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        info!("Initialized default budget for user: {}", user_id);

        Ok(budget)
    }

    async fn update_user_budget(
        &self,
        user_id: &str,
        request: UpdateBudgetRequest,
    ) -> Result<UserCostBudget, ApiError> {
        let now = Utc::now();

        // Build update query dynamically
        let mut updates = Vec::new();

        if let Some(budget) = request.monthly_budget {
            updates.push(format!("monthly_budget = {}", budget));
        }
        if let Some(threshold) = request.alert_threshold {
            updates.push(format!("alert_threshold = {}", threshold));
        }
        if let Some(enabled) = request.alert_enabled {
            updates.push(format!("alert_enabled = {}", if enabled { "true" } else { "false" }));
        }

        if updates.is_empty() {
            return self.get_or_init_user_budget(user_id).await;
        }

        updates.push(format!("updated_at = '{}'", now.to_rfc3339()));

        let query = format!(
            "UPDATE user_cost_budgets SET {} WHERE user_id = $1 RETURNING *",
            updates.join(", ")
        );

        let budget = sqlx::query_as::<_, UserCostBudget>(&query)
            .bind(user_id)
            .fetch_one(&self.db)
            .await
            .map_err(|e| {
                if matches!(e, sqlx::Error::RowNotFound) {
                    ApiError::NotFound(format!("User budget not found: {}", user_id))
                } else {
                    ApiError::DatabaseError(e)
                }
            })?;

        Ok(budget)
    }

    async fn get_user_cost_summary(
        &self,
        user_id: &str,
        month: Option<String>,
    ) -> Result<UserCostSummary, ApiError> {
        let budget = self.get_or_init_user_budget(user_id).await?;
        let (month_start, month_end) = month_bounds(&month)?;

        // Aggregate costs for this user. Tenant scoping follows the crate
        // pattern (runs.rs): runs carry `tenant_id` = creating user; the
        // legacy `owner_id` column still matches for older rows.
        let result: Option<(f64, i64, i64)> = sqlx::query_as(
            r#"
            SELECT
                COALESCE(SUM(rc.total_cost), 0) as total_cost,
                COUNT(rc.id) as run_count,
                COALESCE(SUM(rc.duration_seconds), 0)::BIGINT as total_duration
            FROM run_costs rc
            JOIN runs r ON rc.run_id = r.id
            WHERE (r.tenant_id = $1 OR r.owner_id = $1)
            AND rc.started_at >= $2
            AND ($3::timestamptz IS NULL OR rc.started_at < $3)
            "#,
        )
        .bind(user_id)
        .bind(month_start)
        .bind(month_end)
        .fetch_optional(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        let (current_cost, run_count, total_duration) = result.unwrap_or((0.0, 0, 0));

        // Update current_month_cost in budget
        sqlx::query("UPDATE user_cost_budgets SET current_month_cost = $1 WHERE user_id = $2")
            .bind(current_cost)
            .bind(user_id)
            .execute(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

        let utilization = if budget.monthly_budget > 0.0 {
            (current_cost / budget.monthly_budget) * 100.0
        } else {
            0.0
        };

        Ok(UserCostSummary {
            user_id: user_id.to_string(),
            current_month_cost: current_cost,
            monthly_budget: budget.monthly_budget,
            budget_utilization_percent: utilization,
            currency: budget.currency,
            run_count,
            total_duration_seconds: total_duration,
        })
    }

    async fn get_user_cost_breakdown(
        &self,
        user_id: &str,
        month: Option<String>,
        group_by: Option<String>,
    ) -> Result<Vec<CostBreakdown>, ApiError> {
        let (month_start, month_end) = month_bounds(&month)?;

        // Whitelist-validated dimension; builds e.g.
        // `COALESCE(rc.provider, 'unknown') AS provider` plus `'all'` for
        // the non-selected dimensions.
        let (group_expr, other) = match group_by.as_deref() {
            None => (
                "COALESCE(rc.provider, 'unknown') as provider,
                 COALESCE(rc.region, 'unknown') as region,
                 COALESCE(rc.instance_type, 'unknown') as instance_type"
                    .to_string(),
                None,
            ),
            Some(dim) if BREAKDOWN_GROUP_BY.contains(&dim) => {
                // Keep the SELECT column order provider, region,
                // instance_type regardless of which dimension is selected;
                // non-selected dimensions report the literal "all".
                let ordered = BREAKDOWN_GROUP_BY
                    .iter()
                    .map(|col| {
                        if *col == dim {
                            format!("COALESCE(rc.{col}, 'unknown') as {col}")
                        } else {
                            format!("'all' as {col}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                (ordered, Some(dim.to_string()))
            }
            Some(dim) => {
                return Err(ApiError::BadRequest(format!(
                    "`group_by` must be one of: {} (got `{dim}`).",
                    BREAKDOWN_GROUP_BY.join(", ")
                )))
            }
        };

        let group_clause = other
            .as_deref()
            .map(|dim| format!("GROUP BY rc.{dim}"))
            .unwrap_or_else(|| {
                "GROUP BY rc.provider, rc.region, rc.instance_type".to_string()
            });

        let sql = format!(
            r#"
            SELECT
                {group_expr},
                COALESCE(SUM(rc.total_cost), 0) as total_cost,
                COUNT(rc.id) as run_count,
                COALESCE(SUM(rc.duration_seconds), 0)::DOUBLE PRECISION / 3600.0 as total_duration_hours
            FROM run_costs rc
            JOIN runs r ON rc.run_id = r.id
            WHERE (r.tenant_id = $1 OR r.owner_id = $1)
            AND rc.started_at >= $2
            AND ($3::timestamptz IS NULL OR rc.started_at < $3)
            {group_clause}
            ORDER BY total_cost DESC
            "#
        );

        let breakdown: Vec<CostBreakdown> = sqlx::query_as(&sql)
            .bind(user_id)
            .bind(month_start)
            .bind(month_end)
            .fetch_all(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

        Ok(breakdown)
    }

    async fn check_budget_alerts(&self, user_id: &str) -> Result<Vec<CostAlert>, ApiError> {
        let budget = self.get_or_init_user_budget(user_id).await?;
        let summary = self.get_user_cost_summary(user_id, None).await?;

        if !budget.alert_enabled || budget.monthly_budget <= 0.0 {
            return Ok(Vec::new());
        }

        let utilization_percent = summary.budget_utilization_percent;
        let threshold = budget.alert_threshold;

        let mut alerts = Vec::new();

        // Check if we should send a threshold alert
        if utilization_percent >= threshold {
            // The event backbone gets one `usage.threshold` per crossing (push, MCP
            // subscribers), independent of the 24 h cost_alerts cadence below.
            emit_usage_threshold(&self.db, user_id, budget.monthly_budget, summary.current_month_cost, threshold, utilization_percent, Utc::now()).await;
            // Check if we've already sent an alert recently (within last 24 hours)
            let should_alert = if let Some(last_alert) = budget.last_alert_at {
                (Utc::now() - last_alert) > Duration::hours(24)
            } else {
                true
            };

            if should_alert {
                let alert_type = if utilization_percent >= 100.0 {
                    AlertType::OverBudget
                } else {
                    AlertType::Threshold
                };

                let message = if utilization_percent >= 100.0 {
                    format!(
                        "You have exceeded your monthly budget of ${:.2}. Current cost: ${:.2} ({:.1}%)",
                        budget.monthly_budget, summary.current_month_cost, utilization_percent
                    )
                } else {
                    format!(
                        "You have reached {:.1}% of your monthly budget of ${:.2}. Current cost: ${:.2}",
                        utilization_percent, budget.monthly_budget, summary.current_month_cost
                    )
                };

                let alert_id = Uuid::new_v4().to_string();
                let alert: CostAlert = sqlx::query_as(
                    r#"
                    INSERT INTO cost_alerts (id, user_id, alert_type, threshold_percent, current_cost, budget_amount, message, sent_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    RETURNING *
                    "#
                )
                .bind(&alert_id)
                .bind(user_id)
                .bind(alert_type)
                .bind(threshold)
                .bind(summary.current_month_cost)
                .bind(budget.monthly_budget)
                .bind(&message)
                .bind(Utc::now())
                .fetch_one(&self.db)
                .await
                .map_err(ApiError::DatabaseError)?;

                // Update last_alert_at
                sqlx::query("UPDATE user_cost_budgets SET last_alert_at = $1 WHERE user_id = $2")
                    .bind(Utc::now())
                    .bind(user_id)
                    .execute(&self.db)
                    .await
                    .map_err(ApiError::DatabaseError)?;

                // Send the alert
                self.send_alert(&alert).await?;

                alerts.push(alert);
            }
        }

        Ok(alerts)
    }

    async fn get_cost_rate(
        &self,
        provider: &str,
        region: &str,
        instance_type: &str,
    ) -> Result<CostRate, ApiError> {
        let rate: Option<CostRate> = sqlx::query_as(
            "SELECT * FROM cost_rates WHERE provider = $1 AND region = $2 AND instance_type = $3",
        )
        .bind(provider)
        .bind(region)
        .bind(instance_type)
        .fetch_optional(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        rate.ok_or_else(|| {
            ApiError::NotFound(format!(
                "Cost rate not found for {}/{}/{}",
                provider, region, instance_type
            ))
        })
    }

    async fn set_cost_rate(&self, request: SetCostRateRequest) -> Result<CostRate, ApiError> {
        let now = Utc::now();

        let rate = sqlx::query_as::<_, CostRate>(
            r#"
            INSERT INTO cost_rates (
                provider, region, instance_type, cost_per_hour, storage_cost_per_gb_month,
                transfer_cost_per_gb, currency, effective_from, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT(provider, region, instance_type) DO UPDATE SET
                cost_per_hour = excluded.cost_per_hour,
                storage_cost_per_gb_month = excluded.storage_cost_per_gb_month,
                transfer_cost_per_gb = excluded.transfer_cost_per_gb,
                currency = excluded.currency,
                updated_at = excluded.updated_at
            RETURNING *
            "#,
        )
        .bind(&request.provider)
        .bind(&request.region)
        .bind(&request.instance_type)
        .bind(request.cost_per_hour)
        .bind(request.storage_cost_per_gb_month.unwrap_or(0.0))
        .bind(request.transfer_cost_per_gb.unwrap_or(0.0))
        .bind(request.currency.as_deref().unwrap_or("USD"))
        .bind(now)
        .bind(now)
        .bind(now)
        .fetch_one(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        info!(
            "Updated cost rate for {}/{}/{}: ${:.4}/hour",
            request.provider, request.region, request.instance_type, request.cost_per_hour
        );

        Ok(rate)
    }

    async fn list_cost_rates(&self) -> Result<Vec<CostRate>, ApiError> {
        let rates: Vec<CostRate> =
            sqlx::query_as("SELECT * FROM cost_rates ORDER BY provider, region, instance_type")
                .fetch_all(&self.db)
                .await
                .map_err(ApiError::DatabaseError)?;

        Ok(rates)
    }

    async fn recalculate_monthly_costs(&self) -> Result<(), ApiError> {
        let now = Utc::now();
        let month_start = now
            .with_day(1)
            .unwrap_or(now)
            .with_hour(0)
            .unwrap_or(now)
            .with_minute(0)
            .unwrap_or(now);

        // Get all users with budgets
        let budgets: Vec<UserCostBudget> = sqlx::query_as("SELECT * FROM user_cost_budgets")
            .fetch_all(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

        for budget in budgets {
            let result: Option<(f64,)> = sqlx::query_as(
                r#"
                SELECT COALESCE(SUM(rc.total_cost), 0)
                FROM run_costs rc
                JOIN runs r ON rc.run_id = r.id
                WHERE r.owner_id = $1 
                AND rc.started_at >= $2
                "#,
            )
            .bind(&budget.user_id)
            .bind(month_start)
            .fetch_optional(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;

            if let Some((total_cost,)) = result {
                sqlx::query(
                    "UPDATE user_cost_budgets SET current_month_cost = $1 WHERE user_id = $2",
                )
                .bind(total_cost)
                .bind(&budget.user_id)
                .execute(&self.db)
                .await
                .map_err(ApiError::DatabaseError)?;

                debug!(
                    "Recalculated monthly cost for user {}: ${:.2}",
                    budget.user_id, total_cost
                );
            }
        }

        Ok(())
    }

    async fn update_monthly_costs(&self) -> Result<(), ApiError> {
        // This is a lightweight version that just recalculates
        self.recalculate_monthly_costs().await
    }

    async fn get_credit_balance(&self, user_id: &str) -> Result<f64, ApiError> {
        let balance: Option<f64> = sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await
            .map_err(ApiError::DatabaseError)?;
        Ok(balance.unwrap_or(0.0))
    }

    async fn add_credits(
        &self,
        user_id: &str,
        amount_usd: f64,
        transaction_id: &str,
        source: &str,
    ) -> Result<f64, ApiError> {
        if amount_usd <= 0.0 {
            return Err(ApiError::BadRequest("Credit amount must be positive.".to_string()));
        }

        let mut tx = self.db.begin().await.map_err(ApiError::DatabaseError)?;

        // Idempotency: replaying the same transaction is a no-op.
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT transaction_id FROM credit_transactions WHERE transaction_id = $1"
        )
        .bind(transaction_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        if existing.is_some() {
            tx.commit().await.map_err(ApiError::DatabaseError)?;
            return self.get_credit_balance(user_id).await;
        }

        sqlx::query(
            r#"
            INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(amount_usd)
        .bind(transaction_id)
        .bind(source)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        sqlx::query(
            r#"
            INSERT INTO user_credits (user_id, balance_usd)
            VALUES ($1, $2)
            ON CONFLICT(user_id) DO UPDATE SET
                balance_usd = user_credits.balance_usd + excluded.balance_usd
            "#
        )
        .bind(user_id)
        .bind(amount_usd)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        tx.commit().await.map_err(ApiError::DatabaseError)?;

        info!("Added ${:.2} credits to user {} from {}", amount_usd, user_id, source);
        self.get_credit_balance(user_id).await
    }

    async fn deduct_credits(&self, user_id: &str, amount_usd: f64) -> Result<f64, ApiError> {
        if amount_usd <= 0.0 {
            return self.get_credit_balance(user_id).await;
        }

        let mut tx = self.db.begin().await.map_err(ApiError::DatabaseError)?;

        let current: Option<f64> = sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(ApiError::DatabaseError)?;

        let current = current.unwrap_or(0.0);
        if current < amount_usd {
            return Err(ApiError::Forbidden(format!(
                "Insufficient credits: ${:.2} available, ${:.2} required.",
                current, amount_usd
            )));
        }

        sqlx::query("UPDATE user_credits SET balance_usd = balance_usd - $1 WHERE user_id = $2")
            .bind(amount_usd)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::DatabaseError)?;

        tx.commit().await.map_err(ApiError::DatabaseError)?;

        let new_balance = self.get_credit_balance(user_id).await?;
        info!("Deducted ${:.2} credits from user {}; new balance ${:.2}", amount_usd, user_id, new_balance);
        Ok(new_balance)
    }

    async fn deduct_credits_for_usage(
        &self,
        user_id: &str,
        ref_id: &str,
        amount_usd: f64,
        source: &str,
    ) -> Result<f64, ApiError> {
        if amount_usd <= 0.0 {
            return self.get_credit_balance(user_id).await;
        }

        let mut tx = self.db.begin().await.map_err(ApiError::DatabaseError)?;

        // Idempotency: the reference id is the deduction key, so a replayed
        // usage (repeated stop, retried settlement, ...) conflicts and touches
        // nothing.
        let ledger = sqlx::query(
            r#"
            INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (transaction_id) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(-amount_usd)
        .bind(ref_id)
        .bind(source)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        if ledger.rows_affected() == 0 {
            tx.commit().await.map_err(ApiError::DatabaseError)?;
            return self.get_credit_balance(user_id).await;
        }

        // Atomic clamp at zero: a balance never goes negative, and users
        // without a credits row (plan-cap enforcement) are a no-op success.
        sqlx::query(
            "UPDATE user_credits SET balance_usd = GREATEST(0, balance_usd - $1) WHERE user_id = $2",
        )
        .bind(amount_usd)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        tx.commit().await.map_err(ApiError::DatabaseError)?;

        let new_balance = self.get_credit_balance(user_id).await?;
        info!(
            "Deducted ${:.4} credits from user {} for {} ({}); new balance ${:.4}",
            amount_usd, user_id, source, ref_id, new_balance
        );
        Ok(new_balance)
    }

    async fn deduct_credits_for_session(
        &self,
        user_id: &str,
        session_id: &str,
        amount_usd: f64,
    ) -> Result<f64, ApiError> {
        self.deduct_credits_for_usage(
            user_id,
            &format!("hosted-session-{session_id}"),
            amount_usd,
            "hosted_runtime_usage",
        )
        .await
    }

    async fn grant_subscription_credits(
        &self,
        user_id: &str,
        invoice_id: &str,
        amount_usd: f64,
        rollover_cap_usd: f64,
    ) -> Result<f64, ApiError> {
        if amount_usd <= 0.0 {
            return self.get_credit_balance(user_id).await;
        }

        let mut tx = self.db.begin().await.map_err(ApiError::DatabaseError)?;

        // Idempotency: the invoice id is the grant key, so a replayed Stripe
        // delivery for an already-granted invoice conflicts and touches nothing.
        let ledger = sqlx::query(
            r#"
            INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
            VALUES ($1, $2, $3, $4, 'stripe_subscription', $5)
            ON CONFLICT (transaction_id) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(amount_usd)
        .bind(format!("stripe-invoice-{invoice_id}"))
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        if ledger.rows_affected() == 0 {
            tx.commit().await.map_err(ApiError::DatabaseError)?;
            return self.get_credit_balance(user_id).await;
        }

        // Rollover: only the capped remainder carries into the new month, then
        // the full monthly grant is added on top. A fresh balance row starts at
        // the grant itself — the cap clips leftovers, never the first grant.
        sqlx::query(
            r#"
            INSERT INTO user_credits (user_id, balance_usd)
            VALUES ($1, $2)
            ON CONFLICT (user_id) DO UPDATE SET
                balance_usd = LEAST(user_credits.balance_usd, $3) + excluded.balance_usd
            "#,
        )
        .bind(user_id)
        .bind(amount_usd)
        .bind(rollover_cap_usd)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        tx.commit().await.map_err(ApiError::DatabaseError)?;

        let new_balance = self.get_credit_balance(user_id).await?;
        info!(
            "Granted ${:.2} subscription credits to user {} for invoice {}; new balance ${:.2}",
            amount_usd, user_id, invoice_id, new_balance
        );
        Ok(new_balance)
    }

    async fn debit_credits_for_transfer(
        &self,
        user_id: &str,
        amount_usd: f64,
        transaction_id: &str,
        source: &str,
    ) -> Result<f64, ApiError> {
        if amount_usd <= 0.0 {
            return Err(ApiError::BadRequest(
                "Credit amount must be positive.".to_string(),
            ));
        }

        let mut tx = self.db.begin().await.map_err(ApiError::DatabaseError)?;

        // Idempotency: a replayed transfer conflicts and touches nothing.
        let ledger = sqlx::query(
            r#"
            INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (transaction_id) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(-amount_usd)
        .bind(transaction_id)
        .bind(source)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;

        if ledger.rows_affected() == 0 {
            tx.commit().await.map_err(ApiError::DatabaseError)?;
            return self.get_credit_balance(user_id).await;
        }

        // Refuse — never clamp — when the balance cannot cover the transfer:
        // the caller credits a remote ledger from this debit, so a clamped
        // partial transfer would mint unbacked money there.
        sqlx::query(
            "INSERT INTO user_credits (user_id, balance_usd) VALUES ($1, 0) ON CONFLICT (user_id) DO NOTHING",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;
        let updated = sqlx::query(
            "UPDATE user_credits SET balance_usd = balance_usd - $1 WHERE user_id = $2 AND balance_usd >= $1",
        )
        .bind(amount_usd)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::DatabaseError)?;
        if updated.rows_affected() == 0 {
            tx.rollback().await.map_err(ApiError::DatabaseError)?;
            let current = self.get_credit_balance(user_id).await?;
            return Err(ApiError::PaymentRequired(format!(
                "Insufficient credits for transfer: ${:.2} available, ${:.2} required.",
                current, amount_usd
            )));
        }

        tx.commit().await.map_err(ApiError::DatabaseError)?;

        let new_balance = self.get_credit_balance(user_id).await?;
        info!(
            "Transferred ${:.2} out of user {} wallet ({} / {}); new balance ${:.2}",
            amount_usd, user_id, source, transaction_id, new_balance
        );
        Ok(new_balance)
    }
}

/// Emit `usage.threshold` into the event backbone for a crossed budget level,
/// at most once per (user, month, level, budget): the source id makes the emit
/// idempotent, so the minute-by-minute check never repeats it. `level` is the
/// alert threshold, or 100 once the budget itself is used up. Failures are
/// logged, never fatal to the alert check.
pub async fn emit_usage_threshold(db: &PgPool, user_id: &str, budget: f64, used: f64, threshold: f64, utilization_percent: f64, now: DateTime<Utc>) -> Option<String> {
    let level = if utilization_percent >= 100.0 { 100.0 } else { threshold };
    let period = now.format("%Y-%m").to_string();
    let source_id = format!("{user_id}:{period}:{}:{}", level.round() as i64, (budget * 100.0).round() as i64);
    let data = serde_json::json!({
        "meter": "cloud_spend",
        "percent": level,
        "used": (used * 100.0).round() / 100.0,
        "limit": budget,
        "period": period,
        "currency": "USD",
    });
    match crate::routes::allternit_events::emit_user_event(db, user_id, "usage.threshold", &data, Some(now), "cloud:costs", Some(&source_id)).await {
        Ok(id) => id,
        Err(e) => {
            warn!("usage.threshold emit failed for user {}: {}", user_id, e);
            None
        }
    }
}

/// Start the background cost tracking task
///
/// This task periodically:
/// 1. Updates costs for running instances
/// 2. Checks budget thresholds and sends alerts
/// 3. Recalculates monthly costs
pub async fn start_cost_tracking_task(db: PgPool) {
    let service = CostServiceImpl::new(db);
    let mut interval = interval(tokio::time::Duration::from_secs(60)); // Run every minute

    info!("Starting background cost tracking task");

    loop {
        interval.tick().await;

        // Update running costs
        if let Err(e) = update_running_costs(&service).await {
            error!("Failed to update running costs: {}", e);
        }

        // Check budget alerts for all users
        if let Err(e) = check_all_budget_alerts(&service).await {
            error!("Failed to check budget alerts: {}", e);
        }
    }
}

/// Update costs for all running instances
async fn update_running_costs(service: &CostServiceImpl) -> Result<(), ApiError> {
    // Get all runs that are currently running and have cost records
    let running_run_ids: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT rc.run_id 
        FROM run_costs rc
        JOIN runs r ON rc.run_id = r.id
        WHERE r.status = 'running' AND rc.ended_at IS NULL
        "#,
    )
    .fetch_all(&service.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    for (run_id,) in running_run_ids {
        if let Err(e) = service.update_running_cost(&run_id).await {
            warn!("Failed to update running cost for run {}: {}", run_id, e);
        }
    }

    Ok(())
}

/// Check budget alerts for all users
async fn check_all_budget_alerts(service: &CostServiceImpl) -> Result<(), ApiError> {
    // Get all users with budgets
    let budgets: Vec<UserCostBudget> =
        sqlx::query_as("SELECT * FROM user_cost_budgets WHERE alert_enabled = true")
            .fetch_all(&service.db)
            .await
            .map_err(ApiError::DatabaseError)?;

    for budget in budgets {
        if let Err(e) = service.check_budget_alerts(&budget.user_id).await {
            warn!(
                "Failed to check budget alerts for user {}: {}",
                budget.user_id, e
            );
        }
    }

    Ok(())
}

/// Initialize cost tracking for a run (helper function for run lifecycle integration)
pub async fn init_run_cost_tracking(
    db: &PgPool,
    run_id: &str,
    provider: &str,
    region: &str,
    instance_type: &str,
) -> Result<(), ApiError> {
    let service = CostServiceImpl::new(db.clone());
    service
        .init_run_cost(run_id, provider, region, instance_type)
        .await?;
    Ok(())
}

/// Finalize cost tracking for a run (helper function for run lifecycle integration)
pub async fn finalize_run_cost_tracking(
    db: &PgPool,
    run_id: &str,
    storage_gb: Option<f64>,
    transfer_gb: Option<f64>,
) -> Result<(), ApiError> {
    let service = CostServiceImpl::new(db.clone());
    service
        .finalize_run_cost(run_id, storage_gb, transfer_gb)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables the budget check touches, with production's column types (002 widened `real` to float8).
    async fn budget_db() -> PgPool {
        let db = crate::routes::test_support::test_pool().await;
        crate::routes::test_support::events_backbone_schema(&db).await;
        sqlx::raw_sql(
            "CREATE TABLE runs (id text PRIMARY KEY, owner_id text, tenant_id text);
             CREATE TABLE run_costs (id text PRIMARY KEY, run_id text, total_cost double precision DEFAULT 0, started_at timestamptz, duration_seconds bigint);
             CREATE TABLE user_cost_budgets (user_id text PRIMARY KEY, monthly_budget double precision DEFAULT 0, current_month_cost double precision DEFAULT 0,
                 alert_threshold double precision DEFAULT 80, last_alert_at timestamptz, alert_enabled boolean DEFAULT true, currency text DEFAULT 'USD',
                 created_at timestamptz DEFAULT now(), updated_at timestamptz DEFAULT now());
             CREATE TABLE cost_alerts (id text NOT NULL, user_id text, alert_type text, threshold_percent double precision, current_cost double precision,
                 budget_amount double precision, message text, sent_at timestamptz DEFAULT now());",
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    async fn spend(db: &PgPool, user: &str, id: &str, usd: f64) {
        sqlx::query("INSERT INTO runs (id, owner_id, tenant_id) VALUES ($1, $2, $2)").bind(id).bind(user).execute(db).await.unwrap();
        sqlx::query("INSERT INTO run_costs (id, run_id, total_cost, started_at, duration_seconds) VALUES ($1, $1, $2, now(), 60)").bind(id).bind(usd).execute(db).await.unwrap();
    }

    async fn threshold_events(db: &PgPool, user: &str) -> Vec<Value> {
        sqlx::query_scalar("SELECT data FROM platform_events WHERE subject = 'user' AND user_id = $1 AND type = 'usage.threshold' ORDER BY created_at").bind(user).fetch_all(db).await.unwrap()
    }

    use serde_json::Value;

    #[tokio::test]
    async fn usage_threshold_is_emitted_once_per_crossing() {
        let db = budget_db().await;
        let service = CostServiceImpl::new(db.clone());
        // Default budget: $100, alert at 80%.
        service.get_or_init_user_budget("u1").await.unwrap();
        spend(&db, "u1", "r1", 50.0).await;
        service.check_budget_alerts("u1").await.unwrap();
        assert!(threshold_events(&db, "u1").await.is_empty(), "under the threshold");

        spend(&db, "u1", "r2", 35.0).await;
        let alerts = service.check_budget_alerts("u1").await.unwrap();
        assert_eq!(alerts.len(), 1, "the cost_alerts row still lands");
        // The minute-by-minute check runs again: still one event.
        service.check_budget_alerts("u1").await.unwrap();
        sqlx::query("UPDATE user_cost_budgets SET last_alert_at = now() - interval '2 days'").execute(&db).await.unwrap();
        service.check_budget_alerts("u1").await.unwrap();
        let events = threshold_events(&db, "u1").await;
        assert_eq!(events.len(), 1, "{events:?}");
        let e = &events[0];
        assert_eq!((e["meter"].as_str(), e["percent"].as_f64(), e["used"].as_f64(), e["limit"].as_f64()), (Some("cloud_spend"), Some(80.0), Some(85.0), Some(100.0)));
        assert_eq!(e["period"].as_str().unwrap(), Utc::now().format("%Y-%m").to_string());

        // Crossing the budget itself is a second crossing, emitted once, even inside the 24 h alert gap.
        spend(&db, "u1", "r3", 20.0).await;
        service.check_budget_alerts("u1").await.unwrap();
        service.check_budget_alerts("u1").await.unwrap();
        let events = threshold_events(&db, "u1").await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["percent"].as_f64(), Some(100.0));

        // Alerts off: nothing.
        service.get_or_init_user_budget("u2").await.unwrap();
        sqlx::query("UPDATE user_cost_budgets SET alert_enabled = false WHERE user_id = 'u2'").execute(&db).await.unwrap();
        spend(&db, "u2", "r4", 500.0).await;
        service.check_budget_alerts("u2").await.unwrap();
        assert!(threshold_events(&db, "u2").await.is_empty());
    }

    #[tokio::test]
    async fn test_calculate_instance_cost() {
        let service = CostServiceImpl::new(PgPool::connect_lazy("postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test").unwrap());

        // Test 1 hour at $0.01/hour
        assert_eq!(service.calculate_instance_cost(3600, 0.01), 0.01);

        // Test 30 minutes at $0.01/hour
        assert_eq!(service.calculate_instance_cost(1800, 0.01), 0.005);

        // Test 2 hours at $0.05/hour
        assert_eq!(service.calculate_instance_cost(7200, 0.05), 0.10);
    }

    #[tokio::test]
    async fn test_calculate_storage_cost() {
        let service = CostServiceImpl::new(PgPool::connect_lazy("postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test").unwrap());

        // Test 100GB for 15 days at $0.10/GB/month
        let cost = service.calculate_storage_cost(100.0, 15 * 24 * 3600, 0.10);
        assert!(cost > 0.0);
        assert!(cost < 10.0); // Should be roughly $0.50
    }

    #[tokio::test]
    async fn test_calculate_transfer_cost() {
        let service = CostServiceImpl::new(PgPool::connect_lazy("postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test").unwrap());

        // Test 10GB at $0.09/GB
        assert!((service.calculate_transfer_cost(10.0, 0.09) - 0.9).abs() < 1e-9);

        // Test 0GB
        assert_eq!(service.calculate_transfer_cost(0.0, 0.09), 0.0);
    }

    /// Minimal credits schema (migrations 024) in a scratch schema, matching
    /// the other PG-backed test modules in this crate.
    async fn credits_test_pool() -> PgPool {
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
        sqlx::query("DROP TABLE IF EXISTS credit_transactions CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE credit_transactions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                amount_usd DOUBLE PRECISION NOT NULL,
                transaction_id TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn session_deduction_is_ledgered_and_idempotent() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 25.0, "seed-1", "stripe")
            .await
            .unwrap();

        let balance = service
            .deduct_credits_for_session("user_1", "sess_1", 7.5)
            .await
            .unwrap();
        assert!((balance - 17.5).abs() < 1e-9);

        let (amount, source, transaction_id): (f64, String, String) = sqlx::query_as(
            "SELECT amount_usd, source, transaction_id FROM credit_transactions WHERE transaction_id = 'hosted-session-sess_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!((amount + 7.5).abs() < 1e-9, "the debit is a negative ledger row");
        assert_eq!(source, "hosted_runtime_usage");
        assert_eq!(transaction_id, "hosted-session-sess_1");

        // Replaying the same session deducts nothing and writes no new row.
        let balance = service
            .deduct_credits_for_session("user_1", "sess_1", 7.5)
            .await
            .unwrap();
        assert!((balance - 17.5).abs() < 1e-9, "same session deducts once");
        let ledger_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM credit_transactions WHERE amount_usd < 0")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(ledger_rows, 1);
    }

    #[tokio::test]
    async fn session_deduction_clamps_at_zero() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 5.0, "seed-1", "stripe")
            .await
            .unwrap();

        let balance = service
            .deduct_credits_for_session("user_1", "sess_1", 12.0)
            .await
            .unwrap();
        assert_eq!(balance, 0.0, "the balance never goes negative");

        let stored: f64 =
            sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = 'user_1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, 0.0);
    }

    #[tokio::test]
    async fn session_deduction_without_credits_row_is_noop_success() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());

        // Plan-cap user: no user_credits row. The deduction must not error,
        // and must not create a balance row.
        let balance = service
            .deduct_credits_for_session("user_plan", "sess_1", 3.0)
            .await
            .unwrap();
        assert_eq!(balance, 0.0);
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_credits WHERE user_id = 'user_plan'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn usage_deduction_is_ledgered_with_ref_id_and_source() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 25.0, "seed-1", "stripe")
            .await
            .unwrap();

        let balance = service
            .deduct_credits_for_usage("user_1", "inference-abc123", 0.30, "inference")
            .await
            .unwrap();
        assert!((balance - 24.70).abs() < 1e-9);

        let (amount, source): (f64, String) = sqlx::query_as(
            "SELECT amount_usd, source FROM credit_transactions WHERE transaction_id = 'inference-abc123'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!((amount + 0.30).abs() < 1e-9);
        assert_eq!(source, "inference");

        // Replay with the same ref id is a no-op.
        let balance = service
            .deduct_credits_for_usage("user_1", "inference-abc123", 0.30, "inference")
            .await
            .unwrap();
        assert!((balance - 24.70).abs() < 1e-9, "same ref id deducts once");
        let debits: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM credit_transactions WHERE amount_usd < 0")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(debits, 1);
    }

    #[tokio::test]
    async fn usage_deduction_clamps_and_no_row_noops() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 0.10, "seed-1", "stripe")
            .await
            .unwrap();

        let balance = service
            .deduct_credits_for_usage("user_1", "inference-big", 5.0, "inference")
            .await
            .unwrap();
        assert_eq!(balance, 0.0, "the balance clamps at zero");

        // No credits row (plan-cap user): no-op success, no balance row created.
        let balance = service
            .deduct_credits_for_usage("user_plan", "inference-plan", 1.0, "inference")
            .await
            .unwrap();
        assert_eq!(balance, 0.0);
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_credits WHERE user_id = 'user_plan'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn session_deduction_delegates_to_usage_deduction() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 10.0, "seed-1", "stripe")
            .await
            .unwrap();

        service
            .deduct_credits_for_session("user_1", "sess_9", 2.0)
            .await
            .unwrap();
        let (transaction_id, source): (String, String) = sqlx::query_as(
            "SELECT transaction_id, source FROM credit_transactions WHERE amount_usd < 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(transaction_id, "hosted-session-sess_9", "session ref id shape preserved");
        assert_eq!(source, "hosted_runtime_usage", "session source preserved");
    }

    #[tokio::test]
    async fn transfer_debit_reduces_balance_and_ledgers() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 25.0, "seed-1", "stripe")
            .await
            .unwrap();

        let balance = service
            .debit_credits_for_transfer("user_1", 10.0, "fabric-tx-1", "fabric_transfer")
            .await
            .unwrap();
        assert!((balance - 15.0).abs() < 1e-9);

        let (amount, source): (f64, String) = sqlx::query_as(
            "SELECT amount_usd, source FROM credit_transactions WHERE transaction_id = 'fabric-tx-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!((amount + 10.0).abs() < 1e-9, "the transfer is a negative ledger row");
        assert_eq!(source, "fabric_transfer");
    }

    #[tokio::test]
    async fn transfer_debit_replay_is_noop() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 25.0, "seed-1", "stripe")
            .await
            .unwrap();

        service
            .debit_credits_for_transfer("user_1", 10.0, "fabric-tx-1", "fabric_transfer")
            .await
            .unwrap();
        let balance = service
            .debit_credits_for_transfer("user_1", 10.0, "fabric-tx-1", "fabric_transfer")
            .await
            .unwrap();
        assert!((balance - 15.0).abs() < 1e-9, "a replayed transfer debits once");
        let rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM credit_transactions WHERE transaction_id = 'fabric-tx-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn transfer_debit_refuses_insufficient_without_partial_clamp() {
        let pool = credits_test_pool().await;
        let service = CostServiceImpl::new(pool.clone());
        service
            .add_credits("user_1", 5.0, "seed-1", "stripe")
            .await
            .unwrap();

        let err = service
            .debit_credits_for_transfer("user_1", 12.0, "fabric-tx-big", "fabric_transfer")
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::PaymentRequired(_)),
            "insufficient transfer must be refused, not clamped: {err}"
        );

        let stored: f64 =
            sqlx::query_scalar("SELECT balance_usd FROM user_credits WHERE user_id = 'user_1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, 5.0, "a refused transfer leaves the balance untouched");
        let rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM credit_transactions WHERE transaction_id = 'fabric-tx-big'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rows, 0, "a refused transfer writes no ledger row");
    }
}
