//! OAuth apps for "Sign in with Allternit"
//! (CONTRACT-oauth-linked-billing-2026-11.md, Part A; migration 150).
//!
//! A developer registers an app under a platform project. The app gets a
//! public client id (`oapp_…`) and, when confidential, a client secret
//! (`alt_ocs_…`) that is returned exactly once; only its SHA-256 hash is
//! stored (D1). Public apps are PKCE-only and have no secret (D2).
//!
//! The callers (routes) have already checked that the user may manage the
//! project, as with project API keys. This module enforces the app rules:
//! redirect URIs (A4), scopes (A6), states and the $50 cap before `live`
//! (A7), test users, and an `oauth_audit_log` row for every change (A10).

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use std::net::{Ipv4Addr, Ipv6Addr};
use uuid::Uuid;

use crate::error::ApiError;
use crate::services::api_keys::hash_token;

pub const CLIENT_ID_PREFIX: &str = "oapp_";
pub const CLIENT_SECRET_PREFIX: &str = "alt_ocs_";
const SECRET_ENTROPY_BYTES: usize = 32;
/// Hex characters of the secret kept after the prefix, for display (A3).
const SECRET_DISPLAY_CHARS: usize = 8;
pub const MAX_REDIRECT_URIS: usize = 10;
pub const MAX_TEST_USERS: i64 = 100;
/// Highest monthly cap a user may set while the app is not `live` ($50).
pub const NOT_LIVE_CAP_CENTS: i64 = 5000;
/// The only scopes an app can ask for (D3).
pub const SCOPES: [&str; 4] = ["inference", "usage", "profile", "offline_access"];
const MAX_URL_LEN: usize = 2048;
const MAX_NAME_CHARS: usize = 80;

const APP_COLUMNS: &str = "id, project_id, owner_user_id, client_id, name, logo_url, homepage_url, privacy_url, \
     client_type, secret_hash, secret_prefix, state, scopes, created_at, updated_at, approved_at, approved_by";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientType {
    /// `client_secret_post`; has a secret.
    Confidential,
    /// `none`; PKCE is the only proof.
    Public,
}

impl ClientType {
    pub fn as_str(self) -> &'static str {
        match self {
            ClientType::Confidential => "confidential",
            ClientType::Public => "public",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    Testing,
    InReview,
    Live,
    Suspended,
}

impl AppState {
    pub fn as_str(self) -> &'static str {
        match self {
            AppState::Testing => "testing",
            AppState::InReview => "in_review",
            AppState::Live => "live",
            AppState::Suspended => "suspended",
        }
    }

    /// Unknown values read as `Suspended`, so a bad row fails closed.
    pub fn parse(s: &str) -> AppState {
        match s {
            "testing" => AppState::Testing,
            "in_review" => AppState::InReview,
            "live" => AppState::Live,
            _ => AppState::Suspended,
        }
    }

    /// `None` = the user's choice (live). Every other state caps at $50.
    pub fn max_cap_cents(self) -> Option<i64> {
        match self {
            AppState::Live => None,
            _ => Some(NOT_LIVE_CAP_CENTS),
        }
    }
}

/// Who asks for a state change. The owner may only submit for review or
/// withdraw; going live, suspending and reinstating are admin moves (gate G8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateActor<'a> {
    Owner { project_id: &'a str },
    Admin,
}

/// Events written to `oauth_audit_log` (contract A10, plus `app_deleted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEvent {
    AppCreated,
    AppUpdated,
    AppDeleted,
    SecretRotated,
    RedirectAdded,
    RedirectRemoved,
    TestUserAdded,
    TestUserRemoved,
    AppSubmitted,
    AppApproved,
    AppSuspended,
    RequestApproved,
    RequestDenied,
    CodeIssued,
    CodeReuse,
    TokenIssued,
    RefreshRotated,
    RefreshReuse,
    GrantRevoked,
    CapChanged,
    CapReached,
}

impl AuditEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditEvent::AppCreated => "app_created",
            AuditEvent::AppUpdated => "app_updated",
            AuditEvent::AppDeleted => "app_deleted",
            AuditEvent::SecretRotated => "secret_rotated",
            AuditEvent::RedirectAdded => "redirect_added",
            AuditEvent::RedirectRemoved => "redirect_removed",
            AuditEvent::TestUserAdded => "test_user_added",
            AuditEvent::TestUserRemoved => "test_user_removed",
            AuditEvent::AppSubmitted => "app_submitted",
            AuditEvent::AppApproved => "app_approved",
            AuditEvent::AppSuspended => "app_suspended",
            AuditEvent::RequestApproved => "request_approved",
            AuditEvent::RequestDenied => "request_denied",
            AuditEvent::CodeIssued => "code_issued",
            AuditEvent::CodeReuse => "code_reuse",
            AuditEvent::TokenIssued => "token_issued",
            AuditEvent::RefreshRotated => "refresh_rotated",
            AuditEvent::RefreshReuse => "refresh_reuse",
            AuditEvent::GrantRevoked => "grant_revoked",
            AuditEvent::CapChanged => "cap_changed",
            AuditEvent::CapReached => "cap_reached",
        }
    }
}

/// Who did it, from where. Never put a secret or token in the audit detail.
#[derive(Debug, Clone, Default)]
pub struct AuditCtx {
    pub actor: String,
    pub ip: Option<String>,
    pub ua: Option<String>,
}

