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
use std::net::IpAddr;

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

/// List active (non-revoked) API keys for a user. Console playground keys
/// (`kind = 'playground'`) are short-lived and never listed.
pub async fn list_api_keys(db: &PgPool, user_id: &str) -> Result<Vec<ApiKey>, ApiError> {
    let rows = sqlx::query_as::<_, ApiKeyRow>(
        r#"
        SELECT id, user_id, organization_id, name, prefix, scopes, last_used_at, revoked_at, created_at
        FROM api_keys
        WHERE user_id = $1 AND revoked_at IS NULL AND kind <> 'playground'
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
/// No client IP is known here, so a key with an IP allowlist is refused (see
/// [`authenticate_api_key_at`]); a refused key is `None`.
pub async fn authenticate_api_key(
    db: &PgPool,
    token: &str,
) -> Result<Option<ApiKey>, ApiError> {
    Ok(authenticate_api_key_at(db, token, None).await?.ok())
}

/// [`authenticate_api_key`] with the caller's IP. `Ok(Err(_))` is a key that
/// exists and is not revoked but is past `expires_at` (401) or used from an
/// address outside `allowed_cidrs` (403). A missing key is `Ok(Err(Unknown))`.
pub async fn authenticate_api_key_at(
    db: &PgPool,
    token: &str,
    client_ip: Option<IpAddr>,
) -> Result<Result<ApiKey, KeyRejection>, ApiError> {
    // Project keys (`alt_live_…` / `alt_test_…`) authenticate only on the
    // Platform API (`routes::platform_v1`), never as the owning user here.
    if crate::routes::platform_v1::caller::is_project_key_token(token) {
        return Ok(Err(KeyRejection::Unknown));
    }
    let token_hash = hash_token(token);

    let Some(row) = sqlx::query_as::<_, AuthKeyRow>(
        r#"
        SELECT id, user_id, organization_id, name, prefix, scopes, revoked_at, created_at,
               expires_at, allowed_cidrs
        FROM api_keys
        WHERE token_hash = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(db)
    .await?
    else {
        return Ok(Err(KeyRejection::Unknown));
    };

    if let Err(rejection) = check_key_policy(
        row.expires_at,
        row.allowed_cidrs.as_deref(),
        client_ip,
        Utc::now(),
    ) {
        return Ok(Err(rejection));
    }

    sqlx::query("UPDATE api_keys SET last_used_at = NOW(), updated_at = NOW() WHERE id = $1")
        .bind(&row.id)
        .execute(db)
        .await?;

    Ok(Ok(ApiKey {
        id: row.id,
        user_id: row.user_id,
        organization_id: row.organization_id,
        name: row.name,
        prefix: row.prefix,
        scopes: row.scopes,
        last_used_at: Some(Utc::now()),
        revoked_at: row.revoked_at,
        created_at: row.created_at,
    }))
}

/// Header Cloudflare sets to the real client address on every proxied request
/// (api.allternit.com is proxied; the edge overwrites any client-sent value).
pub const CLIENT_IP_HEADER: &str = "cf-connecting-ip";

/// The caller's IP for key IP allowlists: the trusted proxy header only.
/// `X-Forwarded-For` is client-controlled and never used here.
pub fn client_ip_from_headers(headers: &axum::http::HeaderMap) -> Option<IpAddr> {
    headers
        .get(CLIENT_IP_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
}

/// Why an existing, unrevoked key was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRejection {
    /// No such key (or a revoked one, or a project key on a legacy path).
    Unknown,
    /// Past `expires_at` → 401.
    Expired(DateTime<Utc>),
    /// The client IP is outside `allowed_cidrs`, or unknown while a list is
    /// set → 403.
    IpNotAllowed(Option<IpAddr>),
}

impl KeyRejection {
    /// A message the developer can act on.
    pub fn message(&self) -> String {
        match self {
            KeyRejection::Unknown => "The API key is invalid or has been revoked.".to_string(),
            KeyRejection::Expired(at) => format!(
                "This API key expired on {}. Create a new key in the console.",
                at.format("%Y-%m-%d %H:%M UTC")
            ),
            KeyRejection::IpNotAllowed(Some(ip)) => format!(
                "This API key is not allowed from IP address {ip}. Add it to the key's allowed IP ranges in the console."
            ),
            KeyRejection::IpNotAllowed(None) => {
                "This API key is restricted to certain IP ranges and the request's address could not be determined.".to_string()
            }
        }
    }

    /// Machine code for the error body.
    pub fn code(&self) -> &'static str {
        match self {
            KeyRejection::Unknown => "invalid_api_key",
            KeyRejection::Expired(_) => "api_key_expired",
            KeyRejection::IpNotAllowed(_) => "ip_not_allowed",
        }
    }

    /// 401 for unknown and expired keys, 403 for a refused address.
    pub fn status(&self) -> axum::http::StatusCode {
        match self {
            KeyRejection::IpNotAllowed(_) => axum::http::StatusCode::FORBIDDEN,
            _ => axum::http::StatusCode::UNAUTHORIZED,
        }
    }
}

/// Expiry and IP allowlist rules shared by both key-auth paths. NULL expiry
/// and a NULL or empty allowlist mean no restriction. An allowlist entry is a
/// CIDR (`10.0.0.0/8`, `2001:db8::/32`) or a bare address; entries that do not
/// parse match nothing, so a list of only bad entries refuses every address.
pub fn check_key_policy(
    expires_at: Option<DateTime<Utc>>,
    allowed_cidrs: Option<&[String]>,
    client_ip: Option<IpAddr>,
    now: DateTime<Utc>,
) -> Result<(), KeyRejection> {
    if let Some(at) = expires_at {
        if at <= now {
            return Err(KeyRejection::Expired(at));
        }
    }
    let rules: Vec<&str> = allowed_cidrs
        .unwrap_or(&[])
        .iter()
        .map(|rule| rule.trim())
        .filter(|rule| !rule.is_empty())
        .collect();
    if rules.is_empty() {
        return Ok(());
    }
    match client_ip {
        Some(ip) if rules.iter().any(|rule| ip_in_cidr(ip, rule)) => Ok(()),
        other => Err(KeyRejection::IpNotAllowed(other)),
    }
}

/// True when `ip` is inside `cidr` (`a.b.c.d/n`, `x::/n` or a bare address).
/// IPv4-mapped IPv6 addresses are compared as IPv4.
pub(crate) fn ip_in_cidr(ip: IpAddr, cidr: &str) -> bool {
    let (addr, prefix) = match cidr.split_once('/') {
        Some((addr, prefix)) => match prefix.trim().parse::<u32>() {
            Ok(prefix) => (addr.trim(), Some(prefix)),
            Err(_) => return false,
        },
        None => (cidr.trim(), None),
    };
    let Ok(network) = addr.parse::<IpAddr>() else {
        return false;
    };
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            let prefix = prefix.unwrap_or(32);
            if prefix > 32 {
                return false;
            }
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            (u32::from(ip) & mask) == (u32::from(net) & mask)
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            let prefix = prefix.unwrap_or(128);
            if prefix > 128 {
                return false;
            }
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            (u128::from(ip) & mask) == (u128::from(net) & mask)
        }
        _ => false,
    }
}

