//! Items → model router: the model half of `POST /v1/responses` (`model` mode)
//! and `POST /v1/messages` (Anthropic shape).
//!
//! [`run_items`] turns a transcript of Platform items (`items::history` of a
//! chained conversation plus the new input) into OpenAI chat messages, runs it on
//! the in-process [`super::ModelRouter`] and meters the tokens. It does not go
//! through the `/v1/chat/completions` HTTP handler (that one needs an
//! `AuthContext`), so it runs its own gates: project spend check
//! (`billing::spend_allowed`), the hard-line keyword check and the pool breaker.
//!
//! **The project pays** (Eoj 2026-11-13), the same way an agent turn does
//! (`conversations::meter_turn`): after the model answers, one `tokens_in` and
//! one `tokens_out` row go to `platform_usage_events`, each priced at the
//! model's list price + 15% ([`billing::token_charge_microusd`], never an own
//! key here) and counted toward the project's spend cap. The list price is the
//! provider's price the model router already knows ([`list_per_1m`] over
//! `ModelRouter::retail_prices`: live upstream or wholesale catalog price, else
//! the catalog price). Rows are idempotent per call (`{prefix}:{id}:{meter}`,
//! `ref_id` = the `resp_`/`mdl_` id) and recorded in sandbox too, as agent
//! turns are. No member's personal credits are checked or charged (no
//! `inference_usage` row), for project keys and console callers alike. A
//! stream is billed once, at its end, with the final usage (estimated from the
//! text when the provider reports none); a call that fails upstream bills
//! nothing.
//!
//! Folding rules ([`items_to_messages`]):
//! * `instructions` become a leading `system` message for this turn only;
//! * `message` items keep their role (`developer` is sent as `system`); a failed
//!   or incomplete message (a legacy failed turn) is skipped;
//! * consecutive `function_call` items fold into one assistant message with
//!   `tool_calls[]` (attached to the assistant message right before them when
//!   there is one); the tool call `id` is the stored `call_id`, verbatim;
//! * `function_call_output` → `{role:"tool", tool_call_id, content}`;
//! * hosted `tool_call` and `approval_request` items (agent mode) are not sent.
//!
//! Open-lane (`-open`) models and the user's personal BYOK keys are not used on
//! Platform keys: open models need the account-level terms gate, and a project
//! must not spend a member's personal provider key.

use std::collections::BTreeMap;

use futures::StreamExt;
use serde_json::{json, Map, Value};

use super::{ChatCompletionRequest, Message, ModelRouterError, RetailPrices};
use crate::routes::platform_v1::items::{Item, NewItem};
use crate::routes::platform_v1::usage_events::{self, UsageEvent};
use crate::routes::platform_v1::{billing, PlatformCaller, PlatformError};
use crate::ApiState;

const MAX_BUFFERED_JSON_BODY: usize = 32 * 1024 * 1024;

// ---- folding ------------------------------------------------------------------

/// The fields of an item the folding needs (stored [`Item`] or new [`NewItem`]).
#[derive(Debug, Clone, Copy)]
pub struct ItemView<'a> {
    pub kind: &'a str,
    pub role: Option<&'a str>,
    pub status: &'a str,
    pub content: &'a Value,
}

impl<'a> From<&'a Item> for ItemView<'a> {
    fn from(it: &'a Item) -> Self {
        Self { kind: &it.kind, role: it.role.as_deref(), status: &it.status, content: &it.content }
    }
}

impl<'a> From<&'a NewItem> for ItemView<'a> {
    fn from(it: &'a NewItem) -> Self {
        Self { kind: &it.kind, role: it.role.as_deref(), status: &it.status, content: &it.content }
    }
}

