//! Console management routes (Clerk session) under `/api/v1/platform/`:
//! projects and their API keys. Developers use these from the console; the
//! public `/v1` API cannot mint keys (a key must never mint keys).
//!
//! - `POST/GET /api/v1/platform/projects`
//! - `GET/PATCH /api/v1/platform/projects/:id`
//! - `POST/GET /api/v1/platform/projects/:id/keys`
//! - `DELETE /api/v1/platform/projects/:id/keys/:key_id`
//!
//! Owner-only: the project's owner, or an admin of the project's Clerk org.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    build_page,
    projects::{self, Principal, ProjectPatch},
    ApiJson, ApiQuery, Page, PageParams, PlatformError, ProjectEnv,
};
use crate::{
    auth::clerk,
    services::{
        self,
        api_keys::{CreateProjectKeyInput, CreatedProjectKey, ProjectKey},
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
        .route(
            "/api/v1/platform/projects/:id/keys",
            get(list_keys).post(create_key),
        )
        .route("/api/v1/platform/projects/:id/keys/:key_id", delete(revoke_key))
}

/// Verify the Clerk session and read the org binding and role from its claims.
async fn principal(headers: &HeaderMap) -> Result<(Principal, Option<String>), PlatformError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            PlatformError::authentication("missing_session", "A Clerk session is required.")
        })?;
    let claims = clerk::verified_claims(token).await.map_err(|_| {
        PlatformError::authentication("invalid_session", "The Clerk session is invalid or expired.")
    })?;
    Ok(principal_from_claims(&claims))
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
}

async fn audit(state: &ApiState, who: &Principal, email: Option<String>, action: &str, resource: &str, id: &str, details: Value) {
    services::audit::write_audit_log(
        &state.db,
        services::audit::AuditEvent {
            action: action.to_string(),
            resource_type: resource.to_string(),
            resource_id: Some(id.to_string()),
            user_id: Some(who.user_id.clone()),
            user_email: email,
            details: Some(details),
            success: true,
        },
    )
    .await;
}

async fn create_project(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateProject>,
) -> Result<(StatusCode, Json<projects::Project>), PlatformError> {
    let (who, email) = principal(&headers).await?;
    let env = match body.env.as_deref() {
        None => ProjectEnv::Sandbox,
        Some(raw) => ProjectEnv::parse(raw).ok_or_else(|| {
            PlatformError::invalid_request("invalid_env", "env must be 'sandbox' or 'live'.")
                .with_param("env")
        })?,
    };
    let project = projects::create_project(&state.db, &who, &body.name, env).await?;
    audit(&state, &who, email, "platform_project.create", "platform_project", &project.id,
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

async fn update_project(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<PatchProject>,
) -> Result<Json<projects::Project>, PlatformError> {
    let (who, email) = principal(&headers).await?;
    let project = projects::update_project(
        &state.db,
        &who,
        &id,
        ProjectPatch { name: body.name, spend_cap_cents: body.spend_cap_cents, archived: body.archived },
    )
    .await?;
    audit(&state, &who, email, "platform_project.update", "platform_project", &project.id,
        json!({ "name": project.name, "spend_cap_cents": project.spend_cap_cents, "archived": project.archived_at.is_some() })).await;
    Ok(Json(project))
}

async fn create_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CreateKey>,
) -> Result<(StatusCode, Json<CreatedProjectKey>), PlatformError> {
    let (who, email) = principal(&headers).await?;
    let project = projects::get_project(&state.db, &who, &id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::conflict("project_archived", "This project is archived."));
    }
    if let Some(account_id) = body.account_id.as_deref() {
        projects::account_in_project(&state.db, &project.id, account_id).await?;
    }
    let env = ProjectEnv::parse(&project.env).unwrap_or(ProjectEnv::Sandbox);
    let created = services::api_keys::create_project_key(
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
    )
    .await?;
    audit(&state, &who, email, "platform_key.create", "api_key", &created.key.id,
        json!({ "project_id": project.id, "prefix": created.key.prefix, "scopes": created.key.scopes, "account_id": created.key.account_id })).await;
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
    Path((id, key_id)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    let (who, email) = principal(&headers).await?;
    let project = projects::get_project(&state.db, &who, &id).await?;
    services::api_keys::revoke_project_key(&state.db, &project.id, &key_id).await?;
    audit(&state, &who, email, "platform_key.revoke", "api_key", &key_id,
        json!({ "project_id": project.id })).await;
    Ok(Json(json!({ "id": key_id, "object": "api_key", "revoked": true })))
}
