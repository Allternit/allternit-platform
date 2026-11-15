//! Health check routes
//!
//! Provides endpoints for:
//! - GET /api/v1/health - Basic health check with DB connectivity
//! - GET /api/v1/health/ready - Readiness probe for Kubernetes
//! - GET /api/v1/health/live - Liveness probe
//! - GET /api/v1/health/computers - Cloud computer provisioning health
//! - GET /api/v1/metrics - Prometheus metrics endpoint

use axum::http::header;
use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;
use std::sync::Arc;

use crate::ApiState;

/// Basic health check response
#[derive(Debug, serde::Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
}

/// Readiness probe response
#[derive(Debug, serde::Serialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub checks: ReadinessChecks,
}

/// Readiness checks details
#[derive(Debug, serde::Serialize)]
pub struct ReadinessChecks {
    pub database: bool,
}

/// Liveness probe response
#[derive(Debug, serde::Serialize)]
pub struct LivenessResponse {
    pub alive: bool,
}

/// Basic health check
///
/// GET /api/v1/health
///
/// Returns basic health status and checks database connectivity.
/// This endpoint is publicly accessible (no auth required).
pub async fn health_check(
    State(state): State<Arc<ApiState>>,
) -> impl IntoResponse {
    let timestamp = Utc::now().to_rfc3339();

    // Check database connectivity
    let db_healthy = sqlx::query("SELECT 1").fetch_one(&state.db).await.is_ok();

    let response = HealthResponse {
        status: if db_healthy {
            "healthy".to_string()
        } else {
            "unhealthy".to_string()
        },
        timestamp,
    };

    let status_code = if db_healthy {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(response))
}

/// Readiness probe for Kubernetes
///
/// GET /api/v1/health/ready
///
/// Returns readiness status with detailed checks.
/// Used by Kubernetes to determine if the pod is ready to receive traffic.
/// This endpoint is publicly accessible (no auth required).
pub async fn readiness_check(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    // Check database connectivity
    let db_healthy = sqlx::query("SELECT 1").fetch_one(&state.db).await.is_ok();

    let response = ReadinessResponse {
        ready: db_healthy,
        checks: ReadinessChecks {
            database: db_healthy,
        },
    };

    let status_code = if db_healthy {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(response))
}

/// Liveness probe
///
/// GET /api/v1/health/live
///
/// Returns liveness status.
/// Used by Kubernetes to determine if the pod should be restarted.
/// This endpoint is publicly accessible (no auth required).
pub async fn liveness_check() -> impl IntoResponse {
    Json(LivenessResponse { alive: true })
}

/// Prometheus metrics endpoint
///
/// GET /api/v1/metrics
///
/// Returns Prometheus-formatted metrics for monitoring.
/// This endpoint is publicly accessible (no auth required).
pub async fn metrics(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let mut metrics_output = String::new();

    // Helper function to add metric lines
    let mut add_metric =
        |name: &str, help: &str, metric_type: &str, value: &str, labels: Option<&str>| {
            metrics_output.push_str(&format!("# HELP {} {}\n", name, help));
            metrics_output.push_str(&format!("# TYPE {} {}\n", name, metric_type));
            if let Some(lbls) = labels {
                metrics_output.push_str(&format!("{}{} {}\n", name, lbls, value));
            } else {
                metrics_output.push_str(&format!("{} {}\n", name, value));
            }
        };

    // allternit_runs_total - Total number of runs
    let runs_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    add_metric(
        "allternit_runs_total",
        "Total number of runs",
        "counter",
        &runs_total.to_string(),
        None,
    );

    // allternit_runs_active - Currently active runs
    let runs_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runs WHERE status IN ('pending', 'provisioning', 'running')",
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    add_metric(
        "allternit_runs_active",
        "Number of currently active runs",
        "gauge",
        &runs_active.to_string(),
        None,
    );

    // allternit_cloud_instances - Number of cloud instances
    let instances: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instances")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    add_metric(
        "allternit_cloud_instances",
        "Number of cloud instances",
        "gauge",
        &instances.to_string(),
        None,
    );

    // allternit_deployments_total - Total number of deployments
    let deployments_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployments")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    add_metric(
        "allternit_deployments_total",
        "Total number of deployments",
        "counter",
        &deployments_total.to_string(),
        None,
    );

    // allternit_deployments_active - Active deployments
    let deployments_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM deployments WHERE status IN ('pending', 'provisioning', 'installing')"
    )
    .fetch_one(&state.db)
    .await
    .unwrap_or(0);
    add_metric(
        "allternit_deployments_active",
        "Number of active deployments",
        "gauge",
        &deployments_active.to_string(),
        None,
    );

    // API request metrics from middleware
    let (requests_total, requests_errors, duration_micros_total) = state.metrics_state.snapshot();
    add_metric(
        "allternit_api_requests_total",
        "Total number of API requests",
        "counter",
        &requests_total.to_string(),
        None,
    );
    add_metric(
        "allternit_api_request_errors_total",
        "Total number of API request errors (4xx/5xx)",
        "counter",
        &requests_errors.to_string(),
        None,
    );
    add_metric(
        "allternit_api_request_duration_microseconds_total",
        "Total API request duration in microseconds",
        "counter",
        &duration_micros_total.to_string(),
        None,
    );

    // Add timestamp
    let timestamp = Utc::now().timestamp();
    metrics_output.push_str(&format!("\n# Scrape timestamp: {}\n", timestamp));

    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        metrics_output,
    )
}

