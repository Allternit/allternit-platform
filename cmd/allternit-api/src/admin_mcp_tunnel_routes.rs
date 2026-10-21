//! Admin MCP tunnel management
//!
//! Organization-scoped CRUD for MCP tunnels plus token reveal/rotate.
//! Tunnels connect Allternit to externally hosted MCP servers and carry an
//! optional mTLS/OAuth auth policy that is enforced by `mcp_tunnel_auth`.

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
        .route("/admin/mcp-tunnels", post(create_tunnel).get(list_tunnels))
        .route(
            "/admin/mcp-tunnels/:id",
            get(get_tunnel).put(update_tunnel).delete(delete_tunnel),
        )
        .route("/admin/mcp-tunnels/:id/rotate", post(rotate_token))
        .route("/admin/mcp-tunnels/:id/reveal", post(reveal_token))
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "admin mcp tunnel operation failed");
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
            "Only organization owners/admins can manage MCP tunnels.",
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

fn validate_url(url: &str) -> Result<(), ApiError> {
    if url.trim().is_empty() || url.len() > 2048 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_endpoint_url",
            "Endpoint URL must be 1-2048 characters.",
        ));
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_endpoint_url",
            "Endpoint URL must start with http:// or https://.",
        ));
    }
    Ok(())
}

fn tunnel_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "organization_id": row.get::<_, String>(1)?,
        "name": row.get::<_, String>(2)?,
        "description": row.get::<_, Option<String>>(3)?,
        "endpoint_url": row.get::<_, String>(4)?,
        "client_cert_pem": row.get::<_, Option<String>>(5)?,
        "oauth_issuer": row.get::<_, Option<String>>(6)?,
        "audience": row.get::<_, Option<String>>(7)?,
        "created_by": row.get::<_, String>(8)?,
        "created_at": row.get::<_, String>(9)?,
        "updated_at": row.get::<_, String>(10)?,
    }))
}

fn find_tunnel(
    conn: &rusqlite::Connection,
    id: &str,
    org: &str,
) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT id, organization_id, name, description, endpoint_url,
                client_cert_pem, oauth_issuer, audience, created_by,
                created_at, updated_at
         FROM admin_mcp_tunnels
         WHERE id = ?1 AND organization_id = ?2",
        params![id, org],
        tunnel_json,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "tunnel_not_found", "No such MCP tunnel."))
}

fn new_token() -> String {
    format!("amt_{}", uuid::Uuid::new_v4().to_string().replace("-", ""))
}

#[derive(Deserialize)]
struct CreateTunnel {
    name: String,
    description: Option<String>,
    endpoint_url: String,
    client_cert_pem: Option<String>,
    oauth_issuer: Option<String>,
    audience: Option<String>,
}

async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateTunnel>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_name(&body.name)?;
    validate_url(&body.endpoint_url)?;

    let id = uuid::Uuid::new_v4().to_string();
    let token = new_token();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO admin_mcp_tunnels
         (id, organization_id, name, description, endpoint_url,
          client_cert_pem, oauth_issuer, audience, tunnel_token,
          created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            org,
            body.name.trim(),
            body.description.as_deref().map(str::trim),
            body.endpoint_url.trim(),
            body.client_cert_pem.as_deref().map(str::trim),
            body.oauth_issuer.as_deref().map(str::trim),
            body.audience.as_deref().map(str::trim),
            token,
            user.user_id,
            now,
            now,
        ],
    )
    .map_err(internal)?;

    let mut tunnel = find_tunnel(&conn, &id, &org)?;
    tunnel.as_object_mut()
        .expect("tunnel is object")
        .insert("tunnel_token".to_string(), json!(token));

    Ok::<Response, ApiError>((StatusCode::CREATED, Json(tunnel)).into_response())
}

async fn list_tunnels(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let mut stmt = conn
        .prepare(
            "SELECT id, organization_id, name, description, endpoint_url,
                    client_cert_pem, oauth_issuer, audience, created_by,
                    created_at, updated_at
             FROM admin_mcp_tunnels
             WHERE organization_id = ?1
             ORDER BY updated_at DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org], tunnel_json)
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn get_tunnel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let tunnel = find_tunnel(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(tunnel))
}

#[derive(Deserialize)]
struct UpdateTunnel {
    name: Option<String>,
    description: Option<String>,
    endpoint_url: Option<String>,
    client_cert_pem: Option<String>,
    oauth_issuer: Option<String>,
    audience: Option<String>,
}

