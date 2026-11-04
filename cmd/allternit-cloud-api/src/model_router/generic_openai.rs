//! Generic OpenAI-compatible upstream adapter.
//!
//! Many model hosts — Together AI, Fireworks AI, DeepInfra, Groq, etc. — expose
//! an OpenAI-compatible `/v1/models` and `/v1/chat/completions` surface. This
//! adapter implements `UpstreamProvider` for any of them, requiring only a base
//! URL and bearer token. Provider-specific metadata (name, pricing, context) is
//! extracted from the standard OpenRouter-style fields when present.

use axum::{
    body::{Body, Bytes},
    http::{header, Response, StatusCode},
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::{
    ChatCompletionRequest, ChatCompletionResponse, ModelInfo, ModelRouterError, UpstreamProvider,
};

const DEFAULT_MODEL_LIST_CACHE_TTL: Duration = Duration::from_secs(300);

/// Generic OpenAI-compatible provider configuration.
#[derive(Debug, Clone)]
pub struct GenericOpenAiConfig {
    /// Provider identifier used in the catalog, e.g. `together` or `fireworks`.
    pub provider_id: String,
    /// Base URL without trailing slash, e.g. `https://api.together.xyz/v1`.
    pub base_url: String,
    /// API key sent as `Authorization: Bearer <api_key>`.
    pub api_key: String,
    /// How long to cache the model list.
    pub model_list_cache_ttl: Duration,
}

impl GenericOpenAiConfig {
    /// Build a config from environment variables.
    ///
    /// Variables: `<PREFIX>_API_KEY` and optional `<PREFIX>_BASE_URL`.
    /// `provider_id` is derived from `prefix` (lower-case).
    ///
    /// Example prefix: `TOGETHER` reads `TOGETHER_API_KEY` and
    /// `TOGETHER_BASE_URL`.
    pub fn from_env(prefix: &str) -> Option<Self> {
        Self::from_env_with_default_base(prefix, None)
    }

    /// Build a config with a fallback base URL when `<PREFIX>_BASE_URL` is unset.
    pub fn from_env_with_default_base(
        prefix: &str,
        default_base_url: Option<&str>,
    ) -> Option<Self> {
        let key_var = format!("{}_API_KEY", prefix.to_uppercase());
        let api_key = std::env::var(&key_var).ok()?;
        if api_key.trim().is_empty() {
            return None;
        }

        let base_url_var = format!("{}_BASE_URL", prefix.to_uppercase());
        let base_url = std::env::var(&base_url_var)
            .ok()
            .or_else(|| default_base_url.map(|s| s.to_string()))?;

        Some(Self {
            provider_id: prefix.to_lowercase(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model_list_cache_ttl: DEFAULT_MODEL_LIST_CACHE_TTL,
        })
    }
}

/// Response shape for the standard OpenAI `/v1/models` endpoint.
#[derive(Debug, Deserialize)]
struct OpenAiModelList {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// Together AI and some other hosts use `display_name` instead of `name`.
    #[serde(default, alias = "display_name")]
    display_name: Option<String>,
    #[serde(default)]
    object: Option<String>,
    #[serde(default)]
    created: Option<i64>,
    #[serde(default)]
    owned_by: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    pricing: Option<OpenAiPricing>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// Deserialize a numeric price value that may be represented as a JSON
/// number or a numeric string (e.g. Groq returns `"0.0000006"`).
fn deserialize_price_string<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_f64()),
        Some(serde_json::Value::String(s)) => s
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid numeric price string: {}", s))),
        Some(other) => Err(serde::de::Error::custom(format!(
            "expected number or numeric string for price, got {}",
            other
        ))),
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAiPricing {
    /// OpenAI-style price field (unit varies by host: per 1M on Together /
    /// DeepInfra, per token on Groq — `per_token_price` disambiguates).
    #[serde(default, deserialize_with = "deserialize_price_string")]
    prompt: Option<f64>,
    /// OpenAI-style completion price field (unit varies by host, see above).
    #[serde(default, deserialize_with = "deserialize_price_string")]
    completion: Option<f64>,
    /// Together AI uses `input`/`output` instead of `prompt`/`completion`
    /// (quoted per 1M tokens).
    #[serde(default, alias = "input", deserialize_with = "deserialize_price_string")]
    input: Option<f64>,
    #[serde(default, alias = "output", deserialize_with = "deserialize_price_string")]
    output: Option<f64>,
    /// Some hosts (Together AI) include image/video/transcribe pricing as
    /// objects rather than scalars. Capture them as raw JSON so they do not
    /// break deserialization of text-model pricing.
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

/// Normalize an upstream price to the per-TOKEN convention of `extra`.
///
/// Hosts disagree on the unit: Together/DeepInfra quote USD **per 1M
/// tokens**, while Groq quotes USD **per token** (`"0.0000006"` for a
/// $0.60/1M model). The two ranges never overlap in practice — the cheapest
/// per-1M quote is ~$0.01 and the most expensive per-token quote is ~1e-4
/// ($100/1M) — so a threshold cleanly disambiguates. Storing a per-1M value
/// as-is makes the router meter ~1,000,000x UNDER (REVENUE-CRITICAL: the
/// 2026-09 soak recorded $8.64e-11 for a call that should have metered
/// ~$0.0001); dividing a per-token value again is the same bug in reverse.
fn per_token_price(usd: f64) -> f64 {
    if usd < 1e-4 {
        usd
    } else {
        usd / 1_000_000.0
    }
}

/// Insert `pricing` into `extra` as `prompt_price`/`completion_price`.
///
/// The extras convention is per-TOKEN (OpenRouter, see
/// `ModelRouter::retail_prices`); `per_token_price` normalizes whichever
/// unit the host quoted.
fn insert_pricing_extras(
    extra: &mut serde_json::Map<String, serde_json::Value>,
    pricing: &OpenAiPricing,
) {
    let prompt_price = pricing.prompt.or(pricing.input).map(per_token_price);
    let completion_price = pricing
        .completion
        .or(pricing.output)
        .map(per_token_price);
    if let Some(prompt) = prompt_price {
        extra
            .entry("prompt_price".to_string())
            .or_insert(serde_json::Value::Number(
                serde_json::Number::from_f64(prompt).unwrap_or(0.into()),
            ));
    }
    if let Some(completion) = completion_price {
        extra
            .entry("completion_price".to_string())
            .or_insert(serde_json::Value::Number(
                serde_json::Number::from_f64(completion).unwrap_or(0.into()),
            ));
    }
}

/// Generic OpenAI-compatible upstream provider.
pub struct GenericOpenAiProvider {
    config: GenericOpenAiConfig,
    client: reqwest::Client,
    cache: RwLock<Option<(Instant, Vec<ModelInfo>)>>,
}

impl GenericOpenAiProvider {
    pub fn new(config: GenericOpenAiConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            client: crate::net_client::client(),
            cache: RwLock::new(None),
        })
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", self.config.api_key)
                .parse()
                .expect("valid bearer header"),
        );
        headers
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ModelRouterError> {
        let url = format!("{}/models", self.config.base_url);
        let response = self
            .client
            .get(&url)
            .headers(self.headers())
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| ModelRouterError::UpstreamRequestFailed(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(ModelRouterError::UpstreamError {
                status: status.as_u16(),
                message,
            });
        }

        // OpenAI returns {"data": [...]}; some hosts (e.g. Together AI) return a raw array.
        let body = response
            .text()
            .await
            .map_err(|e| ModelRouterError::UpstreamRequestFailed(format!("failed to read body: {}", e)))?;
        let data: Vec<OpenAiModel> = match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(serde_json::Value::Array(arr)) => arr
                .into_iter()
                .map(|v| {
                    serde_json::from_value(v)
                        .map_err(|e| ModelRouterError::UpstreamRequestFailed(e.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Ok(serde_json::Value::Object(mut obj)) => {
                let data_value = obj.remove("data").unwrap_or(serde_json::Value::Array(vec![]));
                serde_json::from_value(data_value).map_err(|e| {
                    ModelRouterError::UpstreamRequestFailed(format!("invalid model list data: {}", e))
                })?
            }
            Ok(_) => Vec::new(),
            Err(e) => return Err(ModelRouterError::UpstreamRequestFailed(format!("invalid JSON: {}", e))),
        };

        let models = data
            .into_iter()
            .map(|m| {
                let mut extra = m.extra;
                if let Some(name) = m.name.or(m.display_name) {
                    extra.entry("name".to_string()).or_insert(serde_json::Value::String(name));
                }

                if let Some(ctx) = m.context_length {
                    extra.entry(
                        "context_length".to_string(),
                    )
                    .or_insert(serde_json::Value::Number(ctx.into()));
                }
                if let Some(p) = m.pricing {
                    insert_pricing_extras(&mut extra, &p);
                }

                ModelInfo {
                    id: m.id,
                    object: m.object.unwrap_or_else(|| "model".to_string()),
                    created: m.created.map(|c| c.max(0) as u64).unwrap_or(0),
                    owned_by: m.owned_by.unwrap_or_else(|| self.config.provider_id.clone()),
                    upstream_id: None,
                    provider: Some(self.config.provider_id.clone()),
                    aliases: None,
                    extra,
                }
            })
            .collect();

        Ok(models)
    }

    async fn chat_completions_non_streaming(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<Response<Body>, ModelRouterError> {
        let url = format!("{}/chat/completions", self.config.base_url);
        let upstream_response = self
            .client
            .post(&url)
            .headers(self.headers())
            .json(&request)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| ModelRouterError::UpstreamRequestFailed(e.to_string()))?;

        let status = upstream_response.status();
        if !status.is_success() {
            let message = upstream_response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(ModelRouterError::UpstreamError {
                status: status.as_u16(),
                message,
            });
        }

        let response_body: ChatCompletionResponse = upstream_response
            .json()
            .await
            .map_err(|e| ModelRouterError::UpstreamRequestFailed(format!("invalid JSON: {}", e)))?;

        let json = serde_json::to_string(&response_body)
            .map_err(|e| ModelRouterError::Internal(e.to_string()))?;

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json))
            .map_err(|e| ModelRouterError::Internal(e.to_string()))
    }

    async fn chat_completions_streaming(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<Response<Body>, ModelRouterError> {
        let url = format!("{}/chat/completions", self.config.base_url);
        let upstream_response = self
            .client
            .post(&url)
            .headers(self.headers())
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&request)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| ModelRouterError::UpstreamRequestFailed(e.to_string()))?;

        let status = upstream_response.status();
        if !status.is_success() {
            let message = upstream_response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(ModelRouterError::UpstreamError {
                status: status.as_u16(),
                message,
            });
        }

        let byte_stream = upstream_response.bytes_stream();
        let body = Body::from_stream(byte_stream);

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(body)
            .map_err(|e| ModelRouterError::Internal(e.to_string()))
    }
}

#[async_trait::async_trait]
impl UpstreamProvider for GenericOpenAiProvider {
    fn provider_id(&self) -> &str {
        &self.config.provider_id
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelRouterError> {
        {
            let guard = self.cache.read().await;
            if let Some((instant, models)) = guard.as_ref() {
                if instant.elapsed() < self.config.model_list_cache_ttl {
                    return Ok(models.clone());
                }
            }
        }

        let models = self.fetch_models().await?;

        {
            let mut guard = self.cache.write().await;
            *guard = Some((Instant::now(), models.clone()));
        }

        Ok(models)
    }

    async fn chat_completions(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<Response<Body>, ModelRouterError> {
        if request.is_streaming() {
            self.chat_completions_streaming(request).await
        } else {
            self.chat_completions_non_streaming(request).await
        }
    }
}

/// Ensure the raw byte stream yields `Bytes` items compatible with axum Body.
fn _assert_bytes_stream<T>(_: T)
where
    T: futures_util::Stream<Item = std::result::Result<Bytes, reqwest::Error>>,
{
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: generic OpenAI hosts quote pricing per 1M tokens; the
    /// extras convention (and `ModelRouter::retail_prices`) is per token.
    /// Storing per-1M values once metered a 42-token call at $43.68.
    #[test]
    fn pricing_extras_are_normalized_from_per_1m_to_per_token() {
        let pricing = OpenAiPricing {
            prompt: None,
            completion: None,
            input: Some(0.88),
            output: Some(1.20),
            other: serde_json::Map::new(),
        };
        let mut extra = serde_json::Map::new();
        insert_pricing_extras(&mut extra, &pricing);

        let prompt = extra.get("prompt_price").and_then(|v| v.as_f64()).unwrap();
        let completion = extra
            .get("completion_price")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((prompt - 0.88e-6).abs() < 1e-15, "per-1M must become per-token, got {prompt}");
        assert!((completion - 1.20e-6).abs() < 1e-15, "per-1M must become per-token, got {completion}");
    }

    /// Regression: Groq quotes pricing per TOKEN (`"0.0000006"` for a
    /// $0.60/1M model). Dividing it again made the router meter ~1,000,000x
    /// under retail (2026-09 soak: $8.64e-11 recorded for a real call).
    #[test]
    fn pricing_extras_are_normalized_from_per_token_when_host_quotes_per_token() {
        let pricing = OpenAiPricing {
            prompt: Some(0.0000006),
            completion: Some(0.000003),
            input: None,
            output: None,
            other: serde_json::Map::new(),
        };
        let mut extra = serde_json::Map::new();
        insert_pricing_extras(&mut extra, &pricing);

        let prompt = extra.get("prompt_price").and_then(|v| v.as_f64()).unwrap();
        let completion = extra
            .get("completion_price")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((prompt - 6.0e-7).abs() < 1e-18, "per-token must stay per-token, got {prompt}");
        assert!((completion - 3.0e-6).abs() < 1e-18, "per-token must stay per-token, got {completion}");
    }

    #[test]
    fn pricing_extras_leave_existing_values_untouched() {        let pricing = OpenAiPricing {
            prompt: Some(0.88),
            completion: None,
            input: None,
            output: None,
            other: serde_json::Map::new(),
        };
        let mut extra = serde_json::Map::new();
        extra.insert(
            "prompt_price".to_string(),
            serde_json::json!(5.0e-7),
        );
        insert_pricing_extras(&mut extra, &pricing);
        assert_eq!(
            extra.get("prompt_price").and_then(|v| v.as_f64()),
            Some(5.0e-7),
            "or_insert must not overwrite an upstream-provided extra"
        );
    }
}
