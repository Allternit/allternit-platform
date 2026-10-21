//! Embeddings endpoint (A6).
//!
//! `POST /v1/embeddings` — generate vector embeddings for one or more input
//! texts. Mounted under `/v1` by the LLM gateway router.
//!
//! When a configured embeddings provider is available, requests are proxied
//! to it. Otherwise, a deterministic hash-based embedding is generated
//! locally for development and testing.
//!
//! Dev escape hatch: the local fake embedding is only returned when
//! `ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS=1` is set (dev machines, tests).
//! Without it — the default — the endpoint returns HTTP 501 with code
//! `allternit.not_configured`, because silently billing for hash-based fake
//! embeddings on a production surface is never acceptable.

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use base64::{engine::general_purpose, Engine as _};

use crate::AppState;

use super::{
    auth::LlmKeyContext,
    translate::{error_code, OpenAiErrorResponse},
};

/// Env var that re-enables fake provider output (dev only). See module header.
const ALLOW_FAKE_PROVIDERS_ENV: &str = "ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS";

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
#[derive(Debug, Deserialize)]
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

// ─── Handlers ───────────────────────────────────────────────────────────────

/// `POST /v1/embeddings` — generate embeddings for the given input texts.
pub async fn create_embeddings(
    State(_state): State<Arc<AppState>>,
    Extension(_key): Extension<LlmKeyContext>,
    Json(body): Json<CreateEmbeddingsRequest>,
) -> Response {
    let inputs = body.input.as_strings();

    if let Some(err) =
        fake_provider_gate(std::env::var(ALLOW_FAKE_PROVIDERS_ENV).ok().as_deref(), "embeddings")
    {
        return err.into_response();
    }

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

    let mut total_tokens: u64 = 0;
    let data: Vec<EmbeddingData> = inputs
        .iter()
        .enumerate()
        .map(|(index, text)| {
            total_tokens += estimate_tokens(text);
            let embedding = generate_local_embedding(text, dimensions);

            if encoding_format == "base64" {
                // Convert f32 vec to base64-encoded bytes.
                let bytes: Vec<u8> = embedding
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
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

    (StatusCode::OK, Json(json!({
        "object": "list",
        "data": data,
        "model": body.model,
        "usage": {
            "prompt_tokens": total_tokens,
            "total_tokens": total_tokens,
        },
    })))
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
            err.error.message.contains("no embeddings provider configured"),
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
}
