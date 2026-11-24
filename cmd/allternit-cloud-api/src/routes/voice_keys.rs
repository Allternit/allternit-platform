//! Voice keys saved to the account, and the QR handoff relay
//! (services::voice_keys, migrations 128 + 135).
//!
//! These routes hand back decrypted API keys, so they take a signed-in Clerk
//! session only: `auth::require_session_user` answers 403 for every API token
//! (platform `alt_…` keys, `allternit_*` tokens, the gated dev token), and 401
//! with no credentials. Responses carry `Cache-Control: no-store`. Bodies are
//! parsed by hand so a malformed body gets a static 400 that never echoes what
//! was sent, and nothing here logs a key.
//!
//! The handoff's sealed key is end-to-end encrypted between the two devices
//! (AES-256-GCM under their P-256 ECDH key). The relay never sees the sender's
//! public key, which is in QR1 only, so it can neither read the key nor forge
//! a matching 6-digit code. The take is one-time.
//!
//! - `GET    /api/v1/voice/keys`                    → `{keys:[{provider, api_key, voice_id, label, updated_at}]}`
//! - `PUT    /api/v1/voice/keys/:provider`          `{api_key, voice_id?, label?}` → the row
//! - `DELETE /api/v1/voice/keys`                    → 204
//! - `POST   /api/v1/voice/handoff`                 → `{sid, expires_at}`
//! - `PUT    /api/v1/voice/handoff/:sid/receiver`   `{pubkey}` → 204 / 409 / 404
//! - `GET    /api/v1/voice/handoff/:sid`            → `{sid, receiver_pubkey|null, expires_at, delivered}`
//! - `PUT    /api/v1/voice/handoff/:sid/sealed`     `{nonce, ciphertext}` → 204 / 409 / 404 / 400
//! - `GET    /api/v1/voice/handoff/:sid/sealed`     → 200 `{nonce, ciphertext}` once / 204 not yet / 409 delivered / 404

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;
use crate::services::voice_keys::{SealedTake, VoiceKeyService};
use crate::ApiState;

/// No `Debug`: the body holds the API key.
#[derive(Deserialize)]
struct PutKeyBody {
    api_key: String,
    #[serde(default)]
    voice_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
struct ReceiverBody {
    pubkey: String,
}

/// No `Debug`: the body holds the sealed key.
#[derive(Deserialize)]
struct SealedBody {
    nonce: String,
    ciphertext: String,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/voice/keys", get(list_keys).delete(delete_keys))
        .route("/api/v1/voice/keys/:provider", put(put_key))
        .route("/api/v1/voice/handoff", post(create_handoff))
        .route("/api/v1/voice/handoff/:sid", get(get_handoff))
        .route("/api/v1/voice/handoff/:sid/receiver", put(set_receiver))
        .route("/api/v1/voice/handoff/:sid/sealed", put(put_sealed).get(take_sealed))
}

fn service(state: &ApiState) -> VoiceKeyService {
    VoiceKeyService::new(state.db.clone(), state.credential_cipher.clone())
}

/// The signed-in caller's id; API tokens are 403.
async fn session_user(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    // Tests stand in for a Clerk session with this header; compiled out of
    // every non-test build (same pattern as artifacts_v2::caller).
    #[cfg(test)]
    if let Some(id) = headers.get("x-test-session-user").and_then(|v| v.to_str().ok()) {
        return Ok(id.to_string());
    }
    Ok(crate::auth::require_session_user(&state.db, headers).await?.id)
}

fn parse<T: for<'de> Deserialize<'de>>(body: &Bytes, expected: &str) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| ApiError::BadRequest(format!("Expected a JSON body {expected}.")))
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

fn reply(result: Result<Response, ApiError>) -> Response {
    no_store(result.unwrap_or_else(IntoResponse::into_response))
}

async fn list_keys(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let keys = service(&state).list(&user).await?;
        Ok(Json(json!({ "keys": keys })).into_response())
    }
    .await)
}

async fn put_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    body: Bytes,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: PutKeyBody = parse(&body, "{api_key, voice_id?, label?}")?;
        let key = service(&state)
            .upsert(&user, &provider, &body.api_key, body.voice_id.as_deref(), body.label.as_deref())
            .await?;
        Ok(Json(key).into_response())
    }
    .await)
}

async fn delete_keys(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        service(&state).delete_all(&user).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await)
}

async fn create_handoff(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let handoff = service(&state).create_handoff(&user).await?;
        Ok(Json(json!({ "sid": handoff.sid, "expires_at": handoff.expires_at })).into_response())
    }
    .await)
}

async fn set_receiver(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    body: Bytes,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: ReceiverBody = parse(&body, "{pubkey}")?;
        service(&state).set_receiver(&user, &sid, &body.pubkey).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await)
}

async fn put_sealed(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    body: Bytes,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        let body: SealedBody = parse(&body, "{nonce, ciphertext}")?;
        service(&state).put_sealed(&user, &sid, &body.nonce, &body.ciphertext).await?;
        Ok(StatusCode::NO_CONTENT.into_response())
    }
    .await)
}

async fn take_sealed(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        match service(&state).take_sealed(&user, &sid).await? {
            SealedTake::Taken(sealed) => Ok(Json(sealed).into_response()),
            SealedTake::NotYet => Ok(StatusCode::NO_CONTENT.into_response()),
        }
    }
    .await)
}

async fn get_handoff(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
) -> Response {
    reply(async {
        let user = session_user(&state, &headers).await?;
        match service(&state).get_handoff(&user, &sid).await? {
            Some(handoff) => Ok(Json(handoff).into_response()),
            None => Err(ApiError::NotFound("Handoff not found or expired.".to_string())),
        }
    }
    .await)
}

#[cfg(test)]
#[path = "voice_keys_tests.rs"]
mod tests;
