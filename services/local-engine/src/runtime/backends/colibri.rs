//! colibri backend: the pack's bundled Python runs colibri's own
//! `openai_server.py` (CONTRACT-colibri-unkernel section 3). The pack layout is
//! `python/bin/python3` (Windows: `python/python.exe`) and
//! `colibri/c/openai_server.py`; the entrypoint the engine-pack resolver
//! returns is the bundled Python.

use std::path::{Path, PathBuf};

/// Engine-pack name.
pub const ENGINE: &str = "colibri";
/// The only host colibri may bind to.
pub const HOST: &str = "127.0.0.1";

/// Server script, relative to the pack directory.
pub fn script_path(pack_dir: &Path) -> PathBuf {
    pack_dir.join("colibri").join("c").join("openai_server.py")
}

/// Configuration for launching colibri's OpenAI-compatible server.
#[derive(Debug, Clone)]
pub struct ColibriConfig<'a> {
    /// Bundled Python interpreter.
    pub program: &'a Path,
    /// `openai_server.py` inside the pack.
    pub script: &'a Path,
    /// Model directory passed as `--model`.
    pub model_dir: &'a Path,
    /// TCP port to bind.
    pub port: u16,
}

/// Build the argument vector (first element is the program).
pub fn build_argv(config: ColibriConfig<'_>) -> Vec<String> {
    vec![
        config.program.to_string_lossy().into_owned(),
        config.script.to_string_lossy().into_owned(),
        "--model".to_string(),
        config.model_dir.to_string_lossy().into_owned(),
        "--host".to_string(),
        HOST.to_string(),
        "--port".to_string(),
        config.port.to_string(),
    ]
}

/// Extra environment: `COLI_METAL=1` only when the recipe asks for Metal.
pub fn extra_env(metal: bool) -> Vec<(&'static str, &'static str)> {
    if metal {
        vec![("COLI_METAL", "1")]
    } else {
        Vec::new()
    }
}

/// Health endpoint URL (`GET /health`, 200 when ready).
pub fn health_url(port: u16) -> String {
    format!("http://{}:{}/health", HOST, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_runs_openai_server_on_loopback() {
        let program = Path::new("/p/python/bin/python3");
        let script = script_path(Path::new("/p"));
        let argv = build_argv(ColibriConfig {
            program,
            script: &script,
            model_dir: Path::new("/m/dir"),
            port: 4242,
        });
        assert_eq!(
            argv,
            vec![
                "/p/python/bin/python3",
                "/p/colibri/c/openai_server.py",
                "--model",
                "/m/dir",
                "--host",
                "127.0.0.1",
                "--port",
                "4242",
            ]
        );
        assert!(extra_env(false).is_empty());
        assert_eq!(extra_env(true), vec![("COLI_METAL", "1")]);
        assert_eq!(health_url(4242), "http://127.0.0.1:4242/health");
    }
}