/// A message item's parts as chat `content`: a string when it is all text, else
/// OpenAI content parts (`text`, `image_url`). Images given by `file_id` only are
/// left out (callers refuse them up front: there is no Files API here).
fn chat_content(parts: &Value) -> Value {
    let Some(parts) = parts.as_array() else { return Value::String(String::new()) };
    let all_text = parts.iter().all(|p| matches!(p["type"].as_str(), Some("input_text" | "output_text")));
    if all_text {
        return Value::String(parts.iter().filter_map(|p| p["text"].as_str()).collect());
    }
    Value::Array(
        parts
            .iter()
            .filter_map(|p| match p["type"].as_str() {
                Some("input_text" | "output_text") => Some(json!({ "type": "text", "text": p["text"] })),
                Some("input_image") => p["image_url"].as_str().map(|url| {
                    let mut image = json!({ "url": url });
                    if let Some(d) = p["detail"].as_str() {
                        image["detail"] = json!(d);
                    }
                    json!({ "type": "image_url", "image_url": image })
                }),
                _ => None,
            })
            .collect(),
    )
}

fn message(role: &str, content: Value, extra: Map<String, Value>) -> Message {
    Message { role: role.to_string(), content, extra }
}

/// Fold items (oldest first) into chat messages; see the module docs for the rules.
pub fn items_to_messages(instructions: Option<&str>, items: &[ItemView<'_>]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();
    if let Some(i) = instructions.map(str::trim).filter(|i| !i.is_empty()) {
        out.push(Message::text("system", i));
    }
    // Whether the last pushed message may take more tool calls (it is the assistant
    // message right before, or one already holding this run of calls).
    let mut open_assistant = false;
    for it in items {
        match it.kind {
            "message" => {
                if matches!(it.status, "failed" | "incomplete") {
                    open_assistant = false;
                    continue;
                }
                let role = match it.role.unwrap_or("user") {
                    "developer" => "system",
                    r => r,
                };
                out.push(message(role, chat_content(it.content), Map::new()));
                open_assistant = role == "assistant";
            }
            "function_call" => {
                let c = it.content;
                let call = json!({
                    "id": c["call_id"],
                    "type": "function",
                    "function": { "name": c["name"], "arguments": c["arguments"].as_str().unwrap_or("{}") },
                });
                match out.last_mut().filter(|_| open_assistant) {
                    Some(m) => {
                        let calls = m.extra.entry("tool_calls").or_insert_with(|| json!([]));
                        if let Some(a) = calls.as_array_mut() {
                            a.push(call);
                        }
                        if m.content.as_str() == Some("") {
                            m.content = Value::Null;
                        }
                    }
                    None => {
                        let mut extra = Map::new();
                        extra.insert("tool_calls".into(), json!([call]));
                        out.push(message("assistant", Value::Null, extra));
                        open_assistant = true;
                    }
                }
            }
            "function_call_output" => {
                let c = it.content;
                let mut extra = Map::new();
                extra.insert("tool_call_id".into(), c["call_id"].clone());
                let content = match &c["output"] {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                out.push(message("tool", Value::String(content), extra));
                open_assistant = false;
            }
            // Hosted tool steps and approvals belong to agent turns; never sent to a model.
            _ => {}
        }
    }
    out
}

/// `call_id`s of `function_call` items with no `function_call_output` anywhere
/// in `items` (history and new input together), in order.
pub fn unanswered_calls(items: &[ItemView<'_>]) -> Vec<String> {
    let answered: std::collections::HashSet<&str> = items
        .iter()
        .filter(|i| i.kind == "function_call_output")
        .filter_map(|i| i.content["call_id"].as_str())
        .collect();
    items
        .iter()
        .filter(|i| i.kind == "function_call" && !matches!(i.status, "failed" | "incomplete"))
        .filter_map(|i| i.content["call_id"].as_str())
        .filter(|id| !answered.contains(id))
        .map(str::to_string)
        .collect()
}

/// `400 missing_function_call_output` for the first unanswered call.
pub fn missing_output(call_id: &str) -> PlatformError {
    PlatformError::invalid_request(
        "missing_function_call_output",
        format!("No function_call_output for call_id {call_id:?}: send its result in input before continuing."),
    )
    .with_param("input")
}

// ---- running ------------------------------------------------------------------

/// One client function call the model asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallOut {
    /// The provider's tool call id, verbatim (stored as `call_id`).
    pub id: String,
    pub name: String,
    /// JSON text.
    pub arguments: String,
}

/// A finished model turn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCallOut>,
    /// `stop`, `tool_calls`, `length`, … as the upstream reported it.
    pub finish_reason: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A streaming model turn, in order. Ends with exactly one `Done` or `Failed`.
#[derive(Debug, Clone, PartialEq)]
pub enum Delta {
    Text(String),
    ToolStart { index: usize, id: String, name: String },
    ToolArgs { index: usize, delta: String },
    Done(Completion),
    Failed(String),
}

pub enum RunOutput {
    Complete(Completion),
    Stream(tokio::sync::mpsc::UnboundedReceiver<Delta>),
}

/// What to run. `tools` and `tool_choice` are already in chat-completions form.
pub struct RunItems<'a> {
    pub model: &'a str,
    pub history: &'a [Item],
    pub input: &'a [NewItem],
    pub instructions: Option<&'a str>,
    pub tools: Vec<Value>,
    pub tool_choice: Option<Value>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub stream: bool,
    /// Where this call's usage is recorded.
    pub usage: UsageRef,
}

/// Responses `tools` (`{type:"function", name, description, parameters}`) or
/// Anthropic tools (`{name, description, input_schema}`) in chat form.
pub fn chat_tool(name: &str, description: Option<&str>, parameters: Option<&Value>) -> Value {
    let mut f = json!({ "name": name, "parameters": parameters.cloned().unwrap_or_else(|| json!({ "type": "object", "properties": {} })) });
    if let Some(d) = description {
        f["description"] = json!(d);
    }
    json!({ "type": "function", "function": f })
}

fn router_err(e: ModelRouterError) -> PlatformError {
    PlatformError::from(crate::ApiError::from(e))
}

/// Where a model call's usage rows go: `ref_id` = `id`, idempotency
/// `{prefix}:{id}:{meter}`.
#[derive(Debug, Clone)]
pub struct UsageRef {
    /// The response (`resp_…`) or message (`mdl_…`) id the call answers.
    pub id: String,
    /// `resp` (`POST /v1/responses`) or `mdl` (`POST /v1/messages`).
    pub prefix: &'static str,
    /// The account the turn belongs to, if any.
    pub account_id: Option<String>,
}

/// The user a hard-line block by this caller is recorded against: the
/// `api_keys.user_id` of the calling key, else the project owner. Not a payer:
/// the project pays for Platform model calls.
pub async fn billing_owner(db: &sqlx::PgPool, caller: &PlatformCaller) -> String {
    let key_user = sqlx::query_scalar::<_, String>("SELECT user_id FROM api_keys WHERE id = $1 AND project_id = $2")
        .bind(&caller.key_id)
        .bind(&caller.project_id)
        .fetch_optional(db)
        .await;
    match key_user {
        Ok(Some(user)) if !user.is_empty() => user,
        Ok(_) => caller.owner_user_id.clone(),
        Err(error) => {
            tracing::warn!(%error, "platform: key owner lookup failed; recording against the project owner");
            caller.owner_user_id.clone()
        }
    }
}

/// The provider's list price per 1M tokens (input, output) in USD: the
/// wholesale side of [`RetailPrices`] when the router knows it (live upstream
/// pricing or a wholesale catalog row), else the catalog price.
pub fn list_per_1m(prices: &RetailPrices) -> (f64, f64) {
    (
        prices.wholesale_prompt_per_1m.unwrap_or(prices.prompt_per_1m),
        prices.wholesale_completion_per_1m.unwrap_or(prices.completion_per_1m),
    )
}

/// `tokens` at `per_1m_usd` per 1M tokens, in micro-dollars, rounded up
/// (USD per 1M tokens is micro-dollars per token).
pub fn list_microusd(per_1m_usd: f64, tokens: u64) -> i64 {
    let micro = tokens as f64 * per_1m_usd.max(0.0);
    // Shave float noise (1000 × 0.1 = 100.00000000000001) before rounding up.
    (micro - 1e-6).ceil().max(0.0) as i64
}

/// Records a finished model call as project usage.
struct ProjectMeter {
    db: sqlx::PgPool,
    project_id: String,
    key_id: String,
    usage: UsageRef,
    alias: String,
    prices: RetailPrices,
    prompt_chars: usize,
}

impl ProjectMeter {
    /// `tokens_in` / `tokens_out` rows at list price + 15%. Failures are
    /// logged, never surfaced (the answer already happened).
    async fn record(&self, usage: Option<(u64, u64)>, completion_chars: usize) {
        let (prompt, completion) = match usage {
            Some(u) => u,
            None => ((self.prompt_chars / 4) as u64, (completion_chars / 4) as u64),
        };
        let (list_in, list_out) = list_per_1m(&self.prices);
        for (meter, tokens, per_1m) in [("tokens_in", prompt, list_in), ("tokens_out", completion, list_out)] {
            if tokens == 0 {
                continue;
            }
            let amount = billing::token_charge_microusd(list_microusd(per_1m, tokens), false);
            let event = UsageEvent {
                project_id: self.project_id.clone(),
                account_id: self.usage.account_id.clone(),
                key_id: Some(self.key_id.clone()),
                meter: meter.to_string(),
                quantity: tokens as f64,
                unit: Some("token".to_string()),
                ref_id: Some(self.usage.id.clone()),
                idempotency: Some(format!("{}:{}:{meter}", self.usage.prefix, self.usage.id)),
            };
            if let Err(e) = usage_events::record_usage_priced(&self.db, event, Some(amount), Some(&self.alias)).await {
                tracing::error!(id = %self.usage.id, "platform: model usage not recorded: {}", e.message);
            }
        }
    }
}

/// Fold, check, gate, dispatch, meter. Errors before dispatch are plain
/// `PlatformError`s; a stream that fails later ends with [`Delta::Failed`].
pub async fn run_items(state: &ApiState, caller: &PlatformCaller, run: RunItems<'_>) -> Result<RunOutput, PlatformError> {
    let views: Vec<ItemView<'_>> = run.history.iter().map(ItemView::from).chain(run.input.iter().map(ItemView::from)).collect();
    if let Some(id) = unanswered_calls(&views).first() {
        return Err(missing_output(id));
    }
    let messages = items_to_messages(run.instructions, &views);

    let router = &state.model_router;
    if !router.is_enabled() {
        return Err(PlatformError::service_unavailable("model_router_unavailable", "Models aren't available on this deployment."));
    }
    if router.provider_for_alias(run.model).is_none() {
        return Err(PlatformError::invalid_request("model_not_found", format!("Unknown model {:?}; GET /v1/models lists them.", run.model)).with_param("model"));
    }
    if router.is_open_lane_alias(run.model) {
        return Err(PlatformError::invalid_request("model_not_available", "Open-lane models aren't available on Platform keys.").with_param("model"));
    }

    // The project pays: its spend cap (and card) gate the call.
    billing::spend_allowed(&state.db, &caller.project_id).await?;
    if let Some(line) = crate::services::hard_line::standard_check(&messages) {
        let user_id = billing_owner(&state.db, caller).await;
        crate::services::open_lane::record_block(&state.db, &user_id, line.category()).await;
        return Err(PlatformError::invalid_request("content_policy_hard_line", "This request falls under a hard line no Allternit model serves.").with_refusal(Some(line.category().to_string()), None));
    }

    let mut request = ChatCompletionRequest {
        model: run.model.to_string(),
        messages,
        temperature: run.temperature,
        max_tokens: run.max_tokens,
        stream: run.stream.then_some(true),
        top_p: None,
        extra: Map::new(),
    };
    if !run.tools.is_empty() {
        request.extra.insert("tools".into(), Value::Array(run.tools));
    }
    if let Some(tc) = run.tool_choice {
        request.extra.insert("tool_choice".into(), tc);
    }
    if let Some(funded) = router.funded_alternative(run.model) {
        tracing::warn!(from = %run.model, to = %funded, "provider below funding floor; routing to another provider");
        request.model = funded;
    }
    let alias = request.model.clone();

    let prompt_chars: usize = request.messages.iter().map(|m| m.content_chars()).sum();
    let prices = router.retail_prices(&alias).await.map_err(router_err)?;
    let pool = match router.provider_for_alias(&alias) {
        Some(provider_id) => state.inference_pool_service.pool_for_provider(provider_id).await?,
        None => None,
    };
    if let Some(pool) = pool.as_ref() {
        state.inference_pool_service.check_pool_available(pool).await?;
    }
    let meter = ProjectMeter {
        db: state.db.clone(),
        project_id: caller.project_id.clone(),
        key_id: caller.key_id.clone(),
        usage: run.usage,
        alias,
        prices,
        prompt_chars,
    };

    let response = router.chat_completions(request).await.map_err(router_err)?;
    if !response.status().is_success() {
        return Err(PlatformError::service_unavailable("model_unavailable", "The model is unavailable right now. Pick another model."));
    }

    if !run.stream {
        let bytes = axum::body::to_bytes(response.into_body(), MAX_BUFFERED_JSON_BODY)
            .await
            .map_err(|e| PlatformError::api_error("model_failed", format!("The model's answer was cut off: {e}")))?;
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| PlatformError::api_error("model_failed", "The model sent an answer we couldn't read."))?;
        let (completion, usage) = parse_completion(&body);
        meter.record(usage, completion_chars(&completion)).await;
        return Ok(RunOutput::Complete(completion));
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut body = response.into_body().into_data_stream();
    tokio::spawn(async move {
        // Reads to the end even when the client hangs up, so the tokens are metered.
        let mut fold = ChatStreamFold::default();
        let mut cut = None;
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => {
                    for d in fold.push(&bytes) {
                        let _ = tx.send(d);
                    }
                }
                Err(e) => {
                    cut = Some(format!("the model's answer was cut off: {e}"));
                    break;
                }
            }
        }
        let (completion, usage, failure) = fold.finish();
        let _ = tx.send(match cut.or(failure) {
            // Failed upstream: nothing is billed.
            Some(f) => Delta::Failed(f),
            None => {
                meter.record(usage, completion_chars(&completion)).await;
                Delta::Done(completion)
            }
        });
    });
    Ok(RunOutput::Stream(rx))
}

