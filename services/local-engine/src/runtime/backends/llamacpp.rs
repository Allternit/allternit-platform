//! llama.cpp backend argument builder.

use std::path::Path;

/// Configuration for launching a llama.cpp server.
#[derive(Debug, Clone)]
pub struct LlamaCppConfig<'a> {
    /// Program to run: the verified engine pack entrypoint, or `llama-server`
    /// found on PATH (development fallback).
    pub program: &'a Path,
    /// Path to the model file (GGUF, safetensors, etc.).
    pub model_path: &'a Path,
    /// TCP port the server should bind to.
    pub port: u16,
    /// Number of layers to offload to the GPU (`-ngl`).
    pub n_gpu_layers: u32,
    /// Context size (`-c`).
    pub n_ctx: u32,
    /// Enable Flash Attention (`-fa`).
    pub flash_attn: bool,
}

/// Build the argument vector for `llama-server`.
pub fn build_argv(config: LlamaCppConfig<'_>) -> Vec<String> {
    let mut argv = vec![
        config.program.to_string_lossy().into_owned(),
        "-m".to_string(),
        config.model_path.to_string_lossy().into_owned(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        config.port.to_string(),
        "-ngl".to_string(),
        config.n_gpu_layers.to_string(),
        "-c".to_string(),
        config.n_ctx.to_string(),
    ];
    if config.flash_attn {
        argv.push("-fa".to_string());
    }
    argv
}

/// Health endpoint URL for a running llama.cpp server.
pub fn health_url(port: u16) -> String {
    format!("http://127.0.0.1:{}/health", port)
}

/// OpenAI-compatible chat completions endpoint URL for a running llama.cpp server.
pub fn chat_completions_url(port: u16) -> String {
    format!("http://127.0.0.1:{}/v1/chat/completions", port)
}
