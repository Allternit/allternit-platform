//! Anthropic Messages API adapter for chats on a person's own Anthropic key.
//!
//! Clients speak OpenAI chat completions; Anthropic speaks the Messages API.
//! This module translates both ways:
//! - request: `system`/`developer` messages → top-level `system`; user and
//!   assistant text and images; assistant `tool_calls` → `tool_use` blocks;
//!   `tool` messages → `tool_result` blocks; `tools`, `tool_choice`,
//!   `parallel_tool_calls`, `stop`, `max_tokens`, `temperature`, `top_p`.
//! - response (JSON): text, `tool_use` → `tool_calls`, thinking →
//!   `reasoning_content`, `stop_reason` → `finish_reason`, usage.
//! - stream (SSE): `message_start`, `content_block_start`,
//!   `content_block_delta` (`text_delta`, `input_json_delta`,
//!   `thinking_delta`), `message_delta`, `message_stop`, `error` →
//!   `chat.completion.chunk` events, a final usage chunk, and `[DONE]`.
//!
//! Newer Claude models refuse sampling parameters and forced tool choice
//! with a 400. When Anthropic names one of those, the request is sent once
//! more without it (sampling dropped; `tool_choice` set to `auto`).
//!
//! Thinking blocks are not carried back on later turns: the chat-completions
//! format has no place for their signature.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    http::{header, Response, StatusCode},
};
use futures::StreamExt;
use serde_json::{json, Map, Value};

use super::{ChatCompletionRequest, ModelRouterError};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
/// Used when the client sets no `max_tokens` (the Messages API requires one).
const DEFAULT_MAX_TOKENS: u64 = 16_000;

pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self { base_url: DEFAULT_BASE_URL.to_string(), api_key }
    }

    /// Tests only: a local fake Anthropic.
    #[cfg(test)]
    pub(crate) fn with_base_url(base_url: impl Into<String>, api_key: String) -> Self {
        Self { base_url: base_url.into(), api_key }
    }

    pub async fn chat_completions(&self, request: ChatCompletionRequest) -> Result<Response<Body>, ModelRouterError> {
        let streaming = request.is_streaming();
        let mut body = to_messages_request(&request)?;
        let mut response = self.send(&body, streaming).await?;
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            let message = response.text().await.unwrap_or_default();
            if !relax(&mut body, &message) {
                return Err(ModelRouterError::UpstreamError { status: 400, message });
            }
            response = self.send(&body, streaming).await?;
        }
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_else(|_| "unknown error".to_string());
            return Err(ModelRouterError::UpstreamError { status: status.as_u16(), message });
        }
        let model = body["model"].as_str().unwrap_or_default().to_string();
        if streaming {
            Ok(stream_response(response, model))
        } else {
            let value: Value = response
                .json()
                .await
                .map_err(|e| ModelRouterError::UpstreamRequestFailed(format!("invalid JSON: {e}")))?;
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(from_messages_response(&value).to_string()))
                .map_err(|e| ModelRouterError::Internal(e.to_string()))
        }
    }

    async fn send(&self, body: &Value, streaming: bool) -> Result<reqwest::Response, ModelRouterError> {
        let mut request = crate::net_client::client()
            .post(format!("{}/v1/messages", self.base_url.trim_end_matches('/')))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(body)
            .timeout(Duration::from_secs(if streaming { 600 } else { 300 }));
        if streaming {
            request = request.header(reqwest::header::ACCEPT, "text/event-stream");
        }
        request.send().await.map_err(|e| ModelRouterError::UpstreamRequestFailed(e.to_string()))
    }
}

/// Drop what Anthropic's 400 names (sampling parameters, a forced tool
/// choice). True when something changed and the request is worth resending.
fn relax(body: &mut Value, message: &str) -> bool {
    let Some(object) = body.as_object_mut() else { return false };
    let mut changed = false;
    for field in ["temperature", "top_p", "top_k"] {
        if message.contains(field) && object.remove(field).is_some() {
            changed = true;
        }
    }
    if message.contains("tool_choice") {
        if let Some(choice) = object.get_mut("tool_choice") {
            if choice["type"] == "any" || choice["type"] == "tool" {
                let disable = choice.get("disable_parallel_tool_use").cloned();
                let mut auto = json!({ "type": "auto" });
                if let Some(disable) = disable {
                    auto["disable_parallel_tool_use"] = disable;
                }
                *choice = auto;
                changed = true;
            }
        }
    }
    changed
}

