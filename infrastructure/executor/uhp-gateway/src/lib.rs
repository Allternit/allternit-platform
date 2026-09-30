//! uhp-gateway — a UHP (Unified Harness Protocol) 2026-08-11 "full"-class HTTP
//! server that turns prompts into real CLI-agent turns by driving the ao
//! engine (herdr) over its Unix-domain-socket NDJSON JSON-RPC API.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;

pub mod drivers;
pub mod engine;
pub mod protocol;
pub mod routes;
pub mod spawn_gate;
pub mod store;
pub mod turn;

pub use protocol::PROTOCOL_VERSION;

use engine::EngineClient;
use protocol::{BackendModels, Model, ModelCatalog};
use store::Store;
use turn::TurnControl;

pub struct AppState {
    pub store: Store,
    pub engine: EngineClient,
    pub token: String,
    pub data_dir: PathBuf,
    /// response_id → live turn control (cancel handle).
    pub turns: Arc<Mutex<std::collections::HashMap<String, Arc<TurnControl>>>>,
}

pub struct ServeConfig {
    pub bind: SocketAddr,
    pub token: String,
    pub data_dir: PathBuf,
    pub engine_socket: PathBuf,
}

pub async fn serve(cfg: ServeConfig) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !cfg.engine_socket.exists() {
        return Err(format!(
            "engine socket {} not found (set HERDR_SOCKET_PATH or --engine-socket; start the engine first)",
            cfg.engine_socket.display()
        )
        .into());
    }
    std::fs::create_dir_all(&cfg.data_dir)?;
    let store = Store::open(&cfg.data_dir.join("uhp.db")).await?;
    let state = Arc::new(AppState {
        store,
        engine: EngineClient::new(cfg.engine_socket.clone()),
        token: cfg.token,
        data_dir: cfg.data_dir.clone(),
        turns: Arc::new(Mutex::new(std::collections::HashMap::new())),
    });
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    tracing::info!(%cfg.bind, engine_socket = %cfg.engine_socket.display(), "uhp-gateway listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Static model catalog. `available` is computed, not asserted: a model is
/// available when its CLI binary is on PATH (cached for process lifetime).
pub fn model_catalog() -> ModelCatalog {
    let mut backends = BTreeMap::new();
    backends.insert(
        "kimi".to_string(),
        backend_models(
            "kimi",
            "kimi-k2.7-code",
            &["kimi-k2.7-code", "kimi-k3"],
        ),
    );
    backends.insert(
        "claude-code".to_string(),
        backend_models(
            "claude-code",
            "claude-sonnet-4-6",
            &["claude-sonnet-4-6", "claude-opus-4-6", "claude-haiku-4-5"],
        ),
    );
    backends.insert(
        "codex".to_string(),
        backend_models("codex", "gpt-5-codex", &["gpt-5-codex", "gpt-5", "codex-mini-latest"]),
    );
    backends.insert(
        "gemini".to_string(),
        backend_models("gemini", "gemini-2.5-pro", &["gemini-2.5-pro", "gemini-2.5-flash"]),
    );
    backends.insert(
        "qwen".to_string(),
        backend_models("qwen", "qwen3-coder", &["qwen3-coder", "qwen3-coder-plus"]),
    );
    backends.insert(
        "opencode".to_string(),
        backend_models("opencode", "opencode/gpt-5", &["opencode/gpt-5", "opencode/claude-sonnet-4"]),
    );
    backends.insert(
        "cline".to_string(),
        backend_models("cline", "gpt-4o", &["gpt-4o", "gpt-4.1"]),
    );
    backends.insert(
        "pi".to_string(),
        backend_models("pi", "sonnet", &["sonnet", "opus", "gpt-5"]),
    );
    backends.insert(
        "dsh".to_string(),
        backend_models("dsh", "deepseek-chat", &["deepseek-chat", "deepseek-reasoner"]),
    );
    ModelCatalog { backends }
}

fn backend_models(backend: &str, default: &str, ids: &[&str]) -> BackendModels {
    let available = binary_available(binary_for_backend(backend));
    BackendModels {
        default: default.to_string(),
        models: ids
            .iter()
            .map(|id| Model {
                id: id.to_string(),
                label: Some(id.to_string()),
                backend: Some(backend.to_string()),
                available,
                default: Some(*id == default),
            })
            .collect(),
    }
}

pub fn binary_for_backend(base: &str) -> &'static str {
    drivers::DriverKind::from_base(base)
        .map(|kind| kind.binary())
        .unwrap_or("")
}

