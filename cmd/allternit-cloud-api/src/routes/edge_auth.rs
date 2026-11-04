//! Trusted issuers on the MCP edge (docs/MANAGED_AUTH_MAP.md WP4).
//!
//! An org admin trusts an outside identity issuer (a Claude organization,
//! Okta). A member of that org who signs in to the outside app then reaches
//! `mcp.allternit.com` without an Allternit sign-in screen: the app sends an
//! ID-JAG from its issuer to the edge authorization server and gets an edge
//! access token.
//!
//! - `POST /oauth2/edge/token`: RFC 7523 JWT-bearer grant (`assertion` = ID-JAG).
//! - `GET /oauth2/edge/.well-known/oauth-authorization-server` (also the RFC 8414
//!   path, served by `routes::managed_auth::metadata_at`) and `/oauth2/edge/jwks.json`.
//! - `GET|POST /api/v1/orgs/:org/trusted-issuers`, `DELETE …/:id`: Clerk session;
//!   writes need `org:admin`, members read.
//!
//! An assertion is accepted only when: its header `typ` is `oauth-id-jag+jwt`;
//! `iss` is trusted by some org; the signature checks against **that org's**
//! JWKS for the issuer (cached, refetched on an unknown `kid`); `aud` is the
//! edge AS issuer; `resource` is an edge resource that org allowed; `exp` /
//! `iat` are sane; `jti` is new (`edge_jti_seen`, kept until `exp`); `email`
//! is present (and not marked unverified). The user is the one Clerk user who
//! holds that email **verified** and is a member of an org that trusts the
//! issuer and verified the signature. Zero or several ⇒ `invalid_grant`.
//!
//! The edge access token is ES256 (`routes::managed_auth`'s key), `iss` = edge
//! AS issuer, `aud` = resource, `scope` = the target's scope, `client_id` =
//! `issuer:<iss>`, ≤ 1 h. `mcp_edge` accepts it next to Clerk tokens and treats
//! the client as approved for members of an org that still trusts the issuer
//! for that target (org consent; no per-user approval screen). Bots stay
//! per-user: no edge token is ever issued for a bot resource.
//!
//! Claude's exact claims and JWKS location are not public yet: this follows
//! the MCP extension spec, and everything issuer-specific lives in
//! [`assertion_email`]. Claude-issued assertions count as verified only once
//! the Claude Connect-step guide has been read and checked against it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use super::managed_auth::{coded, form_fields, valid_url, IDJAG_TYP, JWT_BEARER};
use crate::auth::es256;
use crate::ApiState;

pub const EDGE_TOKEN_TTL_SECS: i64 = 3600;
pub const EDGE_TOKEN_TYP: &str = "at+jwt";
/// Longest assertion lifetime we accept (`exp - iat`).
const MAX_ASSERTION_LIFETIME_SECS: i64 = 3600;
const LEEWAY_SECS: i64 = 60;
const JWKS_TTL: Duration = Duration::from_secs(3600);
/// An unknown `kid` refetches at most this often per JWKS URL.
const JWKS_REFETCH_GAP: Duration = Duration::from_secs(60);
pub const TARGETS: [&str; 2] = ["connectors", "agents"];
const KINDS: [&str; 3] = ["claude", "okta", "other"];
/// The client id an issuer's members act under on the edge.
pub const ISSUER_CLIENT_PREFIX: &str = "issuer:";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/oauth2/edge/token", post(edge_token))
        .route("/oauth2/edge/.well-known/oauth-authorization-server", get(metadata_route))
        .route("/oauth2/edge/.well-known/openid-configuration", get(metadata_route))
        .route("/oauth2/edge/jwks.json", get(jwks_route))
        .route("/api/v1/orgs/:org/trusted-issuers", get(list_route).post(add_route))
        .route("/api/v1/orgs/:org/trusted-issuers/:id", delete(revoke_route))
}

pub fn edge_as_issuer() -> String {
    format!("{}/oauth2/edge", super::managed_auth::issuer_base())
}

/// The edge AS when it can issue (the ES256 key is set), for RFC 9728 metadata.
pub fn edge_as_if_configured() -> Option<String> {
    es256::key_from_env().map(|_| edge_as_issuer())
}

