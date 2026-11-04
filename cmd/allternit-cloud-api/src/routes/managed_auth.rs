//! Managed connector auth: Allternit as an ID-JAG issuer (docs/MANAGED_AUTH_MAP.md WP1).
//!
//! An org admin turns on managed authorization for a connector. Members then
//! connect with no sign-in screen: Allternit signs an Identity Assertion JWT
//! Authorization Grant (ID-JAG, MCP extension
//! `io.modelcontextprotocol/enterprise-managed-authorization`) for the user,
//! the connector's authorization server trusts the org's Allternit issuer URL
//! and answers the RFC 7523 JWT-bearer grant with an access token.
//!
//! Issuer per Clerk organization: `<ALLTERNIT_IDJAG_ISSUER_BASE>/oauth2/org/<org>`
//! (default base `https://api.allternit.com`):
//! - `GET <issuer>/.well-known/oauth-authorization-server`, `…/openid-configuration`
//!   (also at the RFC 8414 path `/.well-known/oauth-authorization-server/oauth2/org/<org>`)
//! - `GET <issuer>/jwks.json`
//! - `POST <issuer>/token`: RFC 8693 token exchange for Clerk-session callers
//!   (`subject_token` = Clerk session JWT, `requested_token_type` =
//!   `urn:ietf:params:oauth:token-type:id-jag`, `audience` = vendor AS issuer,
//!   optional `resource`, `scope`).
//!
//! Runtime (allternit-api on the user's computer; the runtime device
//! credential, either the relay signature or `Bearer allternit_runtime_…`):
//! - `GET  /api/v1/managed-auth/configs`: enabled configs the user may use. No secrets.
//! - `POST /api/v1/managed-auth/id-jag` `{connector_id, org_id?}`: an ID-JAG plus the
//!   vendor client credentials for the RFC 7523 grant (secret only over this channel).
//!
//! Admin (Clerk session; writes need Clerk role `org:admin` in that org; members read):
//! - `GET /api/v1/orgs/:org/managed-auth` (+ `issuer_url`), `GET|PUT|DELETE …/:connector_id`.
//!
//! An ID-JAG is signed only when: the user is a member of the org, an enabled
//! `managed_auth_configs` row exists, the member's role is in its `roles`, and
//! the user's primary email is verified in Clerk. Every issuance is logged in
//! `idjag_issuance_log`. Without `ALLTERNIT_IDJAG_ES256_PEM` every issuer route
//! answers 503 `idjag_not_configured`. Vendor client secrets are sealed with
//! the credential cipher and never logged or returned to the browser.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::es256;
use crate::ApiState;

pub const ISSUER_BASE_ENV: &str = "ALLTERNIT_IDJAG_ISSUER_BASE";
const DEFAULT_ISSUER_BASE: &str = "https://api.allternit.com";
pub const IDJAG_TYP: &str = "oauth-id-jag+jwt";
pub const IDJAG_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:id-jag";
const TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
pub const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
/// ID-JAG lifetime (the spec caps it at 300 s).
pub const IDJAG_TTL_SECS: i64 = 300;
pub const CONFIGS_PATH: &str = "/api/v1/managed-auth/configs";
pub const IDJAG_PATH: &str = "/api/v1/managed-auth/id-jag";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/oauth2/org/:org/.well-known/oauth-authorization-server", get(org_metadata))
        .route("/oauth2/org/:org/.well-known/openid-configuration", get(org_metadata))
        .route("/oauth2/org/:org/jwks.json", get(org_jwks))
        .route("/oauth2/org/:org/token", post(org_token))
        .route("/.well-known/oauth-authorization-server/*rest", get(metadata_at))
        .route("/.well-known/openid-configuration/*rest", get(metadata_at))
        .route(CONFIGS_PATH, get(runtime_configs))
        .route(IDJAG_PATH, post(runtime_idjag))
        .route("/api/v1/orgs/:org/managed-auth", get(admin_list))
        .route("/api/v1/orgs/:org/managed-auth/:connector_id", get(admin_get).put(admin_put).delete(admin_delete))
}

// ─── issuer URLs and metadata ─────────────────────────────────────────────────

pub fn issuer_base() -> String {
    std::env::var(ISSUER_BASE_ENV).ok().map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT_ISSUER_BASE.to_string())
}

pub fn org_issuer(org: &str) -> String {
    format!("{}/oauth2/org/{org}", issuer_base())
}

