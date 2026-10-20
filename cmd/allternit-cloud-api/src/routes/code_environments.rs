//! Code mode cloud environments (CODE_PARITY Contract B).
//!
//! An environment is a named, per-user recipe that a cloud Code session
//! starts in: network policy (`full` / `limited` / `none` + allowed hosts),
//! a bash setup script, raw `.env` text, repos to clone, and write-only
//! secrets.
//!
//! * `GET    /api/v1/code-environments` — `{ environments }`, archived rows
//!   only with `?archived=1`. A user with no live environment gets one named
//!   "Default" (network `full`, `is_default` true) created on the spot.
//! * `POST   /api/v1/code-environments` — create from the editable fields.
//! * `PATCH  /api/v1/code-environments/:id` — partial update.
//! * `POST   /api/v1/code-environments/:id/archive` / `…/unarchive`.
//! * `POST   /api/v1/code-environments/:id/secrets` — `{ name, value, hosts? }`,
//!   upsert by name; the value is sealed with the platform credential cipher.
//! * `DELETE /api/v1/code-environments/:id/secrets/:name`.
//! * `GET    /api/v1/code-environments/:id/resolve` — runtime device token
//!   only, and only for a device owned by the environment's user. Returns the
//!   environment plus `secret_values`: the ONLY place secret values leave
//!   this server. Values are never logged.
//!
//! The management routes accept a Clerk session or an `allternit_*` API token
//! with the `compute` scope (`auth::resolve_user_scoped`), or a runtime device
//! token acting as its owner (the same trust decision as
//! `runtime_pairing::approver_from_headers`). Rows are always scoped to the
//! caller's user id; another user's id answers 404.
//!
//! Secret values are encrypted with `allternit_cloud_core::CredentialCipher`
//! (`ALLTERNIT_CREDENTIALS_KEY`) and stored as `value_ciphertext` /
//! `value_nonce` like `services::inference_keys`. Without the cipher, the
//! secret write and resolve routes answer 503
//! `code_environment_secrets_not_configured`; everything else keeps working.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json as SqlJson, FromRow, PgPool};
use std::collections::BTreeMap;
use std::sync::Arc;

use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::{ApiError, ApiState};

/// Hosts a new environment allows under `limited` network access when the
/// caller does not send its own list: the common package registries plus
/// GitHub. Documented in `surfaces/docs/code/cloud-environments.mdx`.
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    // GitHub (clone, API, release and raw downloads)
    "github.com",
    "api.github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "raw.githubusercontent.com",
    "*.githubusercontent.com",
    "ghcr.io",
    // JavaScript
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "repo.yarnpkg.com",
    "bun.sh",
    // Python
    "pypi.org",
    "files.pythonhosted.org",
    // Rust
    "crates.io",
    "index.crates.io",
    "static.crates.io",
    "static.rust-lang.org",
    // Go
    "proxy.golang.org",
    "sum.golang.org",
    // Ruby
    "rubygems.org",
    // Java / Kotlin
    "repo.maven.apache.org",
    "repo1.maven.org",
    // OS packages
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
];

const MAX_NAME_CHARS: usize = 80;
const MAX_SCRIPT_BYTES: usize = 64 * 1024;
const MAX_ENV_VARS_BYTES: usize = 64 * 1024;
const MAX_SECRET_VALUE_BYTES: usize = 64 * 1024;
const MAX_SECRET_NAME_CHARS: usize = 128;
const MAX_HOSTS: usize = 200;
const MAX_REPOS: usize = 20;
const MAX_REPO_URL_CHARS: usize = 2048;
const MAX_BRANCH_CHARS: usize = 255;
const MAX_SECRETS_PER_ENV: i64 = 100;
const DEFAULT_ENV_NAME: &str = "Default";

