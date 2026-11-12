//! Console playground (Wave 1, lane L1B): the 15-minute playground key,
//! saved prompts and their signed-in share links. Clerk session auth behind
//! `console_gate`, merged once in `platform_v1::router_with_agency`.
//!
//! - `POST /api/v1/platform/projects/:id/playground-key` →
//!   `{object:"playground_key", key, expires_at}`. A user-style `alt_<hex>`
//!   key (scope `inference`, kind `playground`) for `/v1/chat/completions`,
//!   which refuses project keys. It bills to the signed-in user's credits and
//!   carries the project for attribution; minting revokes the caller's
//!   previous playground key for the project; it is never listed.
//! - `GET,POST /api/v1/platform/projects/:id/prompts` (list is `{data:[…]}`)
//! - `GET,PATCH,DELETE /api/v1/platform/projects/:id/prompts/:prompt_id`
//!   (DELETE soft-deletes and turns sharing off)
//! - `POST,DELETE /api/v1/platform/projects/:id/prompts/:prompt_id/share`
//! - `GET /api/v1/platform/shared-prompts/:token` → `{name, body}` for any
//!   signed-in console user; no ids. 404 once unshared or deleted.
//!
//! Prompt objects are `{id, object, name, body, share_url?, updated_at}`.
//! Project access is `projects::get_project`'s rule (owner, or an admin of
//! the project's Clerk org); anyone else gets 404.

use std::sync::Arc;

use axum::{
    async_trait,
    extract::{FromRequestParts, Path, State},
    http::{request::Parts, StatusCode},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{
    new_id,
    projects::{self, Principal},
    ApiJson, PlatformError,
};
use crate::{services::api_keys, ApiState};

/// Largest prompt body accepted, as serialized JSON.
pub const MAX_PROMPT_BODY_BYTES: usize = 256 * 1024;
/// Where a share link opens (the console's signed-in shared-prompt page).
pub const SHARE_URL_BASE: &str = "https://platform.allternit.com/playground/shared/";

pub(super) fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/platform/projects/:id/playground-key", post(mint_playground_key))
        .route("/api/v1/platform/projects/:id/prompts", get(list_prompts).post(create_prompt))
        .route(
            "/api/v1/platform/projects/:id/prompts/:prompt_id",
            get(get_prompt).patch(update_prompt).delete(delete_prompt),
        )
        .route(
            "/api/v1/platform/projects/:id/prompts/:prompt_id/share",
            post(share_prompt).delete(unshare_prompt),
        )
        .route("/api/v1/platform/shared-prompts/:token", get(read_shared_prompt))
}

/// The signed-in console user (a verified Clerk session; tests put a
/// `TestSession` in its place).
struct Session(Principal);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Session {
    type Rejection = PlatformError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        #[cfg(test)]
        if let Some(test) = parts.extensions.get::<super::TestSession>() {
            return Ok(Session(test.0.clone()));
        }
        Ok(Session(super::console::principal(&parts.headers).await?.0))
    }
}

// ---- playground key ---------------------------------------------------------

async fn mint_playground_key(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    let project = projects::get_project(&state.db, &who, &id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::conflict("project_archived", "This project is archived."));
    }
    let key =
        api_keys::create_playground_key(&state.db, &who.user_id, project.org_id.as_deref(), &project.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "object": "playground_key",
            "key": key.token,
            "expires_at": key.expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        })),
    ))
}

// ---- saved prompts ----------------------------------------------------------

#[derive(Debug, FromRow)]
struct PromptRow {
    id: String,
    name: String,
    body: Value,
    share_token: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct Prompt {
    pub id: String,
    pub object: &'static str,
    pub name: String,
    pub body: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl From<PromptRow> for Prompt {
    fn from(row: PromptRow) -> Self {
        Prompt {
            id: row.id,
            object: "prompt",
            name: row.name,
            body: row.body,
            share_url: row.share_token.map(|t| format!("{SHARE_URL_BASE}{t}")),
            updated_at: row.updated_at,
        }
    }
}

const PROMPT_COLUMNS: &str = "id, name, body, share_token, updated_at";

fn prompt_not_found() -> PlatformError {
    PlatformError::not_found("prompt_not_found", "No such prompt.")
}

fn validate_name(name: &str) -> Result<String, PlatformError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(PlatformError::invalid_request("invalid_name", "name must be 1 to 200 characters.")
            .with_param("name"));
    }
    Ok(name.to_string())
}

/// `null` stores as `{}`; anything over [`MAX_PROMPT_BODY_BYTES`] is a 413.
fn validate_body(body: Value) -> Result<Value, PlatformError> {
    let body = if body.is_null() { json!({}) } else { body };
    let size = serde_json::to_vec(&body).map(|v| v.len()).unwrap_or(usize::MAX);
    if size > MAX_PROMPT_BODY_BYTES {
        return Err(PlatformError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            ..PlatformError::invalid_request(
                "prompt_too_large",
                format!("A saved prompt can be at most {} KB.", MAX_PROMPT_BODY_BYTES / 1024),
            )
            .with_param("body")
        });
    }
    Ok(body)
}

fn new_share_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

#[derive(Debug, Deserialize)]
struct CreatePrompt {
    name: String,
    #[serde(default)]
    body: Value,
}

#[derive(Debug, Deserialize)]
struct PatchPrompt {
    name: Option<String>,
    body: Option<Value>,
}

