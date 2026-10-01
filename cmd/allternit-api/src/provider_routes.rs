//! Provider API routes — LLM provider discovery and management.

use axum::extract::Extension;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashSet, sync::Arc};
use tracing::warn;

use crate::auth::get_user;
use crate::auth::AuthUser;
use crate::config::gizzi_user_config_path;
use crate::AppState;

fn unauthorized() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "Unauthorized"})),
    )
        .into_response()
}

fn db_error(e: impl std::fmt::Display) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "Database error", "details": e.to_string()})),
    )
        .into_response()
}

pub fn provider_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/providers", get(list_providers))
        .route("/providers/:id", get(get_provider))
        .route("/providers/:id/auth/status", get(get_provider_auth_status))
        .route("/providers/:id/quota", get(get_provider_quota))
        .route("/providers/:id/models", get(discover_provider_models))
        .route("/providers/:id/connect", post(connect_provider))
        .route(
            "/providers/:id/connect/status",
            get(connect_provider_status),
        )
        .route(
            "/providers/:id/connect/confirm",
            post(confirm_provider_connect),
        )
        .route("/providers/:id/install", post(install_provider_tool))
        .route(
            "/providers/:id/install/status",
            get(install_provider_tool_status),
        )
        .route("/providers/auth/status", get(list_provider_auth_status))
        .route("/providers/video/generate", post(generate_video))
        .route("/media/catalog", get(crate::media::handlers::get_media_catalog))
        .route(
            "/media/video/jobs",
            post(crate::media::handlers::submit_video_job),
        )
        .route(
            "/media/video/jobs/:job_id",
            get(crate::media::handlers::get_video_job),
        )
        .route(
            "/media/video/jobs/:job_id/download",
            get(crate::media::handlers::download_video_job),
        )
        .route(
            "/media/image/generate",
            post(crate::media::handlers::generate_image).layer(
                axum::extract::DefaultBodyLimit::max(
                    crate::media::handlers::MAX_IMAGE_GENERATE_BODY_BYTES,
                ),
            ),
        )
        .route(
            "/media/artifacts",
            post(crate::media::handlers::upload_media_artifact).layer(
                axum::extract::DefaultBodyLimit::max(crate::media::handlers::MAX_UPLOAD_BYTES),
            ),
        )
        .route(
            "/media/artifacts/:artifact_id",
            get(crate::media::handlers::get_media_artifact),
        )
        .route("/media/providers", get(list_media_providers))
        .route("/media/:mode/providers", get(list_media_providers_for_mode))
        .route("/media/:mode/generate", post(generate_media))
        .route("/provider/ollama/status", get(ollama_live_status))
        .route("/provider/ollama/models", get(list_ollama_models))
        .route("/provider/huggingface/search", get(search_huggingface))
}

async fn generate_video(headers: HeaderMap, Json(payload): Json<serde_json::Value>) -> Response {
    let _user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match crate::gizzi_provider_auth::generate_video(payload).await {
        Ok((status, payload)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(payload),
        )
            .into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "gizzi_provider_error", "message": error })),
        )
            .into_response(),
    }
}

async fn list_media_providers(headers: HeaderMap) -> Response {
    let _user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match crate::gizzi_provider_auth::list_media_providers().await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "gizzi_provider_error", "message": error })),
        )
            .into_response(),
    }
}

async fn list_media_providers_for_mode(
    headers: HeaderMap,
    Path(mode): Path<String>,
) -> Response {
    let _user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match crate::gizzi_provider_auth::list_media_providers_for_mode(mode).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "gizzi_provider_error", "message": error })),
        )
            .into_response(),
    }
}

async fn generate_media(
    headers: HeaderMap,
    Path(mode): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    let _user = match get_user(&headers) {
        Some(user) => user,
        None => return unauthorized(),
    };
    match crate::gizzi_provider_auth::generate_media(mode, payload).await {
        Ok((status, payload)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            Json(payload),
        )
            .into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "gizzi_provider_error", "message": error })),
        )
            .into_response(),
    }
}

/// Probe Ollama live on the blocking thread pool and return the live status.
async fn ollama_live_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let url = state.config.ollama_url();
    let probe = tokio::task::spawn_blocking(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(4))
            .build()
        {
            Ok(c) => c,
            Err(_) => return (false, Vec::<String>::new()),
        };
        let probe = client
            .get(format!("{}/api/tags", url.trim_end_matches('/')))
            .send();
        match probe {
            Ok(res) if res.status().is_success() => {
                let models = res
                    .json::<serde_json::Value>()
                    .ok()
                    .and_then(|b| b.get("models").and_then(|m| m.as_array()).cloned())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                m.get("name")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (true, models)
            }
            _ => (false, vec![]),
        }
    })
    .await;

    match probe {
        Ok((running, models)) => Json(json!({
            "running": running,
            "models": models,
        }))
        .into_response(),
        Err(_) => Json(json!({ "running": false, "models": Vec::<String>::new() })).into_response(),
    }
}

/// Probe a local OpenAI-compatible brain (Ollama/LM Studio/Local Engine) for
/// reachability and its installed models. Cheap local GET with a short timeout;
/// never throws. Tries the Ollama `/api/tags` endpoint first, then falls back
/// to the OpenAI-compatible `/v1/models` endpoint.
async fn probe_local_brain(url: &str) -> (bool, Vec<String>) {
    let url = url.trim_end_matches('/').to_string();
    tokio::task::spawn_blocking(move || {
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(4))
            .build()
        {
            Ok(c) => c,
            Err(_) => return (false, Vec::<String>::new()),
        };

        // Ollama-style endpoint.
        if let Ok(res) = client.get(format!("{}/api/tags", url)).send() {
            if res.status().is_success() {
                let models = res
                    .json::<serde_json::Value>()
                    .ok()
                    .and_then(|b| b.get("models").and_then(|m| m.as_array()).cloned())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                m.get("name")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return (true, models);
            }
        }

        // OpenAI-compatible endpoint (Local Engine, LM Studio, vLLM, etc.).
        if let Ok(res) = client.get(format!("{}/v1/models", url)).send() {
            if res.status().is_success() {
                let models = res
                    .json::<serde_json::Value>()
                    .ok()
                    .and_then(|b| b.get("data").and_then(|d| d.as_array()).cloned())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                m.get("id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return (true, models);
            }
        }

        (false, vec![])
    })
    .await
    .unwrap_or((false, vec![]))
}

// ─── Data models ──────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
struct ProviderRow {
    id: String,
    name: String,
    provider_type: String,
    base_url: Option<String>,
    api_key_set: bool,
    models: Vec<String>,
    status: String,
}

/// Frontend-facing provider shape returned by `GET /api/v1/providers`.
#[derive(Serialize, Clone)]
struct ProviderInfo {
    id: String,
    name: String,
    provider_type: String,
    base_url: Option<String>,
    api_key_set: bool,
    models: Vec<ModelInfo>,
    status: String,
}

/// Frontend-facing model shape returned by `GET /api/v1/providers`.
#[derive(Serialize, Clone)]
struct ModelInfo {
    id: String,
    name: String,
}

/// Static specs for ENV-key providers: (id, display name, api-key env var, models).
/// Shared by env_provider_rows() and available_model_catalog() so the two never drift.
static ENV_PROVIDER_SPECS: &[(&str, &str, &str, &[&str])] = &[
    (
        "openai",
        "OpenAI",
        "OPENAI_API_KEY",
        &["gpt-5", "gpt-5-mini", "gpt-4o", "dall-e-3"],
    ),
    (
        "anthropic",
        "Anthropic",
        "ANTHROPIC_API_KEY",
        &["claude-sonnet-5", "claude-opus-5"],
    ),
    (
        "google",
        "Google AI",
        "GOOGLE_GENERATIVE_AI_API_KEY",
        &["gemini-2.5-pro", "gemini-2.5-flash"],
    ),
    ("alibaba", "Alibaba", "ALIBABA_API_KEY", &["qwen-max", "qwen-plus"]),
    (
        "amazon-bedrock",
        "Amazon Bedrock",
        "AWS_SECRET_ACCESS_KEY",
        &["nova-pro"],
    ),
    ("groq", "Groq", "GROQ_API_KEY", &["llama-3-70b"]),
    ("mistral", "Mistral", "MISTRAL_API_KEY", &["mistral-large"]),
    ("cohere", "Cohere", "COHERE_API_KEY", &["command-r-plus"]),
    (
        "deepseek",
        "DeepSeek",
        "DEEPSEEK_API_KEY",
        &["deepseek-chat"],
    ),
    ("xai", "xAI", "XAI_API_KEY", &["grok-3"]),
    (
        "togetherai",
        "Together AI",
        "TOGETHER_API_KEY",
        &["llama-3-70b"],
    ),
    (
        "perplexity",
        "Perplexity",
        "PERPLEXITY_API_KEY",
        &["sonar-pro"],
    ),
];

