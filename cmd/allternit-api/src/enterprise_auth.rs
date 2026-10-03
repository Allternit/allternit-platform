//! Enterprise API keys and short-lived CLI access tokens.
//!
//! Management endpoints live behind the normal Clerk middleware. Bearer
//! authentication is consumed by `auth_middleware`; only SHA-256 digests are
//! persisted and plaintext credentials are returned once.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::{auth::AuthUser, AppState};

const ADMIN_PREFIX: &str = "allternit_admin_";
const ACCESS_PREFIX: &str = "allternit_access_";

#[derive(Clone, Debug)]
pub struct CredentialContext {
    pub credential_id: String,
    pub kind: &'static str,
    pub scopes: Vec<String>,
}

impl CredentialContext {
    pub fn allows(&self, required: &str) -> bool {
        self.scopes
            .iter()
            .any(|scope| scope == "*" || scope == required)
    }

    pub fn allows_request(&self, method: &axum::http::Method, path: &str) -> bool {
        // A vendor-bot key (`vendor-bot:<id>`) opens that one bot's connector and nothing else.
        if let Some(rest) = path.strip_prefix("/mcp/bots/").or_else(|| path.strip_prefix("/api/v1/mcp/bots/")) {
            let bot = rest.split('/').next().unwrap_or_default();
            return method == axum::http::Method::POST && !bot.is_empty() && self.allows(&format!("vendor-bot:{bot}"));
        }
        let action = if method == axum::http::Method::GET || method == axum::http::Method::HEAD {
            "read"
        } else {
            "write"
        };
        let resource = if path.contains("/vault/") || path.contains("/vaults") {
            "vault"
        } else {
            "api"
        };
        self.allows(&format!("{resource}:{action}"))
    }
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn generate_token(prefix: &str) -> String {
    let mut bytes = [0u8; 24];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..8]);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Mint an access token limited to `scopes`, valid for `days`. Returns `(id, plaintext)`; only
/// the hash is stored.
pub fn mint_scoped_key(db: &crate::db::DbHandle, user: &AuthUser, name: &str, scopes: &[String], days: i64) -> Result<(String, String), String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let plaintext = generate_token(ACCESS_PREFIX);
    let prefix: String = plaintext.chars().take(23).collect();
    let expires = (chrono::Utc::now() + chrono::Duration::days(days)).format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        "INSERT INTO access_tokens (id, user_id, organization_id, token_hash, token_prefix, name, scopes, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, user.user_id, user.organization_id, hash_token(&plaintext), prefix, name, serde_json::to_string(scopes).unwrap(), expires],
    )
    .map_err(|e| e.to_string())?;
    Ok((id, plaintext))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/auth/admin-api-keys",
            post(create_admin_key).get(list_admin_keys),
        )
        .route("/auth/admin-api-keys/:id", delete(revoke_admin_key))
        .route(
            "/auth/access-tokens",
            post(create_access_token).get(list_access_tokens),
        )
        .route("/auth/access-tokens/:id/rotate", post(rotate_access_token))
        .route("/auth/access-tokens/:id", delete(revoke_access_token))
        .route(
            "/auth/workload-identity/providers",
            post(create_wif_provider).get(list_wif_providers),
        )
        .route(
            "/auth/workload-identity/providers/:id",
            get(get_wif_provider)
                .patch(update_wif_provider)
                .delete(delete_wif_provider),
        )
        .route(
            "/auth/workload-identity/token",
            post(exchange_workload_identity_token),
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
    tracing::warn!(error = %err, "enterprise auth operation failed");
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
            "Only organization owners/admins can manage admin API keys.",
        ));
    }
    Ok(org.to_string())
}

#[derive(Deserialize)]
struct CreateCredential {
    name: String,
    #[serde(default)]
    scopes: Vec<String>,
    expires_at: Option<String>,
}