/// An app as the owner sees it. The secret hash never serializes.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct OAuthApp {
    pub id: Uuid,
    pub project_id: String,
    pub owner_user_id: String,
    pub client_id: String,
    pub name: String,
    pub logo_url: Option<String>,
    pub homepage_url: Option<String>,
    pub privacy_url: Option<String>,
    pub client_type: String,
    #[serde(skip)]
    pub(crate) secret_hash: Option<String>,
    pub secret_prefix: Option<String>,
    pub state: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
    pub approved_by: Option<String>,
}

impl OAuthApp {
    pub fn app_state(&self) -> AppState {
        AppState::parse(&self.state)
    }

    pub fn is_public(&self) -> bool {
        self.client_type == ClientType::Public.as_str()
    }

    /// `Some(5000)` while the app is not `live`; `None` once it is.
    pub fn max_cap_cents(&self) -> Option<i64> {
        self.app_state().max_cap_cents()
    }

    /// Checks a monthly cap a user picks for this app.
    pub fn check_cap(&self, cap_cents: i64) -> Result<(), ApiError> {
        if cap_cents < 0 || cap_cents > i64::from(i32::MAX) {
            return Err(ApiError::BadRequest("The monthly cap must be between $0 and $21,474,836.47.".to_string()));
        }
        if let Some(max) = self.max_cap_cents() {
            if cap_cents > max {
                return Err(ApiError::BadRequest(format!(
                    "This app is not live yet, so the monthly cap can be at most ${}.",
                    max / 100
                )));
            }
        }
        Ok(())
    }

    /// True when `presented` is this app's current client secret.
    pub fn verify_secret(&self, presented: &str) -> bool {
        let Some(stored) = self.secret_hash.as_deref() else {
            return false;
        };
        let presented = hash_token(presented);
        stored.len() == presented.len()
            && stored.bytes().zip(presented.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
    }
}

/// Returned once, at creation. `client_secret` is `None` for public apps.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedOAuthApp {
    #[serde(flatten)]
    pub app: OAuthApp,
    pub redirect_uris: Vec<String>,
    pub client_secret: Option<String>,
}

/// Returned once, after a rotate.
#[derive(Debug, Clone, Serialize)]
pub struct RotatedSecret {
    #[serde(flatten)]
    pub app: OAuthApp,
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct TestUser {
    pub email: String,
    pub user_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct CreateOAuthAppInput {
    pub project_id: String,
    pub owner_user_id: String,
    pub name: String,
    pub client_type: ClientType,
    pub logo_url: Option<String>,
    pub homepage_url: Option<String>,
    pub privacy_url: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_uris: Vec<String>,
}

/// `None` leaves a field as is. For the URL fields, `Some("")` clears it.
#[derive(Debug, Default)]
pub struct UpdateOAuthAppInput {
    pub name: Option<String>,
    pub logo_url: Option<String>,
    pub homepage_url: Option<String>,
    pub privacy_url: Option<String>,
    pub scopes: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Validation (pure)
// ---------------------------------------------------------------------------

/// Contract A4: https, or http only for `localhost`, `127.0.0.1` or `[::1]`
/// (any port). No custom schemes, fragments, wildcards or userinfo. The URI is
/// stored and later matched as the exact string given.
pub fn validate_redirect_uri(uri: &str) -> Result<(), ApiError> {
    let bad = |why: &str| -> Result<(), ApiError> {
        Err(ApiError::BadRequest(format!("Redirect URI '{uri}' is not allowed: {why}.")))
    };
    if uri.is_empty() || uri.len() > MAX_URL_LEN {
        return bad("it must be 1 to 2048 characters");
    }
    if uri.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return bad("it contains spaces or control characters");
    }
    if uri.contains('*') {
        return bad("wildcards are not supported");
    }
    if uri.contains('#') {
        return bad("it has a fragment");
    }
    let parsed = match url::Url::parse(uri) {
        Ok(u) => u,
        Err(_) => return bad("it is not an absolute URL"),
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return bad("it has a user name or password");
    }
    match parsed.scheme() {
        "https" => {
            if parsed.host().is_none() {
                return bad("it has no host");
            }
        }
        "http" => {
            let loopback = match parsed.host() {
                Some(url::Host::Domain(d)) => d == "localhost",
                Some(url::Host::Ipv4(ip)) => ip == Ipv4Addr::LOCALHOST,
                Some(url::Host::Ipv6(ip)) => ip == Ipv6Addr::LOCALHOST,
                None => false,
            };
            if !loopback {
                return bad("http is allowed only for localhost, 127.0.0.1 or [::1]; use https");
            }
        }
        _ => return bad("custom schemes are not supported; use https"),
    }
    Ok(())
}

/// Checks every URI, drops exact duplicates (first one wins), then the max.
pub fn normalize_redirect_uris(uris: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut out: Vec<String> = Vec::new();
    for uri in uris {
        validate_redirect_uri(&uri)?;
        if !out.contains(&uri) {
            out.push(uri);
        }
    }
    if out.len() > MAX_REDIRECT_URIS {
        return Err(ApiError::BadRequest(format!("An app can have at most {MAX_REDIRECT_URIS} redirect URIs.")));
    }
    Ok(out)
}

/// Known scopes only (D3), at least one, in the contract's order.
pub fn normalize_app_scopes(scopes: Vec<String>) -> Result<Vec<String>, ApiError> {
    let wanted: Vec<String> = scopes.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if let Some(unknown) = wanted.iter().find(|s| !SCOPES.contains(&s.as_str())) {
        return Err(ApiError::BadRequest(format!(
            "Unknown scope '{unknown}'. Allowed: {}.",
            SCOPES.join(", ")
        )));
    }
    let out: Vec<String> = SCOPES.iter().filter(|s| wanted.iter().any(|w| w.as_str() == **s)).map(|s| s.to_string()).collect();
    if out.is_empty() {
        return Err(ApiError::BadRequest("An app needs at least one scope.".to_string()));
    }
    Ok(out)
}

fn normalize_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::BadRequest(format!("The app name must be 1 to {MAX_NAME_CHARS} characters.")));
    }
    Ok(name.to_string())
}

