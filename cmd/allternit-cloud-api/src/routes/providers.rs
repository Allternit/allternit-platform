//! Provider routes
//!
//! Static provider catalog, REAL credential validation (Hetzner, DigitalOcean,
//! AWS, universal SSH), and per-user provider token management. Tokens are
//! validated against the provider API before storage, encrypted at rest with
//! the credential cipher (migration 019 `provider_tokens`), and never
//! echoed back: reads return only `configured: true`.
//!
//! AWS credentials are a single token string holding JSON:
//! `{"access_key_id": "...", "secret_access_key": "...", "region": "us-east-1"}`
//! — validated live via STS GetCallerIdentity.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{delete, get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{ApiError, ApiState};

/// Providers with an automated API driver (wizard `provider.rs`).
const AUTOMATED_PROVIDERS: &[&str] = &["hetzner", "digitalocean", "aws"];

/// Providers that can hold a stored API token.
const STORABLE_PROVIDERS: &[&str] = &["hetzner", "digitalocean", "aws"];

pub fn clerk_routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/provider-tokens", get(list_provider_tokens))
        .route("/api/v1/provider-tokens/:provider", put(put_provider_token))
        .route(
            "/api/v1/provider-tokens/:provider",
            delete(delete_provider_token),
        )
}

/// List all providers
pub async fn list_providers(
    _state: State<Arc<ApiState>>,
) -> Result<Json<Vec<ProviderResponse>>, ApiError> {
    // Return static list of providers
    let providers = vec![
        ProviderResponse {
            id: "hetzner".to_string(),
            name: "Hetzner Cloud".to_string(),
            automated: true,
        },
        ProviderResponse {
            id: "digitalocean".to_string(),
            name: "DigitalOcean".to_string(),
            automated: true,
        },
        ProviderResponse {
            id: "aws".to_string(),
            name: "Amazon Web Services".to_string(),
            automated: true,
        },
        ProviderResponse {
            id: "contabo".to_string(),
            name: "Contabo".to_string(),
            automated: false,
        },
        ProviderResponse {
            id: "racknerd".to_string(),
            name: "RackNerd".to_string(),
            automated: false,
        },
        ProviderResponse {
            id: "ssh".to_string(),
            name: "Any VPS (universal SSH)".to_string(),
            automated: false,
        },
    ];

    Ok(Json(providers))
}

/// Validate a Hetzner/DigitalOcean API token against the provider's API.
/// Returns `Ok(())` on 2xx, `Err(message)` otherwise; the token is never
/// included in the message.
///
/// AWS is validated separately: its "token" is JSON
/// (`{"access_key_id","secret_access_key","region"}`), checked live via STS
/// GetCallerIdentity (wizard `aws.rs`, hand-rolled SigV4).
async fn validate_provider_token(provider: &str, token: &str) -> Result<(), String> {
    if token.trim().is_empty() {
        return Err("token must not be empty".to_string());
    }
    if provider == "aws" {
        return allternit_cloud_wizard::validate_aws_credentials(token).await;
    }
    let url = match provider {
        "hetzner" => "https://api.hetzner.cloud/v1/servers",
        "digitalocean" => "https://api.digitalocean.com/v2/account",
        other => {
            return Err(format!(
                "no automated validation for {} - use SSH mode",
                other
            ))
        }
    };

    let client = crate::net_client::client();
    let response = client
        .get(url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("{} API unreachable: {}", provider, e))?;

    if response.status().is_success() {
        Ok(())
    } else if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        Err(format!("invalid {} API token", provider))
    } else {
        Err(format!("{} API error: {}", provider, response.status()))
    }
}

