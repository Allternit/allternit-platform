//! Model router REST surface.
//!
//! Exposes:
//! - `GET /v1/models` — public list of available models.
//! - `POST /v1/chat/completions` — auth-protected OpenAI-compatible chat
//!   completions, with optional streaming.

use axum::{
    extract::{Extension, State},
    http::{HeaderMap, HeaderValue, StatusCode},
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
/// Public endpoint listing the models Allternit can route: default model
/// first (each row with `default: bool`), unlisted aliases last with
/// `hidden: true` (pickers skip them; saved ids still route). Enriched with
/// live upstream metadata when providers are healthy.
pub async fn list_models(State(state): State<Arc<ApiState>>) -> Result<Json<ModelListResponse>, ApiError> {
    let mut models = state.model_router.list_models().await;
    // A disabled shared pool is refused by chat_completions. Do not offer its
    // models in the apps' picker either. Unseeded providers keep the existing
    // catalog behavior; this only honors an explicit operator disable.
    let disabled: Vec<String> = sqlx::query_scalar(
        "SELECT provider_id FROM inference_pools WHERE enabled = FALSE",
    )
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::DatabaseError)?;
    models.retain(|model| !disabled.iter().any(|id| model.provider.as_ref() == Some(id)));

    Ok(Json(ModelListResponse {
        object: "list".to_string(),
        data: models,
    }))
}

/// Response header naming the request; the same id is on its `inference_usage` row.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// The `api_keys` row behind a request (its Platform project and kind), when
/// the token is one. `allternit_*` tokens, device tokens and a failed lookup
/// mean none.
async fn api_key_of(db: &sqlx::PgPool, key_id: &str) -> Option<(Option<String>, String)> {
    match sqlx::query_as::<_, (Option<String>, String)>("SELECT project_id, kind FROM api_keys WHERE id = $1")
        .bind(key_id)
        .fetch_optional(db)
        .await
    {
        Ok(row) => row,
        Err(error) => {
            tracing::debug!(%error, "api key attribution lookup skipped");
            None
        }
    }
}

/// Who is making this completion, for its usage row: the key's project and
/// id when the token is an `api_keys` row, and the client (Decision D3,
/// [`inference_settlement::derive_client`]).
pub(crate) async fn inference_attribution(
    db: &sqlx::PgPool,
    user: &crate::auth::AuthenticatedUser,
    headers: &HeaderMap,
    request_id: String,
) -> inference_settlement::InferenceAttribution {
    let device = crate::routes::runtime_pairing::device_token_from_headers(headers).is_some();
    let key = if device { None } else { api_key_of(db, &user.token_id).await };
    let client_header = headers
        .get(inference_settlement::CLIENT_HEADER)
        .and_then(|value| value.to_str().ok());
    let client = inference_settlement::derive_client(
        device,
        key.as_ref().map(|(_, kind)| kind.as_str()),
        client_header,
    );
    inference_settlement::InferenceAttribution {
        project_id: key.as_ref().and_then(|(project_id, _)| project_id.clone()),
        key_id: key.map(|_| user.token_id.clone()),
        client: Some(client.to_string()),
        request_id: Some(request_id),
        started_at: Some(std::time::Instant::now()),
        capture_request: None,
        person_turn: headers
            .get(inference_settlement::TURN_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("person")),
    }
}

/// The Platform project whose hosted runtime makes this completion: a device
/// token (client `runtime`) owned by `platform:<project>`. Such a runtime runs
/// the project's `"allternit"` agents on Allternit Cloud, and the project pays
/// for those turns (`platform_v1::conversations::meter_turn` bills their tokens),
/// so the consumer gates (verified email, the free allowance and its rate
/// limit, the free-tier pool policy) don't apply and no user's credits are
/// charged; the project's own spend gate applies instead. Without this every
/// Cloud turn of a hosted agent was refused: `platform:…` has no Clerk email.
pub(crate) fn platform_runtime_project<'a>(user_id: &'a str, client: Option<&str>) -> Option<&'a str> {
    if client != Some("runtime") {
        return None;
    }
    // The owner `platform_v1::hosting::runtime_owner` gives a project's runtime.
    user_id.strip_prefix("platform:").filter(|project| !project.is_empty())
}