// ---------------------------------------------------------------------------
// Cloud computer provisioning health (public, for the status page prober)
// ---------------------------------------------------------------------------

/// Hard budget for the whole probe.
const COMPUTERS_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);
/// A result is reused this long so a 1-minute prober cannot load the database.
const COMPUTERS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);
/// Provisioning failures in the last hour that mark the lane degraded.
const COMPUTERS_ERROR_THRESHOLD: i64 = 3;

static COMPUTERS_CACHE: std::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>> =
    std::sync::Mutex::new(None);

/// Pure rollup: reason codes only, no counts or identifiers.
pub(crate) fn computers_status(
    db_ok: bool,
    enabled_hosts: i64,
    hosts_with_capacity: i64,
    recent_errors: i64,
) -> (&'static str, Vec<&'static str>) {
    let mut reasons = Vec::new();
    if !db_ok {
        reasons.push("database_unreachable");
        return ("down", reasons);
    }
    if enabled_hosts == 0 {
        reasons.push("no_enabled_hosts");
        return ("down", reasons);
    }
    if hosts_with_capacity == 0 {
        reasons.push("no_host_capacity");
    }
    if recent_errors >= COMPUTERS_ERROR_THRESHOLD {
        reasons.push("recent_provisioning_failures");
    }
    if reasons.is_empty() {
        ("ok", reasons)
    } else {
        ("degraded", reasons)
    }
}

async fn probe_computers(db: &sqlx::PgPool) -> (&'static str, Vec<&'static str>) {
    if sqlx::query("SELECT 1").fetch_one(db).await.is_err() {
        return computers_status(false, 0, 0, 0);
    }
    let hosts: Option<(i64, i64)> = sqlx::query_as(
        "SELECT COUNT(*)::bigint, \
                COUNT(*) FILTER (WHERE memory_mb_total > memory_mb_allocated \
                                  AND cpu_cores_total > cpu_cores_allocated \
                                  AND disk_gb_total > disk_gb_allocated)::bigint \
         FROM provisioned_hosts WHERE enabled = TRUE",
    )
    .fetch_one(db)
    .await
    .ok();
    let errors: Option<i64> = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM provisioned_instances \
         WHERE status = 'error' AND updated_at > NOW() - INTERVAL '1 hour'",
    )
    .fetch_one(db)
    .await
    .ok();
    match (hosts, errors) {
        (Some((enabled, with_capacity)), Some(errs)) => {
            computers_status(true, enabled, with_capacity, errs)
        }
        // Tables unreadable while the connection works: the lane is not usable.
        _ => ("down", vec!["provisioning_state_unreadable"]),
    }
}

/// Cloud computer provisioning health
///
/// GET /api/v1/health/computers
///
/// Public, unauthenticated. Reports whether provisioning can work: database
/// reachable, at least one enabled host with free capacity, and no burst of
/// recent provisioning failures. Never returns secrets, host names, or counts.
/// 200 for ok/degraded, 503 for down. Cached for 30 seconds.
pub async fn computers_health(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let cached = COMPUTERS_CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().cloned())
        .filter(|(at, _)| at.elapsed() < COMPUTERS_CACHE_TTL)
        .map(|(_, body)| body);
    let body = match cached {
        Some(body) => body,
        None => {
            let (status, reasons) =
                match tokio::time::timeout(COMPUTERS_BUDGET, probe_computers(&state.db)).await {
                    Ok(result) => result,
                    Err(_) => ("degraded", vec!["probe_timeout"]),
                };
            let body = serde_json::json!({
                "status": status,
                "checked_at": Utc::now().to_rfc3339(),
                "details": { "reasons": reasons },
            });
            if let Ok(mut guard) = COMPUTERS_CACHE.lock() {
                *guard = Some((std::time::Instant::now(), body.clone()));
            }
            body
        }
    };
    let code = if body["status"] == "down" {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    } else {
        axum::http::StatusCode::OK
    };
    (code, Json(body))
}

#[cfg(test)]
mod computers_tests {
    use super::computers_status;

    #[test]
    fn rolls_up_reason_codes() {
        assert_eq!(computers_status(true, 2, 1, 0), ("ok", vec![]));
        assert_eq!(computers_status(false, 2, 1, 0).0, "down");
        assert_eq!(computers_status(true, 0, 0, 0), ("down", vec!["no_enabled_hosts"]));
        assert_eq!(computers_status(true, 2, 0, 0), ("degraded", vec!["no_host_capacity"]));
        assert_eq!(
            computers_status(true, 2, 0, 5),
            ("degraded", vec!["no_host_capacity", "recent_provisioning_failures"])
        );
        assert_eq!(computers_status(true, 1, 1, 2).0, "ok");
    }
}