pub fn as_metadata() -> Value {
    let issuer = edge_as_issuer();
    json!({
        "issuer": issuer,
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks.json"),
        "grant_types_supported": [JWT_BEARER],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["connectors:use", "agents:read"],
        "response_types_supported": [],
        "service_documentation": "https://docs.allternit.com/dev/trusted-issuers"
    })
}

fn not_configured() -> Response {
    coded(StatusCode::SERVICE_UNAVAILABLE, "idjag_not_configured", "The edge authorization server is not set up on this server.")
}

async fn metadata_route() -> Response {
    if es256::key_from_env().is_none() {
        return not_configured();
    }
    Json(as_metadata()).into_response()
}

async fn jwks_route() -> Response {
    match es256::key_from_env() {
        Some(k) => Json(k.jwks()).into_response(),
        None => not_configured(),
    }
}

/// The MCP edge's resources an edge token may be issued for → their target.
/// Bots (`<base>/bots/<id>`) are never one of them.
pub fn edge_target(resource: &str) -> Option<(&'static str, &'static str)> {
    let base = std::env::var("MCP_PUBLIC_URL").ok().map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| !v.is_empty())?;
    let r = resource.trim().trim_end_matches('/');
    if r == format!("{base}/connectors") {
        Some(("connectors", "connectors:use"))
    } else if r == base {
        Some(("agents", "agents:read"))
    } else {
        None
    }
}

// ─── verifying an edge access token (mcp_edge) ────────────────────────────────

/// `Some` when `token` claims to be ours (unverified `iss` = edge AS issuer):
/// then it is checked here and never sent to Clerk. `None` = not ours.
pub fn verify_access_token(token: &str) -> Option<Result<Value, String>> {
    let (_, claims) = es256::decode_unverified(token)?;
    if claims["iss"].as_str() != Some(edge_as_issuer().as_str()) {
        return None;
    }
    let Some(key) = es256::key_from_env() else { return Some(Err("The edge authorization server is not set up".into())) };
    Some(check_access_token(token, &key, Utc::now().timestamp()))
}

fn check_access_token(token: &str, key: &es256::Es256Key, now: i64) -> Result<Value, String> {
    let (header, claims) = es256::verify_with_jwks(token, &key.jwks()).map_err(|_| "Invalid edge token signature".to_string())?;
    if header["typ"].as_str() != Some(EDGE_TOKEN_TYP) || claims["iss"].as_str() != Some(edge_as_issuer().as_str()) {
        return Err("Not an edge access token".into());
    }
    match claims["exp"].as_i64() {
        Some(exp) if now < exp + LEEWAY_SECS => Ok(claims),
        _ => Err("The edge token has expired".into()),
    }
}

// ─── trusted issuers ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct IssuerRow {
    pub id: String,
    pub org_id: String,
    pub issuer: String,
    pub jwks_uri: Option<String>,
    pub kind: String,
    pub label: String,
    pub targets: Vec<String>,
    pub created_at: chrono::DateTime<Utc>,
}

const ISSUER_COLUMNS: &str = "id, org_id, issuer, jwks_uri, kind, label, targets, created_at";

async fn trusting_rows(db: &sqlx::PgPool, iss: &str) -> Result<Vec<IssuerRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {ISSUER_COLUMNS} FROM mcp_trusted_issuers WHERE issuer = $1 AND revoked_at IS NULL")).bind(iss).fetch_all(db).await
}

/// Org consent for `mcp_edge`: `client` = `issuer:<iss>` acting for `user_id`
/// on `target`. True only while an org the user belongs to trusts that
/// issuer for that target.
pub async fn org_consent(db: &sqlx::PgPool, user_id: &str, client: &str, target: &str) -> Result<bool, String> {
    let Some(iss) = client.strip_prefix(ISSUER_CLIENT_PREFIX) else { return Ok(false) };
    if !TARGETS.contains(&target) {
        return Ok(false);
    }
    let rows = trusting_rows(db, iss).await.map_err(|e| e.to_string())?;
    let orgs: Vec<&str> = rows.iter().filter(|r| r.targets.iter().any(|t| t == target)).map(|r| r.org_id.as_str()).collect();
    if orgs.is_empty() {
        return Ok(false);
    }
    let memberships = crate::services::user_trust::clerk_org_memberships_checked(user_id).await.ok_or("Clerk membership lookup failed")?;
    Ok(memberships.iter().any(|m| orgs.contains(&m.id.as_str())))
}

