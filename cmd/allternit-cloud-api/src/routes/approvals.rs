//! Approval System API Routes
//!
//! Human-in-the-loop endpoints for approving autonomous run actions.
//!
//! `GET /api/v1/approvals?approver=me` (Island bar + Team map, lane 14) is a
//! separate read: the pending asks the caller must answer, from two stores:
//! `platform_approvals` rows routed to them (`approver_user_id`, migration
//! 165) and `meeting_tool_approvals` they can resolve (`user_id` = the session
//! owner, migration 130). Without `approver` the list is the run approvals
//! (`approval_requests`) exactly as before.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

use crate::{auth::middleware::AuthContext, db::cowork_models::*, error::ApiError, ApiState};

// ============================================================================
// Request/Response Types
// ============================================================================

/// Query parameters for listing approvals
#[derive(Debug, Deserialize, Default)]
pub struct ListApprovalsQuery {
    pub run_id: Option<String>,
    pub status: Option<ApprovalStatus>,
    pub limit: Option<i64>,
    /// `me`: the pending asks routed to the caller (see the module docs).
    pub approver: Option<String>,
}

// ============================================================================
// Route Configuration
// ============================================================================

pub fn router() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v1/approvals",
            get(list_approvals).post(create_approval),
        )
        .route("/api/v1/approvals/:id", get(get_approval))
        .route("/api/v1/approvals/:id/approve", post(approve_request))
        .route("/api/v1/approvals/:id/deny", post(deny_request))
        .route("/api/v1/approvals/:id/cancel", post(cancel_request))
}

// ============================================================================
// Route Handlers
// ============================================================================

/// List approval requests with optional filtering
pub async fn list_approvals(
    State(state): State<Arc<ApiState>>,
    auth_context: Option<Extension<AuthContext>>,
    Query(query): Query<ListApprovalsQuery>,
) -> Result<axum::response::Response, ApiError> {
    if let Some(approver) = query.approver.as_deref() {
        if approver != "me" {
            return Err(ApiError::BadRequest("approver must be 'me'.".to_string()));
        }
        let Some(Extension(auth)) = auth_context else {
            return Err(ApiError::Unauthorized("Sign in to list your approvals.".to_string()));
        };
        let limit = query.limit.unwrap_or(50).clamp(1, 100);
        let asks = routed_asks(&state.db, &auth.user.user_id, limit).await?;
        return Ok((StatusCode::OK, Json(json!({ "object": "list", "data": asks }))).into_response());
    }
    let limit = query.limit.unwrap_or(50).min(100);

    let approvals = if let Some(run_id) = query.run_id {
        if let Some(status) = query.status {
            sqlx::query_as::<_, ApprovalRequestSummary>(
                r#"
                SELECT id, run_id, status, priority, title, action_type, created_at, responded_at
                FROM approval_requests
                WHERE run_id = $1 AND status = $2
                ORDER BY 
                    CASE priority
                        WHEN 'critical' THEN 1
                        WHEN 'high' THEN 2
                        WHEN 'normal' THEN 3
                        WHEN 'low' THEN 4
                    END,
                    created_at DESC
                LIMIT $3
                "#,
            )
            .bind(&run_id)
            .bind(status)
            .bind(limit)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::DatabaseError)?
        } else {
            sqlx::query_as::<_, ApprovalRequestSummary>(
                r#"
                SELECT id, run_id, status, priority, title, action_type, created_at, responded_at
                FROM approval_requests
                WHERE run_id = $1
                ORDER BY 
                    CASE priority
                        WHEN 'critical' THEN 1
                        WHEN 'high' THEN 2
                        WHEN 'normal' THEN 3
                        WHEN 'low' THEN 4
                    END,
                    created_at DESC
                LIMIT $2
                "#,
            )
            .bind(&run_id)
            .bind(limit)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::DatabaseError)?
        }
    } else if let Some(status) = query.status {
        sqlx::query_as::<_, ApprovalRequestSummary>(
            r#"
            SELECT id, run_id, status, priority, title, action_type, created_at, responded_at
            FROM approval_requests
            WHERE status = $1
            ORDER BY 
                CASE priority
                    WHEN 'critical' THEN 1
                    WHEN 'high' THEN 2
                    WHEN 'normal' THEN 3
                    WHEN 'low' THEN 4
                END,
                created_at DESC
            LIMIT $2
            "#,
        )
        .bind(status)
        .bind(limit)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::DatabaseError)?
    } else {
        sqlx::query_as::<_, ApprovalRequestSummary>(
            r#"
            SELECT id, run_id, status, priority, title, action_type, created_at, responded_at
            FROM approval_requests
            ORDER BY 
                CASE priority
                    WHEN 'critical' THEN 1
                    WHEN 'high' THEN 2
                    WHEN 'normal' THEN 3
                    WHEN 'low' THEN 4
                END,
                created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::DatabaseError)?
    };

    Ok((StatusCode::OK, Json(approvals)).into_response())
}

