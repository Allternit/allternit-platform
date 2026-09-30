//! Local CLI provider detector for the inference router.
//!
//! Probes the host machine for installed provider CLIs (Claude Code, Codex,
//! Cursor, OpenRouter, and existing Allternit stack providers) and returns a
//! frontend-friendly status list. Detection is best-effort and never panics:
//! missing binaries are reported as unavailable with a human-readable reason.

use serde::Serialize;
use std::path::PathBuf;

/// Public status shape returned by `GET /api/v1/inference-router/cli-status`.
#[derive(Debug, Clone, Serialize)]
pub struct CliProviderInfo {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<CliProviderModel>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CliProviderModel {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

/// Detect all known CLI providers and return their merged status.
pub async fn detect_cli_providers() -> Vec<CliProviderInfo> {
    let mut providers = Vec::new();

    providers.push(detect_claude_code().await);
    providers.push(detect_codex().await);
    providers.push(detect_cursor().await);
    providers.push(detect_openrouter().await);
    providers.push(detect_hermes().await);
    providers.push(detect_openclaw().await);
    providers.push(detect_grok().await);
    providers.push(detect_ollama().await);

    providers
}

// ─── Provider detection implementations ───────────────────────────────────────

async fn detect_claude_code() -> CliProviderInfo {
    let binary = "claude";
    let installed = command_exists(binary).await;
    let authed = installed && (has_home_file(".claude/auth.json").await || env_key_set("ANTHROPIC_API_KEY"));

    let models = vec![
        CliProviderModel {
            id: "claude-sonnet-4-6".to_string(),
            name: "Claude Sonnet 4.6".to_string(),
            default: Some(true),
        },
        CliProviderModel {
            id: "claude-opus-4-6".to_string(),
            name: "Claude Opus 4.6".to_string(),
            default: None,
        },
    ];

    CliProviderInfo {
        id: "claude-cli".to_string(),
        name: "Claude Code".to_string(),
        installed,
        available: authed,
        reason: reason(installed, authed, binary, "Sign in with `claude login`"),
        models: Some(models),
    }
}

async fn detect_codex() -> CliProviderInfo {
    let binary = "codex";
    let installed = command_exists(binary).await;
    let authed = installed && (env_key_set("OPENAI_API_KEY") || has_home_file(".openai/token").await);

    let models = vec![
        CliProviderModel {
            id: "gpt-6-astra".to_string(),
            name: "Astra".to_string(),
            default: Some(true),
        },
        CliProviderModel {
            id: "gpt-5.6-sol".to_string(),
            name: "Sol".to_string(),
            default: None,
        },
        CliProviderModel {
            id: "gpt-5.6-terra".to_string(),
            name: "Terra".to_string(),
            default: None,
        },
        CliProviderModel {
            id: "gpt-5.6-luna".to_string(),
            name: "Luna".to_string(),
            default: None,
        },
    ];

    CliProviderInfo {
        id: "codex-cli".to_string(),
        name: "Codex CLI".to_string(),
        installed,
        available: authed,
        reason: reason(installed, authed, binary, "Set OPENAI_API_KEY or run `codex login`"),
        models: Some(models),
    }
}

async fn detect_cursor() -> CliProviderInfo {
    // Cursor ships either as `cursor-agent` for headless runs or the editor's
    // `cursor` CLI. Prefer `cursor-agent` because it matches the subprocess
    // provider registration; fall back to `cursor`.
    let agent_binary = "cursor-agent";
    let editor_binary = "cursor";
    let installed = command_exists(agent_binary).await || command_exists(editor_binary).await;

    let models = vec![CliProviderModel {
        id: "cursor-agent".to_string(),
        name: "Cursor Agent".to_string(),
        default: Some(true),
    }];

    CliProviderInfo {
        id: "cursor-agent".to_string(),
        name: "Cursor".to_string(),
        installed,
        available: installed,
        reason: if installed {
            None
        } else {
            Some("Cursor agent CLI not installed".to_string())
        },
        models: Some(models),
    }
}

async fn detect_openrouter() -> CliProviderInfo {
    // OpenRouter is primarily API-key driven; there is also a community CLI.
    let key_set = env_key_set("OPENROUTER_API_KEY");
    let cli_installed = command_exists("openrouter").await;
    let installed = key_set || cli_installed;
    let available = key_set; // The key is what lets us route turns today.

    let models = vec![
        CliProviderModel {
            id: "openrouter/auto".to_string(),
            name: "OpenRouter Auto".to_string(),
            default: Some(true),
        },
        CliProviderModel {
            id: "anthropic/claude-sonnet-4".to_string(),
            name: "Claude Sonnet 4 (OpenRouter)".to_string(),
            default: None,
        },
    ];

    CliProviderInfo {
        id: "openrouter".to_string(),
        name: "OpenRouter".to_string(),
        installed,
        available,
        reason: if available {
            None
        } else if cli_installed {
            Some("Set OPENROUTER_API_KEY".to_string())
        } else {
            Some("Set OPENROUTER_API_KEY or install the openrouter CLI".to_string())
        },
        models: Some(models),
    }
}

async fn detect_hermes() -> CliProviderInfo {
    detect_stack_provider("hermes", "Hermes", &["hermes"], &[CliProviderModel {
        id: "hermes".to_string(),
        name: "Hermes".to_string(),
        default: Some(true),
    }])
        .await
}

async fn detect_openclaw() -> CliProviderInfo {
    detect_stack_provider("openclaw", "OpenClaw", &["openclaw"], &[CliProviderModel {
        id: "openclaw".to_string(),
        name: "OpenClaw".to_string(),
        default: Some(true),
    }])
        .await
}

async fn detect_grok() -> CliProviderInfo {
    // gizzi drives the Grok Build CLI over ACP (`grok agent stdio`), so the
    // `grok` binary counts like the other local CLIs; XAI_API_KEY alone keeps
    // the API route usable.
    grok_info(command_exists("grok").await, env_key_set("XAI_API_KEY"))
}

fn grok_info(binary_installed: bool, key_set: bool) -> CliProviderInfo {
    let available = binary_installed || key_set;
    let models = vec![CliProviderModel {
        id: "grok-3".to_string(),
        name: "Grok 3".to_string(),
        default: Some(true),
    }];

    CliProviderInfo {
        id: "grok".to_string(),
        name: "Grok".to_string(),
        installed: available,
        available,
        reason: if available {
            None
        } else {
            Some("`grok` not found on PATH (install Grok Build) or set XAI_API_KEY".to_string())
        },
        models: Some(models),
    }
}

async fn detect_ollama() -> CliProviderInfo {
    let binary = "ollama";
    let installed = command_exists(binary).await;

    // Best-effort live model list. If Ollama is not running, still report the
    // binary as installed so the user knows they can start it.
    let models = if installed {
        probe_ollama_models().await
    } else {
        vec![]
    };

    let available = !models.is_empty();

    CliProviderInfo {
        id: "ollama".to_string(),
        name: "Ollama".to_string(),
        installed,
        available,
        reason: if installed && !available {
            Some("Ollama is installed but not running".to_string())
        } else if !installed {
            Some("Ollama not installed".to_string())
        } else {
            None
        },
        models: if models.is_empty() {
            None
        } else {
            Some(models)
        },
    }
}

async fn detect_stack_provider(
    id: &str,
    name: &str,
    binaries: &[&str],
    models: &[CliProviderModel],
) -> CliProviderInfo {
    let mut installed = false;
    for binary in binaries {
        if command_exists(binary).await {
            installed = true;
            break;
        }
    }

    CliProviderInfo {
        id: id.to_string(),
        name: name.to_string(),
        installed,
        available: installed,
        reason: if installed {
            None
        } else {
            Some(format!("{} CLI not installed", name))
        },
        models: if models.is_empty() {
            None
        } else {
            Some(models.to_vec())
        },
    }
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

fn env_key_set(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| !v.is_empty())
}

async fn command_exists(cmd: &str) -> bool {
    tokio::task::spawn_blocking({
        let cmd = cmd.to_string();
        move || command_on_path(&cmd).is_some()
    })
    .await
    .unwrap_or(false)
}

/// Best-effort PATH probe that does not require the `which` crate.
fn command_on_path(cmd: &str) -> Option<std::path::PathBuf> {
    let path_env = std::env::var("PATH").unwrap_or_default();
    for dir in path_env.split(':') {
        let candidate = std::path::Path::new(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

async fn has_home_file(rel: &str) -> bool {
    let path = home_dir().map(|h| h.join(rel));
    match path {
        Some(p) => tokio::fs::metadata(p).await.is_ok(),
        None => false,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir())
}

fn reason(installed: bool, available: bool, binary: &str, auth_hint: &str) -> Option<String> {
    if available {
        return None;
    }
    if !installed {
        Some(format!("`{}` not found on PATH", binary))
    } else {
        Some(auth_hint.to_string())
    }
}

async fn probe_ollama_models() -> Vec<CliProviderModel> {
    let url = std::env::var("OLLAMA_HOST")
        .ok()
        .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let resp = match client
        .get(format!("{}/api/tags", url.trim_end_matches('/')))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return vec![],
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    body.get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("name")?.as_str()?;
                    Some(CliProviderModel {
                        id: id.to_string(),
                        name: id.to_string(),
                        default: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::grok_info;

    #[test]
    fn grok_binary_on_path_is_detected_without_api_key() {
        let info = grok_info(true, false);
        assert!(info.installed && info.available);
        assert!(info.reason.is_none());
    }

    #[test]
    fn grok_api_key_alone_is_still_available() {
        assert!(grok_info(false, true).available);
    }

    #[test]
    fn grok_missing_reports_both_remedies() {
        let info = grok_info(false, false);
        assert!(!info.available);
        assert!(info.reason.unwrap().contains("grok"));
    }
}