// ─── JWKS of outside issuers ──────────────────────────────────────────────────

struct CachedJwks {
    at: Instant,
    jwks: Value,
}

fn jwks_cache() -> &'static Mutex<HashMap<String, CachedJwks>> {
    static C: OnceLock<Mutex<HashMap<String, CachedJwks>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build().expect("http client")
}

/// An `https` URL on a public host: what cloud-api may fetch for an outside
/// issuer (no loopback, private or link-local addresses: an org admin must not
/// be able to point cloud-api at its own network).
pub fn public_https_url(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw.trim()) else { return false };
    if u.scheme() != "https" || !valid_url(raw) {
        return false;
    }
    match u.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d.contains('.') && d != "localhost" && !d.ends_with(".localhost") && !d.ends_with(".local") && !d.ends_with(".internal")
        }
        Some(url::Host::Ipv4(ip)) => !(ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1])),
        Some(url::Host::Ipv6(ip)) => !(ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80),
        None => false,
    }
}

async fn fetch_json(url: &str) -> Option<Value> {
    if !public_https_url(url) {
        return None;
    }
    let resp = http().get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// Where an issuer publishes its keys: the row's `jwks_uri`, else its
/// authorization-server / OpenID metadata (whose `issuer` must match).
async fn discover_jwks_uri(iss: &str) -> Option<String> {
    let base = iss.trim_end_matches('/');
    let mut candidates = vec![format!("{base}/.well-known/oauth-authorization-server"), format!("{base}/.well-known/openid-configuration")];
    if let Ok(u) = url::Url::parse(base) {
        let path = u.path().trim_end_matches('/');
        if !path.is_empty() {
            candidates.push(format!("{}/.well-known/oauth-authorization-server{path}", u.origin().ascii_serialization()));
        }
    }
    for url in candidates {
        if let Some(meta) = fetch_json(&url).await {
            if meta["issuer"].as_str().map(|i| i.trim_end_matches('/')) == Some(base) {
                if let Some(uri) = meta["jwks_uri"].as_str().filter(|u| public_https_url(u)) {
                    return Some(uri.to_string());
                }
            }
        }
    }
    None
}

/// The JWK set at `uri`, cached for an hour; `kid` not in the cached set
/// refetches (key rotation), at most once a minute.
async fn jwks_for(uri: &str, kid: Option<&str>) -> Option<Value> {
    let cached = {
        let cache = jwks_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.get(uri).map(|c| (c.at, c.jwks.clone()))
    };
    if let Some((at, jwks)) = &cached {
        let has_kid = kid.is_none_or(|k| jwks["keys"].as_array().is_some_and(|keys| keys.iter().any(|j| j["kid"].as_str() == Some(k))));
        if at.elapsed() < JWKS_TTL && (has_kid || at.elapsed() < JWKS_REFETCH_GAP) {
            return Some(jwks.clone());
        }
    }
    match fetch_json(uri).await.filter(|j| j["keys"].is_array()) {
        Some(jwks) => {
            jwks_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(uri.to_string(), CachedJwks { at: Instant::now(), jwks: jwks.clone() });
            Some(jwks)
        }
        None => cached.map(|(_, j)| j),
    }
}

// ─── the assertion checks ─────────────────────────────────────────────────────

/// Claim checks after the signature: `aud`, `resource`, times, `jti`.
/// Returns `(resource, jti, exp)`.
pub fn check_assertion_claims(header: &Value, claims: &Value, requested_resource: Option<&str>, now: i64) -> Result<(String, String, i64), &'static str> {
    if header["typ"].as_str().map(str::to_ascii_lowercase).as_deref() != Some(IDJAG_TYP) {
        return Err("The assertion is not an ID-JAG (typ oauth-id-jag+jwt)");
    }
    if !es256::has_audience(claims, &edge_as_issuer()) {
        return Err("The assertion is not for this authorization server");
    }
    let resource = claims["resource"].as_str().filter(|r| !r.is_empty()).ok_or("The assertion names no resource")?;
    if requested_resource.is_some_and(|r| r.trim_end_matches('/') != resource.trim_end_matches('/')) {
        return Err("The requested resource differs from the assertion's");
    }
    let (Some(iat), Some(exp)) = (claims["iat"].as_i64(), claims["exp"].as_i64()) else { return Err("The assertion has no iat/exp") };
    if exp <= now - LEEWAY_SECS || iat > now + LEEWAY_SECS || exp <= iat || exp - iat > MAX_ASSERTION_LIFETIME_SECS {
        return Err("The assertion is expired or its lifetime is not acceptable");
    }
    let jti = claims["jti"].as_str().filter(|j| !j.is_empty() && j.len() <= 256).ok_or("The assertion has no jti")?;
    Ok((resource.to_string(), jti.to_string(), exp))
}