// ============================================================================
// Routed asks (`?approver=me`)
// ============================================================================

#[derive(sqlx::FromRow)]
struct RoutedPlatformRow {
    id: String,
    project_id: String,
    source: String,
    source_ref: String,
    account_id: Option<String>,
    kind: String,
    proposed_action: Value,
    approver_user_id: String,
    rule_id: Option<String>,
    actor_ref: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct RoutedMeetingRow {
    id: Uuid,
    session_id: Uuid,
    user_id: String,
    tool: String,
    args_summary: String,
    asked_by: Option<String>,
    requested_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The pending, unexpired asks `user_id` must answer, newest first, at most
/// `limit`. Each item:
/// `{ id: "<store>:<id>", store: "platform"|"meeting", store_id, title,
///    summary, actor, approver_user_id, rule_id, tool, state: "pending",
///    created_at, expires_at, resolve, source }`.
/// `resolve` is the store's own resolve route for a signed-in person
/// (`{method, path, decisions}`; send `{"decision": <one of decisions>}`); platform
/// rows have none here (`null`): a twin row is decided on its runtime with
/// `source.source_ref`, a computer row through the Platform API.
async fn routed_asks(db: &sqlx::PgPool, user_id: &str, limit: i64) -> Result<Vec<Value>, ApiError> {
    let platform: Vec<RoutedPlatformRow> = sqlx::query_as(
        "SELECT id, project_id, source, source_ref, account_id, kind, proposed_action, approver_user_id, \
                rule_id, actor_ref, expires_at, created_at \
           FROM platform_approvals \
          WHERE approver_user_id = $1 AND status = 'pending' AND (expires_at IS NULL OR expires_at > NOW()) \
          ORDER BY created_at DESC, id DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await
    .map_err(ApiError::DatabaseError)?;
    let meeting: Vec<RoutedMeetingRow> = sqlx::query_as(
        "SELECT id, session_id, user_id, tool, args_summary, asked_by, requested_at, expires_at \
           FROM meeting_tool_approvals \
          WHERE user_id = $1 AND status = 'pending' AND expires_at > NOW() \
          ORDER BY requested_at DESC, id DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await
    .map_err(ApiError::DatabaseError)?;

    let mut asks: Vec<(DateTime<Utc>, Value)> = Vec::with_capacity(platform.len() + meeting.len());
    for r in platform {
        let summary = r.proposed_action.get("summary").and_then(Value::as_str).map(str::to_string);
        let tool = r.proposed_action.get("tool").and_then(Value::as_str).map(str::to_string);
        asks.push((
            r.created_at,
            json!({
                "id": format!("platform:{}", r.id),
                "store": "platform",
                "store_id": r.id,
                "title": r.kind,
                "summary": summary,
                "actor": r.actor_ref,
                "approver_user_id": r.approver_user_id,
                "rule_id": r.rule_id,
                "tool": tool,
                "state": "pending",
                "created_at": rfc3339(r.created_at),
                "expires_at": r.expires_at.map(rfc3339),
                "resolve": Value::Null,
                "source": {
                    "kind": r.source, "source_ref": r.source_ref, "project_id": r.project_id,
                    "account_id": r.account_id, "proposed_action": r.proposed_action,
                },
            }),
        ));
    }
    for r in meeting {
        asks.push((
            r.requested_at,
            json!({
                "id": format!("meeting:{}", r.id),
                "store": "meeting",
                "store_id": r.id.to_string(),
                "title": r.tool,
                "summary": r.args_summary,
                // Speaker label from the call (not authenticated).
                "actor": r.asked_by,
                "approver_user_id": r.user_id,
                "rule_id": Value::Null,
                "tool": r.tool,
                "state": "pending",
                "created_at": rfc3339(r.requested_at),
                "expires_at": rfc3339(r.expires_at),
                "resolve": {
                    "method": "POST",
                    "path": format!("/api/v1/meeting-sessions/{}/approvals/{}", r.session_id, r.id),
                    "decisions": ["approve", "deny"],
                },
                "source": { "kind": "meeting", "session_id": r.session_id.to_string() },
            }),
        ));
    }
    asks.sort_by(|a, b| b.0.cmp(&a.0));
    asks.truncate(limit as usize);
    Ok(asks.into_iter().map(|(_, v)| v).collect())
}

/// Get a single approval request by ID
pub async fn get_approval(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?
    .ok_or_else(|| ApiError::NotFound(format!("Approval request '{}' not found", id)))?;

    Ok((StatusCode::OK, Json(approval)))
}

/// Create a new approval request
pub async fn create_approval(
    State(state): State<Arc<ApiState>>,
    Extension(auth_context): Extension<AuthContext>,
    Json(request): Json<CreateApprovalRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let priority = request.priority.unwrap_or_default();

    // Verify the run exists and belongs to the caller's tenant (same
    // convention as the run routes: tenant_id == authenticated user id,
    // NULL-tenant internal runs allowed).
    let run: Option<Run> = sqlx::query_as("SELECT * FROM runs WHERE id = $1")
        .bind(&request.run_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::DatabaseError)?;

    let run = match run {
        Some(run) => run,
        None => {
            return Err(ApiError::NotFound(format!(
                "Run '{}' not found",
                request.run_id
            )));
        }
    };
    crate::routes::runs::ensure_run_accessible(&run, &auth_context)?;

    let requested_by = auth_context.user.user_id;

    // Insert the approval request
    sqlx::query(
        r#"
        INSERT INTO approval_requests (
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, timeout_seconds, created_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
    )
    .bind(&id)
    .bind(&request.run_id)
    .bind(&request.step_cursor)
    .bind(ApprovalStatus::Pending)
    .bind(priority)
    .bind(&request.title)
    .bind(&request.description)
    .bind(&request.action_type)
    .bind(
        request
            .action_params
            .as_ref()
            .map(|p| sqlx::types::Json(p.clone())),
    )
    .bind(&request.reasoning)
    .bind(Some(requested_by))
    .bind(request.timeout_seconds)
    .bind(now)
    .execute(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Return the created approval
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Emit approval needed event using shared event store
    let _ = state
        .event_store
        .append(
            &request.run_id,
            EventType::ApprovalNeeded,
            serde_json::json!({
                "approval_id": id,
                "title": request.title,
                "priority": priority,
                "action_type": request.action_type,
            }),
        )
        .await;

    // Notify connected clients via session manager
    let _ = state
        .session_manager
        .notify_approval_needed(
            &request.run_id,
            &id,
            &request.action_type.as_deref().unwrap_or("action"),
            &request
                .action_params
                .as_ref()
                .unwrap_or(&serde_json::json!({})),
            request.reasoning.as_deref(),
            priority,
            request.timeout_seconds.map(|t| t as u32),
        )
        .await;

    Ok((StatusCode::CREATED, Json(approval)))
}

/// Approve an approval request
pub async fn approve_request(
    State(state): State<Arc<ApiState>>,
    Extension(auth_context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(response): Json<ApprovalResponse>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = &auth_context.user.user_id;
    let now = Utc::now();

    // Get the approval request
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?
    .ok_or_else(|| ApiError::NotFound(format!("Approval request '{}' not found", id)))?;

    // Verify it's still pending
    if approval.status != ApprovalStatus::Pending {
        return Err(ApiError::BadRequest(format!(
            "Approval request is already {:?}",
            approval.status
        )));
    }

    // Update the approval
    sqlx::query(
        r#"
        UPDATE approval_requests
        SET status = $1, responded_by = $2, response_message = $3, responded_at = $4
        WHERE id = $5
        "#,
    )
    .bind(ApprovalStatus::Approved)
    .bind(Some(user_id.as_str()))
    .bind(&response.message)
    .bind(now)
    .bind(&id)
    .execute(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Return the updated approval
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Emit approval given event
    let _ = state
        .event_store
        .append(
            &approval.run_id,
            EventType::ApprovalGiven,
            serde_json::json!({
                "approval_id": id,
                "message": response.message,
                "modified_params": response.modified_params,
            }),
        )
        .await;

    // Notify connected clients
    let _ = state
        .session_manager
        .notify_approval_resolved(
            &approval.run_id,
            &id,
            true, // approved
            user_id,
            response.message.as_deref(),
        )
        .await;

    Ok((StatusCode::OK, Json(approval)))
}

/// Deny an approval request
pub async fn deny_request(
    State(state): State<Arc<ApiState>>,
    Extension(auth_context): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(response): Json<ApprovalResponse>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = &auth_context.user.user_id;
    let now = Utc::now();

    // Get the approval request
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?
    .ok_or_else(|| ApiError::NotFound(format!("Approval request '{}' not found", id)))?;

    // Verify it's still pending
    if approval.status != ApprovalStatus::Pending {
        return Err(ApiError::BadRequest(format!(
            "Approval request is already {:?}",
            approval.status
        )));
    }

    // Update the approval
    sqlx::query(
        r#"
        UPDATE approval_requests
        SET status = $1, responded_by = $2, response_message = $3, responded_at = $4
        WHERE id = $5
        "#,
    )
    .bind(ApprovalStatus::Denied)
    .bind(user_id.as_str())
    .bind(&response.message)
    .bind(now)
    .bind(&id)
    .execute(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Return the updated approval
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Emit approval denied event
    let _ = state
        .event_store
        .append(
            &approval.run_id,
            EventType::ApprovalDenied,
            serde_json::json!({
                "approval_id": id,
                "message": response.message,
            }),
        )
        .await;

    // Notify connected clients
    let _ = state
        .session_manager
        .notify_approval_resolved(
            &approval.run_id,
            &id,
            false, // denied
            user_id,
            response.message.as_deref(),
        )
        .await;

    Ok((StatusCode::OK, Json(approval)))
}

/// Cancel an approval request (by the run)
pub async fn cancel_request(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let now = Utc::now();

    // Get the approval request
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?
    .ok_or_else(|| ApiError::NotFound(format!("Approval request '{}' not found", id)))?;

    // Verify it's still pending
    if approval.status != ApprovalStatus::Pending {
        return Err(ApiError::BadRequest(format!(
            "Approval request is already {:?}",
            approval.status
        )));
    }

    // Update the approval
    sqlx::query(
        r#"
        UPDATE approval_requests
        SET status = $1, responded_at = $2
        WHERE id = $3
        "#,
    )
    .bind(ApprovalStatus::Cancelled)
    .bind(now)
    .bind(&id)
    .execute(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    // Return the updated approval
    let approval = sqlx::query_as::<_, ApprovalRequest>(
        r#"
        SELECT 
            id, run_id, step_cursor, status, priority,
            title, description, action_type, action_params,
            reasoning, requested_by, responded_by, response_message,
            timeout_seconds, created_at, responded_at
        FROM approval_requests
        WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;

    Ok((StatusCode::OK, Json(approval)))
}

#[cfg(test)]
#[path = "approvals_routed_tests.rs"]
mod approvals_routed_tests;
