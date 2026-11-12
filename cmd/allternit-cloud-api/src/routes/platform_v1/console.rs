//! Console management routes (Clerk session) under `/api/v1/platform/`:
//! projects and their API keys. Developers use these from the console; the
//! public `/v1` API cannot mint keys (a key must never mint keys).
//!
//! - `POST/GET /api/v1/platform/projects`
//! - `GET/PATCH /api/v1/platform/projects/:id`
//! - `GET /api/v1/platform/projects/:id/limits` (read-only rate and call limits)
//! - `POST/GET /api/v1/platform/projects/:id/keys` (POST takes `expires_at`
//!   and `allowed_cidrs`; key JSON carries `kind`, `expires_at`, `allowed_cidrs`)
//! - `PATCH/DELETE /api/v1/platform/projects/:id/keys/:key_id` (PATCH is
//!   `console_governance::patch_key`)
//!
//! Creating a key records the user's acceptance of the Developer Terms and AUP
//! (`accept_terms`, required for live keys) on the key row. Billing routes
//! (payment method, plan) are in `project_billing`.
//!
//! Owner-only: the project's owner, or an admin of the project's Clerk org.
//! Key writes also need a role that may write keys and must pass the org's
//! key policy (`console_governance`: roles, `check_key_change`). Every write
//! here is audited with its org and project (`console_governance::write_audit`).

use std::sync::Arc;

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, StatusCode},
    routing::{delete, get},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    build_page,
    console_governance::{self as governance, Audit, ConsoleSession, KeyChange},
    projects::{self, Principal, ProjectPatch},
    ApiJson, ApiQuery, Page, PageParams, PlatformError, ProjectEnv,
};
use crate::{
    auth::clerk,
    services::{
        self,
        api_keys::{CreateProjectKeyInput, CreatedProjectKey, KeyGovernance, ProjectKey},
    },
    ApiState,
};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v1/platform/projects",
            get(list_projects).post(create_project),
        )
        .route(
            "/api/v1/platform/projects/:id",
            get(get_project).patch(update_project),
        )
        .route("/api/v1/platform/projects/:id/limits", get(get_limits))
        .route(
            "/api/v1/platform/projects/:id/keys",
            get(list_keys).post(create_key),
        )
        .route(
            "/api/v1/platform/projects/:id/keys/:key_id",
            delete(revoke_key).patch(governance::patch_key),
        )
}

/// Verify the Clerk session and read the org binding and role from its claims.
pub(super) async fn principal(headers: &HeaderMap) -> Result<(Principal, Option<String>), PlatformError> {
    Ok(principal_from_claims(&session_claims(headers).await?))
}

/// [`principal`] plus the org role (`console_governance::ConsoleRole`).
pub(super) async fn session(headers: &HeaderMap) -> Result<ConsoleSession, PlatformError> {
    Ok(ConsoleSession::from_claims(&session_claims(headers).await?))
}

/// Test builds only: claims sent as JSON in this header stand in for a
/// verified Clerk session token, so route tests can act as an org admin,
/// member, billing or viewer. Not compiled into the server.
#[cfg(test)]
pub(super) const TEST_CLAIMS_HEADER: &str = "x-test-clerk-claims";

#[cfg(test)]
fn test_claims(headers: &HeaderMap) -> Option<Result<Value, PlatformError>> {
    let raw = headers.get(TEST_CLAIMS_HEADER)?.to_str().ok()?;
    Some(
        serde_json::from_str(raw)
            .map_err(|_| PlatformError::authentication("invalid_session", "Unreadable test claims.")),
    )
}

#[cfg(not(test))]
fn test_claims(_headers: &HeaderMap) -> Option<Result<Value, PlatformError>> {
    None
}

async fn session_claims(headers: &HeaderMap) -> Result<Value, PlatformError> {
    if let Some(claims) = test_claims(headers) {
        return claims;
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            PlatformError::authentication("missing_session", "A Clerk session is required.")
        })?;
    clerk::verified_claims(token).await.map_err(|_| {
        PlatformError::authentication("invalid_session", "The Clerk session is invalid or expired.")
    })
}

/// Pure claim parsing, split out for tests. Returns the principal and the
/// user's email (for the audit log).
pub fn principal_from_claims(claims: &Value) -> (Principal, Option<String>) {
    let str_at = |v: Option<&Value>| v.and_then(|v| v.as_str()).map(str::to_owned);
    let org_id = str_at(claims.get("o").and_then(|o| o.get("id")))
        .or_else(|| str_at(claims.get("org_id")));
    let role = str_at(claims.get("o").and_then(|o| o.get("rol")))
        .or_else(|| str_at(claims.get("org_role")))
        .unwrap_or_default();
    let org_admin = org_id.is_some() && matches!(role.as_str(), "admin" | "org:admin");
    (
        Principal {
            user_id: str_at(claims.get("sub")).unwrap_or_default(),
            org_id,
            org_admin,
        },
        str_at(claims.get("email")).or_else(|| str_at(claims.get("email_address"))),
    )
}

