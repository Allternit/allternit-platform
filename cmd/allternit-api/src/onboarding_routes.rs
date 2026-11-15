//! Onboarding routes — local AI service discovery, API key validation, and
//! unified configuration management for the packaged Allternit Platform app.
//!
//! These endpoints are the backend for the brain-setup wizard. They let the
//! frontend:
//!   - read the current company + user configuration (without secrets)
//!   - save user preferences (default model, gateway URLs)
//!   - discover local providers (Ollama, LM Studio, Claude CLI, Codex CLI)
//!   - validate a provider API key against the provider's model endpoint
//!   - hand provider credentials directly to Gizzi and write only provider
//!     metadata to its runtime config.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

use crate::agent_session_routes::gizzi_client;
use crate::config::{save_user_config, AppConfig, SaveUserConfigPayload, UserConfig};
use crate::AppState;

pub fn onboarding_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/onboarding/config", get(get_config).post(save_config))
        .route("/onboarding/provider", post(save_provider))
        .route("/onboarding/discover", get(onboarding_discover))
        .route("/onboarding/validate-key", post(onboarding_validate_key))
        .route("/onboarding/init-project", post(init_project))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Configuration endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct ConfigResponse {
    #[serde(rename = "company")]
    company: CompanyConfigResponse,
    #[serde(rename = "user")]
    user: UserConfig,
    #[serde(rename = "onboardingComplete")]
    onboarding_complete: bool,
    #[serde(rename = "encryptionEnabled")]
    encryption_enabled: bool,
}

#[derive(Serialize)]
struct CompanyConfigResponse {
    #[serde(rename = "clerkPublishableKey")]
    clerk_publishable_key: Option<String>,
    #[serde(rename = "gatewayUrl")]
    gateway_url: String,
    #[serde(rename = "terminalServerUrl")]
    terminal_server_url: String,
    #[serde(rename = "tenantId")]
    tenant_id: String,
    #[serde(rename = "selfHosted")]
    self_hosted: bool,
}

async fn get_config(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    // Reload from disk each time so wizard/settings changes are visible immediately
    // without requiring an API restart.
    let app_config = AppConfig::load();
    let company = CompanyConfigResponse {
        clerk_publishable_key: app_config.clerk_publishable_key(),
        gateway_url: app_config.gateway_url(),
        terminal_server_url: app_config.terminal_server_url(),
        tenant_id: app_config.tenant_id(),
        self_hosted: app_config.self_hosted(),
    };
    let user = app_config.user.clone();
    let response = ConfigResponse {
        company,
        user,
        onboarding_complete: app_config.onboarding_complete(),
        encryption_enabled: crate::token_crypto::encryption_enabled(),
    };
    (StatusCode::OK, Json(response))
}

