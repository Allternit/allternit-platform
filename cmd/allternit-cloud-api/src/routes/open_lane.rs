//! Open lane routes and the chat-completions hooks.
//!
//! - `GET /api/v1/open-lane` / `PUT /api/v1/open-lane`: the caller's
//!   Settings > Open models state (acceptable use, 18+ confirmation, handoff
//!   opt-in). Clerk session or an `account` API token.
//! - [`guard_open_request`]: requests that name an open-lane model need the
//!   terms (403 `open_lane_terms_required`), credits (402
//!   `open_lane_requires_credits`) and a clean hard-line check (400
//!   `content_policy_hard_line`, or 503 `open_lane_moderation_unavailable`
//!   when moderation cannot run: fail closed).
//! - [`plan_handoff`] + [`chat_with_handoff`]: with handoff on, a frontier
//!   refusal is re-run on an open model after the hard-line check. The
//!   handed-off response carries `x-allternit-handoff: <from>-><to>` and
//!   `allternit.handoff`; each model's own tokens are billed. Streams hold
//!   the first 400 characters before flushing so a refusal can be swapped
//!   before the client sees it.

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderMap},
    response::Response,
    routing::get,
    Json, Router,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::{
    auth,
    error::ApiError,
    model_router::{catalog::OPEN_LANE_PROVIDER, ChatCompletionRequest, ModelRouterError, RetailPrices},
    services::{
        hard_line, inference_settlement,
        open_lane::{self, OpenLaneSettings, RequestOptions},
        refusal,
    },
    ApiState,
};

/// Longest a stream is held waiting for the first 400 characters.
const STREAM_HOLD_TIMEOUT: Duration = Duration::from_secs(20);

const MAX_BUFFERED_JSON_BODY: usize = 32 * 1024 * 1024;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/open-lane", get(get_settings).put(put_settings))
}

fn open_models(state: &ApiState) -> Vec<String> {
    if !state.model_router.has_provider(OPEN_LANE_PROVIDER) {
        return Vec::new();
    }
    ["deepseek-v4.1-flash-open", "glm-5.3-flash-open"]
        .iter()
        .filter(|alias| state.model_router.is_open_lane_alias(alias))
        .map(|alias| alias.to_string())
        .collect()
}

fn settings_body(state: &ApiState, settings: &OpenLaneSettings) -> Value {
    let models = open_models(state);
    json!({
        "available": !models.is_empty(),
        "models": models,
        "aup_version": open_lane::AUP_VERSION,
        "aup_accepted_at": settings.aup_accepted_at,
        "age_confirmed_at": settings.age_confirmed_at,
        "terms_accepted": settings.terms_ok(),
        "handoff_enabled": settings.handoff_enabled,
        "default_handoff_model": open_lane::DEFAULT_HANDOFF_MODEL,
    })
}

async fn get_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    let settings = open_lane::load_settings(&state.db, &user.id).await?;
    Ok(Json(settings_body(&state, &settings)))
}

#[derive(Debug, Deserialize)]
struct SettingsUpdate {
    accept_aup: Option<bool>,
    confirm_age: Option<bool>,
    handoff_enabled: Option<bool>,
}

async fn put_settings(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(update): Json<SettingsUpdate>,
) -> Result<Json<Value>, ApiError> {
    let user = auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    let current = open_lane::load_settings(&state.db, &user.id).await?;
    let next = open_lane::apply_update(
        current,
        update.accept_aup,
        update.confirm_age,
        update.handoff_enabled,
        chrono::Utc::now(),
    )
    .map_err(ApiError::BadRequest)?;
    open_lane::save_settings(&state.db, &user.id, &next).await?;
    Ok(Json(settings_body(&state, &next)))
}

/// Run the hard-line filter. `Err(response)` = blocked (logged by category)
/// or moderation unavailable (fail closed).
async fn hard_line_gate(state: &ApiState, user_id: &str, request: &ChatCompletionRequest) -> Result<(), Response> {
    match hard_line::check(&state.model_router, &request.messages).await {
        Ok(None) => Ok(()),
        Ok(Some(line)) => {
            open_lane::record_block(&state.db, user_id, line.category()).await;
            Err(open_lane::hard_line_response(line.category()))
        }
        Err(error) => {
            tracing::warn!(%error, "open lane moderation unavailable; failing closed");
            Err(open_lane::moderation_unavailable_response())
        }
    }
}

