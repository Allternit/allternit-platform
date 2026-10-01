//! Clerk JWT Authentication
//!
//! Verifies Bearer tokens against Clerk's JWKS endpoint.
//! - Fetches and caches JWKS from Clerk
//! - Verifies JWT signatures on each request
//! - Extracts user info and ensures user exists in local DB
//! - Attaches AuthUser to request extensions

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Extracts `kid` from a JWT header without jsonwebtoken's Header
/// deserialization, which rejects Clerk's numeric `oiat` claim under v10.
fn jwt_header_kid(token: &str) -> Result<String, AuthError> {
    let part = token
        .split('.')
        .next()
        .ok_or_else(|| AuthError::TokenDecode("Malformed token".to_string()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|e| AuthError::TokenDecode(e.to_string()))?;
    let header: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| AuthError::TokenDecode(e.to_string()))?;
    header
        .get("kid")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| AuthError::TokenDecode("Token missing 'kid' header".to_string()))
}
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::db::DbHandle;

const DESKTOP_ACCESS_TOKEN_HEADER: &str = "x-allternit-desktop-access-token";
const SELF_HOSTED_SETUP_TOKEN_HEADER: &str = "x-allternit-self-hosted-token";
const INTERNAL_SERVICE_TOKEN_HEADER: &str = "x-allternit-internal-token";

/// User id synthesized by `extract_internal_service_user` for requests that
/// authenticated with the internal service token. Routes that accept
/// server-to-server calls (e.g. the Stripe-webhook credit grant in
/// fabric_credits_routes) key off this identity; it is only ever injected by
/// auth_middleware AFTER the token is verified, never from request input.
pub const INTERNAL_SERVICE_USER_ID: &str = "internal-service";
const USER_ID_HEADER: &str = "x-allternit-user-id";
const USER_EMAIL_HEADER: &str = "x-allternit-user-email";
const USER_NAME_HEADER: &str = "x-allternit-user-name";
const ORGANIZATION_ID_HEADER: &str = "x-allternit-organization-id";
const ORGANIZATION_ROLE_HEADER: &str = "x-allternit-organization-role";
const ORGANIZATION_SLUG_HEADER: &str = "x-allternit-organization-slug";

// ─── Configuration ──────────────────────────────────────────────────────────

const DEFAULT_CLERK_JWKS_URL: &str = "https://clerk.allternit.com/.well-known/jwks.json";
const DEFAULT_CLERK_ISSUER: &str = "https://clerk.allternit.com";
/// First-party Clerk proxy used by Fabric Transport. Browser session JWTs
/// carry this `iss`; cloud-api already accepts it.
pub(crate) const CLERK_PROXY_ISSUER: &str = "https://allternit.com/__clerk";

fn allowed_clerk_issuers(primary: &str) -> Vec<&str> {
    let mut list = vec![primary, DEFAULT_CLERK_ISSUER, CLERK_PROXY_ISSUER];
    list.sort_unstable();
    list.dedup();
    list
}

/// True when `iss` is one of the Clerk issuers this API accepts.
pub(crate) fn is_clerk_issuer(iss: &str, primary: &str) -> bool {
    allowed_clerk_issuers(primary).contains(&iss)
}

/// How long to cache JWKS before refreshing
const JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);

/// How long to wait for JWKS fetch
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Runtime authentication configuration.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Clerk JWKS endpoint.
    pub clerk_jwks_url: String,
    /// Expected JWT issuer (`iss`).
    pub clerk_issuer: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            clerk_jwks_url: DEFAULT_CLERK_JWKS_URL.to_string(),
            clerk_issuer: DEFAULT_CLERK_ISSUER.to_string(),
        }
    }
}

impl AuthConfig {
    /// Build configuration from environment, falling back to sensible defaults.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(url) = std::env::var("CLERK_JWKS_URL") {
            config.clerk_jwks_url = url;
        }
        if let Ok(issuer) = std::env::var("CLERK_ISSUER") {
            config.clerk_issuer = issuer;
        }
        config
    }

    /// Build configuration from the unified app config, with env overrides.
    pub fn from_app_config(app_config: &crate::config::AppConfig) -> Self {
        let mut config = Self::default();
        if let Some(url) = app_config.clerk_jwks_url() {
            config.clerk_jwks_url = url;
        }
        if let Some(issuer) = app_config.clerk_issuer() {
            config.clerk_issuer = issuer;
        }
        config
    }
}

// ─── Data Types ─────────────────────────────────────────────────────────────

/// Authenticated user attached to request extensions
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub tenant_id: Option<String>,
    pub organization_id: Option<String>,
    pub organization_role: Option<String>,
    pub organization_slug: Option<String>,
}

/// Build a default local user for dev/test mode.
fn local_dev_user(tenant_id: Option<String>) -> AuthUser {
    AuthUser {
        user_id: "local-dev-user".to_string(),
        email: Some("dev@allternit.local".to_string()),
        name: Some("Local Developer".to_string()),
        avatar_url: None,
        tenant_id,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    }
}

/// Returns true when the request appears to originate from the same machine.
pub(crate) fn is_localhost_origin(headers: &HeaderMap) -> bool {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let referer = headers
        .get("referer")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let local = |s: &str| {
        s.contains("://127.0.0.1")
            || s.contains("://localhost")
            || s.starts_with("127.0.0.1:")
            || s.starts_with("localhost:")
    };

    local(host) || local(origin) || local(referer)
}

