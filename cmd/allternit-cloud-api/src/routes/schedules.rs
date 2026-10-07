//! Schedule routes for Cowork Runtime
//!
//! Provides REST API endpoints for cron/periodic scheduling.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{db::cowork_models::*, ApiError, ApiState};

/// Query parameters for listing schedules
#[derive(Debug, Deserialize, Default)]
pub struct ListSchedulesQuery {
    pub enabled: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Create schedule request
#[derive(Debug, Deserialize)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub description: Option<String>,
    pub cron_expr: String,
    pub natural_lang: Option<String>,
    pub timezone: Option<String>,
    pub job_template: JobConfig,
    pub enabled: Option<bool>,
    pub misfire_policy: Option<MisfirePolicy>,
    /// Optional region preference for scheduled runs
    pub region_id: Option<String>,
}

/// Update schedule request
#[derive(Debug, Deserialize, Default)]
pub struct UpdateScheduleRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub cron_expr: Option<String>,
    pub natural_lang: Option<String>,
    pub timezone: Option<String>,
    pub job_template: Option<JobConfig>,
    pub enabled: Option<bool>,
    pub misfire_policy: Option<MisfirePolicy>,
}

/// Calculate next run time from cron expression
///
/// Supports 5-field cron (minute hour day month weekday) by prepending a seconds
/// field, and 6/7-field cron as-is.
pub(crate) fn calculate_next_run(cron_expr: &str, timezone: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono_tz::Tz;
    use std::str::FromStr;

    let trimmed = cron_expr.trim();
    let field_count = trimmed.split_whitespace().count();

    // cron 0.15 expects seconds as the first field; prepend "0" for classic 5-field cron.
    let expression = if field_count == 5 {
        format!("0 {}", trimmed)
    } else {
        trimmed.to_string()
    };

    // Parse cron expression
    let schedule = cron::Schedule::from_str(&expression).ok()?;

    // Parse timezone, fallback to UTC if invalid
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);

    // Get next occurrence in target timezone
    schedule
        .upcoming(tz)
        .next()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// List all schedules
pub async fn list_schedules(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<ListSchedulesQuery>,
) -> Result<Json<Vec<ScheduleSummary>>, ApiError> {
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    let schedules = if let Some(enabled) = query.enabled {
        sqlx::query_as::<_, ScheduleSummary>(
            "SELECT id, name, enabled, cron_expr, natural_lang, next_run_at, run_count 
             FROM schedules 
             WHERE enabled = $1 
             ORDER BY created_at DESC 
             LIMIT $2 OFFSET $3",
        )
        .bind(enabled)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?
    } else {
        sqlx::query_as::<_, ScheduleSummary>(
            "SELECT id, name, enabled, cron_expr, natural_lang, next_run_at, run_count 
             FROM schedules 
             ORDER BY created_at DESC 
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?
    };

    Ok(Json(schedules))
}

/// Create a new schedule
pub async fn create_schedule(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<CreateScheduleRequest>,
) -> Result<Json<Schedule>, ApiError> {
    tracing::info!("Creating schedule: {}", request.name);

    // Validate cron expression (basic check)
    if request.cron_expr.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Cron expression cannot be empty".to_string(),
        ));
    }

    // Validate region if specified
    if let Some(ref region_id) = request.region_id {
        let region_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM regions WHERE id = $1 AND active = TRUE)",
        )
        .bind(region_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

        if !region_exists {
            return Err(ApiError::BadRequest(format!(
                "Region '{}' not found or inactive",
                region_id
            )));
        }
    }

    let schedule_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let timezone = request.timezone.unwrap_or_else(|| "UTC".to_string());
    let enabled = request.enabled.unwrap_or(true);
    let misfire_policy = request.misfire_policy.unwrap_or_default();

    // Calculate next run time
    let next_run_at = calculate_next_run(&request.cron_expr, &timezone);

    let schedule = sqlx::query_as::<_, Schedule>(
        r#"
        INSERT INTO schedules (
            id, name, description, cron_expr, natural_lang, timezone,
            job_template, enabled, misfire_policy, last_run_at, next_run_at,
            run_count, misfire_count, owner_id, tenant_id, region_id, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
        RETURNING *
        "#,
    )
    .bind(&schedule_id)
    .bind(&request.name)
    .bind(&request.description)
    .bind(&request.cron_expr)
    .bind(&request.natural_lang)
    .bind(&timezone)
    .bind(sqlx::types::Json(request.job_template))
    .bind(enabled)
    .bind(misfire_policy)
    .bind(None::<chrono::DateTime<chrono::Utc>>) // last_run_at
    .bind(next_run_at)
    .bind(0i32) // run_count
    .bind(0i32) // misfire_count
    .bind(None::<String>) // owner_id
    .bind(None::<String>) // tenant_id
    .bind(request.region_id) // region_id
    .bind(now)
    .bind(now)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::DatabaseError(e))?;

    tracing::info!("Schedule created: {}", schedule_id);
    Ok(Json(schedule))
}

/// Get schedule by ID
pub async fn get_schedule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Schedule>, ApiError> {
    let schedule = sqlx::query_as::<_, Schedule>("SELECT * FROM schedules WHERE id = $1")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

    schedule
        .ok_or_else(|| ApiError::NotFound(format!("Schedule not found: {}", id)))
        .map(Json)
}

