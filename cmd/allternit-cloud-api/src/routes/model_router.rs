//! Model router REST surface.
//!
//! Exposes:
//! - `GET /v1/models` — public list of available models.
//! - `POST /v1/chat/completions` — auth-protected OpenAI-compatible chat
//!   completions, with optional streaming.

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::sync::Arc;

use crate::{
    auth::AuthContext,
    model_router::generic_openai::{GenericOpenAiConfig, GenericOpenAiProvider},
    model_router::{ChatCompletionRequest, RetailPrices, UpstreamProvider},
    services::{inference_keys, inference_pool, inference_settlement},
    ApiError, ApiState,
};

/// Permission required to invoke chat completions.
const REQUIRED_PERMISSION: &str = "models:write";

/// Whether a caller may run chat completions. `has_permission` honors the
/// `"*"` wildcard.
fn can_run_inference(user: &crate::auth::AuthenticatedUser) -> bool {
    user.has_permission(REQUIRED_PERMISSION)
        || user.has_permission("inference")
        || user.has_permission("admin")
}

/// Response shape for the public model list endpoint.
#[derive(Debug, Serialize)]
pub struct ModelListResponse {
    object: String,
    data: Vec<crate::model_router::ModelInfo>,
}

/// `GET /v1/models`
///
/// Public endpoint listing all models Allternit can route. Returns a static
/// catalog enriched with live upstream metadata when providers are healthy.
pub async fn list_models(State(state): State<Arc<ApiState>>) -> Result<Json<ModelListResponse>, ApiError> {
    let models = state.model_router.list_models().await;

    Ok(Json(ModelListResponse {
        object: "list".to_string(),
        data: models,
    }))
}

/// `POST /v1/chat/completions`
///
/// Auth-protected OpenAI-compatible chat completions. Supports streaming via
/// `stream: true`. The request `model` field is an Allternit alias; the router
/// resolves it to the upstream model id before dispatching. Every successful
/// completion is metered and settled against the user's prepaid credits
/// (Phase B): blocked up front when the balance is exhausted, deducted after
/// the response from the upstream usage report (or a marked estimate).
pub async fn chat_completions(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    // Legacy tokens carry `models:write`; scoped keys minted for inference
    // carry `inference`. `has_permission` honors the `"*"` wildcard, which is
    // the production default for pre-scoping tokens.
    if !can_run_inference(&auth.user) {
        return Err(ApiError::Forbidden(format!(
            "Missing required permission: {} (or 'inference')",
            REQUIRED_PERMISSION
        )));
    }

    if !state.model_router.is_enabled() {
        return Err(ApiError::ServiceUnavailable(
            "Model router is not configured".to_string(),
        ));
    }

    let user_id = auth.user.user_id.clone();
    let alias = request.model.clone();

    // BYOK first: when the user has their own key for the alias's provider,
    // the call routes through it — their money, so our allowance, free rate
    // limit, pool breaker, and pool policy do not apply. Early return keeps
    // the shared path below readable.
    if let Some(provider_id) = state.model_router.provider_for_alias(&alias) {
        if inference_keys::should_route_byok(
            inference_keys::byok_enabled(),
            inference_keys::byok_base_url(provider_id).is_some(),
            match &state.inference_key_service {
                Some(service) => service.has_key(&user_id, provider_id).await?,
                None => false,
            },
        ) {
            return byok_chat_completions(&state, &user_id, provider_id, request).await;
        }
    }

    // Pre-check: a paying user with an exhausted balance is blocked before we
    // spend upstream money; free users (no credits row) are bounded by the
    // monthly free allowance and a tight per-user rate limit.
    let balance = inference_settlement::credit_balance_row(&state.db, &user_id).await?;
    inference_settlement::check_inference_allowed(&state.db, &user_id, balance).await?;
    if balance.is_none() {
        if let Err(info) = state.free_inference_rate_limiter.check(&user_id).await {
            tracing::warn!(user_id = %user_id, "free inference rate limit exceeded");
            let body = serde_json::json!({
                "error": "RATE_LIMIT_EXCEEDED",
                "message": "Free inference rate limit reached — add credits for higher limits.",
                "retry_after": info.reset_after.as_secs(),
            });
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                [(
                    axum::http::header::RETRY_AFTER,
                    info.reset_after.as_secs().to_string(),
                )],
                Json(body),
            )
                .into_response();
            info.add_headers(&mut response);
            return Ok(response);
        }
    }

    let streaming = request.is_streaming();
    let prompt_chars: usize = request.messages.iter().map(|m| m.content.len()).sum();
    let prices = state.model_router.retail_prices(&alias).await?;

    // Pool circuit breaker + free-tier pool policy. Unknown alias →
    // provider_for_alias is None and chat_completions 400s below; an
    // unseeded provider has no pool (unlimited) and fails cheap_only.
    let pool = match state.model_router.provider_for_alias(&alias) {
        Some(provider_id) => state.inference_pool_service.pool_for_provider(provider_id).await?,
        None => None,
    };
    if let Some(pool) = pool.as_ref() {
        state.inference_pool_service.check_pool_available(pool).await?;
    }
    inference_pool::check_free_tier_pool(
        inference_pool::free_tier_pool_policy(),
        balance.is_none(),
        pool.as_ref(),
    )?;
    let pool_id = pool.map(|pool| pool.id);

    let response = state.model_router.chat_completions(request).await?;

    tracing::info!(
        user_id = %user_id,
        token_id = %auth.user.token_id,
        status = %response.status(),
        streaming = streaming,
        "model router chat completion dispatched"
    );

    // Settle usage: non-streaming settles inline from the buffered JSON usage;
    // streaming settles when the wrapped body ends (or is dropped).
    let response = if streaming {
        inference_settlement::meter_stream_response(
            response,
            inference_settlement::StreamSettlement {
                db: Arc::new(state.db.clone()),
                user_id,
                model: alias,
                pool_id,
                charge_user: true,
                prices,
                prompt_token_estimate: (prompt_chars / 4) as u64,
            },
        )
    } else {
        inference_settlement::meter_json_response(
            &state.db,
            &user_id,
            &alias,
            pool_id.as_deref(),
            true,
            &prices,
            prompt_chars,
            response,
        )
        .await?
    };

    Ok(response)
}