/// Clerk JWT claims
#[derive(Debug, Serialize, Deserialize)]
struct ClerkOrganizationClaims {
    id: String,
    #[serde(default)]
    rol: Option<String>,
    #[serde(default)]
    slg: Option<String>,
}

/// JWKS key entry
#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct JwkKey {
    kid: String,
    kty: String,
    #[serde(rename = "use", default)]
    key_use: Option<String>,
    n: Option<String>,
    e: Option<String>,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

/// JWKS response
#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

/// Cached JWKS with timestamp
struct CachedJwks {
    keys: HashMap<String, JwkKey>,
    fetched_at: Instant,
}

// ─── JWKS Manager ───────────────────────────────────────────────────────────

pub struct JwksManager {
    cache: RwLock<Option<CachedJwks>>,
    client: reqwest::Client,
    jwks_url: String,
}

impl JwksManager {
    pub fn new(config: &AuthConfig) -> Self {
        Self {
            cache: RwLock::new(None),
            client: reqwest::Client::builder()
                .timeout(JWKS_FETCH_TIMEOUT)
                .build()
                .expect("Failed to build HTTP client"),
            jwks_url: config.clerk_jwks_url.clone(),
        }
    }

    /// Returns `true` when a JWKS fetch has succeeded recently enough to be usable.
    pub async fn is_ready(&self) -> bool {
        let cache = self.cache.read().await;
        if let Some(ref cached) = *cache {
            // Ready when we have keys, or when the last successful fetch is still fresh.
            !cached.keys.is_empty() || cached.fetched_at.elapsed() < JWKS_CACHE_TTL
        } else {
            false
        }
    }

    /// Prefetch Clerk JWKS at boot so `/health` is not `jwks: false` until
    /// the first Clerk JWT happens to arrive (desktop sessions use device
    /// tokens and would otherwise stay degraded forever).
    pub async fn warmup(&self) {
        match self.fetch_jwks().await {
            Ok(keys) => {
                let mut cache = self.cache.write().await;
                *cache = Some(CachedJwks {
                    keys,
                    fetched_at: Instant::now(),
                });
            }
            Err(e) => {
                warn!("JWKS warmup failed from {}: {}", self.jwks_url, e);
            }
        }
    }

    /// Get a JWK by key ID, fetching from Clerk if necessary
    async fn get_key(&self, kid: &str) -> Option<JwkKey> {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(ref cached) = *cache {
                if cached.fetched_at.elapsed() < JWKS_CACHE_TTL {
                    if let Some(key) = cached.keys.get(kid) {
                        return Some(key.clone());
                    }
                }
            }
        }

        // Cache miss or expired — fetch fresh JWKS
        match self.fetch_jwks().await {
            Ok(keys) => {
                let mut cache = self.cache.write().await;
                *cache = Some(CachedJwks {
                    keys: keys.clone(),
                    fetched_at: Instant::now(),
                });
                keys.get(kid).cloned()
            }
            Err(e) => {
                error!("Failed to fetch JWKS: {}", e);
                None
            }
        }
    }

    async fn fetch_jwks(&self) -> Result<HashMap<String, JwkKey>, AuthError> {
        info!("Fetching JWKS from {}", self.jwks_url);
        let resp = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::JwksFetch(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(AuthError::JwksFetch(format!("HTTP {}", resp.status())));
        }

        let jwks: JwksResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::JwksParse(e.to_string()))?;

        let mut keys = HashMap::new();
        for key in jwks.keys {
            keys.insert(key.kid.clone(), key);
        }

        info!("Loaded {} JWKS keys", keys.len());
        Ok(keys)
    }
}

impl Default for JwksManager {
    fn default() -> Self {
        Self::new(&AuthConfig::default())
    }
}

// ─── Auth Errors ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AuthError {
    MissingToken,
    InvalidTokenFormat,
    JwksFetch(String),
    JwksParse(String),
    KeyNotFound(String),
    TokenDecode(String),
    Expired,
    DbError(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::MissingToken => write!(f, "Missing authorization token"),
            AuthError::InvalidTokenFormat => write!(f, "Invalid authorization header format"),
            AuthError::JwksFetch(msg) => write!(f, "JWKS fetch error: {}", msg),
            AuthError::JwksParse(msg) => write!(f, "JWKS parse error: {}", msg),
            AuthError::KeyNotFound(kid) => write!(f, "JWKS key not found: {}", kid),
            AuthError::TokenDecode(msg) => write!(f, "Token decode error: {}", msg),
            AuthError::Expired => write!(f, "Token expired"),
            AuthError::DbError(msg) => write!(f, "Database error: {}", msg),
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthError::MissingToken => (StatusCode::UNAUTHORIZED, "Missing authorization token"),
            AuthError::InvalidTokenFormat => (
                StatusCode::UNAUTHORIZED,
                "Invalid authorization header format",
            ),
            AuthError::JwksFetch(msg) => {
                error!("JWKS fetch error: {}", msg);
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Authentication service unavailable",
                )
            }
            AuthError::JwksParse(msg) => {
                error!("JWKS parse error: {}", msg);
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Authentication service unavailable",
                )
            }
            AuthError::KeyNotFound(kid) => {
                warn!("JWKS key not found: {}", kid);
                (StatusCode::UNAUTHORIZED, "Invalid token signature")
            }
            AuthError::TokenDecode(msg) => {
                warn!("Token decode error: {}", msg);
                (StatusCode::UNAUTHORIZED, "Invalid token")
            }
            AuthError::Expired => (StatusCode::UNAUTHORIZED, "Token expired"),
            AuthError::DbError(msg) => {
                error!("Auth DB error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error")
            }
        };

        let body = Json(json!({
            "error": "Unauthorized",
            "message": message,
        }));

        (status, body).into_response()
    }
}

