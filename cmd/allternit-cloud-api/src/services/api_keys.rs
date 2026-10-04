//! Scoped platform API key management.
//!
//! API keys are long-lived credentials that authenticate programmatic access to
//! the Allternit Cloud API. The full token is returned exactly once when the key
//! is created; afterwards only a one-way hash is stored.

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use crate::error::ApiError;

const TOKEN_PREFIX: &str = "alt_";
const TOKEN_ENTROPY_BYTES: usize = 32;
/// Scopes a key gets when the mint request names none. `compute` drives
/// paired nodes (catalog + runtime proxy); `inference` lets the same key call
/// the model gateway (`/v1/chat/completions`), which gizzi's `allternit`
/// provider needs. Neither grants billing or admin.
const DEFAULT_SCOPES: [&str; 2] = ["compute", "inference"];

/// A key as returned to the owner (no hash exposed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub user_id: String,
    pub organization_id: Option<String>,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The plaintext token returned once at creation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedApiKey {
    #[serde(flatten)]
    pub key: ApiKey,
    pub token: String,
}

#[derive(Debug, FromRow)]
struct ApiKeyRow {
    id: String,
    user_id: String,
    organization_id: Option<String>,
    name: String,
    prefix: String,
    scopes: Vec<String>,
    last_used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

/// Input for creating a new API key.
pub struct CreateApiKeyInput {
    pub user_id: String,
    pub organization_id: Option<String>,
    pub name: String,
    pub scopes: Vec<String>,
}

pub(crate) fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> String {
    let mut entropy = [0u8; TOKEN_ENTROPY_BYTES];
    rand::thread_rng().fill_bytes(&mut entropy);
    format!("{}{}", TOKEN_PREFIX, hex::encode(entropy))
}

fn generate_id() -> String {
    format!("ak_{}", hex::encode(rand::random::<[u8; 16]>())).to_lowercase()
}

pub(crate) fn normalize_scopes(scopes: Vec<String>) -> Vec<String> {
    let scopes: Vec<String> = scopes
        .into_iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    // An empty list fails every scope check, so minting without scopes used
    // to produce a key that could neither drive nodes nor run models.
    if scopes.is_empty() {
        DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
    } else {
        scopes
    }
}

/// List active (non-revoked) API keys for a user.
pub async fn list_api_keys(db: &PgPool, user_id: &str) -> Result<Vec<ApiKey>, ApiError> {
    let rows = sqlx::query_as::<_, ApiKeyRow>(
        r#"
        SELECT id, user_id, organization_id, name, prefix, scopes, last_used_at, revoked_at, created_at
        FROM api_keys
        WHERE user_id = $1 AND revoked_at IS NULL
        ORDER BY created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(into_api_key).collect())
}

/// Create a new API key and return the full token exactly once.
pub async fn create_api_key(
    db: &PgPool,
    input: CreateApiKeyInput,
) -> Result<CreatedApiKey, ApiError> {
    let token = generate_token();
    let token_hash = hash_token(&token);
    let prefix = token.chars().take(12).collect::<String>();
    let id = generate_id();
    let scopes = normalize_scopes(input.scopes);

    if input.name.trim().is_empty() {
        return Err(ApiError::BadRequest("API key name is required".to_string()));
    }

    let row = sqlx::query_as::<_, ApiKeyRow>(
        r#"
        INSERT INTO api_keys (id, user_id, organization_id, name, token_hash, prefix, scopes)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, user_id, organization_id, name, prefix, scopes, last_used_at, revoked_at, created_at
        "#,
    )
    .bind(&id)
    .bind(&input.user_id)
    .bind(&input.organization_id)
    .bind(input.name.trim())
    .bind(&token_hash)
    .bind(&prefix)
    .bind(&scopes)
    .fetch_one(db)
    .await?;

    Ok(CreatedApiKey {
        key: into_api_key(row),
        token,
    })
}

/// Revoke an API key. Only the owning user can revoke their own keys.
pub async fn revoke_api_key(db: &PgPool, user_id: &str, key_id: &str) -> Result<(), ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE api_keys
        SET revoked_at = NOW(), updated_at = NOW()
        WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(key_id)
    .bind(user_id)
    .execute(db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("API key not found or already revoked".to_string()));
    }

    Ok(())
}

