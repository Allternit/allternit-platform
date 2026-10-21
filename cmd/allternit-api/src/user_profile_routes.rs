//! User profiles for agents acting on behalf of humans (`/beta/user-profiles`).
//!
//! A profile binds a human (email + display name) to an agent within an
//! organization. Enrolling requires a signed, time-bound token delivered via
//! `POST /beta/user-profiles/:id/enrollment-url` and consumed at
//! `POST /beta/enroll`.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::{auth::AuthUser, error::ApiError, AppState};

fn empty_object() -> Value {
    json!({})
}

/// Default enrollment token lifetime.
const ENROLLMENT_TOKEN_TTL_HOURS: i64 = 48;

pub fn user_profile_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/beta/user-profiles", get(list_profiles).post(create_profile))
        .route(
            "/beta/user-profiles/:id",
            get(get_profile)
                .patch(update_profile)
                .delete(delete_profile),
        )
        .route(
            "/beta/user-profiles/:id/enrollment-url",
            post(create_enrollment_url),
        )
}

/// Public enrollment acceptor. The token itself is the credential, so this
/// route is intentionally mounted outside the auth middleware.
pub fn enrollment_router() -> Router<Arc<AppState>> {
    Router::new().route("/beta/enroll", post(accept_enrollment))
}

#[derive(Debug, Deserialize)]
struct CreateProfileBody {
    agent_id: String,
    email: Option<String>,
    display_name: Option<String>,
    #[serde(default = "empty_object")]
    metadata: Value,
}

