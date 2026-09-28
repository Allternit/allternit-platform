//! Allternit Model Router
//!
//! Abstraction layer that exposes one OpenAI-compatible API to users and
//! dispatches requests to upstream model providers. This module defines the
//! provider trait, request/response types, and the top-level `ModelRouter`.

use axum::{
    body::Body,
    http::Response,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub mod catalog;
pub mod generic_openai;
pub mod openrouter;

#[cfg(test)]
mod tests;

/// Errors that can occur inside the model router.
#[derive(Debug, thiserror::Error)]
pub enum ModelRouterError {
    #[error("Unknown model alias: {0}")]
    UnknownModel(String),

    #[error("Provider {provider} is not configured")]
    ProviderNotConfigured { provider: String },

    #[error("Upstream request failed: {0}")]
    UpstreamRequestFailed(String),

    #[error("Upstream returned error {status}: {message}")]
    UpstreamError { status: u16, message: String },

    #[error("Streaming not supported by provider {0}")]
    StreamingNotSupported(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<ModelRouterError> for crate::ApiError {
    fn from(e: ModelRouterError) -> Self {
        use axum::http::StatusCode;
        match e {
            ModelRouterError::UnknownModel(msg) => crate::ApiError::BadRequest(msg),
            ModelRouterError::ProviderNotConfigured { provider } => {
                crate::ApiError::ServiceUnavailable(format!("provider {} not configured", provider))
            }
            ModelRouterError::UpstreamRequestFailed(msg) => crate::ApiError::ServiceUnavailable(msg),
            ModelRouterError::UpstreamError { status, message } => {
                let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
                if code == StatusCode::UNAUTHORIZED || code == StatusCode::FORBIDDEN {
                    crate::ApiError::Unauthorized(message)
                } else if code == StatusCode::TOO_MANY_REQUESTS {
                    crate::ApiError::ServiceUnavailable(message)
                } else if code.is_server_error() {
                    crate::ApiError::ServiceUnavailable(message)
                } else {
                    crate::ApiError::BadRequest(message)
                }
            }
            ModelRouterError::StreamingNotSupported(msg) => crate::ApiError::BadRequest(msg),
            ModelRouterError::InvalidRequest(msg) => crate::ApiError::BadRequest(msg),
            ModelRouterError::Internal(msg) => crate::ApiError::Internal(msg),
        }
    }
}

/// A single message in a chat completion conversation, passed to the upstream
/// as sent. OpenAI-compatible clients (gizzi's agent loop, the AI SDK) send
/// `content` as a string, an array of content parts (text, image_url…), or
/// null on an assistant turn that only calls tools, and carry `tool_calls`,
/// `tool_call_id` and `name` on the message. A `content: String` field with
/// nothing else rejected every real agent turn with a 422 and dropped tool
/// calls between rounds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: serde_json::Value,
    /// `tool_calls`, `tool_call_id`, `name`, `reasoning_content`… verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Message {
    /// A plain text message.
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: serde_json::Value::String(content.into()),
            extra: serde_json::Map::new(),
        }
    }

    /// Characters of prompt this message carries, for usage estimates: its
    /// text (a string, or the text parts of an array) plus any tool-call
    /// arguments. Images and other non-text parts count as nothing.
    pub fn content_chars(&self) -> usize {
        let text = match &self.content {
            serde_json::Value::String(s) => s.len(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .map(str::len)
                .sum(),
            _ => 0,
        };
        let tool_args: usize = self
            .extra
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|c| c.pointer("/function/arguments").and_then(|a| a.as_str()))
                    .map(str::len)
                    .sum()
            })
            .unwrap_or(0);
        text + tool_args
    }
}

/// User-facing chat completion request. OpenAI-compatible subset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatCompletionRequest {
    pub fn is_streaming(&self) -> bool {
        self.stream == Some(true)
    }
}

/// One choice in a chat completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// Token usage reported by the upstream.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

