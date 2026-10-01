//! Hosted runtime container lifecycle and metering.
//!
//! A usage session is opened whenever a hosted container starts and closed when
//! it stops. The background reconciler also enforces inactivity and monthly
//! runtime-hour limits, so a browser disconnect or API restart cannot leave a
//! paid container running forever.

use crate::{
    error::ApiError,
    services::{
        contabo_runtime_service::hosted_disk_quota_gb, cost_service::CostService,
        ContaboContainerState, ContaboRuntimeService,
    },
    ApiState,
};
use chrono::{DateTime, Datelike, Timelike, Utc};
use sqlx::{FromRow, PgPool};
use std::{sync::Arc, time::Duration};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct HostedUsageSummary {
    pub total_seconds: i64,
    pub estimated_cost_usd: f64,
}

#[derive(Debug, FromRow)]
struct LifecycleRow {
    id: String,
    user_id: String,
    provider: Option<String>,
    status: String,
    memory_mb: i64,
    idle_timeout_minutes: i64,
    last_activity_at: Option<DateTime<Utc>>,
}

fn month_start() -> DateTime<Utc> {
    let now = Utc::now();
    now.with_day(1)
        .unwrap_or(now)
        .with_hour(0)
        .unwrap_or(now)
        .with_minute(0)
        .unwrap_or(now)
        .with_second(0)
        .unwrap_or(now)
        .with_nanosecond(0)
        .unwrap_or(now)
}

/// Return this month's closed plus currently-open hosted runtime usage.
pub async fn hosted_usage_summary(
    db: &PgPool,
    user_id: &str,
) -> Result<HostedUsageSummary, ApiError> {
    let row: (i64, f64) = sqlx::query_as(
        r#"
        SELECT
            COALESCE(SUM(
                CASE WHEN ended_at IS NULL
                    THEN GREATEST(0, EXTRACT(EPOCH FROM NOW())::BIGINT - EXTRACT(EPOCH FROM started_at)::BIGINT)
                    ELSE COALESCE(duration_seconds, 0)
                END
            ), 0)::BIGINT,
            COALESCE(SUM(
                CASE WHEN ended_at IS NULL
                    THEN (GREATEST(0, EXTRACT(EPOCH FROM NOW())::BIGINT - EXTRACT(EPOCH FROM started_at)::BIGINT) / 3600.0) * cost_per_hour
                    ELSE COALESCE(estimated_cost_usd, 0)
                END
            ), 0)
        FROM hosted_runtime_usage_sessions
        WHERE user_id = $1 AND started_at >= $2
        "#,
    )
    .bind(user_id)
    .bind(month_start())
    .fetch_one(db)
    .await?;

    Ok(HostedUsageSummary {
        total_seconds: row.0,
        estimated_cost_usd: row.1,
    })
}

/// Cost accrued so far by the user's currently-open usage sessions. Closed
/// sessions are excluded because their cost was already deducted from the
/// credit balance when they closed — counting them again would double-count.
pub async fn open_session_accrued_cost(db: &PgPool, user_id: &str) -> Result<f64, ApiError> {
    let accrued: f64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(
            (GREATEST(0, EXTRACT(EPOCH FROM NOW())::BIGINT - EXTRACT(EPOCH FROM started_at)::BIGINT) / 3600.0) * cost_per_hour
        ), 0)
        FROM hosted_runtime_usage_sessions
        WHERE user_id = $1 AND ended_at IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(accrued)
}