/// Static specs for CLI/subprocess brains: (id, display name, binary, default model).
/// IDs and default models must match what the gizzi runtime registers
/// (cmd/gizzi-code src/runtime/providers/discovery/subprocess.ts).
/// This list mirrors the agent-runtime surface used by Multica: the user brings
/// their own installed + authenticated CLI tool, and Allternit routes to it.
static CLI_PROVIDER_SPECS: &[(&str, &str, &str, &str)] = &[
    ("claude-cli", "Claude CLI", "claude", "claude-sonnet-5"),
    ("codex-cli", "Codex CLI", "codex", "gpt-6-astra"),
    ("qwen-cli", "Qwen CLI", "qwen", "qwen-plus"),
    ("kimi-cli", "Kimi CLI", "kimi", "kimi-k3"),
    ("antigravity", "Antigravity", "agy", "antigravity"),
    ("cursor-agent", "Cursor Agent", "cursor-agent", "cursor-agent"),
    ("copilot", "GitHub Copilot CLI", "copilot", "copilot"),
    ("opencode", "OpenCode", "opencode", "opencode"),
    ("droid", "Droid (Factory)", "droid", "droid"),
    ("gemini-cli", "Gemini CLI", "gemini", "gemini-cli"),
    ("openclaw", "OpenClaw", "openclaw", "openclaw"),
    ("hermes", "Hermes", "hermes", "hermes"),
    ("pi", "Pi", "pi", "pi"),
    ("codebuddy", "CodeBuddy", "codebuddy", "codebuddy"),
    ("deveco", "DevEco Code", "deveco", "deveco"),
    ("grok", "Grok", "grok", "grok"),
    ("kiro-cli", "Kiro CLI", "kiro-cli", "kiro-cli"),
    ("qodercli", "Qoder CLI", "qodercli", "qodercli"),
    ("qoderclicn", "Qoder CN", "qoderclicn", "qoderclicn"),
    ("qwenpaw", "QwenPaw", "qwenpaw", "qwenpaw"),
    ("reasonix", "Reasonix", "reasonix", "reasonix"),
    ("traecli", "Trae CLI", "traecli", "traecli"),
    ("dsh", "DeepSeek Harness", "dsh", "dsh"),
    ("omp", "Oh-My-Pi", "omp", "omp"),
    ("mcode", "MiniMax Code", "mcode", "mcode"),
    ("dim", "Dim", "dim", "dim"),
];

fn cli_models_for<'a>(id: &str, default_model: &'a str) -> Vec<&'a str> {
    match id {
        "codex-cli" => vec!["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"],
        _ => vec![default_model],
    }
}

/// Per-model display metadata: (model id, description, tier, supports_effort).
/// Tier is flagship | standard | fast | legacy and drives picker grouping on
/// clients (the Claude-app sheet layout). Unknown models default to
/// (no description, "standard", false).
static MODEL_METADATA: &[(&str, &str, &str, bool)] = &[
    ("claude-opus-5", "For your toughest challenges", "flagship", true),
    ("claude-sonnet-5", "Most efficient for everyday tasks", "standard", true),
    ("gpt-5", "Latest GPT reasoning", "flagship", false),
    ("gpt-5-mini", "Fast GPT mini", "fast", false),
    ("gpt-4o", "Prior-generation flagship", "legacy", true),
    ("gpt-6-astra", "Astra — strongest Codex agent", "flagship", false),
    ("gpt-5.6-sol", "Sol — complex open-ended work", "flagship", false),
    ("gpt-5.6-terra", "Terra — everyday coding", "standard", false),
    ("gpt-5.6-luna", "Luna — fast high-volume tasks", "fast", false),
    ("gemini-2.5-pro", "Long-context reasoning", "flagship", false),
    ("gemini-2.5-flash", "Fastest for quick answers", "fast", false),
    ("sonar-pro", "Web-grounded answers", "standard", false),
    ("kimi-k3", "Kimi K3 reasoning", "flagship", false),
    ("kimi-k2.7-code", "Kimi coding specialist", "standard", false),
    ("qwen-max", "Qwen Max", "flagship", false),
    ("qwen-plus", "Qwen Plus", "standard", false),
];

fn model_metadata(model: &str) -> Option<(&'static str, &'static str, bool)> {
    MODEL_METADATA
        .iter()
        .find(|(id, _, _, _)| *id == model)
        .map(|(_, desc, tier, effort)| (*desc, *tier, *effort))
}

/// Flattened `{id, name, provider, description?, tier, supports_effort}`
/// catalog for the agent-creation wizard's model picker (GET /api/v1/models).
/// Derived from the same provider specs used by the /providers endpoints so
/// both stay in sync. `id` uses the `provider/model` convention the runtime
/// harness resolves.
pub fn available_model_catalog() -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut push = |id: &str, name: String, provider: &str, model: &str| {
        let (description, tier, supports_effort) = model_metadata(model)
            .map(|(d, t, e)| (Some(d), t, e))
            .unwrap_or((None, "standard", false));
        out.push(json!({
            "id": id,
            "name": name,
            "provider": provider,
            "description": description,
            "tier": tier,
            "supports_effort": supports_effort,
        }));
    };
    for &(id, name, _binary, model) in CLI_PROVIDER_SPECS {
        for model_id in cli_models_for(id, model) {
            push(
                &format!("{}/{}", id, model_id),
                format!("{} ({})", model_id, name),
                id,
                model_id,
            );
        }
    }
    for &(id, name, _env, models) in ENV_PROVIDER_SPECS {
        for model in models {
            push(&format!("{}/{}", id, model), format!("{} ({})", model, name), id, model);
        }
    }
    out
}

#[derive(Serialize)]
struct ProviderAuthStatusRow {
    provider_id: String,
    status: String,
    authenticated: bool,
    auth_required: bool,
    auth_profile_id: Option<String>,
    chat_profile_ids: Vec<String>,
    details: Option<serde_json::Value>,
}

fn compute_provider_status(provider_type: &str, api_key_env_var: Option<&str>) -> String {
    let key_set = api_key_env_var.map_or(false, |var| std::env::var(var).is_ok());
    if key_set {
        "active".to_string()
    } else if provider_type == "local" {
        "unknown".to_string()
    } else {
        "unconfigured".to_string()
    }
}

