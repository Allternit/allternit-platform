//! `/v1/accounts`: one account per end-customer business a developer serves.
//!
//! Scope: any resource scope (`agents`, `voice`, `messaging`, `numbers`,
//! `channels`, `twin`, `webhooks`); a key with only `usage`, `inference` or
//! `compute` is refused. A key bound to one account can read and rename its own
//! account but cannot create or delete accounts, and sees no other account.
//! Deleting an account is a soft delete: it also revokes the keys bound to it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use std::sync::Arc;

use super::{
    build_page, new_id, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError,
    RouteTable,
};
use crate::ApiState;

const MAX_NAME: usize = 200;
const MAX_EXTERNAL_REF: usize = 255;
const MAX_METADATA_BYTES: usize = 8 * 1024;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/accounts", &["GET", "POST"], get(list_accounts).post(create_account))
        .add(
            "/v1/accounts/:id",
            &["GET", "PATCH", "DELETE"],
            get(get_account).patch(update_account).delete(delete_account),
        )
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Account {
    pub id: String,
    pub object: String,
    pub name: String,
    pub external_ref: Option<String>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, 'account'::text AS object, name, external_ref, metadata, created_at";

#[derive(Debug, Deserialize)]
struct CreateAccount {
    name: String,
    external_ref: Option<String>,
    metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct UpdateAccount {
    name: Option<String>,
    /// `null` clears it; absent leaves it unchanged.
    #[serde(default, deserialize_with = "double_option")]
    external_ref: Option<Option<String>>,
    metadata: Option<Value>,
}

fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    external_ref: Option<String>,
}

fn validate_name(name: &str) -> Result<String, PlatformError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(PlatformError::invalid_request(
            "invalid_name",
            format!("name must be 1 to {MAX_NAME} characters."),
        )
        .with_param("name"));
    }
    Ok(name.to_string())
}

fn validate_external_ref(value: &str) -> Result<String, PlatformError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_EXTERNAL_REF {
        return Err(PlatformError::invalid_request(
            "invalid_external_ref",
            format!("external_ref must be 1 to {MAX_EXTERNAL_REF} characters."),
        )
        .with_param("external_ref"));
    }
    Ok(value.to_string())
}

fn validate_metadata(value: &Value) -> Result<(), PlatformError> {
    let ok = value.is_object() && value.to_string().len() <= MAX_METADATA_BYTES;
    if ok {
        Ok(())
    } else {
        Err(PlatformError::invalid_request(
            "invalid_metadata",
            "metadata must be a JSON object of at most 8 KB.",
        )
        .with_param("metadata"))
    }
}

fn is_external_ref_conflict(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|e| e.constraint())
        .map(|c| c == "uq_platform_accounts_external_ref")
        .unwrap_or(false)
}

fn external_ref_taken() -> PlatformError {
    PlatformError::conflict(
        "external_ref_taken",
        "An account with this external_ref already exists in the project.",
    )
    .with_param("external_ref")
}

async fn create_account(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiJson(body): ApiJson<CreateAccount>,
) -> Result<(StatusCode, Json<Account>), PlatformError> {
    caller.require_resource_scope()?;
    caller.require_unbound()?;
    let name = validate_name(&body.name)?;
    let external_ref = body.external_ref.as_deref().map(validate_external_ref).transpose()?;
    let metadata = body.metadata.unwrap_or_else(|| json!({}));
    validate_metadata(&metadata)?;

    let account = sqlx::query_as::<_, Account>(&format!(
        "INSERT INTO platform_accounts (id, project_id, name, external_ref, metadata) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {COLUMNS}"
    ))
    .bind(new_id("acct_"))
    .bind(&caller.project_id)
    .bind(&name)
    .bind(&external_ref)
    .bind(&metadata)
    .fetch_one(&state.db)
    .await
    .map_err(|e| if is_external_ref_conflict(&e) { external_ref_taken() } else { e.into() })?;
    Ok((StatusCode::CREATED, Json(account)))
}

async fn list_accounts(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Page<Account>>, PlatformError> {
    caller.require_resource_scope()?;
    let limit = query.page.limit()?;
    let cursor = query.page.cursor()?;
    let (after_at, after_id) = match cursor {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let bound = caller.account_filter(None)?;

    let rows = sqlx::query_as::<_, Account>(&format!(
        "SELECT {COLUMNS} FROM platform_accounts \
         WHERE project_id = $1 AND deleted_at IS NULL \
           AND ($2::text IS NULL OR id = $2) \
           AND ($3::text IS NULL OR external_ref = $3) \
           AND ($4::timestamptz IS NULL OR (created_at, id) > ($4, $5)) \
         ORDER BY created_at, id LIMIT $6"
    ))
    .bind(&caller.project_id)
    .bind(&bound)
    .bind(&query.external_ref)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(build_page(rows, limit, |a| (a.created_at, a.id.clone()))))
}

async fn fetch_visible(
    state: &ApiState,
    caller: &PlatformCaller,
    id: &str,
) -> Result<Account, PlatformError> {
    let not_found = || PlatformError::not_found("account_not_found", "No such account.");
    // Another account's id looks exactly like a missing one.
    if caller.account_id.as_deref().is_some_and(|bound| bound != id) {
        return Err(not_found());
    }
    sqlx::query_as::<_, Account>(&format!(
        "SELECT {COLUMNS} FROM platform_accounts \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(not_found)
}

async fn get_account(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Account>, PlatformError> {
    caller.require_resource_scope()?;
    Ok(Json(fetch_visible(&state, &caller, &id).await?))
}

async fn update_account(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateAccount>,
) -> Result<Json<Account>, PlatformError> {
    caller.require_resource_scope()?;
    let current = fetch_visible(&state, &caller, &id).await?;

    let name = match body.name.as_deref() {
        Some(n) => validate_name(n)?,
        None => current.name,
    };
    let external_ref = match body.external_ref {
        None => current.external_ref,
        Some(None) => None,
        Some(Some(v)) => Some(validate_external_ref(&v)?),
    };
    let metadata = match body.metadata {
        Some(m) => {
            validate_metadata(&m)?;
            m
        }
        None => current.metadata,
    };

    let account = sqlx::query_as::<_, Account>(&format!(
        "UPDATE platform_accounts SET name = $3, external_ref = $4, metadata = $5 \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL RETURNING {COLUMNS}"
    ))
    .bind(&id)
    .bind(&caller.project_id)
    .bind(&name)
    .bind(&external_ref)
    .bind(&metadata)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| if is_external_ref_conflict(&e) { external_ref_taken() } else { e.into() })?
    .ok_or_else(|| PlatformError::not_found("account_not_found", "No such account."))?;
    Ok(Json(account))
}

async fn delete_account(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require_resource_scope()?;
    caller.require_unbound()?;
    let mut tx = state.db.begin().await?;
    let deleted = sqlx::query(
        "UPDATE platform_accounts SET deleted_at = NOW() \
         WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL",
    )
    .bind(&id)
    .bind(&caller.project_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(PlatformError::not_found("account_not_found", "No such account."));
    }
    sqlx::query(
        "UPDATE api_keys SET revoked_at = NOW(), updated_at = NOW() \
         WHERE account_id = $1 AND revoked_at IS NULL",
    )
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "object": "account", "deleted": true })))
}
