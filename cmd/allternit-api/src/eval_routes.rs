//! Agent evaluation datasets and runs
//!
//! Provides organization-scoped scaffolding for defining evaluation datasets,
//! scheduling runs against an agent + rubric, and recording per-case results
//! and aggregate scores. Full automated execution is left to the worker that
//! drives the agent runtime; this API owns the durable state.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, eval_metrics::score_rubric_criteria, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/eval/datasets", post(create_dataset).get(list_datasets))
        .route(
            "/admin/eval/datasets/:id",
            get(get_dataset).put(update_dataset).delete(delete_dataset),
        )
        .route("/admin/eval/runs", post(create_run).get(list_runs))
        .route(
            "/admin/eval/runs/:id",
            get(get_run).delete(delete_run),
        )
        .route("/admin/eval/runs/:id/scores", post(record_scores))
        .route("/admin/eval/runs/:id/grade", post(grade_run))
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "eval operation failed");
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
            "Only organization owners/admins can manage evaluations.",
        ));
    }
    Ok(org.to_string())
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.len() > 128 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Name must be 1-128 characters.",
        ));
    }
    Ok(())
}

fn valid_status(status: &str) -> bool {
    matches!(status, "pending" | "running" | "completed" | "failed")
}

fn validate_rubric_id(
    conn: &rusqlite::Connection,
    rubric_id: &str,
    org: &str,
) -> Result<(), ApiError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM outcome_rubrics WHERE id = ?1 AND organization_id = ?2)",
            params![rubric_id, org],
            |row| row.get::<_, bool>(0),
        )
        .map_err(internal)?;
    if !exists {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "rubric_not_found",
            "The specified rubric does not exist or does not belong to this organization.",
        ));
    }
    Ok(())
}

fn dataset_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let cases: Value = serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or(json!([]));
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "organization_id": row.get::<_, String>(1)?,
        "name": row.get::<_, String>(2)?,
        "description": row.get::<_, Option<String>>(3)?,
        "cases": cases,
        "created_by": row.get::<_, String>(5)?,
        "created_at": row.get::<_, String>(6)?,
        "updated_at": row.get::<_, String>(7)?,
    }))
}

fn find_dataset(conn: &rusqlite::Connection, id: &str, org: &str) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT id, organization_id, name, description, cases, created_by, created_at, updated_at
         FROM eval_datasets
         WHERE id = ?1 AND organization_id = ?2",
        params![id, org],
        dataset_json,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "dataset_not_found", "No such eval dataset."))
}

#[derive(Deserialize)]
struct CreateDataset {
    name: String,
    description: Option<String>,
    #[serde(default)]
    cases: Vec<Value>,
}

async fn create_dataset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateDataset>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_name(&body.name)?;

    let id = uuid::Uuid::new_v4().to_string();
    let cases_json = serde_json::to_string(&body.cases).map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO eval_datasets
         (id, organization_id, name, description, cases, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            org,
            body.name.trim(),
            body.description.as_deref().map(str::trim),
            cases_json,
            user.user_id,
            now,
            now,
        ],
    )
    .map_err(internal)?;

    let dataset = find_dataset(&conn, &id, &org)?;
    Ok::<Response, ApiError>((StatusCode::CREATED, Json(dataset)).into_response())
}

async fn list_datasets(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let mut stmt = conn
        .prepare(
            "SELECT id, organization_id, name, description, cases, created_by, created_at, updated_at
             FROM eval_datasets
             WHERE organization_id = ?1
             ORDER BY updated_at DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org], dataset_json)
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn get_dataset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let dataset = find_dataset(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(dataset))
}

#[derive(Deserialize)]
struct UpdateDataset {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    cases: Option<Vec<Value>>,
}

async fn update_dataset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDataset>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let _ = find_dataset(&conn, &id, &org)?;

    if let Some(ref name) = body.name {
        validate_name(name)?;
    }
    let cases_json = body
        .cases
        .as_ref()
        .map(|c| serde_json::to_string(c).map_err(internal))
        .transpose()?;
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE eval_datasets SET
            name = COALESCE(?1, name),
            description = COALESCE(?2, description),
            cases = COALESCE(?3, cases),
            updated_at = ?4
         WHERE id = ?5 AND organization_id = ?6",
        params![
            body.name.as_deref().map(str::trim),
            body.description.as_deref().map(str::trim),
            cases_json,
            now,
            id,
            org,
        ],
    )
    .map_err(internal)?;

    let dataset = find_dataset(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(dataset))
}