/// Best-effort check of whether a command exists on PATH.
pub(crate) fn command_on_path(cmd: &str) -> Option<std::path::PathBuf> {
    // allternit-tools installs into ~/.allternit/tools/bin; a Finder-launched
    // runtime may not have it on PATH yet, so look there first.
    let own = crate::tools_install::tools_bin_dir().join(cmd);
    if own.is_file() {
        return Some(own);
    }
    let path_env = std::env::var("PATH").unwrap_or_default();
    for dir in path_env.split(':') {
        let candidate = std::path::Path::new(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Read providers that the user has configured through the Gizzi runtime config
/// (the same file the chat harness reads). This keeps the UI in sync with
/// whatever brain the runtime will actually use.
fn read_gizzi_providers(connected: &HashSet<String>) -> Vec<ProviderRow> {
    let path = gizzi_user_config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let providers = match value.get("provider").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => return Vec::new(),
    };

    providers
        .iter()
        .map(|(id, config)| {
            let name = config
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(id)
                .to_string();
            let auth_type = config
                .get("auth_type")
                .and_then(|v| v.as_str())
                .unwrap_or("api_key");
            let provider_type =
                if config.get("subprocess_cmd").is_some() || auth_type == "subprocess" {
                    "subprocess"
                } else if config
                    .get("options")
                    .and_then(|o| o.get("baseURL"))
                    .is_some()
                    || auth_type == "local"
                {
                    "local"
                } else {
                    "api"
                }
                .to_string();

            let base_url = config
                .get("options")
                .and_then(|o| o.get("baseURL"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let models: Vec<String> = config
                .get("models")
                .and_then(|m| m.as_object())
                .map(|m| m.keys().map(|k| k.to_string()).collect())
                .unwrap_or_default();

            let env_var = format!("{}_API_KEY", id.to_uppercase().replace('-', "_"));
            let api_key_set = connected.contains(id) || std::env::var(&env_var).is_ok();

            let status = if provider_type == "subprocess" {
                if let Some(cmd) = config.get("subprocess_cmd").and_then(|v| v.as_str()) {
                    let program = cmd.split_whitespace().next().unwrap_or(cmd);
                    if command_on_path(program).is_some() {
                        if api_key_set {
                            "active".to_string()
                        } else {
                            "missing_key".to_string()
                        }
                    } else {
                        "offline".to_string()
                    }
                } else {
                    "unconfigured".to_string()
                }
            } else if provider_type == "local" {
                "unknown".to_string()
            } else if api_key_set {
                "active".to_string()
            } else {
                "unconfigured".to_string()
            };

            ProviderRow {
                id: id.clone(),
                name,
                provider_type,
                base_url,
                api_key_set,
                models,
                status,
            }
        })
        .collect()
}

/// Providers that are automatically registered when their API key env var is
/// present. This lets users add a brain by setting a single env var and having
/// it route through the Gizzi harness without touching a database.
fn env_provider_rows() -> Vec<ProviderRow> {
    ENV_PROVIDER_SPECS
        .iter()
        .map(|(id, name, env_var, models)| {
            let key_set = std::env::var(env_var).is_ok();
            ProviderRow {
                id: id.to_string(),
                name: name.to_string(),
                provider_type: "api".to_string(),
                base_url: None,
                api_key_set: key_set,
                models: models.iter().map(|m| m.to_string()).collect(),
                status: if key_set {
                    "active".to_string()
                } else {
                    "unconfigured".to_string()
                },
            }
        })
        .collect()
}

/// A virtual provider entry for Ollama. The frontend probes Ollama's actual
/// availability via the dedicated `/api/local-brain` endpoint, so this entry is
/// just a static marker that the local brain exists in the provider list.
fn ollama_provider_row(ollama_url: String) -> ProviderRow {
    ProviderRow {
        id: "ollama".to_string(),
        name: "Ollama".to_string(),
        provider_type: "local".to_string(),
        base_url: Some(ollama_url),
        api_key_set: false,
        models: vec![],
        status: "ready".to_string(),
    }
}

/// Virtual providers for common CLI subprocess brains. Status reflects whether
/// the binary and its provider-owned authentication are available.
fn subprocess_provider_rows(connected: &HashSet<String>) -> Vec<ProviderRow> {
    let mut rows = Vec::new();

    // CLI/subprocess brains authenticated via subscription OAuth (the CLI holds
    // the token itself) or, for Claude/Codex, a stored API key. We reuse the same
    // detection as the one-click connect flow so the merged status never reports a
    // signed-in CLI as "missing key" (root-cause correctness, not a fallback).
    for &(id, name, binary, model) in CLI_PROVIDER_SPECS {
        let available = command_on_path(binary).is_some();
        // Gizzi's `connected` list is "binary on PATH", not a live OAuth
        // session. Claude's `auth status` exits 0 even when loggedIn is
        // false — only subscription_auth_check parses that.
        let authed = match id {
            "claude" | "claude-cli" => subscription_auth_check(id, binary),
            "codex" | "codex-cli" => {
                connected.contains(id)
                    || subscription_auth_check(id, binary)
                    || std::env::var("OPENAI_API_KEY").is_ok()
            }
            _ => connected.contains(id) || subscription_auth_check(id, binary),
        };
        rows.push(ProviderRow {
            id: id.to_string(),
            name: name.to_string(),
            provider_type: "subprocess".to_string(),
            base_url: None,
            api_key_set: authed,
            models: cli_models_for(id, model)
                .into_iter()
                .map(str::to_string)
                .collect(),
            status: if available {
                if authed {
                    "active"
                } else {
                    "missing_key"
                }
            } else {
                "offline"
            }
            .to_string(),
        });
    }

    // Z.ai (GLM Coding Plan) is API-key based — no public OAuth yet.
    let zai_authed = connected.contains("zai") || subscription_auth_check("zai", "zai");
    rows.push(ProviderRow {
        id: "zai".to_string(),
        name: "Z.ai".to_string(),
        provider_type: "api".to_string(),
        base_url: Some("https://api.z.ai/api/coding/paas/v4".to_string()),
        api_key_set: zai_authed,
        models: vec!["glm-4.6".to_string()],
        status: if zai_authed { "active" } else { "missing_key" }.to_string(),
    });

    rows
}

fn row_to_provider(row: &rusqlite::Row) -> Result<ProviderRow, rusqlite::Error> {
    let id: String = row.get(0)?;
    let name: String = row.get(1)?;
    let provider_type: String = row.get(2)?;
    let base_url: Option<String> = row.get(3)?;
    let api_key_env_var: Option<String> = row.get(4)?;
    let models_json: Option<String> = row.get(5)?;

    let models: Vec<String> = models_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let api_key_set = api_key_env_var
        .as_deref()
        .map_or(false, |var| std::env::var(var).is_ok());
    let status = compute_provider_status(&provider_type, api_key_env_var.as_deref());

    Ok(ProviderRow {
        id,
        name,
        provider_type,
        base_url,
        api_key_set,
        models,
        status,
    })
}

/// Merge multiple provider sources. Later sources override earlier ones by ID.
fn merge_provider_sources(sources: Vec<Vec<ProviderRow>>) -> Vec<ProviderRow> {
    let mut map = std::collections::HashMap::new();
    for source in sources {
        for row in source {
            // Merge model lists when overriding so we don't drop statically-known models.
            map.entry(row.id.clone())
                .and_modify(|existing: &mut ProviderRow| {
                    existing.name = row.name.clone();
                    existing.provider_type = row.provider_type.clone();
                    if row.base_url.is_some() {
                        existing.base_url = row.base_url.clone();
                    }
                    if row.api_key_set {
                        existing.api_key_set = true;
                    }
                    let mut merged_models: std::collections::HashSet<String> =
                        existing.models.iter().cloned().collect();
                    merged_models.extend(row.models.iter().cloned());
                    existing.models = merged_models.into_iter().collect();
                    // Keep the most optimistic status.
                    if row.status == "active" || existing.status == "unconfigured" {
                        existing.status = row.status.clone();
                    }
                })
                .or_insert(row);
        }
    }
    map.into_values().collect()
}

/// Parse the `connected` array from a Gizzi `/provider` response.
fn parse_connected(payload: &serde_json::Value) -> HashSet<String> {
    payload
        .get("connected")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

/// Transform one Gizzi provider object into the frontend `ProviderInfo` shape.
fn provider_info_from_gizzi(
    provider: &serde_json::Value,
    connected: &HashSet<String>,
) -> Option<ProviderInfo> {
    let id = provider.get("id")?.as_str()?.to_owned();
    let name = provider
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_owned();
    let auth_type = provider
        .get("auth_type")
        .and_then(|v| v.as_str())
        .unwrap_or("api_key");

    let provider_type = if auth_type == "subprocess" || provider.get("subprocess_cmd").is_some() {
        "subprocess"
    } else if auth_type == "none" {
        "local"
    } else {
        "api"
    }
    .to_owned();

    let base_url = provider
        .get("options")
        .and_then(|o| o.get("baseURL"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());

    let api_key_set = connected.contains(&id) || auth_type == "none";

    let models: Vec<ModelInfo> = provider
        .get("models")
        .and_then(|m| m.as_object())
        .map(|m| {
            m.values()
                .filter_map(|model| {
                    let obj = model.as_object()?;
                    let model_id = obj.get("id")?.as_str()?.to_owned();
                    let model_name = obj
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&model_id)
                        .to_owned();
                    Some(ModelInfo {
                        id: model_id,
                        name: model_name,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let status = if connected.contains(&id) || auth_type == "none" {
        "active"
    } else {
        "unconfigured"
    }
    .to_owned();

    Some(ProviderInfo {
        id,
        name,
        provider_type,
        base_url,
        api_key_set,
        models,
        status,
    })
}

/// Convert a legacy merged `ProviderRow` into the frontend `ProviderInfo` shape.
fn provider_info_from_row(row: &ProviderRow) -> ProviderInfo {
    let status = match row.status.as_str() {
        "active" => "active",
        "offline" => "offline",
        "unknown" => "unknown",
        "ready" | "ready_no_models" => "active",
        _ => "unconfigured",
    }
    .to_owned();

    ProviderInfo {
        id: row.id.clone(),
        name: row.name.clone(),
        provider_type: row.provider_type.clone(),
        base_url: row.base_url.clone(),
        api_key_set: row.api_key_set,
        models: row
            .models
            .iter()
            .map(|m| ModelInfo {
                id: m.clone(),
                name: m.clone(),
            })
            .collect(),
        status,
    }
}

// ─── List providers ───────────────────────────────────────────────────────────

async fn list_providers(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Prefer live Gizzi-discovered providers; fall back to the static merge if
    // the runtime is unreachable so the UI never renders an empty list.
    match crate::gizzi_provider_auth::discover_providers().await {
        Ok(payload) => {
            let connected = parse_connected(&payload);
            let providers: Vec<ProviderInfo> = payload
                .get("all")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|provider| provider_info_from_gizzi(provider, &connected))
                .collect();
            return Json(json!({ "providers": providers, "all": providers })).into_response();
        }
        Err(error) => {
            warn!("Gizzi provider discovery failed, falling back to static providers: {}", error);
        }
    }

    // Scoped so the !Send rusqlite handles drop before any `.await` below.
    let db_providers: Vec<ProviderRow> = {
        let conn = match state.db.connect() {
            Ok(c) => c,
            Err(e) => return db_error(e),
        };
        let mut stmt = match conn.prepare(
            "SELECT id, name, provider_type, base_url, api_key_env_var, models FROM providers ORDER BY name"
        ) {
            Ok(s) => s,
            Err(e) => return db_error(e),
        };
        let rows = match stmt.query_map([], row_to_provider) {
            Ok(r) => r,
            Err(e) => return db_error(e),
        };
        match rows.collect::<Result<Vec<_>, _>>() {
            Ok(p) => p,
            Err(e) => return db_error(e),
        }
    };

    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;
    let ollama_row = ollama_provider_row(state.config.ollama_url());

    let mut providers = merge_provider_sources(vec![
        db_providers,
        env_provider_rows(),
        read_gizzi_providers(&connected),
        subprocess_provider_rows(&connected),
        vec![ollama_row],
    ]);

    // Tell the truth about local brains: probe reachability instead of reporting
    // the static "unknown" marker. Only hits local providers (Ollama/LM Studio).
    for p in providers.iter_mut() {
        if p.provider_type == "local" {
            if let Some(url) = p.base_url.clone() {
                let (running, models) = probe_local_brain(&url).await;
                p.status = if running {
                    "active".to_string()
                } else {
                    "offline".to_string()
                };
                if running && p.models.is_empty() {
                    p.models = models;
                }
            }
        }
    }

    let providers: Vec<ProviderInfo> = providers
        .iter()
        .map(provider_info_from_row)
        .collect();
    Json(json!({ "providers": providers, "all": providers })).into_response()
}

// ─── Get provider ─────────────────────────────────────────────────────────────

async fn get_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };
    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;

    // Try DB first, then fall back to merged virtual providers.
    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };

    let db_provider: Option<ProviderRow> = match conn.query_row(
        "SELECT id, name, provider_type, base_url, api_key_env_var, models FROM providers WHERE id = ?1",
        rusqlite::params![id],
        row_to_provider,
    ) {
        Ok(p) => Some(p),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return db_error(e),
    };

    let provider = if let Some(p) = db_provider {
        p
    } else {
        let ollama_row = ollama_provider_row(state.config.ollama_url());
        let all = merge_provider_sources(vec![
            env_provider_rows(),
            read_gizzi_providers(&connected),
            subprocess_provider_rows(&connected),
            vec![ollama_row],
        ]);
        match all.into_iter().find(|p| p.id == id) {
            Some(p) => p,
            None => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))).into_response()
            }
        }
    };

    Json(json!({ "provider": provider })).into_response()
}

// ─── List Ollama models ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct OllamaModel {
    name: String,
    size: Option<u64>,
    parameter_size: Option<String>,
    quantization_level: Option<String>,
    digest: Option<String>,
    modified_at: Option<String>,
}

async fn list_ollama_models(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    let ollama_url = state.config.ollama_url();

    let client = reqwest::Client::new();
    match client
        .get(format!("{}/api/tags", ollama_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(res) => {
            if let Ok(body) = res.json::<serde_json::Value>().await {
                // Ollama returns { models: [...] }
                let models: Vec<OllamaModel> = body
                    .get("models")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                Some(OllamaModel {
                                    name: m.get("name")?.as_str()?.to_string(),
                                    size: m.get("size").and_then(|v| v.as_u64()),
                                    parameter_size: m
                                        .get("details")
                                        .and_then(|d| d.get("parameter_size"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    quantization_level: m
                                        .get("details")
                                        .and_then(|d| d.get("quantization_level"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    digest: m
                                        .get("digest")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    modified_at: m
                                        .get("modified_at")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Json(json!({ "models": models })).into_response()
            } else {
                Json(json!({"models": [], "note": "Failed to parse Ollama response"}))
                    .into_response()
            }
        }
        Err(e) => {
            warn!("Ollama discovery failed: {}", e);
            Json(json!({
                "models": [],
                "note": "Ollama not reachable",
                "url": ollama_url,
            }))
            .into_response()
        }
    }
}

// ─── Provider authentication status ───────────────────────────────────────────

// Single API-side source of truth for provider capabilities + limits.
// The Gizzi runtime keeps its own models.dev registry for actual routing; this
// is what the platform UI consumes via /api/v1/providers/* so the frontend and
// any future fallback logic read from one place instead of scattered constants.
fn provider_capabilities(id: &str) -> serde_json::Value {
    let (tool_call, vision, context, output, default_model): (bool, bool, u64, u64, &str) = match id
    {
        "anthropic" => (true, true, 200_000, 32_000, "claude-sonnet-5"),
        "openai" => (true, true, 128_000, 16_384, "gpt-5"),
        "google" => (true, true, 1_000_000, 65_536, "gemini-2.5-pro"),
        "ollama" | "lmstudio" => (true, false, 128_000, 16_384, "llama3.2:3b"),
        "claude-cli" => (true, true, 200_000, 32_000, "claude-sonnet-5"),
        "codex-cli" => (true, false, 1_050_000, 128_000, "gpt-6-astra"),
        "qwen" => (true, false, 128_000, 16_384, "qwen-plus"),
        "kimi" => (true, false, 128_000, 16_384, "kimi-k3"),
        "antigravity" | "agy" => (true, true, 1_000_000, 65_536, "gemini-2.5-pro"),
        "zai" | "z.ai" | "glm" => (true, false, 200_000, 16_384, "glm-4.6"),
        _ => (true, false, 128_000, 16_384, ""),
    };
    json!({
        "tool_call": tool_call,
        "vision": vision,
        "limits": { "context": context, "output": output },
        "default_model": default_model,
    })
}

fn auth_status_from_row(row: &ProviderRow) -> ProviderAuthStatusRow {
    let is_local = row.provider_type == "local";
    let is_subprocess = row.provider_type == "subprocess";
    let auth_required = !is_local;

    let status = match row.status.as_str() {
        "active" => "ok",
        "missing_key" => "missing",
        "offline" => "unknown",
        "ready_no_models" => "not_required",
        _ if is_local => "not_required",
        _ if row.api_key_set => "ok",
        _ => "missing",
    }
    .to_string();

    let authenticated = status == "ok" || status == "not_required";

    ProviderAuthStatusRow {
        provider_id: row.id.clone(),
        status,
        authenticated,
        auth_required,
        auth_profile_id: auth_required.then(|| format!("{}-auth", row.id)),
        chat_profile_ids: if is_local || is_subprocess || row.api_key_set {
            vec![format!("{}-default", row.id)]
        } else {
            Vec::new()
        },
        details: Some(json!({
            "provider_type": row.provider_type,
            "base_url": row.base_url,
            "api_key_set": row.api_key_set,
            "model_count": row.models.len(),
            "capabilities": provider_capabilities(&row.id),
        })),
    }
}

/// Transform a Gizzi provider object into the frontend auth-status row shape.
fn auth_status_from_gizzi_provider(
    provider: &serde_json::Value,
    connected: &HashSet<String>,
) -> Option<ProviderAuthStatusRow> {
    let id = provider.get("id")?.as_str()?.to_owned();
    let auth_type = provider
        .get("auth_type")
        .and_then(|v| v.as_str())
        .unwrap_or("api_key");

    let provider_type = if auth_type == "subprocess" || provider.get("subprocess_cmd").is_some() {
        "subprocess"
    } else if auth_type == "none" {
        "local"
    } else {
        "api"
    }
    .to_owned();

    let model_count = provider
        .get("models")
        .and_then(|m| m.as_object())
        .map(|m| m.len())
        .unwrap_or(0);

    let auth_required = auth_type != "none";
    let authenticated = connected.contains(&id)
        || auth_type == "none"
        || (auth_type == "subprocess" && provider.get("subprocess_cmd").is_some());

    let status = if !auth_required {
        "not_required"
    } else if authenticated {
        "ok"
    } else {
        "missing"
    }
    .to_owned();

    Some(ProviderAuthStatusRow {
        provider_id: id,
        status,
        authenticated,
        auth_required,
        auth_profile_id: None,
        chat_profile_ids: Vec::new(),
        details: Some(json!({
            "provider_type": provider_type,
            "model_count": model_count,
        })),
    })
}

/// Convert a legacy merged `ProviderRow` into the live auth-status row shape.
fn auth_status_live_from_row(row: &ProviderRow) -> ProviderAuthStatusRow {
    let is_local = row.provider_type == "local";
    let auth_required = !is_local;

    let status = match row.status.as_str() {
        "active" => "ok",
        "missing_key" => "missing",
        "offline" => "unknown",
        "ready_no_models" => "not_required",
        _ if is_local => "not_required",
        _ if row.api_key_set => "ok",
        _ => "missing",
    }
    .to_string();

    let authenticated = status == "ok" || status == "not_required";

    ProviderAuthStatusRow {
        provider_id: row.id.clone(),
        status,
        authenticated,
        auth_required,
        auth_profile_id: None,
        chat_profile_ids: Vec::new(),
        details: Some(json!({
            "provider_type": row.provider_type.clone(),
            "model_count": row.models.len(),
        })),
    }
}

async fn list_provider_auth_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Prefer live Gizzi auth metadata; fall back to the static merge if either
    // Gizzi call fails so the auth-status panel never goes blank.
    match (
        crate::gizzi_provider_auth::discover_providers().await,
        crate::gizzi_provider_auth::provider_auth_methods().await,
    ) {
        (Ok(providers_payload), Ok(_auth_methods_payload)) => {
            let connected = parse_connected(&providers_payload);
            let mut providers: Vec<ProviderAuthStatusRow> = providers_payload
                .get("all")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter_map(|provider| auth_status_from_gizzi_provider(provider, &connected))
                .collect();
            // Gizzi's catalog is often unauthenticated cloud rows. Union the
            // CLIs actually on PATH and the local brains on this machine so
            // Connect can default to a CLI runtime and still list Ollama.
            let mut extra_rows = subprocess_provider_rows(&connected);
            extra_rows.push(ollama_provider_row(state.config.ollama_url()));
            extra_rows.push(ProviderRow {
                id: "allternit-sidecar".to_string(),
                name: "Sidecar".to_string(),
                provider_type: "local".to_string(),
                base_url: None,
                api_key_set: false,
                models: vec![],
                status: "ready".to_string(),
            });
            for row in extra_rows {
                let live = auth_status_live_from_row(&row);
                if let Some(existing) = providers.iter_mut().find(|p| p.provider_id == row.id) {
                    // Local PATH + `claude auth status` is the truth for
                    // subprocess CLIs. Gizzi's catalog often marks them
                    // authenticated just because the binary exists.
                    if row.provider_type == "subprocess" || (live.authenticated && !existing.authenticated) {
                        *existing = live;
                    }
                } else {
                    providers.push(live);
                }
            }
            return Json(json!({ "providers": providers })).into_response();
        }
        (discover_result, auth_result) => {
            if let Err(error) = discover_result {
                warn!(
                    "Gizzi provider discovery failed, falling back to static auth status: {}",
                    error
                );
            }
            if let Err(error) = auth_result {
                warn!(
                    "Gizzi provider auth methods failed, falling back to static auth status: {}",
                    error
                );
            }
        }
    }

    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };

    let mut stmt = match conn.prepare(
        "SELECT id, name, provider_type, base_url, api_key_env_var, models FROM providers ORDER BY name"
    ) {
        Ok(s) => s,
        Err(e) => return db_error(e),
    };

    let rows = match stmt.query_map([], row_to_provider) {
        Ok(r) => r,
        Err(e) => return db_error(e),
    };

    let db_providers: Vec<ProviderRow> = match rows.collect::<Result<Vec<_>, _>>() {
        Ok(p) => p,
        Err(e) => return db_error(e),
    };

    let all = merge_provider_sources(vec![
        db_providers,
        env_provider_rows(),
        read_gizzi_providers(&connected),
        subprocess_provider_rows(&connected),
        vec![ollama_provider_row(state.config.ollama_url())],
    ]);

    let providers: Vec<ProviderAuthStatusRow> = all.iter().map(auth_status_live_from_row).collect();

    Json(json!({ "providers": providers })).into_response()
}

/// GET /providers/:id/quota — plan usage windows (5-hour, weekly, …) for a
/// subscription provider, relayed from the gizzi runtime, which reads them
/// with the provider CLI's own sign-in. Always 200 with a `status` so the
/// client can show "signed out" / "unsupported" instead of an error.
async fn get_provider_quota(
    Path(id): Path<String>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let url = format!(
        "{}/provider/{}/quota",
        crate::agent_session_routes::gizzi_base(),
        urlencoding::encode(&id)
    );
    match crate::agent_session_routes::gizzi_client(&headers)
        .get(url)
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
    {
        Ok(res) if res.status().is_success() => match res.json::<serde_json::Value>().await {
            Ok(body) => Json(body).into_response(),
            Err(e) => Json(json!({"status": "error", "message": format!("bad quota response: {e}")})).into_response(),
        },
        Ok(res) => Json(json!({"status": "error", "message": format!("runtime returned {}", res.status())})).into_response(),
        Err(e) => Json(json!({"status": "error", "message": format!("runtime unreachable: {e}")})).into_response(),
    }
}

async fn get_provider_auth_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };
    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;

    let conn = match state.db.connect() {
        Ok(c) => c,
        Err(e) => return db_error(e),
    };

    let db_provider: Option<ProviderRow> = match conn.query_row(
        "SELECT id, name, provider_type, base_url, api_key_env_var, models FROM providers WHERE id = ?1",
        rusqlite::params![id],
        row_to_provider,
    ) {
        Ok(p) => Some(p),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return db_error(e),
    };

    let provider = if let Some(p) = db_provider {
        p
    } else {
        let ollama_row = ollama_provider_row(state.config.ollama_url());
        let all = merge_provider_sources(vec![
            env_provider_rows(),
            read_gizzi_providers(&connected),
            subprocess_provider_rows(&connected),
            vec![ollama_row],
        ]);
        match all.into_iter().find(|p| p.id == id) {
            Some(p) => p,
            None => {
                return (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))).into_response()
            }
        }
    };

    Json(json!({ "provider": auth_status_from_row(&provider) })).into_response()
}

// ─── Discover provider models ─────────────────────────────────────────────────

async fn discover_provider_models(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
) -> Response {
    let _user = match get_user(&headers) {
        Some(u) => u,
        None => return unauthorized(),
    };

    // Normalize chat-profile ids (e.g. "omlx-default") back to provider ids.
    let normalized_id = id
        .trim_end_matches("-default")
        .trim_end_matches("-auth")
        .to_string();

    if normalized_id == "ollama" {
        return list_ollama_models(State(state), Extension(_user), headers)
            .await
            .into_response();
    }

    let connected = crate::gizzi_provider_auth::connected_provider_ids().await;

    // For env-driven API providers, return the static model list.
    let env = env_provider_rows();
    if let Some(row) = env.into_iter().find(|p| p.id == normalized_id) {
        return Json(json!({
            "supported": true,
            "models": row.models.iter().map(|m| json!({ "id": m, "name": m })).collect::<Vec<_>>(),
            "default_model_id": row.models.first(),
            "allow_freeform": true,
        }))
        .into_response();
    }

    // For Gizzi-configured providers, return the configured models.
    let gizzi = read_gizzi_providers(&connected);
    if let Some(row) = gizzi.into_iter().find(|p| p.id == normalized_id) {
        return Json(json!({
            "supported": true,
            "models": row.models.iter().map(|m| json!({ "id": m, "name": m })).collect::<Vec<_>>(),
            "default_model_id": row.models.first(),
            "allow_freeform": true,
        }))
        .into_response();
    }

    // For subprocess providers, the runtime owns model discovery.
    let subprocess = subprocess_provider_rows(&connected);
    if let Some(row) = subprocess.into_iter().find(|p| p.id == normalized_id) {
        return Json(json!({
            "supported": false,
            "models": row.models.iter().map(|m| json!({ "id": m, "name": m })).collect::<Vec<_>>(),
            "allow_freeform": true,
            "freeform_hint": "Subprocess brains choose models internally; pass any model string the CLI supports.",
        }))
        .into_response();
    }

    Json(json!({
        "supported": false,
        "models": [],
        "allow_freeform": true,
        "freeform_hint": "Provider not recognized.",
    }))
    .into_response()
}

// ─── Subscription (OAuth) connect ─────────────────────────────────────────────
//
// One-click "Connect" for subscription-based CLI providers (Claude, Codex, Qwen,
// Kimi, Antigravity) plus API-key-only Z.ai. The connect endpoint triggers the
// provider's own sign-in flow (browser OAuth or a console page); the status
// endpoint reports whether sign-in completed. We never fabricate a successful
// sign-in: auto-detection is conservative, and the user can confirm explicitly.

struct SubscriptionProvider {
    /// Canonical gizzi provider id (must match the gizzi runtime registry).
    id: &'static str,
    label: &'static str,
    /// Default model id the gizzi runtime registers for this brain.
    model: &'static str,
    /// argv passed to the binary to start sign-in (empty => no CLI flow).
    login: &'static [&'static str],
    /// Fallback page the frontend opens when there is no automatic browser flow.
    page: &'static str,
    /// true => API-key only (no OAuth); false => OAuth/subscription CLI.
    api_key_only: bool,
}

fn subscription_provider(id: &str) -> Option<(&'static str, SubscriptionProvider)> {
    match id {
        "claude" | "claude-cli" => Some((
            "claude",
            SubscriptionProvider {
                id: "claude-cli",
                label: "Claude",
                model: "claude-sonnet-5",
                login: &["auth", "login"],
                page: "https://claude.ai/login",
                api_key_only: false,
            },
        )),
        "codex" | "codex-cli" => Some((
            "codex",
            SubscriptionProvider {
                id: "codex-cli",
                label: "Codex",
                model: "gpt-5.6-sol",
                login: &["login"],
                page: "https://chatgpt.com/",
                api_key_only: false,
            },
        )),
        "qwen" | "qwen-cli" => Some((
            "qwen",
            SubscriptionProvider {
                id: "qwen-cli",
                label: "Qwen",
                model: "qwen-plus",
                login: &["auth"],
                page: "https://qwen.ai/",
                api_key_only: false,
            },
        )),
        "kimi" | "kimi-cli" => Some((
            "kimi",
            SubscriptionProvider {
                id: "kimi-cli",
                label: "Kimi",
                model: "kimi-k3",
                login: &["login"],
                page: "https://www.kimi.com/",
                api_key_only: false,
            },
        )),
        "antigravity" | "agy" => Some((
            "agy",
            SubscriptionProvider {
                id: "antigravity",
                label: "Antigravity",
                model: "gemini-3.7-flash-high",
                login: &[],
                page: "https://antigravity.google/",
                api_key_only: false,
            },
        )),
        "cursor-agent" => Some((
            "cursor-agent",
            SubscriptionProvider {
                id: "cursor-agent",
                label: "Cursor Agent",
                model: "cursor-agent",
                login: &[],
                page: "https://cursor.com/",
                api_key_only: false,
            },
        )),
        "copilot" => Some((
            "copilot",
            SubscriptionProvider {
                id: "copilot",
                label: "GitHub Copilot CLI",
                model: "copilot",
                login: &[],
                page: "https://github.com/features/copilot",
                api_key_only: false,
            },
        )),
        "opencode" => Some((
            "opencode",
            SubscriptionProvider {
                id: "opencode",
                label: "OpenCode",
                model: "opencode",
                login: &[],
                page: "https://opencode.ai/",
                api_key_only: false,
            },
        )),
        "droid" | "factory" | "factory-droid" => Some((
            "droid",
            SubscriptionProvider {
                id: "droid",
                label: "Droid (Factory)",
                model: "droid",
                // Bare `droid` opens Factory's browser sign-in on first run.
                login: &[],
                page: "https://app.factory.ai/",
                api_key_only: false,
            },
        )),
        "gemini" | "gemini-cli" => Some((
            "gemini",
            SubscriptionProvider {
                id: "gemini-cli",
                label: "Gemini CLI",
                model: "gemini-cli",
                login: &[],
                page: "https://geminicli.com/",
                api_key_only: false,
            },
        )),
        "openclaw" => Some((
            "openclaw",
            SubscriptionProvider {
                id: "openclaw",
                label: "OpenClaw",
                model: "openclaw",
                login: &[],
                page: "https://github.com/ConlinJoe/ai-platform",
                api_key_only: false,
            },
        )),
        "hermes" => Some((
            "hermes",
            SubscriptionProvider {
                id: "hermes",
                label: "Hermes",
                model: "hermes",
                login: &[],
                page: "https://hermes.cx/",
                api_key_only: false,
            },
        )),
        "pi" => Some((
            "pi",
            SubscriptionProvider {
                id: "pi",
                label: "Pi",
                model: "pi",
                login: &[],
                page: "https://pi.ai/",
                api_key_only: false,
            },
        )),
        "codebuddy" => Some((
            "codebuddy",
            SubscriptionProvider {
                id: "codebuddy",
                label: "CodeBuddy",
                model: "codebuddy",
                login: &[],
                page: "https://codebuddy.ai/",
                api_key_only: false,
            },
        )),
        "deveco" => Some((
            "deveco",
            SubscriptionProvider {
                id: "deveco",
                label: "DevEco Code",
                model: "deveco",
                login: &[],
                page: "https://developer.huawei.com/",
                api_key_only: false,
            },
        )),
        "grok" => Some((
            "grok",
            SubscriptionProvider {
                id: "grok",
                label: "Grok",
                model: "grok",
                login: &[],
                page: "https://grok.com/",
                api_key_only: false,
            },
        )),
        "kiro-cli" => Some((
            "kiro-cli",
            SubscriptionProvider {
                id: "kiro-cli",
                label: "Kiro CLI",
                model: "kiro-cli",
                login: &[],
                page: "https://kiro.dev/",
                api_key_only: false,
            },
        )),
        "qodercli" => Some((
            "qodercli",
            SubscriptionProvider {
                id: "qodercli",
                label: "Qoder CLI",
                model: "qodercli",
                login: &[],
                page: "https://qoder.ai/",
                api_key_only: false,
            },
        )),
        "qoderclicn" => Some((
            "qoderclicn",
            SubscriptionProvider {
                id: "qoderclicn",
                label: "Qoder CN",
                model: "qoderclicn",
                login: &[],
                page: "https://qoder.ai/",
                api_key_only: false,
            },
        )),
        "qwenpaw" => Some((
            "qwenpaw",
            SubscriptionProvider {
                id: "qwenpaw",
                label: "QwenPaw",
                model: "qwenpaw",
                login: &[],
                page: "https://qwen.ai/",
                api_key_only: false,
            },
        )),
        "reasonix" => Some((
            "reasonix",
            SubscriptionProvider {
                id: "reasonix",
                label: "Reasonix",
                model: "reasonix",
                login: &[],
                page: "https://reasonix.ai/",
                api_key_only: false,
            },
        )),
        "traecli" => Some((
            "traecli",
            SubscriptionProvider {
                id: "traecli",
                label: "Trae CLI",
                model: "traecli",
                login: &[],
                page: "https://trae.ai/",
                api_key_only: false,
            },
        )),
        "dsh" => Some((
            "dsh",
            SubscriptionProvider {
                id: "dsh",
                label: "DeepSeek Harness",
                model: "dsh",
                login: &[],
                page: "https://deepseek.com/",
                api_key_only: false,
            },
        )),
        "omp" => Some((
            "omp",
            SubscriptionProvider {
                id: "omp",
                label: "Oh-My-Pi",
                model: "omp",
                login: &[],
                page: "https://oh-my-pi.dev/",
                api_key_only: false,
            },
        )),
        "mcode" => Some((
            "mcode",
            SubscriptionProvider {
                id: "mcode",
                label: "MiniMax Code",
                model: "mcode",
                login: &[],
                page: "https://minimaxi.com/",
                api_key_only: false,
            },
        )),
        "dim" => Some((
            "dim",
            SubscriptionProvider {
                id: "dim",
                label: "Dim",
                model: "dim",
                login: &[],
                page: "https://dim.ai/",
                api_key_only: false,
            },
        )),
        // Z.ai sells the GLM Coding Plan subscription, but today it has no public
        // OAuth/device-flow for third parties (Z.ai/ZCode OAuth is in progress).
        // The standard path is a console-created API key, so we treat it as key-based.
        "zai" | "z.ai" | "glm" => Some((
            "zai",
            SubscriptionProvider {
                id: "zai",
                label: "Z.ai",
                model: "glm-4.6",
                login: &[],
                page: "https://z.ai/subscribe",
                api_key_only: true,
            },
        )),
        _ => None,
    }
}

fn home_file(parts: &[&str]) -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));
    for part in parts {
        p.push(part);
    }
    p
}