/// Open one billable interval if this instance does not already have one.
///
/// Never meter $0.00 silently: instances whose cost_rate_* columns are NULL
/// (pre-027 rows, or any future insert path that forgets them) fall back to
/// the default 1GB retail rate. If even the fallback is missing (rate table
/// empty = deployment misconfiguration) the session still opens at $0 with a
/// loud warn — blocking the start would break wake-on-demand, which is worse
/// than one unmetered interval.
pub async fn record_runtime_started(db: &PgPool, instance_id: &str) -> Result<(), ApiError> {
    let row: Option<(String, f64)> = sqlx::query_as(
        r#"
        SELECT h.user_id, COALESCE(
            r.cost_per_hour,
            (SELECT cost_per_hour FROM cost_rates
             WHERE provider = 'contabo' AND region = 'hosted' AND instance_type = 'hosted-1024mb'),
            0
        )
        FROM hosted_runtime_instances h
        LEFT JOIN cost_rates r
          ON r.provider = h.cost_rate_provider
         AND r.region = h.cost_rate_region
         AND r.instance_type = h.cost_rate_instance_type
        WHERE h.id = $1
        "#,
    )
    .bind(instance_id)
    .fetch_optional(db)
    .await?;

    let Some((user_id, cost_per_hour)) = row else {
        return Err(ApiError::NotFound("Hosted runtime not found".to_string()));
    };
    if cost_per_hour == 0.0 {
        warn!(
            instance_id,
            "hosted runtime metering at $0.00/hr: no cost_rates row matches and the default \
             contabo/hosted/hosted-1024mb retail rate is missing (migrations 027) — \
             this interval is unmetered until the rate table is fixed"
        );
    }

    sqlx::query(
        r#"
        INSERT INTO hosted_runtime_usage_sessions (
            id, hosted_instance_id, user_id, started_at, cost_per_hour
        ) VALUES ($1, $2, $3, CURRENT_TIMESTAMP, $4) ON CONFLICT DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(instance_id)
    .bind(user_id)
    .bind(cost_per_hour)
    .execute(db)
    .await?;

    Ok(())
}

/// Close the currently-open interval and freeze its duration and cost.
///
/// Only the call that actually closes an open session bills it: the close
/// returns the finalized row, and the deduction itself is ledgered and
/// idempotent per session id, so repeated stops for the same instance can
/// never double-charge.
pub async fn record_runtime_stopped(
    db: &PgPool,
    instance_id: &str,
    reason: &str,
) -> Result<(), ApiError> {
    let closed: Option<(String, String, f64)> = sqlx::query_as(
        r#"
        UPDATE hosted_runtime_usage_sessions
        SET ended_at = CURRENT_TIMESTAMP,
            duration_seconds = GREATEST(
                0,
                EXTRACT(EPOCH FROM NOW())::BIGINT - EXTRACT(EPOCH FROM started_at)::BIGINT
            ),
            estimated_cost_usd = (
                GREATEST(0, EXTRACT(EPOCH FROM NOW())::BIGINT - EXTRACT(EPOCH FROM started_at)::BIGINT) / 3600.0
            ) * cost_per_hour,
            stop_reason = $1
        WHERE hosted_instance_id = $2 AND ended_at IS NULL
        RETURNING id, user_id, estimated_cost_usd
        "#,
    )
    .bind(reason)
    .bind(instance_id)
    .fetch_optional(db)
    .await?;

    let Some((session_id, user_id, cost)) = closed else {
        return Ok(());
    };

    // Deduct the just-finalized cost from the user's prepaid credit balance.
    if cost > 0.0 {
        let cost_service = crate::services::CostServiceImpl::new(db.clone());
        if let Err(e) = cost_service
            .deduct_credits_for_session(&user_id, &session_id, cost)
            .await
        {
            error!(
                "REVENUE-CRITICAL: failed to deduct ${:.4} credits from user {} for instance {} session {}: {}",
                cost, user_id, instance_id, session_id, e
            );
        }
    }

    Ok(())
}

/// Mark user-driven relay traffic as activity. Runtime heartbeats deliberately
/// do not call this; an otherwise idle daemon must still auto-stop.
pub async fn touch_runtime_activity(db: &PgPool, runtime_id: &str) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE hosted_runtime_instances
        SET last_activity_at = CURRENT_TIMESTAMP
        WHERE runtime_device_id = $1 AND status IN ('starting', 'running')
        "#,
    )
    .bind(runtime_id)
    .execute(db)
    .await?;
    // Provisioned (free) computers keep their idle clock on their own row.
    crate::services::touch_provisioned_activity(db, runtime_id).await
}

pub async fn touch_instance_activity(db: &PgPool, instance_id: &str) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE hosted_runtime_instances
        SET last_activity_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND status IN ('starting', 'running')
        "#,
    )
    .bind(instance_id)
    .execute(db)
    .await?;
    Ok(())
}

