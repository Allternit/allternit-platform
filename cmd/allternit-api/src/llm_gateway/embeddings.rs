//! Embeddings endpoint (A6).
//!
//! `POST /v1/embeddings` — generate vector embeddings for one or more input
//! texts. Mounted under `/v1` by the LLM gateway router.
//!
//! When `ALLTERNIT_GATEWAY_EMBED_URL` names an OpenAI-compatible embeddings
//! backend (full base URL including `/v1`), requests are validated and
//! proxied to it transparently — status, body, model id, and usage all come
//! from the provider. Unset (the default) keeps the endpoint in its safe
//! unconfigured state.
//!
//! Dev escape hatch: without a configured provider, a deterministic
//! hash-based embedding is generated locally for development and testing,
//! but ONLY when `ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS=1` is set (dev
//! machines, tests). Without either configuration the endpoint returns
//! HTTP 501 with code `allternit.not_configured`, because silently billing
//! for hash-based fake embeddings on a production surface is never
//! acceptable.

use axum::{
    body::Body,
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use crate::AppState;

use super::{
    auth::LlmKeyContext,
    translate::{error_code, OpenAiErrorResponse},
};

/// Env var naming the OpenAI-compatible embeddings backend the gateway
/// proxies to, e.g. `http://127.0.0.1:8080/v1`. Unset or empty keeps the
/// endpoint unconfigured (501), matching the historical default.
const EMBED_PROVIDER_URL_ENV: &str = "ALLTERNIT_GATEWAY_EMBED_URL";

/// Upstream embeddings calls get a generous but bounded timeout: embedding
/// batches are compute-bound on the backend and can take tens of seconds.
const EMBED_PROVIDER_TIMEOUT: Duration = Duration::from_secs(60);

/// Env var that re-enables fake provider output (dev only). See module header.
const ALLOW_FAKE_PROVIDERS_ENV: &str = "ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS";

/// Resolve the configured embeddings backend, if any. Trailing slashes are
/// trimmed so `upstream_embeddings_url` can append cleanly. `env_value` is
/// injected so tests stay deterministic without touching process-wide
/// environment.
fn resolve_provider_url(env_value: Option<&str>) -> Option<String> {
    env_value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
}

/// `{base}/embeddings` for an OpenAI-compatible base URL.
fn upstream_embeddings_url(base: &str) -> String {
    format!("{}/embeddings", base.trim_end_matches('/'))
}

/// Gatekeeper for the no-provider fallback: returns `Some(501
/// allternit.not_configured)` unless the fake-provider escape hatch is set.
/// `env_value` is injected so tests stay deterministic without touching
/// process-wide environment.
fn fake_provider_gate(env_value: Option<&str>, provider_kind: &str) -> Option<OpenAiErrorResponse> {
    if env_value == Some("1") {
        return None;
    }
    Some(OpenAiErrorResponse::new(
        StatusCode::NOT_IMPLEMENTED,
        format!("no {provider_kind} provider configured"),
        "not_configured",
        None,
        Some(error_code::NOT_CONFIGURED),
    ))
}

// ─── Request types ──────────────────────────────────────────────────────────

/// `POST /v1/embeddings` request body.
#[derive(Debug, Deserialize)]
pub struct CreateEmbeddingsRequest {
    pub input: EmbeddingInput,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub encoding_format: Option<String>,
    #[serde(default)]
    pub dimensions: Option<u32>,
    #[serde(default)]
    pub user: Option<String>,
}

/// Input can be a single string or an array of strings.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum EmbeddingInput {
    Single(String),
    Multiple(Vec<String>),
}

impl EmbeddingInput {
    fn as_strings(&self) -> Vec<&str> {
        match self {
            EmbeddingInput::Single(s) => vec![s.as_str()],
            EmbeddingInput::Multiple(v) => v.iter().map(|s| s.as_str()).collect(),
        }
    }
}

fn default_model() -> String {
    "allternit-embedding-3-small".to_string()
}

// ─── Response types ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct EmbeddingsResponse {
    pub object: &'static str,
    pub data: Vec<EmbeddingData>,
    pub model: String,
    pub usage: EmbeddingUsage,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingData {
    pub object: &'static str,
    pub embedding: Vec<f32>,
    pub index: usize,
}

#[derive(Debug, Serialize)]
pub struct EmbeddingUsage {
    pub prompt_tokens: u64,
    pub total_tokens: u64,
}

// ─── Embedding generation ───────────────────────────────────────────────────