// ─── Token Verification ─────────────────────────────────────────────────────

pub async fn verify_token(
    jwks: &JwksManager,
    token: &str,
    config: &AuthConfig,
) -> Result<AuthUser, AuthError> {
    let claims = verify_token_claims(jwks, token, config).await?;
    user_from_claims(&claims)
}

/// Verifies signature + issuer + expiry and returns the raw claims. Split out
/// of `verify_token` so the MCP server can additionally enforce `aud`/scope on
/// OAuth access tokens (`mcp_agents`).
pub(crate) async fn verify_token_claims(
    jwks: &JwksManager,
    token: &str,
    config: &AuthConfig,
) -> Result<serde_json::Value, AuthError> {
    // jsonwebtoken v10's Header flattens non-standard claims into
    // `HashMap<String, String> extras`, so any non-string custom claim
    // (Clerk's numeric `oiat`) fails decode_header AND decode() for every
    // Clerk token. Verify RS256 + issuer + expiry manually instead.
    let kid = jwt_header_kid(token)?;

    // Fetch the signing key
    let jwk = jwks
        .get_key(&kid)
        .await
        .ok_or_else(|| AuthError::KeyNotFound(kid))?;

    let n = jwk
        .n
        .as_ref()
        .ok_or_else(|| AuthError::KeyNotFound("JWK missing 'n'".to_string()))?;
    let e = jwk
        .e
        .as_ref()
        .ok_or_else(|| AuthError::KeyNotFound("JWK missing 'e'".to_string()))?;

    verify_rs256(token, n, e, &config.clerk_issuer)
}

pub(crate) fn user_from_claims(claims: &serde_json::Value) -> Result<AuthUser, AuthError> {
    let (organization_id, organization_role, organization_slug) = match claims.get("o") {
        Some(o) => (
            str_claim(o, "id"),
            str_claim(o, "rol"),
            str_claim(o, "slg"),
        ),
        None => (
            str_claim(claims, "org_id"),
            str_claim(claims, "org_role"),
            str_claim(claims, "org_slug"),
        ),
    };

    Ok(AuthUser {
        user_id: claims
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthError::TokenDecode("Token missing 'sub'".to_string()))?
            .to_string(),
        email: str_claim(claims, "email"),
        name: str_claim(claims, "name"),
        avatar_url: str_claim(claims, "image_url"),
        tenant_id: None,
        organization_id,
        organization_role,
        organization_slug,
    })
}

fn str_claim(claims: &serde_json::Value, key: &str) -> Option<String> {
    claims.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

/// Verifies an RS256 JWT against RSA components and enforces issuer + expiry
/// (60s leeway), returning the raw claims. See `verify_token` for why
/// jsonwebtoken v10 cannot be used on this path.
fn verify_rs256(
    token: &str,
    n_b64: &str,
    e_b64: &str,
    issuer: &str,
) -> Result<serde_json::Value, AuthError> {
    let mut parts = token.split('.');
    let (header_b64, payload_b64, signature_b64) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => (h, p, s),
        _ => return Err(AuthError::TokenDecode("Malformed token".to_string())),
    };
    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|e| AuthError::TokenDecode(e.to_string()))?;
    let n = URL_SAFE_NO_PAD
        .decode(n_b64)
        .map_err(|e| AuthError::TokenDecode(e.to_string()))?;
    let e = URL_SAFE_NO_PAD
        .decode(e_b64)
        .map_err(|e| AuthError::TokenDecode(e.to_string()))?;

    let public_key = aws_lc_rs::signature::RsaPublicKeyComponents { n: &n, e: &e };
    public_key
        .verify(
            &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{header_b64}.{payload_b64}").as_bytes(),
            &signature,
        )
        .map_err(|_| AuthError::TokenDecode("Invalid signature".to_string()))?;

    let claims: serde_json::Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|e| AuthError::TokenDecode(e.to_string()))?,
    )
    .map_err(|e| AuthError::TokenDecode(e.to_string()))?;

    let iss = claims.get("iss").and_then(|v| v.as_str()).unwrap_or("");
    if !allowed_clerk_issuers(issuer).contains(&iss) {
        return Err(AuthError::TokenDecode("Invalid issuer".to_string()));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match claims.get("exp").and_then(|v| v.as_u64()) {
        Some(exp) if now <= exp + 60 => {}
        _ => return Err(AuthError::Expired),
    }
    Ok(claims)
}