/// Gate a request that names an open-lane model. `Some(response)` = stop
/// and answer with it.
pub async fn guard_open_request(
    state: &ApiState,
    user_id: &str,
    has_credits: bool,
    request: &ChatCompletionRequest,
) -> Result<Option<Response>, ApiError> {
    if !state.model_router.is_open_lane_alias(&request.model) {
        return Ok(None);
    }
    let settings = open_lane::load_settings(&state.db, user_id).await?;
    if !settings.terms_ok() {
        return Ok(Some(open_lane::terms_required_response()));
    }
    if !has_credits {
        return Ok(Some(open_lane::credits_required_response()));
    }
    Ok(hard_line_gate(state, user_id, request).await.err())
}

/// A planned handoff: the model the caller asked for and the open model a
/// refusal goes to.
#[derive(Debug, Clone)]
pub struct HandoffPlan {
    pub from: String,
    pub to: String,
}

/// Decide whether this request may be handed off if refused. Cheap exits
/// first so most requests never touch the database.
pub async fn plan_handoff(
    state: &ApiState,
    user_id: &str,
    alias: &str,
    options: &RequestOptions,
    has_credits: bool,
) -> Result<Option<HandoffPlan>, ApiError> {
    let requested_is_open = state.model_router.is_open_lane_alias(alias);
    if requested_is_open
        || options.handoff_disabled
        || !has_credits
        || !state.model_router.has_provider(OPEN_LANE_PROVIDER)
    {
        return Ok(None);
    }
    let to = options
        .handoff_model
        .clone()
        .filter(|m| state.model_router.is_open_lane_alias(m))
        .unwrap_or_else(|| open_lane::DEFAULT_HANDOFF_MODEL.to_string());
    let target_available = state.model_router.is_open_lane_alias(&to);
    let settings = open_lane::load_settings(&state.db, user_id).await?;
    Ok(open_lane::should_hand_off(&settings, options, requested_is_open, has_credits, target_available)
        .then(|| HandoffPlan { from: alias.to_string(), to }))
}

fn prompt_chars(request: &ChatCompletionRequest) -> usize {
    request.messages.iter().map(|m| m.content_chars()).sum()
}

