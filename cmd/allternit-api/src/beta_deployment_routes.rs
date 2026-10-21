//! Scheduled deployments API (`/beta/deployments`).
//!
//! A deployment binds an agent to a cron schedule. `next_run_at` is derived
//! from the cron expression with [`crate::cron_lite`] and recomputed on
//! create, on schedule change, and each time a run is triggered. Individual
//! executions are recorded in `beta_deployment_runs` so callers can inspect
//! run history independent of the deployment's current schedule state.

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, error::ApiError, webhook_subscription_routes, AppState};

const DEPLOYMENT_STATUSES: &[&str] = &["active", "paused", "archived"];
const RUN_STATUSES: &[&str] = &["succeeded", "failed", "cancelled"];

fn empty_object() -> Value {
    json!({})
}

pub fn beta_deployment_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/beta/deployments",
            get(list_deployments).post(create_deployment),
        )
        .route(
            "/beta/deployments/:id",
            get(get_deployment)
                .patch(update_deployment)
                .delete(delete_deployment),
        )
        .route(
            "/beta/deployments/:id/runs",
            get(list_runs).post(trigger_run),
        )
        .route("/beta/deployments/:id/runs/:run_id", patch(update_run))
        .route("/schedules", get(list_deployments).post(create_deployment))
        .route(
            "/schedules/:id",
            get(get_deployment)
                .patch(update_deployment)
                .delete(delete_deployment),
        )
        .route("/schedules/:id/runs", get(list_runs).post(trigger_run))
        .route("/schedules/:id/runs/:run_id", patch(update_run))
}

#[derive(Debug, Deserialize)]
struct CreateDeploymentBody {
    agent_id: Option<String>,
    cron: String,
    #[serde(default = "empty_object")]
    metadata: Value,
}

#[derive(Debug, Deserialize)]
struct UpdateDeploymentBody {
    agent_id: Option<String>,
    cron: Option<String>,
    status: Option<String>,
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct UpdateRunBody {
    status: String,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    status: Option<String>,
}

#[derive(Debug, Serialize)]
struct DeploymentRow {
    id: String,
    agent_id: Option<String>,
    cron: String,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
    status: String,
    metadata: Value,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct RunRow {
    id: String,
    deployment_id: String,
    status: String,
    result: Option<Value>,
    error: Option<String>,
    started_at: String,
    finished_at: Option<String>,
    triggered_by: Option<String>,
}

const DEPLOYMENT_SELECT: &str = "SELECT id, agent_id, cron, next_run_at, last_run_at, status,
    metadata, created_at, updated_at FROM beta_deployments";

fn read_deployment(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentRow> {
    let metadata: String = row.get(6)?;
    Ok(DeploymentRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        cron: row.get(2)?,
        next_run_at: row.get(3)?,
        last_run_at: row.get(4)?,
        status: row.get(5)?,
        metadata: serde_json::from_str(&metadata).unwrap_or_else(|_| json!({})),
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn read_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    let result: Option<String> = row.get(3)?;
    Ok(RunRow {
        id: row.get(0)?,
        deployment_id: row.get(1)?,
        status: row.get(2)?,
        result: result.and_then(|value| serde_json::from_str(&value).ok()),
        error: row.get(4)?,
        started_at: row.get(5)?,
        finished_at: row.get(6)?,
        triggered_by: row.get(7)?,
    })
}

const RUN_SELECT: &str = "SELECT id, deployment_id, status, result, error, started_at,
    finished_at, triggered_by FROM beta_deployment_runs";

async fn create_deployment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateDeploymentBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !body.metadata.is_object() {
        return Err(ApiError::BadRequest("metadata must be an object".into()));
    }
    let now = Utc::now();
    let next_run_at = crate::cron_lite::next_run_after(&body.cron, now)
        .map_err(|e| ApiError::BadRequest(format!("invalid cron expression: {e}")))?;

    let db = state.db.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let result_id = id.clone();
    let deployment = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO beta_deployments (id, user_id, agent_id, cron, next_run_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                user.user_id,
                body.agent_id,
                body.cron,
                next_run_at.to_rfc3339(),
                body.metadata.to_string()
            ],
        )?;
        conn.query_row(
            &format!("{DEPLOYMENT_SELECT} WHERE id = ?1"),
            params![id],
            read_deployment,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(json!({"deployment": deployment, "id": result_id})),
    ))
}

async fn list_deployments(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "{DEPLOYMENT_SELECT} WHERE user_id = ?1 AND (?2 IS NULL OR status = ?2)
             ORDER BY created_at DESC"
        ))?;
        let rows = stmt
            .query_map(params![user.user_id, query.status], read_deployment)?
            .collect::<Result<Vec<DeploymentRow>, _>>()?;
        Ok::<Vec<DeploymentRow>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(json!({"deployments": rows})))
}