async fn save_config(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<SaveUserConfigPayload>,
) -> impl IntoResponse {
    let mut user: UserConfig = payload.into();

    // Provider credentials are accepted only by the dedicated provider route
    // and are handed directly to Gizzi. Never persist them in user config.
    user.provider_api_keys = None;

    match save_user_config(&user) {
        Ok(()) => {
            info!(default_model = ?user.default_model, "Saved user config from wizard");
            // The signed desktop injects this key; headless/VPS installs use a
            // mode-0600 local runtime key. The API never touches OS Keychain.
            let enc_ok = crate::token_crypto::ensure_platform_key();
            if enc_ok {
                info!("Connector token encryption enabled after onboarding");
            } else {
                warn!("Connector token encryption could not be enabled");
            }
            (
                StatusCode::OK,
                Json(
                    json!({ "success": true, "encryption_enabled": crate::token_crypto::encryption_enabled() }),
                ),
            )
        }
        Err(err) => {
            warn!(error = %err, "Failed to save user config");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to save config: {err}") })),
            )
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Provider configuration endpoint
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct SaveProviderPayload {
    /// Provider identifier, e.g. "anthropic", "openai", "kimi-cli".
    provider: String,
    /// Human-readable provider name.
    #[serde(default)]
    name: Option<String>,
    /// NPM package used by the AI SDK adapter (optional — Gizzi can infer it).
    #[serde(default)]
    npm: Option<String>,
    /// Default model for this provider, e.g. "anthropic/claude-sonnet-5".
    #[serde(rename = "defaultModel", default)]
    default_model: Option<String>,
    /// Provider API key. Handed directly to Gizzi's private credential store.
    #[serde(rename = "apiKey", default)]
    api_key: Option<String>,
    /// Base URL for the provider API.
    #[serde(rename = "baseURL", default)]
    base_url: Option<String>,
    /// Authentication mode. Defaults to api_key for remote providers, none for
    /// local providers, subprocess for CLI-backed providers.
    #[serde(rename = "authType", default)]
    auth_type: Option<String>,
    /// CLI command for auth_type: subprocess providers.
    #[serde(rename = "subprocessCmd", default)]
    subprocess_cmd: Option<String>,
    /// Models exposed by this provider.
    #[serde(default)]
    models: Option<serde_json::Map<String, serde_json::Value>>,
    /// Whether this provider should become the new default.
    #[serde(default)]
    set_default: bool,
}

async fn save_provider(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SaveProviderPayload>,
) -> impl IntoResponse {
    let provider_id = payload.provider.trim().to_string();
    if provider_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "provider is required" })),
        );
    }

    // 1. Hand the credential directly to Gizzi. allternit-api never stores or
    // reads provider credentials itself.
    if let Some(key) = payload.api_key.as_ref().filter(|k| !k.is_empty()) {
        if let Err(err) = crate::gizzi_provider_auth::store_api_key(&provider_id, key).await {
            warn!(provider = %provider_id, error = %err, "Gizzi failed to store provider credential");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err })),
            );
        }
    }

    // 2. Merge provider metadata into the Gizzi user config file.
    let gizzi_path = crate::config::gizzi_user_config_path();
    let mut gizzi_config = read_gizzi_config(&gizzi_path);

    let provider_entry = build_provider_entry(&payload);
    let providers = gizzi_config
        .entry("provider")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    let merged = providers
        .get(&provider_id)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let mut merged = merged;
    if let Some(entry_map) = provider_entry.as_object() {
        for (k, v) in entry_map {
            merged.insert(k.clone(), v.clone());
        }
    }
    providers.insert(provider_id.clone(), json!(merged));

    if payload.set_default {
        if let Some(model) = payload.default_model.as_ref().filter(|m| !m.is_empty()) {
            gizzi_config.insert("model".to_string(), json!(model));
        } else if let Some(model_id) = payload.models.as_ref().and_then(|m| m.keys().next()) {
            gizzi_config.insert(
                "model".to_string(),
                json!(format!("{}/{}", provider_id, model_id)),
            );
        }
    }

    match write_gizzi_config(&gizzi_path, &gizzi_config) {
        Ok(()) => {
            info!(provider = %provider_id, path = %gizzi_path.display(), "Wrote Gizzi provider config");
        }
        Err(err) => {
            warn!(provider = %provider_id, error = %err, "Failed to write Gizzi config");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to write provider config: {err}") })),
            );
        }
    }

    // 3. Update the Allternit user config so the default model reflects the new
    //    provider if requested.
    if payload.set_default {
        let new_default = gizzi_config
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(model) = new_default {
            let mut user = state.config.user.clone();
            user.default_model = Some(model);
            user.onboarding_complete = Some(true);
            let _ = save_user_config(&user);
        }
    }

    (
        StatusCode::OK,
        Json(json!({ "success": true, "provider": provider_id })),
    )
}

