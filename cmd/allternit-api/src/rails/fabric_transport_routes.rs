//! A:// fabric transport routes (contract v0.1 §8) — worker-facing fabric-transport API.
//!
//! v0.1 workers discover work by long-polling the claim endpoint (lock 4; no
//! push machinery). Every handler authenticates the worker as a Principal via
//! bearer token (§8.4) and delegates exclusivity to the store-level CAS in
//! `allternit_cowork_runtime::sqlite_store` (lock 2).

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    routing::{get, post, put},
    Json, Router,
};
use rusqlite::OptionalExtension;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

use allternit_cowork_runtime::{
    sqlite_store, CompleteOutcome, TransportError, JobState, LeaseGrant, RunId, RunManager,
    RunState,
};

use crate::AppState;

use super::routes_cowork::ErrorResponse;

/// Create the A:// fabric transport router (mounted under the v1 API).
pub fn fabric_transport_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/fabric/transport/principals", post(create_principal))
        .route("/fabric/transport/principals", get(list_principals))
        // Managed-runtime local provision (consumer desktop P1): gated on the
        // spawn-time desktop access secret; absent in cloud deployments.
        .route(
            "/fabric/transport/local/ensure-worker-principal",
            post(ensure_worker_principal),
        )
        .route("/fabric/transport/claim", post(claim))
        .route("/fabric/transport/jobs/:job_id", get(get_job))
        .route("/fabric/transport/jobs/:job_id/heartbeat", post(heartbeat))
        .route("/fabric/transport/jobs/:job_id/renew", post(renew))
        .route("/fabric/transport/jobs/:job_id/complete", post(complete))
        // Approval bindings (§8.14): scoped to (executor, capability, target,
        // run, job, lease_generation); invalidated when the lease expires.
        .route(
            "/fabric/transport/jobs/:job_id/approvals/request",
            post(request_approval),
        )
        .route(
            "/fabric/transport/jobs/:job_id/approvals/check",
            post(check_approval),
        )
        .route("/fabric/transport/approvals/:approval_id", get(get_approval))
        .route(
            "/fabric/transport/approvals/:approval_id/grant",
            post(decide_approval_grant),
        )
        .route(
            "/fabric/transport/approvals/:approval_id/deny",
            post(decide_approval_deny),
        )
        .route(
            "/fabric/transport/principals/:principal_id/provision-token",
            post(provision_principal_token),
        )
        .route(
            "/fabric/transport/principals/:principal_id/capabilities",
            put(update_principal_capabilities),
        )
        .route("/fabric/transport/intents", post(submit_intent))
        .route("/fabric/transport/intents/:intent_id", get(get_intent))
        .route("/fabric/transport/approvals", get(list_approvals))
        .route(
            "/fabric/transport/jobs/:job_id/connector-sessions",
            post(request_connector_session),
        )
        .route(
            "/fabric/transport/connector-sessions/:session_id/invoke",
            post(invoke_connector_session),
        )
        .route(
            "/fabric/transport/delegation-rules",
            get(list_delegation_rules).put(upsert_delegation_rule),
        )
        .route(
            "/fabric/transport/delegation-rules/:workspace/:action_type",
            axum::routing::delete(delete_delegation_rule),
        )
        .route(
            "/fabric/transport/connector-sessions",
            get(list_connector_sessions),
        )
        .route(
            "/fabric/transport/jobs/:job_id/continue-in-cloud",
            post(continue_job_in_cloud),
        )
        .route(
            "/fabric/transport/runs/:run_id/continue-in-cloud",
            post(continue_run_in_cloud),
        )
        .route(
            "/fabric/transport/continuation/handoff-all",
            post(handoff_all_in_flight),
        )
        .route(
            "/fabric/transport/continuation/ingest",
            post(ingest_continuation),
        )
}

fn db_error(e: rusqlite::Error) -> ErrorResponse {
    tracing::error!("A:// fabric-transport DB error: {e}");
    ErrorResponse {
        error: e.to_string(),
        code: 500,
    }
}

fn transport_err(e: TransportError) -> ErrorResponse {
    ErrorResponse {
        error: format!("{}: {}", e.wire(), e.message),
        code: e.http_status(),
    }
}

