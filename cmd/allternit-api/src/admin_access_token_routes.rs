//! Organization access-token management for CLI/Codex-style surfaces.
//!
//! Endpoints are gated to organization owners/admins and merged into the
//! `/api/v1` chain in main.rs, so paths land at `/api/v1/admin/access-tokens*`.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::{auth::AuthUser, AppState};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/access-tokens", post(create_token).get(list_tokens))
        .route("/admin/access-tokens/:id", get(get_token))
        .route("/admin/access-tokens/:id/revoke", post(revoke_token))
        .route("/admin/access-tokens/:id/rotate", post(rotate_token))
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "access token operation failed");
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
            "Only organization owners/admins can manage access tokens.",
        ));
    }
    Ok(org.to_string())
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    format!("at-{}-{}-{}",
        URL_SAFE_NO_PAD.encode(&bytes[..8]),
        URL_SAFE_NO_PAD.encode(&bytes[8..16]),
        URL_SAFE_NO_PAD.encode(&bytes[16..]),
    )
}

fn hash_token(plain: &str) -> String {
    hex::encode(Sha256::digest(plain.as_bytes()))
}

/// Look up an organization access token by its plain value. Returns an
/// `AuthUser` scoped to the token's organization when the token is active and
/// not expired. Updates `last_used_at` on success.
pub fn authenticate_access_token(db: &crate::db::DbHandle, token: &str) -> Option<AuthUser> {
    if !token.starts_with("at-") {
        return None;
    }
    let hashed = hash_token(token);
    let conn = db.connect().ok()?;
    let row: Option<(String, String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, org_id, name, scopes, expires_at
             FROM organization_access_tokens
             WHERE hashed_token = ?1 AND revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at > ?2)",
            rusqlite::params![hashed, chrono::Utc::now().to_rfc3339()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .ok()?;

    let (id, org_id, _name, scopes_json, _expires_at) = row?;
    let _ = conn.execute(
        "UPDATE organization_access_tokens SET last_used_at = ?1 WHERE id = ?2",
        rusqlite::params![chrono::Utc::now().to_rfc3339(), &id],
    );

    let scopes: Vec<String> = scopes_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    let is_admin = scopes.iter().any(|s| s == "admin" || s == "owner");
    Some(AuthUser {
        user_id: format!("token:{id}"),
        email: None,
        name: Some(format!("Access token {id}")),
        avatar_url: None,
        tenant_id: Some(org_id.clone()),
        organization_id: Some(org_id),
        organization_role: Some(if is_admin { "owner".to_string() } else { "member".to_string() }),
        organization_slug: None,
    })
}

fn token_prefix(plain: &str) -> String {
    plain.chars().take(8).collect()
}

fn parse_scopes(value: Option<String>) -> Result<Option<Vec<String>>, ApiError> {
    match value {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let parsed: Vec<String> = trimmed
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            Ok(Some(parsed))
        }
    }
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Name must be 1-128 characters.",
        ));
    }
    Ok(())
}

fn token_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let scopes_raw: Option<String> = row.get(4)?;
    let scopes: Option<Vec<String>> = scopes_raw
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok());
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "org_id": row.get::<_, String>(1)?,
        "name": row.get::<_, String>(2)?,
        "token_prefix": row.get::<_, String>(3)?,
        "scopes": scopes,
        "expires_at": row.get::<_, Option<String>>(5)?,
        "created_at": row.get::<_, String>(6)?,
        "last_used_at": row.get::<_, Option<String>>(7)?,
        "revoked_at": row.get::<_, Option<String>>(8)?,
    }))
}

fn find_token(
    conn: &rusqlite::Connection,
    id: &str,
    org: &str,
) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT id, org_id, name, token_prefix, scopes, expires_at, created_at, last_used_at, revoked_at
         FROM organization_access_tokens WHERE id = ?1 AND org_id = ?2",
        params![id, org],
        token_json,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "token_not_found", "No such access token."))
}

#[derive(Deserialize)]
struct CreateToken {
    name: String,
    #[serde(default)]
    scopes: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
}

#[derive(Serialize)]
struct CreateTokenResponse {
    id: String,
    org_id: String,
    name: String,
    token: String,
    token_prefix: String,
    scopes: Option<Vec<String>>,
    expires_at: Option<String>,
    created_at: String,
}