// ---------------------------------------------------------------- request

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// OpenAI content (string or parts) → Anthropic content blocks.
fn blocks_of(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) if !text.is_empty() => vec![json!({ "type": "text", "text": text })],
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part["type"].as_str() {
                Some("text") => part["text"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(|text| json!({ "type": "text", "text": text })),
                Some("image_url") => {
                    let url = part["image_url"]["url"].as_str().or_else(|| part["image_url"].as_str())?;
                    Some(match url.strip_prefix("data:").and_then(|rest| rest.split_once(";base64,")) {
                        Some((media_type, data)) => json!({
                            "type": "image",
                            "source": { "type": "base64", "media_type": media_type, "data": data },
                        }),
                        None => json!({ "type": "image", "source": { "type": "url", "url": url } }),
                    })
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Append blocks as `role`, merging into the previous message when it has
/// the same role (Anthropic alternates user and assistant).
fn push(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last["role"] == role {
            if let Some(content) = last["content"].as_array_mut() {
                content.extend(blocks);
                return;
            }
        }
    }
    messages.push(json!({ "role": role, "content": blocks }));
}

/// A chat-completions request → a Messages API body. `request.model` is the
/// Anthropic model id.
pub fn to_messages_request(request: &ChatCompletionRequest) -> Result<Value, ModelRouterError> {
    let mut system: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message.role.as_str() {
            "system" | "developer" => {
                let text = text_of(&message.content);
                if !text.trim().is_empty() {
                    system.push(text);
                }
            }
            "user" => push(&mut messages, "user", blocks_of(&message.content)),
            "assistant" => {
                let mut blocks = blocks_of(&message.content);
                for call in message.extra.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    let arguments = call["function"]["arguments"].as_str().unwrap_or("");
                    let input: Value = if arguments.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
                    };
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call["id"].as_str().unwrap_or_default(),
                        "name": call["function"]["name"].as_str().unwrap_or_default(),
                        "input": if input.is_object() { input } else { json!({ "value": input }) },
                    }));
                }
                push(&mut messages, "assistant", blocks);
            }
            "tool" => {
                let id = message.extra.get("tool_call_id").and_then(Value::as_str).unwrap_or_default();
                push(
                    &mut messages,
                    "user",
                    vec![json!({ "type": "tool_result", "tool_use_id": id, "content": text_of(&message.content) })],
                );
            }
            other => {
                return Err(ModelRouterError::InvalidRequest(format!("Unsupported message role: {other}")));
            }
        }
    }
    if messages.is_empty() {
        return Err(ModelRouterError::InvalidRequest("The request has no user message.".to_string()));
    }

    let extra = &request.extra;
    let max_tokens = request
        .max_tokens
        .map(u64::from)
        .or_else(|| extra.get("max_completion_tokens").and_then(Value::as_u64))
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let mut body = Map::new();
    body.insert("model".into(), json!(request.model));
    body.insert("max_tokens".into(), json!(max_tokens));
    body.insert("messages".into(), Value::Array(messages));
    if !system.is_empty() {
        body.insert("system".into(), json!(system.join("\n\n")));
    }
    if request.is_streaming() {
        body.insert("stream".into(), json!(true));
    }
    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature.clamp(0.0, 1.0)));
    }
    if let Some(top_p) = request.top_p {
        body.insert("top_p".into(), json!(top_p));
    }
    match extra.get("stop") {
        Some(Value::String(stop)) => {
            body.insert("stop_sequences".into(), json!([stop]));
        }
        Some(Value::Array(stops)) if !stops.is_empty() => {
            body.insert("stop_sequences".into(), Value::Array(stops.clone()));
        }
        _ => {}
    }

    let tools: Vec<Value> = extra
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tool| tool["type"] == "function" || tool.get("function").is_some())
        .map(|tool| {
            let function = &tool["function"];
            let mut out = json!({
                "name": function["name"].as_str().unwrap_or_default(),
                "input_schema": if function["parameters"].is_object() {
                    function["parameters"].clone()
                } else {
                    json!({ "type": "object", "properties": {} })
                },
            });
            if let Some(description) = function["description"].as_str() {
                out["description"] = json!(description);
            }
            out
        })
        .collect();
    let has_tools = !tools.is_empty();
    if has_tools {
        body.insert("tools".into(), Value::Array(tools));
    }
    let mut choice = match extra.get("tool_choice") {
        Some(Value::String(choice)) if choice == "none" => Some(json!({ "type": "none" })),
        Some(Value::String(choice)) if choice == "required" => Some(json!({ "type": "any" })),
        Some(Value::Object(choice)) => choice
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .map(|name| json!({ "type": "tool", "name": name })),
        _ => None,
    };
    if extra.get("parallel_tool_calls") == Some(&Value::Bool(false)) && has_tools {
        let mut with = choice.unwrap_or_else(|| json!({ "type": "auto" }));
        if with["type"] != "none" {
            with["disable_parallel_tool_use"] = json!(true);
        }
        choice = Some(with);
    }
    if let (Some(choice), true) = (choice, has_tools) {
        body.insert("tool_choice".into(), choice);
    }
    Ok(Value::Object(body))
}

