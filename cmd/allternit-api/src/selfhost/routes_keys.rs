//! `/v1/api_keys` and `/v1/environments/{id}/keys`. Admin only.
//!
//! First boot: while the API key table is empty, `POST /v1/api_keys` also
//! accepts the setup token (`ALLTERNIT_SELF_HOSTED_SETUP_TOKEN`, sent as
//! `X-Allternit-Self-Hosted-Token` or as the Bearer). That mints the first
//! admin key, atomically, once. After that the setup token does nothing.
//! `allternit-agents-server admin setup` (step 15) is the other way in.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use super::auth::{
    authenticate_user, key_prefix, setup_token_matches, Admin, API_KEY_PREFIX, ENV_KEY_PREFIX,
};
use super::routes_environments::not_found_env;
use super::store::{
    hash_secret, new_secret, ApiKeyRecord, EnvironmentKeyRecord, NewApiKey, NewEnvironmentKey,
    Role, StoreError,
};
use super::{
    check_len, deleted, double_option, list_envelope, on_delete, on_get, on_post, page_limit,
    rfc3339, rfc3339_opt, ApiError, ApiJson, ApiQuery, PageQuery, RouteBook, SelfHostState,
};

pub fn mount(book: RouteBook) -> RouteBook {
    book.add("/v1/api_keys", vec![on_get(list_api_keys), on_post(create_api_key)])
        .add("/v1/api_keys/:id", vec![on_delete(revoke_api_key)])
        .add(
            "/v1/environments/:id/keys",
            vec![on_get(list_env_keys), on_post(create_env_key)],
        )
        .add("/v1/environments/:id/keys/:key_id", vec![on_delete(revoke_env_key)])
}

pub fn api_key_json(k: &ApiKeyRecord) -> Value {
    json!({
        "id": k.id,
        "object": "api_key",
        "name": k.name,
        "role": k.role.as_str(),
        "prefix": k.prefix,
        "created_at": rfc3339(k.created_at),
        "expires_at": rfc3339_opt(k.expires_at),
        "last_used_at": rfc3339_opt(k.last_used_at),
        "revoked_at": rfc3339_opt(k.revoked_at),
    })
}

pub fn env_key_json(k: &EnvironmentKeyRecord) -> Value {
    json!({
        "id": k.id,
        "object": "environment_key",
        "environment_id": k.environment_id,
        "name": k.name,
        "prefix": k.prefix,
        "created_at": rfc3339(k.created_at),
        "expires_at": rfc3339_opt(k.expires_at),
        "last_used_at": rfc3339_opt(k.last_used_at),
        "revoked_at": rfc3339_opt(k.revoked_at),
    })
}

fn with_secret(mut v: Value, secret: &str) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.insert("secret".into(), Value::String(secret.to_string()));
    }
    v
}

fn check_expiry(expires_at: Option<DateTime<Utc>>) -> Result<(), ApiError> {
    if let Some(e) = expires_at {
        if e <= Utc::now() {
            return Err(ApiError::invalid("invalid_field", "expires_at must be in the future."));
        }
    }
    Ok(())
}

