//! "Sign in with Allternit": the authorization endpoint and the consent API
//! (CONTRACT-oauth-linked-billing-2026-11.md, Part A: A1, A2, A4, A7, A9, A10).
//!
//! Routes (all public at the router; each handler checks its own caller):
//! - `GET  /.well-known/oauth-authorization-server`: RFC 8414 metadata (A2).
//!   `managed_auth` serves only `…/oauth-authorization-server/*rest` (per-org
//!   issuers), which never matches this exact path.
//! - `GET  /oauth2/authorize`: checks the request, stores an
//!   `oauth_auth_requests` row and sends the browser to the consent page.
//! - `GET  /api/v1/oauth/requests/:id`, `POST …/approve`, `POST …/deny`: the
//!   consent page's calls. Clerk sessions only (`auth::require_session_user`);
//!   API keys, platform keys and OAuth tokens get 403.
//!
//! # `/oauth2/authorize`
//! A bad or missing `client_id` or `redirect_uri` (not an exact string match
//! for one of the app's registered URIs) answers 400 here (HTML when the
//! browser asks for it, else JSON) and **never redirects**. Every other error
//! goes back to the trusted `redirect_uri` as `error`, `error_description` and
//! `state` (RFC 6749 §4.1.2.1). PKCE `S256` is required for every client;
//! `plain` or a missing challenge is `invalid_request` (A4). `scope` defaults to
//! `inference` and must be a subset of the app's registered scopes
//! (`invalid_scope`). A parameter sent twice is `invalid_request`.
//!
//! # Consent requests and authorization codes (what `/oauth2/token` reads)
//! One `oauth_auth_requests` row per authorize call:
//!
//! | Column | Value |
//! |---|---|
//! | `id` | `oar_` + 32 hex chars (16 random bytes) |
//! | `app_id`, `redirect_uri`, `scopes`, `state`, `code_challenge` | from the authorize call; `redirect_uri` is the exact registered string, `scopes` are in the contract's order, `code_challenge` is the S256 challenge (43 base64url chars) |
//! | `expires_at` | created + 15 min: the consent window |
//! | `user_id` | null until the first signed-in reader claims it; only that person may then approve or deny (anyone else gets 404) |
//! | `status` | `pending` → `approved` (code issued) or `denied`; the token endpoint moves `approved` → `used` |
//! | `code_hash` | `services::api_keys::hash_token(code)`: lowercase hex SHA-256 of the full code string. The code itself is never stored |
//! | `code_expires_at` | approval time (DB `now()`) + 300 s |
//!
//! The code is `alt_oac_` + 64 hex chars (32 random bytes), given to the app
//! once, in `redirect_to`. Step 4's token endpoint should: hash the presented
//! code, find the row by `code_hash` under `FOR UPDATE`; require
//! `status = 'approved'`, `code_expires_at > now()`, the row's app to be the
//! authenticated client, `redirect_uri` equal to the row's, and
//! `BASE64URL(SHA256(code_verifier)) == code_challenge`; then set
//! `status = 'used'`. A code presented again (`status = 'used'`) is reuse
//! (A3): revoke the live grant for (`app_id`, `user_id`) and its tokens and
//! audit `code_reuse`. The grant is that (`app_id`, `user_id`) row in
//! `oauth_grants` with `revoked_at IS NULL`; approve creates it or updates its
//! scopes (to this request's) and cap, so its `scopes` are what the tokens
//! carry.
//!
//! # Consent API
//! - Read: 404 when the request is unknown, someone else's, expired or no
//!   longer pending (A1). `user_is_test_user` is true when this person may
//!   approve: a live app, its owner, or a listed test user (A7).
//! - Approve `{"monthly_cap_cents": int}`: 400 `invalid_request` (bad body,
//!   cap below 0 or over $50 while not live), 403 `app_not_approved_for_user`,
//!   410 `request_expired` (expired, approved, denied or used); else
//!   `{"redirect_to": "<redirect_uri>?code=…&state=…"}`.
//! - Deny: 410 as above; else `{"redirect_to": "<redirect_uri>?error=access_denied&state=…"}`.
//!
//! Audit (A10): `request_approved`, `cap_changed` (an existing grant's cap
//! moved), `code_issued`, `request_denied`. Never a code or a token in detail.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::managed_auth::coded;
use crate::services::api_keys::{client_ip, hash_token};
use crate::services::oauth_apps::{self, record_audit, AppState, AuditCtx, AuditEvent, OAuthApp, SCOPES};
use crate::ApiState;

/// The issuer and base of every endpoint in the metadata (default `https://api.allternit.com`).
pub const ISSUER_ENV: &str = "ALLTERNIT_OAUTH_ISSUER";
const DEFAULT_ISSUER: &str = "https://api.allternit.com";
/// Where the consent page lives (default `https://ai.allternit.com`).
pub const CONSENT_BASE_ENV: &str = "ALLTERNIT_OAUTH_CONSENT_BASE";
const DEFAULT_CONSENT_BASE: &str = "https://ai.allternit.com";

pub const REQUEST_ID_PREFIX: &str = "oar_";
pub const CODE_PREFIX: &str = "alt_oac_";
/// Consent window (A1: 15 minutes).
pub const REQUEST_TTL_MINUTES: i64 = 15;
/// Authorization code lifetime (A3: 5 minutes, single use).
pub const CODE_TTL_SECONDS: i64 = 300;
/// The cap the consent page starts at, never above the app's maximum.
const SUGGESTED_CAP_CENTS: i64 = 1000;
const DEFAULT_SCOPE: &str = "inference";
const MAX_STATE_LEN: usize = 1024;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/oauth2/authorize", get(authorize))
        .route("/api/v1/oauth/requests/:id", get(read_request))
        .route("/api/v1/oauth/requests/:id/approve", post(approve_request))
        .route("/api/v1/oauth/requests/:id/deny", post(deny_request))
}