/// The email an issuer vouches for. The one issuer-specific step: today every
/// kind uses the spec's `email` claim and refuses `email_verified: false`.
pub fn assertion_email(_kind: &str, claims: &Value) -> Result<String, &'static str> {
    if claims["email_verified"].as_bool() == Some(false) || claims["email_verified"].as_str() == Some("false") {
        return Err("The issuer says the email is not verified");
    }
    claims["email"].as_str().map(str::trim).filter(|e| e.contains('@') && e.len() <= 320).map(str::to_string).ok_or("The assertion carries no email")
}

/// The one user to map to: Clerk users holding the email verified, each with
/// their org ids, against the orgs that may vouch. Exactly one match, never a guess.
pub fn pick_user<'a>(candidates: &'a [(String, Vec<String>)], vouching_orgs: &[&str]) -> Result<(&'a str, &'a str), &'static str> {
    let matches: Vec<(&str, &str)> = candidates
        .iter()
        .filter_map(|(user, orgs)| {
            let mut mine: Vec<&str> = orgs.iter().map(String::as_str).filter(|o| vouching_orgs.contains(o)).collect();
            mine.sort();
            mine.first().map(|org| (user.as_str(), *org))
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err("No Allternit member of an organization that trusts this issuer has that verified email"),
        _ => Err("More than one Allternit account matches that email"),
    }
}

pub fn access_token_claims(user_id: &str, org_id: &str, iss: &str, resource: &str, scope: &str, jti: &str, now: i64) -> Value {
    json!({
        "iss": edge_as_issuer(),
        "sub": user_id,
        "aud": resource,
        "scope": scope,
        "client_id": format!("{ISSUER_CLIENT_PREFIX}{iss}"),
        "org_id": org_id,
        "jti": jti,
        "iat": now,
        "exp": now + EDGE_TOKEN_TTL_SECS,
    })
}

fn grant_error(description: &str) -> Response {
    let mut resp = (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_grant", "error_description": description }))).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    resp
}

fn temporarily_unavailable() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "temporarily_unavailable", "error_description": "Try again shortly" }))).into_response()
}