/// A new key: (secret shown once, its prefix, its hash).
pub fn mint(prefix: &str) -> (String, String, String) {
    let secret = new_secret(prefix);
    let p = key_prefix(&secret);
    let h = hash_secret(&secret);
    (secret, p, h)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateApiKeyBody {
    pub name: String,
    pub role: Role,
    #[serde(default, deserialize_with = "double_option")]
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

async fn list_api_keys(
    State(st): State<SelfHostState>,
    _admin: Admin,
    ApiQuery(q): ApiQuery<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = page_limit(q.limit)?;
    let rows = st.store.list_api_keys(limit + 1, q.after.as_deref()).await?;
    Ok(Json(list_envelope(rows, limit, |k| k.id.clone(), api_key_json)))
}

async fn create_api_key(
    State(st): State<SelfHostState>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateApiKeyBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_len("name", &body.name, 1, 100)?;
    let expires_at = body.expires_at.flatten();
    check_expiry(expires_at)?;

    // First boot through the setup token.
    if st.setup_token.is_some() && setup_token_matches(&st, &headers) {
        if st.store.count_api_keys().await? == 0 {
            if body.role != Role::Admin {
                return Err(ApiError::invalid(
                    "first_key_must_be_admin",
                    "The first API key must have the admin role.",
                ));
            }
            let (secret, prefix, key_hash) = mint(API_KEY_PREFIX);
            let key = st
                .store
                .create_first_api_key(NewApiKey {
                    name: body.name,
                    role: Role::Admin,
                    prefix,
                    key_hash,
                    expires_at,
                })
                .await?;
            tracing::info!(key_id = %key.id, "first admin API key minted with the setup token");
            return Ok((StatusCode::CREATED, Json(with_secret(api_key_json(&key), &secret))));
        }
        return Err(ApiError::unauthorized(
            "The setup token only works while no API key exists. Sign in with an admin key.",
        ));
    }

    let caller = match authenticate_user(&st, &headers).await {
        Ok(c) => c,
        Err(e) => {
            if st.store.count_api_keys().await? == 0 {
                return Err(ApiError::unauthorized(
                    "No API key exists yet. Run `allternit-agents-server admin setup`, or send the setup token (ALLTERNIT_SELF_HOSTED_SETUP_TOKEN) in X-Allternit-Self-Hosted-Token.",
                ));
            }
            return Err(e);
        }
    };
    if caller.role != Role::Admin {
        return Err(ApiError::forbidden("forbidden", "This needs the admin role."));
    }
    let (secret, prefix, key_hash) = mint(API_KEY_PREFIX);
    let key = st
        .store
        .create_api_key(NewApiKey {
            name: body.name,
            role: body.role,
            prefix,
            key_hash,
            expires_at,
        })
        .await?;
    tracing::info!(key_id = %key.id, role = key.role.as_str(), by = %caller.subject, "API key created");
    Ok((StatusCode::CREATED, Json(with_secret(api_key_json(&key), &secret))))
}

async fn revoke_api_key(
    State(st): State<SelfHostState>,
    Admin(caller): Admin,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    st.store
        .revoke_api_key(&id, Utc::now())
        .await
        .map_err(|e| match e {
            StoreError::NotFound => ApiError::not_found("No API key with that id."),
            other => other.into(),
        })?;
    tracing::info!(key_id = %id, by = %caller.subject, "API key revoked");
    Ok(Json(deleted(&id)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEnvKeyBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

async fn list_env_keys(
    State(st): State<SelfHostState>,
    _admin: Admin,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = page_limit(q.limit)?;
    st.store.get_environment(&id).await.map_err(not_found_env)?;
    let rows = st
        .store
        .list_environment_keys(&id, limit + 1, q.after.as_deref())
        .await?;
    Ok(Json(list_envelope(rows, limit, |k| k.id.clone(), env_key_json)))
}

async fn create_env_key(
    State(st): State<SelfHostState>,
    Admin(caller): Admin,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CreateEnvKeyBody>,
) -> Result<impl IntoResponse, ApiError> {
    if let Some(n) = &body.name {
        check_len("name", n, 0, 100)?;
    }
    let expires_at = body.expires_at.flatten();
    check_expiry(expires_at)?;
    let (secret, prefix, key_hash) = mint(ENV_KEY_PREFIX);
    let key = st
        .store
        .create_environment_key(NewEnvironmentKey {
            environment_id: id,
            name: body.name,
            prefix,
            key_hash,
            expires_at,
        })
        .await
        .map_err(not_found_env)?;
    tracing::info!(key_id = %key.id, environment_id = %key.environment_id, by = %caller.subject, "environment key created");
    Ok((StatusCode::CREATED, Json(with_secret(env_key_json(&key), &secret))))
}

async fn revoke_env_key(
    State(st): State<SelfHostState>,
    Admin(caller): Admin,
    Path((id, key_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    st.store
        .revoke_environment_key(&id, &key_id, Utc::now())
        .await
        .map_err(|e| match e {
            StoreError::NotFound => ApiError::not_found("No key with that id in this environment."),
            other => other.into(),
        })?;
    tracing::info!(key_id = %key_id, environment_id = %id, by = %caller.subject, "environment key revoked");
    Ok(Json(deleted(&key_id)))
}