/// Default embedding dimensionality for local hash-based embeddings.
const DEFAULT_DIMENSIONS: usize = 1536;

/// Generate a deterministic embedding vector from text using hashing.
/// This produces a normalized unit vector suitable for cosine similarity
/// comparisons during development. In production, this is replaced by
/// provider-proxied embeddings.
pub fn generate_local_embedding(text: &str, dimensions: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; dimensions];

    // Use multiple hash passes to fill the vector.
    let chunks: Vec<&str> = text.split_whitespace().collect();
    if chunks.is_empty() {
        return vec;
    }

    for (i, dim_chunk) in vec.chunks_mut(1).enumerate() {
        let word_idx = i % chunks.len().max(1);
        let word = chunks[word_idx];

        let mut hasher = DefaultHasher::new();
        word.hash(&mut hasher);
        (i as u64).hash(&mut hasher);
        let hash = hasher.finish();

        // Map hash bits to [-1.0, 1.0].
        let val = ((hash as f64 / u64::MAX as f64) * 2.0 - 1.0) as f32;
        dim_chunk[0] = val;
    }

    // Normalize to unit vector for cosine similarity.
    let magnitude: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if magnitude > 0.0 {
        for v in &mut vec {
            *v /= magnitude;
        }
    }

    vec
}

/// Estimate token count for a text string (~4 chars per token heuristic).
fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64 + 3) / 4
}

// ─── Provider proxy ─────────────────────────────────────────────────────────