/// Authenticate `Authorization: Bearer <token>` to a Principal (§8.4).
fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<allternit_cowork_runtime::PrincipalRecord, ErrorResponse> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| ErrorResponse {
            error: "A_AUTHENTICATION_FAILED: missing Authorization: Bearer <token>".to_string(),
            code: 401,
        })?;
    let conn = state.db.connect().map_err(db_error)?;
    sqlite_store::authenticate_principal(&conn, token).map_err(transport_err)
}

fn run_manager(state: &AppState) -> Result<Arc<RunManager>, ErrorResponse> {
    state
        .cowork_run_manager
        .clone()
        .ok_or_else(|| ErrorResponse {
            error: "Cowork runtime unavailable".to_string(),
            code: 503,
        })
}

fn default_lease_ttl() -> Duration {
    Duration::from_secs(
        std::env::var("ALLTERNIT_FABRIC_TRANSPORT_LEASE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60),
    )
}

/// Best-effort sync of the in-memory runtime mirror after a store-level
/// transition. The SQLite store is canonical (lock 1); the mirror keeps
/// attachments/checkpoints coherent.
async fn sync_job_state(state: &AppState, job_id: &str, to: JobState) {
    if let (Ok(manager), Ok(id)) = (
        run_manager(state),
        uuid::Uuid::parse_str(job_id).map(allternit_cowork_runtime::JobId),
    ) {
        let _ = manager.transition_job_state(id, to).await;
    }
}

async fn sync_run_state(state: &AppState, run_id: &str, to: RunState) {
    if let (Ok(manager), Ok(id)) = (run_manager(state), uuid::Uuid::parse_str(run_id).map(RunId)) {
        let _ = manager.transition_run_state(id, to).await;
    }
}

// ─── Principal registration ─────────────────────────────────────────────────

/// Managed-runtime local provision (consumer desktop P1): the signed desktop
/// process calls this once per profile to obtain the fabric-transport worker
/// credential it stores in the macOS Keychain and hands to the bundled
/// worker. Local-only: requires the spawn-time desktop access secret; the
/// token is minted fresh (rotating any previous one) and returned exactly
/// once — only its hash is stored.
#[derive(Debug, serde::Deserialize)]
pub struct EnsureWorkerPrincipalRequest {
    #[serde(default = "default_worker_workspace")]
    pub workspace: String,
    /// `local` (laptop gizzi) or `cloud` (always-on gizzi-cloud).
    #[serde(default)]
    pub kind: Option<String>,
}

fn default_worker_workspace() -> String {
    "default".to_string()
}

#[derive(Debug, serde::Serialize)]
pub struct EnsureWorkerPrincipalResponse {
    pub principal_id: String,
    pub workspace: String,
    pub token: String,
}

async fn ensure_worker_principal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EnsureWorkerPrincipalRequest>,
) -> Result<Json<EnsureWorkerPrincipalResponse>, ErrorResponse> {
    if !crate::auth::verify_desktop_access_token(&headers, &state.config) {
        return Err(ErrorResponse {
            error: "local desktop auth required to provision the worker principal".to_string(),
            code: 403,
        });
    }
    let mut conn = state.db.connect().map_err(db_error)?;
    let kind = req.kind.as_deref().unwrap_or("local");
    let principal_id = if kind == "cloud" {
        sqlite_store::ensure_gizzi_cloud_principal(&mut conn, &req.workspace).map_err(transport_err)?
    } else {
        sqlite_store::ensure_gizzi_principal(&mut conn, &req.workspace).map_err(transport_err)?
    };
    let token =
        sqlite_store::provision_principal_token(&mut conn, &principal_id).map_err(transport_err)?;
    let workspace = req
        .workspace
        .strip_prefix("a://workspace/")
        .unwrap_or(&req.workspace)
        .to_string();
    info!(principal = %principal_id, "Managed runtime provisioned fabric worker credential (returned once)");
    Ok(Json(EnsureWorkerPrincipalResponse {
        principal_id,
        workspace,
        token,
    }))
}