async fn create_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateToken>,
) -> Response {
    if let Err(e) = validate_name(&body.name) {
        return e.into_response();
    }
    let scopes = match parse_scopes(body.scopes) {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let id = uuid::Uuid::new_v4().to_string();
        let token = generate_token();
        let prefix = token_prefix(&token);
        let hashed = hash_token(&token);
        let scopes_json = scopes.as_ref().and_then(|s| serde_json::to_string(s).ok());
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO organization_access_tokens
             (id, org_id, name, token_prefix, hashed_token, scopes, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, org, body.name.trim(), prefix, hashed, scopes_json, body.expires_at, now],
        ).map_err(internal)?;
        Ok::<_, ApiError>(CreateTokenResponse {
            id,
            org_id: org,
            name: body.name.trim().to_string(),
            token,
            token_prefix: prefix,
            scopes,
            expires_at: body.expires_at,
            created_at: now,
        })
    }).await;

    match result {
        Ok(Ok(resp)) => (StatusCode::CREATED, Json(json!(resp))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn list_tokens(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, token_prefix, scopes, expires_at, created_at, last_used_at, revoked_at
             FROM organization_access_tokens
             WHERE org_id = ?1
             ORDER BY created_at DESC"
        ).map_err(internal)?;
        let rows: Vec<Value> = stmt
            .query_map(params![org], token_json)
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        Ok::<_, ApiError>(Json(json!({ "items": rows })))
    }).await;

    match result {
        Ok(Ok(resp)) => resp.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn get_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        Ok::<_, ApiError>(Json(find_token(&conn, &id, &org)?))
    }).await;

    match result {
        Ok(Ok(resp)) => resp.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let now = chrono::Utc::now().to_rfc3339();
        let updated = conn.execute(
            "UPDATE organization_access_tokens
             SET revoked_at = ?1
             WHERE id = ?2 AND org_id = ?3 AND revoked_at IS NULL",
            params![now, id, org],
        ).map_err(internal)?;
        if updated == 0 {
            // Could already be revoked or not found; fetch to disambiguate.
            find_token(&conn, &id, &org)?;
        }
        Ok::<_, ApiError>(Json(json!({ "status": "revoked", "revoked_at": now })))
    }).await;

    match result {
        Ok(Ok(resp)) => resp.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn rotate_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let new_token = generate_token();
        let prefix = token_prefix(&new_token);
        let hashed = hash_token(&new_token);
        let now = chrono::Utc::now().to_rfc3339();
        let updated = conn.execute(
            "UPDATE organization_access_tokens
             SET hashed_token = ?1, token_prefix = ?2, revoked_at = NULL, created_at = ?3
             WHERE id = ?4 AND org_id = ?5",
            params![hashed, prefix, now, id, org],
        ).map_err(internal)?;
        if updated == 0 {
            return Err(error(StatusCode::NOT_FOUND, "token_not_found", "No such access token."));
        }
        let row = find_token(&conn, &id, &org)?;
        Ok::<_, ApiError>(Json(json!({
            "token": new_token,
            "token_prefix": prefix,
            "record": row,
        })))
    }).await;

    match result {
        Ok(Ok(resp)) => resp.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
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
            rails,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
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
    async fn access_token_lifecycle() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state);

        // Create
        let create_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/access-tokens")
                    .extension(test_user("admin-1", Some("org-1")))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "name": "Codex CLI", "scopes": "cli:read,cli:write" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_resp.status(), StatusCode::CREATED);
        let create_body = body_json(create_resp.into_body()).await;
        let token_id = create_body["id"].as_str().unwrap().to_string();
        let plain_token = create_body["token"].as_str().unwrap();
        assert!(plain_token.starts_with("at-"));
        assert_eq!(create_body["token_prefix"], plain_token.chars().take(8).collect::<String>());
        let scopes = create_body["scopes"].as_array().unwrap();
        assert_eq!(scopes.len(), 2);

        // List
        let list_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/access-tokens")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_resp.status(), StatusCode::OK);
        let list_body = body_json(list_resp.into_body()).await;
        assert_eq!(list_body["items"].as_array().unwrap().len(), 1);

        // Rotate
        let rotate_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/admin/access-tokens/{}/rotate", token_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rotate_resp.status(), StatusCode::OK);
        let rotate_body = body_json(rotate_resp.into_body()).await;
        let new_plain = rotate_body["token"].as_str().unwrap();
        assert_ne!(new_plain, plain_token);

        // Revoke
        let revoke_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/admin/access-tokens/{}/revoke", token_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoke_resp.status(), StatusCode::OK);
        let get_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/admin/access-tokens/{}", token_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let get_body = body_json(get_resp.into_body()).await;
        assert!(get_body["revoked_at"].is_string());
    }

    #[tokio::test]
    async fn authenticate_access_token_allows_bearer_use() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = router().with_state(state.clone());

        let create_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/access-tokens")
                    .extension(test_user("admin-1", Some("org-1")))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "name": "CLI", "scopes": "admin" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let create_body = body_json(create_resp.into_body()).await;
        let plain = create_body["token"].as_str().unwrap().to_string();

        let user = tokio::task::spawn_blocking(move || {
            authenticate_access_token(&state.db, &plain)
        })
        .await
        .unwrap();
        assert!(user.is_some());
        let user = user.unwrap();
        assert_eq!(user.organization_id.as_deref(), Some("org-1"));
        assert_eq!(user.organization_role.as_deref(), Some("owner"));
    }
}
