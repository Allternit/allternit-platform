//! Admin federation management
//!
//! Organization-scoped federation issuers and claim-to-role mapping rules for
//! trusting external identity providers.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/admin/federation/issuers",
            post(create_issuer).get(list_issuers),
        )
        .route(
            "/admin/federation/issuers/:id",
            get(get_issuer).put(update_issuer).delete(delete_issuer),
        )
        .route(
            "/admin/federation/rules",
            post(create_rule).get(list_rules),
        )
        .route(
            "/admin/federation/rules/:id",
            get(get_rule).put(update_rule).delete(delete_rule),
        )
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "federation operation failed");
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
            "Only organization owners/admins can manage federation.",
        ));
    }
    Ok(org.to_string())
}

fn validate_issuer_url(url: &str) -> Result<(), ApiError> {
    if url.trim().is_empty() || url.len() > 512 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_issuer_url",
            "Issuer URL must be 1-512 characters.",
        ));
    }
    if !url.starts_with("https://") {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_issuer_url",
            "Issuer URL must use https://.",
        ));
    }
    Ok(())
}

fn validate_claim(name: &str, value: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.len() > 128 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_claim_name",
            "Claim name must be 1-128 characters.",
        ));
    }
    if value.trim().is_empty() || value.len() > 512 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_claim_value",
            "Claim value must be 1-512 characters.",
        ));
    }
    Ok(())
}

fn issuer_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "organization_id": row.get::<_, String>(1)?,
        "issuer_url": row.get::<_, String>(2)?,
        "enabled": row.get::<_, i64>(3)? == 1,
        "created_by": row.get::<_, String>(4)?,
        "created_at": row.get::<_, String>(5)?,
        "updated_at": row.get::<_, String>(6)?,
    }))
}

fn find_issuer(conn: &rusqlite::Connection, id: &str, org: &str) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT id, organization_id, issuer_url, enabled, created_by, created_at, updated_at
         FROM federation_issuers
         WHERE id = ?1 AND organization_id = ?2",
        params![id, org],
        issuer_json,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "issuer_not_found", "No such federation issuer."))
}

#[derive(Deserialize)]
struct CreateIssuer {
    issuer_url: String,
    enabled: Option<bool>,
}

async fn create_issuer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateIssuer>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_issuer_url(&body.issuer_url)?;

    let id = uuid::Uuid::new_v4().to_string();
    let enabled = if body.enabled.unwrap_or(true) { 1 } else { 0 };
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO federation_issuers
         (id, organization_id, issuer_url, enabled, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, org, body.issuer_url.trim(), enabled, user.user_id, now, now],
    )
    .map_err(internal)?;

    let issuer = find_issuer(&conn, &id, &org)?;
    Ok::<Response, ApiError>((StatusCode::CREATED, Json(issuer)).into_response())
}

async fn list_issuers(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let mut stmt = conn
        .prepare(
            "SELECT id, organization_id, issuer_url, enabled, created_by, created_at, updated_at
             FROM federation_issuers
             WHERE organization_id = ?1
             ORDER BY updated_at DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org], issuer_json)
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn get_issuer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let issuer = find_issuer(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(issuer))
}

#[derive(Deserialize)]
struct UpdateIssuer {
    issuer_url: Option<String>,
    enabled: Option<bool>,
}

async fn update_issuer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateIssuer>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let _ = find_issuer(&conn, &id, &org)?;

    if let Some(ref url) = body.issuer_url {
        validate_issuer_url(url)?;
    }

    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE federation_issuers SET
            issuer_url = COALESCE(?1, issuer_url),
            enabled = COALESCE(?2, enabled),
            updated_at = ?3
         WHERE id = ?4 AND organization_id = ?5",
        params![
            body.issuer_url.as_deref().map(str::trim),
            body.enabled.map(|b| if b { 1 } else { 0 }),
            now,
            id,
            org,
        ],
    )
    .map_err(internal)?;

    let issuer = find_issuer(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(issuer))
}

async fn delete_issuer(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let rows = conn
        .execute(
            "DELETE FROM federation_issuers WHERE id = ?1 AND organization_id = ?2",
            params![id, org],
        )
        .map_err(internal)?;

    if rows == 0 {
        return Err(error(StatusCode::NOT_FOUND, "issuer_not_found", "No such federation issuer."));
    }

    Ok::<Json<Value>, ApiError>(Json(json!({ "deleted": true })))
}

fn rule_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "organization_id": row.get::<_, String>(1)?,
        "issuer_id": row.get::<_, String>(2)?,
        "claim_name": row.get::<_, String>(3)?,
        "claim_value": row.get::<_, String>(4)?,
        "workspace_id": row.get::<_, Option<String>>(5)?,
        "role": row.get::<_, String>(6)?,
        "created_by": row.get::<_, String>(7)?,
        "created_at": row.get::<_, String>(8)?,
        "updated_at": row.get::<_, String>(9)?,
    }))
}

fn find_rule(conn: &rusqlite::Connection, id: &str, org: &str) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT id, organization_id, issuer_id, claim_name, claim_value,
                workspace_id, role, created_by, created_at, updated_at
         FROM federation_rules
         WHERE id = ?1 AND organization_id = ?2",
        params![id, org],
        rule_json,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "rule_not_found", "No such federation rule."))
}

