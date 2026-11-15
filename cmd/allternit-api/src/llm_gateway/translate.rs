//! OpenAI Chat Completions wire types and pure translation helpers.
//!
//! The request side deliberately accepts unknown fields (OpenAI client
//! libraries routinely send extras) but validates the fields it knows,
//! mapping problems to OpenAI-shaped 400 errors. The response side produces
//! spec-faithful `chat.completion` / `chat.completion.chunk` payloads.
//!
//! Everything in this file is pure data + pure functions; the Gizzi-facing
//! logic lives in `proxy.rs`.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Stable machine-readable codes shared by every gateway error response.
pub mod error_code {
    pub const INVALID_REQUEST: &str = "allternit.invalid_request";
    pub const AUTHENTICATION_FAILED: &str = "allternit.authentication_failed";
    pub const PERMISSION_DENIED: &str = "allternit.permission_denied";
    pub const RATE_LIMITED: &str = "allternit.rate_limited";
    pub const BUDGET_EXCEEDED: &str = "allternit.budget_exceeded";
    pub const IDEMPOTENCY_CONFLICT: &str = "allternit.idempotency_conflict";
    pub const MODEL_NOT_FOUND: &str = "allternit.model_not_found";
    pub const UPSTREAM_ERROR: &str = "allternit.upstream_error";
    pub const INTERNAL_ERROR: &str = "allternit.internal_error";
    pub const CONTENT_POLICY_VIOLATION: &str = "allternit.content_policy_violation";
    /// A billed surface has no real provider configured (embeddings, images,
    /// realtime audio). HTTP 501 — the gateway refuses to emit fake output.
    pub const NOT_CONFIGURED: &str = "allternit.not_configured";
    /// G13: request blocked because no candidate provider satisfies the org's
    /// data-residency pin (HTTP 451).
    pub const DATA_RESIDENCY_VIOLATION: &str = "data_residency_violation";
    /// G14: the organization-level gateway RPM cap was exceeded (HTTP 429).
    pub const ORG_RATE_LIMITED: &str = "org_rate_limited";
}

// ─── Error body ─────────────────────────────────────────────────────────────

/// OpenAI-shaped error body: `{"error": {message, type, param, code}}`.
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// An OpenAI error paired with the HTTP status it should be returned with.
#[derive(Debug)]
pub struct OpenAiErrorResponse {
    pub status: StatusCode,
    pub error: OpenAiError,
}

impl OpenAiErrorResponse {
    pub fn new(
        status: StatusCode,
        message: impl Into<String>,
        error_type: &str,
        param: Option<&str>,
        code: Option<&str>,
    ) -> Self {
        Self {
            status,
            error: OpenAiError {
                message: message.into(),
                error_type: error_type.to_string(),
                param: param.map(str::to_string),
                code: code.map(str::to_string),
            },
        }
    }

    /// 400 invalid_request_error for malformed fields.
    pub fn invalid_request(message: impl Into<String>, param: Option<&str>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            message,
            "invalid_request_error",
            param,
            Some(error_code::INVALID_REQUEST),
        )
    }

    /// 502 for failures talking to the Gizzi runtime. `error_type` carries the
    /// Gizzi error name when one is known.
    pub fn upstream(message: impl Into<String>, error_type: &str) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            message,
            error_type,
            None,
            Some(error_code::UPSTREAM_ERROR),
        )
    }
}

impl IntoResponse for OpenAiErrorResponse {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.error }))).into_response()
    }
}

// ─── Request model ──────────────────────────────────────────────────────────

/// OpenAI chat completion request. Unknown fields are tolerated on purpose
/// (no `deny_unknown_fields`): OpenAI client libraries send extra keys.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stop: Option<StopSequences>,
    #[serde(default)]
    pub presence_penalty: Option<f64>,
    #[serde(default)]
    pub frequency_penalty: Option<f64>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    #[serde(default)]
    pub response_format: Option<ResponseFormat>,
    /// Validated for shape but not forwarded to Gizzi (its agent loop owns
    /// tool execution; see proxy.rs doc comment).
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Provider service tier (e.g. `auto`, `default`, `flex`, `priority`).
    /// Forwarded to Gizzi provider options so OpenAI requests can opt into
    /// flex/priority processing.
    #[serde(default)]
    pub service_tier: Option<String>,
    /// Ask providers that support source citations to include them. For
    /// non-Anthropic providers the gateway falls back to a RAG context block
    /// and parses `[cite:<id>]` markers from the response.
    #[serde(default)]
    pub citations: Option<bool>,
    /// Reference a reusable context cache created via `/v1/context-caches`.
    /// Cached messages are prepended to the request messages before forwarding.
    #[serde(default)]
    pub context_cache_id: Option<String>,
    /// Number of completions to generate for partial / best-of sampling.
    #[serde(default)]
    pub n: Option<u32>,
    /// When set with `n > 1`, return only the single best completion instead
    /// of all candidates.
    #[serde(default)]
    pub best_of: Option<bool>,
    #[serde(default)]
    pub user: Option<String>,
    /// Cost-attribution tags (task G8): a JSON object `{key: value}` attached
    /// to the usage event recorded for this request. Max 20 entries; keys and
    /// values are capped at 128 characters each. Requests without tags store
    /// NULL; a key-level default (`llm_virtual_keys.tags`) is inherited when
    /// the request carries none.
    #[serde(default)]
    pub tags: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default)]
    pub cache_control: Option<serde_json::Value>,
    #[serde(default)]
    pub cache: Option<bool>,
}