// ─── configuration and metadata ───────────────────────────────────────────────

fn env_base(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub fn issuer() -> String {
    env_base(ISSUER_ENV, DEFAULT_ISSUER)
}

pub fn consent_base() -> String {
    env_base(CONSENT_BASE_ENV, DEFAULT_CONSENT_BASE)
}

/// Contract A2.
pub fn metadata_doc() -> Value {
    let issuer = issuer();
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth2/authorize"),
        "token_endpoint": format!("{issuer}/oauth2/token"),
        "revocation_endpoint": format!("{issuer}/oauth2/revoke"),
        "userinfo_endpoint": format!("{issuer}/oauth2/userinfo"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_post", "none"],
        "scopes_supported": SCOPES.to_vec(),
    })
}

async fn metadata() -> Response {
    let mut resp = Json(metadata_doc()).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"));
    resp
}

/// `sub` for `/oauth2/userinfo` (A1): opaque, stable per app and user, and
/// different for every app, so two apps cannot match up a person by it.
pub fn subject_for(app_id: Uuid, user_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"allternit-oauth-sub\0");
    hasher.update(app_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(user_id.as_bytes());
    format!("usr_{}", hex::encode(&hasher.finalize()[..16]))
}

// ─── responses ────────────────────────────────────────────────────────────────

fn no_store(mut resp: Response) -> Response {
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn reply(result: Result<Response, ApiError>) -> Response {
    no_store(result.unwrap_or_else(IntoResponse::into_response))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// The 400 shown when the app cannot be trusted with a redirect.
fn error_page(headers: &HeaderMap, error: &str, description: &str) -> Response {
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"));
    let resp = if wants_html {
        let body = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Sign-in error</title></head>\
             <body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 16px;line-height:1.5\">\
             <h1 style=\"font-size:1.25rem\">This sign-in link does not work</h1><p>{}</p>\
             <p style=\"color:#666\">Error code: <code>{}</code>. Contact the app that sent you here.</p></body></html>",
            html_escape(description),
            html_escape(error)
        );
        (StatusCode::BAD_REQUEST, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
    } else {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": error, "error_description": description }))).into_response()
    };
    no_store(resp)
}

/// `uri` with the pairs added to its query. `uri` is a registered redirect
/// URI, already checked to be an absolute https (or loopback http) URL.
fn with_query(uri: &str, pairs: &[(&str, &str)]) -> String {
    match url::Url::parse(uri) {
        Ok(mut url) => {
            {
                let mut query = url.query_pairs_mut();
                for (k, v) in pairs {
                    query.append_pair(k, v);
                }
            }
            url.to_string()
        }
        Err(_) => uri.to_string(),
    }
}

fn found(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(value) => {
            let mut resp = StatusCode::FOUND.into_response();
            resp.headers_mut().insert(header::LOCATION, value);
            no_store(resp)
        }
        Err(_) => no_store(
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "server_error" }))).into_response(),
        ),
    }
}

/// RFC 6749 §4.1.2.1: an error sent back to the app's trusted redirect URI.
fn error_redirect(redirect_uri: &str, state: Option<&str>, error: &str, description: &str) -> Response {
    let mut pairs = vec![("error", error), ("error_description", description)];
    if let Some(state) = state {
        pairs.push(("state", state));
    }
    found(&with_query(redirect_uri, &pairs))
}

// ─── GET /oauth2/authorize ────────────────────────────────────────────────────

/// Query parameters. Each may appear once (RFC 6749 §3.1); an empty value
/// counts as missing.
struct Params(Vec<(String, String)>);

enum Param<'a> {
    Missing,
    One(&'a str),
    Repeated,
}

impl Params {
    fn parse(raw: Option<&str>) -> Self {
        Params(url::form_urlencoded::parse(raw.unwrap_or("").as_bytes()).into_owned().collect())
    }

    fn get(&self, name: &str) -> Param<'_> {
        let mut found = self.0.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str());
        match (found.next(), found.next()) {
            (None, _) => Param::Missing,
            (Some(_), Some(_)) => Param::Repeated,
            (Some(v), None) if v.is_empty() => Param::Missing,
            (Some(v), None) => Param::One(v),
        }
    }
}

