//! Goals, Routines, and Loops API
//!
//! Surfaces gizzi-code's cron/routine/loop runtime through the Allternit
//! platform API. Goals are platform-owned objectives that can own routines
//! (persistent scheduled jobs) and loops (session-scoped recurring jobs).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::auth::get_user;
use crate::AppState;

fn cron_daemon_base() -> String {
    crate::config::AppConfig::load()
        .cron_daemon_url()
        .trim_end_matches('/')
        .to_string()
}

fn normalize_execution_domain(domain: Option<String>) -> String {
    match domain.as_deref() {
        Some("cloud") | Some("hybrid") => "cloud".to_string(),
        _ => "local".to_string(),
    }
}

fn uses_cloud_scheduler(domain: &str) -> bool {
    domain == "cloud" || domain == "hybrid"
}

/// Returns true when the schedule type can be represented as a Gizzi cron job.
/// Gizzi cron only supports recurring schedules ('cron' and 'interval');
/// manual and once-off jobs are stored locally and triggered by the user.
fn is_recurring_schedule(schedule_type: &str) -> bool {
    schedule_type == "cron" || schedule_type == "interval"
}

// ═══════════════════════════════════════════════════════════════════════════════
// Domain Types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub user_id: String,
    pub workspace_id: Option<String>,
    pub agent_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub target_date: Option<String>,
    pub progress: i32,
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routine {
    pub id: String,
    pub user_id: String,
    pub workspace_id: Option<String>,
    pub agent_id: Option<String>,
    pub goal_id: Option<String>,
    pub gizzi_job_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub schedule_type: String,
    pub schedule_expression: String,
    pub timezone: Option<String>,
    pub execution_domain: String,
    pub config: serde_json::Value,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
    pub max_runs: Option<i32>,
    pub timeout_seconds: Option<i32>,
    pub max_retries: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
    /// Local scheduler cursor (V185); None for cloud routines and one-shots
    /// that already ran.
    #[serde(default)]
    pub next_run_at: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Loop {
    pub id: String,
    pub user_id: String,
    pub workspace_id: Option<String>,
    pub agent_id: Option<String>,
    pub goal_id: Option<String>,
    pub gizzi_job_id: Option<String>,
    pub session_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub schedule_type: String,
    pub schedule_expression: String,
    pub execution_domain: String,
    pub config: serde_json::Value,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineRun {
    pub id: String,
    pub routine_id: String,
    pub gizzi_run_id: Option<String>,
    pub status: String,
    pub scheduled_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i32>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub attempt: i32,
    pub triggered_by: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineMetrics {
    pub routine_id: String,
    pub total_runs: i64,
    pub successful_runs: i64,
    pub failed_runs: i64,
    pub success_rate: f64,
    pub average_duration_ms: Option<f64>,
    pub last_run_status: Option<String>,
    pub last_run_at: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Request / Response Types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
pub struct CreateGoalRequest {
    pub title: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub target_date: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGoalRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub target_date: Option<String>,
    #[serde(default)]
    pub progress: Option<i32>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRoutineRequest {
    pub name: String,
    pub schedule_type: String,
    pub schedule_expression: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub execution_domain: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub max_runs: Option<i32>,
    #[serde(default)]
    pub timeout_seconds: Option<i32>,
    #[serde(default)]
    pub max_retries: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoutineRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub schedule_type: Option<String>,
    #[serde(default)]
    pub schedule_expression: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub execution_domain: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub max_runs: Option<i32>,
    #[serde(default)]
    pub timeout_seconds: Option<i32>,
    #[serde(default)]
    pub max_retries: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateLoopRequest {
    pub name: String,
    pub schedule_type: String,
    pub schedule_expression: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub execution_domain: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLoopRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub schedule_type: Option<String>,
    #[serde(default)]
    pub schedule_expression: Option<String>,
    #[serde(default)]
    pub execution_domain: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper: gizzi cron bridge
// ═══════════════════════════════════════════════════════════════════════════════

async fn cron_daemon_create_job(
    client: &reqwest::Client,
    job: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/jobs", cron_daemon_base());
    let res = client
        .post(&url)
        .json(&job)
        .send()
        .await
        .map_err(|e| format!("cron daemon create failed: {}", e))?;

    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!("cron daemon create error: {}", text));
    }

    res.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("cron daemon create decode failed: {}", e))
}

async fn cron_daemon_update_job(
    client: &reqwest::Client,
    daemon_job_id: &str,
    job: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/jobs/{}", cron_daemon_base(), daemon_job_id);
    let res = client
        .patch(&url)
        .json(&job)
        .send()
        .await
        .map_err(|e| format!("cron daemon update failed: {}", e))?;

    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!("cron daemon update error: {}", text));
    }

    res.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("cron daemon update decode failed: {}", e))
}

async fn cron_daemon_delete_job(
    client: &reqwest::Client,
    daemon_job_id: &str,
) -> Result<(), String> {
    let url = format!("{}/jobs/{}", cron_daemon_base(), daemon_job_id);
    let res = client
        .delete(&url)
        .send()
        .await
        .map_err(|e| format!("cron daemon delete failed: {}", e))?;

    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!("cron daemon delete error: {}", text));
    }

    Ok(())
}

async fn cron_daemon_run_job(
    client: &reqwest::Client,
    daemon_job_id: &str,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/jobs/{}/run", cron_daemon_base(), daemon_job_id);
    let res = client
        .post(&url)
        .send()
        .await
        .map_err(|e| format!("cron daemon run failed: {}", e))?;

    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!("cron daemon run error: {}", text));
    }

    res.json::<serde_json::Value>()
        .await
        .map_err(|e| format!("cron daemon run decode failed: {}", e))
}

/// Fetch a gizzi cron job's run history (`GET /jobs/:id/runs` — the daemon's
/// persisted `CronRun` records). Short timeout: this runs on the read path of
/// `list_routine_runs`, so a hung daemon must degrade to stored rows quickly.
async fn cron_daemon_list_runs(
    client: &reqwest::Client,
    daemon_job_id: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let url = format!("{}/jobs/{}/runs", cron_daemon_base(), daemon_job_id);
    let res = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("cron daemon runs fetch failed: {}", e))?;

    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!("cron daemon runs error: {}", text));
    }

    res.json::<Vec<serde_json::Value>>()
        .await
        .map_err(|e| format!("cron daemon runs decode failed: {}", e))
}