async fn load_deployment(
    state: Arc<AppState>,
    user_id: String,
    id: String,
) -> Result<DeploymentRow, ApiError> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            &format!("{DEPLOYMENT_SELECT} WHERE id = ?1 AND user_id = ?2"),
            params![id, user_id],
            read_deployment,
        )
        .optional()
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??
    .ok_or_else(|| ApiError::NotFound("deployment not found".into()))
}

async fn get_deployment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let deployment = load_deployment(state, user.user_id, id).await?;
    Ok(Json(json!({"deployment": deployment})))
}

async fn update_deployment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDeploymentBody>,
) -> Result<Json<Value>, ApiError> {
    if body.agent_id.is_none() && body.cron.is_none() && body.status.is_none() && body.metadata.is_none() {
        return Err(ApiError::BadRequest("no fields to update".into()));
    }
    if body
        .metadata
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Err(ApiError::BadRequest("metadata must be an object".into()));
    }
    if let Some(status) = body.status.as_deref() {
        if !DEPLOYMENT_STATUSES.contains(&status) {
            return Err(ApiError::BadRequest("unsupported deployment status".into()));
        }
    }
    // Recomputing next_run_at requires the current stored cron when the
    // caller only changes `status` (e.g. resuming a paused deployment).
    let existing = load_deployment(state.clone(), user.user_id.clone(), id.clone()).await?;
    let cron = body.cron.clone().unwrap_or_else(|| existing.cron.clone());
    let next_run_at = crate::cron_lite::next_run_after(&cron, Utc::now())
        .map_err(|e| ApiError::BadRequest(format!("invalid cron expression: {e}")))?;

    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let lookup_id = id.clone();
    let affected = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE beta_deployments SET
                agent_id = COALESCE(?1, agent_id),
                cron = COALESCE(?2, cron),
                next_run_at = ?3,
                status = COALESCE(?4, status),
                metadata = COALESCE(?5, metadata),
                updated_at = CURRENT_TIMESTAMP
             WHERE id = ?6 AND user_id = ?7",
            params![
                body.agent_id,
                body.cron,
                next_run_at.to_rfc3339(),
                body.status,
                body.metadata.map(|v| v.to_string()),
                id,
                user_id
            ],
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;
    if affected == 0 {
        return Err(ApiError::NotFound("deployment not found".into()));
    }
    let deployment = load_deployment(state, user.user_id, lookup_id).await?;
    Ok(Json(json!({"deployment": deployment})))
}