/// OpenAI-compatible chat completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Information about a model exposed through `/v1/models`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// An upstream provider that can list models and run chat completions.
#[async_trait::async_trait]
pub trait UpstreamProvider: Send + Sync {
    fn provider_id(&self) -> &str;

    /// List models this provider currently offers.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelRouterError>;

    /// Run a chat completion request. Returns a fully-formed HTTP response so
    /// each provider can handle streaming internally.
    async fn chat_completions(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<Response<Body>, ModelRouterError>;
}

/// Top-level router that maps model aliases to upstream providers.
pub struct ModelRouter {
    providers: HashMap<String, Arc<dyn UpstreamProvider>>,
    alias_map: catalog::ModelAliasMap,
}

impl ModelRouter {
    /// Create a new router from a list of providers and the static alias map.
    pub fn new(
        providers: Vec<Arc<dyn UpstreamProvider>>,
        alias_map: catalog::ModelAliasMap,
    ) -> Self {
        let providers = providers
            .into_iter()
            .map(|p| (p.provider_id().to_string(), p))
            .collect();
        Self {
            providers,
            alias_map,
        }
    }

    /// Create a router with no providers (returns 503 for all requests).
    pub fn disabled(alias_map: catalog::ModelAliasMap) -> Self {
        Self {
            providers: HashMap::new(),
            alias_map,
        }
    }

    /// Returns true if at least one upstream provider is configured.
    pub fn is_enabled(&self) -> bool {
        !self.providers.is_empty()
    }