/// Find a valid API key by its full token and update its last-used timestamp.
pub async fn authenticate_api_key(
    db: &PgPool,
    token: &str,
) -> Result<Option<ApiKey>, ApiError> {
    // Project keys (`alt_live_…` / `alt_test_…`) authenticate only on the
    // Platform API (`routes::platform_v1`), never as the owning user here.
    if crate::routes::platform_v1::caller::is_project_key_token(token) {
        return Ok(None);
    }
    let token_hash = hash_token(token);

    let row = sqlx::query_as::<_, ApiKeyRow>(
        r#"
        UPDATE api_keys
        SET last_used_at = NOW(), updated_at = NOW()
        WHERE token_hash = $1 AND revoked_at IS NULL
        RETURNING id, user_id, organization_id, name, prefix, scopes, last_used_at, revoked_at, created_at
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(db)
    .await?;

    Ok(row.map(into_api_key))
}

/// A project-bound key as shown to the console (no hash).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ProjectKey {
    pub id: String,
    pub project_id: String,
    pub account_id: Option<String>,
    pub env: String,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The plaintext token is returned exactly once, at creation.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedProjectKey {
    #[serde(flatten)]
    pub key: ProjectKey,
    pub token: String,
}

pub struct CreateProjectKeyInput {
    pub user_id: String,
    pub organization_id: Option<String>,
    pub project_id: String,
    pub account_id: Option<String>,
    /// `sandbox` mints `alt_test_…`, `live` mints `alt_live_…`.
    pub env: crate::routes::platform_v1::ProjectEnv,
    pub name: String,
    pub scopes: Vec<String>,
}

/// Validate project-key scopes: lowercase, de-duplicated, non-empty, all known.
/// Unlike [`normalize_scopes`] there is no default: a project key must say
/// what it may do.
pub fn normalize_project_scopes(scopes: Vec<String>) -> Result<Vec<String>, ApiError> {
    use crate::routes::platform_v1::caller::PLATFORM_SCOPES;
    let mut out: Vec<String> = Vec::new();
    for scope in scopes {
        let scope = scope.trim().to_lowercase();
        if scope.is_empty() {
            continue;
        }
        if !PLATFORM_SCOPES.contains(&scope.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "Unknown scope '{scope}'. Valid scopes: {}",
                PLATFORM_SCOPES.join(", ")
            )));
        }
        if !out.contains(&scope) {
            out.push(scope);
        }
    }
    if out.is_empty() {
        return Err(ApiError::BadRequest(
            "At least one scope is required.".to_string(),
        ));
    }
    Ok(out)
}

fn generate_project_token(prefix: &str) -> String {
    let mut entropy = [0u8; TOKEN_ENTROPY_BYTES];
    rand::thread_rng().fill_bytes(&mut entropy);
    format!("{}{}", prefix, hex::encode(entropy))
}

const PROJECT_KEY_COLUMNS: &str =
    "id, project_id, account_id, env, name, prefix, scopes, last_used_at, created_at";

/// Mint a project-bound key (`alt_live_<64hex>` / `alt_test_<64hex>`), stored
/// hashed exactly like legacy keys. The caller has already checked that the
/// user may manage the project and that `account_id` belongs to it.
pub async fn create_project_key(
    db: &PgPool,
    input: CreateProjectKeyInput,
) -> Result<CreatedProjectKey, ApiError> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::BadRequest(
            "API key name must be 1 to 120 characters".to_string(),
        ));
    }
    let scopes = normalize_project_scopes(input.scopes)?;
    let token = generate_project_token(input.env.key_prefix());
    let prefix = token.chars().take(16).collect::<String>();

    let key = sqlx::query_as::<_, ProjectKey>(&format!(
        r#"
        INSERT INTO api_keys (id, user_id, organization_id, name, token_hash, prefix, scopes, project_id, account_id, env)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING {PROJECT_KEY_COLUMNS}
        "#
    ))
    .bind(generate_id())
    .bind(&input.user_id)
    .bind(&input.organization_id)
    .bind(name)
    .bind(hash_token(&token))
    .bind(&prefix)
    .bind(&scopes)
    .bind(&input.project_id)
    .bind(&input.account_id)
    .bind(input.env.as_str())
    .fetch_one(db)
    .await?;

    Ok(CreatedProjectKey { key, token })
}

/// Active (non-revoked) keys of one project, newest first.
pub async fn list_project_keys(db: &PgPool, project_id: &str) -> Result<Vec<ProjectKey>, ApiError> {
    Ok(sqlx::query_as::<_, ProjectKey>(&format!(
        "SELECT {PROJECT_KEY_COLUMNS} FROM api_keys \
         WHERE project_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC, id"
    ))
    .bind(project_id)
    .fetch_all(db)
    .await?)
}

/// Revoke one key of a project (the project check is the caller's job).
pub async fn revoke_project_key(db: &PgPool, project_id: &str, key_id: &str) -> Result<(), ApiError> {
    let result = sqlx::query(
        "UPDATE api_keys SET revoked_at = NOW(), updated_at = NOW() \
         WHERE id = $1 AND project_id = $2 AND revoked_at IS NULL",
    )
    .bind(key_id)
    .bind(project_id)
    .execute(db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("API key not found or already revoked".to_string()));
    }
    Ok(())
}

fn into_api_key(row: ApiKeyRow) -> ApiKey {
    ApiKey {
        id: row.id,
        user_id: row.user_id,
        organization_id: row.organization_id,
        name: row.name,
        prefix: row.prefix,
        scopes: row.scopes,
        last_used_at: row.last_used_at,
        revoked_at: row.revoked_at,
        created_at: row.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_have_expected_prefix_and_length() {
        let token = generate_token();
        assert!(token.starts_with(TOKEN_PREFIX));
        assert_eq!(token.len(), TOKEN_PREFIX.len() + TOKEN_ENTROPY_BYTES * 2);
    }

    #[test]
    fn token_hash_is_deterministic() {
        let token = "alt_test_token";
        let h1 = hash_token(token);
        let h2 = hash_token(token);
        assert_eq!(h1, h2);
        assert_ne!(h1, token);
    }

    #[test]
    fn scopes_are_normalized() {
        let scopes = vec!["  Read ".to_string(), "COMPUTE".to_string(), "".to_string()];
        assert_eq!(normalize_scopes(scopes), vec!["read", "compute"]);
    }

    #[test]
    fn empty_scopes_default_to_compute_and_inference() {
        assert_eq!(normalize_scopes(vec![]), vec!["compute", "inference"]);
        assert_eq!(
            normalize_scopes(vec!["".to_string(), "  ".to_string()]),
            vec!["compute", "inference"]
        );
    }
}