fn validate(payload: &CreateCredential, require_expiry: bool) -> Result<(), ApiError> {
    if payload.name.trim().is_empty() || payload.name.len() > 128 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Name must be 1-128 characters.",
        ));
    }
    if payload.scopes.is_empty() || payload.scopes.iter().any(|s| s.trim().is_empty()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_scopes",
            "At least one non-empty scope is required.",
        ));
    }
    if require_expiry && payload.expires_at.is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "expiration_required",
            "Access tokens require expires_at.",
        ));
    }
    if let Some(expiry) = &payload.expires_at {
        let parsed = chrono::DateTime::parse_from_rfc3339(expiry).map_err(|_| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid_expiration",
                "expires_at must be RFC 3339.",
            )
        })?;
        if parsed <= chrono::Utc::now() {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_expiration",
                "expires_at must be in the future.",
            ));
        }
    }
    Ok(())
}

async fn create_admin_key(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateCredential>,
) -> Response {
    if let Err(e) = validate(&body, false) {
        return e.into_response();
    }
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let id = uuid::Uuid::new_v4().to_string();
        let plaintext = generate_token(ADMIN_PREFIX);
        let prefix: String = plaintext.chars().take(22).collect();
        conn.execute(
            "INSERT INTO admin_api_keys (id, organization_id, created_by, key_hash, key_prefix, name, scopes, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, org, user_id, hash_token(&plaintext), prefix, body.name, serde_json::to_string(&body.scopes).unwrap(), body.expires_at],
        ).map_err(internal)?;
        Ok::<_, ApiError>(json!({"id": id, "key": plaintext, "key_prefix": prefix, "scopes": body.scopes, "expires_at": body.expires_at}))
    }).await;
    match result {
        Ok(Ok(v)) => (StatusCode::CREATED, Json(v)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn list_admin_keys(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let mut stmt = conn.prepare("SELECT id, key_prefix, name, scopes, revoked, expires_at, last_used_at, created_at FROM admin_api_keys WHERE organization_id = ?1 ORDER BY created_at DESC").map_err(internal)?;
        let rows = stmt.query_map([org], |r| Ok(json!({"id": r.get::<_, String>(0)?, "key_prefix": r.get::<_, String>(1)?, "name": r.get::<_, String>(2)?, "scopes": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(json!([])), "revoked": r.get::<_, i64>(4)? != 0, "expires_at": r.get::<_, Option<String>>(5)?, "last_used_at": r.get::<_, Option<String>>(6)?, "created_at": r.get::<_, String>(7)?}))).map_err(internal)?.collect::<rusqlite::Result<Vec<_>>>().map_err(internal)?;
        Ok::<_, ApiError>(rows)
    }).await;
    match result {
        Ok(Ok(v)) => Json(json!({"keys": v})).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn revoke_admin_key(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let changed = conn.execute("UPDATE admin_api_keys SET revoked = 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND organization_id = ?2", params![id, org]).map_err(internal)?;
        if changed == 0 { return Err(error(StatusCode::NOT_FOUND, "key_not_found", "No such admin API key.")); }
        Ok::<_, ApiError>(())
    }).await;
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn create_access_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateCredential>,
) -> Response {
    create_access_token_row(state, user, body, None).await
}

