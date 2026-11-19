//! Linked Claude organization billing (CONTRACT-oauth-linked-billing-2026-11.md,
//! Part B, sections B3 and B4).
//!
//! A `linked/claude/<model>` request runs on Anthropic's own
//! `POST {API}/v1/chat/completions` and Anthropic bills the person's linked
//! Claude organization, not Allternit credits:
//!
//! - **Who.** Only the person's own composer send (`X-Allternit-Turn: person`,
//!   D8, the plan-model rule [`inference_settlement::person_turn_allowed`]).
//!   Apps (Sign in with Allternit), paired computers, Platform keys and the
//!   playground get 403 `plan_person_only`.
//! - **Which org.** `anthropic-user-profile-id` is read from the DB by the
//!   caller's user id ([`claude_link::linked_profile`]) and nowhere else. The
//!   operator token can bill any linked customer, so every payer-shaped field
//!   a client sends is dropped: no incoming header is forwarded at all, and
//!   `billing_policy`, `user_profile_id`, `anthropic*`, `extra_body` and
//!   `extra_headers` body fields are removed before ours are added.
//! - **Headers.** `Authorization: Bearer <operator token>`, the profile id,
//!   `anthropic-beta: user-profiles-billing-2026-10-01` and
//!   `anthropic-version` ([`claude_link::anthropic_version`]). The body adds
//!   `"billing_policy":{"type":"billing_policy","bill_to":"user_profile"}`.
//! - **Tools.** Code-execution server tools are dropped; a `tool_choice` that
//!   forces one is 400 `linked_tools_not_allowed`.
//! - **Money.** No wallet debit, no credits gate, no free-tier pool, no auto
//!   top-up. A billed request writes its usage row with
//!   `billed_to = claude_org` and cost 0 to the wallet.
//! - **Refusals** follow the B4 table, each as
//!   `{"error":{"code","message"},"fallbacks":["allternit-credits"]}`. A
//!   refusal never falls back to another payer: the client offers the credits
//!   button and the person's tap is a new request. A 401 means our operator
//!   token: drop it, exchange once more and retry once. 403
//!   `user_profile_not_billable` within 10 s of linking retries once after the
//!   rest of those 10 s. Nothing else is retried.
//! - **Streams.** An HTTP error before the first byte is a normal refusal. An
//!   SSE `error` event (or a JSON chunk carrying `error`) mid-stream: the text
//!   so far stays, then one final `data:` error event in our shape, then
//!   `data: [DONE]`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures::{Stream, StreamExt};
use serde_json::{json, Map, Value};

use crate::{
    auth::anthropic_client::{self, operator_tokens, AnthropicClient, OperatorToken, NOT_AVAILABLE_YET},
    model_router::{catalog::ModelAliasEntry, ChatCompletionRequest, ModelInfo, RetailPrices},
    services::{
        claude_link::{self, LinkState},
        inference_settlement,
    },
    ApiError, ApiState,
};

/// Model ids that bill the caller's linked Claude organization.
pub const LINKED_PREFIX: &str = "linked/claude/";
/// `anthropic-beta` on a billed request.
pub const BILLING_BETA: &str = "user-profiles-billing-2026-10-01";
/// The header naming the customer Anthropic bills.
pub const PROFILE_HEADER: &str = "anthropic-user-profile-id";
/// The one fallback every refusal offers; the person taps it (a new request).
pub const FALLBACK_CREDITS: &str = "allternit-credits";
/// `GET /v1/models` group and `billed_to` of the linked rows.
pub const MODEL_GROUP: &str = "Claude organization";
pub const MODEL_PROVIDER: &str = "claude-org";

/// 403 `user_profile_not_billable` this soon after linking retries once.
const NOT_BILLABLE_WINDOW: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Whole-request cap, streams included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_ERROR_BODY: usize = 64 * 1024;
const MAX_JSON_BODY: usize = 32 * 1024 * 1024;
/// `retry-after` on `claude_link_upstream` when Anthropic sent none.
const DEFAULT_RETRY_AFTER: &str = "5";