/// What a relay connection attempt should do about a hosted runtime's container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedWakeDecision {
    /// Container is already starting or running; nothing to start.
    AlreadyActive,
    /// Container is stopped or stopping and must be started.
    StartRequired,
    /// Instance cannot be woken (creating, error, destroying, or a legacy
    /// instance whose workload is no longer managed).
    NotWakeable,
}

/// Map a hosted instance status to a wake decision. Only Contabo instances can
/// be started (their container name is derived from the instance id), so any
/// other provider forces `NotWakeable`.
pub fn hosted_wake_decision(status: &str, is_contabo: bool) -> HostedWakeDecision {
    match status {
        "starting" | "running" => HostedWakeDecision::AlreadyActive,
        "stopped" | "stopping" if is_contabo => HostedWakeDecision::StartRequired,
        _ => HostedWakeDecision::NotWakeable,
    }
}

/// The hosted instance behind a runtime device, with its wake decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedWakeTarget {
    pub instance_id: String,
    pub user_id: String,
    pub decision: HostedWakeDecision,
}

/// Resolve the hosted instance a runtime device belongs to. Returns `None`
/// for devices that are not hosted runtimes (plain paired desktops/VPS).
pub async fn hosted_wake_target(
    db: &PgPool,
    runtime_id: &str,
) -> Result<Option<HostedWakeTarget>, ApiError> {
    let row: Option<(String, String, String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, user_id, status, provider
        FROM hosted_runtime_instances
        WHERE runtime_device_id = $1 AND status != 'destroyed'
        "#,
    )
    .bind(runtime_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(instance_id, user_id, status, provider)| {
        let decision = hosted_wake_decision(&status, provider.as_deref() == Some("contabo"));
        HostedWakeTarget {
            instance_id,
            user_id,
            decision,
        }
    }))
}

/// Mark a hosted instance as starting and open its billing session. Shared by
/// the user-driven start route and relay wake-on-demand; call it after the
/// provider has accepted the start.
pub async fn mark_hosted_instance_starting(db: &PgPool, instance_id: &str) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE hosted_runtime_instances SET status = 'starting', started_at = CURRENT_TIMESTAMP, stopped_at = NULL, active_since = CURRENT_TIMESTAMP, last_activity_at = CURRENT_TIMESTAMP, stop_reason = NULL WHERE id = $1",
    )
    .bind(instance_id)
    .execute(db)
    .await?;
    record_runtime_started(db, instance_id).await
}

/// Outcome of a wake-on-demand attempt for a runtime device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostedWakeOutcome {
    /// Device is not a hosted runtime.
    NotHosted,
    /// Instance was already starting or running.
    AlreadyActive,
    /// A start was issued; the daemon still has to boot and reconnect.
    Waking,
    /// Hosted instance exists but cannot be started right now.
    NotWakeable,
}

/// Start the container behind a hosted runtime device when it is stopped, so a
/// connecting client does not fail against an idle-stopped runtime.
///
/// Wake-on-demand runs the same spend-cap and monthly-hours checks as the
/// user-driven start route before starting anything. A `Forbidden` propagates
/// so the relay surfaces the reason to the connecting client instead of
/// booting a machine the user can no longer pay for.
pub async fn wake_hosted_runtime_for_device(
    db: &PgPool,
    contabo: &ContaboRuntimeService,
    quota_service: &crate::services::QuotaService,
    runtime_id: &str,
) -> Result<HostedWakeOutcome, ApiError> {
    let Some(target) = hosted_wake_target(db, runtime_id).await? else {
        return Ok(HostedWakeOutcome::NotHosted);
    };
    match target.decision {
        HostedWakeDecision::AlreadyActive => Ok(HostedWakeOutcome::AlreadyActive),
        HostedWakeDecision::NotWakeable => Ok(HostedWakeOutcome::NotWakeable),
        HostedWakeDecision::StartRequired => {
            let quota = quota_service.ensure_quota(&target.user_id).await?;
            quota_service
                .check_hosted_runtime_hours(&target.user_id, &quota)
                .await?;
            quota_service
                .check_spend_cap(&target.user_id, &quota)
                .await?;
            contabo.start(&target.instance_id).await?;
            mark_hosted_instance_starting(db, &target.instance_id).await?;
            info!(instance_id = %target.instance_id, %runtime_id, "Hosted runtime wake-on-demand start issued");
            Ok(HostedWakeOutcome::Waking)
        }
    }
}