/// BYOK path: dispatch the completion through the user's own provider key.
/// Tokens are metered (charge_user = false → usage row, no ledger deduction)
/// with pool_id NULL — reconciliation buckets NULL as `unattributed`, which
/// is correct: BYOK spend is not ours. Prices are recorded as zero (they are
/// cosmetic — nothing is charged; the token counts are the payload).
async fn byok_chat_completions(
    state: &Arc<ApiState>,
    user_id: &str,
    provider_id: &str,
    mut request: ChatCompletionRequest,
) -> Result<Response, ApiError> {
    let key_service = state
        .inference_key_service
        .as_ref()
        .expect("BYOK branch only runs with a key service");
    let api_key = key_service
        .get_decrypted(user_id, provider_id)
        .await?
        .ok_or_else(|| ApiError::Internal("BYOK key vanished between check and decrypt".to_string()))?;
    let base_url = inference_keys::byok_base_url(provider_id)
        .expect("BYOK branch only runs for registered providers");

    let alias = request.model.clone();
    let upstream_id = state
        .model_router
        .upstream_for_alias(&alias)
        .ok_or_else(|| ApiError::BadRequest(format!("Unknown model alias: {alias}")))?
        .to_string();

    let streaming = request.is_streaming();
    let prompt_chars: usize = request.messages.iter().map(|m| m.content.len()).sum();
    // Same usage-chunk injection as the shared router path.
    if streaming {
        request
            .extra
            .entry("stream_options".to_string())
            .or_insert_with(|| serde_json::json!({ "include_usage": true }));
    }
    request.model = upstream_id;

    let provider = GenericOpenAiProvider::new(GenericOpenAiConfig {
        provider_id: provider_id.to_string(),
        base_url: base_url.to_string(),
        api_key,
        model_list_cache_ttl: std::time::Duration::from_secs(300),
    });
    let response = provider.chat_completions(request).await?;

    tracing::info!(
        user_id = %user_id,
        provider_id = %provider_id,
        status = %response.status(),
        streaming = streaming,
        "byok chat completion dispatched"
    );

    let zero_prices = RetailPrices {
        prompt_per_1m: 0.0,
        completion_per_1m: 0.0,
        wholesale_prompt_per_1m: None,
        wholesale_completion_per_1m: None,
    };
    let response = if streaming {
        inference_settlement::meter_stream_response(
            response,
            inference_settlement::StreamSettlement {
                db: Arc::new(state.db.clone()),
                user_id: user_id.to_string(),
                model: alias,
                pool_id: None,
                charge_user: false,
                prices: zero_prices,
                prompt_token_estimate: (prompt_chars / 4) as u64,
            },
        )
    } else {
        inference_settlement::meter_json_response(
            &state.db,
            user_id,
            &alias,
            None,
            false,
            &zero_prices,
            prompt_chars,
            response,
        )
        .await?
    };

    Ok(response)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthenticatedUser;

    fn key_user(scopes: Vec<String>) -> AuthenticatedUser {
        // Mirrors `auth::middleware`: an `alt_` key authenticates with its
        // stored scopes as permissions.
        AuthenticatedUser {
            user_id: "u1".to_string(),
            token_id: "ak_test".to_string(),
            permissions: crate::services::api_keys::normalize_scopes(scopes),
        }
    }

    #[test]
    fn key_minted_with_default_scopes_can_run_inference() {
        assert!(can_run_inference(&key_user(vec![])));
    }

    #[test]
    fn compute_only_key_cannot_run_inference() {
        assert!(!can_run_inference(&key_user(vec!["compute".to_string()])));
    }

    #[test]
    fn explicit_inference_and_legacy_permissions_pass() {
        assert!(can_run_inference(&key_user(vec!["inference".to_string()])));
        assert!(can_run_inference(&key_user(vec!["models:write".to_string()])));
        assert!(can_run_inference(&AuthenticatedUser::development_user()));
    }
}