impl ChatMessage {
    /// Flatten string-or-multipart content into plain text. Non-text parts
    /// (images, video, audio) become bracketed placeholders so the transcript
    /// still reads sensibly.
    pub fn content_text(&self) -> String {
        match &self.content {
            Some(MessageContent::Text(text)) => text.clone(),
            Some(MessageContent::Parts(parts)) => parts
                .iter()
                .map(|part| match part.part_type.as_str() {
                    "text" => part.text.clone().unwrap_or_default(),
                    "video_url" => {
                        let url = part
                            .video_url
                            .as_ref()
                            .map(|v| v.url.as_str())
                            .unwrap_or("?");
                        format!("[video_url url=\"{url}\"]")
                    }
                    other => format!("[{other}]"),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            None => String::new(),
        }
    }
}

/// Message content is either a plain string or an array of typed parts.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// A content part. `text`, `image_url`, and `video_url` are interpreted and
/// forwarded to Gizzi; other part kinds (input_audio, ...) are kept as their
/// type marker only. A `file_id` may reference a session-scoped file and is
/// resolved to base64 inline data before being forwarded.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub image_url: Option<ImageUrlPart>,
    #[serde(default)]
    pub input_image: Option<InputImagePart>,
    /// Video input: accepts a base64 data-URI (`data:video/mp4;base64,...`),
    /// a plain URL, or a `file_id` reference to a session-scoped upload.
    #[serde(default)]
    pub video_url: Option<VideoUrlPart>,
    #[serde(default)]
    pub input_video: Option<InputVideoPart>,
    #[serde(default)]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageUrlPart {
    pub url: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InputImagePart {
    pub data: String,
    pub format: String,
}

/// Video content part for multimodal models that accept video input.
///
/// The `url` field accepts:
/// - A plain HTTPS URL pointing to a video file
/// - A base64 data-URI (`data:<mime>;base64,<encoded>`)
/// - A `file_id` reference is resolved separately by the proxy layer
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoUrlPart {
    pub url: String,
    #[serde(default)]
    pub detail: Option<String>,
    /// Optional MIME type hint (e.g. `video/mp4`). Inferred from the URL or
    /// data-URI prefix when omitted.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// When the video is provided as raw base64 (without a data-URI prefix),
    /// this field carries the encoded payload directly.
    #[serde(default)]
    pub data: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InputVideoPart {
    pub data: String,
    pub format: String,
}

/// Push a video-aware file part to Gizzi parts.
pub fn push_video(parts: &mut Vec<serde_json::Value>, url: String, mime: &str) {
    parts.push(json!({
        "type": "file",
        "url": url,
        "mime": mime,
    }));
}

pub fn part_video_url(part: &ContentPart) -> Option<String> {
    part.video_url.as_ref().map(|u| u.url.clone())
}

pub fn part_input_video_url(part: &ContentPart) -> Option<String> {
    part.input_video.as_ref().map(|v| {
        let mime = match v.format.as_str() {
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "mov" => "video/quicktime",
            "mkv" => "video/x-matroska",
            _ => "video/mp4",
        };
        format!("data:{};base64,{}", mime, v.data)
    })
}

pub fn video_mime_from_url(url: &str) -> &'static str {
    if url.starts_with("data:video/mp4") {
        "video/mp4"
    } else if url.starts_with("data:video/webm") {
        "video/webm"
    } else if url.starts_with("data:video/quicktime") {
        "video/quicktime"
    } else if url.starts_with("data:video/x-matroska") {
        "video/x-matroska"
    } else {
        "video/mp4"
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// Incremental tool-call delta used in streaming chat completion chunks.
/// Mirrors the OpenAI `choices[].delta.tool_calls[]` shape.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallFunctionDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<ToolCallFunctionDelta>,
}

/// `stop` accepts a single string or an array of strings.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum StopSequences {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    #[serde(default)]
    pub json_schema: Option<JsonSchemaFormat>,
    /// Normalized Allternit shorthand: `{type: "json_schema", schema: ...}`.
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonSchemaFormat {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolFunction {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    #[serde(default)]
    pub strict: Option<bool>,
    #[serde(default)]
    pub cache_control: Option<serde_json::Value>,
}

/// Validate the fields we understand. Returns an OpenAI-shaped 400 on the
/// first problem found.
pub fn validate_request(req: &ChatCompletionRequest) -> Result<(), OpenAiErrorResponse> {
    if req.model.trim().is_empty() {
        return Err(OpenAiErrorResponse::invalid_request(
            "`model` must be a non-empty string.",
            Some("model"),
        ));
    }
    if req.messages.is_empty() {
        return Err(OpenAiErrorResponse::invalid_request(
            "`messages` must contain at least one message.",
            Some("messages"),
        ));
    }
    for (index, message) in req.messages.iter().enumerate() {
        if !matches!(
            message.role.as_str(),
            "system" | "user" | "assistant" | "tool"
        ) {
            return Err(OpenAiErrorResponse::invalid_request(
                format!("messages[{index}].role must be one of system, user, assistant, tool."),
                Some("messages"),
            ));
        }
        if message.role == "tool" && message.tool_call_id.is_none() {
            return Err(OpenAiErrorResponse::invalid_request(
                format!("messages[{index}] with role 'tool' must include `tool_call_id`."),
                Some("messages"),
            ));
        }
    }
    if let Some(tags) = &req.tags {
        if tags.len() > 20 {
            return Err(OpenAiErrorResponse::invalid_request(
                "`tags` may contain at most 20 entries.",
                Some("tags"),
            ));
        }
        for (key, value) in tags {
            if key.is_empty() || key.len() > 128 {
                return Err(OpenAiErrorResponse::invalid_request(
                    "`tags` keys must be 1-128 characters.",
                    Some("tags"),
                ));
            }
            if value.len() > 128 {
                return Err(OpenAiErrorResponse::invalid_request(
                    "`tags` values must be at most 128 characters.",
                    Some("tags"),
                ));
            }
        }
    }
    if let Some(temperature) = req.temperature {
        if !(0.0..=2.0).contains(&temperature) {
            return Err(OpenAiErrorResponse::invalid_request(
                "`temperature` must be between 0 and 2.",
                Some("temperature"),
            ));
        }
    }
    if let Some(top_p) = req.top_p {
        if !(0.0..=1.0).contains(&top_p) {
            return Err(OpenAiErrorResponse::invalid_request(
                "`top_p` must be between 0 and 1.",
                Some("top_p"),
            ));
        }
    }
    if let Some(max_tokens) = req.max_tokens {
        if max_tokens == 0 {
            return Err(OpenAiErrorResponse::invalid_request(
                "`max_tokens` must be at least 1.",
                Some("max_tokens"),
            ));
        }
    }
    if let Some(tools) = &req.tools {
        for tool in tools {
            if tool.tool_type != "function" {
                return Err(OpenAiErrorResponse::invalid_request(
                    "Only `function` tools are supported.",
                    Some("tools"),
                ));
            }
        }
    }
    if let Some(format) = &req.response_format {
        match format.format_type.as_str() {
            "text" | "json_object" => {}
            "json_schema" if format.json_schema.is_some() || format.schema.is_some() => {}
            "json_schema" => {
                return Err(OpenAiErrorResponse::invalid_request(
                    "`response_format.json_schema` is required when type is `json_schema`.",
                    Some("response_format"),
                ))
            }
            _ => {
                return Err(OpenAiErrorResponse::invalid_request(
                    "`response_format.type` must be text, json_object, or json_schema.",
                    Some("response_format"),
                ))
            }
        }
    }
    if let Some(effort) = req.reasoning_effort.as_deref() {
        if !matches!(
            effort,
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        ) {
            return Err(OpenAiErrorResponse::invalid_request(
                "`reasoning_effort` must be one of none, minimal, low, medium, high, xhigh, max, or ultra.",
                Some("reasoning_effort"),
            ));
        }
    }
    if let Some(tier) = req.service_tier.as_deref() {
        if !matches!(tier, "auto" | "default" | "flex" | "priority" | "scale") {
            return Err(OpenAiErrorResponse::invalid_request(
                "`service_tier` must be one of auto, default, flex, priority, or scale.",
                Some("service_tier"),
            ));
        }
    }
    if let Some(n) = req.n {
        if n == 0 || n > 10 {
            return Err(OpenAiErrorResponse::invalid_request(
                "`n` must be between 1 and 10.",
                Some("n"),
            ));
        }
    }
    Ok(())
}

// ─── OpenAI → Gizzi translation ─────────────────────────────────────────────

/// Split an OpenAI message list into Gizzi prompt input: the concatenated
/// system prompt (top-level `system` field of Gizzi's PromptInput) and the
/// remaining history rendered as a labeled plain-text transcript.
///
/// Gizzi owns multi-turn context when a session is reused, so the transcript
/// is only sent in full for fresh sessions (see proxy.rs).
pub fn messages_to_prompt(messages: &[ChatMessage]) -> (Option<String>, String) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut transcript = String::new();

    for message in messages {
        let text = message.content_text();
        match message.role.as_str() {
            "system" => {
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            "user" => {
                if !transcript.is_empty() {
                    transcript.push_str("\n\n");
                }
                match &message.name {
                    Some(name) => transcript.push_str(&format!("User ({name}):\n{text}")),
                    None => transcript.push_str(&format!("User:\n{text}")),
                }
            }
            "assistant" => {
                if !transcript.is_empty() {
                    transcript.push_str("\n\n");
                }
                transcript.push_str(&format!("Assistant:\n{text}"));
                if let Some(tool_calls) = &message.tool_calls {
                    for call in tool_calls {
                        transcript.push_str(&format!(
                            "\n[assistant called tool {}({})]",
                            call.function.name, call.function.arguments
                        ));
                    }
                }
            }
            "tool" => {
                if !transcript.is_empty() {
                    transcript.push_str("\n\n");
                }
                let call_id = message.tool_call_id.as_deref().unwrap_or("unknown");
                transcript.push_str(&format!("Tool result ({call_id}):\n{text}"));
            }
            _ => {}
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };
    (system, transcript)
}

/// Convert an OpenAI message list into Gizzi `parts` plus a separate system
/// prompt. Image content (`image_url` / `input_image`) is preserved as Gizzi
/// `file` parts so vision-capable models receive the actual image.
pub fn messages_to_gizzi_parts(messages: &[ChatMessage]) -> (Option<String>, Vec<serde_json::Value>) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut parts: Vec<serde_json::Value> = Vec::new();

    fn push_text(parts: &mut Vec<serde_json::Value>, text: &str) {
        if !text.is_empty() {
            parts.push(json!({ "type": "text", "text": text }));
        }
    }

    fn push_image(parts: &mut Vec<serde_json::Value>, url: String, mime: &str) {
        parts.push(json!({
            "type": "file",
            "url": url,
            "mime": mime,
        }));
    }

    fn push_video(parts: &mut Vec<serde_json::Value>, url: String, mime: &str) {
        parts.push(json!({
            "type": "file",
            "url": url,
            "mime": mime,
            "kind": "video",
        }));
    }

    fn part_image_url(part: &ContentPart) -> Option<String> {
        part.image_url.as_ref().map(|u| u.url.clone())
    }

    fn part_input_image_url(part: &ContentPart) -> Option<String> {
        part.input_image.as_ref().map(|i| {
            let mime = match i.format.as_str() {
                "png" => "image/png",
                "jpeg" | "jpg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "image/png",
            };
            format!("data:{};base64,{}", mime, i.data)
        })
    }

    fn part_video_url(part: &ContentPart) -> Option<(String, String)> {
        part.video_url.as_ref().map(|v| {
            let url = if let Some(data) = &v.data {
                let mime = v.mime_type.as_deref().unwrap_or("video/mp4");
                format!("data:{mime};base64,{data}")
            } else {
                v.url.clone()
            };
            let mime = v
                .mime_type
                .clone()
                .unwrap_or_else(|| video_mime_from_url(&url).to_string());
            (url, mime)
        })
    }

    fn video_mime_from_url(url: &str) -> &'static str {
        if url.starts_with("data:video/mp4") {
            "video/mp4"
        } else if url.starts_with("data:video/webm") {
            "video/webm"
        } else if url.starts_with("data:video/quicktime") {
            "video/quicktime"
        } else if url.ends_with(".mp4") {
            "video/mp4"
        } else if url.ends_with(".webm") {
            "video/webm"
        } else if url.ends_with(".mov") {
            "video/quicktime"
        } else {
            "video/mp4"
        }
    }

    fn image_mime_from_url(url: &str) -> &'static str {
        if url.starts_with("data:image/png") {
            "image/png"
        } else if url.starts_with("data:image/jpeg") || url.starts_with("data:image/jpg") {
            "image/jpeg"
        } else if url.starts_with("data:image/gif") {
            "image/gif"
        } else if url.starts_with("data:image/webp") {
            "image/webp"
        } else {
            "image/png"
        }
    }

    for message in messages {
        match message.role.as_str() {
            "system" => {
                let text = message.content_text();
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            "user" | "assistant" | "tool" => {
                let prefix = match message.role.as_str() {
                    "user" => match &message.name {
                        Some(name) => format!("User ({name}):\n"),
                        None => "User:\n".to_string(),
                    },
                    "assistant" => "Assistant:\n".to_string(),
                    "tool" => format!("Tool result ({}):\n", message.tool_call_id.as_deref().unwrap_or("unknown")),
                    _ => String::new(),
                };

                match &message.content {
                    Some(MessageContent::Text(text)) => {
                        push_text(&mut parts, &format!("{prefix}{text}"));
                    }
                    Some(MessageContent::Parts(content_parts)) => {
                        let mut text_buffer = prefix;
                        for part in content_parts {
                            if let Some(url) = part_image_url(part)
                                .or_else(|| part_input_image_url(part))
                                .or_else(|| part.video_url.as_ref().map(|v| {
                                    if let Some(data) = &v.data {
                                        let mime = v.mime_type.as_deref().unwrap_or("video/mp4");
                                        format!("data:{mime};base64,{data}")
                                    } else {
                                        v.url.clone()
                                    }
                                }))
                                .or_else(|| part_input_video_url(part))
                            {
                                push_text(&mut parts, &text_buffer);
                                text_buffer = String::new();
                                let mime = if part.video_url.is_some() || part.input_video.is_some() {
                                    video_mime_from_url(&url)
                                } else {
                                    image_mime_from_url(&url)
                                };
                                if part.video_url.is_some() || part.input_video.is_some() {
                                    push_video(&mut parts, url, mime);
                                } else {
                                    push_image(&mut parts, url, mime);
                                }
                            } else {
                                let marker = match part.part_type.as_str() {
                                    "text" => part.text.clone().unwrap_or_default(),
                                    other => format!("[{other}]"),
                                };
                                if !text_buffer.is_empty() && !marker.is_empty() && !text_buffer.ends_with('\n') {
                                    text_buffer.push('\n');
                                }
                                text_buffer.push_str(&marker);
                            }
                        }
                        push_text(&mut parts, &text_buffer);
                    }
                    None => {
                        push_text(&mut parts, &prefix);
                    }
                }

                if message.role == "assistant" {
                    if let Some(tool_calls) = &message.tool_calls {
                        for call in tool_calls {
                            push_text(
                                &mut parts,
                                &format!(
                                    "[assistant called tool {}({})]",
                                    call.function.name, call.function.arguments
                                ),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };
    (system, parts)
}

/// Convert a single OpenAI message into Gizzi `parts` without any role prefix.
/// Used when reusing a Gizzi session and only the last user turn is forwarded.
pub fn message_to_gizzi_parts(message: &ChatMessage) -> Vec<serde_json::Value> {
    let mut parts: Vec<serde_json::Value> = Vec::new();

    fn push_text(parts: &mut Vec<serde_json::Value>, text: &str) {
        if !text.is_empty() {
            parts.push(json!({ "type": "text", "text": text }));
        }
    }

    fn push_image(parts: &mut Vec<serde_json::Value>, url: String, mime: &str) {
        parts.push(json!({
            "type": "file",
            "url": url,
            "mime": mime,
        }));
    }

    fn push_video(parts: &mut Vec<serde_json::Value>, url: String, mime: &str) {
        parts.push(json!({
            "type": "file",
            "url": url,
            "mime": mime,
            "kind": "video",
        }));
    }

    fn part_image_url(part: &ContentPart) -> Option<String> {
        part.image_url.as_ref().map(|u| u.url.clone())
    }

    fn part_input_image_url(part: &ContentPart) -> Option<String> {
        part.input_image.as_ref().map(|i| {
            let mime = match i.format.as_str() {
                "png" => "image/png",
                "jpeg" | "jpg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "image/png",
            };
            format!("data:{};base64,{}" , mime, i.data)
        })
    }

    fn part_video_url(part: &ContentPart) -> Option<(String, String)> {
        part.video_url.as_ref().map(|v| {
            let url = if let Some(data) = &v.data {
                let mime = v.mime_type.as_deref().unwrap_or("video/mp4");
                format!("data:{mime};base64,{data}")
            } else {
                v.url.clone()
            };
            let mime = v
                .mime_type
                .clone()
                .unwrap_or_else(|| video_mime_from_url(&url).to_string());
            (url, mime)
        })
    }

    fn video_mime_from_url(url: &str) -> &'static str {
        if url.starts_with("data:video/mp4") {
            "video/mp4"
        } else if url.starts_with("data:video/webm") {
            "video/webm"
        } else if url.starts_with("data:video/quicktime") {
            "video/quicktime"
        } else if url.ends_with(".mp4") {
            "video/mp4"
        } else if url.ends_with(".webm") {
            "video/webm"
        } else if url.ends_with(".mov") {
            "video/quicktime"
        } else {
            "video/mp4"
        }
    }

    fn image_mime_from_url(url: &str) -> &'static str {
        if url.starts_with("data:image/png") {
            "image/png"
        } else if url.starts_with("data:image/jpeg") || url.starts_with("data:image/jpg") {
            "image/jpeg"
        } else if url.starts_with("data:image/gif") {
            "image/gif"
        } else if url.starts_with("data:image/webp") {
            "image/webp"
        } else {
            "image/png"
        }
    }

    match &message.content {
        Some(MessageContent::Text(text)) => {
            push_text(&mut parts, text);
        }
        Some(MessageContent::Parts(content_parts)) => {
            let mut text_buffer = String::new();
            for part in content_parts {
                if let Some(url) = part_image_url(part)
                    .or_else(|| part_input_image_url(part))
                    .or_else(|| part.video_url.as_ref().map(|v| {
                        if let Some(data) = &v.data {
                            let mime = v.mime_type.as_deref().unwrap_or("video/mp4");
                            format!("data:{mime};base64,{data}")
                        } else {
                            v.url.clone()
                        }
                    }))
                    .or_else(|| part_input_video_url(part))
                {
                    push_text(&mut parts, &text_buffer);
                    text_buffer = String::new();
                    let is_video = part.video_url.is_some() || part.input_video.is_some();
                    let mime = if is_video {
                        video_mime_from_url(&url)
                    } else {
                        image_mime_from_url(&url)
                    };
                    if is_video {
                        push_video(&mut parts, url, mime);
                    } else {
                        push_image(&mut parts, url, mime);
                    }
                } else {
                    let marker = match part.part_type.as_str() {
                        "text" => part.text.clone().unwrap_or_default(),
                        other => format!("[{other}]"),
                    };
                    if !text_buffer.is_empty() && !marker.is_empty() && !text_buffer.ends_with('\n') {
                        text_buffer.push('\n');
                    }
                    text_buffer.push_str(&marker);
                }
            }
            push_text(&mut parts, &text_buffer);
        }
        None => {}
    }

    parts
}

/// Map a Gizzi/AI-SDK finish value to an OpenAI `finish_reason`.
pub fn map_finish_reason(gizzi_finish: Option<&str>) -> &'static str {
    match gizzi_finish.unwrap_or("") {
        "length" | "max_tokens" => "length",
        "tool-calls" | "tool_calls" => "tool_calls",
        "content-filter" | "content_filter" => "content_filter",
        _ => "stop",
    }
}

/// Generate an OpenAI-style completion id.
pub fn new_completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
}

// ─── Response model ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

impl Usage {
    pub fn new(prompt: i64, completion: i64, reasoning: i64, cached: i64) -> Self {
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            completion_tokens_details: (reasoning > 0).then_some(CompletionTokensDetails {
                reasoning_tokens: reasoning,
            }),
            prompt_tokens_details: (cached > 0).then_some(PromptTokensDetails {
                cached_tokens: cached,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionTokensDetails {
    pub reasoning_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptTokensDetails {
    pub cached_tokens: i64,
}

/// OpenAI-style citation/source annotation attached to an assistant message.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Annotation {
    FileCitation { file_citation: FileCitation },
    UrlCitation { url_citation: UrlCitation },
}

#[derive(Debug, Clone, Serialize)]
pub struct FileCitation {
    pub file_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UrlCitation {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<Annotation>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl AssistantMessage {
    pub fn new(content: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            annotations: None,
            refusal: None,
            tool_calls: None,
        }
    }

    pub fn with_annotations(content: String, annotations: Vec<Annotation>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            annotations: Some(annotations),
            refusal: None,
            tool_calls: None,
        }
    }

    pub fn with_refusal(content: String, refusal: String) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            annotations: None,
            refusal: Some(refusal),
            tool_calls: None,
        }
    }

    pub fn with_tool_calls(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            annotations: None,
            refusal: None,
            tool_calls: Some(tool_calls),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: AssistantMessage,
    pub finish_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citations: Option<Vec<super::citations::Citation>>,
}

impl ChatCompletionResponse {
    pub fn new(
        id: String,
        created: i64,
        model: String,
        content: String,
        finish_reason: String,
        usage: Option<Usage>,
    ) -> Self {
        Self {
            id,
            object: "chat.completion".to_string(),
            created,
            model,
            choices: vec![Choice {
                index: 0,
                message: AssistantMessage::new(content),
                finish_reason,
                refusal: None,
            }],
            usage,
            citations: None,
        }
    }

    /// Attach RAG fallback citations to the response body.
    pub fn with_citations(mut self, citations: Vec<super::citations::Citation>) -> Self {
        if !citations.is_empty() {
            self.citations = Some(citations);
        }
        self
    }

    /// Attach OpenAI-style source annotations to the assistant message.
    pub fn with_annotations(mut self, annotations: Vec<Annotation>) -> Self {
        if let Some(choice) = self.choices.first_mut() {
            choice.message.annotations = Some(annotations);
        }
        self
    }
}

// ─── Streaming chunk model ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: ChunkDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

impl ChatCompletionChunk {
    fn base(id: &str, created: i64, model: &str) -> Self {
        Self {
            id: id.to_string(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model.to_string(),
            choices: Vec::new(),
            usage: None,
        }
    }

    /// First chunk of a stream: announces the assistant role.
    pub fn role_chunk(id: &str, created: i64, model: &str) -> Self {
        let mut chunk = Self::base(id, created, model);
        chunk.choices.push(ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: Some("assistant".to_string()),
                content: None,
                tool_calls: None,
            },
            finish_reason: None,
            refusal: None,
        });
        chunk
    }

    /// Content delta chunk.
    pub fn content_chunk(id: &str, created: i64, model: &str, delta: &str) -> Self {
        let mut chunk = Self::base(id, created, model);
        chunk.choices.push(ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: None,
                content: Some(delta.to_string()),
                tool_calls: None,
            },
            finish_reason: None,
            refusal: None,
        });
        chunk
    }

    /// Tool-call delta chunk.
    pub fn tool_calls_chunk(id: &str, created: i64, model: &str, deltas: Vec<ToolCallDelta>) -> Self {
        let mut chunk = Self::base(id, created, model);
        chunk.choices.push(ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: None,
                content: None,
                tool_calls: Some(deltas),
            },
            finish_reason: None,
            refusal: None,
        });
        chunk
    }

    /// Final content chunk: empty delta carrying the finish reason.
    pub fn finish_chunk(id: &str, created: i64, model: &str, finish_reason: &str) -> Self {
        Self::finish_chunk_with_refusal(id, created, model, finish_reason, None)
    }

    /// Final content chunk with an optional refusal annotation.
    pub fn finish_chunk_with_refusal(
        id: &str,
        created: i64,
        model: &str,
        finish_reason: &str,
        refusal: Option<String>,
    ) -> Self {
        let mut chunk = Self::base(id, created, model);
        chunk.choices.push(ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: None,
                content: None,
                tool_calls: None,
            },
            finish_reason: Some(finish_reason.to_string()),
            refusal,
        });
        chunk
    }

    /// Terminal usage chunk (stream_options.include_usage): empty choices,
    /// usage populated.
    pub fn usage_chunk(id: &str, created: i64, model: &str, usage: Usage) -> Self {
        let mut chunk = Self::base(id, created, model);
        chunk.usage = Some(usage);
        chunk
    }

    /// Serialize for an SSE `data:` frame.
    pub fn to_sse_data(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Mid-stream error frame body (OpenAI error shape).
pub fn stream_error_data(message: &str, error_type: &str, code: Option<&str>) -> String {
    json!({
        "error": {
            "message": message,
            "type": error_type,
            "param": serde_json::Value::Null,
            "code": code,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(body: &serde_json::Value) -> Result<ChatCompletionRequest, serde_json::Error> {
        serde_json::from_value(body.clone())
    }

    #[test]
    fn request_tolerates_unknown_fields() {
        let req = parse(&json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "some_future_field": {"nested": true},
            "logit_bias": {},
        }))
        .expect("unknown fields must be tolerated");
        assert_eq!(req.model, "gpt-4o");
        assert!(req.stream.is_none());
    }

    #[test]
    fn request_accepts_string_and_part_content() {
        let req = parse(&json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": "plain"},
                {"role": "user", "content": [
                    {"type": "text", "text": "first"},
                    {"type": "image_url", "image_url": {"url": "data:..."}},
                    {"type": "text", "text": "second"}
                ]}
            ]
        }))
        .unwrap();
        assert_eq!(req.messages[0].content_text(), "plain");
        assert_eq!(req.messages[1].content_text(), "first\n[image_url]\nsecond");
    }

    #[test]
    fn request_accepts_single_or_multiple_stop() {
        let single: StopSequences = serde_json::from_value(json!("END")).unwrap();
        assert!(matches!(single, StopSequences::Single(ref s) if s == "END"));
        let multi: StopSequences = serde_json::from_value(json!(["A", "B"])).unwrap();
        assert!(matches!(multi, StopSequences::Multiple(ref v) if v.len() == 2));
    }

    #[test]
    fn request_accepts_normalized_reasoning_tools_and_json_schema() {
        let req = parse(&json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi", "cache": true}],
            "reasoning_effort": "high",
            "parallel_tool_calls": false,
            "response_format": {"type": "json_schema", "schema": {"type": "object"}},
            "tools": [{"type": "function", "function": {
                "name": "lookup", "parameters": {"type": "object"}, "strict": true,
                "cache_control": {"type": "ephemeral"}
            }}]
        }))
        .unwrap();
        validate_request(&req).unwrap();
        assert_eq!(req.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(req.parallel_tool_calls, Some(false));
        assert_eq!(req.tools.unwrap()[0].function.strict, Some(true));
    }