/// Logo, homepage and privacy links: https only. Empty means none.
fn normalize_info_url(field: &str, value: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let ok = value.len() <= MAX_URL_LEN
        && url::Url::parse(&value).is_ok_and(|u| u.scheme() == "https" && u.host().is_some());
    if !ok {
        return Err(ApiError::BadRequest(format!("The {field} must be an https URL.")));
    }
    Ok(Some(value))
}

/// An update's URL field: `None` keeps the old value, `Some("")` clears it.
fn merge_info_url(field: &str, new: Option<String>, old: &Option<String>) -> Result<Option<String>, ApiError> {
    match new {
        Some(v) => normalize_info_url(field, Some(v)),
        None => Ok(old.clone()),
    }
}

fn normalize_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= 254
        && !email.chars().any(char::is_whitespace)
        && matches!(email.split_once('@'), Some((local, domain)) if !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
    if !valid {
        return Err(ApiError::BadRequest("Enter a valid email address.".to_string()));
    }
    Ok(email)
}

fn generate_client_id() -> String {
    format!("{CLIENT_ID_PREFIX}{}", hex::encode(rand::random::<[u8; 16]>()))
}

fn generate_secret() -> String {
    let mut entropy = [0u8; SECRET_ENTROPY_BYTES];
    rand::thread_rng().fill_bytes(&mut entropy);
    format!("{CLIENT_SECRET_PREFIX}{}", hex::encode(entropy))
}

fn secret_display_prefix(secret: &str) -> String {
    secret.chars().take(CLIENT_SECRET_PREFIX.len() + SECRET_DISPLAY_CHARS).collect()
}

fn not_found() -> ApiError {
    ApiError::NotFound("OAuth app not found".to_string())
}

// ---------------------------------------------------------------------------
// Audit
// ---------------------------------------------------------------------------