/// Conservative, read-only check for whether a subscription provider is already
/// signed in. Never returns a false positive — when we cannot tell, it returns
/// false and the frontend falls back to an explicit user confirm.
///
/// For CLI providers we now require BOTH (a) the binary actually runs
/// (`--version` exit 0, mirroring gizzi's own liveness probe in
/// cmd/gizzi-code/.../subprocess.ts) AND (b) the provider's auth artifact is
/// present. This kills the stale-cred / removed-binary false positives that the
/// old file-exists-only check produced. Claude keeps its real `auth status`.
/// Bare names resolve through `command_on_path` (which also checks
/// ~/.allternit/tools/bin) so tools installed by allternit-tools are probed
/// even when this process's PATH predates the install.
fn resolve_cli(binary: &str) -> std::path::PathBuf {
    if binary.contains('/') {
        return std::path::PathBuf::from(binary);
    }
    command_on_path(binary).unwrap_or_else(|| std::path::PathBuf::from(binary))
}

fn cli_alive(binary: &str, args: &[&str], expect: Option<&str>) -> bool {
    let out = std::process::Command::new(resolve_cli(binary))
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => match expect {
            Some(exp) => String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains(exp),
            None => true,
        },
        _ => false,
    }
}

/// `claude auth status` exits 0 even when `loggedIn` is false (no subscription
/// / expired OAuth). Parse the JSON so Home does not default to a dead Claude.
fn claude_cli_logged_in(binary: &str) -> bool {
    let output = std::process::Command::new(resolve_cli(binary))
        .args(["auth", "status"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout) {
        return value
            .get("loggedIn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    }
    let lower = stdout.to_lowercase();
    !lower.contains("\"loggedin\": false")
        && !lower.contains("not logged")
        && !lower.contains("logged out")
}

fn subscription_auth_check(id: &str, binary: &str) -> bool {
    match id {
        "claude" | "claude-cli" => claude_cli_logged_in(binary),
        "codex" | "codex-cli" => {
            cli_alive(binary, &["--version"], Some("codex"))
                && home_file(&[".codex", "auth.json"]).exists()
        }
        "qwen" | "qwen-cli" => {
            cli_alive(binary, &["--version"], None)
                && (home_file(&[".qwen", "oauth_creds.json"]).exists()
                    || home_file(&[".config", "qwen", "oauth_creds.json"]).exists())
        }
        "kimi" | "kimi-cli" => {
            cli_alive(binary, &["--version"], Some("kimi"))
                && (home_file(&[".kimi-code", "auth.json"]).exists()
                    || home_file(&[".kimi-code", "config.json"]).exists())
        }
        "antigravity" | "agy" => {
            // agy (Antigravity CLI) stores its Google OAuth token at
            // ~/.gemini/oauth_creds.json (shared gemini-cli lineage). The
            // antigravity-cli dir holds per-install metadata, not the token.
            cli_alive(binary, &["--version"], None)
                && (home_file(&[".gemini", "oauth_creds.json"]).exists()
                    || home_file(&[".gemini", "antigravity-cli", "credentials.enc"]).exists()
                    || home_file(&[".config", "antigravity", "auth.json"]).exists())
        }
        "zai" | "z.ai" | "glm" => {
            std::env::var("ZAI_API_KEY").is_ok() || std::env::var("ZHIPU_API_KEY").is_ok()
        }
        "gemini" | "gemini-cli" => {
            cli_alive(binary, &["--version"], None)
                && (home_file(&[".gemini", "oauth_creds.json"]).exists()
                    || std::env::var("GEMINI_API_KEY").is_ok())
        }
        "droid" | "factory" | "factory-droid" => {
            cli_alive(binary, &["--version"], None)
                && (home_file(&[".factory", "auth.json"]).exists()
                    || home_file(&[".factory", "auth.v2.json"]).exists()
                    || std::env::var("FACTORY_API_KEY").is_ok())
        }
        // Generic agent-runtime CLI: if the binary is on PATH and answers a
        // version/help probe, assume the user has already installed and
        // authenticated it locally. This matches the Multica model where users
        // bring their own CLI tools.
        _ => cli_alive(binary, &["--version"], None)
            || cli_alive(binary, &["-v"], None)
            || cli_alive(binary, &["--help"], None),
    }
}

async fn connect_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }

    let (binary, meta) = match subscription_provider(&id) {
        Some(v) => v,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "unknown_provider"})),
            )
                .into_response()
        }
    };

    if meta.api_key_only {
        // Z.ai (GLM) is key-based but routes through gizzi as an OpenAI-compatible
        // provider (gizzi handles providerID "zai" in adapters/transform.ts). If a
        // key is already available, persist a real routing entry + make it the
        // default; otherwise return an actionable setup hint (not a dead stub).
        let gizzi_connected = crate::gizzi_provider_auth::connected_provider_ids().await;
        let key = std::env::var("ZAI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("ZHIPU_API_KEY")
                    .ok()
                    .filter(|s| !s.is_empty())
            });
        if gizzi_connected.contains(meta.id) {
            crate::onboarding_routes::persist_apikey_default(
                &state,
                meta.id,
                meta.label,
                meta.model,
                "@ai-sdk/openai-compatible",
                "https://api.z.ai/api/paas/v4",
                "GLM-4.6",
                200_000,
                131_072,
                true,
            );
            return Json(json!({
                "status": "already_connected",
                "provider": id,
                "label": meta.label,
                "routable": true,
            }))
            .into_response();
        }
        match key {
            Some(k) => {
                if let Err(error) = crate::gizzi_provider_auth::store_api_key(meta.id, &k).await {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": "provider_credential_store_failed", "message": error })),
                    )
                        .into_response();
                }
                crate::onboarding_routes::persist_apikey_default(
                    &state,
                    meta.id,
                    meta.label,
                    meta.model,
                    "@ai-sdk/openai-compatible",
                    "https://api.z.ai/api/paas/v4",
                    "GLM-4.6",
                    200_000,
                    131_072,
                    true,
                );
                return Json(json!({
                    "status": "connected",
                    "provider": id,
                    "label": meta.label,
                    "routable": true,
                }))
                .into_response();
            }
            None => {
                return Json(json!({
                    "status": "needs_api_key",
                    "provider": id,
                    "label": meta.label,
                    "page": meta.page,
                    "routable": false,
                    "key_setup": {
                        "env": "ZAI_API_KEY",
                        "hint": "Create a GLM API key at z.ai, then connect it in Allternit. The selected Gizzi runtime stores it locally.",
                    },
                }))
                .into_response();
            }
        }
    }

    // Resolve through command_on_path so CLIs installed by allternit-tools
    // (~/.allternit/tools/bin, maybe not on this process's PATH) still run.
    let resolved = command_on_path(binary);
    let binary_path = resolved
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| binary.to_string());
    if subscription_auth_check(&id, &binary_path) {
        // Already authenticated: make it the default brain immediately so a click
        // in Settings is enough to route agents through it.
        crate::onboarding_routes::persist_cli_default(
            &state, meta.id, meta.label, binary, meta.model,
        );
        return Json(json!({
            "status": "already_connected",
            "provider": id,
            "label": meta.label,
        }))
        .into_response();
    }

    if resolved.is_none() {
        return Json(json!({
            "status": "not_installed",
            "provider": id,
            "label": meta.label,
            "binary": binary,
            "page": meta.page,
            "action": "install",
            "install": crate::tools_install::install_action(&id),
        }))
        .into_response();
    }

    // Droid's only sign-in is its interactive TUI (bare `droid`, which opens
    // a browser from inside the TUI). Spawned headless with no stdin it just
    // hangs, so hand the UI the terminal command instead of a hidden process.
    if meta.id == "droid" {
        return Json(json!({
            "status": "sign_in_in_terminal",
            "provider": id,
            "label": meta.label,
            "command": binary_path,
            "page": meta.page,
        }))
        .into_response();
    }

    // Fire-and-forget: the CLI opens the user's browser / sign-in flow itself.
    let spawned = std::process::Command::new(&binary_path)
        .args(meta.login)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    match spawned {
        Ok(_) => Json(json!({
            "status": "started",
            "provider": id,
            "label": meta.label,
            "page": meta.page,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "spawn_failed", "details": e.to_string(), "page": meta.page})),
        )
            .into_response(),
    }
}