    #[test]
    fn request_accepts_and_validates_service_tier() {
        let req = parse(&json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "service_tier": "flex"
        }))
        .unwrap();
        validate_request(&req).unwrap();
        assert_eq!(req.service_tier.as_deref(), Some("flex"));

        let bad = parse(&json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "service_tier": "platinum"
        }))
        .unwrap();
        assert!(validate_request(&bad).is_err());
    }

    #[test]
    fn validation_rejects_bad_requests() {
        let no_model =
            parse(&json!({"model": "  ", "messages": [{"role": "user", "content": "x"}]})).unwrap();
        assert!(validate_request(&no_model).is_err());

        let no_messages = parse(&json!({"model": "m", "messages": []})).unwrap();
        assert!(validate_request(&no_messages).is_err());

        let bad_role =
            parse(&json!({"model": "m", "messages": [{"role": "hacker", "content": "x"}]}))
                .unwrap();
        assert!(validate_request(&bad_role).is_err());

        let tool_without_id =
            parse(&json!({"model": "m", "messages": [{"role": "tool", "content": "x"}]})).unwrap();
        assert!(validate_request(&tool_without_id).is_err());

        let bad_temp = parse(&json!({"model": "m", "messages": [{"role": "user", "content": "x"}], "temperature": 3.0})).unwrap();
        assert!(validate_request(&bad_temp).is_err());
    }

    #[test]
    fn prompt_splits_system_and_transcript() {
        let req = parse(&json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "Be terse."},
                {"role": "system", "content": "Be kind."},
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"},
                {"role": "user", "name": "eoj", "content": "How are you?"},
                {"role": "tool", "tool_call_id": "call_1", "content": "42"}
            ]
        }))
        .unwrap();
        let (system, transcript) = messages_to_prompt(&req.messages);
        assert_eq!(system.as_deref(), Some("Be terse.\n\nBe kind."));
        assert_eq!(
            transcript,
            "User:\nHello\n\nAssistant:\nHi there\n\nUser (eoj):\nHow are you?\n\nTool result (call_1):\n42"
        );
    }

    #[test]
    fn prompt_renders_assistant_tool_calls() {
        let req = parse(&json!({
            "model": "m",
            "messages": [
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"SF\"}"}}
                ]}
            ]
        }))
        .unwrap();
        let (_system, transcript) = messages_to_prompt(&req.messages);
        assert_eq!(
            transcript,
            "Assistant:\n\n[assistant called tool get_weather({\"city\":\"SF\"})]"
        );
    }

    #[test]
    fn gizzi_parts_preserve_image_url_and_input_image() {
        let req = parse(&json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "What is this?"},
                    {"type": "image_url", "image_url": {"url": "https://example.com/image.png"}},
                    {"type": "input_image", "input_image": {"data": "abc123", "format": "png"}}
                ]}
            ]
        }))
        .unwrap();
        let (system, parts) = messages_to_gizzi_parts(&req.messages);
        assert!(system.is_none());
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], json!({"type": "text", "text": "User:\nWhat is this?"}));
        assert_eq!(parts[1], json!({"type": "file", "url": "https://example.com/image.png", "mime": "image/png"}));
        assert_eq!(parts[2], json!({"type": "file", "url": "data:image/png;base64,abc123", "mime": "image/png"}));
    }

    #[test]
    fn gizzi_parts_extract_system_prompt_and_text() {
        let req = parse(&json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "Be terse."},
                {"role": "user", "content": "Hello"}
            ]
        }))
        .unwrap();
        let (system, parts) = messages_to_gizzi_parts(&req.messages);
        assert_eq!(system.as_deref(), Some("Be terse."));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], json!({"type": "text", "text": "User:\nHello"}));
    }

    #[test]
    fn message_to_gizzi_parts_for_session_reuse() {
        let message = ChatMessage {
            role: "user".to_string(),
            content: Some(MessageContent::Parts(vec![
                ContentPart { part_type: "text".to_string(), text: Some("Look".to_string()), image_url: None, input_image: None, video_url: None, input_video: None, file_id: None },
                ContentPart { part_type: "image_url".to_string(), text: None, image_url: Some(ImageUrlPart { url: "https://example.com/x.png".to_string(), detail: None }), input_image: None, video_url: None, input_video: None, file_id: None },
            ])),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            cache_control: None,
            cache: None,
        };
        let parts = message_to_gizzi_parts(&message);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], json!({"type": "text", "text": "Look"}));
        assert_eq!(parts[1], json!({"type": "file", "url": "https://example.com/x.png", "mime": "image/png"}));
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(map_finish_reason(Some("stop")), "stop");
        assert_eq!(map_finish_reason(Some("length")), "length");
        assert_eq!(map_finish_reason(Some("tool-calls")), "tool_calls");
        assert_eq!(map_finish_reason(Some("content-filter")), "content_filter");
        assert_eq!(map_finish_reason(None), "stop");
        assert_eq!(map_finish_reason(Some("whatever")), "stop");
    }

    #[test]
    fn error_body_shape() {
        let err = OpenAiErrorResponse::new(
            StatusCode::UNAUTHORIZED,
            "bad key",
            "invalid_request_error",
            None,
            Some(error_code::AUTHENTICATION_FAILED),
        );
        let body = json!({ "error": err.error });
        assert_eq!(body["error"]["message"], "bad key");
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(
            body["error"]["code"],
            error_code::AUTHENTICATION_FAILED
        );
        assert!(body["error"].get("param").is_none());
    }

    #[test]
    fn chunk_serialization() {
        let role = ChatCompletionChunk::role_chunk("chatcmpl-1", 1_700_000_000, "auto");
        let value = serde_json::to_value(&role).unwrap();
        assert_eq!(value["object"], "chat.completion.chunk");
        assert_eq!(value["choices"][0]["delta"]["role"], "assistant");
        assert!(value["choices"][0]["delta"].get("content").is_none());
        assert!(value.get("usage").is_none());

        let tc = ChatCompletionChunk::tool_calls_chunk(
            "chatcmpl-1",
            1_700_000_000,
            "auto",
            vec![ToolCallDelta {
                index: 0,
                id: Some("call_1".to_string()),
                call_type: Some("function".to_string()),
                function: Some(ToolCallFunctionDelta {
                    name: Some("get_weather".to_string()),
                    arguments: Some("{\"city\": \"Paris\"}".to_string()),
                }),
            }],
        );
        let value = serde_json::to_value(&tc).unwrap();
        let delta = &value["choices"][0]["delta"];
        assert!(delta.get("content").is_none());
        assert_eq!(delta["tool_calls"][0]["index"], 0);
        assert_eq!(delta["tool_calls"][0]["id"], "call_1");
        assert_eq!(delta["tool_calls"][0]["function"]["name"], "get_weather");

        let usage = ChatCompletionChunk::usage_chunk(
            "chatcmpl-1",
            1_700_000_000,
            "auto",
            Usage::new(10, 5, 2, 0),
        );
        let value = serde_json::to_value(&usage).unwrap();
        assert_eq!(value["choices"].as_array().unwrap().len(), 0);
        assert_eq!(value["usage"]["total_tokens"], 15);
        assert_eq!(
            value["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
        // cached == 0 → prompt_tokens_details omitted
        assert!(value["usage"].get("prompt_tokens_details").is_none());
    }

    #[test]
    fn completion_response_shape() {
        let resp = ChatCompletionResponse::new(
            new_completion_id(),
            1_700_000_000,
            "anthropic/claude-sonnet-4".to_string(),
            "hello".to_string(),
            "stop".to_string(),
            Some(Usage::new(3, 2, 0, 1)),
        );
        let value = serde_json::to_value(&resp).unwrap();
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(value["choices"][0]["message"]["role"], "assistant");
        assert_eq!(value["choices"][0]["finish_reason"], "stop");
        assert_eq!(value["usage"]["prompt_tokens_details"]["cached_tokens"], 1);
    }

    #[test]
    fn request_accepts_citations_option() {
        let req: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "anthropic/claude-sonnet-4",
            "messages": [{"role": "user", "content": "cite sources"}],
            "citations": true
        }))
        .unwrap();
        assert_eq!(req.citations, Some(true));
        validate_request(&req).unwrap();
    }

    #[test]
    fn response_message_includes_annotations() {
        let annotations = vec![
            Annotation::UrlCitation {
                url_citation: UrlCitation {
                    url: "https://example.com/doc".to_string(),
                    title: Some("Example doc".to_string()),
                },
            },
            Annotation::FileCitation {
                file_citation: FileCitation {
                    file_id: "file_abc123".to_string(),
                },
            },
        ];
        let resp = ChatCompletionResponse::new(
            new_completion_id(),
            1_700_000_000,
            "anthropic/claude-sonnet-4".to_string(),
            "answer with citations".to_string(),
            "stop".to_string(),
            None,
        )
        .with_annotations(annotations);

        let value = serde_json::to_value(&resp).unwrap();
        let message = &value["choices"][0]["message"];
        assert!(message["annotations"].is_array());
        assert_eq!(message["annotations"].as_array().unwrap().len(), 2);
        assert_eq!(message["annotations"][0]["type"], "url_citation");
        assert_eq!(
            message["annotations"][0]["url_citation"]["url"],
            "https://example.com/doc"
        );
        assert_eq!(message["annotations"][1]["type"], "file_citation");
    }

    #[test]
    fn validate_request_accepts_wellformed_tags() {
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "test/model",
            "messages": [{ "role": "user", "content": "hi" }],
            "tags": { "team": "alpha", "env": "prod" }
        }))
        .unwrap();
        assert!(validate_request(&request).is_ok());
    }

    #[test]
    fn validate_request_rejects_malformed_tags() {
        // Over the 20-entry cap.
        let tags: serde_json::Map<String, serde_json::Value> = (0..21)
            .map(|i| (format!("k{i}"), json!("v")))
            .collect();
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "test/model",
            "messages": [{ "role": "user", "content": "hi" }],
            "tags": tags,
        }))
        .unwrap();
        let err = validate_request(&request).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);

        // Over-long key (129 chars).
        let err = validate_request(&serde_json::from_value::<ChatCompletionRequest>(json!({
            "model": "test/model",
            "messages": [{ "role": "user", "content": "hi" }],
            "tags": { "x".repeat(129): "v" },
        }))
        .unwrap())
        .unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);

        // Over-long value (129 chars).
        let err = validate_request(&serde_json::from_value::<ChatCompletionRequest>(json!({
            "model": "test/model",
            "messages": [{ "role": "user", "content": "hi" }],
            "tags": { "k": "x".repeat(129) },
        }))
        .unwrap())
        .unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);

        // Empty key.
        let err = validate_request(&serde_json::from_value::<ChatCompletionRequest>(json!({
            "model": "test/model",
            "messages": [{ "role": "user", "content": "hi" }],
            "tags": { "": "v" },
        }))
        .unwrap())
        .unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn request_body_without_tags_parses_to_none() {
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "test/model",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        assert!(request.tags.is_none());
    }
}