#[derive(Debug, Deserialize)]
struct UpdateProfileBody {
    email: Option<String>,
    display_name: Option<String>,
    #[serde(default)]
    enrollment_status: Option<String>,
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ProfileRow {
    id: String,
    org_id: String,
    agent_id: String,
    email: Option<String>,
    display_name: Option<String>,
    consent_given_at: Option<String>,
    enrollment_status: String,
    metadata: Value,
    created_at: String,
    updated_at: String,
}

const PROFILE_SELECT: &str = "SELECT id, org_id, agent_id, email, display_name, consent_given_at,
    enrollment_status, metadata, created_at, updated_at FROM user_profiles";

fn read_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileRow> {
    let metadata: String = row.get(7)?;
    Ok(ProfileRow {
        id: row.get(0)?,
        org_id: row.get(1)?,
        agent_id: row.get(2)?,
        email: row.get(3)?,
        display_name: row.get(4)?,
        consent_given_at: row.get(5)?,
        enrollment_status: row.get(6)?,
        metadata: serde_json::from_str(&metadata).unwrap_or_else(|_| json!({})),
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn require_org(user: &AuthUser) -> Result<String, ApiError> {
    user.organization_id
        .clone()
        .ok_or_else(|| ApiError::BadRequest("organization scope is required".into()))
}

async fn create_profile(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateProfileBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let org_id = require_org(&user)?;
    let agent_id = body.agent_id.trim();
    if agent_id.is_empty() {
        return Err(ApiError::BadRequest("agent_id is required".into()));
    }
    if !body.metadata.is_object() {
        return Err(ApiError::BadRequest("metadata must be an object".into()));
    }
    let db = state.db.clone();
    let id = uuid::Uuid::new_v4().to_string();
    let result_id = id.clone();
    let agent_id = agent_id.to_string();
    let profile = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO user_profiles
             (id, org_id, agent_id, email, display_name, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                org_id,
                agent_id,
                body.email,
                body.display_name,
                body.metadata.to_string()
            ],
        )?;
        conn.query_row(
            &format!("{PROFILE_SELECT} WHERE id = ?1"),
            params![id],
            read_profile,
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e: rusqlite::Error| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            ApiError::BadRequest("profile already exists".into())
        }
        other => ApiError::DbError(other.to_string()),
    })?;
    Ok((StatusCode::CREATED, Json(json!({ "profile": profile, "id": result_id }))))
}

async fn list_profiles(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let org_id = require_org(&user)?;
    let db = state.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(&format!(
            "{PROFILE_SELECT} WHERE org_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt
            .query_map(params![org_id], read_profile)?
            .collect::<Result<Vec<ProfileRow>, _>>()?;
        Ok::<Vec<ProfileRow>, rusqlite::Error>(rows)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(Json(json!({ "profiles": rows })))
}

async fn get_profile(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let org_id = require_org(&user)?;
    let db = state.db.clone();
    let profile = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.query_row(
            &format!("{PROFILE_SELECT} WHERE id = ?1 AND org_id = ?2"),
            params![id, org_id],
            read_profile,
        )
        .optional()
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??
    .ok_or_else(|| ApiError::NotFound("profile not found".into()))?;
    Ok(Json(json!({ "profile": profile })))
}

async fn update_profile(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProfileBody>,
) -> Result<Json<Value>, ApiError> {
    let org_id = require_org(&user)?;
    if body.email.is_none()
        && body.display_name.is_none()
        && body.enrollment_status.is_none()
        && body.metadata.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".into()));
    }
    if body
        .metadata
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Err(ApiError::BadRequest("metadata must be an object".into()));
    }
    if body
        .enrollment_status
        .as_ref()
        .is_some_and(|s| !matches!(s.as_str(), "pending" | "enrolled" | "revoked"))
    {
        return Err(ApiError::BadRequest(
            "enrollment_status must be pending, enrolled, or revoked".into(),
        ));
    }
    let db = state.db.clone();
    let lookup_id = id.clone();
    let affected = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE user_profiles SET
                email = COALESCE(?1, email),
                display_name = COALESCE(?2, display_name),
                enrollment_status = COALESCE(?3, enrollment_status),
                metadata = COALESCE(?4, metadata),
                updated_at = CURRENT_TIMESTAMP
             WHERE id = ?5 AND org_id = ?6",
            params![
                body.email,
                body.display_name,
                body.enrollment_status,
                body.metadata.map(|v| v.to_string()),
                id,
                org_id,
            ],
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    if affected == 0 {
        return Err(ApiError::NotFound("profile not found".into()));
    }
    get_profile(State(state), Extension(user), Path(lookup_id)).await
}

async fn delete_profile(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let org_id = require_org(&user)?;
    let db = state.db.clone();
    let deleted = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "DELETE FROM user_profiles WHERE id = ?1 AND org_id = ?2",
            params![id, org_id],
        )
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    if deleted == 0 {
        return Err(ApiError::NotFound("profile not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn create_enrollment_url(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let org_id = require_org(&user)?;
    let secret = state
        .config
        .enrollment_secret()
        .ok_or_else(|| ApiError::Internal("enrollment secret not configured".into()))?;

    let db = state.db.clone();
    let profile = tokio::task::spawn_blocking({
        let id = id.clone();
        let org_id = org_id.clone();
        move || {
            let conn = db.connect()?;
            conn.query_row(
                &format!("{PROFILE_SELECT} WHERE id = ?1 AND org_id = ?2"),
                params![id, org_id],
                read_profile,
            )
            .optional()
        }
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??
    .ok_or_else(|| ApiError::NotFound("profile not found".into()))?;

    if profile.enrollment_status == "enrolled" {
        return Err(ApiError::BadRequest("profile is already enrolled".into()));
    }

    let expires_at = chrono::Utc::now() + chrono::Duration::hours(ENROLLMENT_TOKEN_TTL_HOURS);
    let token = sign_enrollment_token(&secret, &id, expires_at.timestamp() as u64);
    let token_hash = token_hash(&token);
    let token_id = uuid::Uuid::new_v4().to_string();
    let expires_at_str = expires_at.to_rfc3339();

    let db = state.db.clone();
    let expires_at_db = expires_at_str.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO enrollment_tokens (id, profile_id, token_hash, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![token_id, id, token_hash, expires_at_db],
        )?;
        Ok::<(), rusqlite::Error>(())
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| ApiError::DbError(e.to_string()))?;

    let gateway_url = state.config.gateway_url();
    let enrollment_url = format!("{}/beta/enroll?token={}", gateway_url, token);
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "enrollment_url": enrollment_url,
            "expires_at": expires_at_str,
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct AcceptEnrollmentBody {
    token: String,
}

async fn accept_enrollment(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AcceptEnrollmentBody>,
) -> Result<Json<Value>, ApiError> {
    let secret = state
        .config
        .enrollment_secret()
        .ok_or_else(|| ApiError::Internal("enrollment secret not configured".into()))?;

    let (profile_id, expires_at) =
        verify_enrollment_token(&secret, &body.token).ok_or_else(|| {
            ApiError::BadRequest("invalid or expired enrollment token".into())
        })?;

    if chrono::Utc::now().timestamp() > expires_at as i64 {
        return Err(ApiError::BadRequest("enrollment token has expired".into()));
    }

    let token_hash = token_hash(&body.token);
    let db = state.db.clone();
    let profile_id = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;
        let used: Option<Option<String>> = tx
            .query_row(
                "SELECT used_at FROM enrollment_tokens WHERE token_hash = ?1",
                params![token_hash],
                |row| row.get(0),
            )
            .optional()?;
        if used.as_ref().and_then(|u| u.as_ref()).is_some() {
            return Err(rusqlite::Error::InvalidParameterName(
                "enrollment token already used".into(),
            ));
        }
        if used.is_none() {
            return Err(rusqlite::Error::InvalidParameterName(
                "enrollment token not found".into(),
            ));
        }
        tx.execute(
            "UPDATE enrollment_tokens SET used_at = CURRENT_TIMESTAMP WHERE token_hash = ?1",
            params![token_hash],
        )?;
        let affected = tx.execute(
            "UPDATE user_profiles SET
                consent_given_at = CURRENT_TIMESTAMP,
                enrollment_status = 'enrolled',
                updated_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            params![profile_id],
        )?;
        if affected == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        tx.commit()?;
        Ok::<String, rusqlite::Error>(profile_id)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .map_err(|e| match e {
        rusqlite::Error::InvalidParameterName(msg) if msg == "enrollment token already used" => {
            ApiError::BadRequest("enrollment token already used".into())
        }
        rusqlite::Error::InvalidParameterName(msg) if msg == "enrollment token not found" => {
            ApiError::BadRequest("enrollment token not found".into())
        }
        rusqlite::Error::QueryReturnedNoRows => ApiError::NotFound("profile not found".into()),
        other => ApiError::DbError(other.to_string()),
    })?;

    Ok(Json(json!({
        "enrolled": true,
        "profile_id": profile_id,
    })))
}

// ─── Token helpers ──────────────────────────────────────────────────────────

fn sign_enrollment_token(secret: &str, profile_id: &str, expires_at: u64) -> String {
    let payload = format!("{}:{}", profile_id, expires_at);
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take a key of any size");
    mac.update(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let combined = format!("{}:{}", payload, signature);
    URL_SAFE_NO_PAD.encode(combined)
}

fn verify_enrollment_token(secret: &str, token: &str) -> Option<(String, u64)> {
    let combined = URL_SAFE_NO_PAD.decode(token).ok()?;
    let combined = String::from_utf8(combined).ok()?;
    let mut parts = combined.splitn(3, ':');
    let profile_id = parts.next()?.to_string();
    let expires_at: u64 = parts.next()?.parse().ok()?;
    let _signature = parts.next()?;
    let expected = sign_enrollment_token(secret, &profile_id, expires_at);
    if !constant_time_eq(&expected, token) {
        return None;
    }
    Some((profile_id, expires_at))
}

fn token_hash(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::collections::HashMap;
    use std::path::Path as FsPath;
    use std::sync::Once;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    fn test_user(id: &str, org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: Some(format!("{}@example.test", id)),
            name: None,
            avatar_url: None,
            tenant_id: org_id.map(str::to_string),
            organization_id: org_id.map(str::to_string),
            organization_role: None,
            organization_slug: org_id.map(str::to_string),
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "allternit-user-profiles-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    static TEST_KEY_INIT: Once = Once::new();

    fn init_test_enrollment_secret() {
        TEST_KEY_INIT.call_once(|| {
            std::env::set_var(
                "ALLTERNIT_ENROLLMENT_SECRET",
                "test-enrollment-secret-32bytes!",
            );
        });
    }

    async fn test_app_state(temp: &FsPath) -> Arc<AppState> {
        init_test_enrollment_secret();
        let mut config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        config.user.gateway_url = Some("http://localhost:8013".to_string());
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
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
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
    async fn profile_crud_lifecycle() {
        let temp = temp_dir("crud");
        let state = test_app_state(&temp).await;
        let app = user_profile_router().with_state(state);

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/user-profiles")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org-1")))
                    .body(json_body(&json!({
                        "agent_id": "agent-1",
                        "email": "human@example.test",
                        "display_name": "Human User",
                        "metadata": { "source": "test" }
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let profile_id = body["profile"]["id"].as_str().unwrap().to_string();
        assert_eq!(body["profile"]["agent_id"], "agent-1");
        assert_eq!(body["profile"]["email"], "human@example.test");
        assert_eq!(body["profile"]["enrollment_status"], "pending");

        // List
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/user-profiles")
                    .extension(test_user("user-a", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["profiles"].as_array().unwrap().len(), 1);

        // Get
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/user-profiles/{}", profile_id))
                    .extension(test_user("user-a", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["profile"]["display_name"], "Human User");

        // Update
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/beta/user-profiles/{}", profile_id))
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org-1")))
                    .body(json_body(&json!({
                        "display_name": "Updated Name",
                        "metadata": { "source": "test", "updated": true }
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["profile"]["display_name"], "Updated Name");
        assert_eq!(body["profile"]["metadata"]["updated"], true);

        // Delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/beta/user-profiles/{}", profile_id))
                    .extension(test_user("user-a", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Get after delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/user-profiles/{}", profile_id))
                    .extension(test_user("user-a", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn profiles_are_isolated_by_organization() {
        let temp = temp_dir("isolation");
        let state = test_app_state(&temp).await;
        let app = user_profile_router().with_state(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/user-profiles")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org-1")))
                    .body(json_body(&json!({ "agent_id": "agent-1" })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let profile_id = body["profile"]["id"].as_str().unwrap().to_string();

        // Different org cannot access.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/user-profiles/{}", profile_id))
                    .extension(test_user("user-b", Some("org-2")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Missing org is rejected.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/beta/user-profiles")
                    .extension(test_user("user-c", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn enrollment_url_generates_and_records_consent() {
        let temp = temp_dir("enroll");
        let state = test_app_state(&temp).await;
        let profile_app = user_profile_router().with_state(state.clone());
        let enroll_app = enrollment_router().with_state(state);

        let resp = profile_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/user-profiles")
                    .header("content-type", "application/json")
                    .extension(test_user("user-a", Some("org-1")))
                    .body(json_body(&json!({ "agent_id": "agent-1" })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let profile_id = body["profile"]["id"].as_str().unwrap().to_string();

        // Generate enrollment URL.
        let resp = profile_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/beta/user-profiles/{}/enrollment-url", profile_id))
                    .extension(test_user("user-a", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let url = body["enrollment_url"].as_str().unwrap().to_string();
        let token = url.split("token=").nth(1).unwrap().to_string();

        // Accept enrollment.
        let resp = enroll_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/enroll")
                    .header("content-type", "application/json")
                    .body(json_body(&json!({ "token": token })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["enrolled"], true);
        assert_eq!(body["profile_id"], profile_id);

        // Profile is now enrolled.
        let resp = profile_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/beta/user-profiles/{}", profile_id))
                    .extension(test_user("user-a", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["profile"]["enrollment_status"], "enrolled");
        assert!(body["profile"]["consent_given_at"].is_string());

        // Reusing the token fails.
        let resp = enroll_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/enroll")
                    .header("content-type", "application/json")
                    .body(json_body(&json!({ "token": token })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn enrollment_rejects_tampered_and_expired_tokens() {
        let temp = temp_dir("enroll-reject");
        let state = test_app_state(&temp).await;
        let app = enrollment_router().with_state(state);

        // Tampered token.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/enroll")
                    .header("content-type", "application/json")
                    .body(json_body(&json!({ "token": "not-a-real-token" })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Expired token.
        let secret = std::env::var("ALLTERNIT_ENROLLMENT_SECRET").unwrap();
        let expired_token = sign_enrollment_token(&secret, "profile-123", 1);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/enroll")
                    .header("content-type", "application/json")
                    .body(json_body(&json!({ "token": expired_token })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