/// Writes one `oauth_audit_log` row. `detail` must hold no secret or token.
pub async fn record_audit<'e, E>(
    exec: E,
    ctx: &AuditCtx,
    app_id: Option<Uuid>,
    grant_id: Option<Uuid>,
    event: AuditEvent,
    detail: serde_json::Value,
) -> Result<(), ApiError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    sqlx::query(
        "INSERT INTO oauth_audit_log (actor, app_id, grant_id, event, ip, ua, detail) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&ctx.actor)
    .bind(app_id)
    .bind(grant_id)
    .bind(event.as_str())
    .bind(&ctx.ip)
    .bind(&ctx.ua)
    .bind(sqlx::types::Json(detail))
    .execute(exec)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Apps
// ---------------------------------------------------------------------------

/// Locks the app row for the rest of the transaction. `project_id` scopes it
/// to the owner's project; `None` is the admin path.
async fn lock_app(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Option<&str>,
    app_id: Uuid,
) -> Result<OAuthApp, ApiError> {
    sqlx::query_as::<_, OAuthApp>(&format!(
        "SELECT {APP_COLUMNS} FROM oauth_apps WHERE id = $1 AND ($2::text IS NULL OR project_id = $2) FOR UPDATE"
    ))
    .bind(app_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(not_found)
}

/// Registers an app. The client secret (confidential apps) is in the result
/// and nowhere else: only its hash is stored.
pub async fn create_app(db: &PgPool, input: CreateOAuthAppInput, ctx: &AuditCtx) -> Result<CreatedOAuthApp, ApiError> {
    let name = normalize_name(&input.name)?;
    let logo_url = normalize_info_url("logo URL", input.logo_url)?;
    let homepage_url = normalize_info_url("homepage URL", input.homepage_url)?;
    let privacy_url = normalize_info_url("privacy policy URL", input.privacy_url)?;
    let scopes = normalize_app_scopes(input.scopes)?;
    let redirect_uris = normalize_redirect_uris(input.redirect_uris)?;

    let secret = (input.client_type == ClientType::Confidential).then(generate_secret);
    let app_id = Uuid::new_v4();

    let mut tx = db.begin().await?;
    let app = sqlx::query_as::<_, OAuthApp>(&format!(
        r#"
        INSERT INTO oauth_apps (id, project_id, owner_user_id, client_id, name, logo_url, homepage_url, privacy_url,
                                client_type, secret_hash, secret_prefix, scopes)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING {APP_COLUMNS}
        "#
    ))
    .bind(app_id)
    .bind(&input.project_id)
    .bind(&input.owner_user_id)
    .bind(generate_client_id())
    .bind(&name)
    .bind(&logo_url)
    .bind(&homepage_url)
    .bind(&privacy_url)
    .bind(input.client_type.as_str())
    .bind(secret.as_deref().map(hash_token))
    .bind(secret.as_deref().map(secret_display_prefix))
    .bind(&scopes)
    .fetch_one(&mut *tx)
    .await?;

    for uri in &redirect_uris {
        sqlx::query("INSERT INTO oauth_app_redirect_uris (id, app_id, uri) VALUES ($1, $2, $3)")
            .bind(Uuid::new_v4())
            .bind(app_id)
            .bind(uri)
            .execute(&mut *tx)
            .await?;
    }

    record_audit(
        &mut *tx,
        ctx,
        Some(app_id),
        None,
        AuditEvent::AppCreated,
        json!({ "client_type": app.client_type, "scopes": app.scopes, "redirect_uris": redirect_uris }),
    )
    .await?;
    tx.commit().await?;

    Ok(CreatedOAuthApp { app, redirect_uris, client_secret: secret })
}

pub async fn get_app(db: &PgPool, project_id: &str, app_id: Uuid) -> Result<OAuthApp, ApiError> {
    sqlx::query_as::<_, OAuthApp>(&format!("SELECT {APP_COLUMNS} FROM oauth_apps WHERE id = $1 AND project_id = $2"))
        .bind(app_id)
        .bind(project_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(not_found)
}

/// The authorization server's lookup (`/oauth2/authorize`, `/oauth2/token`).
pub async fn get_app_by_client_id(db: &PgPool, client_id: &str) -> Result<Option<OAuthApp>, ApiError> {
    Ok(sqlx::query_as::<_, OAuthApp>(&format!("SELECT {APP_COLUMNS} FROM oauth_apps WHERE client_id = $1"))
        .bind(client_id)
        .fetch_optional(db)
        .await?)
}

pub async fn list_apps(db: &PgPool, project_id: &str) -> Result<Vec<OAuthApp>, ApiError> {
    Ok(sqlx::query_as::<_, OAuthApp>(&format!(
        "SELECT {APP_COLUMNS} FROM oauth_apps WHERE project_id = $1 ORDER BY created_at, id"
    ))
    .bind(project_id)
    .fetch_all(db)
    .await?)
}

/// An app with its redirect URIs: what the console and the admin review list
/// return. The secret hash never serializes (see [`OAuthApp`]).
#[derive(Debug, Clone, Serialize)]
pub struct AppView {
    #[serde(flatten)]
    pub app: OAuthApp,
    pub redirect_uris: Vec<String>,
}

pub async fn app_view(db: &PgPool, app: OAuthApp) -> Result<AppView, ApiError> {
    let redirect_uris = list_redirect_uris(db, app.id).await?;
    Ok(AppView { app, redirect_uris })
}

/// An app by id, in any project (the grant routes and the admin routes).
pub async fn get_app_by_id<'e, E>(db: E, app_id: Uuid) -> Result<OAuthApp, ApiError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    sqlx::query_as::<_, OAuthApp>(&format!("SELECT {APP_COLUMNS} FROM oauth_apps WHERE id = $1"))
        .bind(app_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(not_found)
}

/// Apps in any project, oldest change first, for the admin review queue.
/// `None` lists every state. At most 200.
pub async fn list_apps_by_state(db: &PgPool, state: Option<AppState>) -> Result<Vec<OAuthApp>, ApiError> {
    Ok(sqlx::query_as::<_, OAuthApp>(&format!(
        "SELECT {APP_COLUMNS} FROM oauth_apps WHERE ($1::text IS NULL OR state = $1) ORDER BY updated_at, id LIMIT 200"
    ))
    .bind(state.map(AppState::as_str))
    .fetch_all(db)
    .await?)
}

pub async fn update_app(
    db: &PgPool,
    project_id: &str,
    app_id: Uuid,
    input: UpdateOAuthAppInput,
    ctx: &AuditCtx,
) -> Result<OAuthApp, ApiError> {
    let mut tx = db.begin().await?;
    let app = lock_app(&mut tx, Some(project_id), app_id).await?;

    let mut changed: Vec<&str> = Vec::new();
    let name = match input.name {
        Some(n) => normalize_name(&n)?,
        None => app.name.clone(),
    };
    if name != app.name {
        changed.push("name");
    }
    let logo_url = merge_info_url("logo URL", input.logo_url, &app.logo_url)?;
    let homepage_url = merge_info_url("homepage URL", input.homepage_url, &app.homepage_url)?;
    let privacy_url = merge_info_url("privacy policy URL", input.privacy_url, &app.privacy_url)?;
    for (key, new, old) in [
        ("logo_url", &logo_url, &app.logo_url),
        ("homepage_url", &homepage_url, &app.homepage_url),
        ("privacy_url", &privacy_url, &app.privacy_url),
    ] {
        if new != old {
            changed.push(key);
        }
    }
    let scopes = match input.scopes {
        Some(s) => normalize_app_scopes(s)?,
        None => app.scopes.clone(),
    };
    if scopes != app.scopes {
        changed.push("scopes");
    }

    if changed.is_empty() {
        tx.commit().await?;
        return Ok(app);
    }

    let updated = sqlx::query_as::<_, OAuthApp>(&format!(
        r#"
        UPDATE oauth_apps SET name = $2, logo_url = $3, homepage_url = $4, privacy_url = $5, scopes = $6, updated_at = now()
        WHERE id = $1
        RETURNING {APP_COLUMNS}
        "#
    ))
    .bind(app_id)
    .bind(&name)
    .bind(&logo_url)
    .bind(&homepage_url)
    .bind(&privacy_url)
    .bind(&scopes)
    .fetch_one(&mut *tx)
    .await?;

    record_audit(&mut *tx, ctx, Some(app_id), None, AuditEvent::AppUpdated, json!({ "fields": changed })).await?;
    tx.commit().await?;
    Ok(updated)
}

/// Deletes the app. Its redirect URIs, test users, consent requests, grants,
/// tokens and spend rows go with it (cascade), so every token stops working.
pub async fn delete_app(db: &PgPool, project_id: &str, app_id: Uuid, ctx: &AuditCtx) -> Result<(), ApiError> {
    let mut tx = db.begin().await?;
    let app = lock_app(&mut tx, Some(project_id), app_id).await?;
    sqlx::query("DELETE FROM oauth_apps WHERE id = $1").bind(app_id).execute(&mut *tx).await?;
    record_audit(
        &mut *tx,
        ctx,
        Some(app_id),
        None,
        AuditEvent::AppDeleted,
        json!({ "client_id": app.client_id, "state": app.state }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Replaces a confidential app's secret. The old one stops working at once.
pub async fn rotate_secret(
    db: &PgPool,
    project_id: &str,
    app_id: Uuid,
    ctx: &AuditCtx,
) -> Result<RotatedSecret, ApiError> {
    let mut tx = db.begin().await?;
    let app = lock_app(&mut tx, Some(project_id), app_id).await?;
    if app.is_public() {
        return Err(ApiError::BadRequest("Public apps use PKCE and have no client secret.".to_string()));
    }
    let secret = generate_secret();
    let prefix = secret_display_prefix(&secret);
    let updated = sqlx::query_as::<_, OAuthApp>(&format!(
        "UPDATE oauth_apps SET secret_hash = $2, secret_prefix = $3, updated_at = now() WHERE id = $1 RETURNING {APP_COLUMNS}"
    ))
    .bind(app_id)
    .bind(hash_token(&secret))
    .bind(&prefix)
    .fetch_one(&mut *tx)
    .await?;
    record_audit(&mut *tx, ctx, Some(app_id), None, AuditEvent::SecretRotated, json!({ "secret_prefix": prefix })).await?;
    tx.commit().await?;
    Ok(RotatedSecret { app: updated, client_secret: secret })
}

// ---------------------------------------------------------------------------
// States
// ---------------------------------------------------------------------------

/// `testing` → `in_review` → `live`; `in_review` → `testing` (withdrawn or
/// sent back); anything but `suspended` → `suspended`; `suspended` →
/// `testing` (reinstated, reviewed again before live).
pub fn can_transition(from: AppState, to: AppState, actor: StateActor<'_>) -> bool {
    use AppState::*;
    let allowed = matches!(
        (from, to),
        (Testing, InReview) | (InReview, Testing) | (InReview, Live) | (Testing | InReview | Live, Suspended) | (Suspended, Testing)
    );
    let owner_may = matches!((from, to), (Testing, InReview) | (InReview, Testing));
    allowed && (actor == StateActor::Admin || owner_may)
}

pub async fn set_state(
    db: &PgPool,
    app_id: Uuid,
    to: AppState,
    actor: StateActor<'_>,
    ctx: &AuditCtx,
) -> Result<OAuthApp, ApiError> {
    let scope = match actor {
        StateActor::Owner { project_id } => Some(project_id),
        StateActor::Admin => None,
    };
    let mut tx = db.begin().await?;
    let app = lock_app(&mut tx, scope, app_id).await?;
    let from = app.app_state();
    if !can_transition(from, to, actor) {
        let msg = format!("An app in '{}' cannot move to '{}' this way.", from.as_str(), to.as_str());
        return Err(if matches!(actor, StateActor::Owner { .. }) && can_transition(from, to, StateActor::Admin) {
            ApiError::Forbidden(msg)
        } else {
            ApiError::Conflict(msg)
        });
    }

    let updated = sqlx::query_as::<_, OAuthApp>(&format!(
        r#"
        UPDATE oauth_apps
        SET state = $2,
            approved_at = CASE WHEN $2 = 'live' THEN now() ELSE approved_at END,
            approved_by = CASE WHEN $2 = 'live' THEN $3 ELSE approved_by END,
            updated_at = now()
        WHERE id = $1
        RETURNING {APP_COLUMNS}
        "#
    ))
    .bind(app_id)
    .bind(to.as_str())
    .bind(&ctx.actor)
    .fetch_one(&mut *tx)
    .await?;

    let event = match to {
        AppState::InReview => AuditEvent::AppSubmitted,
        AppState::Live => AuditEvent::AppApproved,
        AppState::Suspended => AuditEvent::AppSuspended,
        AppState::Testing => AuditEvent::AppUpdated,
    };
    record_audit(
        &mut *tx,
        ctx,
        Some(app_id),
        None,
        event,
        json!({ "from": from.as_str(), "to": to.as_str() }),
    )
    .await?;
    tx.commit().await?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Redirect URIs
// ---------------------------------------------------------------------------

pub async fn list_redirect_uris(db: &PgPool, app_id: Uuid) -> Result<Vec<String>, ApiError> {
    Ok(sqlx::query_scalar::<_, String>("SELECT uri FROM oauth_app_redirect_uris WHERE app_id = $1 ORDER BY created_at, uri")
        .bind(app_id)
        .fetch_all(db)
        .await?)
}

pub async fn add_redirect_uri(
    db: &PgPool,
    project_id: &str,
    app_id: Uuid,
    uri: &str,
    ctx: &AuditCtx,
) -> Result<Vec<String>, ApiError> {
    validate_redirect_uri(uri)?;
    let mut tx = db.begin().await?;
    lock_app(&mut tx, Some(project_id), app_id).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_app_redirect_uris WHERE app_id = $1")
        .bind(app_id)
        .fetch_one(&mut *tx)
        .await?;
    if count >= MAX_REDIRECT_URIS as i64 {
        return Err(ApiError::BadRequest(format!("An app can have at most {MAX_REDIRECT_URIS} redirect URIs.")));
    }
    let inserted = sqlx::query(
        "INSERT INTO oauth_app_redirect_uris (id, app_id, uri) VALUES ($1, $2, $3) ON CONFLICT (app_id, uri) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(app_id)
    .bind(uri)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(ApiError::Conflict("That redirect URI is already registered.".to_string()));
    }
    record_audit(&mut *tx, ctx, Some(app_id), None, AuditEvent::RedirectAdded, json!({ "uri": uri })).await?;
    tx.commit().await?;
    list_redirect_uris(db, app_id).await
}

pub async fn remove_redirect_uri(
    db: &PgPool,
    project_id: &str,
    app_id: Uuid,
    uri: &str,
    ctx: &AuditCtx,
) -> Result<Vec<String>, ApiError> {
    let mut tx = db.begin().await?;
    lock_app(&mut tx, Some(project_id), app_id).await?;
    let removed = sqlx::query("DELETE FROM oauth_app_redirect_uris WHERE app_id = $1 AND uri = $2")
        .bind(app_id)
        .bind(uri)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(ApiError::NotFound("Redirect URI not found".to_string()));
    }
    record_audit(&mut *tx, ctx, Some(app_id), None, AuditEvent::RedirectRemoved, json!({ "uri": uri })).await?;
    tx.commit().await?;
    list_redirect_uris(db, app_id).await
}

// ---------------------------------------------------------------------------
// Test users
// ---------------------------------------------------------------------------

pub async fn list_test_users(db: &PgPool, app_id: Uuid) -> Result<Vec<TestUser>, ApiError> {
    Ok(sqlx::query_as::<_, TestUser>(
        "SELECT email, user_id, created_at FROM oauth_app_test_users WHERE app_id = $1 ORDER BY created_at, email",
    )
    .bind(app_id)
    .fetch_all(db)
    .await?)
}

/// Adds a test user by email (case-insensitive). The account is linked now if
/// it exists; otherwise the email alone matches when that person signs up.
pub async fn add_test_user(
    db: &PgPool,
    project_id: &str,
    app_id: Uuid,
    email: &str,
    ctx: &AuditCtx,
) -> Result<TestUser, ApiError> {
    let email = normalize_email(email)?;
    let mut tx = db.begin().await?;
    lock_app(&mut tx, Some(project_id), app_id).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM oauth_app_test_users WHERE app_id = $1")
        .bind(app_id)
        .fetch_one(&mut *tx)
        .await?;
    if count >= MAX_TEST_USERS {
        return Err(ApiError::BadRequest(format!("An app can have at most {MAX_TEST_USERS} test users.")));
    }
    let added = sqlx::query_as::<_, TestUser>(
        r#"
        INSERT INTO oauth_app_test_users (app_id, email, user_id)
        VALUES ($1, $2, (SELECT id FROM users WHERE lower(email) = $2 LIMIT 1))
        ON CONFLICT (app_id, email) DO NOTHING
        RETURNING email, user_id, created_at
        "#,
    )
    .bind(app_id)
    .bind(&email)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Conflict("That person is already a test user.".to_string()))?;
    // The address stays out of the audit trail; account erase keeps that trail.
    record_audit(
        &mut *tx,
        ctx,
        Some(app_id),
        None,
        AuditEvent::TestUserAdded,
        json!({ "user_id": added.user_id, "test_users": count + 1 }),
    )
    .await?;
    tx.commit().await?;
    Ok(added)
}

pub async fn remove_test_user(
    db: &PgPool,
    project_id: &str,
    app_id: Uuid,
    email: &str,
    ctx: &AuditCtx,
) -> Result<(), ApiError> {
    let email = normalize_email(email)?;
    let mut tx = db.begin().await?;
    lock_app(&mut tx, Some(project_id), app_id).await?;
    let removed: Option<Option<String>> =
        sqlx::query_scalar("DELETE FROM oauth_app_test_users WHERE app_id = $1 AND email = $2 RETURNING user_id")
            .bind(app_id)
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(user_id) = removed else {
        return Err(ApiError::NotFound("Test user not found".to_string()));
    };
    record_audit(&mut *tx, ctx, Some(app_id), None, AuditEvent::TestUserRemoved, json!({ "user_id": user_id })).await?;
    tx.commit().await?;
    Ok(())
}

/// Contract A7: anyone for a `live` app, nobody for a `suspended` one, and
/// otherwise only the owner and the listed test users.
pub async fn user_may_authorize(
    db: &PgPool,
    app: &OAuthApp,
    user_id: &str,
    email: Option<&str>,
) -> Result<bool, ApiError> {
    match app.app_state() {
        AppState::Live => return Ok(true),
        AppState::Suspended => return Ok(false),
        AppState::Testing | AppState::InReview => {}
    }
    if app.owner_user_id == user_id {
        return Ok(true);
    }
    let email = email.map(|e| e.trim().to_lowercase());
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM oauth_app_test_users WHERE app_id = $1 AND (user_id = $2 OR email = $3))",
    )
    .bind(app.id)
    .bind(user_id)
    .bind(email)
    .fetch_one(db)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "proj_oauth_test";
    const OWNER: &str = "user_owner";

    /// A fresh schema holding the real migration 150, plus the two tables it
    /// reads from earlier migrations.
    async fn test_pool() -> PgPool {
        let url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string());
        let schema = format!("test_{}", Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}")).execute(&mut *conn).await?;
                    sqlx::query(&format!("SET search_path TO {schema}")).execute(&mut *conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE platform_projects (id TEXT PRIMARY KEY);
             CREATE TABLE users (id TEXT PRIMARY KEY, email TEXT);",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations_pg/150_oauth_apps.sql")).execute(&pool).await.expect("150 applies");
        sqlx::raw_sql(include_str!("../../migrations_pg/150_oauth_apps.sql")).execute(&pool).await.expect("150 re-applies");
        sqlx::query("INSERT INTO platform_projects (id) VALUES ($1)").bind(PROJECT).execute(&pool).await.unwrap();
        pool
    }

    fn ctx() -> AuditCtx {
        AuditCtx { actor: OWNER.to_string(), ip: Some("203.0.113.7".to_string()), ua: Some("test".to_string()) }
    }

    fn input(client_type: ClientType, redirect_uris: Vec<String>) -> CreateOAuthAppInput {
        CreateOAuthAppInput {
            project_id: PROJECT.to_string(),
            owner_user_id: OWNER.to_string(),
            name: "Notes Sync".to_string(),
            client_type,
            logo_url: None,
            homepage_url: Some("https://notes.example.com".to_string()),
            privacy_url: None,
            scopes: vec!["inference".to_string(), "offline_access".to_string()],
            redirect_uris,
        }
    }

    fn uris(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("https://notes.example.com/cb/{i}")).collect()
    }

    async fn count(db: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
    }

    fn is_bad_request(r: Result<(), ApiError>) -> bool {
        matches!(r, Err(ApiError::BadRequest(_)))
    }

    #[test]
    fn redirect_custom_scheme_rejected() {
        assert!(is_bad_request(validate_redirect_uri("myapp://cb")));
        assert!(is_bad_request(validate_redirect_uri("com.example.app:/oauth")));
    }

    #[test]
    fn redirect_http_non_localhost_rejected() {
        assert!(is_bad_request(validate_redirect_uri("http://example.com/cb")));
        // A name that only starts with "localhost" is not loopback.
        assert!(is_bad_request(validate_redirect_uri("http://localhost.example.com/cb")));
    }

    #[test]
    fn redirect_localhost_accepted() {
        assert!(validate_redirect_uri("http://localhost:8080/cb").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1:3000/callback?x=1").is_ok());
        assert!(validate_redirect_uri("https://notes.example.com/cb").is_ok());
    }

    #[test]
    fn redirect_localhost_ipv6_accepted() {
        assert!(validate_redirect_uri("http://[::1]:5173/cb").is_ok());
    }

    #[test]
    fn redirect_fragment_rejected() {
        assert!(is_bad_request(validate_redirect_uri("https://notes.example.com/cb#done")));
        assert!(is_bad_request(validate_redirect_uri("https://notes.example.com/cb#")));
    }

    #[test]
    fn redirect_wildcard_and_userinfo_rejected() {
        assert!(is_bad_request(validate_redirect_uri("https://*.example.com/cb")));
        assert!(is_bad_request(validate_redirect_uri("https://user:pw@notes.example.com/cb")));
    }

    #[tokio::test]
    async fn redirect_eleventh_rejected() {
        let db = test_pool().await;

        // Eleven at creation: refused, nothing stored.
        assert!(matches!(
            create_app(&db, input(ClientType::Public, uris(11)), &ctx()).await,
            Err(ApiError::BadRequest(_))
        ));
        assert_eq!(count(&db, "SELECT count(*) FROM oauth_apps").await, 0);

        // Ten at creation, then an eleventh added later: refused, still ten.
        let created = create_app(&db, input(ClientType::Public, uris(10)), &ctx()).await.unwrap();
        assert_eq!(created.redirect_uris.len(), 10);
        let eleventh = add_redirect_uri(&db, PROJECT, created.app.id, "https://notes.example.com/cb/10", &ctx()).await;
        assert!(matches!(eleventh, Err(ApiError::BadRequest(_))));
        assert_eq!(list_redirect_uris(&db, created.app.id).await.unwrap().len(), 10);

        // After one is removed, a tenth fits again and is audited.
        remove_redirect_uri(&db, PROJECT, created.app.id, "https://notes.example.com/cb/0", &ctx()).await.unwrap();
        let now = add_redirect_uri(&db, PROJECT, created.app.id, "http://localhost:8080/cb", &ctx()).await.unwrap();
        assert_eq!(now.len(), 10);
        assert!(now.contains(&"http://localhost:8080/cb".to_string()));
        assert_eq!(
            count(&db, "SELECT count(*) FROM oauth_audit_log WHERE event IN ('redirect_added', 'redirect_removed')").await,
            2
        );
    }

    #[tokio::test]
    async fn client_secret_shown_once_stored_as_hash() {
        let db = test_pool().await;
        let created = create_app(&db, input(ClientType::Confidential, uris(1)), &ctx()).await.unwrap();
        let secret = created.client_secret.clone().expect("confidential apps get a secret");
        assert!(secret.starts_with(CLIENT_SECRET_PREFIX));
        assert_eq!(secret.len(), CLIENT_SECRET_PREFIX.len() + 64);
        assert!(created.app.client_id.starts_with(CLIENT_ID_PREFIX));

        // Stored: the hash and a display prefix. The secret is in no column.
        let (hash, prefix): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT secret_hash, secret_prefix FROM oauth_apps WHERE id = $1")
                .bind(created.app.id)
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(hash.as_deref(), Some(hash_token(&secret).as_str()));
        assert_eq!(prefix.as_deref(), Some(&secret[..CLIENT_SECRET_PREFIX.len() + 8]));
        let leaks: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM oauth_apps a WHERE row_to_json(a)::text LIKE '%' || $1 || '%')
                  + (SELECT count(*) FROM oauth_audit_log l WHERE row_to_json(l)::text LIKE '%' || $1 || '%')",
        )
        .bind(&secret)
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(leaks, 0);

        // Read back later: no secret and no hash in what the owner gets.
        let read = get_app(&db, PROJECT, created.app.id).await.unwrap();
        let body = serde_json::to_string(&read).unwrap();
        assert!(!body.contains(&secret) && !body.contains("secret_hash") && !body.contains(&hash_token(&secret)));
        assert!(read.verify_secret(&secret));

        // Rotate: a new secret, once; the old one stops matching.
        let rotated = rotate_secret(&db, PROJECT, created.app.id, &ctx()).await.unwrap();
        assert_ne!(rotated.client_secret, secret);
        let read = get_app(&db, PROJECT, created.app.id).await.unwrap();
        assert!(read.verify_secret(&rotated.client_secret));
        assert!(!read.verify_secret(&secret));
        assert_eq!(
            count(&db, "SELECT count(*) FROM oauth_audit_log WHERE event IN ('app_created', 'secret_rotated')").await,
            2
        );

        // A public app has no secret and nothing to rotate.
        let public = create_app(&db, input(ClientType::Public, uris(1)), &ctx()).await.unwrap();
        assert!(public.client_secret.is_none());
        assert!(!public.app.verify_secret(""));
        assert!(matches!(rotate_secret(&db, PROJECT, public.app.id, &ctx()).await, Err(ApiError::BadRequest(_))));
    }

    #[tokio::test]
    async fn testing_app_cap_over_50_rejected() {
        let db = test_pool().await;
        let app = create_app(&db, input(ClientType::Public, uris(1)), &ctx()).await.unwrap().app;
        assert_eq!(app.app_state(), AppState::Testing);
        assert_eq!(app.max_cap_cents(), Some(5000));
        assert!(matches!(app.check_cap(5001), Err(ApiError::BadRequest(_))));
        assert!(app.check_cap(5000).is_ok());

        // The owner can submit but cannot approve; an admin can.
        let owner = StateActor::Owner { project_id: PROJECT };
        let in_review = set_state(&db, app.id, AppState::InReview, owner, &ctx()).await.unwrap();
        assert!(matches!(in_review.check_cap(5001), Err(ApiError::BadRequest(_))));
        assert!(matches!(
            set_state(&db, app.id, AppState::Live, owner, &ctx()).await,
            Err(ApiError::Forbidden(_))
        ));
        let admin = AuditCtx { actor: "admin_1".to_string(), ..ctx() };
        let live = set_state(&db, app.id, AppState::Live, StateActor::Admin, &admin).await.unwrap();
        assert_eq!(live.approved_by.as_deref(), Some("admin_1"));
        assert_eq!(live.max_cap_cents(), None);
        assert!(live.check_cap(100_000).is_ok());

        let events: Vec<String> =
            sqlx::query_scalar("SELECT event FROM oauth_audit_log WHERE app_id = $1 ORDER BY id")
                .bind(app.id)
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(events, ["app_created", "app_submitted", "app_approved"]);
    }

    #[tokio::test]
    async fn test_users_capped_at_100_and_case_insensitive() {
        let db = test_pool().await;
        sqlx::query("INSERT INTO users (id, email) VALUES ('user_ada', 'Ada@Example.com')").execute(&db).await.unwrap();
        let app = create_app(&db, input(ClientType::Public, uris(1)), &ctx()).await.unwrap().app;

        let ada = add_test_user(&db, PROJECT, app.id, " ADA@example.com ", &ctx()).await.unwrap();
        assert_eq!(ada.email, "ada@example.com");
        assert_eq!(ada.user_id.as_deref(), Some("user_ada"));
        assert!(matches!(
            add_test_user(&db, PROJECT, app.id, "ada@EXAMPLE.com", &ctx()).await,
            Err(ApiError::Conflict(_))
        ));
        assert!(user_may_authorize(&db, &app, "user_ada", None).await.unwrap());
        assert!(!user_may_authorize(&db, &app, "user_other", Some("other@example.com")).await.unwrap());

        for i in 1..100 {
            add_test_user(&db, PROJECT, app.id, &format!("tester{i}@example.com"), &ctx()).await.unwrap();
        }
        assert!(matches!(
            add_test_user(&db, PROJECT, app.id, "one-too-many@example.com", &ctx()).await,
            Err(ApiError::BadRequest(_))
        ));
        assert_eq!(list_test_users(&db, app.id).await.unwrap().len(), 100);

        remove_test_user(&db, PROJECT, app.id, "Ada@Example.com", &ctx()).await.unwrap();
        assert!(!user_may_authorize(&db, &app, "user_ada", None).await.unwrap());
    }
}