/// GCM nonce length and blob prefix of the credential cipher's
/// `v1:<base64(nonce || ciphertext || tag)>` serialization (credentials.rs).
const NONCE_LEN: usize = 12;
const CIPHER_VERSION_PREFIX: &str = "v1:";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v1/code-environments",
            get(list_environments).post(create_environment),
        )
        .route("/api/v1/code-environments/:id", patch(update_environment))
        .route(
            "/api/v1/code-environments/:id/archive",
            post(archive_environment),
        )
        .route(
            "/api/v1/code-environments/:id/unarchive",
            post(unarchive_environment),
        )
        .route(
            "/api/v1/code-environments/:id/secrets",
            post(put_secret),
        )
        .route(
            "/api/v1/code-environments/:id/secrets/:name",
            delete(delete_secret),
        )
        .route(
            "/api/v1/code-environments/:id/resolve",
            get(resolve_environment),
        )
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSpec {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// A secret as the API shows it: name, hosts, created time. Never the value.
#[derive(Debug, Clone, Serialize)]
pub struct SecretInfo {
    pub name: String,
    pub hosts: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeEnvironment {
    pub id: String,
    pub name: String,
    pub network_access: String,
    pub allowed_hosts: Vec<String>,
    pub secrets: Vec<SecretInfo>,
    pub setup_script: String,
    pub env_vars: String,
    pub repos: Vec<RepoSpec>,
    pub is_default: bool,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `/resolve` answer: the environment plus decrypted secret values.
#[derive(Serialize)]
struct ResolvedEnvironment {
    #[serde(flatten)]
    environment: CodeEnvironment,
    secret_values: BTreeMap<String, String>,
}

/// Editable fields. Every field is optional so the same shape serves POST
/// (missing = default) and PATCH (missing = unchanged).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub network_access: Option<String>,
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
    #[serde(default)]
    pub setup_script: Option<String>,
    #[serde(default)]
    pub env_vars: Option<String>,
    #[serde(default)]
    pub repos: Option<Vec<RepoSpec>>,
    #[serde(default)]
    pub is_default: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretInput {
    name: String,
    value: String,
    #[serde(default)]
    hosts: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    archived: Option<String>,
}

#[derive(Debug, FromRow)]
struct EnvRow {
    id: String,
    name: String,
    network_access: String,
    allowed_hosts: SqlJson<Vec<String>>,
    setup_script: String,
    env_vars: String,
    repos: SqlJson<Vec<RepoSpec>>,
    is_default: bool,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct SecretRow {
    environment_id: String,
    name: String,
    hosts: SqlJson<Vec<String>>,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct SecretValueRow {
    name: String,
    value_ciphertext: Vec<u8>,
    value_nonce: Vec<u8>,
}

const ENV_COLUMNS: &str = "id, name, network_access, allowed_hosts, setup_script, \
     env_vars, repos, is_default, archived_at, created_at, updated_at";

// ---------------------------------------------------------------------------
// Validation (pure; unit-tested below)
// ---------------------------------------------------------------------------

/// Trimmed name, 1–80 characters, no control characters.
pub fn validate_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_NAME_CHARS {
        return Err(ApiError::ValidationError(format!(
            "name must be 1–{MAX_NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::ValidationError(
            "name must not contain control characters".to_string(),
        ));
    }
    Ok(name.to_string())
}

pub fn validate_network_access(raw: &str) -> Result<String, ApiError> {
    match raw {
        "full" | "limited" | "none" => Ok(raw.to_string()),
        _ => Err(ApiError::ValidationError(
            "network_access must be one of full, limited, none".to_string(),
        )),
    }
}

/// `^[A-Z_][A-Z0-9_]*$`, at most 128 characters.
pub fn validate_secret_name(raw: &str) -> Result<String, ApiError> {
    let mut chars = raw.chars();
    let valid_first = matches!(chars.next(), Some(c) if c.is_ascii_uppercase() || c == '_');
    let valid_rest = chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !valid_first || !valid_rest || raw.len() > MAX_SECRET_NAME_CHARS {
        return Err(ApiError::ValidationError(
            "secret name must match ^[A-Z_][A-Z0-9_]*$ (at most 128 characters)".to_string(),
        ));
    }
    Ok(raw.to_string())
}

/// One host: a DNS hostname, optionally with a leading `*.` wildcard.
/// Lower-cased; no scheme, port, path or IP literal with a port.
pub fn validate_host(raw: &str) -> Result<String, ApiError> {
    let host = raw.trim().to_ascii_lowercase();
    let invalid = || {
        ApiError::ValidationError(format!(
            "invalid host {:?}: use a hostname like registry.npmjs.org or *.example.com",
            raw.chars().take(80).collect::<String>()
        ))
    };
    let bare = host.strip_prefix("*.").unwrap_or(&host);
    if bare.is_empty() || bare.len() > 253 {
        return Err(invalid());
    }
    for label in bare.split('.') {
        let ok = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return Err(invalid());
        }
    }
    Ok(host)
}

/// Validated, de-duplicated host list (order kept), at most 200 entries.
pub fn validate_hosts(raw: &[String]) -> Result<Vec<String>, ApiError> {
    if raw.len() > MAX_HOSTS {
        return Err(ApiError::ValidationError(format!(
            "at most {MAX_HOSTS} hosts"
        )));
    }
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for host in raw {
        let host = validate_host(host)?;
        if !out.contains(&host) {
            out.push(host);
        }
    }
    Ok(out)
}

fn validate_text_size(field: &str, value: &str, max: usize) -> Result<(), ApiError> {
    if value.len() > max {
        return Err(ApiError::ValidationError(format!(
            "{field} must be at most {} KB",
            max / 1024
        )));
    }
    if value.contains('\0') {
        return Err(ApiError::ValidationError(format!(
            "{field} must not contain NUL bytes"
        )));
    }
    Ok(())
}

/// Repos: `https://…`, `ssh://…` or `git@host:owner/repo` URLs; optional
/// branch without whitespace or a leading `-` (never a git option).
pub fn validate_repos(raw: &[RepoSpec]) -> Result<Vec<RepoSpec>, ApiError> {
    if raw.len() > MAX_REPOS {
        return Err(ApiError::ValidationError(format!(
            "at most {MAX_REPOS} repos"
        )));
    }
    let mut out: Vec<RepoSpec> = Vec::with_capacity(raw.len());
    for repo in raw {
        let url = repo.url.trim().to_string();
        let scheme_ok = url.starts_with("https://")
            || url.starts_with("ssh://")
            || (url.starts_with("git@") && url.contains(':'));
        if url.is_empty()
            || url.len() > MAX_REPO_URL_CHARS
            || !scheme_ok
            || url.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(ApiError::ValidationError(
                "repo url must be an https://, ssh:// or git@host:owner/repo URL".to_string(),
            ));
        }
        let branch = match repo.branch.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(branch) => {
                if branch.len() > MAX_BRANCH_CHARS
                    || branch.starts_with('-')
                    || branch.contains("..")
                    || branch.chars().any(|c| c.is_whitespace() || c.is_control())
                {
                    return Err(ApiError::ValidationError(format!(
                        "invalid branch for {url}"
                    )));
                }
                Some(branch.to_string())
            }
        };
        if out.iter().any(|existing| existing.url == url) {
            continue;
        }
        out.push(RepoSpec { url, branch });
    }
    Ok(out)
}

/// The validated, normalized subset of an [`EnvironmentInput`].
#[derive(Debug, Default, PartialEq)]
struct ValidatedInput {
    name: Option<String>,
    network_access: Option<String>,
    allowed_hosts: Option<Vec<String>>,
    setup_script: Option<String>,
    env_vars: Option<String>,
    repos: Option<Vec<RepoSpec>>,
    is_default: Option<bool>,
}

fn validate_input(input: EnvironmentInput) -> Result<ValidatedInput, ApiError> {
    Ok(ValidatedInput {
        name: input.name.as_deref().map(validate_name).transpose()?,
        network_access: input
            .network_access
            .as_deref()
            .map(validate_network_access)
            .transpose()?,
        allowed_hosts: input.allowed_hosts.as_deref().map(validate_hosts).transpose()?,
        setup_script: match input.setup_script {
            Some(script) => {
                validate_text_size("setup_script", &script, MAX_SCRIPT_BYTES)?;
                Some(script)
            }
            None => None,
        },
        env_vars: match input.env_vars {
            Some(env) => {
                validate_text_size("env_vars", &env, MAX_ENV_VARS_BYTES)?;
                Some(env)
            }
            None => None,
        },
        repos: input.repos.as_deref().map(validate_repos).transpose()?,
        is_default: input.is_default,
    })
}

fn truthy(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "yes"))
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// The calling user's id: a runtime device token acts as its owner, else a
/// Clerk session or `compute`-scoped API token.
async fn caller_user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    if let Some(token) = device_token_from_headers(headers) {
        return Ok(runtime_device_for_token(&state.db, token, None).await?.user_id);
    }
    Ok(crate::auth::resolve_user_scoped(&state.db, headers, "compute")
        .await?
        .id)
}