async fn create_access_token_row(
    state: Arc<AppState>,
    user: AuthUser,
    body: CreateCredential,
    rotated_from: Option<String>,
) -> Response {
    if let Err(e) = validate(&body, true) {
        return e.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = state.db.connect().map_err(internal)?;
        let tx = conn.transaction().map_err(internal)?;
        if let Some(old) = &rotated_from {
            let changed = tx.execute("UPDATE access_tokens SET revoked = 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2 AND revoked = 0", params![old, user.user_id]).map_err(internal)?;
            if changed == 0 { return Err(error(StatusCode::NOT_FOUND, "token_not_found", "No live access token to rotate.")); }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let plaintext = generate_token(ACCESS_PREFIX);
        let prefix: String = plaintext.chars().take(23).collect();
        tx.execute("INSERT INTO access_tokens (id, user_id, organization_id, token_hash, token_prefix, name, scopes, expires_at, rotated_from_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", params![id, user.user_id, user.organization_id, hash_token(&plaintext), prefix, body.name, serde_json::to_string(&body.scopes).unwrap(), body.expires_at, rotated_from]).map_err(internal)?;
        tx.commit().map_err(internal)?;
        Ok::<_, ApiError>(json!({"id": id, "token": plaintext, "token_prefix": prefix, "scopes": body.scopes, "expires_at": body.expires_at}))
    }).await;
    match result {
        Ok(Ok(v)) => (StatusCode::CREATED, Json(v)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn rotate_access_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateCredential>,
) -> Response {
    create_access_token_row(state, user, body, Some(id)).await
}

async fn list_access_tokens(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, ApiError> {
        let conn = state.db.connect().map_err(internal)?;
        let mut stmt = conn.prepare("SELECT id, token_prefix, name, scopes, revoked, expires_at, last_used_at, rotated_from_id, created_at FROM access_tokens WHERE user_id = ?1 ORDER BY created_at DESC").map_err(internal)?;
        let rows = stmt.query_map([user.user_id], |r| Ok(json!({"id": r.get::<_, String>(0)?, "token_prefix": r.get::<_, String>(1)?, "name": r.get::<_, String>(2)?, "scopes": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(json!([])), "revoked": r.get::<_, i64>(4)? != 0, "expires_at": r.get::<_, String>(5)?, "last_used_at": r.get::<_, Option<String>>(6)?, "rotated_from_id": r.get::<_, Option<String>>(7)?, "created_at": r.get::<_, String>(8)?}))).map_err(internal)?.collect::<rusqlite::Result<Vec<_>>>().map_err(internal)?;
        Ok(rows)
    }).await;
    match result {
        Ok(Ok(v)) => Json(json!({"tokens": v})).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn revoke_access_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || state.db.connect().and_then(|conn| conn.execute("UPDATE access_tokens SET revoked = 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2", params![id, user.user_id]))).await;
    match result {
        Ok(Ok(0)) => error(
            StatusCode::NOT_FOUND,
            "token_not_found",
            "No such access token.",
        )
        .into_response(),
        Ok(Ok(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => internal(e).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// Resolve enterprise bearer credentials for the shared auth middleware.
pub fn authenticate_bearer(
    db: &crate::db::DbHandle,
    token: &str,
) -> rusqlite::Result<Option<(AuthUser, CredentialContext)>> {
    let (table, hash_column, id_column, org_column, kind) = if token.starts_with(ADMIN_PREFIX) {
        (
            "admin_api_keys",
            "key_hash",
            "created_by",
            "organization_id",
            "admin_api_key",
        )
    } else if token.starts_with(ACCESS_PREFIX) {
        (
            "access_tokens",
            "token_hash",
            "user_id",
            "organization_id",
            "access_token",
        )
    } else {
        return Ok(None);
    };
    let conn = db.connect()?;
    let admin_guard = if kind == "admin_api_key" {
        "AND EXISTS (SELECT 1 FROM organization_members m WHERE m.organization_id = c.organization_id AND m.user_id = c.created_by AND m.role IN ('owner', 'admin'))"
    } else {
        ""
    };
    let sql = format!("SELECT c.id, c.{id_column}, c.{org_column}, c.scopes, u.email, u.name, (SELECT m.role FROM organization_members m WHERE m.organization_id = c.{org_column} AND m.user_id = c.{id_column}) FROM {table} c JOIN users u ON u.id = c.{id_column} WHERE c.{hash_column} = ?1 AND c.revoked = 0 AND (c.expires_at IS NULL OR c.expires_at > CURRENT_TIMESTAMP) {admin_guard}");
    let found = conn
        .query_row(&sql, [hash_token(token)], |row| {
            let org: Option<String> = row.get(2)?;
            let scopes: String = row.get(3)?;
            Ok((
                AuthUser {
                    user_id: row.get(1)?,
                    email: row.get(4)?,
                    name: row.get(5)?,
                    avatar_url: None,
                    tenant_id: org.clone(),
                    organization_id: org,
                    organization_role: row.get(6)?,
                    organization_slug: None,
                },
                CredentialContext {
                    credential_id: row.get(0)?,
                    kind,
                    scopes: serde_json::from_str(&scopes).unwrap_or_default(),
                },
            ))
        })
        .optional()?;
    if let Some((_, ctx)) = &found {
        let _ = conn.execute(
            &format!("UPDATE {table} SET last_used_at = CURRENT_TIMESTAMP WHERE id = ?1"),
            [&ctx.credential_id],
        );
    }
    Ok(found)
}

// ─── Workload Identity Federation ───────────────────────────────────────────

const WIF_PROVIDER_TYPES: &[&str] = &[
    "aws",
    "azure",
    "gcp",
    "github",
    "kubernetes",
    "okta",
    "spiffe",
];

fn valid_wif_provider_type(kind: &str) -> bool {
    WIF_PROVIDER_TYPES.contains(&kind)
}

#[derive(Deserialize)]
struct CreateWifProvider {
    name: String,
    provider_type: String,
    #[serde(default)]
    config: Value,
}

#[derive(Deserialize)]
struct UpdateWifProvider {
    name: Option<String>,
    #[serde(default)]
    config: Option<Value>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Deserialize)]
struct ExchangeWorkloadToken {
    provider_id: String,
    #[serde(default)]
    assertion: Option<String>,
    #[serde(default)]
    subject: Option<String>,
}

fn wif_provider_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let config_raw: String = row.get(3)?;
    let config: Value = serde_json::from_str(&config_raw).unwrap_or(json!({}));
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "org_id": row.get::<_, String>(1)?,
        "name": row.get::<_, String>(2)?,
        "config": config,
        "provider_type": row.get::<_, String>(4)?,
        "enabled": row.get::<_, i64>(5)? != 0,
        "created_at": row.get::<_, String>(6)?,
        "updated_at": row.get::<_, String>(7)?,
    }))
}