/// A Clerk organization id (`org_…`): the only thing allowed into an issuer path.
pub fn valid_org(org: &str) -> bool {
    !org.is_empty() && org.len() <= 64 && org.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn org_metadata_doc(org: &str) -> Value {
    let issuer = org_issuer(org);
    json!({
        "issuer": issuer,
        "jwks_uri": format!("{issuer}/jwks.json"),
        "token_endpoint": format!("{issuer}/token"),
        "grant_types_supported": [TOKEN_EXCHANGE],
        "token_endpoint_auth_methods_supported": ["none"],
        "requested_token_types_supported": [IDJAG_TOKEN_TYPE],
        "id_token_signing_alg_values_supported": ["ES256"],
        "subject_types_supported": ["public"],
        "response_types_supported": [],
        "service_documentation": "https://docs.allternit.com/dev/idjag-issuer"
    })
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    let mut resp = (status, Json(json!({ "error": error, "error_description": description }))).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    resp
}

/// The contract's error body: `{ "error": { "code", "message" } }`.
pub(crate) fn coded(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
}

fn idjag_not_configured() -> Response {
    coded(StatusCode::SERVICE_UNAVAILABLE, "idjag_not_configured", "Managed authorization is not set up on this server.")
}

async fn org_metadata(Path(org): Path<String>) -> Response {
    if es256::key_from_env().is_none() {
        return idjag_not_configured();
    }
    if !valid_org(&org) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    }
    Json(org_metadata_doc(&org)).into_response()
}

async fn org_jwks(Path(org): Path<String>) -> Response {
    let Some(key) = es256::key_from_env() else { return idjag_not_configured() };
    if !valid_org(&org) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    }
    let mut resp = Json(key.jwks()).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("public, max-age=300"));
    resp
}

/// RFC 8414 §3.1 form: `/.well-known/oauth-authorization-server/oauth2/org/<org>`
/// and `…/oauth2/edge` (the edge authorization server, `routes::edge_auth`).
async fn metadata_at(Path(rest): Path<String>) -> Response {
    if es256::key_from_env().is_none() {
        return idjag_not_configured();
    }
    let rest = rest.trim_matches('/');
    if rest == "oauth2/edge" {
        return Json(crate::routes::edge_auth::as_metadata()).into_response();
    }
    match rest.strip_prefix("oauth2/org/") {
        Some(org) if valid_org(org) => Json(org_metadata_doc(org)).into_response(),
        _ => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
    }
}

// ─── configs ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConfigRow {
    pub org_id: String,
    pub connector_id: String,
    pub as_issuer: String,
    pub token_endpoint: String,
    pub resource: String,
    pub client_id: String,
    pub client_secret_sealed: Option<String>,
    pub roles: Vec<String>,
    pub scopes: Vec<String>,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}

const CONFIG_COLUMNS: &str = "org_id, connector_id, as_issuer, token_endpoint, resource, client_id, client_secret_sealed, roles, scopes, enabled, updated_at";

/// Clerk names the admin role `org:admin` (older instances: `admin`).
pub fn normalize_role(role: &str) -> String {
    let role = role.trim();
    if role.starts_with("org:") {
        role.to_string()
    } else {
        format!("org:{role}")
    }
}

pub fn is_admin_role(role: &str) -> bool {
    normalize_role(role) == "org:admin"
}

fn role_allowed(role: &str, roles: &[String]) -> bool {
    let role = normalize_role(role);
    roles.iter().any(|r| normalize_role(r) == role)
}

// ─── the issuance decision ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotConfigured,
    NotMember,
    RoleNotAllowed,
    EmailUnverified,
    IdjagNotConfigured,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::NotConfigured => "not_configured",
            Refusal::NotMember => "not_member",
            Refusal::RoleNotAllowed => "role_not_allowed",
            Refusal::EmailUnverified => "email_unverified",
            Refusal::IdjagNotConfigured => "idjag_not_configured",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Refusal::NotConfigured => "Your organization has not turned on managed sign-in for this connector.",
            Refusal::NotMember => "You are not a member of this organization.",
            Refusal::RoleNotAllowed => "Your role in this organization is not allowed to use managed sign-in for this connector. Ask an org admin.",
            Refusal::EmailUnverified => "Verify your primary email address in your Allternit account, then try again.",
            Refusal::IdjagNotConfigured => "Managed authorization is not set up on this server.",
        }
    }
    fn status(self) -> StatusCode {
        match self {
            Refusal::NotConfigured => StatusCode::NOT_FOUND,
            Refusal::IdjagNotConfigured => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::FORBIDDEN,
        }
    }
}

/// What we know about the user asking, for one org.
pub struct Subject<'a> {
    /// Role in the org; `None` = not a member.
    pub role: Option<&'a str>,
    /// Primary email and whether Clerk verified it.
    pub primary_email: Option<(&'a str, bool)>,
}

/// Whether an ID-JAG may be signed. Membership is checked first so a
/// non-member learns nothing about the org's configs.
pub fn decide(config: Option<&ConfigRow>, subject: &Subject) -> Result<(), Refusal> {
    let Some(role) = subject.role else { return Err(Refusal::NotMember) };
    let Some(config) = config.filter(|c| c.enabled) else { return Err(Refusal::NotConfigured) };
    if !role_allowed(role, &config.roles) {
        return Err(Refusal::RoleNotAllowed);
    }
    match subject.primary_email {
        Some((email, true)) if !email.is_empty() => Ok(()),
        _ => Err(Refusal::EmailUnverified),
    }
}

