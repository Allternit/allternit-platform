//! File uploads from a runtime (a bot's own files), charged to the device owner.
//!
//! Same caps and storage as [`super::files`]; only the auth differs: the runtime
//! device credential (`Authorization: Bearer allternit_runtime_…`), as for
//! `runtime-devices/me/phone-numbers`.
//!
//! * `POST /api/v1/runtime-devices/me/files/uploads` `{name, contentType, bytes}`
//!   → `{fileId, key, putUrl, expiresInSeconds}` (same refusals as `/files/uploads`).
//! * `POST /api/v1/runtime-devices/me/files/:id/complete` `{name, contentType, bytes}`
//!   → 201 `{fileId, name, contentType, bytes, createdAt, linkUrl}`. `linkUrl` is the
//!   permanent capability link (see [`super::files`]); `null` when the server has no
//!   `ALLTERNIT_FILE_LINK_SECRET`.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

use super::files::{reply, store, FileBody};
use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::{
    error::ApiError,
    services::{user_files, voice_usage},
    ApiState,
};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/runtime-devices/me/files/uploads", post(begin))
        .route("/api/v1/runtime-devices/me/files/:id/complete", post(complete))
}

async fn owner(db: &sqlx::PgPool, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = device_token_from_headers(headers).ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    Ok(runtime_device_for_token(db, token, None).await?.user_id)
}

async fn begin(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(b): Json<FileBody>) -> Result<Response, ApiError> {
    let user = owner(&state.db, &headers).await?;
    let plan = voice_usage::plan_for_user(&state.db, &user).await?;
    let r2 = match store() {
        Ok(r) => r,
        Err(f) => return reply(f),
    };
    match user_files::begin_upload(&state.db, &r2, &user, &plan, &b.name, &b.content_type, b.bytes).await {
        Ok(u) => Ok(Json(json!({ "fileId": u.file_id, "key": u.key, "putUrl": u.put_url, "expiresInSeconds": user_files::PUT_TTL.as_secs() })).into_response()),
        Err(f) => reply(f),
    }
}

async fn complete(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<Uuid>, Json(b): Json<FileBody>) -> Result<Response, ApiError> {
    let user = owner(&state.db, &headers).await?;
    let r2 = match store() {
        Ok(r) => r,
        Err(f) => return reply(f),
    };
    match user_files::complete_upload(&state.db, &r2, &user, id, &b.name, &b.content_type, b.bytes, Utc::now()).await {
        Ok(v) => Ok((StatusCode::CREATED, Json(v)).into_response()),
        Err(f) => reply(f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway};
    use serial_test::serial;

    fn bearer(id: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, format!("Bearer allternit_runtime_{id}").parse().unwrap());
        h
    }

    #[tokio::test]
    #[serial]
    async fn only_a_device_credential_reaches_the_owner_and_storage_must_be_configured() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::query("ALTER TABLE runtime_devices ADD COLUMN IF NOT EXISTS previous_credential_hash TEXT, ADD COLUMN IF NOT EXISTS previous_credential_expires_at TIMESTAMPTZ").execute(&state.db).await.unwrap();
        seed_runtime_device(&state.db, "rtf", "user_f").await;
        let hash = super::super::runtime_pairing::sha256_hex(b"allternit_runtime_rtf");
        sqlx::query("UPDATE runtime_devices SET credential_hash = $1 WHERE id = 'rtf'").bind(hash).execute(&state.db).await.unwrap();

        // The upload is charged to the device's owner.
        assert_eq!(owner(&state.db, &bearer("rtf")).await.unwrap(), "user_f");
        // No credential, a user token or an unknown device are all refused.
        assert!(matches!(owner(&state.db, &HeaderMap::new()).await, Err(ApiError::Unauthorized(_))));
        let mut user_token = HeaderMap::new();
        user_token.insert(axum::http::header::AUTHORIZATION, "Bearer eyJhbGciOi.user.jwt".parse().unwrap());
        assert!(matches!(owner(&state.db, &user_token).await, Err(ApiError::Unauthorized(_))));
        assert!(owner(&state.db, &bearer("nobody")).await.is_err());

        // Authenticated but no R2 configured: a clear 503, not a silent success.
        if std::env::var("ALLTERNIT_R2_ENDPOINT").is_err() {
            let body = || FileBody { name: "a.txt".into(), content_type: "text/plain".into(), bytes: 5 };
            let r = begin(State(state.clone()), bearer("rtf"), Json(body())).await.unwrap();
            assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
            let r = complete(State(state.clone()), bearer("rtf"), Path(Uuid::new_v4()), Json(body())).await.unwrap();
            assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert!(begin(State(state), HeaderMap::new(), Json(body())).await.is_err());
        }
    }
}