#[derive(Debug, serde::Deserialize)]
pub struct CreatePrincipalRequest {
    pub id: String,
    pub workspace: String,
    pub capabilities: Vec<String>,
    /// Role vocabulary (§3): orchestrator, worker, reviewer, observer, human, system.
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CreatePrincipalResponse {
    pub id: String,
    pub workspace: String,
    pub capabilities: Vec<String>,
    /// Bearer token, returned exactly once; only its SHA-256 hash is stored.
    pub token: String,
}

/// Register a worker principal. Requires the caller's normal user auth; the
/// bearer token is shown once and stored hashed.
async fn create_principal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreatePrincipalRequest>,
) -> Result<Json<CreatePrincipalResponse>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required to register principals".to_string(),
            code: 401,
        });
    }
    let token = format!("atok_{}", uuid::Uuid::new_v4());
    let mut conn = state.db.connect().map_err(db_error)?;
    sqlite_store::register_principal(
        &mut conn,
        &req.id,
        &req.workspace,
        &req.capabilities,
        &req.roles,
        &token,
    )
    .map_err(transport_err)?;
    info!(principal = %req.id, workspace = %req.workspace, "Registered A:// principal");
    Ok(Json(CreatePrincipalResponse {
        id: req.id,
        workspace: req.workspace,
        capabilities: req.capabilities,
        token,
    }))
}

// ─── Claim (§8.10, lock 4 long-poll) ────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ClaimRequest {
    pub job_id: Option<String>,
    /// Long-poll window in seconds (default 0 = single attempt).
    pub wait_secs: Option<u64>,
    pub lease_ttl_secs: Option<u64>,
}

async fn claim(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ClaimRequest>,
) -> Result<Json<LeaseGrant>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let ttl = req
        .lease_ttl_secs
        .map(Duration::from_secs)
        .unwrap_or_else(default_lease_ttl);
    let wait = Duration::from_secs(req.wait_secs.unwrap_or(0).min(60));
    let deadline = std::time::Instant::now() + wait;

    loop {
        let mut conn = state.db.connect().map_err(db_error)?;
        match sqlite_store::claim_job(&mut conn, &principal, req.job_id.as_deref(), ttl) {
            Ok(grant) => {
                drop(conn);
                sync_job_state(&state, &grant.job_id, JobState::Leased).await;
                sync_run_state(&state, &grant.run_id, RunState::Running).await;
                return Ok(Json(grant));
            }
            Err(e)
                if e.code
                    == allternit_cowork_runtime::TransportErrorCode::NoEligibleWorker
                    || e.code
                        == allternit_cowork_runtime::TransportErrorCode::JobAlreadyLeased =>
            {
                if std::time::Instant::now() >= deadline {
                    return Err(transport_err(e));
                }
                drop(conn);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => return Err(transport_err(e)),
        }
    }
}

// ─── Heartbeat / renew (§8.11–8.12) ─────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct HeartbeatRequest {
    pub lease_id: String,
    pub lease_generation: i64,
    /// Advisory only; the server clock decides expiry (lock 3).
    pub worker_time: Option<String>,
}

async fn heartbeat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(req): Json<HeartbeatRequest>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    sqlite_store::record_heartbeat(
        &mut conn,
        &principal,
        &job_id,
        &req.lease_id,
        req.lease_generation,
        req.worker_time,
    )
    .map_err(transport_err)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, serde::Deserialize)]
pub struct RenewRequest {
    pub lease_id: String,
    pub lease_generation: i64,
    pub lease_ttl_secs: Option<u64>,
}

async fn renew(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(req): Json<RenewRequest>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let ttl = req
        .lease_ttl_secs
        .map(Duration::from_secs)
        .unwrap_or_else(default_lease_ttl);
    let mut conn = state.db.connect().map_err(db_error)?;
    let expires_at = sqlite_store::renew_lease(
        &mut conn,
        &principal,
        &job_id,
        &req.lease_id,
        req.lease_generation,
        ttl,
    )
    .map_err(transport_err)?;
    Ok(Json(json!({ "ok": true, "lease_expires_at": expires_at })))
}

// ─── Completion (§8.16) ─────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct CompleteRequest {
    pub lease_id: String,
    pub lease_generation: i64,
    pub success: bool,
    pub summary: Option<String>,
    pub outputs: Option<serde_json::Value>,
}