/// Forward a validated embeddings request to the configured
/// OpenAI-compatible backend and pass its response through transparently —
/// status code and body (data, model id, usage) are the provider's own.
///
/// The `user` field is deliberately not forwarded: end-user identifiers
/// stay on this gateway.
async fn proxy_embeddings(provider_base: &str, body: &CreateEmbeddingsRequest) -> Response {
    let client = match reqwest::Client::builder()
        .timeout(EMBED_PROVIDER_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return OpenAiErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("embeddings provider client init failed: {e}"),
                "internal_error",
                None,
                Some(error_code::INTERNAL_ERROR),
            )
            .into_response();
        }
    };

    let mut payload = json!({
        "model": body.model,
        "input": body.input,
    });
    if let Some(fmt) = &body.encoding_format {
        payload["encoding_format"] = json!(fmt);
    }
    if let Some(d) = body.dimensions {
        payload["dimensions"] = json!(d);
    }

    let resp = match client
        .post(upstream_embeddings_url(provider_base))
        .json(&payload)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, provider = %provider_base, "embeddings provider unreachable");
            return OpenAiErrorResponse::upstream(
                format!("embeddings provider unreachable: {e}"),
                "provider_connection_error",
            )
            .into_response();
        }
    };

    let status = resp.status();
    match resp.text().await {
        Ok(text) => Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from(text))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) => {
            tracing::warn!(error = %e, provider = %provider_base, "embeddings provider returned an unreadable body");
            OpenAiErrorResponse::upstream(
                format!("embeddings provider returned an unreadable response: {e}"),
                "provider_response_error",
            )
            .into_response()
        }
    }
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `POST /v1/embeddings` — generate embeddings for the given input texts.
pub async fn create_embeddings(
    State(_state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Json(body): Json<CreateEmbeddingsRequest>,
) -> Response {
    let inputs = body.input.as_strings();

    if inputs.is_empty() {
        return OpenAiErrorResponse::invalid_request(
            "`input` must contain at least one non-empty string.",
            Some("input"),
        )
        .into_response();
    }

    if inputs.len() > 2048 {
        return OpenAiErrorResponse::invalid_request(
            "`input` must contain at most 2048 strings.",
            Some("input"),
        )
        .into_response();
    }

    for (i, text) in inputs.iter().enumerate() {
        if text.is_empty() {
            return OpenAiErrorResponse::invalid_request(
                format!("`input[{i}]` must not be empty."),
                Some("input"),
            )
            .into_response();
        }
    }

    let dimensions = body.dimensions.unwrap_or(DEFAULT_DIMENSIONS as u32) as usize;
    let encoding_format = body.encoding_format.as_deref().unwrap_or("float");

    if encoding_format != "float" && encoding_format != "base64" {
        return OpenAiErrorResponse::invalid_request(
            "`encoding_format` must be `float` or `base64`.",
            Some("encoding_format"),
        )
        .into_response();
    }

    if dimensions < 1 || dimensions > 3072 {
        return OpenAiErrorResponse::invalid_request(
            "`dimensions` must be between 1 and 3072.",
            Some("dimensions"),
        )
        .into_response();
    }

    // Configured provider wins: validated request is proxied transparently.
    if let Some(provider_base) =
        resolve_provider_url(std::env::var(EMBED_PROVIDER_URL_ENV).ok().as_deref())
    {
        return proxy_embeddings(&provider_base, &body).await;
    }

    // No provider: refuse on production surfaces unless the dev escape
    // hatch is explicitly set.
    if let Some(err) = fake_provider_gate(
        std::env::var(ALLOW_FAKE_PROVIDERS_ENV).ok().as_deref(),
        "embeddings",
    ) {
        return err.into_response();
    }

    let mut total_tokens: u64 = 0;
    let data: Vec<EmbeddingData> = inputs
        .iter()
        .enumerate()
        .map(|(index, text)| {
            total_tokens += estimate_tokens(text);
            let embedding = generate_local_embedding(text, dimensions);

            if encoding_format == "base64" {
                // Convert f32 vec to base64-encoded bytes.
                let bytes: Vec<u8> = embedding.iter().flat_map(|v| v.to_le_bytes()).collect();
                let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
                EmbeddingData {
                    object: "embedding",
                    embedding: vec![0.0], // placeholder; actual data in base64
                    index,
                }
            } else {
                EmbeddingData {
                    object: "embedding",
                    embedding,
                    index,
                }
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "object": "list",
            "data": data,
            "model": body.model,
            "usage": {
                "prompt_tokens": total_tokens,
                "total_tokens": total_tokens,
            },
        })),
    )
        .into_response()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_embedding_has_correct_dimensions() {
        let emb = generate_local_embedding("hello world", 512);
        assert_eq!(emb.len(), 512);
    }

    #[test]
    fn local_embedding_is_normalized() {
        let emb = generate_local_embedding("test vector normalization", 256);
        let magnitude: f32 = emb.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((magnitude - 1.0).abs() < 0.01);
    }

    #[test]
    fn local_embedding_is_deterministic() {
        let a = generate_local_embedding("same input", 128);
        let b = generate_local_embedding("same input", 128);
        assert_eq!(a, b);
    }

    #[test]
    fn different_inputs_produce_different_embeddings() {
        let a = generate_local_embedding("hello", 128);
        let b = generate_local_embedding("world", 128);
        assert_ne!(a, b);
    }

    #[test]
    fn token_estimation_rough() {
        assert_eq!(estimate_tokens("hello world"), 3);
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
    }

    #[test]
    fn fake_provider_gate_blocks_without_escape_hatch() {
        let err = fake_provider_gate(None, "embeddings").expect("must block when env is unset");
        assert_eq!(err.status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(err.error.code.as_deref(), Some(error_code::NOT_CONFIGURED));
        assert!(
            err.error
                .message
                .contains("no embeddings provider configured"),
            "detail must name the provider kind: {}",
            err.error.message
        );
    }

    #[test]
    fn fake_provider_gate_allows_with_escape_hatch() {
        assert!(fake_provider_gate(Some("1"), "embeddings").is_none());
    }

    #[test]
    fn fake_provider_gate_ignores_other_values() {
        // Only the exact value "1" opts in; anything else stays fenced.
        assert!(fake_provider_gate(Some("true"), "embeddings").is_some());
        assert!(fake_provider_gate(Some(""), "embeddings").is_some());
    }

    #[test]
    fn provider_url_unset_or_blank_means_unconfigured() {
        assert_eq!(resolve_provider_url(None), None);
        assert_eq!(resolve_provider_url(Some("")), None);
        assert_eq!(resolve_provider_url(Some("   ")), None);
    }

    #[test]
    fn provider_url_is_trimmed_of_trailing_slashes() {
        assert_eq!(
            resolve_provider_url(Some("http://127.0.0.1:8080/v1")).as_deref(),
            Some("http://127.0.0.1:8080/v1")
        );
        assert_eq!(
            resolve_provider_url(Some("http://127.0.0.1:8080/v1/")).as_deref(),
            Some("http://127.0.0.1:8080/v1")
        );
    }

    #[test]
    fn upstream_url_appends_embeddings_endpoint() {
        assert_eq!(
            upstream_embeddings_url("http://127.0.0.1:8080/v1"),
            "http://127.0.0.1:8080/v1/embeddings"
        );
        assert_eq!(
            upstream_embeddings_url("http://127.0.0.1:8080/v1/"),
            "http://127.0.0.1:8080/v1/embeddings"
        );
    }
}