async fn edge_token(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(key) = es256::key_from_env() else { return not_configured() };
    let f = form_fields(&headers, &body);
    let field = |k: &str| f.get(k).map(|v| v.trim()).filter(|v| !v.is_empty());
    if field("grant_type") != Some(JWT_BEARER) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "unsupported_grant_type", "error_description": "Use urn:ietf:params:oauth:grant-type:jwt-bearer" }))).into_response();
    }
    let Some(assertion) = field("assertion") else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_request", "error_description": "assertion is required" }))).into_response();
    };
    let Some((header, unverified)) = es256::decode_unverified(assertion) else { return grant_error("The assertion is malformed") };
    let Some(iss) = unverified["iss"].as_str().map(str::to_string) else { return grant_error("The assertion has no issuer") };

    let rows = match trusting_rows(&state.db, &iss).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("edge auth: issuer lookup failed: {e}");
            return temporarily_unavailable();
        }
    };
    if rows.is_empty() {
        return grant_error("This issuer is not trusted by any Allternit organization");
    }
    // The signature must check against the keys of the org that trusts the
    // issuer: an org's own jwks_uri never vouches for another org's members.
    let kid = header["kid"].as_str();
    let mut claims: Option<Value> = None;
    let mut vouching: Vec<&IssuerRow> = Vec::new();
    for row in &rows {
        let uri = match &row.jwks_uri {
            Some(u) => Some(u.clone()),
            None => discover_jwks_uri(&iss).await,
        };
        let Some(jwks) = (match uri {
            Some(u) => jwks_for(&u, kid).await,
            None => None,
        }) else {
            continue;
        };
        if let Ok((_, c)) = es256::verify_with_jwks(assertion, &jwks) {
            claims = Some(c);
            vouching.push(row);
        }
    }
    let Some(claims) = claims else { return grant_error("The assertion's signature does not check against the issuer's keys") };
    let now = Utc::now().timestamp();
    let (resource, jti, exp) = match check_assertion_claims(&header, &claims, field("resource"), now) {
        Ok(v) => v,
        Err(m) => return grant_error(m),
    };
    let Some((target, scope)) = edge_target(&resource) else { return grant_error("The resource is not an Allternit edge server this issuer can open") };
    vouching.retain(|r| r.targets.iter().any(|t| t == target));
    if vouching.is_empty() {
        return grant_error("No organization allowed this issuer for that server");
    }
    let email = match assertion_email(&vouching[0].kind, &claims) {
        Ok(e) => e,
        Err(m) => return grant_error(m),
    };

    // One use per assertion, remembered until it expires.
    let seen_key = format!("{iss}|{jti}");
    let _ = sqlx::query("DELETE FROM edge_jti_seen WHERE expires_at < now()").execute(&state.db).await;
    match sqlx::query("INSERT INTO edge_jti_seen (jti, expires_at) VALUES ($1, to_timestamp($2)) ON CONFLICT (jti) DO NOTHING").bind(&seen_key).bind((exp + LEEWAY_SECS) as f64).execute(&state.db).await {
        Ok(r) if r.rows_affected() == 1 => {}
        Ok(_) => return grant_error("The assertion was already used"),
        Err(e) => {
            tracing::error!("edge auth: replay table write failed: {e}");
            return temporarily_unavailable();
        }
    }

    let Ok(users) = crate::services::user_trust::clerk_users_with_verified_email(&email).await else { return temporarily_unavailable() };
    let mut candidates: Vec<(String, Vec<String>)> = Vec::new();
    for user in users {
        let Some(memberships) = crate::services::user_trust::clerk_org_memberships_checked(&user).await else { return temporarily_unavailable() };
        candidates.push((user, memberships.into_iter().map(|m| m.id).collect()));
    }
    let orgs: Vec<&str> = vouching.iter().map(|r| r.org_id.as_str()).collect();
    let (user_id, org_id) = match pick_user(&candidates, &orgs) {
        Ok(v) => v,
        Err(m) => {
            tracing::info!(iss = %iss, jti = %jti, "edge auth: assertion refused: {m}");
            return grant_error(m);
        }
    };
    let token_jti = uuid::Uuid::new_v4().to_string();
    let token = key.sign(EDGE_TOKEN_TYP, &access_token_claims(user_id, org_id, &iss, &resource, scope, &token_jti, now));
    tracing::info!(iss = %iss, assertion_jti = %jti, user = %user_id, org = %org_id, target = %target, "edge auth: assertion accepted, edge token issued");
    let mut resp = Json(json!({ "access_token": token, "token_type": "Bearer", "expires_in": EDGE_TOKEN_TTL_SECS, "scope": scope })).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store"));
    resp
}

// ─── admin API ────────────────────────────────────────────────────────────────

fn issuer_json(r: &IssuerRow) -> Value {
    json!({ "id": r.id, "issuer": r.issuer, "jwks_uri": r.jwks_uri, "kind": r.kind, "label": r.label, "targets": r.targets, "created_at": r.created_at })
}

fn db_error(e: sqlx::Error) -> Response {
    tracing::error!("trusted issuers: {e}");
    coded(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong; try again.")
}

async fn list_route(State(state): State<Arc<ApiState>>, Path(org): Path<String>, headers: HeaderMap) -> Response {
    let caller = match super::managed_auth::org_caller(&headers, &org).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    match sqlx::query_as::<_, IssuerRow>(&format!("SELECT {ISSUER_COLUMNS} FROM mcp_trusted_issuers WHERE org_id = $1 AND revoked_at IS NULL ORDER BY created_at")).bind(&org).fetch_all(&state.db).await {
        Ok(rows) => Json(json!({
            "issuers": rows.iter().map(issuer_json).collect::<Vec<_>>(),
            "edge_issuer": edge_as_issuer(),
            "is_admin": caller.is_admin,
        }))
        .into_response(),
        Err(e) => db_error(e),
    }
}

#[derive(Deserialize)]
struct AddBody {
    issuer: String,
    #[serde(default)]
    jwks_uri: Option<String>,
    kind: String,
    #[serde(default)]
    label: String,
    targets: Vec<String>,
}

fn validate_add(b: &AddBody) -> Result<Vec<String>, &'static str> {
    if !public_https_url(&b.issuer) {
        return Err("issuer must be an https URL.");
    }
    if b.jwks_uri.as_deref().map(str::trim).is_some_and(|u| !u.is_empty() && !public_https_url(u)) {
        return Err("jwks_uri must be an https URL.");
    }
    if !KINDS.contains(&b.kind.as_str()) {
        return Err("kind must be claude, okta or other.");
    }
    let mut targets: Vec<String> = b.targets.iter().map(|t| t.trim().to_string()).collect();
    targets.sort();
    targets.dedup();
    if targets.is_empty() || targets.iter().any(|t| !TARGETS.contains(&t.as_str())) {
        return Err("targets must be connectors and/or agents.");
    }
    Ok(targets)
}