fn cipher(state: &ApiState) -> Result<&Arc<allternit_cloud_core::CredentialCipher>, Response> {
    state.credential_cipher.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "code_environment_secrets_not_configured" })),
        )
            .into_response()
    })
}

// ---------------------------------------------------------------------------
// Crypto (same split as services::inference_keys; messages never carry values)
// ---------------------------------------------------------------------------

fn encrypt_parts(
    cipher: &allternit_cloud_core::CredentialCipher,
    plaintext: &str,
) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
    let encoded = cipher
        .encrypt(plaintext)
        .map_err(|_| ApiError::Internal("Failed to encrypt the secret".to_string()))?;
    let blob_b64 = encoded
        .strip_prefix(CIPHER_VERSION_PREFIX)
        .ok_or_else(|| ApiError::Internal("Unexpected cipher output format".to_string()))?;
    let blob = BASE64
        .decode(blob_b64)
        .map_err(|_| ApiError::Internal("Unexpected cipher output encoding".to_string()))?;
    if blob.len() < NONCE_LEN {
        return Err(ApiError::Internal("Unexpected cipher output length".to_string()));
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    Ok((ciphertext.to_vec(), nonce.to_vec()))
}

fn decrypt_parts(
    cipher: &allternit_cloud_core::CredentialCipher,
    ciphertext: &[u8],
    nonce: &[u8],
) -> Result<String, ApiError> {
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(ciphertext);
    let encoded = format!("{}{}", CIPHER_VERSION_PREFIX, BASE64.encode(blob));
    cipher
        .decrypt(&encoded)
        .map_err(|_| ApiError::Internal("Failed to decrypt a secret".to_string()))
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

fn to_api(row: EnvRow, secrets: Vec<SecretInfo>) -> CodeEnvironment {
    CodeEnvironment {
        id: row.id,
        name: row.name,
        network_access: row.network_access,
        allowed_hosts: row.allowed_hosts.0,
        secrets,
        setup_script: row.setup_script,
        env_vars: row.env_vars,
        repos: row.repos.0,
        is_default: row.is_default,
        archived: row.archived_at.is_some(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

async fn secrets_for(db: &PgPool, ids: &[String]) -> Result<Vec<SecretRow>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, SecretRow>(
        "SELECT environment_id, name, hosts, created_at FROM code_environment_secrets \
         WHERE environment_id = ANY($1) ORDER BY name",
    )
    .bind(ids)
    .fetch_all(db)
    .await?)
}

async fn hydrate(db: &PgPool, rows: Vec<EnvRow>) -> Result<Vec<CodeEnvironment>, ApiError> {
    let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    let mut secrets: BTreeMap<String, Vec<SecretInfo>> = BTreeMap::new();
    for secret in secrets_for(db, &ids).await? {
        secrets
            .entry(secret.environment_id)
            .or_default()
            .push(SecretInfo {
                name: secret.name,
                hosts: secret.hosts.0,
                created_at: secret.created_at,
            });
    }
    Ok(rows
        .into_iter()
        .map(|row| {
            let list = secrets.remove(&row.id).unwrap_or_default();
            to_api(row, list)
        })
        .collect())
}

async fn load_owned(db: &PgPool, user_id: &str, id: &str) -> Result<EnvRow, ApiError> {
    sqlx::query_as::<_, EnvRow>(&format!(
        "SELECT {ENV_COLUMNS} FROM code_environments WHERE id = $1 AND user_id = $2"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("Code environment not found".to_string()))
}

async fn load_one(db: &PgPool, user_id: &str, id: &str) -> Result<CodeEnvironment, ApiError> {
    let row = load_owned(db, user_id, id).await?;
    Ok(hydrate(db, vec![row]).await?.remove(0))
}

/// Ensure the user has at least one live environment; creates "Default".
/// Concurrent first loads race on the one-default unique index, and the
/// loser's insert is a no-op.
async fn ensure_default(db: &PgPool, user_id: &str) -> Result<(), ApiError> {
    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM code_environments WHERE user_id = $1 AND archived_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    if live > 0 {
        return Ok(());
    }
    let hosts: Vec<String> = DEFAULT_ALLOWED_HOSTS.iter().map(|h| h.to_string()).collect();
    sqlx::query(
        "INSERT INTO code_environments \
         (id, user_id, name, network_access, allowed_hosts, is_default) \
         VALUES ($1, $2, $3, 'full', $4, true) ON CONFLICT DO NOTHING",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(DEFAULT_ENV_NAME)
    .bind(SqlJson(hosts))
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_environments(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    ensure_default(&state.db, &user_id).await?;
    let include_archived = truthy(query.archived.as_deref());
    let rows = sqlx::query_as::<_, EnvRow>(&format!(
        "SELECT {ENV_COLUMNS} FROM code_environments \
         WHERE user_id = $1 AND ($2 OR archived_at IS NULL) \
         ORDER BY is_default DESC, created_at ASC"
    ))
    .bind(&user_id)
    .bind(include_archived)
    .fetch_all(&state.db)
    .await?;
    let environments = hydrate(&state.db, rows).await?;
    Ok(Json(serde_json::json!({ "environments": environments })))
}

async fn create_environment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(input): Json<EnvironmentInput>,
) -> Result<(StatusCode, Json<CodeEnvironment>), ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let input = validate_input(input)?;
    let name = input
        .name
        .ok_or_else(|| ApiError::ValidationError("name is required".to_string()))?;
    let allowed_hosts = input
        .allowed_hosts
        .unwrap_or_else(|| DEFAULT_ALLOWED_HOSTS.iter().map(|h| h.to_string()).collect());
    let id = uuid::Uuid::new_v4().to_string();
    let make_default = input.is_default.unwrap_or(false);

    let mut tx = state.db.begin().await?;
    if make_default {
        sqlx::query("UPDATE code_environments SET is_default = false WHERE user_id = $1 AND is_default")
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO code_environments \
         (id, user_id, name, network_access, allowed_hosts, setup_script, env_vars, repos, is_default) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&id)
    .bind(&user_id)
    .bind(&name)
    .bind(input.network_access.as_deref().unwrap_or("full"))
    .bind(SqlJson(allowed_hosts))
    .bind(input.setup_script.unwrap_or_default())
    .bind(input.env_vars.unwrap_or_default())
    .bind(SqlJson(input.repos.unwrap_or_default()))
    .bind(make_default)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(load_one(&state.db, &user_id, &id).await?)))
}

async fn update_environment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<EnvironmentInput>,
) -> Result<Json<CodeEnvironment>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    let input = validate_input(input)?;
    let existing = load_owned(&state.db, &user_id, &id).await?;
    if input.is_default == Some(true) && existing.archived_at.is_some() {
        return Err(ApiError::Conflict(
            "Unarchive the environment before making it the default".to_string(),
        ));
    }

    let mut tx = state.db.begin().await?;
    if input.is_default == Some(true) {
        sqlx::query(
            "UPDATE code_environments SET is_default = false \
             WHERE user_id = $1 AND is_default AND id <> $2",
        )
        .bind(&user_id)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE code_environments SET \
           name = COALESCE($3, name), \
           network_access = COALESCE($4, network_access), \
           allowed_hosts = COALESCE($5, allowed_hosts), \
           setup_script = COALESCE($6, setup_script), \
           env_vars = COALESCE($7, env_vars), \
           repos = COALESCE($8, repos), \
           is_default = COALESCE($9, is_default), \
           updated_at = CURRENT_TIMESTAMP \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(&id)
    .bind(&user_id)
    .bind(input.name)
    .bind(input.network_access)
    .bind(input.allowed_hosts.map(SqlJson))
    .bind(input.setup_script)
    .bind(input.env_vars)
    .bind(input.repos.map(SqlJson))
    .bind(input.is_default)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(load_one(&state.db, &user_id, &id).await?))
}