pub const CODE_PERSON_ONLY: &str = "plan_person_only";
pub const CODE_UNAVAILABLE: &str = "claude_link_unavailable";
pub const CODE_DISCONNECTED: &str = "claude_link_disconnected";
pub const CODE_NOT_BILLABLE: &str = "claude_link_not_billable";
pub const CODE_SPEND_LIMIT: &str = "claude_link_spend_limit";
pub const CODE_BILLING_UNAVAILABLE: &str = "claude_link_billing_unavailable";
pub const CODE_UPSTREAM: &str = "claude_link_upstream";
pub const CODE_TOOLS_NOT_ALLOWED: &str = "linked_tools_not_allowed";
/// Anthropic refused the request itself (a 4xx the B4 table does not name,
/// such as an unknown model): not a billing refusal.
pub const CODE_REJECTED: &str = "claude_link_rejected";

const PERSON_ONLY_MESSAGE: &str = "Your Claude organization only answers messages you send yourself, in chat.";
const NOT_AVAILABLE_MESSAGE: &str = "Linked billing is not available yet.";
const NOT_BILLABLE_MESSAGE: &str =
    "Your Claude organization can't be billed. It must pay Anthropic directly (not Max/Team plan credits or a cloud marketplace).";
const SPEND_LIMIT_MESSAGE: &str =
    "Your Claude organization's spend limit for Allternit is reached. Raise it on Claude's Linked platforms page.";
const BILLING_UNAVAILABLE_MESSAGE: &str = "Billing on your Claude organization is unavailable right now.";
const DISCONNECTED_MESSAGE: &str = "Your Claude organization is disconnected. Link it again.";
const UNAVAILABLE_MESSAGE: &str = "Linked billing is unavailable. Try again later.";
const UPSTREAM_MESSAGE: &str = "Claude is busy. Try again.";
const TOOLS_MESSAGE: &str = "Code execution tools can't run on your Claude organization.";

/// A `linked/claude/<model>` id.
pub fn is_linked_model(model: &str) -> bool {
    model.trim().starts_with(LINKED_PREFIX)
}

/// The Anthropic model id behind a linked id (`linked/claude/claude-sonnet-5-5`
/// → `claude-sonnet-5-5`).
fn upstream_model(model: &str) -> Option<&str> {
    model
        .trim()
        .strip_prefix(LINKED_PREFIX)
        .map(str::trim)
        .filter(|m| !m.is_empty() && !m.contains(char::is_whitespace))
}

// ─── refusals ─────────────────────────────────────────────────────────────────

/// One refusal: `{"error":{"code","message"[,"reason"]},"fallbacks":["allternit-credits"]}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub reason: Option<&'static str>,
    /// `retry-after` (seconds) on `claude_link_upstream`.
    pub retry_after: Option<String>,
}

