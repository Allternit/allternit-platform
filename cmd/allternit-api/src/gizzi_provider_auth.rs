//! Provider credentials belong to the Gizzi runtime.
//!
//! `allternit-api` may ask Gizzi to store a credential or query connection
//! metadata, but it never persists or reads the credential itself. This keeps
//! the frequently rebuilt API binary away from macOS Keychain and gives every
//! desktop/VPS runtime one provider-auth authority.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::{header, Client};
use serde_json::json;
use std::collections::HashSet;

use crate::config::AppConfig;

fn base_url() -> String {
    AppConfig::load()
        .terminal_server_url()
        .trim_end_matches('/')
        .to_string()
}

fn client() -> Result<Client, String> {
    client_with_timeout(std::time::Duration::from_secs(15))
}

/// Default headers for any allternit-api -> gizzi-code call: HTTP basic auth
/// from `GIZZI_PASSWORD` / `GIZZI_SERVER_PASSWORD` when set (the service login
/// a Clerk-protected gizzi accepts), otherwise none (loopback dev mode).
pub fn gizzi_auth_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    let password = std::env::var("GIZZI_PASSWORD")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("GIZZI_SERVER_PASSWORD").ok().filter(|v| !v.is_empty()));
    if let Some(password) = password {
        let username = std::env::var("GIZZI_USERNAME")
            .or_else(|_| std::env::var("GIZZI_SERVER_USERNAME"))
            .unwrap_or_else(|_| "gizzi".to_string());
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!(
            "Basic {}",
            STANDARD.encode(format!("{username}:{password}"))
        )) {
            headers.insert(header::AUTHORIZATION, value);
        }
    }
    headers
}

fn client_with_timeout(timeout: std::time::Duration) -> Result<Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    let password = std::env::var("GIZZI_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("GIZZI_SERVER_PASSWORD")
                .ok()
                .filter(|value| !value.is_empty())
        });
    if let Some(password) = password {
        let username = std::env::var("GIZZI_USERNAME")
            .or_else(|_| std::env::var("GIZZI_SERVER_USERNAME"))
            .unwrap_or_else(|_| "gizzi".to_string());
        let value = reqwest::header::HeaderValue::from_str(&format!(
            "Basic {}",
            STANDARD.encode(format!("{username}:{password}"))
        ))
        .map_err(|error| format!("Invalid Gizzi authorization header: {error}"))?;
        headers.insert(header::AUTHORIZATION, value);
    }
    Client::builder()
        .default_headers(headers)
        .timeout(timeout)
        .build()
        .map_err(|error| format!("Failed to create Gizzi client: {error}"))
}

/// Ask Gizzi to execute a provider operation with its own credential. Only the
/// operation result crosses back; the provider key never leaves Gizzi.
pub async fn generate_video(
    payload: serde_json::Value,
) -> Result<(u16, serde_json::Value), String> {
    generate_media("video".to_string(), payload).await
}

pub async fn list_media_providers() -> Result<serde_json::Value, String> {
    let response = client()?
        .get(format!("{}/provider/media/providers", base_url()))
        .send()
        .await
        .map_err(|error| format!("Could not reach Gizzi media providers: {error}"))?;
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Gizzi returned an invalid media providers response: {error}"))
}

pub async fn list_media_providers_for_mode(mode: String) -> Result<serde_json::Value, String> {
    let response = client()?
        .get(format!(
            "{}/provider/media/{}/providers",
            base_url(),
            urlencoding::encode(&mode)
        ))
        .send()
        .await
        .map_err(|error| format!("Could not reach Gizzi media providers: {error}"))?;
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Gizzi returned an invalid media providers response: {error}"))
}

pub async fn generate_media(
    mode: String,
    payload: serde_json::Value,
) -> Result<(u16, serde_json::Value), String> {
    let timeout = if mode == "video" {
        std::time::Duration::from_secs(330)
    } else {
        std::time::Duration::from_secs(120)
    };
    let response = client_with_timeout(timeout)?
        .post(format!(
            "{}/provider/media/{}/generate",
            base_url(),
            urlencoding::encode(&mode)
        ))
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("Could not reach Gizzi media provider: {error}"))?;
    let status = response.status().as_u16();
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Gizzi returned an invalid media response: {error}"))?;
    Ok((status, payload))
}

/// Hand an API key directly to Gizzi's mode-0600 credential store.
pub async fn store_api_key(provider_id: &str, key: &str) -> Result<(), String> {
    let response = client()?
        .put(format!(
            "{}/auth/{}",
            base_url(),
            urlencoding::encode(provider_id)
        ))
        .json(&json!({ "type": "api", "key": key }))
        .send()
        .await
        .map_err(|error| format!("Could not reach the Gizzi credential broker: {error}"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "Gizzi rejected the provider credential ({})",
            response.status()
        ))
    }
}

/// Return provider IDs Gizzi reports as connected. No secret values cross this
/// boundary; callers receive connection metadata only.
pub async fn connected_provider_ids() -> HashSet<String> {
    let Ok(client) = client() else {
        return HashSet::new();
    };
    let Ok(response) = client.get(format!("{}/provider", base_url())).send().await else {
        return HashSet::new();
    };
    let Ok(payload) = response.json::<serde_json::Value>().await else {
        return HashSet::new();
    };
    payload
        .get("connected")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect()
}

/// Ask Gizzi for the full live provider discovery payload.
pub async fn discover_providers() -> Result<serde_json::Value, String> {
    let response = client()?
        .get(format!("{}/provider", base_url()))
        .send()
        .await
        .map_err(|error| format!("Could not reach Gizzi provider discovery: {error}"))?;
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Gizzi returned an invalid provider discovery response: {error}"))
}

/// Ask Gizzi for provider authentication-method metadata.
pub async fn provider_auth_methods() -> Result<serde_json::Value, String> {
    let response = client()?
        .get(format!("{}/provider/auth", base_url()))
        .send()
        .await
        .map_err(|error| format!("Could not reach Gizzi provider auth methods: {error}"))?;
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("Gizzi returned an invalid provider auth methods response: {error}"))
}