async fn connect_provider_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let (binary, meta) = match subscription_provider(&id) {
        Some(v) => v,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "unknown_provider"})),
            )
                .into_response()
        }
    };
    let binary_path = command_on_path(binary)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| binary.to_string());
    let connected = subscription_auth_check(&id, &binary_path);
    if connected {
        // Auto-detected completion of an interactive sign-in: promote to default.
        crate::onboarding_routes::persist_cli_default(
            &state, meta.id, meta.label, binary, meta.model,
        );
    }
    Json(json!({
        "status": if connected { "success" } else { "pending" },
        "provider": id,
        "label": meta.label,
    }))
    .into_response()
}

async fn confirm_provider_connect(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let (binary, meta) = match subscription_provider(&id) {
        Some(v) => v,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "unknown_provider"})),
            )
                .into_response()
        }
    };
    // User-attested completion of an interactive sign-in we could not auto-detect.
    // Promote to default so the one-click flow actually routes agents to it.
    crate::onboarding_routes::persist_cli_default(&state, meta.id, meta.label, binary, meta.model);
    Json(json!({ "status": "success", "provider": id, "confirmed": true })).into_response()
}

// ─── Install (allternit-tools) ────────────────────────────────────────────────
//
// POST /providers/:id/install            body: {"accept_terms": bool}
// GET  /providers/:id/install/status
// The installer runs one tool with --json; progress events are polled.