async fn complete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(req): Json<CompleteRequest>,
) -> Result<Json<CompleteOutcome>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let outcome = sqlite_store::complete_job(
        &mut conn,
        &principal,
        &job_id,
        &req.lease_id,
        req.lease_generation,
        req.success,
        req.summary,
        req.outputs,
    )
    .map_err(transport_err)?;
    drop(conn);

    match &outcome {
        CompleteOutcome::Committed { job_state, .. }
        | CompleteOutcome::AlreadyCommitted { job_state, .. } => {
            let job_state = job_state.clone();
            let view = {
                let conn = state.db.connect().map_err(db_error)?;
                sqlite_store::get_job_view(&conn, &job_id).map_err(transport_err)?
            };
            let run_id = view
                .as_ref()
                .and_then(|v| v["run_id"].as_str())
                .unwrap_or_default()
                .to_string();
            let run_terminal = {
                let conn = state.db.connect().map_err(db_error)?;
                conn.query_row(
                    "SELECT state FROM cowork_runs WHERE id = ?1",
                    rusqlite::params![run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(db_error)?
            };
            sync_job_state(
                &state,
                &job_id,
                if job_state == "completed" {
                    JobState::Completed
                } else {
                    JobState::Failed
                },
            )
            .await;
            if let Some(run_state) = run_terminal {
                if run_state == "completed" {
                    sync_run_state(&state, &run_id, RunState::Completed).await;
                } else if run_state == "failed" {
                    sync_run_state(&state, &run_id, RunState::Failed).await;
                }
            }
        }
    }
    Ok(Json(outcome))
}

// ─── Canonical job view (read-only) ─────────────────────────────────────────

async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let conn = state.db.connect().map_err(db_error)?;
    let view = sqlite_store::get_job_view(&conn, &job_id)
        .map_err(transport_err)?
        .ok_or_else(|| ErrorResponse {
            error: "A_JOB_NOT_FOUND: job not found".to_string(),
            code: 404,
        })?;
    Ok(Json(view))
}

// ─── Approval bindings (§8.14) ──────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ApprovalScopeRequest {
    pub lease_id: String,
    pub lease_generation: i64,
    pub capability: String,
    pub target: String,
    /// Server-clock lifetime of the approval request (default 300s).
    pub approval_ttl_secs: Option<u64>,
}

async fn request_approval(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(req): Json<ApprovalScopeRequest>,
) -> Result<Json<allternit_cowork_runtime::ApprovalBinding>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let binding = sqlite_store::request_approval(
        &mut conn,
        &principal,
        &job_id,
        &req.lease_id,
        req.lease_generation,
        &req.capability,
        &req.target,
        req.approval_ttl_secs.map(Duration::from_secs),
    )
    .map_err(transport_err)?;
    Ok(Json(binding))
}

async fn check_approval(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(req): Json<ApprovalScopeRequest>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let binding = sqlite_store::check_approval(
        &mut conn,
        &principal,
        &job_id,
        &req.lease_id,
        req.lease_generation,
        &req.capability,
        &req.target,
    )
    .map_err(transport_err)?;
    Ok(Json(json!({ "ok": true, "approval_id": binding.id, "status": binding.status })))
}

async fn get_approval(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(approval_id): Path<String>,
) -> Result<Json<allternit_cowork_runtime::ApprovalBinding>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let conn = state.db.connect().map_err(db_error)?;
    let binding = sqlite_store::get_approval(&conn, &approval_id)
        .map_err(transport_err)?
        .ok_or_else(|| ErrorResponse {
            error: "A_JOB_NOT_FOUND: approval binding not found".to_string(),
            code: 404,
        })?;
    if binding.executor != principal.id {
        return Err(ErrorResponse {
            error: "A_PERMISSION_DENIED: approval belongs to another executor".to_string(),
            code: 403,
        });
    }
    Ok(Json(binding))
}

/// Human decision endpoints — require the normal user auth (Extension-style
/// via headers), not a worker bearer token.
async fn decide_approval_grant(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(approval_id): Path<String>,
) -> Result<Json<allternit_cowork_runtime::ApprovalBinding>, ErrorResponse> {
    decide_approval(state, headers, approval_id, true).await
}

async fn decide_approval_deny(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(approval_id): Path<String>,
) -> Result<Json<allternit_cowork_runtime::ApprovalBinding>, ErrorResponse> {
    decide_approval(state, headers, approval_id, false).await
}

async fn decide_approval(
    state: Arc<AppState>,
    headers: HeaderMap,
    approval_id: String,
    grant: bool,
) -> Result<Json<allternit_cowork_runtime::ApprovalBinding>, ErrorResponse> {
    let user = crate::auth::get_user(&headers).ok_or_else(|| ErrorResponse {
        error: "authentication required to decide approvals".to_string(),
        code: 401,
    })?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let binding =
        sqlite_store::decide_approval(&mut conn, &approval_id, grant, &user.user_id)
            .map_err(transport_err)?;
    Ok(Json(binding))
}

// ─── Default principals, intents, approval inbox (§3 / §5 / §8.15) ──────────