/// The granted scope string: the requested scopes when all are configured,
/// else `None` (invalid_scope); nothing requested = every configured scope.
pub fn grant_scope(requested: Option<&str>, configured: &[String]) -> Option<String> {
    match requested.map(str::split_whitespace) {
        Some(req) => {
            let req: Vec<&str> = req.collect();
            if req.is_empty() {
                return Some(configured.join(" "));
            }
            req.iter().all(|s| configured.iter().any(|c| c == s)).then(|| req.join(" "))
        }
        None => Some(configured.join(" ")),
    }
}

/// ID-JAG claims (draft-ietf-oauth-identity-assertion-authz-grant §3).
pub fn idjag_claims(org: &str, config: &ConfigRow, user_id: &str, email: &str, scope: &str, jti: &str, now: i64) -> Value {
    let mut claims = json!({
        "iss": org_issuer(org),
        "sub": user_id,
        "email": email,
        "email_verified": true,
        "aud": config.as_issuer,
        "client_id": config.client_id,
        "resource": config.resource,
        "jti": jti,
        "iat": now,
        "exp": now + IDJAG_TTL_SECS,
    });
    if !scope.is_empty() {
        claims["scope"] = json!(scope);
    }
    claims
}

pub struct Issued {
    pub id_jag: String,
    pub scope: String,
}

enum IssueError {
    Refused(Refusal),
    InvalidScope,
    Unavailable(&'static str),
}

/// Check, log and sign. `role` is the caller's role in `org` when already
/// known from the session; otherwise Clerk is asked (fail closed).
async fn issue(state: &ApiState, user_id: &str, org: &str, config: Option<&ConfigRow>, known_role: Option<String>, scope: Option<&str>) -> Result<Issued, IssueError> {
    let Some(key) = es256::key_from_env() else { return Err(IssueError::Refused(Refusal::IdjagNotConfigured)) };
    let role = match known_role {
        Some(r) => Some(r),
        None => crate::services::user_trust::clerk_org_memberships_checked(user_id).await.ok_or(IssueError::Unavailable("Organization membership could not be checked right now."))?.into_iter().find(|m| m.id == org).map(|m| m.role),
    };
    // Skip the email lookup for a refusal that doesn't need it.
    decide(config, &Subject { role: role.as_deref(), primary_email: Some(("-", true)) }).map_err(IssueError::Refused)?;
    let email = crate::services::user_trust::clerk_primary_email_status(user_id).await.map_err(|_| IssueError::Unavailable("Your email address could not be checked right now."))?;
    decide(config, &Subject { role: role.as_deref(), primary_email: email.as_ref().map(|(e, v)| (e.as_str(), *v)) }).map_err(IssueError::Refused)?;
    let (config, (email, _)) = (config.expect("decide checked"), email.expect("decide checked"));
    let scope = grant_scope(scope, &config.scopes).ok_or(IssueError::InvalidScope)?;
    let jti = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().timestamp();
    sqlx::query("INSERT INTO idjag_issuance_log (jti, org_id, user_id, aud, resource) VALUES ($1, $2, $3, $4, $5)")
        .bind(&jti)
        .bind(org)
        .bind(user_id)
        .bind(&config.as_issuer)
        .bind(&config.resource)
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("managed auth: issuance log write failed: {e}");
            IssueError::Unavailable("The sign-in could not be recorded right now.")
        })?;
    tracing::info!(org = %org, user = %user_id, connector = %config.connector_id, aud = %config.as_issuer, jti = %jti, "managed auth: ID-JAG issued");
    let id_jag = key.sign(IDJAG_TYP, &idjag_claims(org, config, user_id, &email, &scope, &jti, now));
    Ok(Issued { id_jag, scope })
}

// ─── POST <issuer>/token (RFC 8693, Clerk session) ────────────────────────────

/// A form (or JSON) body as string pairs.
pub(crate) fn form_fields(headers: &HeaderMap, body: &[u8]) -> std::collections::HashMap<String, String> {
    let is_json = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.contains("json"));
    if is_json {
        return serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .map(|o| o.into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string()))).collect())
            .unwrap_or_default();
    }
    url::form_urlencoded::parse(body).into_owned().collect()
}