pub(crate) async fn install_provider_tool(
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let id = id.to_ascii_lowercase();
    if !crate::tools_install::valid_tool_id(&id) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_tool_id"}))).into_response();
    }
    let Some(cmd) = crate::tools_install::installer_command() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "installer_unavailable",
                "message": "This runtime cannot install tools. Install from Allternit Desktop or on your Allternit computer.",
            })),
        )
            .into_response();
    };
    let accept_terms = body
        .as_ref()
        .and_then(|b| b.get("accept_terms"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    match crate::tools_install::start_install(cmd, &id, accept_terms) {
        Ok(job) => (StatusCode::ACCEPTED, Json(json!({"status": "started", "job": job}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "install_failed_to_start", "message": e}))).into_response(),
    }
}

async fn install_provider_tool_status(Path(id): Path<String>, headers: HeaderMap) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }
    let id = id.to_ascii_lowercase();
    if !crate::tools_install::valid_tool_id(&id) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_tool_id"}))).into_response();
    }
    let installed = subscription_provider(&id)
        .map(|(binary, _)| command_on_path(binary).is_some())
        .unwrap_or(false);
    let job = crate::tools_install::job(&id);
    let state = job.as_ref().map(|j| j.state.clone()).unwrap_or_else(|| {
        if installed { "installed".into() } else { "idle".into() }
    });
    Json(json!({
        "provider": id,
        "state": state,
        "installed": installed,
        "installer_available": crate::tools_install::installer_command().is_some(),
        "job": job,
    }))
    .into_response()
}