// ---------------------------------------------------------------- response

fn finish_reason(stop_reason: Option<&str>) -> Option<&'static str> {
    stop_reason.map(|reason| match reason {
        "max_tokens" | "model_context_window_exceeded" => "length",
        "tool_use" => "tool_calls",
        "refusal" => "content_filter",
        _ => "stop",
    })
}

/// Anthropic usage → OpenAI usage. Cached prompt tokens count as prompt.
fn usage_of(usage: &Value) -> (u64, u64) {
    let prompt = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
        .iter()
        .filter_map(|field| usage[*field].as_u64())
        .sum();
    (prompt, usage["output_tokens"].as_u64().unwrap_or(0))
}

fn usage_json(prompt: u64, completion: u64) -> Value {
    json!({ "prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A Messages API response → a `chat.completion` body.
pub fn from_messages_response(value: &Value) -> Value {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    for block in value["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
            Some("thinking") => reasoning.push_str(block["thinking"].as_str().unwrap_or_default()),
            Some("tool_use") => tool_calls.push(json!({
                "id": block["id"],
                "type": "function",
                "function": { "name": block["name"], "arguments": block["input"].to_string() },
            })),
            _ => {}
        }
    }
    let mut message = json!({ "role": "assistant", "content": if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { json!(text) } });
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    let (prompt, completion) = usage_of(&value["usage"]);
    json!({
        "id": value["id"].as_str().unwrap_or("msg"),
        "object": "chat.completion",
        "created": now(),
        "model": value["model"],
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason(value["stop_reason"].as_str()),
        }],
        "usage": usage_json(prompt, completion),
    })
}

// ---------------------------------------------------------------- stream

/// Turns Anthropic SSE bytes into chat-completion SSE text.
pub struct StreamTranslator {
    buffer: String,
    id: String,
    model: String,
    created: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    /// Anthropic content block index → OpenAI tool call index.
    tool_index: Vec<(u64, usize)>,
    finished: bool,
}