/// An S256 challenge is the base64url (no padding) of a SHA-256: 43 chars.
fn is_s256_challenge(challenge: &str) -> bool {
    challenge.len() == 43 && challenge.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The requested scopes, deduplicated, in the contract's order. Each must be
/// a known scope the app is registered for.
fn requested_scopes(requested: &str, app_scopes: &[String]) -> Result<Vec<String>, String> {
    let mut wanted: Vec<&str> = requested.split(' ').filter(|s| !s.is_empty()).collect();
    if wanted.is_empty() {
        wanted.push(DEFAULT_SCOPE);
    }
    for scope in &wanted {
        if !SCOPES.contains(scope) {
            return Err(format!("Unknown scope '{scope}'. Allowed: {}.", SCOPES.join(", ")));
        }
        if !app_scopes.iter().any(|s| s == scope) {
            return Err(format!("This app is not registered for the '{scope}' scope."));
        }
    }
    Ok(SCOPES.iter().filter(|s| wanted.contains(*s)).map(|s| s.to_string()).collect())
}

async fn authorize(State(state): State<Arc<ApiState>>, headers: HeaderMap, RawQuery(raw): RawQuery) -> Response {
    let params = Params::parse(raw.as_deref());

    // 1. The client and its redirect URI. Until both check out, never redirect.
    let client_id = match params.get("client_id") {
        Param::One(v) => v,
        Param::Missing => return error_page(&headers, "invalid_request", "The client_id parameter is missing."),
        Param::Repeated => {
            return error_page(&headers, "invalid_request", "The client_id parameter was sent more than once.")
        }
    };
    let app = match oauth_apps::get_app_by_client_id(&state.db, client_id).await {
        Ok(Some(app)) => app,
        Ok(None) => return error_page(&headers, "invalid_client", "No app is registered with this client_id."),
        Err(e) => return no_store(e.into_response()),
    };
    let redirect_uri = match params.get("redirect_uri") {
        Param::One(v) => v.to_string(),
        Param::Missing => return error_page(&headers, "invalid_request", "The redirect_uri parameter is missing."),
        Param::Repeated => {
            return error_page(&headers, "invalid_request", "The redirect_uri parameter was sent more than once.")
        }
    };
    let registered = match oauth_apps::list_redirect_uris(&state.db, app.id).await {
        Ok(uris) => uris,
        Err(e) => return no_store(e.into_response()),
    };
    if !registered.iter().any(|uri| *uri == redirect_uri) {
        return error_page(
            &headers,
            "redirect_uri_mismatch",
            "The redirect_uri is not an exact match for a redirect URI registered for this app.",
        );
    }

    // 2. From here the redirect URI is trusted: errors go back to the app.
    let app_state = match params.get("state") {
        Param::Missing => None,
        Param::One(v) if v.len() <= MAX_STATE_LEN => Some(v.to_string()),
        Param::One(_) => {
            return error_redirect(&redirect_uri, None, "invalid_request", "The state parameter is over 1024 characters.")
        }
        Param::Repeated => {
            return error_redirect(&redirect_uri, None, "invalid_request", "The state parameter was sent more than once.")
        }
    };
    let fail = |error: &str, description: &str| error_redirect(&redirect_uri, app_state.as_deref(), error, description);

    if app.app_state() == AppState::Suspended {
        return fail("unauthorized_client", "This app is suspended.");
    }
    match params.get("response_type") {
        Param::One("code") => {}
        Param::One(_) => return fail("unsupported_response_type", "Only response_type=code is supported."),
        Param::Missing | Param::Repeated => return fail("invalid_request", "response_type=code is required."),
    }
    const PKCE_REQUIRED: &str = "PKCE is required: send code_challenge and code_challenge_method=S256.";
    match params.get("code_challenge_method") {
        Param::One("S256") => {}
        Param::One("plain") => return fail("invalid_request", "code_challenge_method=plain is not allowed; use S256."),
        Param::One(_) => return fail("invalid_request", "code_challenge_method must be S256."),
        Param::Missing => return fail("invalid_request", PKCE_REQUIRED),
        Param::Repeated => return fail("invalid_request", "code_challenge_method was sent more than once."),
    }
    let code_challenge = match params.get("code_challenge") {
        Param::One(v) if is_s256_challenge(v) => v.to_string(),
        Param::One(_) => {
            return fail(
                "invalid_request",
                "code_challenge must be the base64url SHA-256 of the code verifier (43 characters).",
            )
        }
        Param::Missing => return fail("invalid_request", PKCE_REQUIRED),
        Param::Repeated => return fail("invalid_request", "code_challenge was sent more than once."),
    };
    let scopes = match params.get("scope") {
        Param::Missing => requested_scopes(DEFAULT_SCOPE, &app.scopes),
        Param::One(v) => requested_scopes(v, &app.scopes),
        Param::Repeated => return fail("invalid_request", "The scope parameter was sent more than once."),
    };
    let scopes = match scopes {
        Ok(scopes) => scopes,
        Err(description) => return fail("invalid_scope", description.as_str()),
    };

    // 3. Store the request and hand the person to the consent page.
    let id = format!("{REQUEST_ID_PREFIX}{}", hex::encode(rand::random::<[u8; 16]>()));
    let stored = sqlx::query(
        "INSERT INTO oauth_auth_requests (id, app_id, redirect_uri, scopes, state, code_challenge, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(app.id)
    .bind(&redirect_uri)
    .bind(&scopes)
    .bind(&app_state)
    .bind(&code_challenge)
    .bind(Utc::now() + Duration::minutes(REQUEST_TTL_MINUTES))
    .execute(&state.db)
    .await;
    if let Err(e) = stored {
        tracing::error!(error = %e, "oauth: could not store the authorization request");
        return fail("server_error", "The request could not be saved. Try again.");
    }
    found(&format!("{}/oauth/authorize?request={id}", consent_base()))
}

// ─── consent API ──────────────────────────────────────────────────────────────

/// The signed-in person on the consent page: Clerk sessions only.
struct SessionUser {
    id: String,
    email: Option<String>,
}

async fn session_user(state: &ApiState, headers: &HeaderMap) -> Result<SessionUser, ApiError> {
    #[cfg(test)]
    if let Some(id) = headers.get("x-test-session-user").and_then(|v| v.to_str().ok()) {
        let email = headers.get("x-test-session-email").and_then(|v| v.to_str().ok()).map(str::to_string);
        return Ok(SessionUser { id: id.to_string(), email });
    }
    let user = crate::auth::require_session_user(&state.db, headers).await?;
    Ok(SessionUser { id: user.id, email: user.email })
}

#[derive(sqlx::FromRow)]
struct AuthRequest {
    id: String,
    redirect_uri: String,
    scopes: Vec<String>,
    state: Option<String>,
    user_id: Option<String>,
    status: String,
    expires_at: DateTime<Utc>,
    client_id: String,
}

impl AuthRequest {
    fn is_open(&self) -> bool {
        self.status == "pending" && self.expires_at > Utc::now()
    }

    fn claimed_by_other(&self, user: &SessionUser) -> bool {
        self.user_id.as_deref().is_some_and(|owner| owner != user.id)
    }
}

async fn load_request(db: &PgPool, id: &str) -> Result<Option<(AuthRequest, OAuthApp)>, ApiError> {
    if !id.starts_with(REQUEST_ID_PREFIX) || id.len() > 64 {
        return Ok(None);
    }
    let request = sqlx::query_as::<_, AuthRequest>(
        "SELECT r.id, r.redirect_uri, r.scopes, r.state, r.user_id, r.status, r.expires_at, a.client_id
         FROM oauth_auth_requests r JOIN oauth_apps a ON a.id = r.app_id WHERE r.id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(request) = request else { return Ok(None) };
    let Some(app) = oauth_apps::get_app_by_client_id(db, &request.client_id).await? else { return Ok(None) };
    Ok(Some((request, app)))
}

fn not_found() -> Response {
    coded(StatusCode::NOT_FOUND, "not_found", "This sign-in request was not found.")
}

fn request_expired() -> Response {
    coded(
        StatusCode::GONE,
        "request_expired",
        "This sign-in request has expired or was already answered. Go back to the app and start again.",
    )
}

fn invalid_request(message: &str) -> Response {
    coded(StatusCode::BAD_REQUEST, "invalid_request", message)
}

fn audit_ctx(actor: &str, headers: &HeaderMap, peer: Option<SocketAddr>) -> AuditCtx {
    AuditCtx {
        actor: actor.to_string(),
        ip: client_ip(headers, peer).map(|ip| ip.to_string()),
        ua: headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|ua| ua.chars().take(512).collect()),
    }
}

/// Locks the request row and re-checks it inside the transaction, so a
/// second approve, a deny or another person in between loses. `Err` is the
/// response to send.
async fn lock_open_request(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: &str,
    user: &SessionUser,
) -> Result<(), Response> {
    let row: Option<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT status, user_id, expires_at > now() FROM oauth_auth_requests WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| ApiError::from(e).into_response())?;
    match row {
        None => Err(not_found()),
        Some((_, Some(owner), _)) if owner != user.id => Err(not_found()),
        Some((status, _, true)) if status == "pending" => Ok(()),
        Some(_) => Err(request_expired()),
    }
}

async fn read_request(State(state): State<Arc<ApiState>>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    reply(read(&state, &headers, &id).await)
}

async fn read(state: &ApiState, headers: &HeaderMap, id: &str) -> Result<Response, ApiError> {
    let user = session_user(state, headers).await?;
    let Some((request, app)) = load_request(&state.db, id).await? else { return Ok(not_found()) };
    if request.claimed_by_other(&user) || !request.is_open() {
        return Ok(not_found());
    }
    // The first signed-in reader claims the request; nobody else can answer it.
    let claimed = sqlx::query(
        "UPDATE oauth_auth_requests SET user_id = $2 WHERE id = $1 AND (user_id IS NULL OR user_id = $2)",
    )
    .bind(&request.id)
    .bind(&user.id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok(not_found());
    }
    let may_authorize = oauth_apps::user_may_authorize(&state.db, &app, &user.id, user.email.as_deref()).await?;
    let max_cap_cents = app.max_cap_cents();
    let redirect_host = url::Url::parse(&request.redirect_uri).ok().and_then(|u| u.host_str().map(str::to_string));
    Ok(Json(json!({
        "id": request.id,
        "app": {
            "client_id": app.client_id,
            "name": app.name,
            "logo_url": app.logo_url,
            "homepage_url": app.homepage_url,
            "state": app.state,
        },
        "scopes": request.scopes,
        "redirect_host": redirect_host,
        "suggested_cap_cents": max_cap_cents.map_or(SUGGESTED_CAP_CENTS, |max| max.min(SUGGESTED_CAP_CENTS)),
        "max_cap_cents": max_cap_cents,
        "expires_at": request.expires_at.to_rfc3339(),
        // True when this person may approve: a live app, its owner, or a listed test user.
        "user_is_test_user": may_authorize,
    }))
    .into_response())
}

#[derive(Deserialize)]
struct ApproveBody {
    monthly_cap_cents: i64,
}

async fn approve_request(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    reply(approve(&state, &headers, peer.map(|c| c.0), &id, &body).await)
}

async fn approve(
    state: &ApiState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    id: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    let user = session_user(state, headers).await?;
    let Some((request, app)) = load_request(&state.db, id).await? else { return Ok(not_found()) };
    if request.claimed_by_other(&user) {
        return Ok(not_found());
    }
    if !request.is_open() {
        return Ok(request_expired());
    }
    let Ok(ApproveBody { monthly_cap_cents: cap }) = serde_json::from_slice::<ApproveBody>(body) else {
        return Ok(invalid_request("Send {\"monthly_cap_cents\": <whole cents>}."));
    };
    if !oauth_apps::user_may_authorize(&state.db, &app, &user.id, user.email.as_deref()).await? {
        return Ok(coded(
            StatusCode::FORBIDDEN,
            "app_not_approved_for_user",
            "This app is not live yet, and you are not one of its test users.",
        ));
    }
    if let Err(e) = app.check_cap(cap) {
        let message = match e {
            ApiError::BadRequest(message) => message,
            other => other.to_string(),
        };
        return Ok(invalid_request(&message));
    }

    let code = format!("{CODE_PREFIX}{}", hex::encode(rand::random::<[u8; 32]>()));
    let ctx = audit_ctx(&user.id, headers, peer);
    let mut tx = state.db.begin().await?;
    if let Err(resp) = lock_open_request(&mut tx, &request.id, &user).await {
        return Ok(resp);
    }

    // One live grant per (app, user): a new consent replaces its scopes and cap.
    let previous_cap: Option<i32> = sqlx::query_scalar(
        "SELECT monthly_cap_cents FROM oauth_grants WHERE app_id = $1 AND user_id = $2 AND revoked_at IS NULL FOR UPDATE",
    )
    .bind(app.id)
    .bind(&user.id)
    .fetch_optional(&mut *tx)
    .await?;
    // check_cap keeps the cap within i32.
    let cap_i32 = i32::try_from(cap).map_err(|_| ApiError::BadRequest("The monthly cap is too large.".to_string()))?;
    let grant_id: Uuid = sqlx::query_scalar(
        "INSERT INTO oauth_grants (id, app_id, user_id, scopes, monthly_cap_cents, sub)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (app_id, user_id) WHERE revoked_at IS NULL
         DO UPDATE SET scopes = EXCLUDED.scopes, monthly_cap_cents = EXCLUDED.monthly_cap_cents
         RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(app.id)
    .bind(&user.id)
    .bind(&request.scopes)
    .bind(cap_i32)
    .bind(subject_for(app.id, &user.id))
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(&format!(
        "UPDATE oauth_auth_requests
         SET status = 'approved', user_id = $2, code_hash = $3, code_expires_at = now() + interval '{CODE_TTL_SECONDS} seconds'
         WHERE id = $1"
    ))
    .bind(&request.id)
    .bind(&user.id)
    .bind(hash_token(&code))
    .execute(&mut *tx)
    .await?;

    record_audit(
        &mut *tx,
        &ctx,
        Some(app.id),
        Some(grant_id),
        AuditEvent::RequestApproved,
        json!({ "request_id": request.id, "scopes": request.scopes, "monthly_cap_cents": cap }),
    )
    .await?;
    if let Some(previous) = previous_cap.filter(|previous| i64::from(*previous) != cap) {
        record_audit(
            &mut *tx,
            &ctx,
            Some(app.id),
            Some(grant_id),
            AuditEvent::CapChanged,
            json!({ "request_id": request.id, "from_cents": previous, "to_cents": cap }),
        )
        .await?;
    }
    record_audit(
        &mut *tx,
        &ctx,
        Some(app.id),
        Some(grant_id),
        AuditEvent::CodeIssued,
        json!({ "request_id": request.id, "expires_in": CODE_TTL_SECONDS }),
    )
    .await?;
    tx.commit().await?;

    let mut pairs = vec![("code", code.as_str())];
    if let Some(app_state) = request.state.as_deref() {
        pairs.push(("state", app_state));
    }
    Ok(Json(json!({ "redirect_to": with_query(&request.redirect_uri, &pairs) })).into_response())
}

async fn deny_request(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    reply(deny(&state, &headers, peer.map(|c| c.0), &id).await)
}

async fn deny(state: &ApiState, headers: &HeaderMap, peer: Option<SocketAddr>, id: &str) -> Result<Response, ApiError> {
    let user = session_user(state, headers).await?;
    let Some((request, app)) = load_request(&state.db, id).await? else { return Ok(not_found()) };
    if request.claimed_by_other(&user) {
        return Ok(not_found());
    }
    if !request.is_open() {
        return Ok(request_expired());
    }
    let ctx = audit_ctx(&user.id, headers, peer);
    let mut tx = state.db.begin().await?;
    if let Err(resp) = lock_open_request(&mut tx, &request.id, &user).await {
        return Ok(resp);
    }
    sqlx::query("UPDATE oauth_auth_requests SET status = 'denied', user_id = $2 WHERE id = $1")
        .bind(&request.id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
    record_audit(&mut *tx, &ctx, Some(app.id), None, AuditEvent::RequestDenied, json!({ "request_id": request.id }))
        .await?;
    tx.commit().await?;

    let mut pairs = vec![("error", "access_denied")];
    if let Some(app_state) = request.state.as_deref() {
        pairs.push(("state", app_state));
    }
    Ok(Json(json!({ "redirect_to": with_query(&request.redirect_uri, &pairs) })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use crate::services::oauth_apps::{add_test_user, create_app, ClientType, CreateOAuthAppInput};
    use tower::ServiceExt;

    const PROJECT: &str = "proj_oauth_server";
    const OWNER: &str = "user_owner";
    const CB: &str = "https://notes.example.com/cb";
    /// RFC 7636 appendix B.
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    const BASE: [(&str, &str); 6] = [
        ("response_type", "code"),
        ("redirect_uri", CB),
        ("code_challenge", CHALLENGE),
        ("code_challenge_method", "S256"),
        ("state", "xyz"),
        ("scope", "inference offline_access"),
    ];

    type Changes = Vec<(&'static str, Option<&'static str>)>;

    struct Fixture {
        state: Arc<ApiState>,
        app: OAuthApp,
    }

    /// The real migration 150 in a fresh schema, with one testing-state public
    /// app (scopes inference + offline_access, one redirect URI).
    async fn fixture() -> Fixture {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS platform_projects (id TEXT PRIMARY KEY);
             CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT);",
        )
        .execute(&state.db)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations_pg/150_oauth_apps.sql")).execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO platform_projects (id) VALUES ($1)").bind(PROJECT).execute(&state.db).await.unwrap();
        let app = create_app(
            &state.db,
            CreateOAuthAppInput {
                project_id: PROJECT.to_string(),
                owner_user_id: OWNER.to_string(),
                name: "Notes Sync".to_string(),
                client_type: ClientType::Public,
                logo_url: None,
                homepage_url: Some("https://notes.example.com".to_string()),
                privacy_url: None,
                scopes: vec!["inference".to_string(), "offline_access".to_string()],
                redirect_uris: vec![CB.to_string()],
            },
            &AuditCtx { actor: OWNER.to_string(), ..Default::default() },
        )
        .await
        .unwrap()
        .app;
        Fixture { state, app }
    }

    async fn call(
        state: &Arc<ApiState>,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut req = axum::http::Request::builder().method(method).uri(uri).header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = super::routes()
            .with_state(state.clone())
            .oneshot(req.body(axum::body::Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let resp_headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, resp_headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The good parameters with some removed (`None`) or replaced.
    fn with(changes: &[(&'static str, Option<&'static str>)]) -> Vec<(&'static str, &'static str)> {
        let mut out: Vec<(&'static str, &'static str)> =
            BASE.iter().copied().filter(|(k, _)| !changes.iter().any(|(c, _)| c == k)).collect();
        for &(k, v) in changes {
            if let Some(v) = v {
                out.push((k, v));
            }
        }
        out
    }

    fn authorize_uri(client_id: Option<&str>, params: &[(&str, &str)]) -> String {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        if let Some(client_id) = client_id {
            query.append_pair("client_id", client_id);
        }
        for (k, v) in params {
            query.append_pair(k, v);
        }
        format!("/oauth2/authorize?{}", query.finish())
    }

    fn location(headers: &HeaderMap) -> Option<url::Url> {
        headers.get(header::LOCATION).map(|v| url::Url::parse(v.to_str().unwrap()).unwrap())
    }

    fn query(url: &url::Url, key: &str) -> Option<String> {
        url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
    }

    async fn count(db: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
    }

    fn as_user(user: &str) -> [(&str, &str); 1] {
        [("x-test-session-user", user)]
    }

    /// A good authorize call; returns the stored request id.
    async fn start(f: &Fixture) -> String {
        let (status, headers, _) =
            call(&f.state, "GET", &authorize_uri(Some(&f.app.client_id), &with(&[])), &[], "").await;
        assert_eq!(status, StatusCode::FOUND);
        let loc = location(&headers).unwrap();
        assert!(loc.as_str().starts_with(&format!("{}/oauth/authorize?request=", consent_base())), "{loc}");
        query(&loc, "request").unwrap()
    }

    async fn approve_as(f: &Fixture, id: &str, user: &str, body: &str) -> (StatusCode, Value) {
        let (status, _, body) =
            call(&f.state, "POST", &format!("/api/v1/oauth/requests/{id}/approve"), &as_user(user), body).await;
        (status, body)
    }

    async fn request_row(db: &PgPool, id: &str) -> (String, Option<String>, Option<String>) {
        sqlx::query_as("SELECT status, user_id, code_hash FROM oauth_auth_requests WHERE id = $1")
            .bind(id)
            .fetch_one(db)
            .await
            .unwrap()
    }

    /// Asserts the app got an RFC 6749 error redirect, with its state, and no code.
    fn assert_error_redirect(status: StatusCode, headers: &HeaderMap, error: &str) {
        assert_eq!(status, StatusCode::FOUND);
        let loc = location(headers).expect("redirects to the app");
        assert!(loc.as_str().starts_with(&format!("{CB}?")), "{loc}");
        assert_eq!(query(&loc, "error").as_deref(), Some(error), "{loc}");
        assert!(query(&loc, "error_description").is_some());
        assert_eq!(query(&loc, "state").as_deref(), Some("xyz"));
        assert!(query(&loc, "code").is_none());
    }

    #[tokio::test]
    async fn full_router_builds_and_serves_metadata() {
        // axum panics at startup on overlapping routes; managed_auth serves
        // `/.well-known/oauth-authorization-server/*rest` next to this path.
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let app = crate::create_router(state);
        let req = axum::http::Request::builder()
            .uri("/.well-known/oauth-authorization-server")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let doc: Value = serde_json::from_slice(&bytes).unwrap();
        let issuer = issuer();
        assert_eq!(doc["issuer"], issuer);
        assert_eq!(doc["authorization_endpoint"], format!("{issuer}/oauth2/authorize"));
        assert_eq!(doc["token_endpoint"], format!("{issuer}/oauth2/token"));
        assert_eq!(doc["code_challenge_methods_supported"], json!(["S256"]));
        assert_eq!(doc["token_endpoint_auth_methods_supported"], json!(["client_secret_post", "none"]));
        assert_eq!(doc["scopes_supported"], json!(["inference", "usage", "profile", "offline_access"]));
    }

    #[tokio::test]
    async fn authorize_stores_request_for_consent() {
        let f = fixture().await;
        let id = start(&f).await;
        assert!(id.starts_with(REQUEST_ID_PREFIX));
        let (scopes, app_state, challenge, status, user_id, ttl): (Vec<String>, Option<String>, String, String, Option<String>, f64) =
            sqlx::query_as(
                "SELECT scopes, state, code_challenge, status, user_id, EXTRACT(EPOCH FROM expires_at - now())::float8
                 FROM oauth_auth_requests WHERE id = $1",
            )
            .bind(&id)
            .fetch_one(&f.state.db)
            .await
            .unwrap();
        assert_eq!(scopes, ["inference", "offline_access"]);
        assert_eq!((app_state.as_deref(), challenge.as_str(), status.as_str(), user_id), (Some("xyz"), CHALLENGE, "pending", None));
        assert!(ttl > 14.0 * 60.0 && ttl <= 15.0 * 60.0, "{ttl}");

        // No scope: inference. A scope the app is not registered for: invalid_scope.
        let (status, _, _) =
            call(&f.state, "GET", &authorize_uri(Some(&f.app.client_id), &with(&[("scope", None)])), &[], "").await;
        assert_eq!(status, StatusCode::FOUND);
        let (status, headers, _) = call(
            &f.state,
            "GET",
            &authorize_uri(Some(&f.app.client_id), &with(&[("scope", Some("inference profile"))])),
            &[],
            "",
        )
        .await;
        assert_error_redirect(status, &headers, "invalid_scope");
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_auth_requests").await, 2);
    }

    #[tokio::test]
    async fn authorize_without_pkce_is_invalid_request() {
        let f = fixture().await;
        let cases: [Changes; 4] = [
            vec![("code_challenge", None), ("code_challenge_method", None)],
            vec![("code_challenge", None)],
            vec![("code_challenge_method", None)],
            vec![("code_challenge", Some("too-short"))],
        ];
        for changes in cases {
            let (status, headers, _) =
                call(&f.state, "GET", &authorize_uri(Some(&f.app.client_id), &with(&changes)), &[], "").await;
            assert_error_redirect(status, &headers, "invalid_request");
        }
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_auth_requests").await, 0);
    }

    #[tokio::test]
    async fn authorize_plain_method_rejected() {
        let f = fixture().await;
        for method in ["plain", "s256"] {
            let (status, headers, _) = call(
                &f.state,
                "GET",
                &authorize_uri(Some(&f.app.client_id), &with(&[("code_challenge_method", Some(method))])),
                &[],
                "",
            )
            .await;
            assert_error_redirect(status, &headers, "invalid_request");
        }
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_auth_requests").await, 0);
    }

    #[tokio::test]
    async fn authorize_bad_redirect_never_redirects() {
        let f = fixture().await;
        let cases: [(Changes, &str); 6] = [
            (vec![("redirect_uri", Some("https://notes.example.com/cb/"))], "redirect_uri_mismatch"),
            (vec![("redirect_uri", Some("https://notes.example.com/cb?next=1"))], "redirect_uri_mismatch"),
            (vec![("redirect_uri", Some("https://NOTES.example.com/cb"))], "redirect_uri_mismatch"),
            (vec![("redirect_uri", Some("https://evil.example/cb"))], "redirect_uri_mismatch"),
            // Other errors too stay here while the URI is untrusted.
            (vec![("redirect_uri", Some("https://evil.example/cb")), ("code_challenge_method", Some("plain"))], "redirect_uri_mismatch"),
            (vec![("redirect_uri", None)], "invalid_request"),
        ];
        for (changes, error) in cases {
            let (status, headers, body) =
                call(&f.state, "GET", &authorize_uri(Some(&f.app.client_id), &with(&changes)), &[], "").await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{changes:?}");
            assert!(headers.get(header::LOCATION).is_none(), "{changes:?}");
            assert_eq!(body["error"], error, "{changes:?}");
        }

        // A browser gets an HTML page, still with no redirect.
        let (status, headers, _) = call(
            &f.state,
            "GET",
            &authorize_uri(Some(&f.app.client_id), &with(&[("redirect_uri", Some("https://evil.example/cb"))])),
            &[("accept", "text/html,application/xhtml+xml")],
            "",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(headers.get(header::LOCATION).is_none());
        assert!(headers[header::CONTENT_TYPE].to_str().unwrap().starts_with("text/html"));
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_auth_requests").await, 0);
    }

    #[tokio::test]
    async fn authorize_unknown_client_never_redirects() {
        let f = fixture().await;
        let good = with(&[]);
        let doubled = format!("{}&client_id={}", authorize_uri(Some(&f.app.client_id), &good), f.app.client_id);
        for (uri, error) in [
            (authorize_uri(Some("oapp_00000000000000000000000000000000"), &good), "invalid_client"),
            (authorize_uri(None, &good), "invalid_request"),
            (doubled, "invalid_request"),
        ] {
            let (status, headers, body) = call(&f.state, "GET", &uri, &[], "").await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
            assert!(headers.get(header::LOCATION).is_none(), "{uri}");
            assert_eq!(body["error"], error, "{uri}");
        }
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_auth_requests").await, 0);
    }

    #[tokio::test]
    async fn approve_issues_single_use_code() {
        let f = fixture().await;
        let id = start(&f).await;
        let (status, body) = approve_as(&f, &id, OWNER, r#"{"monthly_cap_cents": 2500}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let to = url::Url::parse(body["redirect_to"].as_str().unwrap()).unwrap();
        assert!(to.as_str().starts_with(&format!("{CB}?")), "{to}");
        let code = query(&to, "code").unwrap();
        assert!(code.starts_with(CODE_PREFIX) && code.len() == CODE_PREFIX.len() + 64);
        assert_eq!(query(&to, "state").as_deref(), Some("xyz"));

        // Stored: the code's hash, 5 minutes, bound to the person.
        let (status_col, user_id, code_hash) = request_row(&f.state.db, &id).await;
        assert_eq!((status_col.as_str(), user_id.as_deref()), ("approved", Some(OWNER)));
        assert_eq!(code_hash.as_deref(), Some(hash_token(&code).as_str()));
        let ttl: f64 = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM code_expires_at - now())::float8 FROM oauth_auth_requests WHERE id = $1",
        )
        .bind(&id)
        .fetch_one(&f.state.db)
        .await
        .unwrap();
        assert!(ttl > 280.0 && ttl <= 300.0, "{ttl}");

        // The grant: this request's scopes, the chosen cap, an opaque sub.
        let (cap, scopes, sub): (i32, Vec<String>, String) = sqlx::query_as(
            "SELECT monthly_cap_cents, scopes, sub FROM oauth_grants WHERE app_id = $1 AND user_id = $2 AND revoked_at IS NULL",
        )
        .bind(f.app.id)
        .bind(OWNER)
        .fetch_one(&f.state.db)
        .await
        .unwrap();
        assert_eq!(cap, 2500);
        assert_eq!(scopes, ["inference", "offline_access"]);
        assert_eq!(sub, subject_for(f.app.id, OWNER));
        assert!(!sub.contains(OWNER));

        // The plain code is in no row.
        let leaks: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM oauth_auth_requests r WHERE row_to_json(r)::text LIKE '%' || $1 || '%')
                  + (SELECT count(*) FROM oauth_grants g WHERE row_to_json(g)::text LIKE '%' || $1 || '%')
                  + (SELECT count(*) FROM oauth_audit_log l WHERE row_to_json(l)::text LIKE '%' || $1 || '%')",
        )
        .bind(&code)
        .fetch_one(&f.state.db)
        .await
        .unwrap();
        assert_eq!(leaks, 0);
        let events: Vec<String> =
            sqlx::query_scalar("SELECT event FROM oauth_audit_log WHERE app_id = $1 AND event <> 'app_created' ORDER BY id")
                .bind(f.app.id)
                .fetch_all(&f.state.db)
                .await
                .unwrap();
        assert_eq!(events, ["request_approved", "code_issued"]);

        // A second approve gets no second code.
        let (status, body) = approve_as(&f, &id, OWNER, r#"{"monthly_cap_cents": 2500}"#).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::GONE, Some("request_expired")));
        assert_eq!(request_row(&f.state.db, &id).await.2.as_deref(), Some(hash_token(&code).as_str()));
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_grants").await, 1);

        // A new consent for the same app updates the one live grant and audits the cap change.
        let again = start(&f).await;
        let (status, _) = approve_as(&f, &again, OWNER, r#"{"monthly_cap_cents": 500}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_grants").await, 1);
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_audit_log WHERE event = 'cap_changed'").await, 1);
    }

    #[tokio::test]
    async fn approve_over_cap_rejected() {
        let f = fixture().await;
        let id = start(&f).await;
        for body in [r#"{"monthly_cap_cents": 5001}"#, r#"{"monthly_cap_cents": -1}"#, r#"{"monthly_cap_cents": 10.5}"#, "{}"] {
            let (status, resp) = approve_as(&f, &id, OWNER, body).await;
            assert_eq!((status, resp["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_request")), "{body}");
        }
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_grants").await, 0);
        let (status_col, _, code_hash) = request_row(&f.state.db, &id).await;
        assert_eq!((status_col.as_str(), code_hash), ("pending", None));

        // $50 is the most a testing app can get.
        let (status, _) = approve_as(&f, &id, OWNER, r#"{"monthly_cap_cents": 5000}"#).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn non_test_user_cannot_approve_testing_app() {
        let f = fixture().await;
        let id = start(&f).await;
        let (status, _, read) =
            call(&f.state, "GET", &format!("/api/v1/oauth/requests/{id}"), &as_user("user_stranger"), "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read["user_is_test_user"], false);
        assert_eq!(read["app"]["state"], "testing");
        assert_eq!((read["max_cap_cents"].as_i64(), read["suggested_cap_cents"].as_i64()), (Some(5000), Some(1000)));
        assert_eq!(read["redirect_host"], "notes.example.com");
        assert_eq!(read["scopes"], json!(["inference", "offline_access"]));

        let (status, body) = approve_as(&f, &id, "user_stranger", r#"{"monthly_cap_cents": 100}"#).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("app_not_approved_for_user")));
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_grants").await, 0);
        assert_eq!(request_row(&f.state.db, &id).await.0, "pending");
        // The stranger opened it first, so nobody else can answer it.
        assert_eq!(approve_as(&f, &id, OWNER, r#"{"monthly_cap_cents": 100}"#).await.0, StatusCode::NOT_FOUND);

        // Once listed as a test user (matched by email), the same person may approve.
        add_test_user(&f.state.db, PROJECT, f.app.id, "tester@example.com", &AuditCtx::default()).await.unwrap();
        let id = start(&f).await;
        let (status, _, _) = call(
            &f.state,
            "POST",
            &format!("/api/v1/oauth/requests/{id}/approve"),
            &[("x-test-session-user", "user_stranger"), ("x-test-session-email", "Tester@Example.com")],
            r#"{"monthly_cap_cents": 100}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn expired_request_410() {
        let f = fixture().await;
        let id = start(&f).await;
        sqlx::query("UPDATE oauth_auth_requests SET expires_at = now() - interval '1 minute' WHERE id = $1")
            .bind(&id)
            .execute(&f.state.db)
            .await
            .unwrap();
        let (status, body) = approve_as(&f, &id, OWNER, r#"{"monthly_cap_cents": 100}"#).await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::GONE, Some("request_expired")));
        let (status, _, body) =
            call(&f.state, "POST", &format!("/api/v1/oauth/requests/{id}/deny"), &as_user(OWNER), "").await;
        assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::GONE, Some("request_expired")));
        // The read answers 404 for an expired request (A1).
        let (status, _, _) = call(&f.state, "GET", &format!("/api/v1/oauth/requests/{id}"), &as_user(OWNER), "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_grants").await, 0);
        assert_eq!(request_row(&f.state.db, &id).await, ("pending".to_string(), None, None));
    }

    #[tokio::test]
    async fn deny_redirects_access_denied() {
        let f = fixture().await;
        let id = start(&f).await;
        let (status, _, body) =
            call(&f.state, "POST", &format!("/api/v1/oauth/requests/{id}/deny"), &as_user(OWNER), "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["redirect_to"], format!("{CB}?error=access_denied&state=xyz"));
        assert_eq!(request_row(&f.state.db, &id).await, ("denied".to_string(), Some(OWNER.to_string()), None));
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_audit_log WHERE event = 'request_denied'").await, 1);

        // A denied request cannot be approved after.
        let (status, _) = approve_as(&f, &id, OWNER, r#"{"monthly_cap_cents": 100}"#).await;
        assert_eq!(status, StatusCode::GONE);
        assert_eq!(count(&f.state.db, "SELECT count(*) FROM oauth_grants").await, 0);
    }
}
