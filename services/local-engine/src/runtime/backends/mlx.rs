//! MLX backend: `allternit-mlx-server` (engine pack `mlx-metal-macos-arm64`,
//! source `scripts/engine-packs/mlx-server`). OpenAI-compatible like
//! llama-server, with the same `/health` (503 while loading) and the same
//! `usage` + `timings` on replies.

use std::path::Path;

/// Binary name inside the pack and, for development, on PATH.
pub const BINARY: &str = "allternit-mlx-server";

#[derive(Debug, Clone)]
pub struct MlxConfig<'a> {
    /// Program to run (the verified pack entrypoint, or the PATH fallback).
    pub program: &'a Path,
    /// Local model directory (config.json, *.safetensors, tokenizer files).
    pub model_path: &'a Path,
    pub port: u16,
    /// Id the server reports in `/v1/models` and replies.
    pub model_id: &'a str,
    /// KV cache bound in tokens.
    pub n_ctx: u32,
}

pub fn build_argv(config: MlxConfig<'_>) -> Vec<String> {
    vec![
        config.program.to_string_lossy().into_owned(),
        "--model".into(),
        config.model_path.to_string_lossy().into_owned(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        config.port.to_string(),
        "--model-id".into(),
        config.model_id.to_string(),
        "--ctx".into(),
        config.n_ctx.to_string(),
    ]
}
