//! Analytics routes
//!
//! Organization-scoped LLM usage analytics for the admin control plane.
//! Aggregates `llm_usage_events` into time-series and per-user summaries.

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, AppState};

pub fn analytics_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/analytics/csp-violation", post(analytics_csp_violation))
        .route("/analytics/gizzi-code/events", post(ingest_gizzi_code_events))
        .route("/admin/analytics/cost-over-time", get(cost_over_time))
        .route("/admin/analytics/token-usage", get(token_usage))
        .route("/admin/analytics/request-volume", get(request_volume))
        .route("/admin/analytics/per-user-cost", get(per_user_cost))
        .route("/admin/analytics/active-users", get(active_users))
        .route("/admin/analytics/artifact-activity", get(artifact_activity))
        .route("/admin/analytics/chat-project-usage", get(chat_project_usage))
        .route("/admin/analytics/connector-usage", get(connector_usage))
        .route("/admin/analytics/plugin-usage", get(plugin_usage))
        .route("/admin/analytics/skill-usage", get(skill_usage))
        .route("/admin/analytics/gizzi-code/usage", get(gizzi_code_usage))
}

async fn analytics_csp_violation(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "stub": true,
    }))
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "analytics query failed");
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        err.to_string(),
    )
}

fn admin_org(conn: &rusqlite::Connection, user: &AuthUser) -> Result<String, ApiError> {
    let org = user.organization_id.as_deref().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required.",
        )
    })?;
    if !crate::rbac::is_org_admin(conn, org, &user.user_id).map_err(internal)? {
        return Err(error(
            StatusCode::FORBIDDEN,
            "insufficient_role",
            "Only organization owners/admins can view analytics.",
        ));
    }
    Ok(org.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Granularity {
    Hour,
    Day,
    Week,
    Month,
}

impl Default for Granularity {
    fn default() -> Self {
        Self::Day
    }
}

impl Granularity {
    fn strftime(&self) -> &'static str {
        match self {
            Granularity::Hour => "%Y-%m-%d %H:00",
            Granularity::Day => "%Y-%m-%d",
            Granularity::Week => "%Y-%W",
            Granularity::Month => "%Y-%m",
        }
    }
}

#[derive(Debug, Deserialize)]
struct AnalyticsQuery {
    organization_id: String,
    start: String,
    end: String,
    #[serde(default)]
    granularity: Granularity,
}

fn validate_query(query: &AnalyticsQuery, org: &str) -> Result<(), ApiError> {
    if query.organization_id != org {
        return Err(error(
            StatusCode::FORBIDDEN,
            "organization_mismatch",
            "Requested organization does not match the active organization.",
        ));
    }
    if query.start >= query.end {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_range",
            "start must be before end.",
        ));
    }
    Ok(())
}

fn bucket_sql(granularity: &Granularity) -> String {
    format!("strftime('{}', created_at)", granularity.strftime())
}

fn bucket_expr(alias: &str, granularity: &Granularity) -> String {
    format!("strftime('{}', {}.created_at)", granularity.strftime(), alias)
}