async fn create_wif_provider(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateWifProvider>,
) -> Response {
    if body.name.trim().is_empty() || body.name.len() > 128 {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Name must be 1-128 characters.",
        )
        .into_response();
    }
    if !valid_wif_provider_type(&body.provider_type) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_provider_type",
            "provider_type must be one of aws, azure, gcp, github, kubernetes, okta, spiffe.",
        )
        .into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let id = uuid::Uuid::new_v4().to_string();
        let config_json = serde_json::to_string(&body.config).unwrap_or_else(|_| "{}".into());
        conn.execute(
            "INSERT INTO workload_identity_providers (id, org_id, name, provider_type, config) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, org, body.name.trim(), body.provider_type, config_json],
        ).map_err(internal)?;
        conn.query_row(
            "SELECT id, org_id, name, config, provider_type, enabled, created_at, updated_at FROM workload_identity_providers WHERE id = ?1",
            [&id],
            wif_provider_json,
        )
        .map_err(internal)
    }).await;
    match result {
        Ok(Ok(v)) => (StatusCode::CREATED, Json(v)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn list_wif_providers(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<Value>, ApiError> {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let mut stmt = conn.prepare(
            "SELECT id, org_id, name, config, provider_type, enabled, created_at, updated_at FROM workload_identity_providers WHERE org_id = ?1 ORDER BY created_at DESC"
        ).map_err(internal)?;
        let rows = stmt
            .query_map([&org], wif_provider_json)
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(internal)?;
        Ok(rows)
    }).await;
    match result {
        Ok(Ok(v)) => Json(json!({"providers": v})).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn get_wif_provider(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        conn.query_row(
            "SELECT id, org_id, name, config, provider_type, enabled, created_at, updated_at FROM workload_identity_providers WHERE id = ?1 AND org_id = ?2",
            params![id, org],
            wif_provider_json,
        )
        .optional()
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "provider_not_found", "No such workload identity provider."))
    }).await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn update_wif_provider(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<UpdateWifProvider>,
) -> Response {
    if let Some(name) = &body.name {
        if name.trim().is_empty() || name.len() > 128 {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_name",
                "Name must be 1-128 characters.",
            )
            .into_response();
        }
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let existing = conn
            .query_row(
                "SELECT id FROM workload_identity_providers WHERE id = ?1 AND org_id = ?2",
                params![id, org],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(internal)?;
        if existing.is_none() {
            return Err(error(StatusCode::NOT_FOUND, "provider_not_found", "No such workload identity provider."));
        }
        let config_json = body.config.as_ref().and_then(|c| serde_json::to_string(c).ok());
        let enabled = body.enabled.map(|e| if e { 1 } else { 0 });
        conn.execute(
            "UPDATE workload_identity_providers
             SET name = COALESCE(?1, name),
                 config = COALESCE(?2, config),
                 enabled = COALESCE(?3, enabled),
                 updated_at = CURRENT_TIMESTAMP
             WHERE id = ?4 AND org_id = ?5",
            params![body.name.as_deref().map(|n| n.trim()), config_json, enabled, id, org],
        ).map_err(internal)?;
        conn.query_row(
            "SELECT id, org_id, name, config, provider_type, enabled, created_at, updated_at FROM workload_identity_providers WHERE id = ?1",
            [&id],
            wif_provider_json,
        )
        .map_err(internal)
    }).await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn delete_wif_provider(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let changed = conn.execute(
            "DELETE FROM workload_identity_providers WHERE id = ?1 AND org_id = ?2",
            params![id, org],
        ).map_err(internal)?;
        if changed == 0 {
            return Err(error(
                StatusCode::NOT_FOUND,
                "provider_not_found",
                "No such workload identity provider.",
            ));
        }
        Ok(())
    }).await;
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn exchange_workload_identity_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ExchangeWorkloadToken>,
) -> Response {
    if body.provider_id.trim().is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_provider_id",
            "provider_id is required.",
        )
        .into_response();
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Value, ApiError> {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let provider = conn
            .query_row(
                "SELECT id, org_id, name, config, provider_type, enabled, created_at, updated_at FROM workload_identity_providers WHERE id = ?1 AND org_id = ?2 AND enabled = 1",
                params![body.provider_id, org],
                wif_provider_json,
            )
            .optional()
            .map_err(internal)?
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "provider_not_found", "No enabled workload identity provider found."))?;

        // Phase 1 scaffold: validate that an assertion or subject is present,
        // persist a credential reference, and return a short-lived access token.
        let assertion = body.assertion.as_deref().unwrap_or("");
        let subject = body.subject.as_deref().unwrap_or("");
        if assertion.is_empty() && subject.is_empty() {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "missing_assertion",
                "assertion or subject is required to exchange a workload identity token.",
            ));
        }

        let credential_id = uuid::Uuid::new_v4().to_string();
        let token = generate_token(ACCESS_PREFIX);
        let preview: String = token.chars().take(23).collect();
        let expires_at = (chrono::Utc::now() + chrono::TimeDelta::hours(1)).to_rfc3339();
        conn.execute(
            "INSERT INTO workload_identity_credentials (id, provider_id, org_id, subject, token_preview, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![credential_id, body.provider_id, org, subject, preview, expires_at],
        ).map_err(internal)?;

        Ok(json!({
            "access_token": token,
            "token_type": "Bearer",
            "expires_at": expires_at,
            "provider": provider,
            "credential_id": credential_id,
        }))
    }).await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_context_honors_exact_and_wildcard_scopes() {
        let exact = CredentialContext {
            credential_id: "1".into(),
            kind: "test",
            scopes: vec!["vault:read".into()],
        };
        assert!(exact.allows("vault:read"));
        assert!(!exact.allows("vault:write"));
        let wildcard = CredentialContext {
            credential_id: "2".into(),
            kind: "test",
            scopes: vec!["*".into()],
        };
        assert!(wildcard.allows("anything"));
    }

    #[test]
    fn generated_credentials_have_distinct_expected_prefixes() {
        let admin = generate_token(ADMIN_PREFIX);
        let access = generate_token(ACCESS_PREFIX);
        assert!(admin.starts_with(ADMIN_PREFIX));
        assert!(access.starts_with(ACCESS_PREFIX));
        assert_ne!(hash_token(&admin), hash_token(&access));
    }

    #[test]
    fn request_authorization_maps_method_and_vault_resource() {
        let context = CredentialContext {
            credential_id: "1".into(),
            kind: "test",
            scopes: vec!["api:read".into(), "vault:write".into()],
        };
        assert!(context.allows_request(&axum::http::Method::GET, "/api/v1/agents"));
        assert!(!context.allows_request(&axum::http::Method::POST, "/api/v1/agents"));
        assert!(context.allows_request(&axum::http::Method::POST, "/api/v1/vault/credentials"));
        assert!(context.allows_request(&axum::http::Method::POST, "/api/v1/beta/vaults"));
        assert!(!context.allows_request(&axum::http::Method::GET, "/api/v1/beta/vaults"));
    }

    #[test]
    fn wif_provider_type_validation() {
        assert!(valid_wif_provider_type("aws"));
        assert!(valid_wif_provider_type("spiffe"));
        assert!(!valid_wif_provider_type("custom"));
    }
}