impl StreamTranslator {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            buffer: String::new(),
            id: "chatcmpl".to_string(),
            model: model.into(),
            created: now(),
            prompt_tokens: 0,
            completion_tokens: 0,
            tool_index: Vec::new(),
            finished: false,
        }
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> String {
        let chunk = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        });
        format!("data: {chunk}\n\n")
    }

    /// The usage chunk and `[DONE]`, once.
    fn finish(&mut self) -> String {
        if self.finished {
            return String::new();
        }
        self.finished = true;
        let usage = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [],
            "usage": usage_json(self.prompt_tokens, self.completion_tokens),
        });
        format!("data: {usage}\n\ndata: [DONE]\n\n")
    }

    /// Feed upstream bytes; returns the SSE text to send on.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.buffer.push_str(&String::from_utf8_lossy(bytes).replace("\r\n", "\n"));
        let mut out = String::new();
        while let Some(end) = self.buffer.find("\n\n") {
            let event: String = self.buffer.drain(..end + 2).collect();
            let data: String = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            if let Ok(value) = serde_json::from_str::<Value>(&data) {
                out.push_str(&self.event(&value));
            }
        }
        out
    }

    /// End of the upstream body: close the stream if Anthropic did not.
    pub fn end(&mut self) -> String {
        self.finish()
    }

    fn event(&mut self, value: &Value) -> String {
        match value["type"].as_str() {
            Some("message_start") => {
                let message = &value["message"];
                if let Some(id) = message["id"].as_str() {
                    self.id = id.to_string();
                }
                if let Some(model) = message["model"].as_str() {
                    self.model = model.to_string();
                }
                let (prompt, completion) = usage_of(&message["usage"]);
                self.prompt_tokens = prompt;
                self.completion_tokens = completion;
                self.chunk(json!({ "role": "assistant", "content": "" }), None)
            }
            Some("content_block_start") => {
                let block = &value["content_block"];
                let index = value["index"].as_u64().unwrap_or(0);
                match block["type"].as_str() {
                    Some("tool_use") => {
                        let call = self.tool_index.len();
                        self.tool_index.push((index, call));
                        self.chunk(
                            json!({ "tool_calls": [{
                                "index": call,
                                "id": block["id"],
                                "type": "function",
                                "function": { "name": block["name"], "arguments": "" },
                            }]}),
                            None,
                        )
                    }
                    Some("text") => match block["text"].as_str().filter(|t| !t.is_empty()) {
                        Some(text) => self.chunk(json!({ "content": text }), None),
                        None => String::new(),
                    },
                    _ => String::new(),
                }
            }
            Some("content_block_delta") => {
                let delta = &value["delta"];
                let index = value["index"].as_u64().unwrap_or(0);
                match delta["type"].as_str() {
                    Some("text_delta") => self.chunk(json!({ "content": delta["text"] }), None),
                    Some("thinking_delta") => self.chunk(json!({ "reasoning_content": delta["thinking"] }), None),
                    Some("input_json_delta") => {
                        let Some(&(_, call)) = self.tool_index.iter().find(|(block, _)| *block == index) else {
                            return String::new();
                        };
                        self.chunk(
                            json!({ "tool_calls": [{ "index": call, "function": { "arguments": delta["partial_json"] } }] }),
                            None,
                        )
                    }
                    _ => String::new(),
                }
            }
            Some("message_delta") => {
                if let Some(output) = value["usage"]["output_tokens"].as_u64() {
                    self.completion_tokens = output;
                }
                if value["usage"]["input_tokens"].is_u64() {
                    let (prompt, _) = usage_of(&value["usage"]);
                    self.prompt_tokens = prompt;
                }
                match finish_reason(value["delta"]["stop_reason"].as_str()) {
                    Some(reason) => self.chunk(json!({}), Some(reason)),
                    None => String::new(),
                }
            }
            Some("message_stop") => self.finish(),
            Some("error") => {
                let error = json!({ "error": {
                    "message": value["error"]["message"].as_str().unwrap_or("The model stopped with an error."),
                    "type": value["error"]["type"].as_str().unwrap_or("api_error"),
                }});
                format!("data: {error}\n\n{}", self.finish())
            }
            _ => String::new(),
        }
    }
}