    /// List all models known to the router, starting with the static catalog.
    /// Live upstream metadata is merged in when a provider is healthy.
    pub async fn list_models(&self) -> Vec<ModelInfo> {
        let mut models = Vec::new();

        for entry in self.alias_map.entries() {
            let mut info = ModelInfo {
                id: entry.alias.clone(),
                object: "model".to_string(),
                created: entry.created,
                owned_by: entry.provider.clone(),
                upstream_id: Some(entry.upstream_id.clone()),
                provider: Some(entry.provider.clone()),
                aliases: entry.aliases.clone(),
                extra: serde_json::Map::new(),
            };

            // Start with static catalog metadata so the public model list is useful
            // even when no upstream provider is configured or the upstream is slow.
            info.extra.insert(
                "name".to_string(),
                serde_json::Value::String(entry.name.clone()),
            );
            info.extra.insert(
                "prompt_price".to_string(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(entry.prompt_price).unwrap_or(0.into()),
                ),
            );
            info.extra.insert(
                "completion_price".to_string(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(entry.completion_price).unwrap_or(0.into()),
                ),
            );
            info.extra.insert(
                "context_length".to_string(),
                serde_json::Value::Number(entry.context_length.into()),
            );

            // If we have a live provider, enrich with upstream metadata.
            // Upstream values take precedence over static catalog defaults.
            if let Some(provider) = self.providers.get(&entry.provider) {
                match provider.list_models().await {
                    Ok(upstream_models) => {
                        if let Some(upstream) = upstream_models
                            .into_iter()
                            .find(|m| m.id == entry.upstream_id)
                        {
                            for (k, v) in upstream.extra {
                                info.extra.insert(k, v);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            provider = %entry.provider,
                            upstream_id = %entry.upstream_id,
                            error = %e,
                            "failed to enrich model metadata"
                        );
                    }
                }
            }

            models.push(info);
        }

        models
    }

    /// The provider id a model alias routes to, without dispatching.
    pub fn provider_for_alias(&self, alias: &str) -> Option<&str> {
        self.alias_map.resolve(alias).map(|entry| entry.provider.as_str())
    }

    /// The upstream model id a model alias routes to, without dispatching.
    pub fn upstream_for_alias(&self, alias: &str) -> Option<&str> {
        self.alias_map.resolve(alias).map(|entry| entry.upstream_id.as_str())
    }

    /// Dispatch a chat completion request to the provider responsible for the
    /// requested model alias.
    pub async fn chat_completions(
        &self,
        mut request: ChatCompletionRequest,
    ) -> Result<Response<Body>, ModelRouterError> {
        let alias = &request.model;
        let entry = self
            .alias_map
            .resolve(alias)
            .ok_or_else(|| ModelRouterError::UnknownModel(alias.clone()))?;

        let provider = self
            .providers
            .get(&entry.provider)
            .ok_or_else(|| ModelRouterError::ProviderNotConfigured {
                provider: entry.provider.clone(),
            })?;

        // Streaming responses only carry token usage when asked: inject
        // stream_options so the usage-settlement body scanner sees a final
        // usage chunk. OpenAI-spec; providers that ignore unknown fields are
        // unaffected, and an explicit caller value always wins.
        if request.is_streaming() {
            request
                .extra
                .entry("stream_options".to_string())
                .or_insert_with(|| serde_json::json!({ "include_usage": true }));
        }

        // Rewrite the model field to the upstream id before dispatching.
        request.model = entry.upstream_id.clone();

        provider.chat_completions(request).await
    }

    /// Resolve the retail prices (per 1M tokens, USD) for a model alias.
    ///
    /// Live upstream pricing (from the provider's cached model list — no extra
    /// fetches) is treated as our wholesale cost and marked up by
    /// `INFERENCE_MARKUP` (default 1.5, clamped 1.0..=5.0); the static catalog
    /// entry is the fallback when the upstream exposes no pricing, in which
    /// case the wholesale side is unknown (None).
    pub async fn retail_prices(&self, alias: &str) -> Result<RetailPrices, ModelRouterError> {
        let entry = self
            .alias_map
            .resolve(alias)
            .ok_or_else(|| ModelRouterError::UnknownModel(alias.to_string()))?;

        if let Some(provider) = self.providers.get(&entry.provider) {
            // A failed/slow upstream list must never block a request: fall
            // back to the static catalog prices.
            if let Ok(models) = provider.list_models().await {
                if let Some(upstream) = models.iter().find(|m| m.id == entry.upstream_id) {
                    // Provider pricing extras are per-TOKEN USD (OpenRouter
                    // convention, see openrouter::fetch_models); the catalog
                    // and everything downstream are per-1M.
                    let prompt = upstream
                        .extra
                        .get("prompt_price")
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1_000_000.0);
                    let completion = upstream
                        .extra
                        .get("completion_price")
                        .and_then(|v| v.as_f64())
                        .map(|v| v * 1_000_000.0);
                    if let (Some(wholesale_prompt), Some(wholesale_completion)) =
                        (prompt, completion)
                    {
                        let markup = inference_markup();
                        return Ok(RetailPrices {
                            prompt_per_1m: wholesale_prompt * markup,
                            completion_per_1m: wholesale_completion * markup,
                            wholesale_prompt_per_1m: Some(wholesale_prompt),
                            wholesale_completion_per_1m: Some(wholesale_completion),
                        });
                    }
                }
            }
        }

        Ok(RetailPrices {
            prompt_per_1m: entry.prompt_price,
            completion_per_1m: entry.completion_price,
            wholesale_prompt_per_1m: None,
            wholesale_completion_per_1m: None,
        })
    }
}

/// Retail prices for one model, per 1M tokens USD. The wholesale fields are
/// our actual upstream cost when live pricing was available.
#[derive(Debug, Clone, PartialEq)]
pub struct RetailPrices {
    pub prompt_per_1m: f64,
    pub completion_per_1m: f64,
    pub wholesale_prompt_per_1m: Option<f64>,
    pub wholesale_completion_per_1m: Option<f64>,
}

/// The retail markup over live upstream pricing: `INFERENCE_MARKUP`, default
/// 1.5, clamped to 1.0..=5.0 so a typo can never give inference away for free
/// or price it absurdly.
fn inference_markup() -> f64 {
    std::env::var("INFERENCE_MARKUP")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .map(|value| value.clamp(1.0, 5.0))
        .unwrap_or(1.5)
}