/// Provision or rotate a principal's bearer token. Requires user auth; the
/// raw token is returned exactly once (hashed at rest).
async fn provision_principal_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(principal_id): Path<String>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let user = crate::auth::get_user(&headers).ok_or_else(|| ErrorResponse {
        error: "authentication required to provision principal tokens".to_string(),
        code: 401,
    })?;
    let _ = user;
    let mut conn = state.db.connect().map_err(db_error)?;
    let token = sqlite_store::provision_principal_token(&mut conn, &principal_id)
        .map_err(transport_err)?;
    info!(principal = %principal_id, "Provisioned fabric-transport token (returned once)");
    Ok(Json(json!({ "principal_id": principal_id, "token": token })))
}

/// Replace a principal's declared capabilities (operator action, P-T2):
/// workers declare `compute.vm` here when running VM mode so placement
/// (§8.8) routes vm-required jobs to them.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateCapabilitiesRequest {
    pub capabilities: Vec<String>,
}

async fn update_principal_capabilities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(principal_id): Path<String>,
    Json(req): Json<UpdateCapabilitiesRequest>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required to update principal capabilities".to_string(),
            code: 401,
        });
    }
    let conn = state.db.connect().map_err(db_error)?;
    sqlite_store::set_principal_capabilities(&conn, &principal_id, &req.capabilities)
        .map_err(transport_err)?;
    info!(principal = %principal_id, capabilities = ?req.capabilities, "Updated principal capabilities");
    Ok(Json(json!({ "principal_id": principal_id, "capabilities": req.capabilities })))
}

/// List principals (P-T6 control surface: principals/bots management view).
/// Token hashes are never returned.
async fn list_principals(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PrincipalListQuery>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required to list principals".to_string(),
            code: 401,
        });
    }
    let conn = state.db.connect().map_err(db_error)?;
    let workspace = q
        .workspace
        .as_deref()
        .map(|w| w.strip_prefix("a://workspace/").unwrap_or(w).to_string());
    let principals = sqlite_store::list_principals(&conn, workspace.as_deref())
        .map_err(transport_err)?;
    Ok(Json(json!({ "principals": principals })))
}

#[derive(Debug, serde::Deserialize)]
pub struct PrincipalListQuery {
    pub workspace: Option<String>,
}

/// List delegation rules for a workspace (P-T6).
async fn list_delegation_rules(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<RulesQuery>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required".to_string(),
            code: 401,
        });
    }
    let workspace = q.workspace.unwrap_or_else(|| "default".to_string());
    let workspace = workspace
        .strip_prefix("a://workspace/")
        .unwrap_or(&workspace)
        .to_string();
    let conn = state.db.connect().map_err(db_error)?;
    let rules = sqlite_store::list_delegation_rules(&conn, &workspace).map_err(transport_err)?;
    Ok(Json(json!({ "workspace": workspace, "rules": rules })))
}

#[derive(Debug, serde::Deserialize)]
pub struct RulesQuery {
    pub workspace: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpsertRuleRequest {
    pub workspace: String,
    pub action_type: String,
    pub target_principal: String,
    pub priority: Option<i64>,
}

/// Upsert a delegation rule (P-T6 editor). Takes effect on the next
/// orchestrator tick / persona resolution.
async fn upsert_delegation_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<UpsertRuleRequest>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required".to_string(),
            code: 401,
        });
    }
    if req.action_type.is_empty() || req.target_principal.is_empty() {
        return Err(ErrorResponse {
            error: "action_type and target_principal are required".to_string(),
            code: 400,
        });
    }
    let workspace = req
        .workspace
        .strip_prefix("a://workspace/")
        .unwrap_or(&req.workspace)
        .to_string();
    let conn = state.db.connect().map_err(db_error)?;
    sqlite_store::upsert_delegation_rule(
        &conn,
        &workspace,
        &req.action_type,
        &req.target_principal,
        req.priority.unwrap_or(100),
    )
    .map_err(transport_err)?;
    info!(workspace = %workspace, action_type = %req.action_type, target = %req.target_principal, "Upserted delegation rule");
    Ok(Json(json!({
        "workspace": workspace,
        "action_type": req.action_type,
        "target_principal": req.target_principal,
        "priority": req.priority.unwrap_or(100),
    })))
}