/// Ensure a user exists in the local SQLite DB, creating if necessary.
/// Returns the verified active organization, if the request has one. Callers
/// assign this back onto the per-request `AuthUser` rather than trusting a
/// possibly stale users-table pointer. Enterprise BYOC remains organization
/// scoped; a personal organization is never synthesized for a solo session.
pub fn ensure_user_in_db(db: &DbHandle, user: &AuthUser) -> Result<Option<String>, AuthError> {
    let conn = db
        .connect()
        .map_err(|e| AuthError::DbError(e.to_string()))?;

    // A user row may already exist under a different id with the same email
    // (e.g. `dev-user` vs `local-dev-user`, both seeded with dev@allternit.local).
    // ON CONFLICT(id) can't resolve that — the email update would violate
    // UNIQUE(email) — so adopt the existing row's id for all writes here.
    let mut effective_user_id = user.user_id.clone();
    if let Some(email) = user.email.as_deref() {
        let prior: Result<String, _> = conn.query_row(
            "SELECT id FROM users WHERE email = ?1",
            [email],
            |row| row.get(0),
        );
        if let Ok(prior_id) = prior {
            if prior_id != user.user_id {
                effective_user_id = prior_id;
            }
        }
    }

    conn.execute(
        "INSERT INTO users (id, email, name, avatar_url, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET
            email = COALESCE(excluded.email, users.email),
            name = COALESCE(excluded.name, users.name),
            avatar_url = COALESCE(excluded.avatar_url, users.avatar_url),
            updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![
            &effective_user_id,
            user.email.as_deref(),
            user.name.as_deref(),
            user.avatar_url.as_deref(),
        ],
    )
    .map_err(|e| AuthError::DbError(e.to_string()))?;

    let Some(organization_id) = user.organization_id.as_deref() else {
        conn.execute(
            "UPDATE users SET organization_id = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            [&effective_user_id],
        )
        .map_err(|e| AuthError::DbError(e.to_string()))?;
        return Ok(None);
    };
    let organization_name = user
        .organization_slug
        .as_deref()
        .unwrap_or(organization_id)
        .to_string();
    let role = user
        .organization_role
        .as_deref()
        .unwrap_or("member")
        .strip_prefix("org:")
        .unwrap_or_else(|| user.organization_role.as_deref().unwrap_or("member"))
        .to_string();

    let member_id = format!("{organization_id}:{}", effective_user_id);

    conn.execute(
        "INSERT INTO organizations (id, name, billing_email, created_at, updated_at)
         VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            billing_email = COALESCE(organizations.billing_email, excluded.billing_email),
            updated_at = CURRENT_TIMESTAMP",
        rusqlite::params![organization_id, organization_name, user.email.as_deref()],
    )
    .map_err(|e| AuthError::DbError(e.to_string()))?;

    // Only a sign-in path that carries the org role (a Clerk session's
    // `o.rol` / `org_role`) may change an existing member's role. Paths with
    // no role (runtime-device and DP tokens) used to write "member" here on
    // every request, downgrading the org owner to member and locking them out
    // of owner/admin routes (e.g. metered usage) in Desktop.
    let role_known = user.organization_role.as_deref().is_some_and(|r| !r.trim().is_empty());
    conn.execute(
        "INSERT INTO organization_members (id, organization_id, user_id, role, joined_at)
         VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
         ON CONFLICT(organization_id, user_id) DO UPDATE SET
            role = CASE WHEN ?5 THEN excluded.role ELSE organization_members.role END",
        rusqlite::params![member_id, organization_id, &effective_user_id, role, role_known],
    )
    .map_err(|e| AuthError::DbError(e.to_string()))?;

    conn.execute(
        "UPDATE users SET organization_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        rusqlite::params![organization_id, &effective_user_id],
    )
    .map_err(|e| AuthError::DbError(e.to_string()))?;

    Ok(Some(organization_id.to_string()))
}

// ─── Axum Middleware ────────────────────────────────────────────────────────

/// Extract Bearer token from Authorization header
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth_header = headers.get(axum::http::header::AUTHORIZATION)?;
    let auth_str = auth_header.to_str().ok()?;
    auth_str.strip_prefix("Bearer ").map(|s| s.to_string())
}

