//! LLM gateway — the OpenAI-compatible control plane in front of Gizzi.
//!
//! Three routers:
//!
//! - [`llm_gateway_router`] — the public OpenAI surface, nested at `/v1` in
//!   main.rs: `POST /v1/chat/completions`, `GET /v1/models`. It carries its
//!   own middleware chain (virtual-key auth → rate limit → org rate limit →
//!   DLP → budget pre-check) and is therefore mounted on the *public* router,
//!   outside Clerk auth.
//! - [`gateway_keys_router`] — key management, merged into the
//!   Clerk-protected `/api/v1` chain in main.rs (`/api/v1/gateway/keys*`).
//! - [`admin_routes::gateway_admin_router`] — observability + tenant config
//!   (usage, logs, routing decisions/policies, DLP rules, budgets), merged
//!   next to the keys router (`/api/v1/gateway/*`).
//!
//! Middleware order on the gateway router (first to run listed first):
//!   1. `auth::llm_key_middleware` — Bearer `ak-…` → [`auth::LlmKeyContext`]
//!      request extension.
//!   2. `auth::rate_limit_middleware` — per-key sliding window.
//!   3. `auth::org_rate_limit_middleware` — per-org sliding window
//!      (`organizations.gateway_rate_limit_rpm`, G14).
//!   4. `dlp::dlp_middleware` — B6 secret scanning + injection screening.
//!   5. `auth::budget_middleware` — monthly key/tenant cap pre-check.
//!
//! Cross-cutting modules:
//! - `router` + `benchmarks` — B5 routing policies: benchmark-weighted model
//!   selection with fallback-chain derivation; decisions persisted to
//!   `llm_routing_decisions`, linked to the usage row.
//! - `llm_pricing` — B4: server-side cost recompute from the models.dev
//!   cache; both costs stored, mismatch flagged, Prometheus series emitted
//!   from the single `proxy::record_usage_event` choke point.

pub mod admin_routes;
pub mod auth;
pub mod batches;
pub mod benchmarks;
pub mod cache;
pub mod citations;
pub mod context_cache;
pub mod data_residency;
pub mod dlp;
pub mod dlp_patterns;
pub mod embeddings;
pub mod estimation;
pub mod failover;
pub mod genai_spans;
pub mod files;
pub mod gizzi_bus;
pub mod images;
pub mod inference_hooks;
pub mod keys;
pub mod llm_pricing;
pub mod partial;
pub mod provider_routing;
pub mod proxy;
pub mod realtime_audio;
pub mod route_credentials;
pub mod refusal;
pub mod response_cache;
pub mod router;
pub mod shared_state;
pub mod safety;
pub mod translate;
pub mod vector_store;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Router,
};
use std::sync::Arc;

use crate::AppState;

/// Public OpenAI-compatible router (mounted at `/v1` in main.rs). Carries
/// its own virtual-key middleware chain — do not put it behind Clerk auth.
///
/// Takes the shared state because `from_fn_with_state` layers need it at
/// construction time (same pattern as `auth_middleware` in main.rs).
pub fn llm_gateway_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/chat/completions", post(proxy::chat_completions))
        .route("/chat/completions/best_of", post(partial::best_of_completions))
        .route("/chat/completions/partial", post(partial::partial_completions))
        .route("/tokens", post(proxy::count_tokens))
        .route("/rate-limits", get(proxy::rate_limits))
        .route(
            "/batches",
            post(batches::create_batch).get(batches::list_batches),
        )
        .route("/batches/:id", get(batches::get_batch))
        .route("/batches/:id/cancel", post(batches::cancel_batch))
        .route("/batches/:id/results", get(batches::batch_results))
        .route("/context-caches", post(context_cache::create_cache).get(context_cache::list_caches))
        .route("/context-caches/:id", get(context_cache::get_cache).delete(context_cache::delete_cache))
        // /models and /models/:id are intentionally omitted here; the Fabric
        // model catalog owns /v1/models when the LLM gateway is nested under /v1.
        .route("/pricing", get(proxy::list_pricing))
        .route("/files", post(files::create_file).get(files::list_files))
        .route("/files/:id", get(files::get_file).delete(files::delete_file))
        .route("/images/generations", post(images::create_images))
        .route(
            "/images/edits",
            post(images::edit_images).layer(axum::extract::DefaultBodyLimit::max(
                crate::media::handlers::MAX_IMAGE_GENERATE_BODY_BYTES,
            )),
        )
        .route("/images/variations", post(images::create_image_variations))
        .route("/embeddings", post(embeddings::create_embeddings))
        .route("/estimates/tokens", post(estimation::estimate_tokens))
        .route("/estimates/cost", post(estimation::estimate_cost))
        .route("/cache/prompts", post(cache::create_cache).get(cache::list_caches))
        .route("/cache/prompts/:id", get(cache::get_cache).delete(cache::delete_cache))
        .route("/realtime/sessions", post(realtime_audio::create_session).get(realtime_audio::list_sessions))
        .route("/realtime/sessions/:id", get(realtime_audio::get_session).delete(realtime_audio::delete_session))
        .route("/realtime/sessions/:id/ws", get(realtime_audio::ws_handler))
        .merge(vector_store::vector_store_router())
        // Layer order: the LAST layer added runs FIRST. Execution order is
        // therefore llm_key auth → rate limit → org rate limit → DLP →
        // budget → handler.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::budget_middleware,
        ))
        // B6: DLP runs between rate limiting and the budget pre-check so
        // secrets are caught before any spend is accounted.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            dlp::dlp_middleware,
        ))
        // G14: org-level RPM cap between the per-key limiter and DLP.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::org_rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state,
            auth::llm_key_middleware,
        ))
        // Header shape is request-global and is rejected before auth/spend.
        .layer(axum::middleware::from_fn(idempotency_key_middleware))
}

/// Reject malformed idempotency keys before authentication, routing, or spend.
/// Replay/persistence remains in the completion handler where the response body
/// and usage record can be finalized atomically.
async fn idempotency_key_middleware(request: Request, next: Next) -> Response {
    if let Some(value) = request.headers().get("idempotency-key") {
        let valid = value
            .to_str()
            .ok()
            .map(str::trim)
            .is_some_and(|key| !key.is_empty() && key.len() <= 255 && key.is_ascii());
        if !valid {
            return translate::OpenAiErrorResponse::new(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key must be 1-255 ASCII characters.",
                "invalid_request_error",
                Some("Idempotency-Key"),
                Some(translate::error_code::INVALID_REQUEST),
            )
            .into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::post};
    use tower::ServiceExt;

    #[tokio::test]
    async fn rejects_invalid_idempotency_key_before_handler() {
        let app = Router::new()
            .route("/", post(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn(idempotency_key_middleware));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("idempotency-key", " ")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

/// Clerk-protected key-management router (merged into the `/api/v1` chain in
/// main.rs, so paths land at `/api/v1/gateway/keys*`).
pub fn gateway_keys_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/gateway/keys", post(keys::create_key).get(keys::list_keys))
        .route(
            "/gateway/keys/:id",
            patch(keys::update_key).delete(keys::revoke_key),
        )
}