async fn add_route(State(state): State<Arc<ApiState>>, Path(org): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match super::managed_auth::org_caller(&headers, &org).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if !caller.is_admin {
        return super::managed_auth::admin_required();
    }
    let Ok(b) = serde_json::from_slice::<AddBody>(&body) else {
        return coded(StatusCode::BAD_REQUEST, "invalid_request", "Body must be {issuer, jwks_uri?, kind, label, targets}.");
    };
    let targets = match validate_add(&b) {
        Ok(t) => t,
        Err(m) => return coded(StatusCode::BAD_REQUEST, "invalid_request", m),
    };
    let label: String = b.label.trim().chars().take(80).collect();
    let res = sqlx::query_as::<_, IssuerRow>(&format!(
        "INSERT INTO mcp_trusted_issuers (id, org_id, issuer, jwks_uri, kind, label, targets, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (org_id, issuer) WHERE revoked_at IS NULL DO UPDATE SET jwks_uri = EXCLUDED.jwks_uri, kind = EXCLUDED.kind, label = EXCLUDED.label, targets = EXCLUDED.targets
         RETURNING {ISSUER_COLUMNS}"
    ))
    .bind(format!("mti_{}", uuid::Uuid::new_v4().simple()))
    .bind(&org)
    .bind(b.issuer.trim())
    .bind(b.jwks_uri.as_deref().map(str::trim).filter(|u| !u.is_empty()))
    .bind(&b.kind)
    .bind(&label)
    .bind(&targets)
    .bind(&caller.user_id)
    .fetch_one(&state.db)
    .await;
    match res {
        Ok(row) => {
            tracing::info!(org = %org, issuer = %row.issuer, by = %caller.user_id, targets = ?row.targets, "trusted issuer added");
            (StatusCode::CREATED, Json(issuer_json(&row))).into_response()
        }
        Err(e) => db_error(e),
    }
}