fn stream_response(response: reqwest::Response, model: String) -> Response<Body> {
    let translator = Arc::new(Mutex::new(StreamTranslator::new(model)));
    let tail = translator.clone();
    let body = response
        .bytes_stream()
        .map(move |item| {
            item.map(|bytes| Bytes::from(translator.lock().unwrap_or_else(|e| e.into_inner()).push(&bytes)))
        })
        .chain(futures::stream::once(async move {
            Ok::<Bytes, reqwest::Error>(Bytes::from(tail.lock().unwrap_or_else(|e| e.into_inner()).end()))
        }))
        .filter(|item| futures::future::ready(!matches!(item, Ok(bytes) if bytes.is_empty())));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};

    fn request(value: Value) -> ChatCompletionRequest {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn translates_a_tool_loop_into_messages() {
        let body = to_messages_request(&request(json!({
            "model": "claude-opus-5-5",
            "temperature": 0.2,
            "stream": true,
            "parallel_tool_calls": false,
            "stop": "END",
            "messages": [
                { "role": "system", "content": "Be brief." },
                { "role": "user", "content": [
                    { "type": "text", "text": "Weather in Paris?" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } },
                ]},
                { "role": "assistant", "content": null, "tool_calls": [
                    { "id": "toolu_1", "type": "function", "function": { "name": "weather", "arguments": "{\"city\":\"Paris\"}" } },
                ]},
                { "role": "tool", "tool_call_id": "toolu_1", "content": "18C" },
            ],
            "tools": [{ "type": "function", "function": {
                "name": "weather", "description": "Get weather",
                "parameters": { "type": "object", "properties": { "city": { "type": "string" } } },
            }}],
        })))
        .unwrap();
        assert_eq!(body["system"], "Be brief.");
        assert_eq!(body["max_tokens"], 16000);
        assert_eq!(body["stop_sequences"], json!(["END"]));
        assert_eq!(body["stream"], true);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3, "{messages:#?}");
        assert_eq!(messages[0]["content"][1]["source"], json!({ "type": "base64", "media_type": "image/png", "data": "AAAA" }));
        assert_eq!(messages[1]["content"][0], json!({ "type": "tool_use", "id": "toolu_1", "name": "weather", "input": { "city": "Paris" } }));
        assert_eq!(messages[2]["content"][0], json!({ "type": "tool_result", "tool_use_id": "toolu_1", "content": "18C" }));
        assert_eq!(body["tools"][0]["input_schema"]["properties"]["city"]["type"], "string");
        assert_eq!(body["tool_choice"], json!({ "type": "auto", "disable_parallel_tool_use": true }));
    }

    #[test]
    fn retry_drops_what_the_400_names() {
        let mut body = json!({ "model": "m", "temperature": 0.5, "tool_choice": { "type": "any" } });
        assert!(relax(&mut body, "temperature: not supported for this model"));
        assert!(body.get("temperature").is_none());
        assert!(relax(&mut body, "tool_choice: type \"tool\" and \"any\" are not supported for this model."));
        assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
        assert!(!relax(&mut body, "messages: roles must alternate"), "other 400s are not retried");
    }

    /// Anthropic's SSE for a streamed text turn and a streamed tool-call turn.
    const TEXT_TURN: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-5-5\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":3,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: ping\n",
        "data: {\"type\":\"ping\"}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    const TOOL_TURN: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_02\",\"model\":\"claude-opus-5-5\",\"usage\":{\"input_tokens\":40,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"weather\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"Paris\\\"}\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":22}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    fn chunks(sse: &str) -> Vec<Value> {
        sse.split("\n\n")
            .filter_map(|event| event.strip_prefix("data: "))
            .filter(|data| *data != "[DONE]")
            .map(|data| serde_json::from_str(data).unwrap())
            .collect()
    }

    /// The real adapter against a local fake Anthropic: headers, path, and the
    /// translated stream a client reads, for a text turn and a tool-call turn.
    #[tokio::test]
    async fn streams_text_and_tool_calls_as_chat_completion_chunks() {
        async fn messages(headers: axum::http::HeaderMap, Json(body): Json<Value>) -> (StatusCode, String) {
            assert_eq!(headers["x-api-key"], "sk-ant-test");
            assert_eq!(headers["anthropic-version"], API_VERSION);
            if body.get("temperature").is_some() {
                return (StatusCode::BAD_REQUEST, r#"{"type":"error","error":{"type":"invalid_request_error","message":"temperature is not supported for this model"}}"#.into());
            }
            let turn = if body.get("tools").is_some() { TOOL_TURN } else { TEXT_TURN };
            if body["stream"] == true {
                (StatusCode::OK, turn.to_string())
            } else {
                (StatusCode::OK, json!({
                    "id": "msg_03", "model": "claude-opus-5-5", "stop_reason": "tool_use",
                    "content": [
                        { "type": "text", "text": "Checking." },
                        { "type": "tool_use", "id": "toolu_3", "name": "weather", "input": { "city": "Oslo" } },
                    ],
                    "usage": { "input_tokens": 30, "output_tokens": 9 },
                }).to_string())
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v1/messages", post(messages))).await.unwrap();
        });
        let provider = AnthropicProvider::with_base_url(format!("http://{addr}"), "sk-ant-test".into());
        let read = |response: Response<Body>| async move {
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        };

        // Text turn (with a temperature the model refuses: resent without it).
        let response = provider
            .chat_completions(request(json!({
                "model": "claude-opus-5-5", "stream": true, "temperature": 0.7,
                "messages": [{ "role": "user", "content": "Hi" }],
            })))
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/event-stream");
        let sse = read(response).await;
        println!("streamed text turn →\n{sse}");
        assert!(sse.ends_with("data: [DONE]\n\n"));
        let text = chunks(&sse);
        let content: String = text.iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
        assert_eq!(content, "Hello there");
        assert_eq!(text[0]["choices"][0]["delta"]["role"], "assistant");
        assert!(text.iter().any(|c| c["choices"][0]["finish_reason"] == "stop"));
        assert_eq!(text.last().unwrap()["usage"], json!({ "prompt_tokens": 15, "completion_tokens": 5, "total_tokens": 20 }));
        assert!(text.iter().all(|c| c["id"] == "msg_01" && c["object"] == "chat.completion.chunk"));

        // Tool-call turn.
        let tools = json!([{ "type": "function", "function": { "name": "weather", "parameters": { "type": "object" } } }]);
        let response = provider
            .chat_completions(request(json!({
                "model": "claude-opus-5-5", "stream": true, "tools": tools,
                "messages": [{ "role": "user", "content": "Weather in Paris?" }],
            })))
            .await
            .unwrap();
        let sse = read(response).await;
        println!("streamed tool-call turn →\n{sse}");
        let tool = chunks(&sse);
        let calls: Vec<&Value> = tool.iter().filter_map(|c| c["choices"][0]["delta"]["tool_calls"].get(0)).collect();
        assert_eq!(calls[0]["id"], "toolu_9");
        assert_eq!(calls[0]["function"]["name"], "weather");
        assert!(calls.iter().all(|call| call["index"] == 0), "one call, index 0 (the thinking block is not a call)");
        let arguments: String = calls.iter().filter_map(|c| c["function"]["arguments"].as_str()).collect();
        assert_eq!(serde_json::from_str::<Value>(&arguments).unwrap(), json!({ "city": "Paris" }));
        assert!(tool.iter().any(|c| c["choices"][0]["finish_reason"] == "tool_calls"));
        assert_eq!(tool.last().unwrap()["usage"]["completion_tokens"], 22);

        // Not streamed.
        let response = provider
            .chat_completions(request(json!({
                "model": "claude-opus-5-5", "tools": tools,
                "messages": [{ "role": "user", "content": "Weather in Oslo?" }],
            })))
            .await
            .unwrap();
        let body: Value = serde_json::from_str(&read(response).await).unwrap();
        assert_eq!(body["object"], "chat.completion");
        assert_eq!(body["choices"][0]["message"]["content"], "Checking.");
        assert_eq!(body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"], r#"{"city":"Oslo"}"#);
        assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(body["usage"]["total_tokens"], 39);
        server.abort();
    }

    #[test]
    fn events_split_across_reads_still_translate() {
        let mut translator = StreamTranslator::new("claude");
        let (first, second) = TEXT_TURN.split_at(TEXT_TURN.len() / 2);
        let out = translator.push(first.as_bytes()) + &translator.push(second.as_bytes()) + &translator.end();
        let content: String = chunks(&out).iter().filter_map(|c| c["choices"][0]["delta"]["content"].as_str()).collect();
        assert_eq!(content, "Hello there");
        assert_eq!(out.matches("[DONE]").count(), 1, "end() after message_stop adds nothing");
    }
}