/// The request body to keep for the console Logs page (Decision D6): only
/// with `PLATFORM_LOG_BODIES=1`, a key whose project has `log_bodies` on, and
/// a body under the capture cap. `None` otherwise.
async fn captured_request_body(
    db: &sqlx::PgPool,
    project_id: Option<&str>,
    request: &ChatCompletionRequest,
) -> Option<Arc<serde_json::Value>> {
    let project_id = project_id?;
    if !inference_settlement::log_bodies_enabled() {
        return None;
    }
    let opted_in: Option<bool> = sqlx::query_scalar("SELECT log_bodies FROM platform_projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(db)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, project_id, "log_bodies lookup failed; bodies not kept");
            None
        });
    if opted_in != Some(true) {
        return None;
    }
    let body = serde_json::to_value(request).ok()?;
    let size = serde_json::to_vec(&body).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
    (size <= inference_settlement::MAX_CAPTURED_BODY_BYTES).then(|| Arc::new(body))
}

/// `POST /v1/chat/completions`
///
/// Auth-protected OpenAI-compatible chat completions. Supports streaming via
/// `stream: true`. The request `model` field is an Allternit alias; the router
/// resolves it to the upstream model id before dispatching. Every successful
/// completion is metered and settled against the user's prepaid credits
/// (Phase B): blocked up front when the balance is exhausted, deducted after
/// the response from the upstream usage report (or a marked estimate).
/// Every response, errors and streams included, carries `x-request-id`; the
/// usage row records it with the key's project, the key and the client.
pub async fn chat_completions(
    State(state): State<Arc<ApiState>>,
    Extension(auth): Extension<AuthContext>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Response, ApiError> {
    let request_id = format!("req_{}", uuid::Uuid::new_v4().simple());
    let mut attribution = inference_attribution(&state.db, &auth.user, &headers, request_id.clone()).await;
    attribution.capture_request =
        captured_request_body(&state.db, attribution.project_id.as_deref(), &request).await;
    let mut response = match chat_completions_attributed(&state, auth, request, attribution).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    };
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    Ok(response)
}

