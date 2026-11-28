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
        // Use the model's own chat template. llama-server needs this for OpenAI
        // `tools` / `tool_calls` (supported since early-2025 builds; pin is b12414).
        "--jinja".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn argv_enables_jinja_for_tool_calls() {
        let argv = build_argv(LlamaCppConfig {
            program: Path::new("llama-server"),
            model_path: Path::new("/m/model.gguf"),
            port: 8081,
            n_gpu_layers: 99,
            n_ctx: 4096,
            flash_attn: true,
        });
        assert!(argv.iter().any(|a| a == "--jinja"));
        assert_eq!(argv.last().map(String::as_str), Some("-fa"));
    }
}