impl Refusal {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), reason: None, retry_after: None }
    }
    pub fn person_only() -> Self {
        Self::new(StatusCode::FORBIDDEN, CODE_PERSON_ONLY, PERSON_ONLY_MESSAGE)
    }
    fn not_available_yet() -> Self {
        Self { reason: Some(NOT_AVAILABLE_YET), ..Self::new(StatusCode::FORBIDDEN, CODE_UNAVAILABLE, NOT_AVAILABLE_MESSAGE) }
    }
    fn disconnected() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, CODE_DISCONNECTED, DISCONNECTED_MESSAGE)
    }
    /// 403 for Anthropic's `permission_error`; 500 for our own bug (bad
    /// profile, operator token refused twice, exchange failed).
    fn unavailable(status: StatusCode) -> Self {
        Self::new(status, CODE_UNAVAILABLE, UNAVAILABLE_MESSAGE)
    }
    fn upstream(retry_after: Option<String>) -> Self {
        Self {
            retry_after: Some(retry_after.unwrap_or_else(|| DEFAULT_RETRY_AFTER.to_string())),
            ..Self::new(StatusCode::SERVICE_UNAVAILABLE, CODE_UPSTREAM, UPSTREAM_MESSAGE)
        }
    }
    fn tools_not_allowed() -> Self {
        Self::new(StatusCode::BAD_REQUEST, CODE_TOOLS_NOT_ALLOWED, TOOLS_MESSAGE)
    }

    pub fn body(&self) -> Value {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(reason) = self.reason {
            error["reason"] = json!(reason);
        }
        json!({ "error": error, "fallbacks": [FALLBACK_CREDITS] })
    }

    /// The last bytes of a stream that Anthropic refused part way.
    fn sse_tail(&self) -> String {
        format!("data: {}\n\ndata: [DONE]\n\n", self.body())
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.body())).into_response();
        if let Some(value) = self.retry_after.as_deref().and_then(|v| HeaderValue::from_str(v).ok()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

/// `linked_organization_*` / `user_profile_*`: the codes that say why billing refused.
fn is_billing_code(code: &str) -> bool {
    (code.starts_with("linked_organization_") || code.starts_with("user_profile_"))
        && code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The codes B4 names, for a search inside free text.
const KNOWN_BILLING_CODES: [&str; 4] = [
    "user_profile_not_billable",
    "linked_organization_spend_limit_reached",
    "linked_organization_billing_unavailable",
    "user_profile_required",
];

fn find_billing_code(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if is_billing_code(s) => Some(s.clone()),
        Value::Array(items) => items.iter().find_map(find_billing_code),
        Value::Object(map) => map.values().find_map(find_billing_code),
        _ => None,
    }
}

fn mentioned_billing_code(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => KNOWN_BILLING_CODES.iter().find(|code| s.contains(*code)).map(|c| c.to_string()),
        Value::Array(items) => items.iter().find_map(mentioned_billing_code),
        Value::Object(map) => map.values().find_map(mentioned_billing_code),
        _ => None,
    }
}