async fn delete_dataset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let rows = conn
        .execute(
            "DELETE FROM eval_datasets WHERE id = ?1 AND organization_id = ?2",
            params![id, org],
        )
        .map_err(internal)?;
    if rows == 0 {
        return Err(error(StatusCode::NOT_FOUND, "dataset_not_found", "No such eval dataset."));
    }

    Ok::<Json<Value>, ApiError>(Json(json!({ "deleted": true })))
}

fn run_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let results: Value = serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or(json!([]));
    let scores: Value = serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or(json!({}));
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "organization_id": row.get::<_, String>(1)?,
        "dataset_id": row.get::<_, String>(2)?,
        "rubric_id": row.get::<_, Option<String>>(3)?,
        "agent_id": row.get::<_, Option<String>>(4)?,
        "name": row.get::<_, String>(5)?,
        "status": row.get::<_, String>(6)?,
        "results": results,
        "scores": scores,
        "created_by": row.get::<_, String>(9)?,
        "created_at": row.get::<_, String>(10)?,
        "updated_at": row.get::<_, String>(11)?,
    }))
}

fn find_run(conn: &rusqlite::Connection, id: &str, org: &str) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT id, organization_id, dataset_id, rubric_id, agent_id, name, status,
                results, scores, created_by, created_at, updated_at
         FROM eval_runs
         WHERE id = ?1 AND organization_id = ?2",
        params![id, org],
        run_json,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "run_not_found", "No such eval run."))
}

#[derive(Deserialize)]
struct CreateRun {
    dataset_id: String,
    #[serde(default)]
    rubric_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    name: String,
}

async fn create_run(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateRun>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_name(&body.name)?;
    let _ = find_dataset(&conn, &body.dataset_id, &org)?;
    if let Some(ref rubric_id) = body.rubric_id {
        validate_rubric_id(&conn, rubric_id, &org)?;
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO eval_runs
         (id, organization_id, dataset_id, rubric_id, agent_id, name, status,
          results, scores, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', '[]', '{}', ?7, ?8, ?9)",
        params![
            id,
            org,
            body.dataset_id,
            body.rubric_id,
            body.agent_id,
            body.name.trim(),
            user.user_id,
            now,
            now,
        ],
    )
    .map_err(internal)?;

    let run = find_run(&conn, &id, &org)?;
    Ok::<Response, ApiError>((StatusCode::CREATED, Json(run)).into_response())
}

async fn list_runs(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let mut stmt = conn
        .prepare(
            "SELECT id, organization_id, dataset_id, rubric_id, agent_id, name, status,
                    results, scores, created_by, created_at, updated_at
             FROM eval_runs
             WHERE organization_id = ?1
             ORDER BY updated_at DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org], run_json)
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn get_run(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let run = find_run(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(run))
}

async fn delete_run(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let rows = conn
        .execute(
            "DELETE FROM eval_runs WHERE id = ?1 AND organization_id = ?2",
            params![id, org],
        )
        .map_err(internal)?;
    if rows == 0 {
        return Err(error(StatusCode::NOT_FOUND, "run_not_found", "No such eval run."));
    }

    Ok::<Json<Value>, ApiError>(Json(json!({ "deleted": true })))
}

#[derive(Deserialize)]
struct RecordScores {
    status: String,
    #[serde(default)]
    results: Vec<Value>,
    #[serde(default)]
    scores: Value,
}

async fn record_scores(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<RecordScores>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let _ = find_run(&conn, &id, &org)?;

    if !valid_status(&body.status) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_status",
            "Status must be one of: pending, running, completed, failed.",
        ));
    }

    let results_json = serde_json::to_string(&body.results).map_err(internal)?;
    let scores_json = serde_json::to_string(&body.scores).map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "UPDATE eval_runs SET
            status = ?1,
            results = ?2,
            scores = ?3,
            updated_at = ?4
         WHERE id = ?5 AND organization_id = ?6",
        params![body.status, results_json, scores_json, now, id, org],
    )
    .map_err(internal)?;

    let run = find_run(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(run))
}