async fn chat_completions_attributed(
    state: &Arc<ApiState>,
    auth: AuthContext,
    mut request: ChatCompletionRequest,
    attribution: inference_settlement::InferenceAttribution,
) -> Result<Response, ApiError> {
    // `allternit.*` request options (handoff model) never reach an upstream.
    let open_lane_options = crate::services::open_lane::take_request_options(&mut request);

    // Legacy tokens carry `models:write`; scoped keys minted for inference
    // carry `inference`. `has_permission` honors the `"*"` wildcard, which is
    // the production default for pre-scoping tokens.
    if !can_run_inference(&auth.user) {
        return Err(ApiError::Forbidden(format!(
            "Missing required permission: {} (or 'inference')",
            REQUIRED_PERMISSION
        )));
    }

    // Plan and Copilot models (contract 9.3): the person's own plan on their
    // brain runner, person-sent chat only (D7). They never reach the catalog,
    // the credit checks or an upstream.
    if is_person_only_model(&request.model) {
        return plan_turn(state, &auth.user.user_id, request, &attribution).await;
    }

    if !state.model_router.is_enabled() {
        return Err(ApiError::ServiceUnavailable(
            "Model router is not configured".to_string(),
        ));
    }

    // A key that belongs to a Platform API project bills to its project: no
    // card on file, no inference (Eoj 2026-10-08, no free usage).
    // The console playground's key is the signed-in user's own key with the
    // project attached only for attribution: it bills to the user's credits
    // (check_inference_allowed below), so the project's card is not required.
    // `client` is `playground` only for an `api_keys` row of that kind.
    let playground = attribution.client.as_deref() == Some(crate::services::api_keys::KEY_KIND_PLAYGROUND);
    let platform_runtime = platform_runtime_project(&auth.user.user_id, attribution.client.as_deref()).map(str::to_string);
    if let Some(project_id) = attribution.project_id.as_deref().filter(|_| !playground).or(platform_runtime.as_deref()) {
        if let Err(e) = crate::routes::platform_v1::billing::spend_allowed(&state.db, project_id).await {
            return Ok(e.into_response());
        }
    }

    let user_id = auth.user.user_id.clone();
    let alias = request.model.clone();

    // Hard lines hold on every Cloud request, BYOK included: the keyword
    // pre-check here, the full moderation check on the open lane below.
    if let Some(line) = crate::services::hard_line::standard_check(&request.messages) {
        crate::services::open_lane::record_block(&state.db, &user_id, line.category()).await;
        return Ok(crate::services::open_lane::hard_line_response(line.category()));
    }

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
            let upstream_id = state
                .model_router
                .upstream_for_alias(&alias)
                .ok_or_else(|| ApiError::BadRequest(format!("Unknown model alias: {alias}")))?
                .to_string();
            return byok_chat_completions(state, &user_id, provider_id, upstream_id, request, attribution).await;
        }
    } else if let Some((provider_id, vendor_model)) = inference_keys::direct_model(&alias) {
        // `<provider>/<vendor model>` (not a catalog alias): the person's own
        // model on their own key, e.g. `xai/grok-4`, `google/gemini-2.5-pro`.
        let has_key = match &state.inference_key_service {
            Some(service) if inference_keys::byok_enabled() => service.has_key(&user_id, provider_id).await?,
            _ => false,
        };
        if !has_key {
            return Err(ApiError::BadRequest(format!(
                "{alias} runs on your own {provider_id} key. Add one in Connections."
            )));
        }
        let vendor_model = vendor_model.to_string();
        return byok_chat_completions(state, &user_id, provider_id, vendor_model, request, attribution).await;
    }

    // Upstream funding: skip a provider under its funding floor when another
    // provider serves the same model (prices, pool and usage follow the
    // provider that actually runs it).
    let mut request = request;
    if let Some(funded) = state.model_router.funded_alternative(&alias) {
        tracing::warn!(from = %alias, to = %funded, "provider below funding floor; routing to another provider");
        request.model = funded;
    }
    let alias = request.model.clone();

    // Pre-check: a paying user with an exhausted balance is blocked before we
    // spend upstream money; free users (no credits row) are bounded by the
    // monthly free allowance and a tight per-user rate limit.
    let balance = inference_settlement::credit_balance_row(&state.db, &user_id).await?;
    if platform_runtime.is_none() {
        inference_settlement::check_inference_allowed(&state.db, &user_id, balance).await?;
    }
    if balance.is_none() && platform_runtime.is_none() {
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

    // Open lane: an `-open` model needs the terms, credits and a clean
    // hard-line check; any other model may be handed off if it refuses.
    let has_credits = balance.is_some();
    if let Some(blocked) =
        crate::routes::open_lane::guard_open_request(&state, &user_id, has_credits, &request).await?
    {
        return Ok(blocked);
    }
    let handoff = crate::routes::open_lane::plan_handoff(
        &state,
        &user_id,
        &alias,
        &open_lane_options,
        has_credits,
    )
    .await?;

    let streaming = request.is_streaming();
    let prompt_chars: usize = request.messages.iter().map(|m| m.content_chars()).sum();
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
        balance.is_none() && platform_runtime.is_none(),
        pool.as_ref(),
    )?;
    let pool_id = pool.map(|pool| pool.id);

    if let Some(plan) = handoff {
        return crate::routes::open_lane::chat_with_handoff(state, user_id, plan, request, pool_id, prices, attribution).await;
    }

    let response = state.model_router.chat_completions(request).await?;

    tracing::info!(
        user_id = %user_id,
        token_id = %auth.user.token_id,
        request_id = attribution.request_id.as_deref().unwrap_or(""),
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
                charge_user: platform_runtime.is_none(),
                prices,
                prompt_token_estimate: (prompt_chars / 4) as u64,
                attribution,
            },
        )
    } else {
        inference_settlement::meter_json_response(
            &state.db,
            &user_id,
            &alias,
            pool_id.as_deref(),
            platform_runtime.is_none(),
            &prices,
            prompt_chars,
            response,
            &attribution,
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
    upstream_id: String,
    mut request: ChatCompletionRequest,
    attribution: inference_settlement::InferenceAttribution,
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

    let streaming = request.is_streaming();
    let prompt_chars: usize = request.messages.iter().map(|m| m.content_chars()).sum();
    // Same usage-chunk injection as the shared router path.
    if streaming {
        request
            .extra
            .entry("stream_options".to_string())
            .or_insert_with(|| serde_json::json!({ "include_usage": true }));
    }
    request.model = upstream_id;

    // Anthropic speaks the Messages API (translated both ways); every other
    // BYOK host is OpenAI-compatible.
    let response = if provider_id == "anthropic" {
        crate::model_router::anthropic::AnthropicProvider::new(api_key).chat_completions(request).await?
    } else {
        let provider = GenericOpenAiProvider::new(GenericOpenAiConfig {
            provider_id: provider_id.to_string(),
            base_url: base_url.to_string(),
            api_key,
            model_list_cache_ttl: std::time::Duration::from_secs(300),
        });
        provider.chat_completions(request).await?
    };

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
                attribution,
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
            &attribution,
        )
        .await?
    };

    Ok(response)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthenticatedUser;

    // Regression: a disabled pool rejected every chat but stayed selectable
    // in the public model list. Exercise the actual GET route and Postgres.
    #[tokio::test]
    async fn disabled_pool_models_are_hidden_until_pool_is_reenabled() {
        use axum::{body::{to_bytes, Body}, http::Request, routing::get, Router};
        use tower::ServiceExt;
        use crate::routes::test_support::{test_state, MockGateway};
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::query("CREATE TABLE inference_pools (provider_id TEXT PRIMARY KEY, enabled BOOLEAN NOT NULL)")
            .execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO inference_pools VALUES ('groq', FALSE), ('fireworks', TRUE)")
            .execute(&state.db).await.unwrap();
        let app = Router::new().route("/v1/models", get(list_models)).with_state(state.clone());
        let get_models = |app: Router| async move {
            let response = app.oneshot(Request::builder().uri("/v1/models").body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["data"].as_array().unwrap().clone()
        };
        let models = get_models(app.clone()).await;
        assert!(!models.iter().any(|model| model["provider"] == "groq"));
        // An enabled pool stays listed; an unlisted alias is still returned,
        // flagged hidden.
        let glm = models.iter().find(|model| model["id"] == "glm-5.3-fireworks").unwrap();
        assert_eq!(glm["hidden"], true);
        assert_eq!(models[0]["id"], "kimi-k3");
        assert_eq!(models[0]["default"], true);
        assert!(models[0].get("hidden").is_none());
        sqlx::query("UPDATE inference_pools SET enabled = TRUE WHERE provider_id = 'groq'")
            .execute(&state.db).await.unwrap();
        let models = get_models(app).await;
        assert!(models.iter().any(|model| model["id"] == "gpt-oss-20b-groq"));
    }

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

// ---- plan models (CONTRACT-connections-2026-11 section 9.3) -----------------

use crate::services::brain_runners::{self, ChatMessage, TurnEvent, Vendor, VendorState};

/// Plan (`plan/…`, web's in-app `plan-…`) and Copilot (`copilot/…`) ids:
/// person-sent chat only. Every other route that takes a model refuses them.
pub fn is_person_only_model(model: &str) -> bool {
    let model = model.trim();
    model.starts_with("plan/") || model.starts_with("plan-") || model.starts_with("copilot/")
}

/// `plan/<vendor>/<model>` (wire) or `plan-<vendor>/<model>` (web in-app).
/// The model may hold a slash (`plan/kimi/kimi-code/k3`).
pub fn plan_model(model: &str) -> Option<(Vendor, &str)> {
    let model = model.trim();
    let rest = model.strip_prefix("plan/").or_else(|| model.strip_prefix("plan-"))?;
    let (vendor, name) = rest.split_once('/')?;
    let vendor = Vendor::parse(vendor)?;
    (!name.trim().is_empty()).then_some((vendor, name))
}

/// `{"error":{"code","message"}}` with its status (contract 9.3).
pub fn plan_refusal(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": { "code": code, "message": message } }))).into_response()
}

pub const PLAN_PERSON_ONLY_MESSAGE: &str = "Your plan only answers messages you send yourself, in chat.";

pub fn plan_person_only() -> Response {
    plan_refusal(StatusCode::FORBIDDEN, "plan_person_only", PLAN_PERSON_ONLY_MESSAGE)
}

/// The text of a message (a string, or the text parts of an array).
fn message_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn turn_error(code: &str, message: &str) -> Response {
    match code {
        "login_expired" => plan_refusal(StatusCode::CONFLICT, code, "Sign-in expired"),
        "vendor_limited" => plan_refusal(StatusCode::CONFLICT, code, if message.is_empty() { "The vendor limited this plan" } else { message }),
        brain_runners::RUNNER_BUSY => plan_refusal(StatusCode::SERVICE_UNAVAILABLE, code, "Busy. Try again"),
        _ => plan_refusal(StatusCode::BAD_GATEWAY, brain_runners::COULD_NOT_START, "Could not start. Try again"),
    }
}

/// One plan turn on the person's runner (D6, D7, D8). Not metered: the
/// person's own plan pays, no Allternit credits or upstream are used.
async fn plan_turn(
    state: &Arc<ApiState>,
    user_id: &str,
    request: ChatCompletionRequest,
    attribution: &inference_settlement::InferenceAttribution,
) -> Result<Response, ApiError> {
    if !inference_settlement::person_turn_allowed(attribution) {
        return Ok(plan_person_only());
    }
    if request.messages.iter().any(|m| m.role == "tool") {
        return Ok(plan_refusal(StatusCode::BAD_REQUEST, "plan_no_tools", "Plan chats can't use tools."));
    }
    if let Some(line) = crate::services::hard_line::standard_check(&request.messages) {
        crate::services::open_lane::record_block(&state.db, user_id, line.category()).await;
        return Ok(crate::services::open_lane::hard_line_response(line.category()));
    }
    let not_connected = || plan_refusal(StatusCode::FORBIDDEN, "plan_not_connected", "Connect this plan in Connections first.");
    // Copilot (`copilot/…`) is its own server flow (step 1.5); until then it is not connected.
    let Some((vendor, model)) = plan_model(&request.model) else {
        return Ok(not_connected());
    };
    let service = crate::routes::connections::runners(state);
    let signed_in = service.offers(vendor)
        && service
            .vendor_state(user_id)
            .await?
            .iter()
            .any(|(v, s)| *v == vendor && matches!(s, VendorState::On { .. }));
    if !signed_in {
        return Ok(not_connected());
    }
    // Only role and text reach the runner: `tools`, `tool_choice` and every
    // other field are dropped here. History is replayed each turn (D8).
    let messages = request
        .messages
        .iter()
        .map(|m| ChatMessage {
            role: if m.role == "developer" { "system".to_string() } else { m.role.clone() },
            content: message_text(&m.content),
        })
        .collect();
    let mut events = match service.turn(user_id, vendor, model, messages).await {
        Ok(events) => events,
        Err(error) => return brain_runners::error_response(&error).map(Ok).unwrap_or(Err(error)),
    };
    use futures::StreamExt;
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let created = chrono::Utc::now().timestamp();
    let alias = request.model.clone();
    if !request.is_streaming() {
        let mut text = String::new();
        let mut usage = serde_json::Value::Null;
        while let Some(event) = events.next().await {
            match event {
                TurnEvent::Delta(delta) => text.push_str(&delta),
                TurnEvent::Done { input_tokens, output_tokens } => {
                    usage = serde_json::json!({
                        "prompt_tokens": input_tokens,
                        "completion_tokens": output_tokens,
                        "total_tokens": input_tokens.zip(output_tokens).map(|(a, b)| a + b),
                    });
                    break;
                }
                TurnEvent::Error { code, message } => return Ok(turn_error(code, &message)),
            }
        }
        return Ok(Json(serde_json::json!({
            "id": id,
            "object": "chat.completion",
            "created": created,
            "model": alias,
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
            "usage": usage,
        }))
        .into_response());
    }
    let chunk = move |delta: serde_json::Value, finish: Option<&str>, usage: Option<serde_json::Value>| {
        let mut body = serde_json::json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": alias,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        });
        if let Some(usage) = usage {
            body["usage"] = usage;
        }
        format!("data: {body}\n\n")
    };
    let sse = events.map(move |event| {
        let text = match event {
            TurnEvent::Delta(delta) => chunk(serde_json::json!({ "role": "assistant", "content": delta }), None, None),
            TurnEvent::Done { input_tokens, output_tokens } => {
                let usage = serde_json::json!({ "prompt_tokens": input_tokens, "completion_tokens": output_tokens });
                format!("{}data: [DONE]\n\n", chunk(serde_json::json!({}), Some("stop"), Some(usage)))
            }
            TurnEvent::Error { code, message } => {
                let error = serde_json::json!({ "error": { "code": code, "message": message } });
                format!("data: {error}\n\ndata: [DONE]\n\n")
            }
        };
        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(text))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
        .header(axum::http::header::CACHE_CONTROL, "no-cache")
        .body(axum::body::Body::from_stream(sse))
        .map_err(|e| ApiError::Internal(e.to_string()))
}