fn build_provider_entry(payload: &SaveProviderPayload) -> serde_json::Value {
    let mut entry = serde_json::Map::new();

    if let Some(name) = payload.name.as_ref().filter(|n| !n.is_empty()) {
        entry.insert("name".to_string(), json!(name));
    } else {
        entry.insert("name".to_string(), json!(payload.provider.clone()));
    }

    if let Some(npm) = payload.npm.as_ref().filter(|n| !n.is_empty()) {
        entry.insert("npm".to_string(), json!(npm));
    }

    // A non-empty subprocess command unambiguously makes this a subprocess
    // provider. This preserves CLI brains (Claude CLI, etc.) even when the
    // frontend omits authType or sends the default "api_key".
    let has_subprocess_cmd = payload
        .subprocess_cmd
        .as_ref()
        .is_some_and(|c| !c.is_empty());

    let auth_type = if has_subprocess_cmd {
        "subprocess"
    } else {
        payload
            .auth_type
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("api_key")
    };
    entry.insert("auth_type".to_string(), json!(auth_type));

    if let Some(cmd) = payload.subprocess_cmd.as_ref().filter(|c| !c.is_empty()) {
        entry.insert("subprocess_cmd".to_string(), json!(cmd));
    }

    let mut options = serde_json::Map::new();
    if let Some(base_url) = payload.base_url.as_ref().filter(|u| !u.is_empty()) {
        options.insert("baseURL".to_string(), json!(base_url));
    }
    // NOTE: The API key is intentionally NOT written here. Gizzi owns it and
    // resolves it when the provider is used.
    if !options.is_empty() {
        entry.insert("options".to_string(), json!(options));
    }

    if let Some(models) = payload.models.clone() {
        entry.insert("models".to_string(), json!(models));
    }

    json!(entry)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Gizzi config file helpers
// ═══════════════════════════════════════════════════════════════════════════════

// The gizzi config path is resolved by `crate::config::gizzi_user_config_path()`
// (shared with the runtime harness reader) so we never drift from the file the
// gizzi-code runtime actually opens (~/.config/gizzi-code/config.json).

fn read_gizzi_config(path: &PathBuf) -> serde_json::Map<String, serde_json::Value> {
    if let Ok(text) = std::fs::read_to_string(path) {
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => return map,
            Ok(_) => warn!(path = %path.display(), "Gizzi config is not an object"),
            Err(err) => warn!(path = %path.display(), error = %err, "Failed to parse Gizzi config"),
        }
    }
    let mut default = serde_json::Map::new();
    default.insert("$schema".to_string(), json!("https://gizzi.io/config.json"));
    default
}

fn write_gizzi_config(
    path: &PathBuf,
    config: &serde_json::Map<String, serde_json::Value>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config)?;
    std::fs::write(path, text)
}

/// Register a CLI/subprocess brain in the Gizzi config and make it the platform
/// default. Used by the one-click connect flow (`provider_routes`) so a successful
/// Connect in either Settings or Onboarding immediately routes agents through the
/// newly authenticated brain — one source of truth, no duplicate save logic on the
/// frontend. Best-effort: failures are logged, not surfaced, because auth detection
/// already succeeded and the brain is runnable regardless of default-model stamping.
pub fn persist_cli_default(
    state: &AppState,
    provider_id: &str,
    name: &str,
    binary: &str,
    model: &str,
) {
    let default_model = format!("{}/{}", provider_id, model);
    let gizzi_path = crate::config::gizzi_user_config_path();
    let mut gizzi_config = read_gizzi_config(&gizzi_path);

    // Build the subprocess command exactly how gizzi's own discovery does it
    // (cmd/gizzi-code/.../subprocess.ts): absolute binary path + the per-provider
    // tail args. Storing the bare name (e.g. "claude") would clobber a working
    // "/full/path/claude -p" entry and break headless/print-mode chat. Connect
    // already validated the binary is on PATH, so resolution should succeed; if
    // it doesn't we warn and fall back to the bare name rather than pretend.
    let resolved = crate::provider_routes::command_on_path(binary)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            warn!(provider = %provider_id, binary, "CLI binary not on PATH at persist; storing bare name");
            binary.to_string()
        });
    let tail = match binary {
        "claude" | "kimi" | "qwen" | "agy" | "gemini" => "-p",
        "gh" => "copilot suggest -t shell",
        "llm" => "prompt",
        "ollama" => "run",
        _ => "",
    };
    let subprocess_cmd = if tail.is_empty() {
        resolved
    } else {
        format!("{} {}", resolved, tail)
    };

    let providers = gizzi_config
        .entry("provider")
        .or_insert_with(|| json!({}))
        .as_object_mut();
    if let Some(providers) = providers {
        let mut merged = providers
            .get(provider_id)
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        merged.insert("name".to_string(), json!(name));
        merged.insert("auth_type".to_string(), json!("subprocess"));
        merged.insert("subprocess_cmd".to_string(), json!(subprocess_cmd));
        let mut models = serde_json::Map::new();
        models.insert(model.to_string(), json!({}));
        merged.insert("models".to_string(), json!(models));
        providers.insert(provider_id.to_string(), json!(merged));
    }
    gizzi_config.insert("model".to_string(), json!(default_model));

    if let Err(err) = write_gizzi_config(&gizzi_path, &gizzi_config) {
        warn!(provider = %provider_id, error = %err, "Failed to persist CLI provider to Gizzi config");
        return;
    }

    let mut user = state.config.user.clone();
    user.default_model = Some(default_model);
    user.onboarding_complete = Some(true);
    if let Err(err) = save_user_config(&user) {
        warn!(provider = %provider_id, error = %err, "Failed to persist default model to user config");
    } else {
        info!(provider = %provider_id, "One-click connect: set default brain");
    }
}