/// Anthropic's code in an error body. On chat completions `error.type` is
/// always `invalid_request_error` and the code is in `error.code`; on
/// messages it is in `error.details.error_code`. Preference: `error.code`,
/// `error.details.error_code`, `error.type`. A generic word there (such as
/// `billing_error`) loses to a `linked_organization_*` / `user_profile_*`
/// code anywhere in the error object.
fn anthropic_code(body: &Value) -> Option<String> {
    let error = body.get("error").filter(|e| e.is_object()).unwrap_or(body);
    let candidates: Vec<&str> = ["/code", "/details/error_code", "/type"]
        .iter()
        .filter_map(|p| error.pointer(p).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(code) = candidates.iter().find(|c| is_billing_code(c)) {
        return Some(code.to_string());
    }
    find_billing_code(error)
        .or_else(|| mentioned_billing_code(error))
        .or_else(|| candidates.first().map(|c| c.to_string()))
}

fn upstream_message(body: &Value) -> Option<String> {
    let text = body.pointer("/error/message").and_then(Value::as_str)?.trim();
    (!text.is_empty()).then(|| text.chars().take(300).collect())
}

/// The B4 table. `status` is `None` for an error inside a stream.
fn map_refusal(status: Option<u16>, body: &Value, retry_after: Option<String>) -> Refusal {
    let code = anthropic_code(body);
    match code.as_deref().unwrap_or("") {
        "user_profile_not_billable" => Refusal::new(StatusCode::FORBIDDEN, CODE_NOT_BILLABLE, NOT_BILLABLE_MESSAGE),
        "linked_organization_spend_limit_reached" => Refusal::new(StatusCode::PAYMENT_REQUIRED, CODE_SPEND_LIMIT, SPEND_LIMIT_MESSAGE),
        "linked_organization_billing_unavailable" => {
            Refusal::new(StatusCode::PAYMENT_REQUIRED, CODE_BILLING_UNAVAILABLE, BILLING_UNAVAILABLE_MESSAGE)
        }
        c if c.starts_with("linked_organization_") => {
            tracing::warn!(code = c, "claude linked: unknown linked organization code; reported as billing unavailable");
            Refusal::new(StatusCode::PAYMENT_REQUIRED, CODE_BILLING_UNAVAILABLE, BILLING_UNAVAILABLE_MESSAGE)
        }
        // `user_profile_required` or a malformed profile: our bug.
        c if c.starts_with("user_profile_") => {
            tracing::error!(code = c, "claude linked: Anthropic refused our user profile; this is an Allternit bug");
            Refusal::unavailable(StatusCode::INTERNAL_SERVER_ERROR)
        }
        "overloaded_error" | "rate_limit_error" | "api_error" => Refusal::upstream(retry_after),
        "authentication_error" => {
            tracing::error!("claude linked: Anthropic refused the operator token");
            Refusal::unavailable(StatusCode::INTERNAL_SERVER_ERROR)
        }
        "permission_error" => {
            // Our org is not billed directly by Anthropic, or similar: alert ops.
            tracing::error!("claude linked: permission_error with no billing code; check Allternit's Anthropic organization");
            Refusal::unavailable(StatusCode::FORBIDDEN)
        }
        other => match status {
            Some(429) | Some(500..=599) | None => Refusal::upstream(retry_after),
            Some(401) => {
                tracing::error!("claude linked: Anthropic answered 401 to the operator token");
                Refusal::unavailable(StatusCode::INTERNAL_SERVER_ERROR)
            }
            Some(402) => Refusal::new(StatusCode::PAYMENT_REQUIRED, CODE_BILLING_UNAVAILABLE, BILLING_UNAVAILABLE_MESSAGE),
            Some(403) => {
                tracing::error!(code = other, "claude linked: 403 with no billing code; check Allternit's Anthropic organization");
                Refusal::unavailable(StatusCode::FORBIDDEN)
            }
            Some(status) => {
                let message = match upstream_message(body) {
                    Some(text) => format!("Claude refused this request: {text}"),
                    None => "Claude refused this request.".to_string(),
                };
                Refusal::new(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST), CODE_REJECTED, message)
            }
        },
    }
}

// ─── the request body ─────────────────────────────────────────────────────────

/// A body field that could name a payer: never forwarded, whatever the client sends.
fn is_payer_field(key: &str) -> bool {
    // `user_profile_id`, `anthropic-user-profile-id`, `userProfileId`, `billingPolicy`…
    let key: String = key.trim().to_ascii_lowercase().chars().filter(|c| !matches!(c, '_' | '-')).collect();
    key == "billingpolicy" || key == "billto" || key.contains("userprofile") || key.starts_with("anthropic")
}

/// Anthropic's code execution server tool (`code_execution_<date>`) and
/// OpenAI's `code_interpreter`. Function tools run on the client: kept.
fn is_code_execution_tool(tool: &Value) -> bool {
    let kind = tool.get("type").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    kind.starts_with("code_execution") || kind == "code_interpreter"
}

/// What a `tool_choice` forces: a tool name or a server tool type. `None` for
/// `auto` / `none` and an unknown shape.
fn forced_tool(choice: &Value) -> Option<String> {
    if let Some(name) = choice.pointer("/function/name").or_else(|| choice.get("name")).and_then(Value::as_str) {
        return Some(name.to_string());
    }
    let kind = choice.get("type").and_then(Value::as_str)?;
    (!matches!(kind, "auto" | "none" | "any" | "required" | "function" | "tool")).then(|| kind.to_string())
}

fn forces_some_tool(choice: &Value) -> bool {
    matches!(choice.as_str(), Some("required" | "any"))
        || matches!(choice.get("type").and_then(Value::as_str), Some("required" | "any"))
}