async fn org_token(State(state): State<Arc<ApiState>>, Path(org): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    if es256::key_from_env().is_none() {
        return idjag_not_configured();
    }
    if !valid_org(&org) {
        return oauth_error(StatusCode::NOT_FOUND, "invalid_request", "Unknown issuer");
    }
    let f = form_fields(&headers, &body);
    let field = |k: &str| f.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
    if field("grant_type") != Some(TOKEN_EXCHANGE) {
        return oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type", "Use urn:ietf:params:oauth:grant-type:token-exchange");
    }
    if field("requested_token_type") != Some(IDJAG_TOKEN_TYPE) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "requested_token_type must be urn:ietf:params:oauth:token-type:id-jag");
    }
    let (Some(subject_token), Some(audience)) = (field("subject_token"), field("audience")) else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "subject_token and audience are required");
    };
    let user = match crate::auth::clerk::user_from_token(subject_token).await {
        Ok(u) => u,
        Err(_) => return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "The subject token is not a valid Allternit session"),
    };
    let rows: Vec<ConfigRow> = match sqlx::query_as(&format!("SELECT {CONFIG_COLUMNS} FROM managed_auth_configs WHERE org_id = $1 AND rtrim(as_issuer, '/') = rtrim($2, '/') AND enabled"))
        .bind(&org)
        .bind(audience)
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("managed auth: config lookup failed: {e}");
            return oauth_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "Try again shortly");
        }
    };
    let norm = |s: &str| s.trim_end_matches('/').to_string();
    let matching: Vec<&ConfigRow> = match field("resource") {
        Some(r) => rows.iter().filter(|c| norm(&c.resource) == norm(r)).collect(),
        None => rows.iter().collect(),
    };
    if matching.len() > 1 {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_target", "Several connectors use this authorization server; name the resource");
    }
    // The session's own org role is fresh; another org is looked up in Clerk.
    let known_role = (user.organization_id.as_deref() == Some(org.as_str())).then(|| user.org_role.clone()).flatten();
    match issue(&state, &user.id, &org, matching.first().copied(), known_role, field("scope")).await {
        Ok(issued) => {
            let mut resp = Json(json!({
                "access_token": issued.id_jag,
                "issued_token_type": IDJAG_TOKEN_TYPE,
                "token_type": "N_A",
                "expires_in": IDJAG_TTL_SECS,
                "scope": issued.scope,
            }))
            .into_response();
            resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
            resp
        }
        Err(IssueError::Refused(Refusal::NotConfigured)) => oauth_error(StatusCode::BAD_REQUEST, "invalid_target", Refusal::NotConfigured.message()),
        Err(IssueError::Refused(Refusal::IdjagNotConfigured)) => idjag_not_configured(),
        Err(IssueError::Refused(r)) => oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", r.message()),
        Err(IssueError::InvalidScope) => oauth_error(StatusCode::BAD_REQUEST, "invalid_scope", "A requested scope is not allowed for this connector"),
        Err(IssueError::Unavailable(m)) => oauth_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", m),
    }
}

// ─── runtime endpoints ────────────────────────────────────────────────────────

/// The user behind a runtime device credential: the relay signature
/// (`x-allternit-runtime-id/-ts/-sig`, what allternit-api signs its cloud
/// calls with) or `Authorization: Bearer allternit_runtime_…`.
async fn runtime_owner(state: &ApiState, headers: &HeaderMap, method: &str, path: &str, body: &[u8]) -> Result<String, Response> {
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim);
    if let Some(token) = bearer.filter(|t| t.starts_with(crate::routes::runtime_pairing::DEVICE_TOKEN_PREFIX)) {
        return crate::routes::runtime_pairing::runtime_device_for_token(&state.db, token, None)
            .await
            .map(|r| r.user_id)
            .map_err(|_| coded(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid runtime credential."));
    }
    crate::routes::runtime_events::authenticate_runtime(&state.db, headers, method, path, body, Utc::now().timestamp())
        .await
        .map(|(owner, _)| owner)
        .map_err(|_| coded(StatusCode::UNAUTHORIZED, "unauthorized", "A runtime credential is required."))
}

fn unavailable(message: &str) -> Response {
    coded(StatusCode::SERVICE_UNAVAILABLE, "unavailable", message)
}

async fn runtime_configs(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user_id = match runtime_owner(&state, &headers, "GET", CONFIGS_PATH, b"").await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(memberships) = crate::services::user_trust::clerk_org_memberships_checked(&user_id).await else {
        return unavailable("Organization membership could not be checked right now.");
    };
    let orgs: Vec<String> = memberships.iter().map(|m| m.id.clone()).collect();
    let rows: Vec<ConfigRow> = match sqlx::query_as(&format!("SELECT {CONFIG_COLUMNS} FROM managed_auth_configs WHERE enabled AND org_id = ANY($1) ORDER BY org_id, connector_id"))
        .bind(&orgs)
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("managed auth: config list failed: {e}");
            return unavailable("Try again shortly.");
        }
    };
    let configs: Vec<Value> = rows
        .iter()
        .filter(|c| memberships.iter().any(|m| m.id == c.org_id && role_allowed(&m.role, &c.roles)))
        .map(|c| json!({ "org_id": c.org_id, "connector_id": c.connector_id, "as_issuer": c.as_issuer, "token_endpoint": c.token_endpoint, "resource": c.resource, "scopes": c.scopes }))
        .collect();
    Json(json!({ "configs": configs })).into_response()
}

#[derive(Deserialize)]
struct IdJagBody {
    connector_id: String,
    #[serde(default)]
    org_id: Option<String>,
}