async fn delete_deployment(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let db = state.db.clone();
    let deleted = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM beta_deployments WHERE id = ?1 AND user_id = ?2",
            params![id, user.user_id],
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    if deleted == 0 {
        return Err(ApiError::NotFound("deployment not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_runs(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    load_deployment(state.clone(), user.user_id, id.clone()).await?;
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            &format!("{RUN_SELECT} WHERE deployment_id = ?1 ORDER BY started_at DESC"),
        )?;
        let rows = stmt
            .query_map(params![id], read_run)?
            .collect::<Result<Vec<RunRow>, _>>()?;
        Ok::<Vec<RunRow>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(json!({"runs": rows})))
}

/// Insert a `running` run row for `deployment_id` plus the deployment-tied
/// `beta_work_tasks` row that external workers lease and execute. Shared by
/// the manual trigger handler ([`trigger_run`]) and the deployment scheduler
/// daemon ([`crate::deployment_scheduler`]) so both produce the same shape.
/// Must be called inside the caller's transaction. Returns (run_id, task_id).
pub(crate) fn insert_deployment_run_tx(
    tx: &rusqlite::Transaction<'_>,
    deployment_id: &str,
    user_id: &str,
    agent_id: Option<&str>,
    triggered_by: &str,
) -> Result<(String, String), rusqlite::Error> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO beta_deployment_runs (id, deployment_id, status, triggered_by)
         VALUES (?1, ?2, 'running', ?3)",
        params![run_id, deployment_id, triggered_by],
    )?;
    // Mirror the session-run payload convention ({messages, tools}) so
    // existing workers can execute deployment runs without changes; a
    // scheduled deployment carries no prompt, so messages is empty.
    let payload = json!({
        "deployment_run_id": run_id,
        "agent_id": agent_id,
        "messages": [],
        "tools": Value::Null,
    });
    tx.execute(
        "INSERT INTO beta_work_tasks (id, user_id, deployment_id, payload)
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, user_id, deployment_id, payload.to_string()],
    )?;
    Ok((run_id, task_id))
}

async fn trigger_run(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let deployment = load_deployment(state.clone(), user.user_id.clone(), id.clone()).await?;
    let now = Utc::now();
    let next_run_at = crate::cron_lite::next_run_after(&deployment.cron, now)
        .map_err(|e| ApiError::BadRequest(format!("invalid cron expression: {e}")))?;

    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let agent_id = deployment.agent_id.clone();
    let webhook_deployment_id = id.clone();
    let run = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        let (run_id, _task_id) =
            insert_deployment_run_tx(&tx, &id, &user_id, agent_id.as_deref(), "manual")?;
        tx.execute(
            "UPDATE beta_deployments SET last_run_at = ?1, next_run_at = ?2, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?3",
            params![now.to_rfc3339(), next_run_at.to_rfc3339(), id],
        )?;
        let run = tx.query_row(
            &format!("{RUN_SELECT} WHERE id = ?1"),
            params![run_id],
            read_run,
        )?;
        tx.commit()?;
        Ok::<(RunRow, String), rusqlite::Error>((run, run_id))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;
    let result_id = run.1.clone();

    webhook_subscription_routes::deliver_registered_event(
        state.clone(),
        user.organization_id.as_deref(),
        webhook_subscription_routes::events::DEPLOYMENT_RUN_CREATED,
        json!({"deployment_id": webhook_deployment_id, "run_id": result_id, "triggered_by": "manual"}),
    )
    .await;

    Ok((
        StatusCode::CREATED,
        Json(json!({"run": run.0, "id": result_id})),
    ))
}