fn valid_role(role: &str) -> bool {
    matches!(role, "owner" | "admin" | "member")
}

#[derive(Deserialize)]
struct CreateRule {
    issuer_id: String,
    claim_name: String,
    claim_value: String,
    workspace_id: Option<String>,
    role: String,
}

async fn create_rule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateRule>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    // Ensure issuer exists and belongs to org.
    let _ = find_issuer(&conn, &body.issuer_id, &org)?;

    validate_claim(&body.claim_name, &body.claim_value)?;
    if !valid_role(&body.role) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_role",
            "Role must be one of: owner, admin, member.",
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO federation_rules
         (id, organization_id, issuer_id, claim_name, claim_value, workspace_id, role, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            org,
            body.issuer_id,
            body.claim_name.trim(),
            body.claim_value.trim(),
            body.workspace_id,
            body.role,
            user.user_id,
            now,
            now,
        ],
    )
    .map_err(internal)?;

    let rule = find_rule(&conn, &id, &org)?;
    Ok::<Response, ApiError>((StatusCode::CREATED, Json(rule)).into_response())
}

async fn list_rules(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let mut stmt = conn
        .prepare(
            "SELECT id, organization_id, issuer_id, claim_name, claim_value,
                    workspace_id, role, created_by, created_at, updated_at
             FROM federation_rules
             WHERE organization_id = ?1
             ORDER BY updated_at DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org], rule_json)
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn get_rule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let rule = find_rule(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(rule))
}

#[derive(Deserialize)]
struct UpdateRule {
    claim_name: Option<String>,
    claim_value: Option<String>,
    workspace_id: Option<String>,
    role: Option<String>,
}

async fn update_rule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateRule>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let _ = find_rule(&conn, &id, &org)?;

    if let (Some(ref name), Some(ref value)) = (&body.claim_name, &body.claim_value) {
        validate_claim(name, value)?;
    } else if body.claim_name.is_some() || body.claim_value.is_some() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_claim",
            "claim_name and claim_value must be updated together.",
        ));
    }

    if let Some(ref role) = body.role {
        if !valid_role(role) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_role",
                "Role must be one of: owner, admin, member.",
            ));
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE federation_rules SET
            claim_name = COALESCE(?1, claim_name),
            claim_value = COALESCE(?2, claim_value),
            workspace_id = COALESCE(?3, workspace_id),
            role = COALESCE(?4, role),
            updated_at = ?5
         WHERE id = ?6 AND organization_id = ?7",
        params![
            body.claim_name.as_deref().map(str::trim),
            body.claim_value.as_deref().map(str::trim),
            body.workspace_id,
            body.role,
            now,
            id,
            org,
        ],
    )
    .map_err(internal)?;

    let rule = find_rule(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(rule))
}

async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let rows = conn
        .execute(
            "DELETE FROM federation_rules WHERE id = ?1 AND organization_id = ?2",
            params![id, org],
        )
        .map_err(internal)?;

    if rows == 0 {
        return Err(error(StatusCode::NOT_FOUND, "rule_not_found", "No such federation rule."));
    }

    Ok::<Json<Value>, ApiError>(Json(json!({ "deleted": true })))
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
            tenant_id: None,
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
        for (org_id, user_id, role) in [
            ("org-1", "admin-1", "owner"),
            ("org-1", "member-1", "member"),
            ("org-2", "admin-2", "owner"),
        ] {
            conn.execute(
                "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
                params![org_id],
            )
            .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
                params![user_id, format!("{}@test.local", user_id)],
            )
            .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, ?4)",
                params![format!("{}:{}", org_id, user_id), org_id, user_id, role],
            )
            .unwrap();
        }
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
    async fn issuer_and_rule_lifecycle() {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        // Create issuer
        let issuer = json!({ "issuer_url": "https://issuer.example" });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/federation/issuers")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&issuer))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let issuer_id = body["id"].as_str().unwrap().to_string();
        assert_eq!(body["issuer_url"], "https://issuer.example");
        assert_eq!(body["enabled"], true);

        // Create rule
        let rule = json!({
            "issuer_id": issuer_id,
            "claim_name": "groups",
            "claim_value": "engineering",
            "workspace_id": "ws-1",
            "role": "admin"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/federation/rules")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&rule))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let rule_id = body["id"].as_str().unwrap().to_string();
        assert_eq!(body["claim_name"], "groups");
        assert_eq!(body["role"], "admin");

        // List rules
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/federation/rules")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let list = body_json(resp.into_body()).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);

        // Delete issuer cascades rules
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&format!("/admin/federation/issuers/{}", issuer_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Rule should be gone
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/admin/federation/rules/{}", rule_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn issuer_requires_https() {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let issuer = json!({ "issuer_url": "http://issuer.example" });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/federation/issuers")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&issuer))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn federation_isolation_by_organization() {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let issuer = json!({ "issuer_url": "https://issuer.example" });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/federation/issuers")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&issuer))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        let issuer_id = body["id"].as_str().unwrap().to_string();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/admin/federation/issuers/{}", issuer_id))
                    .extension(test_user("admin-2", Some("org-2")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