/// The live (not deleted) prompts of a project the caller can open.
pub(super) async fn list_project_prompts(
    db: &PgPool,
    who: &Principal,
    project_id: &str,
) -> Result<Vec<Prompt>, PlatformError> {
    let project = projects::get_project(db, who, project_id).await?;
    let rows = sqlx::query_as::<_, PromptRow>(&format!(
        "SELECT {PROMPT_COLUMNS} FROM platform_saved_prompts \
         WHERE project_id = $1 AND deleted_at IS NULL ORDER BY updated_at DESC, id"
    ))
    .bind(&project.id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Prompt::from).collect())
}

async fn list_prompts(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    let data = list_project_prompts(&state.db, &who, &id).await?;
    Ok(Json(json!({ "data": data })))
}

async fn create_prompt(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<CreatePrompt>,
) -> Result<(StatusCode, Json<Prompt>), PlatformError> {
    let project = projects::get_project(&state.db, &who, &id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::conflict("project_archived", "This project is archived."));
    }
    let name = validate_name(&input.name)?;
    let body = validate_body(input.body)?;
    let row = sqlx::query_as::<_, PromptRow>(&format!(
        "INSERT INTO platform_saved_prompts (id, project_id, owner_user_id, name, body) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {PROMPT_COLUMNS}"
    ))
    .bind(new_id("prompt_"))
    .bind(&project.id)
    .bind(&who.user_id)
    .bind(&name)
    .bind(&body)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn get_prompt(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path((id, prompt_id)): Path<(String, String)>,
) -> Result<Json<Prompt>, PlatformError> {
    let project = projects::get_project(&state.db, &who, &id).await?;
    let row = sqlx::query_as::<_, PromptRow>(&format!(
        "SELECT {PROMPT_COLUMNS} FROM platform_saved_prompts \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL"
    ))
    .bind(&prompt_id)
    .bind(&project.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(prompt_not_found)?;
    Ok(Json(row.into()))
}

async fn update_prompt(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path((id, prompt_id)): Path<(String, String)>,
    ApiJson(patch): ApiJson<PatchPrompt>,
) -> Result<Json<Prompt>, PlatformError> {
    let project = projects::get_project(&state.db, &who, &id).await?;
    let name = patch.name.as_deref().map(validate_name).transpose()?;
    let body = patch.body.map(validate_body).transpose()?;
    let row = sqlx::query_as::<_, PromptRow>(&format!(
        "UPDATE platform_saved_prompts \
         SET name = COALESCE($3, name), body = COALESCE($4, body), updated_at = NOW() \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL RETURNING {PROMPT_COLUMNS}"
    ))
    .bind(&prompt_id)
    .bind(&project.id)
    .bind(name)
    .bind(body)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(prompt_not_found)?;
    Ok(Json(row.into()))
}

async fn delete_prompt(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path((id, prompt_id)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    let project = projects::get_project(&state.db, &who, &id).await?;
    let result = sqlx::query(
        "UPDATE platform_saved_prompts SET deleted_at = NOW(), share_token = NULL, updated_at = NOW() \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL",
    )
    .bind(&prompt_id)
    .bind(&project.id)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(prompt_not_found());
    }
    Ok(Json(json!({ "id": prompt_id, "object": "prompt", "deleted": true })))
}

/// Turn the share link on (keeps an existing token, so the link is stable).
async fn share_prompt(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path((id, prompt_id)): Path<(String, String)>,
) -> Result<Json<Prompt>, PlatformError> {
    let project = projects::get_project(&state.db, &who, &id).await?;
    let row = sqlx::query_as::<_, PromptRow>(&format!(
        "UPDATE platform_saved_prompts SET share_token = COALESCE(share_token, $3), updated_at = NOW() \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL RETURNING {PROMPT_COLUMNS}"
    ))
    .bind(&prompt_id)
    .bind(&project.id)
    .bind(new_share_token())
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(prompt_not_found)?;
    Ok(Json(row.into()))
}

async fn unshare_prompt(
    State(state): State<Arc<ApiState>>,
    Session(who): Session,
    Path((id, prompt_id)): Path<(String, String)>,
) -> Result<Json<Prompt>, PlatformError> {
    let project = projects::get_project(&state.db, &who, &id).await?;
    let row = sqlx::query_as::<_, PromptRow>(&format!(
        "UPDATE platform_saved_prompts SET share_token = NULL, updated_at = NOW() \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL RETURNING {PROMPT_COLUMNS}"
    ))
    .bind(&prompt_id)
    .bind(&project.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(prompt_not_found)?;
    Ok(Json(row.into()))
}

/// Any signed-in console user with the link reads the name and body only:
/// no prompt, project or owner ids. Archived projects' links stop working.
async fn read_shared_prompt(
    State(state): State<Arc<ApiState>>,
    Session(_who): Session,
    Path(token): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    let row: Option<(String, Value)> = sqlx::query_as(
        "SELECT s.name, s.body FROM platform_saved_prompts s \
         JOIN platform_projects p ON p.id = s.project_id \
         WHERE s.share_token = $1 AND s.deleted_at IS NULL AND p.archived_at IS NULL",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await?;
    let (name, body) = row.ok_or_else(prompt_not_found)?;
    Ok(Json(json!({ "name": name, "body": body })))
}

#[cfg(test)]
#[path = "tests_l1_playground.rs"]
mod tests;
