//! BYOK inference key management.
//!
//! Caller verified per request — Clerk session or `allternit_*` API token
//! (`auth::resolve_user_id`), like the billing routes. Keys are
//! validated against the provider before storage, encrypted at rest with the
//! platform credential cipher, and never returned — list responses carry
//! masked fingerprints only. When `ALLTERNIT_CREDENTIALS_KEY` is unset (dev
//! without the cipher), all three endpoints answer 503
//! `inference_keys_not_configured` like the other operator-gated surfaces.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{services::inference_keys::is_key_provider, ApiState};

#[derive(Debug, Deserialize)]
pub struct PutKeyRequest {
    provider_id: String,
    api_key: String,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/inference/keys", get(list_keys).put(put_key))
        .route("/api/v1/inference/keys/:provider_id", delete(delete_key))
}

/// The BYOK key service, or 503 when the deployment has no credential cipher.
fn key_service(state: &ApiState) -> Result<&Arc<crate::services::InferenceKeyService>, Response> {
    state.inference_key_service.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "inference_keys_not_configured" })),
        )
            .into_response()
    })
}

async fn list_keys(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Response {
    let user_id = match crate::auth::resolve_user_scoped(&state.db, &headers, "inference").await {
        Ok(user) => user.id,
        Err(error) => return error.into_response(),
    };
    let service = match key_service(&state) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.list_for_user(&user_id).await {
        Ok(keys) => Json(keys).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn put_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<PutKeyRequest>,
) -> Response {
    let user_id = match crate::auth::resolve_user_scoped(&state.db, &headers, "inference").await {
        Ok(user) => user.id,
        Err(error) => return error.into_response(),
    };
    if !is_key_provider(&request.provider_id) {
        return crate::error::ApiError::BadRequest(format!(
            "Unknown BYOK provider: {:?}.",
            request.provider_id
        ))
        .into_response();
    }
    let service = match key_service(&state) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .upsert_and_validate(&user_id, &request.provider_id, &request.api_key)
        .await
    {
        Ok(info) => Json(info).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn delete_key(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
) -> Response {
    let user_id = match crate::auth::resolve_user_scoped(&state.db, &headers, "inference").await {
        Ok(user) => user.id,
        Err(error) => return error.into_response(),
    };
    let service = match key_service(&state) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.delete(&user_id, &provider_id).await {
        Ok(true) => Json(serde_json::json!({ "deleted": true, "provider_id": provider_id }))
            .into_response(),
        Ok(false) => crate::error::ApiError::NotFound(format!(
            "No inference key for provider {provider_id:?}."
        ))
        .into_response(),
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::inference_keys::InferenceKeyInfo;

    #[test]
    fn key_info_serializes_without_plaintext() {
        let info = InferenceKeyInfo {
            provider_id: "groq".to_string(),
            masked: "gsk…9999".to_string(),
            status: "active".to_string(),
            last_validated_at: None,
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["provider_id"], serde_json::json!("groq"));
        assert_eq!(value["masked"], serde_json::json!("gsk…9999"));
        assert_eq!(value["status"], serde_json::json!("active"));
    }
}