fn completion_chars(c: &Completion) -> usize {
    c.text.len() + c.tool_calls.iter().map(|t| t.arguments.len()).sum::<usize>()
}

fn usage_of(v: &Value) -> Option<(u64, u64)> {
    let p = v["prompt_tokens"].as_u64();
    let c = v["completion_tokens"].as_u64();
    (p.is_some() || c.is_some()).then(|| (p.unwrap_or(0), c.unwrap_or(0)))
}

/// A non-streaming chat completion body → [`Completion`] (+ the reported usage).
pub fn parse_completion(body: &Value) -> (Completion, Option<(u64, u64)>) {
    let choice = &body["choices"][0];
    let msg = &choice["message"];
    let text = match &msg["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect(),
        _ => String::new(),
    };
    let tool_calls = msg["tool_calls"]
        .as_array()
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .map(|(i, c)| ToolCallOut {
                    id: c["id"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| fallback_call_id(i)),
                    name: c["function"]["name"].as_str().unwrap_or_default().to_string(),
                    arguments: match &c["function"]["arguments"] {
                        Value::String(s) => s.clone(),
                        Value::Null => "{}".to_string(),
                        other => other.to_string(),
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    let usage = usage_of(&body["usage"]);
    let (input_tokens, output_tokens) = usage.unwrap_or((0, 0));
    (Completion { text, tool_calls, finish_reason: choice["finish_reason"].as_str().map(str::to_string), input_tokens, output_tokens }, usage)
}

/// Only for an upstream that sends a tool call without an id (the client still
/// needs one to answer it). A provider id is always kept verbatim.
fn fallback_call_id(index: usize) -> String {
    format!("call_{}{index}", hex::encode(rand::random::<[u8; 8]>()))
}

/// Streaming chat-completions SSE → [`Delta`]s. Buffers bytes so a UTF-8
/// character split across chunks is never mangled.
#[derive(Default)]
pub struct ChatStreamFold {
    pending: Vec<u8>,
    text: String,
    calls: BTreeMap<usize, ToolCallOut>,
    finish_reason: Option<String>,
    usage: Option<(u64, u64)>,
    error: Option<String>,
}

impl ChatStreamFold {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Delta> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(i) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim().strip_prefix("data:").map(str::trim) else { continue };
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            self.chunk(&v, &mut out);
        }
        out
    }

    fn chunk(&mut self, v: &Value, out: &mut Vec<Delta>) {
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            self.error = Some(e["message"].as_str().unwrap_or("the model failed").to_string());
            return;
        }
        if let Some(u) = usage_of(&v["usage"]) {
            self.usage = Some(u);
        }
        let choice = &v["choices"][0];
        if let Some(r) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(r.to_string());
        }
        let delta = &choice["delta"];
        if let Some(t) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            self.text.push_str(t);
            out.push(Delta::Text(t.to_string()));
        }
        for (pos, c) in delta["tool_calls"].as_array().into_iter().flatten().enumerate() {
            let index = c["index"].as_u64().map(|i| i as usize).unwrap_or(pos);
            if !self.calls.contains_key(&index) {
                let id = c["id"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| fallback_call_id(index));
                let name = c["function"]["name"].as_str().unwrap_or_default().to_string();
                self.calls.insert(index, ToolCallOut { id: id.clone(), name: name.clone(), arguments: String::new() });
                out.push(Delta::ToolStart { index, id, name });
            } else if let (Some(name), Some(call)) = (c["function"]["name"].as_str(), self.calls.get_mut(&index)) {
                if call.name.is_empty() {
                    call.name = name.to_string();
                }
            }
            if let Some(a) = c["function"]["arguments"].as_str().filter(|a| !a.is_empty()) {
                if let Some(call) = self.calls.get_mut(&index) {
                    call.arguments.push_str(a);
                }
                out.push(Delta::ToolArgs { index, delta: a.to_string() });
            }
        }
    }

    /// The whole turn, the reported usage, and the stream's own error if it sent one.
    pub fn finish(self) -> (Completion, Option<(u64, u64)>, Option<String>) {
        let (input_tokens, output_tokens) = self.usage.unwrap_or((0, 0));
        let tool_calls = self
            .calls
            .into_values()
            .map(|mut c| {
                if c.arguments.is_empty() {
                    c.arguments = "{}".into();
                }
                c
            })
            .collect();
        (Completion { text: self.text, tool_calls, finish_reason: self.finish_reason, input_tokens, output_tokens }, self.usage, self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn views(items: &[NewItem]) -> Vec<ItemView<'_>> {
        items.iter().map(ItemView::from).collect()
    }

    #[test]
    fn consecutive_function_calls_fold_into_one_assistant_message() {
        let items = vec![
            NewItem::user_text("Weather in Paris and Rome?"),
            NewItem::function_call("call_a", "weather", r#"{"city":"Paris"}"#),
            NewItem::function_call("call_b", "weather", r#"{"city":"Rome"}"#),
            NewItem::function_call_output("call_a", "18C"),
            NewItem::function_call_output("call_b", "24C"),
        ];
        let m = items_to_messages(Some("Be brief."), &views(&items));
        let roles: Vec<&str> = m.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["system", "user", "assistant", "tool", "tool"]);
        assert_eq!(m[0].content, json!("Be brief."));
        assert_eq!(m[2].content, Value::Null);
        let calls = m[2].extra["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], json!({ "id": "call_a", "type": "function", "function": { "name": "weather", "arguments": "{\"city\":\"Paris\"}" } }));
        assert_eq!(calls[1]["id"], "call_b", "call_id is sent verbatim");
        assert_eq!((m[3].extra["tool_call_id"].as_str(), m[3].content.as_str()), (Some("call_a"), Some("18C")));
        assert!(unanswered_calls(&views(&items)).is_empty());
    }

    #[test]
    fn calls_attach_to_the_assistant_text_before_them_and_failed_messages_are_skipped() {
        let items = vec![
            NewItem::user_text("old"),
            NewItem::assistant_text("half an answer").failed("the agent's turn failed"),
            NewItem { role: Some("developer".into()), ..NewItem::user_text("Use metric.") },
            NewItem::user_text("Look it up"),
            NewItem::assistant_text("Checking."),
            NewItem::function_call("call_1", "lookup", "{}"),
        ];
        let m = items_to_messages(None, &views(&items));
        let roles: Vec<&str> = m.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "system", "user", "assistant"], "failed message skipped, developer sent as system");
        assert_eq!(m[3].content, json!("Checking."));
        assert_eq!(m[3].extra["tool_calls"][0]["id"], "call_1");
        assert_eq!(unanswered_calls(&views(&items)), vec!["call_1".to_string()]);
        let e = missing_output("call_1");
        assert_eq!((e.status.as_u16(), e.code.as_str()), (400, "missing_function_call_output"));
    }

    #[test]
    fn images_become_image_url_parts_and_agent_items_are_dropped() {
        let img = crate::routes::platform_v1::items::parse_input(&json!([{ "role": "user", "content": [{ "type": "input_text", "text": "What is it?" }, { "type": "input_image", "image_url": "https://x/y.png", "detail": "low" }] }])).unwrap();
        let mut items = img;
        items.push(NewItem::tool_call("web_search", json!({ "q": "x" }), json!("r"), "completed", None));
        items.push(NewItem::approval_request("apr_1", json!({})));
        let m = items_to_messages(None, &views(&items));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].content, json!([{ "type": "text", "text": "What is it?" }, { "type": "image_url", "image_url": { "url": "https://x/y.png", "detail": "low" } }]));
    }

    #[test]
    fn stream_fold_accumulates_tool_arguments_by_index() {
        let mut f = ChatStreamFold::default();
        let mut out = f.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi \"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"function\":{\"name\":\"f\",\"arguments\":\"{\\\"a\\\"\"}}]}}]}\n");
        out.extend(f.push(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\":1}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n"));
        out.extend(f.push(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\ndata: [DONE]\n"));
        assert_eq!(out, vec![
            Delta::Text("Hi ".into()),
            Delta::ToolStart { index: 0, id: "call_9".into(), name: "f".into() },
            Delta::ToolArgs { index: 0, delta: "{\"a\"".into() },
            Delta::ToolArgs { index: 0, delta: ":1}".into() },
        ]);
        let (c, usage, err) = f.finish();
        assert_eq!(c.tool_calls, vec![ToolCallOut { id: "call_9".into(), name: "f".into(), arguments: "{\"a\":1}".into() }]);
        assert_eq!((c.text.as_str(), c.finish_reason.as_deref(), usage, err), ("Hi ", Some("tool_calls"), Some((7, 3)), None));
    }
}