pub fn start_hosted_runtime_lifecycle_task(state: Arc<ApiState>) {
    let interval_seconds = std::env::var("HOSTED_RUNTIME_RECONCILE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value >= 15)
        .unwrap_or(60);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        info!(interval_seconds, "Hosted runtime lifecycle task started");
        loop {
            interval.tick().await;
            if let Err(error) = reconcile_hosted_runtimes(&state).await {
                error!("Hosted runtime lifecycle reconciliation failed: {}", error);
            }
        }
    });
}

async fn reconcile_hosted_runtimes(state: &ApiState) -> Result<(), ApiError> {
    let contabo = &state.contabo_runtime_service;
    let rows = sqlx::query_as::<_, LifecycleRow>(
        r#"
        SELECT id, user_id, provider, status, memory_mb,
               idle_timeout_minutes, last_activity_at
        FROM hosted_runtime_instances
        WHERE status IN ('starting', 'running', 'stopping')
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    for row in rows {
        // Legacy instances from before the Contabo migration have no managed
        // container; there is nothing to reconcile for them.
        if row.provider.as_deref() != Some("contabo") {
            debug!(instance_id = %row.id, provider = ?row.provider, "Skipping lifecycle reconciliation for legacy hosted runtime");
            continue;
        }

        if row.status == "running" {
            let quota = state.quota_service.ensure_quota(&row.user_id).await?;
            let usage = hosted_usage_summary(&state.db, &row.user_id).await?;
            let instance_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM hosted_runtime_instances WHERE user_id = $1 AND status NOT IN ('destroying', 'destroyed')",
            )
            .bind(&row.user_id)
            .fetch_one(&state.db)
            .await?;
            let entitlement_revoked = !quota.can_create_hosted_runtime
                || quota.max_hosted_runtimes <= 0
                || quota.max_hosted_runtime_hours_monthly <= 0;
            let plan_limit_changed = row.memory_mb > quota.max_hosted_runtime_memory_mb
                || instance_count > quota.max_hosted_runtimes;
            let hours_exhausted = quota.max_hosted_runtime_hours_monthly > 0
                && usage.total_seconds >= quota.max_hosted_runtime_hours_monthly * 3600;
            let spend_cap_exhausted = match state
                .quota_service
                .check_spend_cap(&row.user_id, &quota)
                .await
            {
                Ok(()) => false,
                Err(ApiError::Forbidden(_)) => true,
                Err(error) => {
                    warn!(instance_id = %row.id, "Unable to evaluate hosted spend cap: {}", error);
                    false
                }
            };
            let idle = row
                .last_activity_at
                .map(|last| {
                    row.idle_timeout_minutes > 0
                        && Utc::now().signed_duration_since(last).num_minutes()
                            >= row.idle_timeout_minutes
                })
                .unwrap_or(false);

            if entitlement_revoked
                || plan_limit_changed
                || hours_exhausted
                || spend_cap_exhausted
                || idle
            {
                let reason = if entitlement_revoked {
                    "plan_entitlement_removed"
                } else if plan_limit_changed {
                    "plan_limit_changed"
                } else if hours_exhausted {
                    "monthly_hours_exhausted"
                } else if spend_cap_exhausted {
                    "monthly_spend_cap"
                } else {
                    "idle_timeout"
                };
                match contabo.stop(&row.id).await {
                    Ok(()) => {
                        sqlx::query(
                            r#"
                            UPDATE hosted_runtime_instances
                            SET status = 'stopped', stopped_at = CURRENT_TIMESTAMP,
                                active_since = NULL, stop_reason = $1
                            WHERE id = $2
                            "#,
                        )
                        .bind(reason)
                        .bind(&row.id)
                        .execute(&state.db)
                        .await?;
                        record_runtime_stopped(&state.db, &row.id, reason).await?;
                        info!(instance_id = %row.id, %reason, "Hosted runtime auto-stopped");
                    }
                    Err(error) => {
                        warn!(instance_id = %row.id, "Hosted runtime auto-stop failed: {}", error)
                    }
                }
                continue;
            }
        }

        match contabo.status(&row.id).await {
            Ok(ContaboContainerState::Running) => {
                debug!(instance_id = %row.id, current_status = %row.status, "reconciler: container running, marking instance running");
                sqlx::query(
                    r#"
                    UPDATE hosted_runtime_instances
                    SET status = 'running', active_since = COALESCE(active_since, CURRENT_TIMESTAMP),
                        started_at = COALESCE(started_at, CURRENT_TIMESTAMP),
                        last_activity_at = COALESCE(last_activity_at, CURRENT_TIMESTAMP),
                        last_synced_at = CURRENT_TIMESTAMP, error_message = NULL
                    WHERE id = $1
                    "#,
                )
                .bind(&row.id)
                .execute(&state.db)
                .await?;
                record_runtime_started(&state.db, &row.id).await?;
            }
            Ok(ContaboContainerState::Stopped) => {
                sqlx::query(
                    r#"
                    UPDATE hosted_runtime_instances
                    SET status = 'stopped', stopped_at = COALESCE(stopped_at, CURRENT_TIMESTAMP),
                        active_since = NULL, last_synced_at = CURRENT_TIMESTAMP,
                        stop_reason = COALESCE(stop_reason, 'provider_stopped')
                    WHERE id = $1
                    "#,
                )
                .bind(&row.id)
                .execute(&state.db)
                .await?;
                record_runtime_stopped(&state.db, &row.id, "provider_stopped").await?;
            }
            Ok(ContaboContainerState::Starting) => {
                debug!(instance_id = %row.id, current_status = %row.status, "reconciler: container still starting");
                sqlx::query(
                    "UPDATE hosted_runtime_instances SET status = 'starting', last_synced_at = CURRENT_TIMESTAMP WHERE id = $1",
                )
                .bind(&row.id)
                .execute(&state.db)
                .await?;
            }
            Ok(ContaboContainerState::Removed) => {
                // The box is gone but the instance is still owned. Park it
                // stopped so start() can recreate; do not destroy the row.
                sqlx::query(
                    r#"
                    UPDATE hosted_runtime_instances
                    SET status = 'stopped',
                        stopped_at = COALESCE(stopped_at, CURRENT_TIMESTAMP),
                        active_since = NULL,
                        last_synced_at = CURRENT_TIMESTAMP,
                        stop_reason = 'container_missing'
                    WHERE id = $1 AND status != 'destroyed'
                    "#,
                )
                .bind(&row.id)
                .execute(&state.db)
                .await?;
                record_runtime_stopped(&state.db, &row.id, "container_missing").await?;
            }
            Ok(ContaboContainerState::Other(container_state)) => {
                debug!(instance_id = %row.id, %container_state, "Unmapped container state");
            }
            Err(error) => {
                warn!(instance_id = %row.id, "Container status reconciliation failed: {}", error)
            }
        }
    }

    // Belt-and-suspenders disk quota: docker's --storage-opt is unavailable on
    // the nodes (overlay2 on ext4), so the sweep stops runaway writable layers.
    // Sweep failure must not fail the rest of reconciliation.
    if let Err(error) = sweep_disk_quotas(state).await {
        warn!("Hosted runtime disk quota sweep failed: {}", error);
    }

    Ok(())
}