async fn runtime_idjag(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let user_id = match runtime_owner(&state, &headers, "POST", IDJAG_PATH, &body).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    if es256::key_from_env().is_none() {
        return idjag_not_configured();
    }
    let Ok(req) = serde_json::from_slice::<IdJagBody>(&body) else {
        return coded(StatusCode::BAD_REQUEST, "invalid_request", "Body must be {\"connector_id\", \"org_id\"?}.");
    };
    let Some(memberships) = crate::services::user_trust::clerk_org_memberships_checked(&user_id).await else {
        return unavailable("Organization membership could not be checked right now.");
    };
    let orgs: Vec<String> = match req.org_id.as_deref().filter(|o| !o.is_empty()) {
        Some(org) if memberships.iter().any(|m| m.id == org) => vec![org.to_string()],
        Some(_) => return coded(Refusal::NotMember.status(), Refusal::NotMember.code(), Refusal::NotMember.message()),
        None => memberships.iter().map(|m| m.id.clone()).collect(),
    };
    let rows: Vec<ConfigRow> = match sqlx::query_as(&format!("SELECT {CONFIG_COLUMNS} FROM managed_auth_configs WHERE connector_id = $1 AND enabled AND org_id = ANY($2)"))
        .bind(&req.connector_id)
        .bind(&orgs)
        .fetch_all(&state.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("managed auth: config lookup failed: {e}");
            return unavailable("Try again shortly.");
        }
    };
    if rows.len() > 1 {
        // Never guess between organizations.
        return coded(StatusCode::NOT_FOUND, "not_configured", "More than one of your organizations manages this connector; pass org_id.");
    }
    let Some(config) = rows.into_iter().next() else {
        return coded(Refusal::NotConfigured.status(), Refusal::NotConfigured.code(), Refusal::NotConfigured.message());
    };
    let role = memberships.iter().find(|m| m.id == config.org_id).map(|m| m.role.clone());
    let issued = match issue(&state, &user_id, &config.org_id, Some(&config), role, None).await {
        Ok(i) => i,
        Err(IssueError::Refused(r)) => return coded(r.status(), r.code(), r.message()),
        Err(IssueError::InvalidScope) => return coded(StatusCode::BAD_REQUEST, "invalid_request", "Invalid scope."),
        Err(IssueError::Unavailable(m)) => return unavailable(m),
    };
    let secret = match config.client_secret_sealed.as_deref() {
        None => None,
        Some(sealed) => match state.credential_cipher.as_ref().map(|c| c.decrypt(sealed)) {
            Some(Ok(s)) => Some(s),
            _ => return unavailable("The connector's client secret could not be read on this server."),
        },
    };
    let mut out = json!({ "id_jag": issued.id_jag, "token_endpoint": config.token_endpoint, "client_id": config.client_id, "expires_in": IDJAG_TTL_SECS });
    if let Some(secret) = secret {
        out["client_secret"] = json!(secret);
    }
    let mut resp = Json(out).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    resp
}

// ─── admin API (Clerk session; writes need org:admin) ─────────────────────────

pub(crate) struct OrgCaller {
    pub user_id: String,
    pub is_admin: bool,
}