/// Update schedule
pub async fn update_schedule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(request): Json<UpdateScheduleRequest>,
) -> Result<Json<Schedule>, ApiError> {
    // Get existing schedule
    let schedule = sqlx::query_as::<_, Schedule>("SELECT * FROM schedules WHERE id = $1")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

    let schedule =
        schedule.ok_or_else(|| ApiError::NotFound(format!("Schedule not found: {}", id)))?;

    let name = request.name.unwrap_or_else(|| schedule.name.clone());
    let description = request.description.or(schedule.description.clone());
    let cron_expr = request
        .cron_expr
        .clone()
        .unwrap_or_else(|| schedule.cron_expr.clone());
    let natural_lang = request.natural_lang.or(schedule.natural_lang.clone());
    let timezone = request
        .timezone
        .unwrap_or_else(|| schedule.timezone.clone());
    let job_template = request
        .job_template
        .map(sqlx::types::Json)
        .unwrap_or(schedule.job_template.clone());
    let enabled = request.enabled.unwrap_or(schedule.enabled);
    let misfire_policy = request.misfire_policy.unwrap_or(schedule.misfire_policy);

    // Recalculate next run if cron changed
    let next_run_at = if request.cron_expr.is_some() {
        calculate_next_run(&cron_expr, &timezone)
    } else {
        schedule.next_run_at
    };

    let updated = sqlx::query_as::<_, Schedule>(
        r#"
        UPDATE schedules 
        SET name = $1, description = $2, cron_expr = $3, natural_lang = $4, timezone = $5,
            job_template = $6, enabled = $7, misfire_policy = $8, next_run_at = $9, updated_at = $10
        WHERE id = $11
        RETURNING *
        "#,
    )
    .bind(&name)
    .bind(&description)
    .bind(&cron_expr)
    .bind(&natural_lang)
    .bind(&timezone)
    .bind(job_template)
    .bind(enabled)
    .bind(misfire_policy)
    .bind(next_run_at)
    .bind(chrono::Utc::now())
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::DatabaseError(e))?;

    Ok(Json(updated))
}

/// Delete a schedule
pub async fn delete_schedule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<(), ApiError> {
    let result = sqlx::query("DELETE FROM schedules WHERE id = $1")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("Schedule not found: {}", id)));
    }

    Ok(())
}

/// Enable a schedule
pub async fn enable_schedule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Schedule>, ApiError> {
    // Get schedule to recalculate next run
    let schedule = sqlx::query_as::<_, Schedule>("SELECT * FROM schedules WHERE id = $1")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?
        .ok_or_else(|| ApiError::NotFound(format!("Schedule not found: {}", id)))?;

    let next_run_at = calculate_next_run(&schedule.cron_expr, &schedule.timezone);

    let updated = sqlx::query_as::<_, Schedule>(
        r#"
        UPDATE schedules 
        SET enabled = $1, next_run_at = $2, updated_at = $3
        WHERE id = $4
        RETURNING *
        "#,
    )
    .bind(true)
    .bind(next_run_at)
    .bind(chrono::Utc::now())
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::DatabaseError(e))?;

    Ok(Json(updated))
}

/// Disable a schedule
pub async fn disable_schedule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Schedule>, ApiError> {
    let updated = sqlx::query_as::<_, Schedule>(
        r#"
        UPDATE schedules 
        SET enabled = $1, next_run_at = $2, updated_at = $3
        WHERE id = $4
        RETURNING *
        "#,
    )
    .bind(false)
    .bind(None::<chrono::DateTime<chrono::Utc>>)
    .bind(chrono::Utc::now())
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::DatabaseError(e))?;

    Ok(Json(updated))
}

/// Trigger a schedule immediately (create a run)
pub async fn trigger_schedule(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Get schedule
    let schedule = sqlx::query_as::<_, Schedule>("SELECT * FROM schedules WHERE id = $1")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::DatabaseError(e))?
        .ok_or_else(|| ApiError::NotFound(format!("Schedule not found: {}", id)))?;

    // Create a run from this schedule
    let run_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();

    let run = sqlx::query_as::<_, Run>(
        r#"
        INSERT INTO runs (
            id, name, description, mode, status, step_cursor, total_steps, completed_steps,
            config, owner_id, tenant_id, runtime_id, runtime_type, schedule_id, region_id,
            created_at, updated_at, started_at, completed_at, error_message, error_details
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)
        RETURNING *
        "#,
    )
    .bind(&run_id)
    .bind(format!("Scheduled: {}", schedule.name))
    .bind(schedule.description.clone())
    .bind(RunMode::Remote) // Default to remote for scheduled runs
    .bind(RunStatus::Pending)
    .bind(None::<String>)
    .bind(None::<i32>)
    .bind(0i32)
    .bind(sqlx::types::Json(RunConfig {
        working_dir: schedule.job_template.0.working_dir.clone(),
        env: schedule.job_template.0.env.clone(),
        command: Some(schedule.job_template.0.command.clone()),
        args: schedule.job_template.0.args.clone(),
        resource_limits: None,
        sync: None,
        extra: serde_json::json!({"scheduled": true}),
    }))
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(&id) // schedule_id
    .bind(schedule.region_id.clone()) // Use schedule's region preference
    .bind(now)
    .bind(now)
    .bind(None::<chrono::DateTime<chrono::Utc>>)
    .bind(None::<chrono::DateTime<chrono::Utc>>)
    .bind(None::<String>)
    .bind(None::<sqlx::types::Json<serde_json::Value>>)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::DatabaseError(e))?;

    // Update schedule run count
    let _ =
        sqlx::query("UPDATE schedules SET run_count = run_count + 1, last_run_at = $1 WHERE id = $2")
            .bind(now)
            .bind(&id)
            .execute(&state.db)
            .await;

    Ok(Json(serde_json::json!({
        "message": "Schedule triggered",
        "schedule_id": id,
        "run_id": run_id,
        "run": run
    })))
}