/// Stop running hosted containers whose writable layer exceeds their disk
/// quota (5g/10g/20g by container size, ×1.2 grace). One `docker ps -s` per
/// docker host that actually runs hosted containers; hosts that fail are
/// skipped with a warning so one unreachable node never shields the others.
async fn sweep_disk_quotas(state: &ApiState) -> Result<(), ApiError> {
    let running: Vec<(String, String, i64, Option<String>)> = sqlx::query_as(
        r#"
        SELECT i.id, i.user_id, i.memory_mb, n.docker_host
        FROM hosted_runtime_instances i
        LEFT JOIN hosted_runtime_nodes n ON n.id = i.node_id
        WHERE i.status = 'running' AND i.provider = 'contabo'
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    if running.is_empty() {
        return Ok(());
    }

    // Group by docker host; legacy rows (node_id NULL) live on the local daemon.
    let mut by_host: std::collections::HashMap<String, Vec<(String, String, i64)>> =
        std::collections::HashMap::new();
    for (instance_id, user_id, memory_mb, docker_host) in running {
        by_host
            .entry(docker_host.unwrap_or_else(|| "local".to_string()))
            .or_default()
            .push((instance_id, user_id, memory_mb));
    }

    let contabo = &state.contabo_runtime_service;
    for (docker_host, instances) in by_host {
        let sizes = match contabo.container_writable_bytes(&docker_host).await {
            Ok(sizes) => sizes,
            Err(error) => {
                warn!(%docker_host, "disk sweep: docker ps failed, skipping host: {}", error);
                continue;
            }
        };
        for (instance_id, user_id, memory_mb) in instances {
            let container_name = ContaboRuntimeService::container_name(&instance_id);
            let Some(writable_bytes) = sizes
                .iter()
                .find(|(name, _)| *name == container_name)
                .map(|(_, bytes)| *bytes)
            else {
                continue;
            };
            let limit_bytes = hosted_disk_quota_gb(memory_mb) * 1_000_000_000 * 6 / 5;
            if writable_bytes <= limit_bytes {
                continue;
            }
            warn!(
                instance_id = %instance_id,
                user_id = %user_id,
                writable_bytes,
                limit_bytes,
                "Hosted container exceeded its disk quota; stopping"
            );
            match contabo.stop(&instance_id).await {
                Ok(()) => {
                    sqlx::query(
                        r#"
                        UPDATE hosted_runtime_instances
                        SET status = 'stopped', stopped_at = CURRENT_TIMESTAMP,
                            active_since = NULL, stop_reason = 'disk_quota_exceeded'
                        WHERE id = $1
                        "#,
                    )
                    .bind(&instance_id)
                    .execute(&state.db)
                    .await?;
                    record_runtime_stopped(&state.db, &instance_id, "disk_quota_exceeded").await?;
                    info!(instance_id = %instance_id, "Hosted runtime stopped for disk_quota_exceeded");
                }
                Err(error) => {
                    warn!(instance_id = %instance_id, "disk-quota stop failed: {}", error)
                }
            }
        }
    }

    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal hosted-instance/usage/cost-rate shape for the wake path. The
    /// partial unique index matters: `record_runtime_started` relies on
    /// INSERT OR IGNORE to keep one open session per instance.
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
        sqlx::query("DROP TABLE IF EXISTS hosted_runtime_instances CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE hosted_runtime_instances (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                runtime_device_id TEXT,
                status TEXT NOT NULL,
                provider TEXT,
                cost_rate_provider TEXT,
                cost_rate_region TEXT,
                cost_rate_instance_type TEXT,
                started_at TIMESTAMPTZ,
                stopped_at TIMESTAMPTZ,
                active_since TIMESTAMPTZ,
                last_activity_at TIMESTAMPTZ,
                stop_reason TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS cost_rates CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE cost_rates (
                provider TEXT NOT NULL,
                region TEXT NOT NULL,
                instance_type TEXT NOT NULL,
                cost_per_hour DOUBLE PRECISION NOT NULL DEFAULT 0,
                PRIMARY KEY (provider, region, instance_type)
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
        sqlx::query(
            r#"
            CREATE UNIQUE INDEX idx_hosted_usage_one_open_session
                ON hosted_runtime_usage_sessions(hosted_instance_id)
                WHERE ended_at IS NULL
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Credits schema (migration 024) for the stop-time deduction path.
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
        // record_runtime_started reads the rate as DOUBLE PRECISION; a matching cost_rates
        // row keeps COALESCE(cost_per_hour, 0) from degrading to BIGINT.
        sqlx::query(
            "INSERT INTO cost_rates (provider, region, instance_type, cost_per_hour) VALUES ('contabo', 'local', 'shared-cpu-1x-1024mb', 0.0079)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_instance(
        pool: &PgPool,
        id: &str,
        runtime_device_id: Option<&str>,
        status: &str,
        provider: Option<&str>,
    ) {
        sqlx::query(
            r#"
            INSERT INTO hosted_runtime_instances (
                id, user_id, runtime_device_id, status, provider,
                cost_rate_provider, cost_rate_region, cost_rate_instance_type
            ) VALUES ($1, 'user_1', $2, $3, $4, 'contabo', 'local', 'shared-cpu-1x-1024mb')
            "#,
        )
        .bind(id)
        .bind(runtime_device_id)
        .bind(status)
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
    }

    #[test]
    fn wake_decision_maps_statuses() {
        assert_eq!(
            hosted_wake_decision("stopped", true),
            HostedWakeDecision::StartRequired
        );
        assert_eq!(
            hosted_wake_decision("stopping", true),
            HostedWakeDecision::StartRequired
        );
        assert_eq!(
            hosted_wake_decision("running", true),
            HostedWakeDecision::AlreadyActive
        );
        assert_eq!(
            hosted_wake_decision("starting", true),
            HostedWakeDecision::AlreadyActive
        );
        for status in ["creating", "error", "destroying", "destroyed"] {
            assert_eq!(
                hosted_wake_decision(status, true),
                HostedWakeDecision::NotWakeable,
                "{status} must not be woken"
            );
        }
        assert_eq!(
            hosted_wake_decision("stopped", false),
            HostedWakeDecision::NotWakeable,
            "non-Contabo instances have no managed container to start"
        );
    }

    #[tokio::test]
    async fn wake_target_resolves_hosted_device_and_decision() {
        let pool = test_pool().await;
        insert_instance(&pool, "hr_1", Some("rd_1"), "stopped", Some("contabo")).await;
        insert_instance(&pool, "hr_2", Some("rd_2"), "running", Some("contabo")).await;
        insert_instance(&pool, "hr_3", Some("rd_3"), "stopped", Some("legacy")).await;

        let target = hosted_wake_target(&pool, "rd_1").await.unwrap().unwrap();
        assert_eq!(target.instance_id, "hr_1");
        assert_eq!(target.decision, HostedWakeDecision::StartRequired);

        let target = hosted_wake_target(&pool, "rd_2").await.unwrap().unwrap();
        assert_eq!(target.decision, HostedWakeDecision::AlreadyActive);

        let target = hosted_wake_target(&pool, "rd_3").await.unwrap().unwrap();
        assert_eq!(
            target.decision,
            HostedWakeDecision::NotWakeable,
            "legacy instances cannot be woken"
        );

        assert!(
            hosted_wake_target(&pool, "rd_desktop").await.unwrap().is_none(),
            "non-hosted devices are not wake targets"
        );
    }

    #[tokio::test]
    async fn mark_starting_transitions_status_and_opens_one_billing_session() {
        let pool = test_pool().await;
        insert_instance(&pool, "hr_1", Some("rd_1"), "stopped", Some("contabo")).await;

        mark_hosted_instance_starting(&pool, "hr_1").await.unwrap();

        let (status, stop_reason, started_at): (String, Option<String>, Option<DateTime<Utc>>) =
            sqlx::query_as(
                "SELECT status, stop_reason, started_at FROM hosted_runtime_instances WHERE id = 'hr_1'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "starting");
        assert!(stop_reason.is_none());
        assert!(started_at.is_some());

        // The wake helper is safe to repeat (e.g. two clients racing a wake):
        // the status transition is idempotent and the open-session unique
        // index keeps a single billable interval.
        mark_hosted_instance_starting(&pool, "hr_1").await.unwrap();
        let open_sessions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hosted_runtime_usage_sessions WHERE hosted_instance_id = 'hr_1' AND ended_at IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_sessions, 1, "wake opens exactly one billing session");
    }

    #[tokio::test]
    async fn stopping_twice_deducts_the_session_cost_once() {
        let pool = test_pool().await;
        insert_instance(&pool, "hr_1", Some("rd_1"), "running", Some("contabo")).await;
        let cost_service = crate::services::CostServiceImpl::new(pool.clone());
        cost_service
            .add_credits("user_1", 25.0, "seed-1", "stripe")
            .await
            .unwrap();

        mark_hosted_instance_starting(&pool, "hr_1").await.unwrap();
        // Backdate the open session so the finalized cost is deterministic:
        // exactly one hour at the seeded 0.0079/hr rate.
        sqlx::query(
            "UPDATE hosted_runtime_usage_sessions SET started_at = NOW() - INTERVAL '1 hour' WHERE hosted_instance_id = 'hr_1' AND ended_at IS NULL",
        )
        .execute(&pool)
        .await
        .unwrap();

        record_runtime_stopped(&pool, "hr_1", "user_stopped")
            .await
            .unwrap();
        let balance = cost_service.get_credit_balance("user_1").await.unwrap();
        let expected = 25.0 - 0.0079;
        assert!(
            (balance - expected).abs() < 1e-6,
            "first stop deducts the session cost: balance {balance}"
        );
        let debits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM credit_transactions WHERE user_id = 'user_1' AND amount_usd < 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(debits, 1, "the deduction is ledgered");

        // A second stop for the same instance (destroy path after user stop,
        // reconciler provider_stopped, etc.) must not deduct again.
        record_runtime_stopped(&pool, "hr_1", "destroyed").await.unwrap();
        let balance = cost_service.get_credit_balance("user_1").await.unwrap();
        assert!(
            (balance - expected).abs() < 1e-6,
            "second stop deducts nothing: balance {balance}"
        );
        let debits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM credit_transactions WHERE user_id = 'user_1' AND amount_usd < 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(debits, 1, "still exactly one debit row");
    }

    /// The three retail rates from migrations 027, inserted into the scratch
    /// schema (which otherwise only carries the legacy Fly default row).
    async fn insert_retail_rates(pool: &PgPool) {
        sqlx::query(
            r#"
            INSERT INTO cost_rates (provider, region, instance_type, cost_per_hour) VALUES
                ('contabo', 'hosted', 'hosted-512mb',  0.0075),
                ('contabo', 'hosted', 'hosted-1024mb', 0.0150),
                ('contabo', 'hosted', 'hosted-2048mb', 0.0290)
            ON CONFLICT (provider, region, instance_type) DO NOTHING
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn legacy_instance_without_rate_columns_snapshots_the_default_retail_rate() {
        let pool = test_pool().await;
        insert_retail_rates(&pool).await;
        // Pre-027 rows have NULL cost_rate_* — they must meter at the default
        // 1GB retail rate, never $0.00/hr.
        sqlx::query(
            "INSERT INTO hosted_runtime_instances (id, user_id, status) VALUES ('hr_legacy', 'user_1', 'stopped')",
        )
        .execute(&pool)
        .await
        .unwrap();

        record_runtime_started(&pool, "hr_legacy").await.unwrap();

        let rate: f64 = sqlx::query_scalar(
            "SELECT cost_per_hour FROM hosted_runtime_usage_sessions WHERE hosted_instance_id = 'hr_legacy'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            (rate - 0.015).abs() < 1e-9,
            "legacy instance must snapshot the default 1GB retail rate, got {rate}"
        );
    }

    #[tokio::test]
    async fn sized_instance_snapshots_its_own_retail_rate() {
        let pool = test_pool().await;
        insert_retail_rates(&pool).await;
        sqlx::query(
            r#"
            INSERT INTO hosted_runtime_instances (
                id, user_id, status, cost_rate_provider, cost_rate_region, cost_rate_instance_type
            ) VALUES ('hr_2gb', 'user_1', 'stopped', 'contabo', 'hosted', 'hosted-2048mb')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        record_runtime_started(&pool, "hr_2gb").await.unwrap();

        let rate: f64 = sqlx::query_scalar(
            "SELECT cost_per_hour FROM hosted_runtime_usage_sessions WHERE hosted_instance_id = 'hr_2gb'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            (rate - 0.029).abs() < 1e-9,
            "2048mb instance must snapshot the 2048mb retail rate, got {rate}"
        );
    }
}