/// Upsert fetched gizzi cron run history into `routine_runs` for one routine.
/// Idempotent: matches on `gizzi_run_id` first, falling back to
/// (routine_id, scheduled_at) for rows written before gizzi ids were tracked;
/// matched rows are refreshed (status/output/error/duration move as the
/// daemon's record evolves), unmatched runs are inserted. Returns the number
/// of runs processed. Kept synchronous (no .await while holding the
/// connection) so the calling handler's future stays Send.
fn upsert_routine_runs(
    conn: &rusqlite::Connection,
    routine_id: &str,
    runs: &[serde_json::Value],
) -> usize {
    let mut synced = 0usize;
    for run in runs {
        let gizzi_run_id = run.get("id").and_then(|v| v.as_str());
        // scheduled_at is NOT NULL locally; gizzi always sets scheduledAt,
        // with startedAt as a sane fallback.
        let scheduled_at = run
            .get("scheduledAt")
            .or_else(|| run.get("startedAt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if scheduled_at.is_empty() {
            continue;
        }
        // Gizzi RunStatus (pending|running|completed|failed|aborted) maps
        // directly onto the local status column.
        let status = run
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending");
        let started_at = run.get("startedAt").and_then(|v| v.as_str());
        let finished_at = run.get("finishedAt").and_then(|v| v.as_str());
        let duration_ms = run.get("durationMs").and_then(|v| v.as_i64());
        let output = run.get("output").and_then(|v| v.as_str());
        let error = run.get("error").and_then(|v| v.as_str());
        let attempt = run
            .get("attempt")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let triggered_by = run
            .get("triggeredBy")
            .and_then(|v| v.as_str())
            .unwrap_or("schedule");
        let metadata = run
            .get("metadata")
            .filter(|m| m.is_object())
            .map(|m| m.to_string());

        let existing_id: Option<String> = gizzi_run_id
            .and_then(|rid| {
                conn.query_row(
                    "SELECT id FROM routine_runs WHERE gizzi_run_id = ?1",
                    [rid],
                    |row| row.get(0),
                )
                .ok()
            })
            .or_else(|| {
                conn.query_row(
                    "SELECT id FROM routine_runs WHERE routine_id = ?1 AND scheduled_at = ?2",
                    rusqlite::params![routine_id, scheduled_at],
                    |row| row.get(0),
                )
                .ok()
            });

        let result = match existing_id {
            Some(local_id) => conn.execute(
                "UPDATE routine_runs SET gizzi_run_id = ?2, status = ?3, started_at = ?4,
                        finished_at = ?5, duration_ms = ?6, output = ?7, error = ?8,
                        attempt = ?9, triggered_by = ?10, metadata = ?11
                 WHERE id = ?1",
                rusqlite::params![
                    local_id,
                    gizzi_run_id,
                    status,
                    started_at,
                    finished_at,
                    duration_ms,
                    output,
                    error,
                    attempt,
                    triggered_by,
                    metadata,
                ],
            ),
            None => conn.execute(
                "INSERT INTO routine_runs
                    (id, routine_id, gizzi_run_id, status, scheduled_at, started_at,
                     finished_at, duration_ms, output, error, attempt, triggered_by, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    routine_id,
                    gizzi_run_id,
                    status,
                    scheduled_at,
                    started_at,
                    finished_at,
                    duration_ms,
                    output,
                    error,
                    attempt,
                    triggered_by,
                    metadata,
                ],
            ),
        };
        if let Err(e) = result {
            warn!(
                "routine-runs sync: failed to upsert run {:?} for routine {}: {}",
                gizzi_run_id, routine_id, e
            );
        } else {
            synced += 1;
        }
    }
    synced
}

/// Extract the daemon job id from a Gizzi cron response.
/// Gizzi returns `{ "id": "...", ... }`, but some older versions may nest
/// it under `job.id`; accept both shapes for backward compatibility.
fn extract_daemon_job_id(res: &serde_json::Value) -> Option<String> {
    res.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            res.get("job")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// Convert a platform interval expression (e.g. "5m", "1h") into seconds.
/// Returns `None` when the expression is not a recognised interval shorthand.
pub(crate) fn parse_interval_seconds(expression: &str) -> Option<i64> {
    let s = expression.trim();
    let num_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if num_end == 0 {
        return None;
    }
    let num: i64 = s[..num_end].parse().ok()?;
    let unit = s[num_end..].trim().to_lowercase();
    let multiplier = match unit.as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
        "d" | "day" | "days" => 60 * 60 * 24,
        _ => return None,
    };
    Some(num * multiplier)
}

/// Build the schedule payload expected by Gizzi's CronService.
/// Interval schedules are converted to `seconds`; cron schedules keep their
/// expression so Gizzi can compute the next run time.
fn build_gizzi_schedule(schedule_type: &str, schedule_expression: &str) -> serde_json::Value {
    if schedule_type == "interval" {
        if let Some(seconds) = parse_interval_seconds(schedule_expression) {
            json!({ "type": "interval", "seconds": seconds })
        } else {
            json!({ "type": "cron", "expression": schedule_expression })
        }
    } else {
        json!({ "type": "cron", "expression": schedule_expression })
    }
}

async fn resolve_agent_harness(
    db: &crate::db::DbHandle,
    agent_id: &str,
) -> Option<serde_json::Value> {
    let db = db.clone();
    let agent_id = agent_id.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().ok()?;
        let harness: String = conn
            .query_row(
                "SELECT harness_config FROM agents WHERE id = ?1",
                params![agent_id],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str::<serde_json::Value>(&harness).ok()
    })
    .await
    .ok()
    .flatten()
}

fn build_daemon_job_from_routine(
    routine: &Routine,
    job_type: &str,
    harness_config: Option<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "name": routine.name,
        "description": routine.description,
        "type": job_type,
        "status": routine.status,
        "schedule": build_gizzi_schedule(&routine.schedule_type, &routine.schedule_expression),
        "config": routine.config,
        "scope": "persistent",
        "agentId": routine.agent_id,
        "harness": harness_config,
        "tags": routine.tags,
        "metadata": routine.metadata,
        "maxRuns": routine.max_runs,
        "timeoutSeconds": routine.timeout_seconds,
        "maxRetries": routine.max_retries,
    })
}