/// Validate provider credentials (real API calls).
///
/// - hetzner/digitalocean: `api_key` is the provider token, checked live.
/// - aws: `api_key` is the JSON credential string
///   (`{"access_key_id","secret_access_key","region"}`), checked live via STS.
/// - ssh: `api_key` is the SSH private key (or `api_secret` the password);
///   `ssh_host`/`ssh_username` are required — a real connection is attempted.
/// - anything else: not validatable, use SSH mode.
pub async fn validate_credentials(
    State(_state): State<Arc<ApiState>>,
    Path(provider_id): Path<String>,
    Json(request): Json<ValidateCredentialsRequest>,
) -> Result<Json<ValidateCredentialsResponse>, ApiError> {
    let provider_id = provider_id.to_lowercase();

    let (valid, message) = if provider_id == "ssh" {
        match (&request.ssh_host, &request.ssh_username) {
            (Some(host), Some(username)) => {
                let checker = allternit_cloud_wizard::PreflightChecker::new();
                let key = (!request.api_key.is_empty()).then_some(request.api_key.as_str());
                let password =
                    (!request.api_secret.is_empty()).then_some(request.api_secret.as_str());
                match checker
                    .validate_ssh_connection(
                        host,
                        request.ssh_port.unwrap_or(22),
                        username,
                        key,
                        password,
                    )
                    .await
                {
                    Ok(()) => (true, "SSH connection validated successfully".to_string()),
                    Err(e) => (false, format!("SSH validation failed: {}", e)),
                }
            }
            _ => (
                false,
                "ssh_host and ssh_username are required for SSH validation".to_string(),
            ),
        }
    } else if AUTOMATED_PROVIDERS.contains(&provider_id.as_str()) {
        match validate_provider_token(&provider_id, &request.api_key).await {
            Ok(()) => (true, "Credentials validated successfully".to_string()),
            Err(message) => (false, message),
        }
    } else {
        (
            false,
            format!("no automated validation for {} - use SSH mode", provider_id),
        )
    };

    Ok(Json(ValidateCredentialsResponse {
        provider_id,
        valid,
        message,
    }))
}

/// `provider_tokens.user_id` references `users(id)`; backfill a
/// placeholder row for users who have never touched another flow (mirrors
/// the gizzi_instances device-token pattern).
async fn ensure_user_row(db: &sqlx::PgPool, user_id: &str) -> Result<(), ApiError> {
    let email = format!("{}@users.allternit.local", user_id.replace('@', "_"));
    sqlx::query(
        r#"
        INSERT INTO users (id, email, status, last_login_at)
        VALUES ($1, $2, 'active', CURRENT_TIMESTAMP)
        ON CONFLICT(id) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(&email)
    .execute(db)
    .await?;
    Ok(())
}

/// Load and decrypt a user's stored provider token. `pub(crate)` for the
/// instance lifecycle handlers (restart/destroy credential lookup).
pub(crate) async fn load_provider_token(
    state: &ApiState,
    user_id: &str,
    provider: &str,
) -> Result<Option<String>, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT encrypted_token FROM provider_tokens WHERE user_id = $1 AND provider = $2",
    )
    .bind(user_id)
    .bind(provider)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some((stored,)) => match &state.credential_cipher {
            Some(cipher) => {
                let token = cipher
                    .decrypt(&stored)
                    .map_err(|e| ApiError::Internal(format!("credential decrypt failed: {}", e)))?;
                Ok(Some(token))
            }
            None => Ok(Some(stored)),
        },
        None => Ok(None),
    }
}

/// List which providers the authenticated user has tokens for (never the
/// tokens themselves).
async fn list_provider_tokens(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT provider FROM provider_tokens WHERE user_id = $1 ORDER BY provider",
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await?;
    let configured: std::collections::HashSet<String> =
        rows.into_iter().map(|(p,)| p).collect();

    let providers: Vec<serde_json::Value> = STORABLE_PROVIDERS
        .iter()
        .map(|provider| {
            serde_json::json!({
                "provider": provider,
                "configured": configured.contains(*provider),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "providers": providers })))
}