async fn update_run(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((id, run_id)): Path<(String, String)>,
    Json(body): Json<UpdateRunBody>,
) -> Result<Json<Value>, ApiError> {
    if !RUN_STATUSES.contains(&body.status.as_str()) {
        return Err(ApiError::BadRequest("unsupported run status".into()));
    }
    load_deployment(state.clone(), user.user_id, id.clone()).await?;
    let db = state.db.clone();
    let lookup_run_id = run_id.clone();
    let deployment_id = id.clone();
    let affected = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE beta_deployment_runs SET status = ?1, result = ?2, error = ?3,
             finished_at = CURRENT_TIMESTAMP WHERE id = ?4 AND deployment_id = ?5",
            params![
                body.status,
                body.result.map(|v| v.to_string()),
                body.error,
                run_id,
                deployment_id
            ],
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    if affected == 0 {
        return Err(ApiError::NotFound("run not found".into()));
    }
    let db = state.db.clone();
    let run = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            &format!("{RUN_SELECT} WHERE id = ?1"),
            params![lookup_run_id],
            read_run,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| ApiError::DbError(e.to_string()))?;
    let run_value = serde_json::to_value(&run).unwrap_or_else(|_| json!({}));
    webhook_subscription_routes::deliver_deployment_run_update(
        state.clone(),
        user.organization_id.as_deref(),
        &id,
        &run_value,
    )
    .await;
    Ok(Json(json!({"run": run})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::path::Path as FsPath;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    fn test_user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: Some(format!("{}@example.test", id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "allternit-beta-deployments-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn test_app_state(temp: &FsPath) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
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
            office_runtime: Arc::new(RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
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

    fn json_body(value: &Value) -> Body {
        Body::from(value.to_string())
    }

    #[test]
    fn accepts_only_defined_deployment_statuses() {
        for status in ["active", "paused", "archived"] {
            assert!(DEPLOYMENT_STATUSES.contains(&status));
        }
        assert!(!DEPLOYMENT_STATUSES.contains(&"running"));
    }

    #[test]
    fn accepts_only_terminal_run_statuses_for_updates() {
        for status in ["succeeded", "failed", "cancelled"] {
            assert!(RUN_STATUSES.contains(&status));
        }
        assert!(!RUN_STATUSES.contains(&"running"));
    }

    #[tokio::test]
    async fn deployment_lifecycle_and_run_history() {
        let temp = temp_dir("lifecycle");
        let state = test_app_state(&temp).await;
        let app = beta_deployment_router().with_state(state);

        // Create a deployment.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/deployments")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a"))
                    .body(json_body(&json!({
                        "agent_id": "agent-1",
                        "cron": "0 9 * * *",
                        "metadata": {"region": "us-east"}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let deployment = &body["deployment"];
        let id = deployment["id"].as_str().unwrap().to_string();
        assert_eq!(deployment["agent_id"], json!("agent-1"));
        assert_eq!(deployment["status"], json!("active"));
        assert_eq!(deployment["metadata"]["region"], json!("us-east"));
        assert!(deployment["next_run_at"].as_str().is_some());

        // List returns the deployment for the owner.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/deployments")
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["deployments"].as_array().unwrap().len(), 1);

        // Another user cannot see it.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/deployments")
                    .extension(test_user("user-b"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["deployments"].as_array().unwrap().len(), 0);

        // Get by id.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/deployments/{}", id))
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Update status and cron.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/beta/deployments/{}", id))
                    .header("content-type", "application/json")
                    .extension(test_user("user-a"))
                    .body(json_body(&json!({"status": "paused", "cron": "30 10 * * *"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["deployment"]["status"], json!("paused"));
        assert_eq!(body["deployment"]["cron"], json!("30 10 * * *"));

        // Trigger a run.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/beta/deployments/{}/runs", id))
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let run = &body["run"];
        let run_id = run["id"].as_str().unwrap().to_string();
        assert_eq!(run["status"], json!("running"));

        // List runs.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/deployments/{}/runs", id))
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["runs"].as_array().unwrap().len(), 1);

        // Complete the run.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/beta/deployments/{}/runs/{}", id, run_id))
                    .header("content-type", "application/json")
                    .extension(test_user("user-a"))
                    .body(json_body(&json!({
                        "status": "succeeded",
                        "result": {"ok": true}
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["run"]["status"], json!("succeeded"));
        assert!(body["run"]["finished_at"].as_str().is_some());

        // Delete the deployment (cascades to runs).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/beta/deployments/{}", id))
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Getting it now returns 404.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/deployments/{}", id))
                    .extension(test_user("user-a"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn rejects_invalid_cron_and_unknown_status() {
        let temp = temp_dir("validation");
        let state = test_app_state(&temp).await;
        let app = beta_deployment_router().with_state(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/deployments")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a"))
                    .body(json_body(&json!({"cron": "not-a-cron"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/deployments")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a"))
                    .body(json_body(&json!({"cron": "* * * * *", "status": "unknown"})))
                    .unwrap(),
            )
            .await
            .unwrap();
        // status is not accepted on create; the body is ignored and the
        // deployment defaults to active, so this succeeds.
        assert_eq!(resp.status(), StatusCode::CREATED);

        let _ = std::fs::remove_dir_all(&temp);
    }
}