fn build_daemon_job_from_loop(
    loop_item: &Loop,
    job_type: &str,
    harness_config: Option<serde_json::Value>,
) -> serde_json::Value {
    json!({
        "name": loop_item.name,
        "description": loop_item.description,
        "type": job_type,
        "status": loop_item.status,
        "schedule": build_gizzi_schedule(&loop_item.schedule_type, &loop_item.schedule_expression),
        "config": loop_item.config,
        "scope": "session",
        "sessionId": loop_item.session_id,
        "agentId": loop_item.agent_id,
        "harness": harness_config,
        "expiresAt": loop_item.expires_at,
        "tags": loop_item.tags,
        "metadata": loop_item.metadata,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Goals Handlers
// ═══════════════════════════════════════════════════════════════════════════════

async fn list_goals(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<Goal>>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, workspace_id, agent_id, title, description, status, priority, target_date, progress, metadata, created_at, updated_at
             FROM goals WHERE user_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let goals = stmt
        .query_map([&user.user_id], |row| {
            Ok(Goal {
                id: row.get(0)?,
                user_id: row.get(1)?,
                workspace_id: row.get(2)?,
                agent_id: row.get(3)?,
                title: row.get(4)?,
                description: row.get(5)?,
                status: row.get(6)?,
                priority: row.get(7)?,
                target_date: row.get(8)?,
                progress: row.get(9)?,
                metadata: row
                    .get::<_, Option<String>>(10)?
                    .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(goals))
}

async fn create_goal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateGoalRequest>,
) -> Result<Json<Goal>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    conn.execute(
        "INSERT INTO goals (id, user_id, workspace_id, agent_id, title, description, status, priority, target_date, progress, metadata, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        (
            &id,
            &user.user_id,
            &req.workspace_id,
            &req.agent_id,
            &req.title,
            &req.description,
            "active",
            req.priority.as_deref().unwrap_or("medium"),
            &req.target_date,
            0i32,
            req.metadata.as_ref().map(|m| m.to_string()),
            &now,
            &now,
        ),
    )
    .map_err(|e| {
        warn!("insert goal failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!("goal created: {}", id);

    Ok(Json(Goal {
        id,
        user_id: user.user_id,
        workspace_id: req.workspace_id,
        agent_id: req.agent_id,
        title: req.title,
        description: req.description,
        status: "active".to_string(),
        priority: req.priority.unwrap_or_else(|| "medium".to_string()),
        target_date: req.target_date,
        progress: 0,
        metadata: req.metadata,
        created_at: now.clone(),
        updated_at: now,
    }))
}

async fn get_goal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Goal>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let goal = conn
        .query_row(
            "SELECT id, user_id, workspace_id, agent_id, title, description, status, priority, target_date, progress, metadata, created_at, updated_at
             FROM goals WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| {
                Ok(Goal {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    agent_id: row.get(3)?,
                    title: row.get(4)?,
                    description: row.get(5)?,
                    status: row.get(6)?,
                    priority: row.get(7)?,
                    target_date: row.get(8)?,
                    progress: row.get(9)?,
                    metadata: row.get::<_, Option<String>>(10)?.map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            },
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(goal))
}

async fn update_goal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateGoalRequest>,
) -> Result<Json<Goal>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let existing = conn
        .query_row(
            "SELECT status, progress FROM goals WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?)),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let title = req.title;
    let description = req.description;
    let status = req.status.unwrap_or(existing.0);
    let priority = req.priority;
    let target_date = req.target_date;
    let progress = req.progress.unwrap_or(existing.1);
    let metadata = req.metadata;
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE goals SET
            title = COALESCE(?1, title),
            description = COALESCE(?2, description),
            status = ?3,
            priority = COALESCE(?4, priority),
            target_date = COALESCE(?5, target_date),
            progress = ?6,
            metadata = COALESCE(?7, metadata),
            updated_at = ?8
         WHERE id = ?9 AND user_id = ?10",
        (
            title.as_ref(),
            description.as_ref(),
            &status,
            priority.as_ref(),
            target_date.as_ref(),
            progress,
            metadata.as_ref().map(|m| m.to_string()),
            &now,
            &id,
            &user.user_id,
        ),
    )
    .map_err(|e| {
        warn!("update goal failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    get_goal(State(state), headers, Path(id)).await
}

async fn delete_goal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    conn.execute(
        "DELETE FROM goals WHERE id = ?1 AND user_id = ?2",
        [&id, &user.user_id],
    )
    .map_err(|e| {
        warn!("delete goal failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!("goal deleted: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn list_goal_children(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(goal_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let routines: Vec<Routine> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, name, description, status, schedule_type, schedule_expression, timezone, execution_domain, config, tags, metadata, max_runs, timeout_seconds, max_retries, created_at, updated_at, next_run_at, last_run_at
                 FROM routines WHERE goal_id = ?1 AND user_id = ?2 ORDER BY created_at DESC",
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let rows = stmt
            .query_map([&goal_id, &user.user_id], |row| row_to_routine(row))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let loops: Vec<Loop> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, session_id, name, description, status, schedule_type, schedule_expression, execution_domain, config, tags, metadata, expires_at, created_at, updated_at
                 FROM loops WHERE goal_id = ?1 AND user_id = ?2 ORDER BY created_at DESC",
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let rows = stmt
            .query_map([&goal_id, &user.user_id], |row| row_to_loop(row))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    Ok(Json(json!({
        "routines": routines,
        "loops": loops,
    })))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Routines Handlers
// ═══════════════════════════════════════════════════════════════════════════════

fn row_to_routine(row: &rusqlite::Row) -> Result<Routine, rusqlite::Error> {
    Ok(Routine {
        id: row.get(0)?,
        user_id: row.get(1)?,
        workspace_id: row.get(2)?,
        agent_id: row.get(3)?,
        goal_id: row.get(4)?,
        gizzi_job_id: row.get(5)?,
        name: row.get(6)?,
        description: row.get(7)?,
        status: row.get(8)?,
        schedule_type: row.get(9)?,
        schedule_expression: row.get(10)?,
        timezone: row.get(11)?,
        execution_domain: row.get(12)?,
        config: row
            .get::<_, String>(13)
            .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null))?,
        tags: row
            .get::<_, Option<String>>(14)?
            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
        metadata: row
            .get::<_, Option<String>>(15)?
            .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
        max_runs: row.get(16)?,
        timeout_seconds: row.get(17)?,
        max_retries: row.get(18)?,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
        // Optional trailing columns: queries that omit them still map.
        next_run_at: row.get::<_, Option<String>>(21).ok().flatten(),
        last_run_at: row.get::<_, Option<String>>(22).ok().flatten(),
    })
}

async fn list_routines(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<Routine>>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, name, description, status, schedule_type, schedule_expression, timezone, execution_domain, config, tags, metadata, max_runs, timeout_seconds, max_retries, created_at, updated_at, next_run_at, last_run_at
             FROM routines WHERE user_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let routines = stmt
        .query_map([&user.user_id], row_to_routine)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(routines))
}

async fn create_routine(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateRoutineRequest>,
) -> Result<Json<Routine>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let config = req.config.unwrap_or_else(|| json!({}));
    let job_type = config
        .get("jobType")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_string();

    let execution_domain = normalize_execution_domain(req.execution_domain.clone());

    let routine = Routine {
        id: id.clone(),
        user_id: user.user_id.clone(),
        workspace_id: req.workspace_id.clone(),
        agent_id: req.agent_id.clone(),
        goal_id: req.goal_id.clone(),
        gizzi_job_id: None,
        name: req.name.clone(),
        description: req.description.clone(),
        status: "active".to_string(),
        schedule_type: req.schedule_type.clone(),
        schedule_expression: req.schedule_expression.clone(),
        timezone: req.timezone.clone(),
        execution_domain,
        config: config.clone(),
        tags: req.tags.clone(),
        metadata: req.metadata.clone(),
        max_runs: req.max_runs,
        timeout_seconds: req.timeout_seconds,
        max_retries: req.max_retries,
        created_at: now.clone(),
        updated_at: now.clone(),
        next_run_at: None,
        last_run_at: None,
    };

    let daemon_job_id = if uses_cloud_scheduler(&routine.execution_domain)
        && is_recurring_schedule(&routine.schedule_type)
    {
        let harness_config = if let Some(ref agent_id) = routine.agent_id {
            resolve_agent_harness(&state.db, agent_id).await
        } else {
            None
        };
        let daemon_job = build_daemon_job_from_routine(&routine, &job_type, harness_config);
        let client = reqwest::Client::new();
        match cron_daemon_create_job(&client, daemon_job).await {
            Ok(res) => extract_daemon_job_id(&res),
            Err(e) => {
                warn!("failed to create daemon routine job: {}", e);
                None
            }
        }
    } else {
        None
    };

    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    conn.execute(
        "INSERT INTO routines (id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, name, description, status, schedule_type, schedule_expression, timezone, execution_domain, config, tags, metadata, max_runs, timeout_seconds, max_retries, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
        params![
            &routine.id,
            &routine.user_id,
            &routine.workspace_id,
            &routine.agent_id,
            &routine.goal_id,
            &daemon_job_id,
            &routine.name,
            &routine.description,
            &routine.status,
            &routine.schedule_type,
            &routine.schedule_expression,
            &routine.timezone,
            &routine.execution_domain,
            routine.config.to_string(),
            routine.tags.as_ref().map(|t| serde_json::to_string(t).unwrap_or_default()),
            routine.metadata.as_ref().map(|m| m.to_string()),
            routine.max_runs,
            routine.timeout_seconds,
            routine.max_retries,
            &routine.created_at,
            &routine.updated_at,
        ],
    )
    .map_err(|e| {
        warn!("insert routine failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!("routine created: {} daemon_job_id={:?}", id, daemon_job_id);

    Ok(Json(Routine {
        gizzi_job_id: daemon_job_id,
        ..routine
    }))
}

async fn get_routine(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Routine>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let routine = conn
        .query_row(
            "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, name, description, status, schedule_type, schedule_expression, timezone, execution_domain, config, tags, metadata, max_runs, timeout_seconds, max_retries, created_at, updated_at, next_run_at, last_run_at
             FROM routines WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            row_to_routine,
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(routine))
}

async fn update_routine(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateRoutineRequest>,
) -> Result<Json<Routine>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let existing = conn
        .query_row(
            "SELECT config, gizzi_job_id, execution_domain FROM routines WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let mut config: serde_json::Value = serde_json::from_str(&existing.0).unwrap_or_default();
    if let Some(new_config) = req.config {
        config = new_config;
    }

    let job_type = config
        .get("jobType")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_string();

    let new_execution_domain = req
        .execution_domain
        .as_ref()
        .map(|d| normalize_execution_domain(Some(d.clone())));

    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE routines SET
            name = COALESCE(?1, name),
            description = COALESCE(?2, description),
            status = COALESCE(?3, status),
            schedule_type = COALESCE(?4, schedule_type),
            schedule_expression = COALESCE(?5, schedule_expression),
            timezone = COALESCE(?6, timezone),
            execution_domain = COALESCE(?7, execution_domain),
            config = COALESCE(?8, config),
            tags = COALESCE(?9, tags),
            metadata = COALESCE(?10, metadata),
            max_runs = COALESCE(?11, max_runs),
            timeout_seconds = COALESCE(?12, timeout_seconds),
            max_retries = COALESCE(?13, max_retries),
            updated_at = ?14,
            -- Any schedule/status/domain change resets the local scheduler
            -- cursor so it recomputes from the new definition; a new
            -- schedule also re-arms a one-shot that already ran.
            next_run_at = CASE WHEN ?3 IS NOT NULL OR ?4 IS NOT NULL OR ?5 IS NOT NULL OR ?7 IS NOT NULL
                               THEN NULL ELSE next_run_at END,
            last_run_at = CASE WHEN ?4 IS NOT NULL OR ?5 IS NOT NULL THEN NULL ELSE last_run_at END
         WHERE id = ?15 AND user_id = ?16",
        (
            req.name.as_ref(),
            req.description.as_ref(),
            req.status.as_ref(),
            req.schedule_type.as_ref(),
            req.schedule_expression.as_ref(),
            req.timezone.as_ref(),
            new_execution_domain.as_ref(),
            Some(config.to_string()),
            req.tags
                .as_ref()
                .map(|t| serde_json::to_string(t).unwrap_or_default()),
            req.metadata.as_ref().map(|m| m.to_string()),
            req.max_runs,
            req.timeout_seconds,
            req.max_retries,
            &now,
            &id,
            &user.user_id,
        ),
    )
    .map_err(|e| {
        warn!("update routine failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let updated_routine = get_routine(State(state.clone()), headers.clone(), Path(id.clone()))
        .await?
        .0;
    let old_uses_cloud = uses_cloud_scheduler(&existing.2);
    let new_uses_cloud = uses_cloud_scheduler(&updated_routine.execution_domain);

    if old_uses_cloud && new_uses_cloud {
        if let Some(ref gizzi_job_id) = existing.1 {
            let harness_config = if let Some(ref agent_id) = updated_routine.agent_id {
                resolve_agent_harness(&state.db, agent_id).await
            } else {
                None
            };
            let gizzi_job =
                build_daemon_job_from_routine(&updated_routine, &job_type, harness_config);
            let client = reqwest::Client::new();
            if let Err(e) = cron_daemon_update_job(&client, gizzi_job_id, gizzi_job).await {
                warn!("failed to update daemon routine job: {}", e);
            }
        }
    } else if old_uses_cloud && !new_uses_cloud {
        if let Some(ref gizzi_job_id) = existing.1 {
            let client = reqwest::Client::new();
            if let Err(e) = cron_daemon_delete_job(&client, gizzi_job_id).await {
                warn!(
                    "failed to delete daemon routine job on domain change: {}",
                    e
                );
            }
        }
        conn.execute(
            "UPDATE routines SET gizzi_job_id = NULL, updated_at = ?1 WHERE id = ?2 AND user_id = ?3",
            (&now, &id, &user.user_id),
        )
        .map_err(|e| {
            warn!("failed to clear gizzi_job_id: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    } else if !old_uses_cloud && new_uses_cloud {
        let harness_config = if let Some(ref agent_id) = updated_routine.agent_id {
            resolve_agent_harness(&state.db, agent_id).await
        } else {
            None
        };
        let daemon_job = build_daemon_job_from_routine(&updated_routine, &job_type, harness_config);
        let client = reqwest::Client::new();
        let daemon_job_id = match cron_daemon_create_job(&client, daemon_job).await {
            Ok(res) => extract_daemon_job_id(&res),
            Err(e) => {
                warn!(
                    "failed to create daemon routine job on domain change: {}",
                    e
                );
                None
            }
        };
        if let Some(daemon_job_id) = daemon_job_id {
            conn.execute(
                "UPDATE routines SET gizzi_job_id = ?1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4",
                (&daemon_job_id, &now, &id, &user.user_id),
            )
            .map_err(|e| {
                warn!("failed to set gizzi_job_id: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }
    }

    get_routine(State(state), headers, Path(id)).await
}

async fn delete_routine(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let gizzi_job_id: Option<String> = conn
        .query_row(
            "SELECT gizzi_job_id FROM routines WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if let Some(gizzi_job_id) = gizzi_job_id {
        let client = reqwest::Client::new();
        if let Err(e) = cron_daemon_delete_job(&client, &gizzi_job_id).await {
            warn!("failed to delete daemon routine job: {}", e);
        }
    }

    conn.execute(
        "DELETE FROM routines WHERE id = ?1 AND user_id = ?2",
        [&id, &user.user_id],
    )
    .map_err(|e| {
        warn!("delete routine failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!("routine deleted: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn run_routine(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (gizzi_job_id, execution_domain): (Option<String>, String) = conn
        .query_row(
            "SELECT gizzi_job_id, execution_domain FROM routines WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if !uses_cloud_scheduler(&execution_domain) {
        // Local routines run on this API (routine_local_scheduler). Answer
        // at once; the run lands in routine_runs and the bot's thread.
        let routine = crate::routine_local_scheduler::get_routine(&state.db, &id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND)?;
        let guard = crate::routine_local_scheduler::FlightGuard::acquire(&id).ok_or(StatusCode::CONFLICT)?;
        let user_id = user.user_id.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let driver = crate::routine_local_scheduler::GizziDriver { state: state.clone() };
            crate::routine_local_scheduler::execute(
                &state.db,
                &driver,
                &routine,
                crate::routine_local_scheduler::Trigger::Manual { user_id },
            )
            .await;
        });
        return Ok(Json(json!({ "status": "running", "routineId": id, "domain": "local" })));
    }

    let gizzi_job_id = gizzi_job_id.ok_or(StatusCode::BAD_REQUEST)?;
    let client = reqwest::Client::new();
    let run = cron_daemon_run_job(&client, &gizzi_job_id)
        .await
        .map_err(|e| {
            warn!("routine run failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(run))
}

async fn list_routine_runs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let gizzi_job_id: Option<String> = conn
        .query_row(
            "SELECT gizzi_job_id FROM routines WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // On-read sync: pull the gizzi cron daemon's persisted run history into
    // routine_runs before serving, so the read is truthful without a poller.
    // Never fails the read — a daemon outage is reported in the `sync` field
    // and the stored rows are served as-is.
    let mut sync = json!({"attempted": false, "synced": 0, "error": serde_json::Value::Null});
    if let Some(ref job_id) = gizzi_job_id {
        sync["attempted"] = json!(true);
        let client = reqwest::Client::new();
        match cron_daemon_list_runs(&client, job_id).await {
            Ok(runs) => {
                let n = upsert_routine_runs(&conn, &id, &runs);
                sync["synced"] = json!(n);
            }
            Err(e) => {
                warn!("routine-runs sync failed for routine {}: {}", id, e);
                sync["error"] = json!(e);
            }
        }
    }

    let mut stmt = conn
        .prepare(
            "SELECT id, routine_id, gizzi_run_id, status, scheduled_at, started_at, finished_at, duration_ms, output, error, attempt, triggered_by, metadata
             FROM routine_runs WHERE routine_id = ?1 ORDER BY scheduled_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let runs = stmt
        .query_map([&id], |row| {
            Ok(RoutineRun {
                id: row.get(0)?,
                routine_id: row.get(1)?,
                gizzi_run_id: row.get(2)?,
                status: row.get(3)?,
                scheduled_at: row.get(4)?,
                started_at: row.get(5)?,
                finished_at: row.get(6)?,
                duration_ms: row.get(7)?,
                output: row.get(8)?,
                error: row.get(9)?,
                attempt: row.get(10)?,
                triggered_by: row.get(11)?,
                metadata: row
                    .get::<_, Option<String>>(12)?
                    .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({"runs": runs, "sync": sync})))
}

async fn get_routine_metrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<RoutineMetrics>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    conn.query_row(
        "SELECT 1 FROM routines WHERE id = ?1 AND user_id = ?2",
        [&id, &user.user_id],
        |_| Ok(()),
    )
    .map_err(|_| StatusCode::NOT_FOUND)?;

    let metrics = conn
        .query_row(
            "SELECT
                COUNT(*),
                COUNT(CASE WHEN status = 'completed' THEN 1 END),
                COUNT(CASE WHEN status = 'failed' THEN 1 END),
                AVG(CASE WHEN status = 'completed' THEN duration_ms END),
                (SELECT status FROM routine_runs WHERE routine_id = ?1 ORDER BY scheduled_at DESC LIMIT 1),
                (SELECT scheduled_at FROM routine_runs WHERE routine_id = ?1 ORDER BY scheduled_at DESC LIMIT 1)
             FROM routine_runs WHERE routine_id = ?1",
            [&id],
            |row| {
                let total_runs: i64 = row.get(0)?;
                let successful_runs: i64 = row.get(1)?;
                let failed_runs: i64 = row.get(2)?;
                let success_rate = if total_runs > 0 {
                    successful_runs as f64 / total_runs as f64
                } else {
                    0.0
                };
                Ok(RoutineMetrics {
                    routine_id: id.clone(),
                    total_runs,
                    successful_runs,
                    failed_runs,
                    success_rate,
                    average_duration_ms: row.get(3)?,
                    last_run_status: row.get(4)?,
                    last_run_at: row.get(5)?,
                })
            },
        )
        .map_err(|e| {
            warn!("metrics query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(metrics))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Loops Handlers
// ═══════════════════════════════════════════════════════════════════════════════

fn row_to_loop(row: &rusqlite::Row) -> Result<Loop, rusqlite::Error> {
    Ok(Loop {
        id: row.get(0)?,
        user_id: row.get(1)?,
        workspace_id: row.get(2)?,
        agent_id: row.get(3)?,
        goal_id: row.get(4)?,
        gizzi_job_id: row.get(5)?,
        session_id: row.get(6)?,
        name: row.get(7)?,
        description: row.get(8)?,
        status: row.get(9)?,
        schedule_type: row.get(10)?,
        schedule_expression: row.get(11)?,
        execution_domain: row.get(12)?,
        config: row
            .get::<_, String>(13)
            .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null))?,
        tags: row
            .get::<_, Option<String>>(14)?
            .map(|s| serde_json::from_str(&s).unwrap_or_default()),
        metadata: row
            .get::<_, Option<String>>(15)?
            .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
        expires_at: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

async fn list_loops(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<Loop>>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, session_id, name, description, status, schedule_type, schedule_expression, execution_domain, config, tags, metadata, expires_at, created_at, updated_at
             FROM loops WHERE user_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let loops = stmt
        .query_map([&user.user_id], row_to_loop)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(loops))
}

async fn create_loop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateLoopRequest>,
) -> Result<Json<Loop>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let config = req.config.unwrap_or_else(|| json!({}));
    let job_type = config
        .get("jobType")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_string();

    let execution_domain = normalize_execution_domain(req.execution_domain.clone());

    let loop_item = Loop {
        id: id.clone(),
        user_id: user.user_id.clone(),
        workspace_id: req.workspace_id.clone(),
        agent_id: req.agent_id.clone(),
        goal_id: req.goal_id.clone(),
        gizzi_job_id: None,
        session_id: req.session_id.clone(),
        name: req.name.clone(),
        description: req.description.clone(),
        status: "active".to_string(),
        schedule_type: req.schedule_type.clone(),
        schedule_expression: req.schedule_expression.clone(),
        execution_domain,
        config: config.clone(),
        tags: req.tags.clone(),
        metadata: req.metadata.clone(),
        expires_at: req.expires_at.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };

    let daemon_job_id = if uses_cloud_scheduler(&loop_item.execution_domain)
        && is_recurring_schedule(&loop_item.schedule_type)
    {
        let harness_config = if let Some(ref agent_id) = loop_item.agent_id {
            resolve_agent_harness(&state.db, agent_id).await
        } else {
            None
        };
        let daemon_job = build_daemon_job_from_loop(&loop_item, &job_type, harness_config);
        let client = reqwest::Client::new();
        match cron_daemon_create_job(&client, daemon_job).await {
            Ok(res) => extract_daemon_job_id(&res),
            Err(e) => {
                warn!("failed to create daemon loop job: {}", e);
                None
            }
        }
    } else {
        None
    };

    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    conn.execute(
        "INSERT INTO loops (id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, session_id, name, description, status, schedule_type, schedule_expression, execution_domain, config, tags, metadata, expires_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            &loop_item.id,
            &loop_item.user_id,
            &loop_item.workspace_id,
            &loop_item.agent_id,
            &loop_item.goal_id,
            &daemon_job_id,
            &loop_item.session_id,
            &loop_item.name,
            &loop_item.description,
            &loop_item.status,
            &loop_item.schedule_type,
            &loop_item.schedule_expression,
            &loop_item.execution_domain,
            loop_item.config.to_string(),
            loop_item.tags.as_ref().map(|t| serde_json::to_string(t).unwrap_or_default()),
            loop_item.metadata.as_ref().map(|m| m.to_string()),
            &loop_item.expires_at,
            &loop_item.created_at,
            &loop_item.updated_at,
        ],
    )
    .map_err(|e| {
        warn!("insert loop failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!("loop created: {} daemon_job_id={:?}", id, daemon_job_id);

    Ok(Json(Loop {
        gizzi_job_id: daemon_job_id,
        ..loop_item
    }))
}

async fn get_loop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Loop>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let loop_item = conn
        .query_row(
            "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, session_id, name, description, status, schedule_type, schedule_expression, execution_domain, config, tags, metadata, expires_at, created_at, updated_at
             FROM loops WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            row_to_loop,
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(loop_item))
}

async fn update_loop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateLoopRequest>,
) -> Result<Json<Loop>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let existing = conn
        .query_row(
            "SELECT config, gizzi_job_id, execution_domain FROM loops WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let mut config: serde_json::Value = serde_json::from_str(&existing.0).unwrap_or_default();
    if let Some(new_config) = req.config {
        config = new_config;
    }

    let job_type = config
        .get("jobType")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_string();

    let new_execution_domain = req
        .execution_domain
        .as_ref()
        .map(|d| normalize_execution_domain(Some(d.clone())));

    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE loops SET
            name = COALESCE(?1, name),
            description = COALESCE(?2, description),
            status = COALESCE(?3, status),
            schedule_type = COALESCE(?4, schedule_type),
            schedule_expression = COALESCE(?5, schedule_expression),
            execution_domain = COALESCE(?6, execution_domain),
            config = COALESCE(?7, config),
            tags = COALESCE(?8, tags),
            metadata = COALESCE(?9, metadata),
            expires_at = COALESCE(?10, expires_at),
            updated_at = ?11
         WHERE id = ?12 AND user_id = ?13",
        (
            req.name.as_ref(),
            req.description.as_ref(),
            req.status.as_ref(),
            req.schedule_type.as_ref(),
            req.schedule_expression.as_ref(),
            new_execution_domain.as_ref(),
            Some(config.to_string()),
            req.tags
                .as_ref()
                .map(|t| serde_json::to_string(t).unwrap_or_default()),
            req.metadata.as_ref().map(|m| m.to_string()),
            req.expires_at.as_ref(),
            &now,
            &id,
            &user.user_id,
        ),
    )
    .map_err(|e| {
        warn!("update loop failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let updated_loop = get_loop(State(state.clone()), headers.clone(), Path(id.clone()))
        .await?
        .0;
    let old_uses_cloud = uses_cloud_scheduler(&existing.2);
    let new_uses_cloud = uses_cloud_scheduler(&updated_loop.execution_domain);

    if old_uses_cloud && new_uses_cloud {
        if let Some(ref gizzi_job_id) = existing.1 {
            let harness_config = if let Some(ref agent_id) = updated_loop.agent_id {
                resolve_agent_harness(&state.db, agent_id).await
            } else {
                None
            };
            let gizzi_job = build_daemon_job_from_loop(&updated_loop, &job_type, harness_config);
            let client = reqwest::Client::new();
            if let Err(e) = cron_daemon_update_job(&client, gizzi_job_id, gizzi_job).await {
                warn!("failed to update daemon loop job: {}", e);
            }
        }
    } else if old_uses_cloud && !new_uses_cloud {
        if let Some(ref gizzi_job_id) = existing.1 {
            let client = reqwest::Client::new();
            if let Err(e) = cron_daemon_delete_job(&client, gizzi_job_id).await {
                warn!("failed to delete daemon loop job on domain change: {}", e);
            }
        }
        conn.execute(
            "UPDATE loops SET gizzi_job_id = NULL, updated_at = ?1 WHERE id = ?2 AND user_id = ?3",
            (&now, &id, &user.user_id),
        )
        .map_err(|e| {
            warn!("failed to clear loop gizzi_job_id: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    } else if !old_uses_cloud && new_uses_cloud {
        let harness_config = if let Some(ref agent_id) = updated_loop.agent_id {
            resolve_agent_harness(&state.db, agent_id).await
        } else {
            None
        };
        let daemon_job = build_daemon_job_from_loop(&updated_loop, &job_type, harness_config);
        let client = reqwest::Client::new();
        let daemon_job_id = match cron_daemon_create_job(&client, daemon_job).await {
            Ok(res) => extract_daemon_job_id(&res),
            Err(e) => {
                warn!("failed to create daemon loop job on domain change: {}", e);
                None
            }
        };
        if let Some(daemon_job_id) = daemon_job_id {
            conn.execute(
                "UPDATE loops SET gizzi_job_id = ?1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4",
                (&daemon_job_id, &now, &id, &user.user_id),
            )
            .map_err(|e| {
                warn!("failed to set loop gizzi_job_id: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        }
    }

    get_loop(State(state), headers, Path(id)).await
}

async fn delete_loop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let gizzi_job_id: Option<String> = conn
        .query_row(
            "SELECT gizzi_job_id FROM loops WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| row.get(0),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if let Some(gizzi_job_id) = gizzi_job_id {
        let client = reqwest::Client::new();
        if let Err(e) = cron_daemon_delete_job(&client, &gizzi_job_id).await {
            warn!("failed to delete daemon loop job: {}", e);
        }
    }

    conn.execute(
        "DELETE FROM loops WHERE id = ?1 AND user_id = ?2",
        [&id, &user.user_id],
    )
    .map_err(|e| {
        warn!("delete loop failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    info!("loop deleted: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn run_loop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (gizzi_job_id, execution_domain): (Option<String>, String) = conn
        .query_row(
            "SELECT gizzi_job_id, execution_domain FROM loops WHERE id = ?1 AND user_id = ?2",
            [&id, &user.user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    if !uses_cloud_scheduler(&execution_domain) {
        warn!("loop {} is local-domain; run via local scheduler", id);
        return Err(StatusCode::NOT_IMPLEMENTED);
    }

    let gizzi_job_id = gizzi_job_id.ok_or(StatusCode::BAD_REQUEST)?;
    let client = reqwest::Client::new();
    let run = cron_daemon_run_job(&client, &gizzi_job_id)
        .await
        .map_err(|e| {
            warn!("loop run failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(run))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalSchedules {
    pub routines: Vec<Routine>,
    pub loops: Vec<Loop>,
}

async fn list_local_schedules(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<LocalSchedules>, StatusCode> {
    let user = get_user(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let conn = state.db.connect().map_err(|e| {
        warn!("db error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut routines_stmt = conn
        .prepare(
            "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, name, description, status, schedule_type, schedule_expression, timezone, execution_domain, config, tags, metadata, max_runs, timeout_seconds, max_retries, created_at, updated_at, next_run_at, last_run_at
             FROM routines WHERE user_id = ?1 AND execution_domain = 'local' ORDER BY updated_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let routines = routines_stmt
        .query_map([&user.user_id], row_to_routine)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut loops_stmt = conn
        .prepare(
            "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, session_id, name, description, status, schedule_type, schedule_expression, execution_domain, config, tags, metadata, expires_at, created_at, updated_at
             FROM loops WHERE user_id = ?1 AND execution_domain = 'local' ORDER BY updated_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let loops = loops_stmt
        .query_map([&user.user_id], row_to_loop)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(LocalSchedules { routines, loops }))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Router
// ═══════════════════════════════════════════════════════════════════════════════

pub fn automation_router() -> Router<Arc<AppState>> {
    Router::new()
        // Goals
        .route("/automation/goals", get(list_goals).post(create_goal))
        .route(
            "/automation/goals/:id",
            get(get_goal).put(update_goal).delete(delete_goal),
        )
        .route("/automation/goals/:id/children", get(list_goal_children))
        // Routines
        .route(
            "/automation/routines",
            get(list_routines).post(create_routine),
        )
        .route(
            "/automation/routines/:id",
            get(get_routine).put(update_routine).delete(delete_routine),
        )
        .route("/automation/routines/:id/run", post(run_routine))
        .route("/automation/routines/:id/runs", get(list_routine_runs))
        .route("/automation/routines/:id/metrics", get(get_routine_metrics))
        // Loops
        .route("/automation/loops", get(list_loops).post(create_loop))
        .route(
            "/automation/loops/:id",
            get(get_loop).put(update_loop).delete(delete_loop),
        )
        .route("/automation/loops/:id/run", post(run_loop))
        // Local scheduler poll endpoint
        .route("/automation/local-schedules", get(list_local_schedules))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    fn test_db() -> (String, DbHandle) {
        let id = uuid::Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(format!("allternit-schedule-test-{}.db", id));
        let db = DbHandle::new(path.clone()).unwrap();
        (path.to_string_lossy().to_string(), db)
    }

    fn cleanup(path: &str) {
        let _ = std::fs::remove_file(path);
    }

    fn insert_test_routine(conn: &rusqlite::Connection, id: &str, user_id: &str) {
        conn.execute(
            "INSERT INTO routines (id, user_id, name, status, schedule_type, schedule_expression, timezone, execution_domain, config, created_at, updated_at)
             VALUES (?1, ?2, 'Test Routine', 'active', 'cron', '0 9 * * *', 'UTC', 'local', '{}', datetime('now'), datetime('now'))",
            rusqlite::params![id, user_id],
        ).unwrap();
    }

    fn insert_test_run(
        conn: &rusqlite::Connection,
        id: &str,
        routine_id: &str,
        status: &str,
        duration_ms: i32,
        scheduled_at: &str,
    ) {
        conn.execute(
            "INSERT INTO routine_runs (id, routine_id, status, scheduled_at, duration_ms, attempt, triggered_by)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, 'schedule')",
            rusqlite::params![id, routine_id, status, scheduled_at, duration_ms],
        ).unwrap();
    }

    #[test]
    fn test_routine_metrics_aggregation() {
        let (path, db) = test_db();
        let conn = db.connect().unwrap();
        let routine_id = "routine-1";
        let user_id = "user-1";

        insert_test_routine(&conn, routine_id, user_id);
        insert_test_run(
            &conn,
            "run-1",
            routine_id,
            "completed",
            100,
            "2024-01-01T10:00:00Z",
        );
        insert_test_run(
            &conn,
            "run-2",
            routine_id,
            "completed",
            200,
            "2024-01-01T11:00:00Z",
        );
        insert_test_run(
            &conn,
            "run-3",
            routine_id,
            "failed",
            0,
            "2024-01-01T12:00:00Z",
        );

        let metrics: RoutineMetrics = conn
            .query_row(
                "SELECT
                    COUNT(*),
                    COUNT(CASE WHEN status = 'completed' THEN 1 END),
                    COUNT(CASE WHEN status = 'failed' THEN 1 END),
                    AVG(CASE WHEN status = 'completed' THEN duration_ms END),
                    (SELECT status FROM routine_runs WHERE routine_id = ?1 ORDER BY scheduled_at DESC LIMIT 1),
                    (SELECT scheduled_at FROM routine_runs WHERE routine_id = ?1 ORDER BY scheduled_at DESC LIMIT 1)
                 FROM routine_runs WHERE routine_id = ?1",
                [routine_id],
                |row| {
                    let total_runs: i64 = row.get(0)?;
                    let successful_runs: i64 = row.get(1)?;
                    let failed_runs: i64 = row.get(2)?;
                    Ok(RoutineMetrics {
                        routine_id: routine_id.to_string(),
                        total_runs,
                        successful_runs,
                        failed_runs,
                        success_rate: successful_runs as f64 / total_runs as f64,
                        average_duration_ms: row.get(3)?,
                        last_run_status: row.get(4)?,
                        last_run_at: row.get(5)?,
                    })
                },
            )
            .unwrap();

        assert_eq!(metrics.total_runs, 3);
        assert_eq!(metrics.successful_runs, 2);
        assert_eq!(metrics.failed_runs, 1);
        assert!((metrics.success_rate - 0.6667).abs() < 0.01);
        assert!(metrics.average_duration_ms.unwrap() >= 150.0);
        assert_eq!(metrics.last_run_status.as_deref(), Some("failed"));
        cleanup(&path);
    }

    #[test]
    fn test_routine_metrics_empty_runs() {
        let (path, db) = test_db();
        let conn = db.connect().unwrap();
        let routine_id = "routine-empty";
        let user_id = "user-empty";

        insert_test_routine(&conn, routine_id, user_id);

        let metrics: RoutineMetrics = conn
            .query_row(
                "SELECT
                    COUNT(*),
                    COUNT(CASE WHEN status = 'completed' THEN 1 END),
                    COUNT(CASE WHEN status = 'failed' THEN 1 END),
                    AVG(CASE WHEN status = 'completed' THEN duration_ms END),
                    (SELECT status FROM routine_runs WHERE routine_id = ?1 ORDER BY scheduled_at DESC LIMIT 1),
                    (SELECT scheduled_at FROM routine_runs WHERE routine_id = ?1 ORDER BY scheduled_at DESC LIMIT 1)
                 FROM routine_runs WHERE routine_id = ?1",
                [routine_id],
                |row| {
                    let total_runs: i64 = row.get(0)?;
                    let successful_runs: i64 = row.get(1)?;
                    let failed_runs: i64 = row.get(2)?;
                    Ok(RoutineMetrics {
                        routine_id: routine_id.to_string(),
                        total_runs,
                        successful_runs,
                        failed_runs,
                        success_rate: if total_runs > 0 { successful_runs as f64 / total_runs as f64 } else { 0.0 },
                        average_duration_ms: row.get(3)?,
                        last_run_status: row.get(4)?,
                        last_run_at: row.get(5)?,
                    })
                },
            )
            .unwrap();

        assert_eq!(metrics.total_runs, 0);
        assert_eq!(metrics.successful_runs, 0);
        assert_eq!(metrics.failed_runs, 0);
        assert_eq!(metrics.success_rate, 0.0);
        assert!(metrics.average_duration_ms.is_none());
        assert!(metrics.last_run_status.is_none());
        cleanup(&path);
    }

    #[test]
    fn test_execution_domain_normalization() {
        assert_eq!(
            normalize_execution_domain(Some("local".to_string())),
            "local"
        );
        assert_eq!(
            normalize_execution_domain(Some("cloud".to_string())),
            "cloud"
        );
        assert_eq!(
            normalize_execution_domain(Some("hybrid".to_string())),
            "cloud"
        );
        assert_eq!(
            normalize_execution_domain(Some("invalid".to_string())),
            "local"
        );
        assert_eq!(normalize_execution_domain(None), "local");

        assert!(uses_cloud_scheduler("cloud"));
        assert!(uses_cloud_scheduler("hybrid"));
        assert!(!uses_cloud_scheduler("local"));
        assert!(!uses_cloud_scheduler("unknown"));
    }

    #[test]
    fn test_local_schedules_query() {
        let (path, db) = test_db();
        let conn = db.connect().unwrap();

        conn.execute(
            "INSERT INTO routines (id, user_id, name, status, schedule_type, schedule_expression, timezone, execution_domain, config, created_at, updated_at)
             VALUES ('r-local', 'user-1', 'Local Routine', 'active', 'cron', '0 9 * * *', 'UTC', 'local', '{}', datetime('now'), datetime('now'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO routines (id, user_id, name, status, schedule_type, schedule_expression, timezone, execution_domain, config, created_at, updated_at)
             VALUES ('r-cloud', 'user-1', 'Cloud Routine', 'active', 'cron', '0 10 * * *', 'UTC', 'cloud', '{}', datetime('now'), datetime('now'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO loops (id, user_id, name, status, schedule_type, schedule_expression, execution_domain, config, created_at, updated_at)
             VALUES ('l-local', 'user-1', 'Local Loop', 'active', 'interval', '5m', 'local', '{}', datetime('now'), datetime('now'))",
            [],
        ).unwrap();

        let mut routines_stmt = conn
            .prepare(
                "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, name, description, status, schedule_type, schedule_expression, timezone, execution_domain, config, tags, metadata, max_runs, timeout_seconds, max_retries, created_at, updated_at, next_run_at, last_run_at
                 FROM routines WHERE user_id = ?1 AND execution_domain = 'local' ORDER BY updated_at DESC",
            )
            .unwrap();
        let routines: Vec<Routine> = routines_stmt
            .query_map(["user-1"], row_to_routine)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        let mut loops_stmt = conn
            .prepare(
                "SELECT id, user_id, workspace_id, agent_id, goal_id, gizzi_job_id, session_id, name, description, status, schedule_type, schedule_expression, execution_domain, config, tags, metadata, expires_at, created_at, updated_at
                 FROM loops WHERE user_id = ?1 AND execution_domain = 'local' ORDER BY updated_at DESC",
            )
            .unwrap();
        let loops: Vec<Loop> = loops_stmt
            .query_map(["user-1"], row_to_loop)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(routines.len(), 1);
        assert_eq!(routines[0].id, "r-local");
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].id, "l-local");
        cleanup(&path);
    }
}