fn extract_header_string(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn insert_user_headers(headers: &mut HeaderMap, user: &AuthUser) {
    // Never let caller-supplied identity/scope headers survive authentication.
    // Desktop bootstrap values have already been copied into `user` before
    // this function runs; Clerk values came from the verified JWT.
    for name in [
        USER_ID_HEADER,
        USER_EMAIL_HEADER,
        USER_NAME_HEADER,
        "x-allternit-tenant-id",
        ORGANIZATION_ID_HEADER,
        ORGANIZATION_ROLE_HEADER,
        ORGANIZATION_SLUG_HEADER,
    ] {
        headers.remove(name);
    }
    headers.insert(
        HeaderName::from_static(USER_ID_HEADER),
        HeaderValue::from_str(&user.user_id).unwrap(),
    );
    if let Some(ref email) = user.email {
        if let Ok(value) = HeaderValue::from_str(email) {
            headers.insert(HeaderName::from_static(USER_EMAIL_HEADER), value);
        }
    }
    if let Some(ref name) = user.name {
        if let Ok(value) = HeaderValue::from_str(name) {
            headers.insert(HeaderName::from_static(USER_NAME_HEADER), value);
        }
    }
    if let Some(ref organization_id) = user.organization_id {
        if let Ok(value) = HeaderValue::from_str(organization_id) {
            headers.insert(HeaderName::from_static(ORGANIZATION_ID_HEADER), value);
        }
    }
    if let Some(ref role) = user.organization_role {
        if let Ok(value) = HeaderValue::from_str(role) {
            headers.insert(HeaderName::from_static(ORGANIZATION_ROLE_HEADER), value);
        }
    }
    if let Some(ref slug) = user.organization_slug {
        if let Ok(value) = HeaderValue::from_str(slug) {
            headers.insert(HeaderName::from_static(ORGANIZATION_SLUG_HEADER), value);
        }
    }
}

/// Constant-time comparison — avoids leaking the secret's length/prefix via
/// early-exit timing (same concern `internal_auth::require_internal_token`
/// and `token_crypto`'s AEAD tag check handle for their own secrets).
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Local-only gate for managed-runtime endpoints (e.g. the fabric worker
/// auto-provision route): true only when the caller presents the
/// `x-allternit-desktop-access-token` header matching the configured
/// `ALLTERNIT_DESKTOP_ACCESS_TOKEN` spawn-time secret. Fail-closed when no
/// secret is configured — in cloud deployments this path simply does not
/// exist.
pub fn verify_desktop_access_token(
    headers: &HeaderMap,
    config: &crate::config::AppConfig,
) -> bool {
    let expected = match config.desktop_access_token() {
        Some(token) => token,
        None => return false,
    };
    let provided = headers
        .get(DESKTOP_ACCESS_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    !provided.is_empty() && constant_time_eq(provided, &expected)
}

fn extract_desktop_bootstrap_user(
    headers: &HeaderMap,
    config: &crate::config::AppConfig,
) -> Option<AuthUser> {
    let user_id = extract_header_string(headers, USER_ID_HEADER)?;
    let provided_token = extract_header_string(headers, DESKTOP_ACCESS_TOKEN_HEADER)?;

    // This path trusts every other caller-supplied identity header outright
    // (organization_id, organization_role, ...), so it must never activate on
    // the strength of "a token-shaped header was present" alone. Require a
    // real match against a configured secret; no secret configured means the
    // path is disabled, not silently permissive.
    let expected_token = config.desktop_access_token()?;
    if !constant_time_eq(&provided_token, &expected_token) {
        return None;
    }

    let email = extract_header_string(headers, USER_EMAIL_HEADER);
    let name = extract_header_string(headers, USER_NAME_HEADER);
    let tenant_id = extract_header_string(headers, "x-allternit-tenant-id");
    let organization_id =
        extract_header_string(headers, ORGANIZATION_ID_HEADER).or_else(|| tenant_id.clone());
    Some(AuthUser {
        user_id,
        email,
        name,
        avatar_url: None,
        tenant_id,
        organization_id,
        organization_role: extract_header_string(headers, ORGANIZATION_ROLE_HEADER),
        organization_slug: extract_header_string(headers, ORGANIZATION_SLUG_HEADER),
    })
}

/// Self-hosted setup-token authentication. When a deployment has no Clerk
/// tenant, the onboarding wizard proves it was bootstrapped by the operator by
/// sending the one-time `ALLTERNIT_SELF_HOSTED_SETUP_TOKEN` in a header. This
/// path is disabled unless both `self_hosted` is true and a setup token is
/// configured.
fn extract_self_hosted_setup_user(
    headers: &HeaderMap,
    config: &crate::config::AppConfig,
) -> Option<AuthUser> {
    if !config.self_hosted() {
        return None;
    }
    let provided_token = extract_header_string(headers, SELF_HOSTED_SETUP_TOKEN_HEADER)?;
    let expected_token = config.self_hosted_setup_token()?;
    if !constant_time_eq(&provided_token, &expected_token) {
        return None;
    }
    Some(AuthUser {
        user_id: "self-hosted-admin".to_string(),
        email: Some("admin@allternit.local".to_string()),
        name: Some("Self-Hosted Admin".to_string()),
        avatar_url: None,
        tenant_id: Some(config.tenant_id()),
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    })
}

/// Internal service-token authentication. Peer services and health probes prove
/// identity with the shared `ALLTERNIT_INTERNAL_SERVICE_TOKEN`.
fn extract_internal_service_user(
    headers: &HeaderMap,
    config: &crate::config::AppConfig,
) -> Option<AuthUser> {
    let provided_token = extract_header_string(headers, INTERNAL_SERVICE_TOKEN_HEADER)?;
    let expected_token = config.internal_service_token()?;
    if !constant_time_eq(&provided_token, &expected_token) {
        return None;
    }
    Some(AuthUser {
        user_id: INTERNAL_SERVICE_USER_ID.to_string(),
        email: Some("internal@allternit.local".to_string()),
        name: Some("Internal Service".to_string()),
        avatar_url: None,
        tenant_id: Some(config.tenant_id()),
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    })
}

/// Auth middleware — verifies Clerk JWT and adds user context to request headers.
/// In local development, falls back to `x-allternit-user-id` header for testing.
pub async fn auth_middleware(
    State(state): State<Arc<crate::AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    // 1. Desktop bootstrap shared-secret (signed desktop process or trusted peer).
    if let Some(mut user) = extract_desktop_bootstrap_user(request.headers(), &state.config) {
        match ensure_user_in_db(&state.db, &user) {
            Ok(organization_id) => user.organization_id = organization_id,
            Err(e) => return e.into_response(),
        }

        let headers = request.headers_mut();
        insert_user_headers(headers, &user);
        request.extensions_mut().insert(user);
        return next.run(request).await;
    }

    // 2. Self-hosted setup token (onboarding wizard on a headless VPS).
    if let Some(mut user) = extract_self_hosted_setup_user(request.headers(), &state.config) {
        match ensure_user_in_db(&state.db, &user) {
            Ok(organization_id) => user.organization_id = organization_id,
            Err(e) => return e.into_response(),
        }

        let headers = request.headers_mut();
        insert_user_headers(headers, &user);
        request.extensions_mut().insert(user);
        return next.run(request).await;
    }

    // 2b. Internal service token (service-to-service calls, e.g. health probes).
    if let Some(mut user) = extract_internal_service_user(request.headers(), &state.config) {
        match ensure_user_in_db(&state.db, &user) {
            Ok(organization_id) => user.organization_id = organization_id,
            Err(e) => return e.into_response(),
        }

        let headers = request.headers_mut();
        insert_user_headers(headers, &user);
        request.extensions_mut().insert(user);
        return next.run(request).await;
    }

    // 2c. Cloud-issued data-plane JWT (decision A1): the second hop of the
    // control-plane → data-plane model. cloud-api mints a short-lived EdDSA
    // JWT (`sub` = Clerk user id, `aud` = this node's device id) and relays
    // user calls with it; `state.dp_jwks` verifies it against cloud-api's
    // published JWKS. Anything that is not a valid DP token (Clerk session
    // JWTs, access tokens, garbage) returns `None` and falls through to the
    // bearer paths below, which are unchanged. The `at-` / `allternit_*`
    // prefixes are skipped up front so those lookups never touch the DP
    // JWKS cache.
    if let Some(token) = extract_bearer_token(request.headers()) {
        if !token.starts_with("at-") && !token.starts_with("allternit_") {
            if let Some(claims) = state.dp_jwks.authenticate(&token).await {
                let mut user = AuthUser {
                    user_id: claims.sub,
                    email: None,
                    name: None,
                    avatar_url: None,
                    tenant_id: None,
                    organization_id: None,
                    organization_role: None,
                    organization_slug: None,
                };
                match ensure_user_in_db(&state.db, &user) {
                    Ok(organization_id) => user.organization_id = organization_id,
                    Err(e) => return e.into_response(),
                }
                let headers = request.headers_mut();
                insert_user_headers(headers, &user);
                request.extensions_mut().insert(user);
                return next.run(request).await;
            }
        }
    }

    // 4. Cloud-issued runtime-device token (`Authorization: Bearer
    // allternit_runtime_…`), the same mechanism already proven for
    // `mcp_proxy_internal`/`/internal/*`. Introspected against
    // allternit-cloud-api; the token-derived user_id is the identity.
    // Only run when a cloud-api URL is configured; without one the token is
    // not verifiable, and self-hosted/local fallbacks below take over.
    if state.config.cloud_api_url().is_some() {
        if let Some(token) = crate::connector_routes::device_token_from_headers(request.headers()) {
            let token = token.to_string();
            return match crate::connector_routes::verify_runtime_device_token(&state, &token).await {
                Ok(identity) => {
                    let header_email = request
                        .headers()
                        .get(USER_EMAIL_HEADER)
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    let header_name = request
                        .headers()
                        .get("x-allternit-user-name")
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    let header_org = request
                        .headers()
                        .get("x-allternit-tenant-id")
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    // The role comes only from the cloud-verified identity
                    // (the owner's Clerk memberships), never from a header.
                    let organization_role = header_org
                        .as_deref()
                        .and_then(|org| identity.role_in(org))
                        .map(str::to_string);
                    let email = header_email
                        .filter(|value| !value.contains("@users.allternit.local"))
                        .or(identity.email);
                    let mut user = AuthUser {
                        user_id: identity.user_id,
                        email,
                        name: header_name,
                        avatar_url: None,
                        tenant_id: header_org.clone(),
                        organization_id: header_org,
                        organization_role,
                        organization_slug: None,
                    };
                    match ensure_user_in_db(&state.db, &user) {
                        Ok(organization_id) => user.organization_id = organization_id,
                        Err(e) => return e.into_response(),
                    }
                    let headers = request.headers_mut();
                    insert_user_headers(headers, &user);
                    request.extensions_mut().insert(user);
                    next.run(request).await
                }
                Err(resp) => resp,
            };
        }
    }

    // 3. Clerk JWT bearer token.
    if let Some(token) = extract_bearer_token(request.headers()) {
        // Fabric Transport worker credentials (atok_…) are principal tokens,
        // not user JWTs — the fabric routes authenticate them themselves
        // (`sqlite_store::authenticate_principal`) against the hashed store.
        // Passing them through unverified here is safe: any route that needs
        // a user identity still 401s on the absent user context, and without
        // this the managed worker would be rejected before reaching its
        // route in packaged (non-bypass) deployments.
        if token.starts_with("atok_") {
            return next.run(request).await;
        }
        if token.starts_with("at-") {
            let db = state.db.clone();
            let token_for_lookup = token.clone();
            return match tokio::task::spawn_blocking(move || {
                crate::admin_access_token_routes::authenticate_access_token(&db, &token_for_lookup)
            })
            .await
            {
                Ok(Some(user)) => {
                    insert_user_headers(request.headers_mut(), &user);
                    request.extensions_mut().insert(user);
                    next.run(request).await
                }
                Ok(None) => AuthError::TokenDecode("Unknown, revoked, or expired access token".into()).into_response(),
                Err(e) => AuthError::DbError(e.to_string()).into_response(),
            };
        }

        if token.starts_with("allternit_admin_") || token.starts_with("allternit_access_") {
            let db = state.db.clone();
            let token_for_lookup = token.clone();
            match tokio::task::spawn_blocking(move || crate::enterprise_auth::authenticate_bearer(&db, &token_for_lookup)).await {
                Ok(Ok(Some((user, credential)))) => {
                    if !credential.allows_request(request.method(), request.uri().path()) {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({
                                "error": "Forbidden",
                                "message": "Credential does not grant the required scope"
                            })),
                        )
                            .into_response();
                    }
                    insert_user_headers(request.headers_mut(), &user);
                    request.extensions_mut().insert(user);
                    request.extensions_mut().insert(credential);
                    return next.run(request).await;
                }
                Ok(Ok(None)) => return AuthError::TokenDecode("Unknown, revoked, or expired enterprise credential".into()).into_response(),
                Ok(Err(e)) => return AuthError::DbError(e.to_string()).into_response(),
                Err(e) => return AuthError::DbError(e.to_string()).into_response(),
            }
        }
        match verify_token(&state.jwks, &token, &state.auth_config).await {
            Ok(mut user) => {
                // Sync only the signed active Clerk organization into the
                // request and local membership tables.
                match ensure_user_in_db(&state.db, &user) {
                    Ok(organization_id) => user.organization_id = organization_id,
                    Err(e) => return e.into_response(),
                }

                // Add user context as headers for backward compatibility
                // (handlers should prefer Extension<AuthUser> over header extraction)
                let headers = request.headers_mut();
                insert_user_headers(headers, &user);

                // Also attach to extensions for any middleware that needs it
                request.extensions_mut().insert(user);
                return next.run(request).await;
            }
            Err(e) => {
                // Local/self-hosted fallback: if the token is invalid but the
                // request is clearly from the local app, continue to the
                // localhost bypass below instead of hard-failing.
                if (state.config.local_dev_bypass() || state.config.self_hosted())
                    && is_localhost_origin(request.headers())
                {
                    warn!(error = %e, "JWT verification failed; falling back to local user");
                } else {
                    return e.into_response();
                }
            }
        }
    }

    // 4b. Desktop WebSocket token (`/ws/*?token=…`). Browser WebSocket
    // handshakes cannot carry the Clerk bearer — header injection applies to
    // fetches, not upgrade requests — so the VNC stream was unreachable: every
    // handshake 401'd at this middleware before the route could validate its
    // own short-lived HMAC-signed desktop token (live defect, bote2e-0913:
    // Observe/Take Over rendered nothing; external clients with a valid token
    // also got 401). Authenticate /ws/* requests that carry a valid desktop
    // token by injecting the claimed user; the ws route re-verifies the
    // claims against the path. Invalid tokens fall through to the existing
    // fallbacks (unchanged behavior).
    if request.uri().path().starts_with("/ws/") {
        if let Some(user) = auth_user_from_desktop_ws_token(&state, request.uri()) {
            insert_user_headers(request.headers_mut(), &user);
            request.extensions_mut().insert(user);
            return next.run(request).await;
        }
    }

    // 5. Self-hosted / local-dev fallback: when the deployment is local and no
    // cloud auth succeeded, trust the loopback origin as the default local user.
    // This keeps packaged apps and browser dev flows working without Clerk tokens.
    if (state.config.self_hosted() || state.config.local_dev_bypass())
        && is_localhost_origin(request.headers())
    {
        let mut user = local_dev_user(Some(state.config.tenant_id()));
        match ensure_user_in_db(&state.db, &user) {
            Ok(organization_id) => user.organization_id = organization_id,
            Err(e) => return e.into_response(),
        }
        let headers = request.headers_mut();
        insert_user_headers(headers, &user);
        request.extensions_mut().insert(user);
        return next.run(request).await;
    }

    AuthError::MissingToken.into_response()
}

/// Authenticate a `/ws/*` request via the desktop WebSocket token query param.
///
/// Returns `Some(AuthUser)` when the request carries a well-formed,
/// HMAC-valid, unexpired desktop token; `None` otherwise (caller falls
/// through to the remaining auth fallbacks). Pure token verification — the
/// ws route re-checks that the claims match the request path and user.
fn auth_user_from_desktop_ws_token(
    state: &Arc<crate::AppState>,
    uri: &axum::http::Uri,
) -> Option<AuthUser> {
    let query = uri.query()?;
    let token = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == "token")
        .map(|(_, v)| v)?;
    let secret = crate::bot_desktop_stream::desktop_ws_secret(state)?;
    user_from_desktop_token(&secret, token)
}