#[derive(Debug, Deserialize)]
struct CreateProject {
    name: String,
    env: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PatchProject {
    name: Option<String>,
    spend_cap_cents: Option<i64>,
    archived: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CreateKey {
    name: String,
    #[serde(default)]
    scopes: Vec<String>,
    account_id: Option<String>,
    /// The user ticked "I accept the Developer Terms and Acceptable Use Policy".
    /// Required for live keys; recorded on the key either way.
    #[serde(default)]
    accept_terms: bool,
    /// RFC 3339; absent or null = never expires (unless the org policy requires one).
    expires_at: Option<DateTime<Utc>>,
    /// CIDRs or addresses the key may be used from; needs
    /// `PLATFORM_KEY_IP_ALLOWLIST=1` when non-empty.
    #[serde(default)]
    allowed_cidrs: Vec<String>,
}

/// An audit row about `project` (org and project columns from the project).
#[allow(clippy::too_many_arguments)]
async fn audit_project(
    state: &ApiState,
    session: &ConsoleSession,
    ip: Option<String>,
    project: &projects::Project,
    action: &str,
    resource: &str,
    id: &str,
    details: Value,
) {
    governance::write_audit(
        &state.db,
        session,
        Audit {
            action,
            resource_type: resource,
            resource_id: id,
            organization_id: project.org_id.clone(),
            project_id: Some(project.id.clone()),
            ip,
            details,
        },
    )
    .await;
}

async fn create_project(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    ApiJson(body): ApiJson<CreateProject>,
) -> Result<(StatusCode, Json<projects::Project>), PlatformError> {
    let session = session(&headers).await?;
    let who = &session.who;
    let env = match body.env.as_deref() {
        None => ProjectEnv::Sandbox,
        Some(raw) => ProjectEnv::parse(raw).ok_or_else(|| {
            PlatformError::invalid_request("invalid_env", "env must be 'sandbox' or 'live'.")
                .with_param("env")
        })?,
    };
    let project = projects::create_project(&state.db, who, &body.name, env).await?;
    audit_project(&state, &session, governance::request_ip(&headers, connect), &project,
        "platform_project.create", "platform_project", &project.id,
        json!({ "name": project.name, "env": project.env })).await;
    Ok((StatusCode::CREATED, Json(project)))
}

async fn list_projects(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Page<projects::Project>>, PlatformError> {
    let (who, _) = principal(&headers).await?;
    let limit = page.limit()?;
    let rows = projects::list_projects(&state.db, &who, page.cursor()?, limit).await?;
    Ok(Json(build_page(rows, limit, |p| (p.created_at, p.id.clone()))))
}

async fn get_project(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<projects::Project>, PlatformError> {
    let (who, _) = principal(&headers).await?;
    Ok(Json(projects::get_project(&state.db, &who, &id).await?))
}

async fn get_limits(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    let (who, _) = principal(&headers).await?;
    Ok(Json(project_limits(&state.db, &who, &id).await?))
}

/// The project's effective rate and call limits plus current-window usage.
/// Same access as `get_project`: it calls `projects::get_project` directly, so
/// an archived project still answers 200 for its owner and a stranger gets 404.
/// The limits use the same rules as `PlatformCaller::rpm_limit` and the plan's
/// `call_cap` (the values the `/v1` limiter enforces); `used` is a read-only
/// look at the `project:{id}` counter and never writes.
pub(super) async fn project_limits(
    db: &sqlx::PgPool,
    who: &Principal,
    id: &str,
) -> Result<Value, PlatformError> {
    let project = projects::get_project(db, who, id).await?;
    let plan = super::caller::Plan::parse(&project.plan);
    let source = |overridden: bool| if overridden { "override" } else { "plan" };
    let (rpm, rpm_over) = super::caller::effective_rpm(plan, project.rpm_override);
    let (call_limit, call_over) = super::caller::effective_call_cap(plan, project.call_cap_override);
    let (rpm_source, call_source) = (source(rpm_over), source(call_over));

    let now = chrono::Utc::now().timestamp();
    let window_secs = super::limits::WINDOW_SECS;
    let window_start = now - now.rem_euclid(window_secs);
    let resets_at = chrono::DateTime::from_timestamp(window_start + window_secs, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let used: Option<i64> = match sqlx::query_scalar::<_, i32>(
        "SELECT count FROM platform_rate_windows WHERE bucket = $1 AND window_start = to_timestamp($2::float8)",
    )
    .bind(format!("project:{}", project.id))
    .bind(window_start as f64)
    .fetch_optional(db)
    .await
    {
        Ok(row) => Some(row.unwrap_or(0) as i64),
        Err(error) => {
            tracing::error!(%error, "platform limits: reading the rate window failed");
            None
        }
    };
    let remaining = used.map(|u| (rpm as i64 - u).max(0));

    Ok(json!({
        "object": "project.limits",
        "project_id": project.id,
        "plan": plan,
        "rate_limit": {
            "requests_per_minute": rpm,
            "source": rpm_source,
            "window_seconds": window_secs,
            "used": used,
            "remaining": remaining,
            "resets_at": resets_at,
        },
        "concurrent_calls": { "limit": call_limit, "source": call_source },
    }))
}

async fn update_project(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<PatchProject>,
) -> Result<Json<projects::Project>, PlatformError> {
    let session = session(&headers).await?;
    let was_archived = projects::get_project(&state.db, &session.who, &id).await?.archived_at.is_some();
    let project = projects::update_project(
        &state.db,
        &session.who,
        &id,
        ProjectPatch { name: body.name, spend_cap_cents: body.spend_cap_cents, archived: body.archived },
    )
    .await?;
    // Archiving (which also revokes every key) gets its own action so the log
    // reads as what happened; everything else is an update.
    let action = match (was_archived, project.archived_at.is_some()) {
        (false, true) => "platform_project.archive",
        (true, false) => "platform_project.unarchive",
        _ => "platform_project.update",
    };
    audit_project(&state, &session, governance::request_ip(&headers, connect), &project,
        action, "platform_project", &project.id,
        json!({ "name": project.name, "spend_cap_cents": project.spend_cap_cents, "archived": project.archived_at.is_some() })).await;
    Ok(Json(project))
}

async fn create_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CreateKey>,
) -> Result<(StatusCode, Json<CreatedProjectKey>), PlatformError> {
    let session = session(&headers).await?;
    let who = &session.who;
    let project = projects::get_project(&state.db, who, &id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::conflict("project_archived", "This project is archived."));
    }
    governance::check_key_change(
        &state.db,
        &session,
        &project,
        KeyChange {
            expires_at: body.expires_at,
            expiry_changed: true,
            new_cidrs: Some(body.allowed_cidrs.as_slice()),
            issued_at: Utc::now(),
        },
    )
    .await?;
    if let Some(account_id) = body.account_id.as_deref() {
        projects::account_in_project(&state.db, &project.id, account_id).await?;
    }
    let env = ProjectEnv::parse(&project.env).unwrap_or(ProjectEnv::Sandbox);
    if env == ProjectEnv::Live && !body.accept_terms {
        return Err(PlatformError::invalid_request(
            "terms_not_accepted",
            format!(
                "Accept the Developer Terms ({}) and Acceptable Use Policy ({}) to create a live key: send \"accept_terms\": true.",
                super::project_billing::TERMS_URL,
                super::project_billing::AUP_URL
            ),
        )
        .with_param("accept_terms"));
    }
    let created = services::api_keys::create_project_key_with(
        &state.db,
        CreateProjectKeyInput {
            user_id: who.user_id.clone(),
            organization_id: project.org_id.clone(),
            project_id: project.id.clone(),
            account_id: body.account_id,
            env,
            name: body.name,
            scopes: body.scopes,
        },
        KeyGovernance {
            kind: services::api_keys::KEY_KIND_STANDARD.to_string(),
            expires_at: body.expires_at,
            allowed_cidrs: body.allowed_cidrs,
        },
    )
    .await?;
    if body.accept_terms {
        sqlx::query("UPDATE api_keys SET terms_accepted_at = NOW(), terms_version = $2 WHERE id = $1")
            .bind(&created.key.id)
            .bind(super::project_billing::TERMS_VERSION)
            .execute(&state.db)
            .await?;
    }
    audit_project(&state, &session, governance::request_ip(&headers, connect), &project,
        "platform_key.create", "api_key", &created.key.id,
        json!({ "project_id": project.id, "prefix": created.key.prefix, "scopes": created.key.scopes, "account_id": created.key.account_id,
                "expires_at": created.key.expires_at, "allowed_cidrs": created.key.allowed_cidrs,
                "terms_accepted": body.accept_terms, "terms_version": body.accept_terms.then_some(super::project_billing::TERMS_VERSION) })).await;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn list_keys(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    let (who, _) = principal(&headers).await?;
    let project = projects::get_project(&state.db, &who, &id).await?;
    let keys: Vec<ProjectKey> = services::api_keys::list_project_keys(&state.db, &project.id).await?;
    Ok(Json(json!({ "data": keys, "has_more": false, "next_cursor": null })))
}

async fn revoke_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    connect: Option<ConnectInfo<SocketAddr>>,
    Path((id, key_id)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    let session = session(&headers).await?;
    let project = projects::get_project(&state.db, &session.who, &id).await?;
    if !session.role.can_write_keys() {
        return Err(PlatformError::permission(
            "insufficient_role",
            "Your organization role can view this project but not create, change or revoke its API keys. Ask an org admin.",
        ));
    }
    services::api_keys::revoke_project_key(&state.db, &project.id, &key_id).await?;
    audit_project(&state, &session, governance::request_ip(&headers, connect), &project,
        "platform_key.revoke", "api_key", &key_id,
        json!({ "project_id": project.id })).await;
    Ok(Json(json!({ "id": key_id, "object": "api_key", "revoked": true })))
}