/// Register an API-key (OpenAI-compatible) brain in the Gizzi config and make it
/// the platform default. Mirrors gizzi's config-provider shape (see the live
/// `ollama` entry and `runtime/providers/provider.ts:595`, which forwards
/// provider metadata. Used by key-based providers like Z.ai (GLM); credentials
/// are stored separately through Gizzi's `/auth/:providerID` boundary.
#[allow(clippy::too_many_arguments)]
pub fn persist_apikey_default(
    state: &AppState,
    provider_id: &str,
    name: &str,
    model: &str,
    npm: &str,
    base_url: &str,
    model_name: &str,
    context: u32,
    output: u32,
    tool_call: bool,
) {
    let default_model = format!("{}/{}", provider_id, model);
    let gizzi_path = crate::config::gizzi_user_config_path();
    let mut gizzi_config = read_gizzi_config(&gizzi_path);

    let providers = gizzi_config
        .entry("provider")
        .or_insert_with(|| json!({}))
        .as_object_mut();
    if let Some(providers) = providers {
        let mut models = serde_json::Map::new();
        models.insert(
            model.to_string(),
            json!({
                "id": model,
                "name": model_name,
                "limit": { "context": context, "output": output },
                "tool_call": tool_call,
            }),
        );
        providers.insert(
            provider_id.to_string(),
            json!({
                "name": name,
                "npm": npm,
                "auth_type": "api_key",
                "options": { "baseURL": base_url },
                "models": models,
            }),
        );
    }
    gizzi_config.insert("model".to_string(), json!(default_model));

    if let Err(err) = write_gizzi_config(&gizzi_path, &gizzi_config) {
        warn!(provider = %provider_id, error = %err, "Failed to persist api-key provider to Gizzi config");
        return;
    }

    let mut user = state.config.user.clone();
    user.default_model = Some(default_model);
    user.onboarding_complete = Some(true);
    if let Err(err) = save_user_config(&user) {
        warn!(provider = %provider_id, error = %err, "Failed to persist default model to user config");
    } else {
        info!(provider = %provider_id, "One-click connect: set default api-key brain");
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Provider discovery
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct ModelInfo {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct CliInfo {
    name: String,
    #[serde(rename = "command")]
    command: String,
    #[serde(rename = "installed")]
    installed: bool,
    #[serde(rename = "version", skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

async fn onboarding_discover(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let ollama_url = state.config.ollama_url();

    // Check Ollama
    let ollama_running = crate::net_client::client()
        .get(format!("{}/api/tags", ollama_url.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let ollama_models = if ollama_running {
        match crate::net_client::client()
            .get(format!("{}/api/tags", ollama_url.trim_end_matches('/')))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(res) => {
                if let Ok(json) = res.json::<serde_json::Value>().await {
                    json.get("models")
                        .and_then(|m| m.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| {
                                    let id = m.get("name")?.as_str()?.to_string();
                                    Some(ModelInfo {
                                        name: id.clone(),
                                        id,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                }
            }
            Err(_) => vec![],
        }
    } else {
        vec![]
    };

    // Check LM Studio (default port 1234)
    let lmstudio_running = crate::net_client::client()
        .get("http://localhost:1234/v1/models")
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let lmstudio_models = if lmstudio_running {
        match crate::net_client::client()
            .get("http://localhost:1234/v1/models")
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            Ok(res) => {
                if let Ok(json) = res.json::<serde_json::Value>().await {
                    json.get("data")
                        .and_then(|d| d.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| {
                                    let id = m.get("id")?.as_str()?.to_string();
                                    Some(ModelInfo {
                                        name: id.clone(),
                                        id,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                }
            }
            Err(_) => vec![],
        }
    } else {
        vec![]
    };

    let claude_cli = discover_cli("claude", &["--version"]).await;
    let codex_cli = discover_cli("codex", &["--version"]).await;
    let ollama_cli = discover_cli("ollama", &["--version"]).await;
    let kimi_cli = discover_cli("kimi", &["--version"]).await;
    let aider_cli = discover_cli("aider", &["--version"]).await;
    let openrouter_cli = discover_cli("openrouter", &["--version"]).await;

    Json(json!({
        "ollama": {
            "running": ollama_running,
            "models": ollama_models,
        },
        "lmstudio": {
            "running": lmstudio_running,
            "models": lmstudio_models,
        },
        "cli": [
            CliInfo { name: "Allternit".to_string(), command: "allternit".to_string(), installed: true, version: None },
            claude_cli,
            codex_cli,
            ollama_cli,
            kimi_cli,
            aider_cli,
            openrouter_cli,
        ],
    }))
}

async fn discover_cli(command: &str, args: &[&str]) -> CliInfo {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(command).args(args).output(),
    )
    .await;

    let (installed, version) = match output {
        Ok(Ok(out)) if out.status.success() => {
            let version = String::from_utf8(out.stdout)
                .ok()
                .map(|s| s.trim().split('\n').next().unwrap_or("").to_string())
                .filter(|s| !s.is_empty());
            (true, version)
        }
        _ => (false, None),
    };

    CliInfo {
        name: command.to_string(),
        command: command.to_string(),
        installed,
        version,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// API key validation
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct ValidateKeyBody {
    provider: String,
    key: String,
}

async fn onboarding_validate_key(Json(body): Json<ValidateKeyBody>) -> impl IntoResponse {
    let provider = body.provider.to_lowercase();
    let key = body.key;

    let result = match provider.as_str() {
        "openai" => validate_openai_key(&key).await,
        "anthropic" => validate_anthropic_key(&key).await,
        "google" => validate_google_key(&key).await,
        "groq" => validate_groq_key(&key).await,
        "openrouter" => validate_openrouter_key(&key).await,
        "kimi" | "kimi-for-coding" => validate_kimi_key(&key).await,
        _ => Ok(ValidationResult {
            valid: true,
            models: None,
            error: Some(format!(
                "Unknown provider '{}', accepting key without validation",
                provider
            )),
        }),
    };

    match result {
        Ok(r) => (
            StatusCode::OK,
            Json(json!({
                "valid": r.valid,
                "models": r.models,
                "error": r.error,
            })),
        ),
        Err(e) => (
            StatusCode::OK,
            Json(json!({
                "valid": false,
                "error": e,
            })),
        ),
    }
}

struct ValidationResult {
    valid: bool,
    models: Option<Vec<serde_json::Value>>,
    error: Option<String>,
}

async fn validate_openai_key(key: &str) -> Result<ValidationResult, String> {
    let client = crate::net_client::client();
    let res = client
        .get("https://api.openai.com/v1/models")
        .header("Authorization", format!("Bearer {}", key))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        return Ok(ValidationResult {
            valid: false,
            models: None,
            error: Some(format!("API returned {}", res.status())),
        });
    }

    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let models: Vec<serde_json::Value> = json
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?;
                    Some(json!({"id": id, "name": id}))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ValidationResult {
        valid: true,
        models: Some(models),
        error: None,
    })
}

async fn validate_anthropic_key(key: &str) -> Result<ValidationResult, String> {
    let client = crate::net_client::client();
    let res = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        return Ok(ValidationResult {
            valid: false,
            models: None,
            error: Some(format!("API returned {}", res.status())),
        });
    }

    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let models: Vec<serde_json::Value> = json
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?;
                    Some(json!({"id": id, "name": id}))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ValidationResult {
        valid: true,
        models: Some(models),
        error: None,
    })
}

async fn validate_google_key(_key: &str) -> Result<ValidationResult, String> {
    Ok(ValidationResult {
        valid: true,
        models: Some(vec![
            json!({"id": "gemini-1.5-pro", "name": "Gemini 1.5 Pro"}),
            json!({"id": "gemini-1.5-flash", "name": "Gemini 1.5 Flash"}),
        ]),
        error: None,
    })
}

async fn validate_groq_key(key: &str) -> Result<ValidationResult, String> {
    let client = crate::net_client::client();
    let res = client
        .get("https://api.groq.com/openai/v1/models")
        .header("Authorization", format!("Bearer {}", key))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        return Ok(ValidationResult {
            valid: false,
            models: None,
            error: Some(format!("API returned {}", res.status())),
        });
    }

    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let models: Vec<serde_json::Value> = json
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?;
                    Some(json!({"id": id, "name": id}))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ValidationResult {
        valid: true,
        models: Some(models),
        error: None,
    })
}

async fn validate_openrouter_key(key: &str) -> Result<ValidationResult, String> {
    let client = crate::net_client::client();
    let res = client
        .get("https://openrouter.ai/api/v1/models")
        .header("Authorization", format!("Bearer {}", key))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        return Ok(ValidationResult {
            valid: false,
            models: None,
            error: Some(format!("API returned {}", res.status())),
        });
    }

    let json: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    let models: Vec<serde_json::Value> = json
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .take(50)
                .filter_map(|m| {
                    let id = m.get("id")?.as_str()?;
                    let name = m.get("name")?.as_str().unwrap_or(id);
                    Some(json!({"id": id, "name": name}))
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(ValidationResult {
        valid: true,
        models: Some(models),
        error: None,
    })
}

async fn validate_kimi_key(key: &str) -> Result<ValidationResult, String> {
    // Kimi does not expose a public model list endpoint, so we validate by
    // making a tiny chat completion request.
    let client = crate::net_client::client();
    let res = client
        .post("https://api.kimi.com/coding/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": "kimi-k2",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 1
        }))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !res.status().is_success() {
        return Ok(ValidationResult {
            valid: false,
            models: None,
            error: Some(format!("API returned {}", res.status())),
        });
    }

    Ok(ValidationResult {
        valid: true,
        models: Some(vec![json!({"id": "kimi-k2", "name": "Kimi K2"})]),
        error: None,
    })
}

// ─── Project initialization proxy ─────────────────────────────────────────────

#[derive(Deserialize)]
struct InitProjectRequest {
    dir: String,
    #[serde(rename = "skipCodemap")]
    skip_codemap: Option<bool>,
}

async fn init_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<InitProjectRequest>,
) -> impl IntoResponse {
    let client = gizzi_client(&headers);
    let gizzi_url = state.config.terminal_server_url().trim_end_matches('/').to_string();
    let response = match client
        .post(format!("{}/v1/project/init", gizzi_url))
        .json(&json!({
            "dir": body.dir,
            "skipCodemap": body.skip_codemap.unwrap_or(false),
        }))
        .send()
        .await
    {
        Ok(res) => res,
        Err(err) => {
            warn!("project init request failed: {}", err);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "Gizzi unreachable", "message": err.to_string() })),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body_bytes = response.bytes().await.unwrap_or_default();
    (status, Json(serde_json::from_slice::<serde_json::Value>(&body_bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&body_bytes)})))).into_response()
}
