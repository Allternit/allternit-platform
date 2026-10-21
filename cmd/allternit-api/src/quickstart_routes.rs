//! Managed agent quickstart state
//!
//! Organization-scoped checklist for the managed-agent onboarding flow.
//! The frontend reads the predefined step list and the user's completion state,
//! then marks individual steps complete as the user finishes them.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/quickstart", get(get_progress))
        .route("/admin/quickstart/:step", put(complete_step))
        .route("/admin/quickstart/:step", delete(uncomplete_step))
        .route("/admin/quickstart/reset", post(reset_progress))
}

const QUICKSTART_STEPS: &[(&str, &str)] = &[
    ("create_agent", "Create your first managed agent"),
    ("run_agent", "Run the agent with a prompt"),
    ("add_tool", "Add a tool to the agent"),
    ("enable_mcp", "Connect an MCP server"),
    ("invite_member", "Invite a team member"),
    ("deploy_agent", "Deploy the agent to a workspace"),
];

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "quickstart operation failed");
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
            "Only organization owners/admins can manage quickstart progress.",
        ));
    }
    Ok(org.to_string())
}

fn valid_step(step: &str) -> bool {
    QUICKSTART_STEPS.iter().any(|(id, _)| *id == step)
}

fn load_completed(conn: &rusqlite::Connection, org: &str) -> Result<Vec<String>, ApiError> {
    let row: Option<String> = conn
        .query_row(
            "SELECT completed_steps FROM organization_quickstart WHERE organization_id = ?1",
            params![org],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    match row {
        Some(text) => serde_json::from_str(&text).map_err(internal),
        None => Ok(Vec::new()),
    }
}

fn save_completed(conn: &rusqlite::Connection, org: &str, completed: &[String]) -> Result<(), ApiError> {
    let text = serde_json::to_string(completed).map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO organization_quickstart (organization_id, completed_steps, updated_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(organization_id) DO UPDATE SET
            completed_steps = excluded.completed_steps,
            updated_at = excluded.updated_at",
        params![org, text, now],
    )
    .map_err(internal)?;
    Ok(())
}

async fn get_progress(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let completed = load_completed(&conn, &org)?;

    let steps: Vec<Value> = QUICKSTART_STEPS
        .iter()
        .map(|(id, label)| {
            json!({
                "id": *id,
                "label": *label,
                "completed": completed.iter().any(|s| s == *id),
            })
        })
        .collect();

    Ok::<Json<Value>, ApiError>(Json(json!({
        "organization_id": org,
        "steps": steps,
        "completed_count": completed.len(),
        "total": QUICKSTART_STEPS.len(),
    })))
}

async fn complete_step(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(step): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    if !valid_step(&step) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_step", "Unknown quickstart step."));
    }

    let mut completed = load_completed(&conn, &org)?;
    if !completed.iter().any(|s| s == &step) {
        completed.push(step.clone());
    }
    save_completed(&conn, &org, &completed)?;

    Ok::<Json<Value>, ApiError>(Json(json!({
        "organization_id": org,
        "step": step,
        "completed": true,
        "completed_count": completed.len(),
        "total": QUICKSTART_STEPS.len(),
    })))
}

async fn uncomplete_step(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(step): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    if !valid_step(&step) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_step", "Unknown quickstart step."));
    }

    let mut completed = load_completed(&conn, &org)?;
    completed.retain(|s| s != &step);
    save_completed(&conn, &org, &completed)?;

    Ok::<Json<Value>, ApiError>(Json(json!({
        "organization_id": org,
        "step": step,
        "completed": false,
        "completed_count": completed.len(),
        "total": QUICKSTART_STEPS.len(),
    })))
}

async fn reset_progress(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    conn.execute(
        "DELETE FROM organization_quickstart WHERE organization_id = ?1",
        params![org],
    )
    .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({
        "organization_id": org,
        "completed_count": 0,
        "total": QUICKSTART_STEPS.len(),
    })))
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

    #[tokio::test]
    async fn quickstart_progress_lifecycle() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        // Initial progress
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/quickstart")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["completed_count"], 0);
        assert_eq!(body["total"], QUICKSTART_STEPS.len());
        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps[0]["id"], "create_agent");
        assert_eq!(steps[0]["completed"], false);

        // Complete a step
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/admin/quickstart/create_agent")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["completed_count"], 1);

        // Uncomplete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/admin/quickstart/create_agent")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Reset
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/quickstart/reset")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn quickstart_rejects_unknown_step() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/admin/quickstart/no_such_step")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