/// Delete a delegation rule (P-T6 editor).
async fn delete_delegation_rule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((workspace, action_type)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required".to_string(),
            code: 401,
        });
    }
    let workspace = workspace
        .strip_prefix("a://workspace/")
        .unwrap_or(&workspace)
        .to_string();
    let conn = state.db.connect().map_err(db_error)?;
    let deleted = sqlite_store::delete_delegation_rule(&conn, &workspace, &action_type)
        .map_err(transport_err)?;
    if !deleted {
        return Err(ErrorResponse {
            error: format!("no delegation rule for {workspace}/{action_type}"),
            code: 404,
        });
    }
    Ok(Json(json!({ "deleted": true })))
}

/// List brokered connector sessions (P-T6 control surface). User auth;
/// optionally filtered by run.
async fn list_connector_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ConnectorSessionQuery>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required".to_string(),
            code: 401,
        });
    }
    let conn = state.db.connect().map_err(db_error)?;
    let sessions = sqlite_store::list_connector_sessions(&conn, q.run_id.as_deref(), q.limit.unwrap_or(50).min(200))
        .map_err(transport_err)?;
    Ok(Json(json!({ "sessions": sessions })))
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectorSessionQuery {
    pub run_id: Option<String>,
    pub limit: Option<i64>,
}

/// Submit a canonical IntentEnvelope (§5). User auth (the initiator side).
/// Idempotent on intent_id; the created run is mirrored into the runtime
/// manager best-effort.
async fn submit_intent(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut envelope): Json<allternit_cowork_runtime::IntentEnvelope>,
) -> Result<Json<allternit_cowork_runtime::IntentSubmission>, ErrorResponse> {
    let user = crate::auth::get_user(&headers).ok_or_else(|| ErrorResponse {
        error: "authentication required to submit intents".to_string(),
        code: 401,
    })?;
    // A job for one of the user's bots runs as that bot (P4.1).
    if let Some(target) = envelope.target.clone() {
        if let Some((bot_id, payload)) = crate::agent_session_routes::bot_job_payload(
            &state.db,
            &user.user_id,
            &target,
            &envelope.action.description,
            envelope.action.payload.as_ref(),
        ) {
            tracing::info!(intent = %envelope.intent_id, bot = %bot_id, "intent targets a bot; running it as the bot");
            envelope.action.payload = Some(payload);
        }
    }
    let mut conn = state.db.connect().map_err(db_error)?;
    // The owner is stamped on the run at creation (V142/V169 scoping), so
    // intent-created runs are listable and job-postable like any other run.
    let submission = sqlite_store::submit_intent_for_user(
        &mut conn,
        &envelope,
        Some(user.user_id.as_str()),
    )
    .map_err(transport_err)?;
    if submission.created {
        // Mirror the intent-created run into the in-memory manager so the
        // legacy run/job routes (which consult the mirror) see it.
        if let Ok(manager) = run_manager(&state) {
            if let Ok(Some(run)) = sqlite_store::load_run_record(&conn, &submission.run_id) {
                let _ = manager.load_run(run).await;
            }
        }
    }
    Ok(Json(submission))
}

async fn get_intent(
    State(state): State<Arc<AppState>>,
    Path(intent_id): Path<String>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let conn = state.db.connect().map_err(db_error)?;
    sqlite_store::get_intent(&conn, &intent_id)
        .map_err(transport_err)?
        .ok_or_else(|| ErrorResponse {
            error: "A_JOB_NOT_FOUND: intent not found".to_string(),
            code: 404,
        })
        .map(Json)
}

/// Approval inbox for the control surface (Cowork): pending/granted/denied/
/// expired/invalidated bindings for a workspace, newest first. User auth.
#[derive(Debug, serde::Deserialize)]
pub struct ListApprovalsQuery {
    pub workspace: Option<String>,
    pub status: Option<String>,
}