async fn archive_environment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<CodeEnvironment>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    load_owned(&state.db, &user_id, &id).await?;
    // Archiving clears the default flag so the one-default index never pins
    // an archived row; the next list re-creates "Default" if nothing is left.
    sqlx::query(
        "UPDATE code_environments SET \
           archived_at = COALESCE(archived_at, CURRENT_TIMESTAMP), \
           is_default = false, updated_at = CURRENT_TIMESTAMP \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(&id)
    .bind(&user_id)
    .execute(&state.db)
    .await?;
    Ok(Json(load_one(&state.db, &user_id, &id).await?))
}

async fn unarchive_environment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<CodeEnvironment>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    load_owned(&state.db, &user_id, &id).await?;
    sqlx::query(
        "UPDATE code_environments SET archived_at = NULL, updated_at = CURRENT_TIMESTAMP \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(&id)
    .bind(&user_id)
    .execute(&state.db)
    .await?;
    Ok(Json(load_one(&state.db, &user_id, &id).await?))
}

async fn put_secret(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SecretInput>,
) -> Response {
    let user_id = match caller_user_id(&state, &headers).await {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    let cipher = match cipher(&state) {
        Ok(cipher) => cipher.clone(),
        Err(response) => return response,
    };
    match put_secret_inner(&state.db, &cipher, &user_id, &id, input).await {
        Ok(environment) => Json(environment).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn put_secret_inner(
    db: &PgPool,
    cipher: &allternit_cloud_core::CredentialCipher,
    user_id: &str,
    id: &str,
    input: SecretInput,
) -> Result<CodeEnvironment, ApiError> {
    let name = validate_secret_name(&input.name)?;
    if input.value.is_empty() {
        return Err(ApiError::ValidationError("secret value is required".to_string()));
    }
    if input.value.len() > MAX_SECRET_VALUE_BYTES {
        return Err(ApiError::ValidationError(format!(
            "secret value must be at most {} KB",
            MAX_SECRET_VALUE_BYTES / 1024
        )));
    }
    let hosts = validate_hosts(input.hosts.as_deref().unwrap_or(&[]))?;
    load_owned(db, user_id, id).await?;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM code_environment_secrets WHERE environment_id = $1 AND name <> $2",
    )
    .bind(id)
    .bind(&name)
    .fetch_one(db)
    .await?;
    if count >= MAX_SECRETS_PER_ENV {
        return Err(ApiError::ValidationError(format!(
            "at most {MAX_SECRETS_PER_ENV} secrets per environment"
        )));
    }

    let (ciphertext, nonce) = encrypt_parts(cipher, &input.value)?;
    sqlx::query(
        "INSERT INTO code_environment_secrets \
         (environment_id, name, value_ciphertext, value_nonce, hosts) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (environment_id, name) DO UPDATE SET \
           value_ciphertext = EXCLUDED.value_ciphertext, \
           value_nonce = EXCLUDED.value_nonce, \
           hosts = EXCLUDED.hosts, \
           updated_at = CURRENT_TIMESTAMP",
    )
    .bind(id)
    .bind(&name)
    .bind(ciphertext)
    .bind(nonce)
    .bind(SqlJson(hosts))
    .execute(db)
    .await?;
    touch(db, id).await?;
    load_one(db, user_id, id).await
}

async fn delete_secret(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
) -> Result<Json<CodeEnvironment>, ApiError> {
    let user_id = caller_user_id(&state, &headers).await?;
    load_owned(&state.db, &user_id, &id).await?;
    let deleted = sqlx::query(
        "DELETE FROM code_environment_secrets WHERE environment_id = $1 AND name = $2",
    )
    .bind(&id)
    .bind(&name)
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(ApiError::NotFound("Secret not found".to_string()));
    }
    touch(&state.db, &id).await?;
    Ok(Json(load_one(&state.db, &user_id, &id).await?))
}

async fn touch(db: &PgPool, id: &str) -> Result<(), ApiError> {
    sqlx::query("UPDATE code_environments SET updated_at = CURRENT_TIMESTAMP WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Runtime-only: the environment with decrypted secret values, for the
/// device's own owner. A Clerk session or API token is refused (401); another
/// user's environment answers 404. Archived environments still resolve so a
/// session started just before an archive can finish its setup.
async fn resolve_environment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(token) = device_token_from_headers(&headers) else {
        return ApiError::Unauthorized("Runtime credential required".to_string()).into_response();
    };
    let device = match runtime_device_for_token(&state.db, token, None).await {
        Ok(device) => device,
        Err(error) => return error.into_response(),
    };
    let cipher = match cipher(&state) {
        Ok(cipher) => cipher.clone(),
        Err(response) => return response,
    };
    match resolve_inner(&state.db, &cipher, &device.user_id, &id).await {
        Ok(resolved) => Json(resolved).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn resolve_inner(
    db: &PgPool,
    cipher: &allternit_cloud_core::CredentialCipher,
    user_id: &str,
    id: &str,
) -> Result<ResolvedEnvironment, ApiError> {
    let environment = load_one(db, user_id, id).await?;
    let rows = sqlx::query_as::<_, SecretValueRow>(
        "SELECT name, value_ciphertext, value_nonce FROM code_environment_secrets \
         WHERE environment_id = $1",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let mut secret_values = BTreeMap::new();
    for row in rows {
        let value = decrypt_parts(cipher, &row.value_ciphertext, &row.value_nonce)?;
        secret_values.insert(row.name, value);
    }
    tracing::info!(
        environment_id = %id,
        secret_count = secret_values.len(),
        "code environment resolved for runtime"
    );
    Ok(ResolvedEnvironment {
        environment,
        secret_values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name("  Web app ").unwrap(), "Web app");
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(81)).is_err());
        assert!(validate_name(&"é".repeat(80)).is_ok());
        assert!(validate_name("bad\nname").is_err());
    }

    #[test]
    fn secret_names_follow_env_var_rules() {
        assert!(validate_secret_name("GITHUB_TOKEN").is_ok());
        assert!(validate_secret_name("_X1").is_ok());
        assert!(validate_secret_name("1X").is_err());
        assert!(validate_secret_name("lower").is_err());
        assert!(validate_secret_name("A-B").is_err());
        assert!(validate_secret_name("").is_err());
    }

    #[test]
    fn hosts_accept_hostnames_and_wildcards_only() {
        assert_eq!(validate_host("Registry.NPMjs.org").unwrap(), "registry.npmjs.org");
        assert_eq!(validate_host("*.example.com").unwrap(), "*.example.com");
        assert!(validate_host("https://github.com").is_err());
        assert!(validate_host("github.com:443").is_err());
        assert!(validate_host("a..b").is_err());
        assert!(validate_host("-bad.com").is_err());
        assert!(validate_host("*").is_err());
        assert_eq!(
            validate_hosts(&["a.com".into(), "A.com".into()]).unwrap(),
            vec!["a.com".to_string()]
        );
    }

    #[test]
    fn default_hosts_are_valid() {
        let hosts: Vec<String> = DEFAULT_ALLOWED_HOSTS.iter().map(|h| h.to_string()).collect();
        assert_eq!(validate_hosts(&hosts).unwrap().len(), hosts.len());
    }

    #[test]
    fn repos_reject_option_injection_and_bad_schemes() {
        let ok = validate_repos(&[RepoSpec {
            url: " https://github.com/a/b.git ".into(),
            branch: Some("main".into()),
        }])
        .unwrap();
        assert_eq!(ok[0].url, "https://github.com/a/b.git");
        assert!(validate_repos(&[RepoSpec { url: "git@github.com:a/b.git".into(), branch: None }]).is_ok());
        assert!(validate_repos(&[RepoSpec { url: "file:///etc".into(), branch: None }]).is_err());
        assert!(validate_repos(&[RepoSpec { url: "--upload-pack=x".into(), branch: None }]).is_err());
        assert!(validate_repos(&[RepoSpec {
            url: "https://github.com/a/b".into(),
            branch: Some("--orphan".into()),
        }])
        .is_err());
    }

    #[test]
    fn script_and_env_sizes_are_capped() {
        let input = EnvironmentInput {
            setup_script: Some("x".repeat(MAX_SCRIPT_BYTES + 1)),
            ..Default::default()
        };
        assert!(validate_input(input).is_err());
        let input = EnvironmentInput {
            env_vars: Some("A=1\n".into()),
            network_access: Some("limited".into()),
            ..Default::default()
        };
        assert!(validate_input(input).is_ok());
        let input = EnvironmentInput {
            network_access: Some("open".into()),
            ..Default::default()
        };
        assert!(validate_input(input).is_err());
    }

    #[test]
    fn api_shape_never_carries_secret_values() {
        let env = CodeEnvironment {
            id: "e".into(),
            name: "Default".into(),
            network_access: "full".into(),
            allowed_hosts: vec![],
            secrets: vec![SecretInfo {
                name: "TOKEN".into(),
                hosts: vec!["api.example.com".into()],
                created_at: Utc::now(),
            }],
            setup_script: String::new(),
            env_vars: String::new(),
            repos: vec![],
            is_default: true,
            archived: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let value = serde_json::to_value(&env).unwrap();
        assert!(value["secrets"][0].get("value").is_none());
        assert!(value.get("secret_values").is_none());
        assert_eq!(value["secrets"][0]["name"], "TOKEN");
    }
}