pub fn binary_available(binary: &str) -> bool {
    if binary.is_empty() {
        return false;
    }
    static CACHE: std::sync::OnceLock<std::collections::HashMap<String, bool>> =
        std::sync::OnceLock::new();
    CACHE
        .get_or_init(scan_path_binaries)
        .get(binary)
        .copied()
        .unwrap_or(false)
}

fn scan_path_binaries() -> std::collections::HashMap<String, bool> {
    let mut map = std::collections::HashMap::new();
    let Some(paths) = std::env::var_os("PATH") else {
        return map;
    };
    for binary in [
        "kimi", "claude", "codex", "gemini", "qwen", "opencode", "cline", "pi", "dsh",
    ] {
        let found = std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(binary);
            std::fs::metadata(&candidate)
                .map(|meta| {
                    meta.is_file()
                        && {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                meta.permissions().mode() & 0o111 != 0
                            }
                            #[cfg(not(unix))]
                            {
                                true
                            }
                        }
                })
                .unwrap_or(false)
        });
        map.insert(binary.to_string(), found);
    }
    map
}

/// Resolve the model that will actually run. A requested model the catalog
/// does not serve is substituted for the backend default and reported via
/// `metadata.requested_model` + `model_fallback` (T-03).
pub fn resolve_model(
    base: &str,
    requested: Option<&str>,
) -> (String, Option<String>, Option<bool>) {
    let catalog = model_catalog();
    let default = catalog
        .backends
        .get(base)
        .map(|backend| backend.default.clone())
        .unwrap_or_else(|| "default".to_string());
    match requested {
        None => (default, None, None),
        Some(model) if catalog
            .backends
            .get(base)
            .map(|backend| backend.models.iter().any(|entry| entry.id == model))
            .unwrap_or(false) =>
        {
            (model.to_string(), None, None)
        }
        Some(model) => (default, Some(model.to_string()), Some(true)),
    }
}

#[derive(Debug, Serialize)]
pub struct DeleteAck {
    pub id: String,
    pub object: String,
    pub deleted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_shape() {
        let catalog = model_catalog();
        assert!(catalog.backends.contains_key("kimi"));
        assert!(catalog.backends.contains_key("claude-code"));
        assert!(catalog.backends.contains_key("codex"));
        for extra in ["gemini", "qwen", "opencode", "cline", "pi", "dsh"] {
            assert!(catalog.backends.contains_key(extra), "missing backend {extra}");
        }
        for backend in catalog.backends.values() {
            assert!(!backend.default.is_empty());
            assert!(!backend.models.is_empty());
            for model in &backend.models {
                assert!(!model.id.is_empty());
            }
        }
    }

    #[test]
    fn model_resolution() {
        let (final_model, requested, fallback) = resolve_model("kimi", None);
        assert_eq!(final_model, "kimi-k2.7-code");
        assert_eq!(requested, None);
        assert_eq!(fallback, None);

        let (final_model, requested, fallback) = resolve_model("kimi", Some("kimi-k3"));
        assert_eq!(final_model, "kimi-k3");
        assert_eq!(requested, None); // honored request: not reported as substitution
        assert_eq!(fallback, None);

        let (final_model, requested, fallback) = resolve_model("kimi", Some("no-such-model"));
        assert_eq!(final_model, "kimi-k2.7-code");
        assert_eq!(requested.as_deref(), Some("no-such-model"));
        assert_eq!(fallback, Some(true));
    }
}