async fn cost_over_time(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket = bucket_sql(&query.granularity);
    let sql = format!(
        "SELECT {bucket} AS bucket,
                SUM(cost_microdollars) AS cost_microdollars,
                COUNT(*) AS requests
         FROM llm_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "cost_microdollars": row.get::<_, i64>(1)?,
                "requests": row.get::<_, i64>(2)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn token_usage(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket = bucket_sql(&query.granularity);
    let sql = format!(
        "SELECT {bucket} AS bucket,
                SUM(prompt_tokens) AS prompt_tokens,
                SUM(completion_tokens) AS completion_tokens,
                SUM(reasoning_tokens) AS reasoning_tokens,
                SUM(cached_tokens) AS cached_tokens,
                COUNT(*) AS requests
         FROM llm_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "prompt_tokens": row.get::<_, i64>(1)?,
                "completion_tokens": row.get::<_, i64>(2)?,
                "reasoning_tokens": row.get::<_, i64>(3)?,
                "cached_tokens": row.get::<_, i64>(4)?,
                "requests": row.get::<_, i64>(5)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn request_volume(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket = bucket_sql(&query.granularity);
    let sql = format!(
        "SELECT {bucket} AS bucket,
                COUNT(*) AS requests,
                SUM(CASE WHEN status = 'ok' THEN 1 ELSE 0 END) AS ok,
                SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) AS errors,
                SUM(CASE WHEN status = 'budget_exceeded' THEN 1 ELSE 0 END) AS budget_exceeded,
                SUM(CASE WHEN status = 'rate_limited' THEN 1 ELSE 0 END) AS rate_limited,
                SUM(CASE WHEN status = 'dlp_blocked' THEN 1 ELSE 0 END) AS dlp_blocked
         FROM llm_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "requests": row.get::<_, i64>(1)?,
                "ok": row.get::<_, i64>(2)?,
                "errors": row.get::<_, i64>(3)?,
                "budget_exceeded": row.get::<_, i64>(4)?,
                "rate_limited": row.get::<_, i64>(5)?,
                "dlp_blocked": row.get::<_, i64>(6)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

#[derive(Debug, Deserialize)]
struct PerUserQuery {
    organization_id: String,
    start: String,
    end: String,
}

async fn per_user_cost(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<PerUserQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    if query.organization_id != org {
        return Err(error(
            StatusCode::FORBIDDEN,
            "organization_mismatch",
            "Requested organization does not match the active organization.",
        ));
    }
    if query.start >= query.end {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_range",
            "start must be before end.",
        ));
    }

    let mut stmt = conn
        .prepare(
            "SELECT user_id,
                    SUM(cost_microdollars) AS cost_microdollars,
                    COUNT(*) AS requests
             FROM llm_usage_events
             WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3
             GROUP BY user_id
             ORDER BY cost_microdollars DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "user_id": row.get::<_, String>(0)?,
                "cost_microdollars": row.get::<_, i64>(1)?,
                "requests": row.get::<_, i64>(2)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn active_users(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket = bucket_sql(&query.granularity);
    let daily_sql = format!(
        "SELECT {bucket} AS bucket,
                COUNT(DISTINCT user_id) AS unique_users
         FROM llm_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&daily_sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "unique_users": row.get::<_, i64>(1)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    let total_unique: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT user_id)
             FROM llm_usage_events
             WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3",
            params![org, query.start, query.end],
            |row| row.get(0),
        )
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({
        "daily": rows,
        "total_unique_users": total_unique,
    })))
}

async fn artifact_activity(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket_a = bucket_expr("a", &query.granularity);
    let bucket_r = bucket_expr("r", &query.granularity);
    let bucket_s = bucket_expr("s", &query.granularity);
    let sql = format!(
        "WITH artifacts_count AS (
            SELECT {bucket_a} AS bucket,
                   COUNT(*) AS artifacts_created
            FROM artifacts a
            JOIN admin_workspaces aw ON aw.id = a.workspace_id
            WHERE aw.organization_id = ?1
              AND a.created_at >= ?2 AND a.created_at < ?3
            GROUP BY bucket
         ),
         revisions_count AS (
            SELECT {bucket_r} AS bucket,
                   COUNT(*) AS revisions_created
            FROM artifact_revisions r
            JOIN artifacts a ON a.id = r.artifact_id
            JOIN admin_workspaces aw ON aw.id = a.workspace_id
            WHERE aw.organization_id = ?1
              AND r.created_at >= ?2 AND r.created_at < ?3
            GROUP BY bucket
         ),
         sections_count AS (
            SELECT {bucket_s} AS bucket,
                   COUNT(*) AS sections_created
            FROM artifact_sections s
            JOIN artifacts a ON a.id = s.artifact_id
            JOIN admin_workspaces aw ON aw.id = a.workspace_id
            WHERE aw.organization_id = ?1
              AND s.created_at >= ?2 AND s.created_at < ?3
            GROUP BY bucket
         ),
         all_buckets(bucket) AS (
            SELECT bucket FROM artifacts_count
            UNION SELECT bucket FROM revisions_count
            UNION SELECT bucket FROM sections_count
         )
         SELECT b.bucket,
                COALESCE(a.artifacts_created, 0) AS artifacts_created,
                COALESCE(r.revisions_created, 0) AS revisions_created,
                COALESCE(s.sections_created, 0) AS sections_created
         FROM all_buckets b
         LEFT JOIN artifacts_count a ON a.bucket = b.bucket
         LEFT JOIN revisions_count r ON r.bucket = b.bucket
         LEFT JOIN sections_count s ON s.bucket = b.bucket
         ORDER BY b.bucket"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "artifacts_created": row.get::<_, i64>(1)?,
                "revisions_created": row.get::<_, i64>(2)?,
                "sections_created": row.get::<_, i64>(3)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn chat_project_usage(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket_p = bucket_expr("p", &query.granularity);
    let bucket_s = bucket_expr("s", &query.granularity);
    let sql = format!(
        "WITH org_users AS (
            SELECT user_id FROM organization_members WHERE organization_id = ?1
         ),
         projects_count AS (
            SELECT {bucket_p} AS bucket,
                   COUNT(*) AS projects_created
            FROM cowork_projects p
            WHERE p.user_id IN org_users
              AND p.created_at >= ?2 AND p.created_at < ?3
            GROUP BY bucket
         ),
         sessions_count AS (
            SELECT {bucket_s} AS bucket,
                   COUNT(*) AS sessions_created,
                   COUNT(DISTINCT s.project_id) AS projects_active
            FROM cowork_sessions s
            WHERE s.user_id IN org_users
              AND s.created_at >= ?2 AND s.created_at < ?3
            GROUP BY bucket
         ),
         all_buckets(bucket) AS (
            SELECT bucket FROM projects_count
            UNION SELECT bucket FROM sessions_count
         )
         SELECT b.bucket,
                COALESCE(p.projects_created, 0) AS projects_created,
                COALESCE(s.sessions_created, 0) AS sessions_created,
                COALESCE(s.projects_active, 0) AS projects_active
         FROM all_buckets b
         LEFT JOIN projects_count p ON p.bucket = b.bucket
         LEFT JOIN sessions_count s ON s.bucket = b.bucket
         ORDER BY b.bucket"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "projects_created": row.get::<_, i64>(1)?,
                "sessions_created": row.get::<_, i64>(2)?,
                "projects_active": row.get::<_, i64>(3)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn connector_usage(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<PerUserQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    if query.organization_id != org {
        return Err(error(
            StatusCode::FORBIDDEN,
            "organization_mismatch",
            "Requested organization does not match the active organization.",
        ));
    }
    if query.start >= query.end {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_range",
            "start must be before end.",
        ));
    }

    let mut stmt = conn
        .prepare(
            "SELECT c.connector_id,
                    COUNT(*) AS connection_count,
                    SUM(CASE WHEN c.status = 'connected' THEN 1 ELSE 0 END) AS active_count,
                    SUM(CASE WHEN c.status = 'error' THEN 1 ELSE 0 END) AS error_count
             FROM connector_connections c
             WHERE c.user_id IN (SELECT user_id FROM organization_members WHERE organization_id = ?1)
               AND c.created_at >= ?2 AND c.created_at < ?3
             GROUP BY c.connector_id
             ORDER BY connection_count DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "connector_id": row.get::<_, String>(0)?,
                "connection_count": row.get::<_, i64>(1)?,
                "active_count": row.get::<_, i64>(2)?,
                "error_count": row.get::<_, i64>(3)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn plugin_usage(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket = bucket_sql(&query.granularity);
    let sql = format!(
        "SELECT {bucket} AS bucket,
                plugin_id,
                action,
                COUNT(*) AS invocations
         FROM plugin_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3
         GROUP BY bucket, plugin_id, action
         ORDER BY bucket, invocations DESC"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "plugin_id": row.get::<_, String>(1)?,
                "action": row.get::<_, String>(2)?,
                "invocations": row.get::<_, i64>(3)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn skill_usage(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket_ts = format!(
        "strftime('{}', ts.installed_at)",
        query.granularity.strftime()
    );
    let bucket_e = bucket_expr("e", &query.granularity);
    let sql = format!(
        "WITH installed AS (
            SELECT {bucket_ts} AS bucket,
                   COUNT(*) AS skills_installed
            FROM team_skills ts
            JOIN admin_workspaces aw ON aw.id = ts.workspace_id
            WHERE aw.organization_id = ?1
              AND ts.installed_at >= ?2 AND ts.installed_at < ?3
            GROUP BY bucket
         ),
         invoked AS (
            SELECT {bucket_e} AS bucket,
                   skill_id,
                   action,
                   COUNT(*) AS invocations
            FROM skill_usage_events e
            WHERE tenant_id = ?1 AND e.created_at >= ?2 AND e.created_at < ?3
            GROUP BY bucket, skill_id, action
         ),
         all_buckets(bucket) AS (
            SELECT bucket FROM installed
            UNION SELECT bucket FROM invoked
         )
         SELECT b.bucket,
                COALESCE(i.skills_installed, 0) AS skills_installed,
                COALESCE(iv.skill_id, '') AS skill_id,
                COALESCE(iv.action, '') AS action,
                COALESCE(iv.invocations, 0) AS invocations
         FROM all_buckets b
         LEFT JOIN installed i ON i.bucket = b.bucket
         LEFT JOIN invoked iv ON iv.bucket = b.bucket
         ORDER BY b.bucket"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "skills_installed": row.get::<_, i64>(1)?,
                "skill_id": row.get::<_, String>(2)?,
                "action": row.get::<_, String>(3)?,
                "invocations": row.get::<_, i64>(4)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

// ─── Gizzi-code (Allternit CLI) usage analytics ───────────────────────────────

#[derive(Debug, Deserialize)]
struct GizziCodeUsageEvent {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default = "default_gizzi_event_type")]
    event_type: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    prompt_tokens: i64,
    #[serde(default)]
    completion_tokens: i64,
    #[serde(default)]
    cost_microdollars: i64,
    #[serde(default)]
    tool_calls_accepted: i64,
    #[serde(default)]
    tool_calls_rejected: i64,
    #[serde(default)]
    lines_accepted: i64,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    created_at: Option<String>,
}

fn default_gizzi_event_type() -> String {
    "turn".to_string()
}

#[derive(Debug, Deserialize)]
struct IngestGizziCodeEvents {
    events: Vec<GizziCodeUsageEvent>,
}

async fn ingest_gizzi_code_events(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<IngestGizziCodeEvents>,
) -> impl IntoResponse {
    let tenant_id = user.organization_id.as_deref().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required to ingest CLI analytics.",
        )
    })?;

    let db = state.db.clone();
    let tenant_id = tenant_id.to_string();
    let user_id = user.user_id.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(internal)?;
        let mut stmt = conn
            .prepare(
                "INSERT INTO gizzi_code_usage_events
                 (id, tenant_id, user_id, session_id, event_type, model, provider,
                  prompt_tokens, completion_tokens, cost_microdollars,
                  tool_calls_accepted, tool_calls_rejected, lines_accepted, metadata, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )
            .map_err(internal)?;
        for event in body.events {
            let created_at = event.created_at.unwrap_or_else(|| {
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            });
            let metadata_json = event
                .metadata
                .map(|m| m.to_string())
                .unwrap_or_else(|| "{}".to_string());
            stmt.execute(params![
                uuid::Uuid::new_v4().to_string(),
                &tenant_id,
                &user_id,
                event.session_id,
                event.event_type,
                event.model,
                event.provider,
                event.prompt_tokens,
                event.completion_tokens,
                event.cost_microdollars,
                event.tool_calls_accepted,
                event.tool_calls_rejected,
                event.lines_accepted,
                metadata_json,
                created_at,
            ])
            .map_err(internal)?;
        }
        Ok::<(), ApiError>(())
    })
    .await
    .map_err(internal)??;

    Ok::<Json<Value>, ApiError>(Json(json!({ "status": "ok" })))
}

async fn gizzi_code_usage(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<AnalyticsQuery>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_query(&query, &org)?;

    let bucket = bucket_sql(&query.granularity);
    let sql = format!(
        "SELECT {bucket} AS bucket,
                COUNT(*) AS events,
                COUNT(DISTINCT user_id) AS unique_users,
                COUNT(DISTINCT session_id) AS unique_sessions,
                SUM(prompt_tokens) AS prompt_tokens,
                SUM(completion_tokens) AS completion_tokens,
                SUM(cost_microdollars) AS cost_microdollars,
                SUM(tool_calls_accepted) AS tool_calls_accepted,
                SUM(tool_calls_rejected) AS tool_calls_rejected,
                SUM(lines_accepted) AS lines_accepted
         FROM gizzi_code_usage_events
         WHERE tenant_id = ?1 AND created_at >= ?2 AND created_at < ?3
         GROUP BY bucket
         ORDER BY bucket"
    );
    let mut stmt = conn.prepare(&sql).map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org, query.start, query.end], |row| {
            Ok(json!({
                "bucket": row.get::<_, String>(0)?,
                "events": row.get::<_, i64>(1)?,
                "unique_users": row.get::<_, i64>(2)?,
                "unique_sessions": row.get::<_, i64>(3)?,
                "prompt_tokens": row.get::<_, i64>(4)?,
                "completion_tokens": row.get::<_, i64>(5)?,
                "cost_microdollars": row.get::<_, i64>(6)?,
                "tool_calls_accepted": row.get::<_, i64>(7)?,
                "tool_calls_rejected": row.get::<_, i64>(8)?,
                "lines_accepted": row.get::<_, i64>(9)?,
            }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::path::Path;
    use tower::ServiceExt;

    fn test_user(user_id: &str, org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: org_id.map(|s| s.to_string()),
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: Some("org:admin".to_string()),
            organization_slug: None,
        }
    }

    async fn test_app_state(temp: &Path) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let conn = db.connect().expect("test db conn");
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
            params!["org-1"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            params!["admin-1", "admin-1@test.local"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, ?4)",
            params!["org-1:admin-1", "org-1", "admin-1", "owner"],
        )
        .unwrap();
        drop(conn);
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(tokio::sync::RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            office_cli_docs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_watches: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
                "__test__".to_string(),
            ),
            fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn seed_events(conn: &rusqlite::Connection) {
        for (user, cost, prompt, completion, status, created) in [
            ("user-a", 100_000, 10, 5, "ok", "2026-08-01T10:00:00Z"),
            ("user-a", 200_000, 20, 10, "ok", "2026-08-01T12:00:00Z"),
            ("user-b", 50_000, 5, 2, "error", "2026-08-02T08:00:00Z"),
            ("user-b", 150_000, 15, 8, "ok", "2026-08-02T14:00:00Z"),
        ] {
            conn.execute(
                "INSERT INTO llm_usage_events
                 (id, tenant_id, user_id, cost_microdollars, prompt_tokens, completion_tokens,
                  reasoning_tokens, cached_tokens, status, latency_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, 0, ?8)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    "org-1",
                    user,
                    cost,
                    prompt,
                    completion,
                    status,
                    created,
                ],
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn cost_over_time_groups_by_day() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        seed_events(&state.db.connect().unwrap());
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/cost-over-time?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["bucket"], "2026-08-01");
        assert_eq!(items[0]["cost_microdollars"], 300_000);
        assert_eq!(items[1]["cost_microdollars"], 200_000);
    }

    #[tokio::test]
    async fn token_usage_groups_by_day() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        seed_events(&state.db.connect().unwrap());
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/token-usage?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["prompt_tokens"], 30);
        assert_eq!(items[0]["completion_tokens"], 15);
    }

    #[tokio::test]
    async fn request_volume_breaks_down_status() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        seed_events(&state.db.connect().unwrap());
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/request-volume?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["requests"], 2);
        assert_eq!(items[1]["errors"], 1);
        assert_eq!(items[1]["ok"], 1);
    }

    #[tokio::test]
    async fn per_user_cost_sorted_descending() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        seed_events(&state.db.connect().unwrap());
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/per-user-cost?organization_id=org-1&start=2026-08-01&end=2026-08-03")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["user_id"], "user-a");
        assert_eq!(items[0]["cost_microdollars"], 300_000);
    }

    #[tokio::test]
    async fn active_users_counts_distinct_users_per_day() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        seed_events(&state.db.connect().unwrap());
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/active-users?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let daily = body["daily"].as_array().unwrap();
        assert_eq!(daily.len(), 2);
        assert_eq!(daily[0]["bucket"], "2026-08-01");
        assert_eq!(daily[0]["unique_users"], 1);
        assert_eq!(daily[1]["bucket"], "2026-08-02");
        assert_eq!(daily[1]["unique_users"], 1);
        assert_eq!(body["total_unique_users"], 2);
    }

    #[tokio::test]
    async fn analytics_rejects_non_admin() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            params!["member-1", "member-1@test.local"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, ?4)",
            params!["org-1:member-1", "org-1", "member-1", "member"],
        )
        .unwrap();
        drop(conn);
        let app = analytics_router().with_state(state);

        let mut user = test_user("member-1", Some("org-1"));
        user.organization_role = Some("org:member".to_string());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/request-volume?organization_id=org-1&start=2026-08-01&end=2026-08-03")
                    .extension(user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    fn seed_resource_analytics(conn: &rusqlite::Connection) {
        for uid in ["user-a", "user-b"] {
            conn.execute(
                "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
                params![uid, format!("{}@test.local", uid)],
            )
            .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, ?4)",
                params![format!("org-1:{}", uid), "org-1", uid, "member"],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT OR IGNORE INTO admin_workspaces (id, organization_id, name, created_by) VALUES (?1, ?2, ?3, ?4)",
            params!["ws-1", "org-1", "Platform", "admin-1"],
        )
        .unwrap();

        // Artifacts
        conn.execute(
            "INSERT INTO artifacts (id, user_id, workspace_id, title, type, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params!["art-1", "user-a", "ws-1", "Design doc", "document", "draft", "2026-08-01T10:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO artifact_sections (id, artifact_id, heading, kind, body, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params!["sec-1", "art-1", "Intro", "text", "hello", 0, "2026-08-01T10:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO artifact_revisions (id, artifact_id, reason, snapshot, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["rev-1", "art-1", "initial", "{}", "2026-08-02T10:00:00Z"],
        )
        .unwrap();

        // Chat projects
        conn.execute(
            "INSERT INTO cowork_projects (id, user_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params!["proj-1", "user-a", "Onboarding", "2026-08-01T10:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cowork_sessions (id, user_id, project_id, title, status, mode, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params!["sess-1", "user-a", "proj-1", "Session 1", "idle", "agent", "2026-08-02T10:00:00Z"],
        )
        .unwrap();

        // Connectors
        conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params!["conn-1", "github", "user-a", "oauth2", "connected", "2026-08-01T10:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params!["conn-2", "slack", "user-a", "oauth2", "error", "2026-08-02T10:00:00Z"],
        )
        .unwrap();

        // Plugins
        conn.execute(
            "INSERT INTO plugin_usage_events (id, tenant_id, user_id, plugin_id, action, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["pue-1", "org-1", "user-a", "vscode", "invoke", "2026-08-01T10:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plugin_usage_events (id, tenant_id, user_id, plugin_id, action, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["pue-2", "org-1", "user-a", "vscode", "invoke", "2026-08-02T10:00:00Z"],
        )
        .unwrap();

        // Skills
        conn.execute(
            "INSERT INTO team_skills (id, workspace_id, name, version, installed_by, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["skill-1", "ws-1", "Code reviewer", "1.0.0", "user-a", "2026-08-01T10:00:00Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_usage_events (id, tenant_id, user_id, skill_id, action, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["sue-1", "org-1", "user-a", "skill-1", "invoke", "2026-08-02T10:00:00Z"],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn artifact_activity_aggregates_counts() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_resource_analytics(&conn);
        drop(conn);
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/artifact-activity?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["bucket"], "2026-08-01");
        assert_eq!(items[0]["artifacts_created"], 1);
        assert_eq!(items[0]["sections_created"], 1);
        assert_eq!(items[1]["revisions_created"], 1);
    }

    #[tokio::test]
    async fn chat_project_usage_aggregates_counts() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_resource_analytics(&conn);
        drop(conn);
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/chat-project-usage?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["projects_created"], 1);
        assert_eq!(items[1]["sessions_created"], 1);
        assert_eq!(items[1]["projects_active"], 1);
    }

    #[tokio::test]
    async fn connector_usage_groups_by_connector() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_resource_analytics(&conn);
        drop(conn);
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/connector-usage?organization_id=org-1&start=2026-08-01&end=2026-08-03")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let github = items.iter().find(|i| i["connector_id"] == "github").unwrap();
        assert_eq!(github["connection_count"], 1);
        assert_eq!(github["active_count"], 1);
        let slack = items.iter().find(|i| i["connector_id"] == "slack").unwrap();
        assert_eq!(slack["error_count"], 1);
    }

    #[tokio::test]
    async fn plugin_usage_aggregates_invocations() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_resource_analytics(&conn);
        drop(conn);
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/plugin-usage?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let total: i64 = items.iter().map(|i| i["invocations"].as_i64().unwrap()).sum();
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn skill_usage_aggregates_installs_and_invocations() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_resource_analytics(&conn);
        drop(conn);
        let app = analytics_router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/skill-usage?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let aug1 = items.iter().find(|i| i["bucket"] == "2026-08-01").unwrap();
        assert_eq!(aug1["skills_installed"], 1);
        let aug2 = items.iter().find(|i| i["bucket"] == "2026-08-02").unwrap();
        assert_eq!(aug2["invocations"], 1);
        assert_eq!(aug2["skill_id"], "skill-1");
    }

    #[tokio::test]
    async fn gizzi_code_usage_ingest_and_aggregate() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = analytics_router().with_state(state);

        let ingest = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/analytics/gizzi-code/events")
                    .extension(test_user("admin-1", Some("org-1")))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "events": [
                                {
                                    "session_id": "sess-a",
                                    "event_type": "turn",
                                    "model": "claude-sonnet-4-6",
                                    "provider": "anthropic",
                                    "prompt_tokens": 100,
                                    "completion_tokens": 50,
                                    "cost_microdollars": 5000,
                                    "tool_calls_accepted": 1,
                                    "tool_calls_rejected": 0,
                                    "lines_accepted": 42,
                                    "created_at": "2026-08-01T10:00:00Z",
                                },
                                {
                                    "session_id": "sess-a",
                                    "event_type": "turn",
                                    "model": "claude-sonnet-4-6",
                                    "provider": "anthropic",
                                    "prompt_tokens": 200,
                                    "completion_tokens": 100,
                                    "cost_microdollars": 10000,
                                    "tool_calls_accepted": 0,
                                    "tool_calls_rejected": 1,
                                    "created_at": "2026-08-02T10:00:00Z",
                                },
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ingest.status(), StatusCode::OK);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/analytics/gizzi-code/usage?organization_id=org-1&start=2026-08-01&end=2026-08-03&granularity=day")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let aug1 = items.iter().find(|i| i["bucket"] == "2026-08-01").unwrap();
        assert_eq!(aug1["events"], 1);
        assert_eq!(aug1["prompt_tokens"], 100);
        assert_eq!(aug1["tool_calls_accepted"], 1);
        assert_eq!(aug1["lines_accepted"], 42);
        let aug2 = items.iter().find(|i| i["bucket"] == "2026-08-02").unwrap();
        assert_eq!(aug2["events"], 1);
        assert_eq!(aug2["completion_tokens"], 100);
        assert_eq!(aug2["tool_calls_rejected"], 1);
        // Pre-V151 payloads without lines_accepted ingest and aggregate as 0.
        assert_eq!(aug2["lines_accepted"], 0);
    }
}