async fn revoke_route(State(state): State<Arc<ApiState>>, Path((org, id)): Path<(String, String)>, headers: HeaderMap) -> Response {
    let caller = match super::managed_auth::org_caller(&headers, &org).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if !caller.is_admin {
        return super::managed_auth::admin_required();
    }
    match sqlx::query("UPDATE mcp_trusted_issuers SET revoked_at = now() WHERE id = $1 AND org_id = $2 AND revoked_at IS NULL").bind(&id).bind(&org).execute(&state.db).await {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::info!(org = %org, id = %id, by = %caller.user_id, "trusted issuer removed");
            Json(json!({ "ok": true })).into_response()
        }
        Ok(_) => coded(StatusCode::NOT_FOUND, "not_found", "No such trusted issuer."),
        Err(e) => db_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Value {
        json!({ "typ": "oauth-id-jag+jwt", "alg": "ES256" })
    }

    fn assertion(now: i64) -> Value {
        json!({ "iss": "https://idp.example", "aud": edge_as_issuer(), "resource": "https://mcp.allternit.com/mcp/connectors", "iat": now, "exp": now + 300, "jti": "j1", "email": "ann@acme.com" })
    }

    #[test]
    fn an_assertion_must_be_an_idjag_for_this_as_and_resource_current_and_carry_a_jti() {
        let now = Utc::now().timestamp();
        assert_eq!(check_assertion_claims(&header(), &assertion(now), None, now).unwrap(), ("https://mcp.allternit.com/mcp/connectors".to_string(), "j1".to_string(), now + 300));
        assert!(check_assertion_claims(&json!({ "typ": "JWT" }), &assertion(now), None, now).is_err(), "an id_token is not an ID-JAG");
        let mut a = assertion(now);
        a["aud"] = json!("https://auth.vendor.example");
        assert!(check_assertion_claims(&header(), &a, None, now).is_err(), "wrong aud");
        assert!(check_assertion_claims(&header(), &assertion(now), Some("https://mcp.allternit.com/mcp"), now).is_err(), "resource mismatch");
        assert!(check_assertion_claims(&header(), &assertion(now - 1000), None, now).is_err(), "expired");
        let mut a = assertion(now);
        a["exp"] = json!(now + 86_400);
        assert!(check_assertion_claims(&header(), &a, None, now).is_err(), "lifetime too long");
        let mut a = assertion(now);
        a.as_object_mut().unwrap().remove("jti");
        assert!(check_assertion_claims(&header(), &a, None, now).is_err(), "no jti, no replay protection");
    }

    #[test]
    fn the_user_is_the_one_verified_email_holder_in_a_vouching_org_never_a_guess() {
        let c = |u: &str, orgs: &[&str]| (u.to_string(), orgs.iter().map(|o| o.to_string()).collect::<Vec<_>>());
        assert_eq!(pick_user(&[c("u1", &["org_a"]), c("u2", &["org_z"])], &["org_a"]), Ok(("u1", "org_a")));
        assert!(pick_user(&[], &["org_a"]).is_err(), "0 matches");
        assert!(pick_user(&[c("u2", &["org_z"])], &["org_a"]).is_err(), "holder outside the org");
        assert!(pick_user(&[c("u1", &["org_a"]), c("u2", &["org_a"])], &["org_a"]).is_err(), "2 matches");
        assert!(assertion_email("claude", &json!({ "email": "ann@acme.com", "email_verified": false })).is_err());
        assert!(assertion_email("okta", &json!({})).is_err());
    }

    #[test]
    fn edge_tokens_verify_here_and_a_forged_expired_or_id_jag_typed_one_does_not() {
        let key = es256::Es256Key::from_pem(&es256::testing::pem()).unwrap();
        let now = Utc::now().timestamp();
        let claims = access_token_claims("user_a", "org_a", "https://idp.example", "https://mcp.allternit.com/mcp/connectors", "connectors:use", "t1", now);
        let ok = check_access_token(&key.sign(EDGE_TOKEN_TYP, &claims), &key, now).unwrap();
        assert_eq!((ok["client_id"].as_str(), ok["scope"].as_str()), (Some("issuer:https://idp.example"), Some("connectors:use")));
        assert!(check_access_token(&key.sign(EDGE_TOKEN_TYP, &claims), &key, now + EDGE_TOKEN_TTL_SECS + 120).is_err(), "expired");
        assert!(check_access_token(&key.sign(IDJAG_TYP, &claims), &key, now).is_err(), "an ID-JAG is not an access token");
        let other = es256::Es256Key::from_pem(&es256::testing::pem()).unwrap();
        assert!(check_access_token(&other.sign(EDGE_TOKEN_TYP, &claims), &key, now).is_err(), "someone else's key");
    }

    #[test]
    fn trusted_issuer_input_is_checked() {
        let b = |issuer: &str, kind: &str, targets: &[&str]| AddBody { issuer: issuer.into(), jwks_uri: None, kind: kind.into(), label: String::new(), targets: targets.iter().map(|t| t.to_string()).collect() };
        assert_eq!(validate_add(&b("https://claude.ai/org/1", "claude", &["connectors", "agents", "agents"])).unwrap(), ["agents", "connectors"]);
        assert!(validate_add(&b("https://idp.example", "okta", &["bots"])).is_err(), "bots stay per-user");
        assert!(validate_add(&b("https://idp.example", "okta", &[])).is_err());
        assert!(validate_add(&b("http://idp.example", "okta", &["agents"])).is_err());
        assert!(validate_add(&b("https://idp.example", "google", &["agents"])).is_err());
        assert!(validate_add(&b("https://localhost/x", "other", &["agents"])).is_err(), "no loopback fetches");
        assert!(validate_add(&b("https://10.0.0.5", "other", &["agents"])).is_err(), "no private fetches");
        assert!(validate_add(&b("https://169.254.10.10", "other", &["agents"])).is_err(), "no link-local fetches");
    }
}