/// Drop code execution tools; 400 when the client insists on one.
fn drop_code_execution_tools(body: &mut Map<String, Value>) -> Result<(), Refusal> {
    let Some(Value::Array(tools)) = body.get_mut("tools") else { return Ok(()) };
    let (dropped, kept): (Vec<Value>, Vec<Value>) = std::mem::take(tools).into_iter().partition(is_code_execution_tool);
    *tools = kept;
    if dropped.is_empty() {
        return Ok(());
    }
    let labels: Vec<&str> = dropped
        .iter()
        .flat_map(|t| [t.get("name"), t.get("type")])
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let choice = body.get("tool_choice").cloned().unwrap_or(Value::Null);
    if forced_tool(&choice).is_some_and(|name| labels.contains(&name.as_str())) {
        return Err(Refusal::tools_not_allowed());
    }
    let none_left = body.get("tools").and_then(Value::as_array).is_some_and(Vec::is_empty);
    if none_left {
        if forces_some_tool(&choice) {
            return Err(Refusal::tools_not_allowed());
        }
        body.remove("tools");
        body.remove("tool_choice");
    }
    Ok(())
}

/// The body sent to Anthropic: the client's, with the model stripped of the
/// linked prefix, payer fields replaced by ours and code execution dropped.
fn upstream_body(mut request: ChatCompletionRequest, model: &str) -> Result<Value, Refusal> {
    request.strip_openai_only_fields("anthropic");
    let streaming = request.is_streaming();
    let Ok(Value::Object(mut body)) = serde_json::to_value(&request) else {
        return Err(Refusal::unavailable(StatusCode::INTERNAL_SERVER_ERROR));
    };
    body.retain(|key, _| !is_payer_field(key));
    // SDK pass-through containers a proxy might merge: never ours to forward.
    body.remove("extra_body");
    body.remove("extra_headers");
    if let Some(Value::Object(metadata)) = body.get_mut("metadata") {
        metadata.retain(|key, _| !is_payer_field(key));
    }
    drop_code_execution_tools(&mut body)?;
    body.insert("model".to_string(), Value::String(model.to_string()));
    if streaming {
        body.entry("stream_options".to_string()).or_insert_with(|| json!({ "include_usage": true }));
    }
    body.insert("billing_policy".to_string(), json!({ "type": "billing_policy", "bill_to": "user_profile" }));
    Ok(Value::Object(body))
}

// ─── the call ─────────────────────────────────────────────────────────────────

fn http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default()
        })
        .clone()
}

/// One call to Anthropic. Every header is set here; nothing from the
/// incoming request is forwarded.
async fn send(
    client: &AnthropicClient,
    token: &OperatorToken,
    user_profile_id: &str,
    body: &Value,
    streaming: bool,
) -> Result<reqwest::Response, reqwest::Error> {
    let mut request = http()
        .post(format!("{}/v1/chat/completions", client.api_base()))
        .bearer_auth(token.bearer())
        .header(PROFILE_HEADER, user_profile_id)
        .header("anthropic-beta", BILLING_BETA)
        .header("anthropic-version", claude_link::anthropic_version())
        .json(body);
    if streaming {
        request = request.header("accept", "text/event-stream");
    }
    request.send().await
}

async fn read_capped(response: reqwest::Response, cap: usize) -> Option<Bytes> {
    let bytes = response.bytes().await.ok()?;
    (bytes.len() <= cap).then_some(bytes)
}

fn retry_after_of(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
}