#[derive(Deserialize)]
struct GradeRun {
    rubric_id: Option<String>,
}

async fn grade_run(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<GradeRun>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let run = find_run(&conn, &id, &org)?;

    let rubric_id = body
        .rubric_id
        .or_else(|| run["rubric_id"].as_str().map(str::to_string))
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "rubric_required",
                "A rubric_id must be provided or set on the run.",
            )
        })?;
    validate_rubric_id(&conn, &rubric_id, &org)?;

    let criteria_json: String = conn
        .query_row(
            "SELECT criteria FROM outcome_rubrics WHERE id = ?1 AND organization_id = ?2",
            params![rubric_id, org],
            |row| row.get::<_, String>(0),
        )
        .map_err(internal)?;
    let criteria: Vec<Value> = serde_json::from_str(&criteria_json).map_err(internal)?;

    let results: Vec<Value> = serde_json::from_str(
        &run["results"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| run["results"].to_string()),
    )
    .unwrap_or_default();

    let mut grades = Vec::new();
    for case in &results {
        let prediction = case["prediction"].as_str().unwrap_or("");
        let reference = case["expected"].as_str().unwrap_or("");
        let criterion_scores = score_rubric_criteria(&criteria, prediction, reference);
        grades.push(json!({
            "case_index": case["case_index"],
            "criterion_scores": criterion_scores,
        }));
    }

    let mut scores = run["scores"].clone();
    if let Some(scores_obj) = scores.as_object_mut() {
        scores_obj.insert(
            "rubric_grades".to_string(),
            json!({
                "rubric_id": rubric_id,
                "grades": grades,
            }),
        );
    } else {
        scores = json!({
            "rubric_grades": {
                "rubric_id": rubric_id,
                "grades": grades,
            }
        });
    }

    let scores_json = serde_json::to_string(&scores).map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE eval_runs SET scores = ?1, updated_at = ?2 WHERE id = ?3 AND organization_id = ?4",
        params![scores_json, now, id, org],
    )
    .map_err(internal)?;

    let run = find_run(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(run))
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

    fn json_body(value: &Value) -> Body {
        Body::from(value.to_string())
    }

    #[tokio::test]
    async fn eval_dataset_and_run_lifecycle() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let dataset = json!({
            "name": "Greeting tests",
            "description": "Basic greeting eval",
            "cases": [
                { "prompt": "Say hello", "expected": "hello" }
            ]
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/eval/datasets")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&dataset))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let dataset_id = body["id"].as_str().unwrap().to_string();
        assert_eq!(body["name"], "Greeting tests");

        let run = json!({
            "dataset_id": dataset_id,
            "name": "Run 1",
            "agent_id": "agent-1",
            "rubric_id": null
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/eval/runs")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&run))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let run_id = body["id"].as_str().unwrap().to_string();
        assert_eq!(body["status"], "pending");

        // Record scores
        let scores = json!({
            "status": "completed",
            "results": [{ "case_index": 0, "score": 1.0 }],
            "scores": { "total": 1.0 }
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/admin/eval/runs/{}/scores", run_id))
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&scores))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], "completed");
        assert_eq!(body["scores"]["total"], 1.0);

        // Delete run and dataset
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&format!("/admin/eval/runs/{}", run_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&format!("/admin/eval/datasets/{}", dataset_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn eval_rejects_invalid_status() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let dataset = json!({ "name": "Ds", "cases": [] });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/eval/datasets")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&dataset))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        let dataset_id = body["id"].as_str().unwrap().to_string();

        let run = json!({ "dataset_id": dataset_id, "name": "Run" });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/eval/runs")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&run))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        let run_id = body["id"].as_str().unwrap().to_string();

        let scores = json!({
            "status": "bad_status",
            "results": [],
            "scores": {}
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/admin/eval/runs/{}/scores", run_id))
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&scores))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