async fn update_tunnel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTunnel>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    // Ensure tunnel exists.
    let _ = find_tunnel(&conn, &id, &org)?;

    if let Some(ref name) = body.name {
        validate_name(name)?;
    }
    if let Some(ref url) = body.endpoint_url {
        validate_url(url)?;
    }

    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE admin_mcp_tunnels SET
            name = COALESCE(?1, name),
            description = COALESCE(?2, description),
            endpoint_url = COALESCE(?3, endpoint_url),
            client_cert_pem = COALESCE(?4, client_cert_pem),
            oauth_issuer = COALESCE(?5, oauth_issuer),
            audience = COALESCE(?6, audience),
            updated_at = ?7
         WHERE id = ?8 AND organization_id = ?9",
        params![
            body.name.as_deref().map(str::trim),
            body.description.as_deref().map(str::trim),
            body.endpoint_url.as_deref().map(str::trim),
            body.client_cert_pem.as_deref().map(str::trim),
            body.oauth_issuer.as_deref().map(str::trim),
            body.audience.as_deref().map(str::trim),
            now,
            id,
            org,
        ],
    )
    .map_err(internal)?;

    let tunnel = find_tunnel(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(tunnel))
}

async fn delete_tunnel(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let rows = conn
        .execute(
            "DELETE FROM admin_mcp_tunnels WHERE id = ?1 AND organization_id = ?2",
            params![id, org],
        )
        .map_err(internal)?;

    if rows == 0 {
        return Err(error(StatusCode::NOT_FOUND, "tunnel_not_found", "No such MCP tunnel."));
    }

    Ok::<Json<Value>, ApiError>(Json(json!({ "deleted": true })))
}

async fn rotate_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    // Ensure tunnel exists.
    let _ = find_tunnel(&conn, &id, &org)?;

    let new = new_token();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE admin_mcp_tunnels SET tunnel_token = ?1, updated_at = ?2
         WHERE id = ?3 AND organization_id = ?4",
        params![new, now, id, org],
    )
    .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({
        "id": id,
        "tunnel_token": new,
        "updated_at": now,
    })))
}

async fn reveal_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let token: String = conn
        .query_row(
            "SELECT tunnel_token FROM admin_mcp_tunnels
             WHERE id = ?1 AND organization_id = ?2",
            params![id, org],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "tunnel_not_found", "No such MCP tunnel."))?;

    Ok::<Json<Value>, ApiError>(Json(json!({
        "id": id,
        "tunnel_token": token,
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
    async fn tunnel_crud_lifecycle() {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let create = json!({
            "name": "prod-mcp",
            "description": "Production MCP tunnel",
            "endpoint_url": "https://mcp.example.com/sse",
            "oauth_issuer": "https://issuer.example",
            "audience": "mcp-audience"
        });

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/mcp-tunnels")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&create))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let id = body["id"].as_str().unwrap().to_string();
        assert_eq!(body["name"], "prod-mcp");
        assert!(body["tunnel_token"].as_str().unwrap().starts_with("amt_"));

        // List
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/mcp-tunnels")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let list = body_json(resp.into_body()).await;
        assert_eq!(list["items"].as_array().unwrap().len(), 1);

        // Get
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/admin/mcp-tunnels/{}", id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Update
        let update = json!({ "name": "prod-mcp-renamed" });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(&format!("/admin/mcp-tunnels/{}", id))
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&update))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["name"], "prod-mcp-renamed");

        // Rotate
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/admin/mcp-tunnels/{}/rotate", id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let rotated = body_json(resp.into_body()).await;
        assert!(rotated["tunnel_token"].as_str().unwrap().starts_with("amt_"));

        // Reveal
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/admin/mcp-tunnels/{}/reveal", id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let revealed = body_json(resp.into_body()).await;
        assert_eq!(revealed["tunnel_token"], rotated["tunnel_token"]);

        // Delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&format!("/admin/mcp-tunnels/{}", id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn tunnel_rejects_non_admin() {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let user = AuthUser {
            user_id: "member-1".to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: Some("org-1".to_string()),
            organization_role: Some("org:member".to_string()),
            organization_slug: None,
        };
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/mcp-tunnels")
                    .extension(user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn tunnel_requires_organization() {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/mcp-tunnels")
                    .extension(test_user("admin-1", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn tunnel_isolation_by_organization() {
        let temp = tempfile::tempdir().unwrap().into_path();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        let create = json!({
            "name": "org1-tunnel",
            "endpoint_url": "https://mcp.example.com/sse"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/mcp-tunnels")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&create))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let id = body["id"].as_str().unwrap().to_string();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(&format!("/admin/mcp-tunnels/{}", id))
                    .extension(test_user("admin-2", Some("org-2")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