/// Dispatch a request that may be handed off, settle every model's own
/// tokens, and answer with the frontier response or the handed-off one.
#[allow(clippy::too_many_arguments)]
pub async fn chat_with_handoff(
    state: &Arc<ApiState>,
    user_id: String,
    plan: HandoffPlan,
    request: ChatCompletionRequest,
    pool_id: Option<String>,
    prices: RetailPrices,
) -> Result<Response, ApiError> {
    let original = request.clone();
    let streaming = request.is_streaming();
    let chars = prompt_chars(&request);

    let frontier = match state.model_router.chat_completions(request).await {
        Ok(response) => response,
        // A provider that refuses with a content-policy error produced no
        // tokens: nothing to bill on the frontier side.
        Err(ModelRouterError::UpstreamError { status, message }) if refusal::error_is_refusal(&message) => {
            return match hand_off(state, &user_id, &plan, original, streaming).await {
                Some(response) => Ok(response),
                None => Err(ModelRouterError::UpstreamError { status, message }.into()),
            };
        }
        Err(error) => return Err(error.into()),
    };

    let frontier_settlement = || inference_settlement::StreamSettlement {
        db: Arc::new(state.db.clone()),
        user_id: user_id.clone(),
        model: plan.from.clone(),
        pool_id: pool_id.clone(),
        charge_user: true,
        prices: prices.clone(),
        prompt_token_estimate: (chars / 4) as u64,
    };

    if !streaming {
        let metered = inference_settlement::meter_json_response(
            &state.db,
            &user_id,
            &plan.from,
            pool_id.as_deref(),
            true,
            &prices,
            chars,
            frontier,
        )
        .await?;
        if !metered.status().is_success() {
            return Ok(metered);
        }
        let (parts, body) = metered.into_parts();
        let bytes = axum::body::to_bytes(body, MAX_BUFFERED_JSON_BODY)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to buffer the response: {e}")))?;
        let refused = serde_json::from_slice::<Value>(&bytes)
            .map(|v| refusal::response_is_refusal(&v))
            .unwrap_or(false);
        if refused {
            if let Some(response) = hand_off(state, &user_id, &plan, original, false).await {
                return Ok(response);
            }
        }
        return Ok(Response::from_parts(parts, Body::from(bytes)));
    }

    if !frontier.status().is_success() {
        return Ok(inference_settlement::meter_stream_response(frontier, frontier_settlement()));
    }

    // Streaming: hold the start of the answer.
    let (parts, body) = frontier.into_parts();
    let mut stream = body.into_data_stream();
    let mut held: Vec<Bytes> = Vec::new();
    let mut hold = HoldScanner::default();
    let deadline = tokio::time::Instant::now() + STREAM_HOLD_TIMEOUT;
    let mut ended = false;
    while !hold.decided() {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                hold.push(&chunk);
                held.push(chunk);
            }
            Ok(Some(Err(error))) => {
                tracing::warn!(%error, "frontier stream error while held");
                ended = true;
                break;
            }
            Ok(None) => {
                ended = true;
                break;
            }
            Err(_) => break, // hold timeout: decide on what arrived
        }
    }

    if hold.is_refusal(ended) {
        if let Some(response) = hand_off(state, &user_id, &plan, original.clone(), true).await {
            // Bill the frontier for what it streamed (usage if it finished,
            // else the marked estimate), then drop the rest of its stream.
            drop(stream);
            let held_response = Response::new(Body::from_stream(futures_util::stream::iter(
                held.into_iter().map(Ok::<Bytes, axum::Error>),
            )));
            let metered = inference_settlement::meter_stream_response(held_response, frontier_settlement());
            tokio::spawn(async move {
                let _ = axum::body::to_bytes(metered.into_body(), MAX_BUFFERED_JSON_BODY).await;
            });
            tracing::info!(from = %plan.from, to = %plan.to, "open lane streaming handoff");
            return Ok(response);
        }
    }

    // Not a refusal (or the handoff could not run): flush what was held and
    // keep streaming the frontier answer.
    let rest = futures_util::stream::iter(held.into_iter().map(Ok::<Bytes, axum::Error>)).chain(stream);
    let rebuilt = Response::from_parts(parts, Body::from_stream(rest));
    Ok(inference_settlement::meter_stream_response(rebuilt, frontier_settlement()))
}

/// Re-run a refused request on the plan's open model. `None` = do not hand
/// off (hard-line block, moderation down, or the open model failed): the
/// caller answers with the frontier's own refusal.
async fn hand_off(
    state: &Arc<ApiState>,
    user_id: &str,
    plan: &HandoffPlan,
    original: ChatCompletionRequest,
    streaming: bool,
) -> Option<Response> {
    if hard_line_gate(state, user_id, &original).await.is_err() {
        return None;
    }
    let mut request = original;
    request.model = plan.to.clone();
    let chars = prompt_chars(&request);
    let prices = state.model_router.retail_prices(&plan.to).await.ok()?;
    let response = match state.model_router.chat_completions(request).await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::warn!(status = %response.status(), to = %plan.to, "open lane handoff upstream error");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, to = %plan.to, "open lane handoff failed");
            return None;
        }
    };
    let header_value = open_lane::handoff_header_value(&plan.from, &plan.to);

    if streaming {
        let metered = inference_settlement::meter_stream_response(
            response,
            inference_settlement::StreamSettlement {
                db: Arc::new(state.db.clone()),
                user_id: user_id.to_string(),
                model: plan.to.clone(),
                pool_id: None,
                charge_user: true,
                prices,
                prompt_token_estimate: (chars / 4) as u64,
            },
        );
        let (mut parts, body) = metered.into_parts();
        let head = futures_util::stream::once(futures_util::future::ready(Ok::<Bytes, axum::Error>(
            Bytes::from(open_lane::handoff_sse_event(&plan.from, &plan.to)),
        )));
        parts.headers.insert(open_lane::HANDOFF_HEADER, header_value);
        parts.headers.remove(header::CONTENT_LENGTH);
        return Some(Response::from_parts(parts, Body::from_stream(head.chain(body.into_data_stream()))));
    }

    let metered = inference_settlement::meter_json_response(
        &state.db, user_id, &plan.to, None, true, &prices, chars, response,
    )
    .await
    .ok()?;
    let (mut parts, body) = metered.into_parts();
    let bytes = axum::body::to_bytes(body, MAX_BUFFERED_JSON_BODY).await.ok()?;
    let mut value: Value = serde_json::from_slice(&bytes).ok()?;
    open_lane::annotate_handoff(&mut value, &plan.from, &plan.to);
    // Name the open model by its alias (the upstream may call it otherwise).
    if let Some(object) = value.as_object_mut() {
        object.insert("model".to_string(), Value::String(plan.to.clone()));
    }
    parts.headers.insert(open_lane::HANDOFF_HEADER, header_value);
    parts.headers.remove(header::CONTENT_LENGTH);
    tracing::info!(from = %plan.from, to = %plan.to, "open lane handoff");
    Some(Response::from_parts(parts, Body::from(serde_json::to_vec(&value).ok()?)))
}