/// `POST /v1/chat/completions` for a `linked/claude/<model>` id. Runs before
/// plans, BYOK, the funding reroute and every credits gate, and never hands
/// the request to another payer.
pub(super) async fn chat_completions(
    state: &Arc<ApiState>,
    user_id: &str,
    request: ChatCompletionRequest,
    attribution: inference_settlement::InferenceAttribution,
) -> Result<Response, ApiError> {
    if !inference_settlement::person_turn_allowed(&attribution) {
        return Ok(Refusal::person_only().into_response());
    }
    let Ok(client) = AnthropicClient::from_env() else {
        return Ok(Refusal::not_available_yet().into_response());
    };
    let Some(model) = upstream_model(&request.model).map(str::to_string) else {
        return Err(ApiError::BadRequest(format!("{} needs a Claude model after {LINKED_PREFIX}", request.model.trim())));
    };
    // The payer, from the DB by the caller's user id. Never from the request.
    let link = match claude_link::linked_profile(&state.db, user_id).await.map_err(ApiError::DatabaseError)? {
        Some(link) if link.state != LinkState::Disconnected => link,
        _ => return Ok(Refusal::disconnected().into_response()),
    };
    if let Some(line) = crate::services::hard_line::standard_check(&request.messages) {
        crate::services::open_lane::record_block(&state.db, user_id, line.category()).await;
        return Ok(crate::services::open_lane::hard_line_response(line.category()));
    }

    let alias = request.model.trim().to_string();
    let streaming = request.is_streaming();
    let prompt_chars: usize = request.messages.iter().map(|m| m.content_chars()).sum();
    let body = match upstream_body(request, &model) {
        Ok(body) => body,
        Err(refusal) => return Ok(refusal.into_response()),
    };

    let tokens = operator_tokens();
    let mut token = match tokens.get(&client).await {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "claude linked: no operator token");
            return Ok(Refusal::unavailable(StatusCode::INTERNAL_SERVER_ERROR).into_response());
        }
    };
    let mut reexchanged = false;
    let mut waited_for_billable = false;
    let response = loop {
        let response = match send(&client, &token, &link.user_profile_id, &body, streaming).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error.without_url(), "claude linked: Anthropic did not answer");
                return Ok(Refusal::upstream(None).into_response());
            }
        };
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            break response;
        }
        // A 401 is our operator token (B4): drop it, exchange once more, retry once.
        if status == 401 && !reexchanged {
            reexchanged = true;
            tokens.invalidate(&token).await;
            token = match tokens.get(&client).await {
                Ok(token) => token,
                Err(error) => {
                    tracing::error!(%error, "claude linked: operator token re-exchange failed");
                    return Ok(Refusal::unavailable(StatusCode::INTERNAL_SERVER_ERROR).into_response());
                }
            };
            continue;
        }
        let retry_after = retry_after_of(&response);
        let error_body = read_capped(response, MAX_ERROR_BODY)
            .await
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or(Value::Null);
        let upstream_code = anthropic_code(&error_body);
        // A new connection can take a few seconds to become billable.
        if status == 403 && !waited_for_billable && upstream_code.as_deref() == Some("user_profile_not_billable") {
            let age = (chrono::Utc::now() - link.linked_at).to_std().unwrap_or(Duration::ZERO);
            if age < NOT_BILLABLE_WINDOW {
                waited_for_billable = true;
                tokio::time::sleep(NOT_BILLABLE_WINDOW - age).await;
                continue;
            }
        }
        let refusal = if status == 401 {
            tracing::error!("claude linked: Anthropic refused a freshly exchanged operator token");
            Refusal::unavailable(StatusCode::INTERNAL_SERVER_ERROR)
        } else {
            map_refusal(Some(status), &error_body, retry_after)
        };
        if refusal.code == CODE_SPEND_LIMIT {
            claude_link::note_spend_limit(user_id, true);
        }
        tracing::warn!(
            user_id,
            request_id = attribution.request_id.as_deref().unwrap_or(""),
            upstream_status = status,
            anthropic_code = upstream_code.as_deref().unwrap_or("-"),
            code = refusal.code,
            "claude linked: request refused; billed to nobody, no fallback"
        );
        return Ok(refusal.into_response());
    };

    claude_link::note_spend_limit(user_id, false);
    tracing::info!(
        user_id,
        request_id = attribution.request_id.as_deref().unwrap_or(""),
        streaming,
        billed_to = inference_settlement::BILLED_TO_CLAUDE_ORG,
        "claude linked: chat completion dispatched"
    );

    // Anthropic bills the org: the usage row has the tokens, the wallet pays 0.
    let mut attribution = attribution;
    attribution.billed_to = Some(inference_settlement::BILLED_TO_CLAUDE_ORG);
    let zero = RetailPrices { prompt_per_1m: 0.0, completion_per_1m: 0.0, wholesale_prompt_per_1m: None, wholesale_completion_per_1m: None };

    if streaming {
        let guarded = guard_stream(Box::pin(response.bytes_stream()), user_id.to_string());
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(guarded))
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        return Ok(inference_settlement::meter_stream_response(
            response,
            inference_settlement::StreamSettlement {
                db: Arc::new(state.db.clone()),
                user_id: user_id.to_string(),
                model: alias,
                pool_id: None,
                charge_user: false,
                prices: zero,
                prompt_token_estimate: (prompt_chars / 4) as u64,
                attribution,
            },
        ));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let Some(bytes) = read_capped(response, MAX_JSON_BODY).await else {
        tracing::warn!("claude linked: Anthropic's reply did not arrive whole");
        return Ok(Refusal::upstream(None).into_response());
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    inference_settlement::meter_json_response(&state.db, user_id, &alias, None, false, &zero, prompt_chars, response, &attribution).await
}