/// Key row as read for authentication: the public fields plus the policy.
#[derive(Debug, FromRow)]
struct AuthKeyRow {
    id: String,
    user_id: String,
    organization_id: Option<String>,
    name: String,
    prefix: String,
    scopes: Vec<String>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    allowed_cidrs: Option<Vec<String>>,
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
    /// `standard` or `playground` (migrations_pg/122).
    pub kind: String,
    /// NULL = never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// NULL or empty = usable from any address.
    pub allowed_cidrs: Option<Vec<String>>,
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

const PROJECT_KEY_COLUMNS: &str = "id, project_id, account_id, env, name, prefix, scopes, last_used_at, created_at, \
     kind, expires_at, allowed_cidrs";

/// Key kinds stored in `api_keys.kind`.
pub const KEY_KIND_STANDARD: &str = "standard";
/// The console playground's short-lived key; usage rows say client `playground`.
pub const KEY_KIND_PLAYGROUND: &str = "playground";

/// Governance fields set when a project key is minted.
#[derive(Debug, Clone)]
pub struct KeyGovernance {
    /// [`KEY_KIND_STANDARD`] or [`KEY_KIND_PLAYGROUND`].
    pub kind: String,
    pub expires_at: Option<DateTime<Utc>>,
    /// CIDRs or bare addresses; empty = no restriction (stored as NULL).
    pub allowed_cidrs: Vec<String>,
}

impl Default for KeyGovernance {
    fn default() -> Self {
        Self { kind: KEY_KIND_STANDARD.to_string(), expires_at: None, allowed_cidrs: Vec::new() }
    }
}

/// Mint a project-bound key (`alt_live_<64hex>` / `alt_test_<64hex>`), stored
/// hashed exactly like legacy keys. The caller has already checked that the
/// user may manage the project and that `account_id` belongs to it.
pub async fn create_project_key(
    db: &PgPool,
    input: CreateProjectKeyInput,
) -> Result<CreatedProjectKey, ApiError> {
    create_project_key_with(db, input, KeyGovernance::default()).await
}

/// [`create_project_key`] with a kind, an expiry and an IP allowlist.
pub async fn create_project_key_with(
    db: &PgPool,
    input: CreateProjectKeyInput,
    governance: KeyGovernance,
) -> Result<CreatedProjectKey, ApiError> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError::BadRequest(
            "API key name must be 1 to 120 characters".to_string(),
        ));
    }
    if governance.kind != KEY_KIND_STANDARD && governance.kind != KEY_KIND_PLAYGROUND {
        return Err(ApiError::BadRequest(format!("Unknown key kind '{}'", governance.kind)));
    }
    let scopes = normalize_project_scopes(input.scopes)?;
    let allowed_cidrs = normalize_allowed_cidrs(governance.allowed_cidrs)?;
    let token = generate_project_token(input.env.key_prefix());
    let prefix = token.chars().take(16).collect::<String>();

    let key = sqlx::query_as::<_, ProjectKey>(&format!(
        r#"
        INSERT INTO api_keys (id, user_id, organization_id, name, token_hash, prefix, scopes, project_id, account_id, env,
                              kind, expires_at, allowed_cidrs)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
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
    .bind(&governance.kind)
    .bind(governance.expires_at)
    .bind(allowed_cidrs)
    .fetch_one(db)
    .await?;

    Ok(CreatedProjectKey { key, token })
}

/// Trim and validate an IP allowlist: every entry must be a CIDR or a bare
/// address. Empty → `None` (no restriction).
pub fn normalize_allowed_cidrs(entries: Vec<String>) -> Result<Option<Vec<String>>, ApiError> {
    let mut out: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.trim().to_string();
        if entry.is_empty() {
            continue;
        }
        let addr = entry.split_once('/').map(|(a, _)| a).unwrap_or(&entry);
        let Ok(ip) = addr.trim().parse::<IpAddr>() else {
            return Err(ApiError::BadRequest(format!("'{entry}' is not an IP address or CIDR range.")));
        };
        if !ip_in_cidr(ip, &entry) {
            return Err(ApiError::BadRequest(format!("'{entry}' is not an IP address or CIDR range.")));
        }
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    Ok((!out.is_empty()).then_some(out))
}

/// Active (non-revoked) keys of one project, newest first. Playground keys
/// are not listed (see [`create_playground_key`]).
pub async fn list_project_keys(db: &PgPool, project_id: &str) -> Result<Vec<ProjectKey>, ApiError> {
    Ok(sqlx::query_as::<_, ProjectKey>(&format!(
        "SELECT {PROJECT_KEY_COLUMNS} FROM api_keys \
         WHERE project_id = $1 AND revoked_at IS NULL AND kind <> 'playground' ORDER BY created_at DESC, id"
    ))
    .bind(project_id)
    .fetch_all(db)
    .await?)
}

/// How long a console playground key lives.
pub const PLAYGROUND_KEY_TTL_MINUTES: i64 = 15;

/// A freshly minted playground key: the plaintext token (shown once, held in
/// the console's memory only) and when it stops working.
#[derive(Debug, Clone)]
pub struct PlaygroundKey {
    pub id: String,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// Mint the console playground's key for `user_id` on `project_id`: a
/// user-style `alt_<64hex>` key (so `/v1/chat/completions` accepts it; project
/// keys are refused there), scope `inference` only, kind `playground`,
/// expiring [`PLAYGROUND_KEY_TTL_MINUTES`] from now. The project is attached
/// for usage attribution. The caller's previous playground key for the same
/// project is revoked in the same transaction, so at most one is live. The
/// caller has already checked project access.
pub async fn create_playground_key(
    db: &PgPool,
    user_id: &str,
    organization_id: Option<&str>,
    project_id: &str,
) -> Result<PlaygroundKey, ApiError> {
    let token = generate_token();
    let prefix = token.chars().take(12).collect::<String>();
    let id = generate_id();
    let expires_at = Utc::now() + chrono::Duration::minutes(PLAYGROUND_KEY_TTL_MINUTES);
    let scopes = vec!["inference".to_string()];

    let mut tx = db.begin().await?;
    sqlx::query(
        "UPDATE api_keys SET revoked_at = NOW(), updated_at = NOW() \
         WHERE user_id = $1 AND project_id = $2 AND kind = $3 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(project_id)
    .bind(KEY_KIND_PLAYGROUND)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO api_keys (id, user_id, organization_id, name, token_hash, prefix, scopes, project_id, kind, expires_at)
        VALUES ($1, $2, $3, 'Console playground', $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(&id)
    .bind(user_id)
    .bind(organization_id)
    .bind(hash_token(&token))
    .bind(&prefix)
    .bind(&scopes)
    .bind(project_id)
    .bind(KEY_KIND_PLAYGROUND)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(PlaygroundKey { id, token, expires_at })
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
    fn cidr_matching() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(ip_in_cidr(ip("10.1.2.3"), "10.0.0.0/8"));
        assert!(!ip_in_cidr(ip("11.1.2.3"), "10.0.0.0/8"));
        assert!(ip_in_cidr(ip("203.0.113.9"), "203.0.113.9"));
        assert!(!ip_in_cidr(ip("203.0.113.10"), "203.0.113.9/32"));
        assert!(ip_in_cidr(ip("1.2.3.4"), "0.0.0.0/0"));
        assert!(ip_in_cidr(ip("2001:db8::1"), "2001:db8::/32"));
        assert!(!ip_in_cidr(ip("2001:db9::1"), "2001:db8::/32"));
        assert!(ip_in_cidr(ip("::ffff:10.0.0.5"), "10.0.0.0/24"));
        assert!(!ip_in_cidr(ip("10.0.0.5"), "10.0.0.0/33"));
        assert!(!ip_in_cidr(ip("10.0.0.5"), "not-an-ip/8"));
        assert!(!ip_in_cidr(ip("10.0.0.5"), "2001:db8::/32"));
    }

    #[test]
    fn key_policy_expiry_and_allowlist() {
        let now = Utc::now();
        let past = now - chrono::Duration::minutes(1);
        let future = now + chrono::Duration::minutes(1);
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let list = vec!["10.0.0.0/8".to_string()];
        assert_eq!(check_key_policy(None, None, None, now), Ok(()));
        let none: Vec<String> = Vec::new();
        assert_eq!(check_key_policy(Some(future), Some(none.as_slice()), None, now), Ok(()));
        assert_eq!(check_key_policy(Some(past), None, ip("10.0.0.1"), now), Err(KeyRejection::Expired(past)));
        assert_eq!(check_key_policy(None, Some(list.as_slice()), ip("10.9.9.9"), now), Ok(()));
        assert_eq!(
            check_key_policy(None, Some(list.as_slice()), ip("203.0.113.9"), now),
            Err(KeyRejection::IpNotAllowed(ip("203.0.113.9")))
        );
        assert_eq!(check_key_policy(None, Some(list.as_slice()), None, now), Err(KeyRejection::IpNotAllowed(None)));
        assert_eq!(KeyRejection::Expired(past).status(), axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(KeyRejection::IpNotAllowed(None).status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn allowed_cidrs_are_validated() {
        assert_eq!(normalize_allowed_cidrs(vec![]).unwrap(), None);
        assert_eq!(normalize_allowed_cidrs(vec![" ".into()]).unwrap(), None);
        assert_eq!(
            normalize_allowed_cidrs(vec![" 10.0.0.0/8 ".into(), "10.0.0.0/8".into(), "2001:db8::1".into()]).unwrap(),
            Some(vec!["10.0.0.0/8".to_string(), "2001:db8::1".to_string()])
        );
        assert!(normalize_allowed_cidrs(vec!["10.0.0.0/40".into()]).is_err());
        assert!(normalize_allowed_cidrs(vec!["office".into()]).is_err());
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
