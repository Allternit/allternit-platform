//! `/v1/model_keys`: a project's own model provider keys (spec §5, bring your own key).
//!
//! An agent whose `model` is `anthropic/…`, `openai/…` or `xai/…` runs on the
//! project's key for that provider; `allternit` uses Allternit's routed model and
//! needs none. Keys are encrypted at rest with the credential cipher and only a
//! masked form is ever returned. Before the next turn the project's hosted runtime
//! gets the key (see `conversations::prepare`). Scope `agents`; not for
//! account-bound keys (a model key is project-wide).

use std::sync::Arc;

use allternit_cloud_core::CredentialCipher;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::{ApiJson, PlatformCaller, PlatformError, RouteTable};
use crate::ApiState;

/// Providers a project can bring its own key for.
pub const PROVIDERS: [&str; 3] = ["anthropic", "openai", "xai"];

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/model_keys", &["GET"], get(list_keys))
        .add("/v1/model_keys/:provider", &["PUT", "DELETE"], put(put_key).delete(delete_key))
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ModelKey {
    pub provider: String,
    #[sqlx(skip)]
    pub object: &'static str,
    pub masked: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn obj(mut k: ModelKey) -> ModelKey {
    k.object = "model_key";
    k
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PutBody {
    api_key: String,
}

pub fn mask(key: &str) -> String {
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

fn provider_ok(provider: &str) -> Result<(), PlatformError> {
    if PROVIDERS.contains(&provider) {
        Ok(())
    } else {
        Err(PlatformError::invalid_request("invalid_provider", format!("provider must be one of {}.", PROVIDERS.join(", "))).with_param("provider"))
    }
}

fn cipher(state: &ApiState) -> Result<&CredentialCipher, PlatformError> {
    state.credential_cipher.as_deref().ok_or_else(|| PlatformError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        kind: "api_error",
        code: "model_keys_unavailable".into(),
        message: "Model keys aren't available on this deployment.".into(),
        param: None,
    })
}

/// Store (or replace) a project's key. Agents of the project re-sync before their next turn.
pub async fn store_key(db: &PgPool, cipher: &CredentialCipher, project_id: &str, provider: &str, api_key: &str) -> Result<ModelKey, PlatformError> {
    provider_ok(provider)?;
    let key = api_key.trim();
    if key.len() < 8 || key.len() > 512 || key.chars().any(char::is_whitespace) {
        return Err(PlatformError::invalid_request("invalid_api_key", "api_key must be 8 to 512 characters with no spaces.").with_param("api_key"));
    }
    let enc = cipher.encrypt(key).map_err(|_| PlatformError::api_error("internal_error", "The key couldn't be stored."))?;
    let row = sqlx::query_as::<_, ModelKey>(
        "INSERT INTO platform_model_keys (project_id, provider, key_encrypted, masked) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (project_id, provider) DO UPDATE SET key_encrypted = excluded.key_encrypted, masked = excluded.masked, updated_at = NOW() \
         RETURNING provider, masked, created_at, updated_at",
    )
    .bind(project_id)
    .bind(provider)
    .bind(&enc)
    .bind(mask(key))
    .fetch_one(db)
    .await?;
    sqlx::query("UPDATE platform_agents SET synced_at = NULL WHERE project_id = $1").bind(project_id).execute(db).await?;
    Ok(obj(row))
}

/// The plaintext key for a provider, if the project has one.
pub async fn key_for(db: &PgPool, cipher: &CredentialCipher, project_id: &str, provider: &str) -> Result<Option<String>, PlatformError> {
    let enc: Option<(String,)> = sqlx::query_as("SELECT key_encrypted FROM platform_model_keys WHERE project_id = $1 AND provider = $2")
        .bind(project_id)
        .bind(provider)
        .fetch_optional(db)
        .await?;
    enc.map(|(e,)| cipher.decrypt(&e).map_err(|_| PlatformError::api_error("internal_error", "The stored key couldn't be read.")))
        .transpose()
}

async fn put_key(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(provider): Path<String>,
    ApiJson(body): ApiJson<PutBody>,
) -> Result<Json<ModelKey>, PlatformError> {
    caller.require("agents")?;
    caller.require_unbound()?;
    provider_ok(&provider)?;
    let c = cipher(&state)?;
    Ok(Json(store_key(&state.db, c, &caller.project_id, &provider, &body.api_key).await?))
}

async fn list_keys(State(state): State<Arc<ApiState>>, caller: PlatformCaller) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    let rows = sqlx::query_as::<_, ModelKey>("SELECT provider, masked, created_at, updated_at FROM platform_model_keys WHERE project_id = $1 ORDER BY provider")
        .bind(&caller.project_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({ "object": "list", "data": rows.into_iter().map(obj).collect::<Vec<_>>(), "has_more": false })))
}

async fn delete_key(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    layered: Option<axum::Extension<Arc<dyn super::hosting::AgentHost>>>,
    Path(provider): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    caller.require("agents")?;
    caller.require_unbound()?;
    provider_ok(&provider)?;
    let gone = sqlx::query("DELETE FROM platform_model_keys WHERE project_id = $1 AND provider = $2")
        .bind(&caller.project_id)
        .bind(&provider)
        .execute(&state.db)
        .await?;
    if gone.rows_affected() == 0 {
        return Err(PlatformError::not_found("model_key_not_found", "This project has no key for that provider."));
    }
    // Best effort: the runtime forgets it too (never provisions one to do so).
    let runtime: Option<(Option<String>,)> = sqlx::query_as("SELECT runtime_id FROM platform_agents WHERE project_id = $1 AND runtime_id IS NOT NULL LIMIT 1")
        .bind(&caller.project_id)
        .fetch_optional(&state.db)
        .await?;
    if let Some((Some(runtime_id),)) = runtime {
        let host = super::hosting::host_for(&state, layered);
        let rt = super::hosting::HostRuntime { owner: super::hosting::runtime_owner(&caller.project_id), runtime_id };
        if let Err(e) = host.call(&rt, "DELETE", &format!("/api/v1/platform/model-keys/{provider}"), &json!({})).await {
            tracing::warn!(%provider, code = %e.code, "platform: runtime didn't forget the deleted model key");
        }
    }
    Ok(Json(json!({ "provider": provider, "object": "model_key", "deleted": true })))
}