async fn list_approvals(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListApprovalsQuery>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    if crate::auth::get_user(&headers).is_none() {
        return Err(ErrorResponse {
            error: "authentication required".to_string(),
            code: 401,
        });
    }
    let conn = state.db.connect().map_err(db_error)?;
    let (sql, filter) = match (&query.workspace, &query.status) {
        (Some(w), Some(st)) => (
            "SELECT id, run_id, job_id, executor, capability, target, status, decided_by, lease_generation, created_at
             FROM cowork_approval_bindings WHERE run_id IN (SELECT id FROM cowork_runs WHERE workspace_id = ?1) AND status = ?2
             ORDER BY created_at DESC, rowid DESC",
            Some((w.clone(), st.clone())),
        ),
        (Some(w), None) => (
            "SELECT id, run_id, job_id, executor, capability, target, status, decided_by, lease_generation, created_at
             FROM cowork_approval_bindings WHERE run_id IN (SELECT id FROM cowork_runs WHERE workspace_id = ?1)
             ORDER BY created_at DESC, rowid DESC",
            Some((w.clone(), String::new())),
        ),
        (None, Some(st)) => (
            "SELECT id, run_id, job_id, executor, capability, target, status, decided_by, lease_generation, created_at
             FROM cowork_approval_bindings WHERE status = ?1
             ORDER BY created_at DESC, rowid DESC",
            Some((String::new(), st.clone())),
        ),
        (None, None) => (
            "SELECT id, run_id, job_id, executor, capability, target, status, decided_by, lease_generation, created_at
             FROM cowork_approval_bindings ORDER BY created_at DESC, rowid DESC LIMIT 200",
            None,
        ),
    };
    let mut stmt = conn.prepare(sql).map_err(db_error)?;
    let rows: Vec<serde_json::Value> = match filter {
        Some((w, st)) if !w.is_empty() && !st.is_empty() => stmt
            .query_map(rusqlite::params![w, st], approval_row)
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?,
        Some((w, _)) if !w.is_empty() => stmt
            .query_map(rusqlite::params![w], approval_row)
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?,
        Some((_, st)) if !st.is_empty() => stmt
            .query_map(rusqlite::params![st], approval_row)
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?,
        _ => stmt
            .query_map([], approval_row)
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?,
    };
    Ok(Json(json!({ "approvals": rows })))
}

fn approval_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<serde_json::Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "run_id": row.get::<_, String>(1)?,
        "job_id": row.get::<_, String>(2)?,
        "executor": row.get::<_, String>(3)?,
        "capability": row.get::<_, String>(4)?,
        "target": row.get::<_, String>(5)?,
        "status": row.get::<_, String>(6)?,
        "decided_by": row.get::<_, Option<String>>(7)?,
        "lease_generation": row.get::<_, i64>(8)?,
        "created_at": row.get::<_, String>(9)?,
    }))
}

// ─── Connector broker (§8.5, A-T5) ──────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct ConnectorSessionRequest {
    pub lease_id: String,
    pub lease_generation: i64,
    pub capability: String,
    pub ttl_secs: Option<u64>,
}

async fn request_connector_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(req): Json<ConnectorSessionRequest>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let session = sqlite_store::request_connector_session(
        &mut conn,
        &principal,
        &job_id,
        &req.lease_id,
        req.lease_generation,
        &req.capability,
        req.ttl_secs.map(Duration::from_secs),
    )
    .map_err(transport_err)?;
    Ok(Json(session))
}

#[derive(Debug, serde::Deserialize)]
pub struct ConnectorInvokeRequest {
    pub job_id: String,
    pub lease_id: String,
    pub lease_generation: i64,
    pub payload: serde_json::Value,
}

async fn invoke_connector_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<ConnectorInvokeRequest>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    let principal = authenticate(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let outcome = sqlite_store::invoke_connector_session(
        &mut conn,
        &principal,
        &req.job_id,
        &req.lease_id,
        req.lease_generation,
        &session_id,
        req.payload,
    )
    .await
    .map_err(transport_err)?;
    Ok(Json(outcome))
}

fn require_user_or_desktop(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), ErrorResponse> {
    if crate::auth::get_user(headers).is_some() {
        return Ok(());
    }
    if crate::auth::verify_desktop_access_token(headers, &state.config) {
        return Ok(());
    }
    if crate::continuation::verify_continuation_token(headers, &state.config) {
        return Ok(());
    }
    Err(ErrorResponse {
        error: "authentication required".to_string(),
        code: 401,
    })
}