// ─── Hugging Face GGUF search (PocketPal-style local model discovery) ────────

#[derive(Deserialize)]
struct HuggingFaceSearchQuery {
    q: Option<String>,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct HuggingFaceModelResult {
    #[serde(rename = "repoId")]
    repo_id: String,
    downloads: u64,
    likes: u64,
}

/// Searches HuggingFace's public model API for GGUF-tagged repos, so the
/// Models settings panel can offer any GGUF (not just a fixed catalog) the
/// same way PocketPal's model picker does. No auth needed for public repos.
/// Installing a result reuses Ollama's own `hf.co/<repo>` pull support via
/// `POST /api/local-brain/pull-custom`.
async fn search_huggingface(
    Extension(_user): Extension<AuthUser>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<HuggingFaceSearchQuery>,
) -> Response {
    if get_user(&headers).is_none() {
        return unauthorized();
    }

    let search_term = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(20).min(50);

    let client = reqwest::Client::new();
    let response = client
        .get("https://huggingface.co/api/models")
        .query(&[
            ("search", search_term.as_str()),
            ("filter", "gguf"),
            ("sort", "downloads"),
            ("direction", "-1"),
            ("limit", &limit.to_string()),
        ])
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await;

    let response = match response {
        Ok(res) if res.status().is_success() => res,
        Ok(res) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "huggingface_error", "status": res.status().as_u16() })),
            )
                .into_response();
        }
        Err(err) => {
            warn!("huggingface search request failed: {}", err);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "huggingface_unreachable", "message": err.to_string() })),
            )
                .into_response();
        }
    };

    let body = match response.json::<serde_json::Value>().await {
        Ok(value) => value,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "huggingface_parse_error", "message": err.to_string() })),
            )
                .into_response();
        }
    };

    let results: Vec<HuggingFaceModelResult> = body
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let repo_id = m
                        .get("id")
                        .or_else(|| m.get("modelId"))
                        .and_then(|v| v.as_str())?
                        .to_string();
                    Some(HuggingFaceModelResult {
                        repo_id,
                        downloads: m.get("downloads").and_then(|v| v.as_u64()).unwrap_or(0),
                        likes: m.get("likes").and_then(|v| v.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Json(json!({ "models": results })).into_response()
}

#[cfg(test)]
mod provider_install_surface_tests {
    use super::*;

    #[test]
    fn provider_table_has_opencode_droid_and_gemini_cli() {
        let (bin, meta) = subscription_provider("droid").expect("droid");
        assert_eq!((bin, meta.id), ("droid", "droid"));
        assert_eq!(subscription_provider("factory").unwrap().0, "droid");
        let (bin, meta) = subscription_provider("gemini-cli").expect("gemini-cli");
        assert_eq!((bin, meta.id), ("gemini", "gemini-cli"));
        assert_eq!(subscription_provider("opencode").unwrap().0, "opencode");
        for id in ["droid", "gemini-cli", "opencode"] {
            assert!(CLI_PROVIDER_SPECS.iter().any(|(sid, ..)| *sid == id), "{id} in CLI_PROVIDER_SPECS");
        }
    }

    #[test]
    fn provider_install_routes_merge_without_collisions() {
        // axum panics on overlapping routes; build the same merge main.rs does.
        let _ = provider_router().merge(crate::v1_routes::v1_router());
        let _ = axum::Router::<std::sync::Arc<AppState>>::new().nest("/api", provider_router());
    }

    #[test]
    fn provider_not_installed_carries_an_install_action() {
        let action = crate::tools_install::install_action("droid");
        assert_eq!(action["route"], "/api/v1/providers/droid/install");
        assert_eq!(action["status_route"], "/api/v1/providers/droid/install/status");
        assert!(action["available"].is_boolean());
    }
}