// ─── streams ──────────────────────────────────────────────────────────────────

/// End of the first SSE event in `buf` (after its blank line).
fn event_end(buf: &[u8]) -> Option<usize> {
    let find = |needle: &[u8]| buf.windows(needle.len()).position(|w| w == needle).map(|i| i + needle.len());
    match (find(b"\n\n"), find(b"\r\n\r\n")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The refusal an SSE event carries: an `event: error` line, or a `data:`
/// JSON object with `error` (or `"type":"error"`). `None` for every other event.
fn event_refusal(event: &[u8]) -> Option<Refusal> {
    let text = std::str::from_utf8(event).ok()?;
    let mut error_event = false;
    let mut data = String::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix("event:") {
            error_event |= name.trim() == "error";
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    let parsed = serde_json::from_str::<Value>(data.trim()).ok();
    let error_chunk = parsed.as_ref().is_some_and(|v| {
        v.get("error").is_some_and(|e| !e.is_null()) || v.get("type").and_then(Value::as_str) == Some("error")
    });
    if !error_event && !error_chunk {
        return None;
    }
    Some(map_refusal(None, parsed.as_ref().unwrap_or(&Value::Null), None))
}

/// Forward Anthropic's SSE event by event. An error event ends the stream
/// with our refusal event; the text before it stays. Never a switch of payer.
fn guard_stream<S>(upstream: S, user_id: String) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static
where
    S: Stream<Item = reqwest::Result<Bytes>> + Send + Unpin + 'static,
{
    struct Guard<S> {
        upstream: S,
        buf: Vec<u8>,
        done: bool,
        user_id: String,
    }
    fn refused<S>(mut guard: Guard<S>, refusal: Refusal) -> Option<(Result<Bytes, std::io::Error>, Guard<S>)> {
        if refusal.code == CODE_SPEND_LIMIT {
            claude_link::note_spend_limit(&guard.user_id, true);
        }
        tracing::warn!(user_id = %guard.user_id, code = refusal.code, "claude linked: refused mid-stream; partial text kept, no fallback");
        guard.done = true;
        Some((Ok(Bytes::from(refusal.sse_tail())), guard))
    }
    futures::stream::unfold(Guard { upstream, buf: Vec::new(), done: false, user_id }, |mut guard| async move {
        loop {
            if guard.done {
                return None;
            }
            if let Some(end) = event_end(&guard.buf) {
                let event: Vec<u8> = guard.buf.drain(..end).collect();
                return match event_refusal(&event) {
                    Some(refusal) => refused(guard, refusal),
                    None => Some((Ok(Bytes::from(event)), guard)),
                };
            }
            let next = guard.upstream.next().await;
            match next {
                Some(Ok(chunk)) => guard.buf.extend_from_slice(&chunk),
                Some(Err(error)) => {
                    tracing::warn!(error = %error.without_url(), "claude linked: stream broke");
                    return refused(guard, Refusal::upstream(None));
                }
                None => {
                    guard.done = true;
                    let rest = std::mem::take(&mut guard.buf);
                    if rest.is_empty() {
                        return None;
                    }
                    return match event_refusal(&rest) {
                        Some(refusal) => refused(guard, refusal),
                        None => Some((Ok(Bytes::from(rest)), guard)),
                    };
                }
            }
        }
    })
}

// ─── the model list ───────────────────────────────────────────────────────────

/// The Anthropic id of a catalog entry served by Anthropic models
/// (`anthropic/claude-sonnet-5.5` → `claude-sonnet-5-5`).
fn anthropic_id(entry: &ModelAliasEntry) -> Option<String> {
    let id = entry.upstream_id.strip_prefix("anthropic/")?;
    (!id.is_empty()).then(|| id.replace('.', "-"))
}

/// One `linked/claude/<model>` row per Claude model in the catalog, in
/// catalog order (unlisted ones flagged `hidden`, like the catalog's own rows).
fn linked_rows(catalog: &[ModelAliasEntry]) -> Vec<ModelInfo> {
    let mut seen: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    for entry in catalog {
        let Some(id) = anthropic_id(entry) else { continue };
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        let mut extra = Map::new();
        extra.insert("name".to_string(), json!(format!("{} (your organization)", entry.name)));
        extra.insert("context_length".to_string(), json!(entry.context_length));
        extra.insert("lane".to_string(), json!("linked"));
        extra.insert("billed_to".to_string(), json!(inference_settlement::BILLED_TO_CLAUDE_ORG));
        extra.insert("group".to_string(), json!(MODEL_GROUP));
        extra.insert("default".to_string(), json!(false));
        if !entry.listed {
            extra.insert("hidden".to_string(), json!(true));
        }
        rows.push(ModelInfo {
            id: format!("{LINKED_PREFIX}{id}"),
            object: "model".to_string(),
            created: entry.created,
            owned_by: "anthropic".to_string(),
            upstream_id: Some(id),
            provider: Some(MODEL_PROVIDER.to_string()),
            aliases: None,
            extra,
        });
    }
    rows
}

/// The linked rows for this caller of `GET /v1/models`: only a signed-in
/// person (session or user API key with `inference`) whose Claude
/// organization is linked, while the feature is on. Apps, paired computers
/// and anonymous callers get none (they could not use them, D8). Any lookup
/// failure lists none.
pub(super) async fn models_for_caller(state: &ApiState, headers: &HeaderMap) -> Vec<ModelInfo> {
    if !anthropic_client::is_configured() || headers.get(header::AUTHORIZATION).is_none() {
        return Vec::new();
    }
    if crate::routes::runtime_pairing::device_token_from_headers(headers).is_some() {
        return Vec::new();
    }
    // OAuth tokens and client secrets never resolve to the account here.
    let user = match crate::auth::resolve::resolve_user(&state.db, headers).await {
        Ok(user) if user.has_scope("inference") => user,
        _ => return Vec::new(),
    };
    match claude_link::linked_profile(&state.db, &user.id).await {
        Ok(Some(link)) if link.state != LinkState::Disconnected => linked_rows(state.model_router.catalog().entries()),
        Ok(_) => Vec::new(),
        Err(error) => {
            tracing::debug!(%error, "claude linked: link lookup for the model list failed; listing none");
            Vec::new()
        }
    }
}

#[cfg(test)]
#[path = "anthropic_linked_tests.rs"]
mod tests;