/// Pure token → user lift: verifies the HMAC signature and expiry, then maps
/// the claims to an AuthUser. Split from the request extraction so it is
/// unit-testable without an AppState.
fn user_from_desktop_token(secret: &str, token: &str) -> Option<AuthUser> {
    let claims = crate::bot_desktop_stream::verify_desktop_token(secret, token).ok()?;
    Some(AuthUser {
        user_id: claims.user_id,
        email: None,
        name: None,
        avatar_url: None,
        tenant_id: None,
        organization_id: None,
        organization_role: None,
        organization_slug: None,
    })
}

/// Optional auth middleware — verifies token if present, but allows anonymous requests
pub async fn optional_auth_middleware(
    State(state): State<Arc<crate::AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(token) = extract_bearer_token(request.headers()) {
        if let Ok(mut user) = verify_token(&state.jwks, &token, &state.auth_config).await {
            if let Ok(organization_id) = ensure_user_in_db(&state.db, &user) {
                user.organization_id = organization_id;
            }
            request.extensions_mut().insert(user);
        }
    }

    next.run(request).await
}

// ─── Header-based auth extraction (works around multiple axum versions) ─────

/// Extract AuthUser from request headers set by auth_middleware.
///
/// **Deprecated**: Now that the axum version conflict is resolved, handlers should
/// use `Extension<AuthUser>` instead. This function is kept for backward
/// compatibility with any code that hasn't been migrated yet.
pub fn get_user(headers: &HeaderMap) -> Option<AuthUser> {
    let user_id = headers.get("x-allternit-user-id")?.to_str().ok()?;
    let email = headers
        .get("x-allternit-user-email")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let name = headers
        .get("x-allternit-user-name")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let tenant_id = headers
        .get("x-allternit-tenant-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let organization_id = headers
        .get(ORGANIZATION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| tenant_id.clone());
    Some(AuthUser {
        user_id: user_id.to_string(),
        email,
        name,
        avatar_url: None,
        tenant_id,
        organization_id,
        organization_role: headers
            .get(ORGANIZATION_ROLE_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        organization_slug: headers
            .get(ORGANIZATION_SLUG_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
    })
}

/// Test-only Clerk: signs RS256 session tokens with a key it seeds into a
/// [`JwksManager`]'s cache, so tests exercise the real `verify_token` path.
#[cfg(test)]
pub(crate) async fn test_clerk_token(jwks: &JwksManager, issuer: &str, sub: &str, expires_in: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    test_clerk_token_with_claims(jwks, json!({"iss": issuer, "sub": sub, "exp": now + expires_in, "iat": now})).await
}

/// Like [`test_clerk_token`] but signs an arbitrary claim set (OAuth `aud`/`scope` tests).
#[cfg(test)]
pub(crate) async fn test_clerk_token_with_claims(jwks: &JwksManager, claims: serde_json::Value) -> String {
    use aws_lc_rs::rsa::{KeyPair, KeySize, PublicKeyComponents};
    use aws_lc_rs::signature::KeyPair as _;
    let key_pair = KeyPair::generate(KeySize::Rsa2048).expect("rsa key");
    let components: PublicKeyComponents<Vec<u8>> = key_pair.public_key().into();
    let kid = format!("test-{}", uuid::Uuid::new_v4());
    let jwk = JwkKey {
        kid: kid.clone(),
        kty: "RSA".into(),
        key_use: Some("sig".into()),
        n: Some(URL_SAFE_NO_PAD.encode(&components.n)),
        e: Some(URL_SAFE_NO_PAD.encode(&components.e)),
        extra: HashMap::new(),
    };
    {
        let mut cache = jwks.cache.write().await;
        let cached = cache.get_or_insert_with(|| CachedJwks { keys: HashMap::new(), fetched_at: Instant::now() });
        cached.keys.insert(kid.clone(), jwk);
        cached.fetched_at = Instant::now();
    }
    let header = URL_SAFE_NO_PAD.encode(json!({"alg": "RS256", "typ": "JWT", "kid": kid}).to_string());
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signing_input = format!("{header}.{payload}");
    let mut signature = vec![0; key_pair.public_modulus_len()];
    key_pair
        .sign(&aws_lc_rs::signature::RSA_PKCS1_SHA256, &aws_lc_rs::rand::SystemRandom::new(), signing_input.as_bytes(), &mut signature)
        .expect("sign");
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

#[cfg(test)]
mod desktop_ws_token_tests {
    use super::user_from_desktop_token;

    const TEST_SECRET: &str = "test-desktop-ws-secret";

    #[test]
    fn valid_desktop_token_maps_to_claimed_user() {
        let token = crate::bot_desktop_stream::sign_desktop_token(
            TEST_SECRET,
            "bot-1",
            "sandbox-1",
            "user-9",
            300,
        );
        let user = user_from_desktop_token(TEST_SECRET, &token).expect("valid token must authenticate");
        assert_eq!(user.user_id, "user-9");
        assert!(user.organization_id.is_none());
    }

    #[test]
    fn wrong_secret_rejects_token() {
        let token = crate::bot_desktop_stream::sign_desktop_token(
            TEST_SECRET,
            "bot-1",
            "sandbox-1",
            "user-9",
            300,
        );
        assert!(user_from_desktop_token("a-different-secret", &token).is_none());
    }

    #[test]
    fn expired_token_rejects() {
        let token = crate::bot_desktop_stream::sign_desktop_token(
            TEST_SECRET,
            "bot-1",
            "sandbox-1",
            "user-9",
            0,
        );
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(user_from_desktop_token(TEST_SECRET, &token).is_none());
    }

    #[test]
    fn malformed_token_rejects() {
        assert!(user_from_desktop_token(TEST_SECRET, "not-a-token").is_none());
        assert!(user_from_desktop_token(TEST_SECRET, "").is_none());
    }
}

#[cfg(test)]
mod org_role_sync_tests {
    use super::{ensure_user_in_db, AuthUser};

    fn user(role: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: "u-owner".into(),
            email: Some("owner@test.local".into()),
            name: None,
            avatar_url: None,
            tenant_id: Some("org-1".into()),
            organization_id: Some("org-1".into()),
            organization_role: role.map(str::to_string),
            organization_slug: None,
        }
    }

    fn role_of(db: &crate::db::DbHandle) -> String {
        db.connect().unwrap()
            .query_row("SELECT role FROM organization_members WHERE organization_id = 'org-1' AND user_id = 'u-owner'", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn a_path_without_a_role_never_downgrades_a_known_role() {
        let temp = tempfile::tempdir().unwrap();
        let db = crate::db::DbHandle::new(temp.path().join("t.db")).unwrap();
        // First seen through a path with no role: a plain member.
        ensure_user_in_db(&db, &user(None)).unwrap();
        assert_eq!(role_of(&db), "member");
        // A Clerk session carrying the role sets it.
        ensure_user_in_db(&db, &user(Some("org:admin"))).unwrap();
        assert_eq!(role_of(&db), "admin");
        // A device/DP token (no role) must not downgrade it again.
        ensure_user_in_db(&db, &user(None)).unwrap();
        assert_eq!(role_of(&db), "admin");
        ensure_user_in_db(&db, &user(Some(""))).unwrap();
        assert_eq!(role_of(&db), "admin");
        // A real role change still applies.
        ensure_user_in_db(&db, &user(Some("org:member"))).unwrap();
        assert_eq!(role_of(&db), "member");
    }
}