/// The Clerk-session caller's standing in `org`, checked server-side on every
/// call: 401 without a session, 403 for a non-member, 503 when Clerk can't say.
pub(crate) async fn org_caller(headers: &HeaderMap, org: &str) -> Result<OrgCaller, Response> {
    if !valid_org(org) {
        return Err(coded(StatusCode::NOT_FOUND, "not_found", "Unknown organization."));
    }
    let user = crate::auth::clerk::user_from_headers(headers).await.map_err(|_| coded(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in to Allternit."))?;
    let role = match (user.organization_id.as_deref(), user.org_role.as_deref()) {
        (Some(o), Some(r)) if o == org => Some(r.to_string()),
        _ => crate::services::user_trust::clerk_org_memberships_checked(&user.id)
            .await
            .ok_or_else(|| unavailable("Organization membership could not be checked right now."))?
            .into_iter()
            .find(|m| m.id == org)
            .map(|m| m.role),
    };
    match role {
        Some(role) => Ok(OrgCaller { user_id: user.id, is_admin: is_admin_role(&role) }),
        None => Err(coded(StatusCode::FORBIDDEN, "not_member", "You are not a member of this organization.")),
    }
}

pub(crate) fn admin_required() -> Response {
    coded(StatusCode::FORBIDDEN, "admin_required", "Only an organization admin can change this.")
}

fn config_json(c: &ConfigRow) -> Value {
    json!({
        "connector_id": c.connector_id,
        "as_issuer": c.as_issuer,
        "token_endpoint": c.token_endpoint,
        "resource": c.resource,
        "client_id": c.client_id,
        "has_secret": c.client_secret_sealed.is_some(),
        "roles": c.roles,
        "scopes": c.scopes,
        "enabled": c.enabled,
        "updated_at": c.updated_at,
    })
}

fn db_error(e: sqlx::Error) -> Response {
    tracing::error!("managed auth: {e}");
    coded(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong; try again.")
}

async fn admin_list(State(state): State<Arc<ApiState>>, Path(org): Path<String>, headers: HeaderMap) -> Response {
    let caller = match org_caller(&headers, &org).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let rows: Vec<ConfigRow> = match sqlx::query_as(&format!("SELECT {CONFIG_COLUMNS} FROM managed_auth_configs WHERE org_id = $1 ORDER BY connector_id")).bind(&org).fetch_all(&state.db).await {
        Ok(r) => r,
        Err(e) => return db_error(e),
    };
    Json(json!({
        "issuer_url": org_issuer(&org),
        "issuer_configured": es256::key_from_env().is_some(),
        "is_admin": caller.is_admin,
        "configs": rows.iter().map(config_json).collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn admin_get(State(state): State<Arc<ApiState>>, Path((org, connector_id)): Path<(String, String)>, headers: HeaderMap) -> Response {
    if let Err(r) = org_caller(&headers, &org).await {
        return r;
    }
    match sqlx::query_as::<_, ConfigRow>(&format!("SELECT {CONFIG_COLUMNS} FROM managed_auth_configs WHERE org_id = $1 AND connector_id = $2")).bind(&org).bind(&connector_id).fetch_optional(&state.db).await {
        Ok(Some(c)) => Json(config_json(&c)).into_response(),
        Ok(None) => coded(StatusCode::NOT_FOUND, "not_found", "No managed sign-in is set for this connector."),
        Err(e) => db_error(e),
    }
}

#[derive(Deserialize)]
struct PutBody {
    as_issuer: String,
    token_endpoint: String,
    resource: String,
    client_id: String,
    /// Absent = keep the stored secret; `""` = remove it.
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    scopes: Vec<String>,
    enabled: bool,
}

/// `https://…`, or `http://` on a loopback host (a local test authorization server).
pub(crate) fn valid_url(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw.trim()) else { return false };
    let loopback = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    (u.scheme() == "https" || (u.scheme() == "http" && loopback)) && u.host_str().is_some() && u.username().is_empty() && u.password().is_none() && u.fragment().is_none() && raw.len() <= 512
}

pub(crate) fn valid_connector_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

/// Normalized roles and scopes, or the reason the body is refused.
fn validate_put(b: &PutBody) -> Result<(Vec<String>, Vec<String>), &'static str> {
    if !valid_url(&b.as_issuer) || !valid_url(&b.token_endpoint) || !valid_url(&b.resource) {
        return Err("as_issuer, token_endpoint and resource must be https URLs.");
    }
    if b.client_id.trim().is_empty() || b.client_id.len() > 512 {
        return Err("client_id is required.");
    }
    let mut roles: Vec<String> = b.roles.iter().map(|r| normalize_role(r)).collect();
    roles.sort();
    roles.dedup();
    if roles.is_empty() || roles.len() > 20 || roles.iter().any(|r| r.len() > 64 || !r[4..].bytes().all(|c| c.is_ascii_alphanumeric() || b"_:-".contains(&c)) || r.len() == 4) {
        return Err("Pick at least one role (org:admin, org:member or a custom org role).");
    }
    let scopes: Vec<String> = b.scopes.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if scopes.len() > 50 || scopes.iter().any(|s| s.len() > 200 || s.chars().any(char::is_whitespace)) {
        return Err("Scopes are single words without spaces.");
    }
    if b.client_secret.as_ref().is_some_and(|s| s.len() > 4096) {
        return Err("client_secret is too long.");
    }
    Ok((roles, scopes))
}

async fn admin_put(State(state): State<Arc<ApiState>>, Path((org, connector_id)): Path<(String, String)>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match org_caller(&headers, &org).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if !caller.is_admin {
        return admin_required();
    }
    if !valid_connector_id(&connector_id) {
        return coded(StatusCode::BAD_REQUEST, "invalid_request", "Invalid connector id.");
    }
    let Ok(b) = serde_json::from_slice::<PutBody>(&body) else {
        return coded(StatusCode::BAD_REQUEST, "invalid_request", "Body must be {as_issuer, token_endpoint, resource, client_id, client_secret?, roles, scopes, enabled}.");
    };
    let (roles, scopes) = match validate_put(&b) {
        Ok(v) => v,
        Err(m) => return coded(StatusCode::BAD_REQUEST, "invalid_request", m),
    };
    let secret_given = b.client_secret.is_some();
    let sealed = match b.client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(secret) => match state.credential_cipher.as_ref().map(|c| c.encrypt(secret)) {
            Some(Ok(sealed)) => Some(sealed),
            // Never store a vendor secret in plaintext.
            _ => return coded(StatusCode::SERVICE_UNAVAILABLE, "secrets_unavailable", "This server cannot store secrets right now."),
        },
    };
    let res = sqlx::query_as::<_, ConfigRow>(&format!(
        "INSERT INTO managed_auth_configs (id, org_id, connector_id, as_issuer, token_endpoint, resource, client_id, client_secret_sealed, roles, scopes, enabled, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         ON CONFLICT (org_id, connector_id) DO UPDATE SET
           as_issuer = EXCLUDED.as_issuer, token_endpoint = EXCLUDED.token_endpoint, resource = EXCLUDED.resource,
           client_id = EXCLUDED.client_id,
           client_secret_sealed = CASE WHEN $13 THEN EXCLUDED.client_secret_sealed ELSE managed_auth_configs.client_secret_sealed END,
           roles = EXCLUDED.roles, scopes = EXCLUDED.scopes, enabled = EXCLUDED.enabled, updated_at = now()
         RETURNING {CONFIG_COLUMNS}"
    ))
    .bind(format!("mac_{}", uuid::Uuid::new_v4().simple()))
    .bind(&org)
    .bind(&connector_id)
    .bind(b.as_issuer.trim())
    .bind(b.token_endpoint.trim())
    .bind(b.resource.trim())
    .bind(b.client_id.trim())
    .bind(sealed)
    .bind(&roles)
    .bind(&scopes)
    .bind(b.enabled)
    .bind(&caller.user_id)
    .bind(secret_given)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(c) => {
            tracing::info!(org = %org, connector = %connector_id, by = %caller.user_id, enabled = c.enabled, "managed auth: config saved");
            Json(config_json(&c)).into_response()
        }
        Err(e) => db_error(e),
    }
}

async fn admin_delete(State(state): State<Arc<ApiState>>, Path((org, connector_id)): Path<(String, String)>, headers: HeaderMap) -> Response {
    let caller = match org_caller(&headers, &org).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if !caller.is_admin {
        return admin_required();
    }
    match sqlx::query("DELETE FROM managed_auth_configs WHERE org_id = $1 AND connector_id = $2").bind(&org).bind(&connector_id).execute(&state.db).await {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(org = %org, connector = %connector_id, by = %caller.user_id, "managed auth: config removed");
            Json(json!({ "ok": true })).into_response()
        }
        Ok(_) => coded(StatusCode::NOT_FOUND, "not_found", "No managed sign-in is set for this connector."),
        Err(e) => db_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(enabled: bool, roles: &[&str]) -> ConfigRow {
        ConfigRow {
            org_id: "org_1".into(),
            connector_id: "linear".into(),
            as_issuer: "https://auth.vendor.example".into(),
            token_endpoint: "https://auth.vendor.example/token".into(),
            resource: "https://mcp.vendor.example/mcp".into(),
            client_id: "allternit-at-vendor".into(),
            client_secret_sealed: None,
            roles: roles.iter().map(|r| r.to_string()).collect(),
            scopes: vec!["read".into(), "write".into()],
            enabled,
            updated_at: Utc::now(),
        }
    }

    fn subject<'a>(role: Option<&'a str>, email: Option<(&'a str, bool)>) -> Subject<'a> {
        Subject { role, primary_email: email }
    }

    #[test]
    fn an_idjag_is_refused_for_non_members_disabled_configs_other_roles_and_unverified_email() {
        let ok = Some(("ann@acme.com", true));
        let c = config(true, &["org:member"]);
        assert_eq!(decide(Some(&c), &subject(Some("org:member"), ok)), Ok(()));
        assert_eq!(decide(Some(&c), &subject(Some("member"), ok)), Ok(()), "Clerk v1 role names normalize");
        assert_eq!(decide(Some(&c), &subject(None, ok)), Err(Refusal::NotMember));
        assert_eq!(decide(None, &subject(None, ok)), Err(Refusal::NotMember), "a non-member learns nothing about configs");
        assert_eq!(decide(None, &subject(Some("org:member"), ok)), Err(Refusal::NotConfigured));
        assert_eq!(decide(Some(&config(false, &["org:member"])), &subject(Some("org:member"), ok)), Err(Refusal::NotConfigured));
        assert_eq!(decide(Some(&config(true, &["org:admin"])), &subject(Some("org:member"), ok)), Err(Refusal::RoleNotAllowed));
        assert_eq!(decide(Some(&c), &subject(Some("org:member"), Some(("ann@acme.com", false)))), Err(Refusal::EmailUnverified));
        assert_eq!(decide(Some(&c), &subject(Some("org:member"), None)), Err(Refusal::EmailUnverified));
    }

    #[test]
    fn the_minted_idjag_has_the_spec_header_binds_aud_resource_client_and_lives_at_most_five_minutes() {
        let key = es256::Es256Key::from_pem(&es256::testing::pem()).unwrap();
        let c = config(true, &["org:member"]);
        let now = Utc::now().timestamp();
        let token = key.sign(IDJAG_TYP, &idjag_claims("org_1", &c, "user_a", "ann@acme.com", "read", "jti-1", now));
        let (header, claims) = es256::verify_with_jwks(&token, &key.jwks()).unwrap();
        assert_eq!((header["typ"].as_str(), header["alg"].as_str(), header["kid"].as_str()), (Some("oauth-id-jag+jwt"), Some("ES256"), Some(key.kid())));
        assert_eq!(claims["iss"], format!("{}/oauth2/org/org_1", issuer_base()));
        assert_eq!((claims["sub"].as_str(), claims["aud"].as_str(), claims["resource"].as_str()), (Some("user_a"), Some("https://auth.vendor.example"), Some("https://mcp.vendor.example/mcp")));
        assert_eq!((claims["client_id"].as_str(), claims["email"].as_str(), claims["email_verified"].as_bool()), (Some("allternit-at-vendor"), Some("ann@acme.com"), Some(true)));
        assert_eq!((claims["scope"].as_str(), claims["jti"].as_str()), (Some("read"), Some("jti-1")));
        let ttl = claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap();
        assert!(ttl > 0 && ttl <= 300, "{ttl}");
    }

    #[test]
    fn requested_scopes_must_all_be_configured() {
        let configured = vec!["read".to_string(), "write".to_string()];
        assert_eq!(grant_scope(None, &configured).as_deref(), Some("read write"));
        assert_eq!(grant_scope(Some("read"), &configured).as_deref(), Some("read"));
        assert_eq!(grant_scope(Some("read admin"), &configured), None);
    }

    #[test]
    fn admin_input_is_checked_before_it_is_stored() {
        let body = |issuer: &str, roles: &[&str]| PutBody {
            as_issuer: issuer.into(),
            token_endpoint: "https://auth.vendor.example/token".into(),
            resource: "https://mcp.vendor.example/mcp".into(),
            client_id: "c".into(),
            client_secret: None,
            roles: roles.iter().map(|r| r.to_string()).collect(),
            scopes: vec![],
            enabled: true,
        };
        assert_eq!(validate_put(&body("https://auth.vendor.example", &["admin", "org:member"])).unwrap().0, ["org:admin", "org:member"]);
        assert!(validate_put(&body("http://auth.vendor.example", &["org:member"])).is_err(), "plain http only on loopback");
        assert!(validate_put(&body("http://localhost:9000", &["org:member"])).is_ok());
        assert!(validate_put(&body("https://auth.vendor.example", &[])).is_err(), "at least one role");
        assert!(validate_put(&body("https://user:pw@auth.vendor.example", &["org:member"])).is_err());
        assert!(!valid_org("org_1/../x") && valid_org("org_2abcDEF"));
    }
}

#[cfg(test)]
mod router_tests {
    use crate::routes::test_support::{test_state, MockGateway};
    use std::sync::Arc;

    /// axum panics at startup on a duplicate route: build the whole app once.
    #[tokio::test]
    async fn the_full_router_builds_with_the_issuer_edge_and_connector_routes() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let _app = crate::create_router(state);
    }

    #[tokio::test]
    async fn issuer_routes_fail_closed_without_the_key_and_publish_metadata_and_jwks_with_it() {
        use tower::ServiceExt;
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let app = super::routes().merge(crate::routes::edge_auth::routes()).with_state(state);
        let send = |method: &str, uri: &str| {
            let req = axum::http::Request::builder().method(method).uri(uri).header("content-type", "application/x-www-form-urlencoded").body(axum::body::Body::from("grant_type=x")).unwrap();
            let app = app.clone();
            async move {
                let resp = app.oneshot(req).await.unwrap();
                let status = resp.status();
                let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
                (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
            }
        };
        let routes = [
            ("GET", "/oauth2/org/org_1/.well-known/oauth-authorization-server"),
            ("GET", "/.well-known/oauth-authorization-server/oauth2/org/org_1"),
            ("GET", "/oauth2/org/org_1/jwks.json"),
            ("POST", "/oauth2/org/org_1/token"),
            ("GET", "/oauth2/edge/jwks.json"),
            ("POST", "/oauth2/edge/token"),
        ];
        std::env::remove_var(crate::auth::es256::PEM_ENV);
        for (m, u) in routes {
            let (status, body) = send(m, u).await;
            assert_eq!((status.as_u16(), body["error"]["code"].as_str()), (503, Some("idjag_not_configured")), "{u}");
        }
        std::env::set_var(crate::auth::es256::PEM_ENV, crate::auth::es256::testing::pem());
        let (status, meta) = send("GET", "/.well-known/oauth-authorization-server/oauth2/org/org_1").await;
        assert_eq!(status.as_u16(), 200);
        assert_eq!(meta["issuer"], super::org_issuer("org_1"));
        assert_eq!(meta["token_endpoint"], format!("{}/token", super::org_issuer("org_1")));
        let (_, jwks) = send("GET", "/oauth2/org/org_1/jwks.json").await;
        assert_eq!(jwks["keys"][0]["kid"].as_str(), Some(crate::auth::es256::key_from_env().unwrap().kid()));
        let (_, edge) = send("GET", "/.well-known/oauth-authorization-server/oauth2/edge").await;
        assert_eq!(edge["issuer"], crate::routes::edge_auth::edge_as_issuer());
        // A bad grant type is refused before anything else is looked at.
        assert_eq!(send("POST", "/oauth2/edge/token").await.1["error"], "unsupported_grant_type");
        assert_eq!(send("POST", "/oauth2/org/org_1/token").await.1["error"], "unsupported_grant_type");
        assert_eq!(send("GET", "/oauth2/org/org..1/jwks.json").await.0.as_u16(), 404);
        std::env::remove_var(crate::auth::es256::PEM_ENV);
    }
}