/// Promote one job to `compute.cloud` and drop any local lease.
async fn continue_job_in_cloud(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<sqlite_store::CloudContinuation>, ErrorResponse> {
    require_user_or_desktop(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let outcome = sqlite_store::continue_job_in_cloud(&mut conn, &job_id).map_err(transport_err)?;
    info!(job = %job_id, run = %outcome.run_id, "Job handed off to cloud continuation");
    Ok(Json(outcome))
}

/// Promote every non-terminal job on a run.
async fn continue_run_in_cloud(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    require_user_or_desktop(&state, &headers)?;
    let mut conn = state.db.connect().map_err(db_error)?;
    let jobs = sqlite_store::continue_run_in_cloud(&mut conn, &run_id).map_err(transport_err)?;
    Ok(Json(json!({ "run_id": run_id, "jobs": jobs })))
}

/// Desktop quit: retag in-flight jobs AND POST them to the always-on API.
/// Fail closed (409) when jobs exist but no continuation URL/token is set.
async fn handoff_all_in_flight(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    require_user_or_desktop(&state, &headers)?;
    let user = crate::auth::get_user(&headers);
    let mut conn = state.db.connect().map_err(db_error)?;
    let jobs = sqlite_store::handoff_all_in_flight(&mut conn).map_err(transport_err)?;
    if jobs.is_empty() {
        return Ok(Json(json!({ "jobs": jobs, "forwarded": 0 })));
    }
    let prefs_url = user.as_ref().and_then(|u| {
        conn.query_row(
            "SELECT continuation_api_url FROM user_cowork_preferences WHERE user_id = ?1",
            rusqlite::params![u.user_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    });
    let folders: Vec<String> = user
        .as_ref()
        .and_then(|u| {
            conn.query_row(
                "SELECT trusted_folders FROM user_cowork_preferences WHERE user_id = ?1",
                rusqlite::params![u.user_id],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    drop(conn);
    let files = crate::continuation::pack_trusted_folders(&folders).unwrap_or_default();
    let bundles: Vec<crate::continuation::ContinuationBundle> = jobs
        .iter()
        .filter_map(|j| {
            j.envelope.clone().map(|envelope| crate::continuation::ContinuationBundle {
                envelope,
                files: files.clone(),
            })
        })
        .collect();
    let mut forwarded = 0u64;
    let mut target_out: Option<String> = None;
    if let Some(target) = crate::continuation::resolve_target(&state.config, prefs_url.as_deref()) {
        if !bundles.is_empty() {
            forwarded = crate::continuation::forward_bundles(&state.config, &target, &bundles)
                .await
                .map_err(|e| ErrorResponse {
                    error: format!("continuation ingest failed: {e}"),
                    code: 502,
                })? as u64;
            target_out = Some(target);
        }
    }
    info!(count = jobs.len(), forwarded, "In-flight jobs retagged; envelopes ready for cloud-api relay");
    Ok(Json(json!({
        "jobs": jobs,
        "files": files,
        "forwarded": forwarded,
        "target": target_out,
    })))
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum IngestBody {
    Bundle(crate::continuation::ContinuationBundle),
    Envelope(allternit_cowork_runtime::IntentEnvelope),
}

/// Replay an intent onto this always-on API. Auth: user, desktop, or
/// `x-allternit-continuation-token`.
async fn ingest_continuation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<IngestBody>,
) -> Result<Json<serde_json::Value>, ErrorResponse> {
    require_user_or_desktop(&state, &headers)?;
    let (mut envelope, files) = match body {
        IngestBody::Bundle(b) => {
            let env: allternit_cowork_runtime::IntentEnvelope =
                serde_json::from_value(b.envelope).map_err(|e| ErrorResponse {
                    error: format!("invalid envelope: {e}"),
                    code: 400,
                })?;
            (env, b.files)
        }
        IngestBody::Envelope(env) => (env, Vec::new()),
    };
    envelope.compute = Some(json!({ "policy": "cloud" }));
    if envelope.intent_id.is_empty() {
        envelope.intent_id = format!("cont_{}", uuid::Uuid::new_v4());
    } else if !envelope.intent_id.starts_with("cont_") {
        envelope.intent_id = format!("cont_{}", envelope.intent_id);
    }
    let written = if files.is_empty() {
        0
    } else {
        crate::continuation::write_ingested_files(&state.config.cloud_workspace_dir(), &files)
            .map_err(|e| ErrorResponse {
                error: format!("folder handoff write failed: {e}"),
                code: 500,
            })?
    };
    let user = crate::auth::get_user(&headers);
    let mut conn = state.db.connect().map_err(db_error)?;
    let owner = user.as_ref().map(|u| u.user_id.as_str());
    let submission = sqlite_store::submit_intent_for_user(&mut conn, &envelope, owner)
        .map_err(transport_err)?;
    Ok(Json(json!({
        "intent_id": submission.intent_id,
        "run_id": submission.run_id,
        "created": submission.created,
        "files_written": written,
    })))
}