/// Store (or replace) a provider token. The token is validated against the
/// provider API first; only valid tokens are stored.
///
/// For `aws` the token body must be the JSON credential string
/// `{"access_key_id":"...","secret_access_key":"...","region":"..."}` —
/// malformed JSON is rejected with a 400 naming the expected shape, and the
/// parsed credentials are validated via STS before storage.
async fn put_provider_token(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(body): Json<PutProviderTokenRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let provider = provider.to_lowercase();

    if !STORABLE_PROVIDERS.contains(&provider.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "no token storage for {} - supply SSH credentials per wizard session instead",
            provider
        )));
    }

    // Validate before storing — never persist a dead token.
    validate_provider_token(&provider, &body.token)
        .await
        .map_err(ApiError::BadRequest)?;

    let stored = match &state.credential_cipher {
        Some(cipher) => cipher
            .encrypt(&body.token)
            .map_err(|e| ApiError::Internal(format!("credential encrypt failed: {}", e)))?,
        None => {
            tracing::warn!("Storing provider token PLAINTEXT (ALLTERNIT_CREDENTIALS_KEY unset)");
            body.token
        }
    };

    ensure_user_row(&state.db, &user_id).await?;
    sqlx::query(
        r#"
        INSERT INTO provider_tokens (user_id, provider, encrypted_token)
        VALUES ($1, $2, $3)
        ON CONFLICT(user_id, provider) DO UPDATE SET
            encrypted_token = excluded.encrypted_token,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(&user_id)
    .bind(&provider)
    .bind(&stored)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({
        "provider": provider,
        "configured": true,
    })))
}

/// Remove a stored provider token.
async fn delete_provider_token(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(provider): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let provider = provider.to_lowercase();

    let affected = sqlx::query(
        "DELETE FROM provider_tokens WHERE user_id = $1 AND provider = $2",
    )
    .bind(&user_id)
    .bind(&provider)
    .execute(&state.db)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(ApiError::NotFound(format!(
            "no {} token configured",
            provider
        )));
    }
    Ok(Json(serde_json::json!({
        "provider": provider,
        "configured": false,
    })))
}

#[derive(Debug, Serialize)]
pub struct ProviderResponse {
    pub id: String,
    pub name: String,
    pub automated: bool,
}

#[derive(Debug, Deserialize)]
pub struct ValidateCredentialsRequest {
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    /// SSH mode only
    #[serde(default)]
    pub ssh_host: Option<String>,
    #[serde(default)]
    pub ssh_port: Option<u16>,
    /// SSH mode only
    #[serde(default)]
    pub ssh_username: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ValidateCredentialsResponse {
    pub provider_id: String,
    pub valid: bool,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct PutProviderTokenRequest {
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_token_is_rejected_before_any_network_call() {
        assert!(validate_provider_token("hetzner", "   ").await.is_err());
        assert!(validate_provider_token("digitalocean", "").await.is_err());
        assert!(validate_provider_token("aws", "").await.is_err());
    }

    #[tokio::test]
    async fn unsupported_providers_route_to_ssh_mode_message() {
        let err = validate_provider_token("contabo", "token").await.unwrap_err();
        assert!(err.contains("use SSH mode"), "got: {}", err);
        let err = validate_provider_token("racknerd", "token").await.unwrap_err();
        assert!(err.contains("use SSH mode"), "got: {}", err);
    }

    #[tokio::test]
    async fn aws_rejects_malformed_json_before_any_network_call() {
        // Must fail locally (JSON shape error) — never reaching STS.
        let err = validate_provider_token("aws", "plain-token").await.unwrap_err();
        assert!(err.contains("access_key_id"), "got: {}", err);
        let err = validate_provider_token("aws", "{\"access_key_id\":\"AKIA\"}")
            .await
            .unwrap_err();
        assert!(err.contains("access_key_id"), "got: {}", err);
    }

    #[test]
    fn response_types_never_carry_the_token() {
        // The response structs must not gain a token field by accident.
        let response = ValidateCredentialsResponse {
            provider_id: "hetzner".to_string(),
            valid: true,
            message: "ok".to_string(),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert!(!json.to_string().contains("token"));
    }
}