/// Reads held SSE chunks: assistant text so far, tool calls, finish reason
/// and explicit refusal deltas.
#[derive(Default)]
struct HoldScanner {
    buf: Vec<u8>,
    text: String,
    tool_call: bool,
    refusal_delta: bool,
    finish: Option<String>,
    done: bool,
}

impl HoldScanner {
    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            if let Ok(line) = std::str::from_utf8(&line) {
                self.scan_line(line);
            }
        }
    }

    fn scan_line(&mut self, line: &str) {
        let Some(payload) = line.trim().strip_prefix("data:") else {
            return;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            self.done = true;
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            return;
        };
        let Some(choice) = value["choices"].get(0) else {
            return;
        };
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str() {
            self.text.push_str(text);
        }
        if delta["tool_calls"].as_array().is_some_and(|c| !c.is_empty()) {
            self.tool_call = true;
        }
        if delta["refusal"].as_str().is_some_and(|r| !r.trim().is_empty()) {
            self.refusal_delta = true;
        }
        if let Some(finish) = choice["finish_reason"].as_str() {
            self.finish = Some(finish.to_string());
        }
    }

    /// Enough is in to decide: 400 characters, a tool call, a finish, or
    /// the end marker.
    fn decided(&self) -> bool {
        self.tool_call
            || self.refusal_delta
            || self.finish.is_some()
            || self.done
            || self.text.chars().count() >= refusal::STREAM_HOLD_CHARS
    }

    fn is_refusal(&self, ended: bool) -> bool {
        if self.tool_call {
            return false;
        }
        if self.refusal_delta || self.finish.as_deref().is_some_and(refusal::is_refusal_finish_reason) {
            return true;
        }
        let complete = ended || self.done || self.finish.is_some();
        refusal::text_is_refusal(&self.text, complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(content: &str, finish: Option<&str>) -> String {
        let value = json!({
            "choices": [{ "index": 0, "delta": { "content": content }, "finish_reason": finish }]
        });
        format!("data: {value}\n\n")
    }

    #[test]
    fn hold_scanner_flags_a_streamed_refusal() {
        let mut hold = HoldScanner::default();
        hold.push(chunk("I'm sorry, but I can't ", None).as_bytes());
        assert!(!hold.decided());
        hold.push(chunk("help with that.", Some("stop")).as_bytes());
        assert!(hold.decided());
        assert!(hold.is_refusal(false));
    }

    #[test]
    fn hold_scanner_passes_an_answer_after_400_chars() {
        let mut hold = HoldScanner::default();
        let long = "Sure. ".repeat(80);
        // Split across two network chunks mid-line.
        let event = chunk(&long, None);
        let (a, b) = event.split_at(event.len() / 2);
        hold.push(a.as_bytes());
        assert!(!hold.decided());
        hold.push(b.as_bytes());
        assert!(hold.decided());
        assert!(!hold.is_refusal(false));
    }

    #[test]
    fn hold_scanner_never_hands_off_tool_calls() {
        let mut hold = HoldScanner::default();
        let event = json!({ "choices": [{ "delta": { "content": "I can't do that without", "tool_calls": [{ "index": 0 }] } }] });
        hold.push(format!("data: {event}\n\n").as_bytes());
        assert!(hold.decided());
        assert!(!hold.is_refusal(false));
    }

    #[test]
    fn hold_scanner_content_filter_finish_is_a_refusal() {
        let mut hold = HoldScanner::default();
        hold.push(chunk("", Some("content_filter")).as_bytes());
        assert!(hold.is_refusal(true));
    }
}
