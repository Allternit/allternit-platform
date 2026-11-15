//! BYOK (bring-your-own-key) inference keys.
//!
//! Users attach their own provider API keys; their chat completions then route
//! through their key (they pay upstream; we meter tokens but deduct nothing —
//! see the BYOK branch in `routes::model_router::chat_completions`).
//!
//! Keys are encrypted at rest with the platform credential cipher
//! (`allternit_cloud_core::CredentialCipher`, env `ALLTERNIT_CREDENTIALS_KEY` —
//! the same key-management story as provider tokens). The cipher serializes to
//! `v1:<base64(nonce || ciphertext || tag)>`; this module splits that blob
//! into the `key_nonce` / `key_ciphertext` BYTEA columns of
//! `user_inference_keys` and reassembles it on read, so no second cipher or
//! key env exists.
//!
//! Plaintext keys are never logged and never leave the service: validation
//! error messages are static strings, and list endpoints return masked
//! fingerprints (`sk-…4f9c`) only.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use std::time::Duration;

use allternit_cloud_core::CredentialCipher;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::error::ApiError;

/// BYOK providers that route chat, as (provider_id, base URL). All but
/// Anthropic are OpenAI-compatible chat/completions hosts; Google is reached
/// through Gemini's own OpenAI-compatible endpoint (`/v1beta/openai`,
/// streaming supported). Anthropic goes through the native Messages API
/// (`model_router::anthropic`). OpenRouter keys route the catalog's
/// OpenRouter-served aliases through the person's own OpenRouter account.
pub const BYOK_PROVIDERS: &[(&str, &str)] = &[
    ("together", "https://api.together.xyz/v1"),
    ("fireworks", "https://api.fireworks.ai/inference/v1"),
    ("deepinfra", "https://api.deepinfra.com/v1/openai"),
    ("groq", "https://api.groq.com/openai/v1"),
    ("openai", "https://api.openai.com/v1"),
    ("deepseek", "https://api.deepseek.com"),
    ("kimi", "https://api.moonshot.ai/v1"),
    ("xai", "https://api.x.ai/v1"),
    ("openrouter", "https://openrouter.ai/api/v1"),
    ("google", "https://generativelanguage.googleapis.com/v1beta/openai"),
    ("anthropic", "https://api.anthropic.com"),
];

/// Providers whose own model list is saved with the key, so a person can pick
/// `<provider>/<model>` from it (they have no catalog aliases).
pub const LISTS_VENDOR_MODELS: &[&str] = &["anthropic", "google", "xai"];

/// At most this many vendor models are saved per key.
pub const MAX_SAVED_MODELS: usize = 50;

/// Base URL for a BYOK provider that routes chat, if any. Every key provider
/// routes chat.
pub fn byok_base_url(provider_id: &str) -> Option<&'static str> {
    BYOK_PROVIDERS
        .iter()
        .find(|(id, _)| *id == provider_id)
        .map(|(_, base_url)| *base_url)
}

/// Whether a key for this provider can be saved (routes chat or key-only).
pub fn is_key_provider(provider_id: &str) -> bool {
    byok_base_url(provider_id).is_some()
}

/// Every provider a key can be saved for, in a stable order.
pub fn key_provider_ids() -> Vec<&'static str> {
    BYOK_PROVIDERS.iter().map(|(id, _)| *id).collect()
}

/// A person's own model on their own key: `<provider>/<vendor model id>`
/// (`xai/grok-4`, `google/gemini-2.5-pro`). Used only when the id is not a
/// catalog alias. Returns (provider_id, vendor model id).
pub fn direct_model(model: &str) -> Option<(&'static str, &str)> {
    let (prefix, rest) = model.split_once('/')?;
    if rest.trim().is_empty() {
        return None;
    }
    key_provider_ids()
        .into_iter()
        .find(|id| *id == prefix)
        .map(|id| (id, rest))
}

/// How a provider's key is checked: one cheap authenticated GET.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCheck {
    pub url: String,
    pub auth: KeyAuth,
    /// The vendor answers a bad key with 400 (Google, xAI), not only 401/403.
    pub rejects_with_400: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAuth {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>` + `anthropic-version`.
    Anthropic,
    /// `?key=<key>` on the URL.
    QueryKey,
    /// `xi-api-key: <key>` (ElevenLabs).
    ElevenLabs,
}

/// The vendor check for a key provider (contract section 4): OpenAI-compatible
/// hosts `GET {base}/models`, Anthropic `GET /v1/models`, Google
/// `GET /v1beta/models?key=`, OpenRouter `GET /api/v1/key`. Also the
/// ElevenLabs voice key: `GET /v1/user` (`xi-api-key`), the same check the
/// voice key screens use.
pub fn key_check(provider_id: &str) -> Option<KeyCheck> {
    let check = |url: String, auth, rejects_with_400| KeyCheck { url, auth, rejects_with_400 };
    match provider_id {
        "anthropic" => Some(check("https://api.anthropic.com/v1/models?limit=100".into(), KeyAuth::Anthropic, false)),
        "elevenlabs" => Some(check("https://api.elevenlabs.io/v1/user".into(), KeyAuth::ElevenLabs, false)),
        "google" => Some(check(
            "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000".into(),
            KeyAuth::QueryKey,
            true,
        )),
        "openrouter" => Some(check("https://openrouter.ai/api/v1/key".into(), KeyAuth::Bearer, false)),
        "xai" => Some(check("https://api.x.ai/v1/models".into(), KeyAuth::Bearer, true)),
        other => byok_base_url(other)
            .map(|base| check(format!("{}/models", base.trim_end_matches('/')), KeyAuth::Bearer, false)),
    }
}

/// BYOK master switch (`BYOK_ENABLED`, default true).
pub fn byok_enabled() -> bool {
    std::env::var("BYOK_ENABLED")
        .map(|value| !matches!(value.trim(), "false" | "0"))
        .unwrap_or(true)
}

/// The BYOK routing decision, factored out of the axum handler: master switch
/// on, provider in the BYOK registry, and the user has an active stored key.
pub fn should_route_byok(enabled: bool, byok_registered: bool, has_key: bool) -> bool {
    enabled && byok_registered && has_key
}

/// Masked key fingerprint for list endpoints: first 3 + last 4 chars
/// (`sk-…4f9c`). Short or non-ASCII keys mask to a bare ellipsis.
pub fn mask_key(key: &str) -> String {
    if key.is_ascii() && key.len() >= 8 {
        format!("{}…{}", &key[..3], &key[key.len() - 4..])
    } else {
        "…".to_string()
    }
}

/// GCM nonce length, matching the cipher's serialization.
const NONCE_LEN: usize = 12;
/// The cipher's serialized blob prefix (see credentials.rs).
const CIPHER_VERSION_PREFIX: &str = "v1:";

/// Encrypt with the platform cipher, returning (ciphertext, nonce) parts for
/// the BYTEA columns. The plaintext never appears in error text.
pub(crate) fn encrypt_parts(cipher: &CredentialCipher, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
    let encoded = cipher
        .encrypt(plaintext)
        .map_err(|_| ApiError::Internal("Failed to encrypt the stored key".to_string()))?;
    let blob_b64 = encoded
        .strip_prefix(CIPHER_VERSION_PREFIX)
        .ok_or_else(|| ApiError::Internal("Unexpected cipher output format".to_string()))?;
    let blob = BASE64
        .decode(blob_b64)
        .map_err(|_| ApiError::Internal("Unexpected cipher output encoding".to_string()))?;
    if blob.len() < NONCE_LEN {
        return Err(ApiError::Internal("Unexpected cipher output length".to_string()));
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    Ok((ciphertext.to_vec(), nonce.to_vec()))
}

/// Reassemble the cipher's serialized blob from the BYTEA columns and decrypt.
pub(crate) fn decrypt_parts(
    cipher: &CredentialCipher,
    ciphertext: &[u8],
    nonce: &[u8],
) -> Result<String, ApiError> {
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(ciphertext);
    let encoded = format!("{}{}", CIPHER_VERSION_PREFIX, BASE64.encode(blob));
    cipher
        .decrypt(&encoded)
        .map_err(|_| ApiError::Internal("Failed to decrypt the stored key".to_string()))
}

/// The BYOK key-validation call, abstracted so tests can stub it (the crate's
/// existing pattern for outbound provider calls, e.g. the Stripe checkout
/// trait).
#[async_trait::async_trait]
pub trait KeyValidator: Send + Sync {
    /// The provider's [`key_check`]. Ok = key accepted, with the chat model
    /// ids the vendor listed (empty when the check lists none); `BadRequest` =
    /// the vendor rejected the key; `ServiceUnavailable` = it could not tell.
    async fn validate(&self, provider_id: &str, api_key: &str) -> Result<Vec<String>, ApiError>;
}

/// Chat model ids from a key check's body, capped at [`MAX_SAVED_MODELS`].
/// OpenAI-style and Anthropic lists carry `data[].id`; Google carries
/// `models[].name` (`models/gemini-…`) with the methods each supports.
pub fn models_from_check(provider_id: &str, body: &serde_json::Value) -> Vec<String> {
    let mut models: Vec<String> = if provider_id == "google" {
        body["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| {
                model["supportedGenerationMethods"]
                    .as_array()
                    .is_some_and(|methods| methods.iter().any(|m| m == "generateContent"))
            })
            .filter_map(|model| model["name"].as_str())
            .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string())
            .collect()
    } else {
        body["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| model["id"].as_str())
            .map(str::to_string)
            .collect()
    };
    let mut seen = std::collections::HashSet::new();
    models.retain(|id| !id.trim().is_empty() && seen.insert(id.clone()));
    models.truncate(MAX_SAVED_MODELS);
    models
}

/// The message for a rejected key. Callers match on `BadRequest`.
pub const KEY_REJECTED_MESSAGE: &str = "Key rejected by provider — check the key and try again.";

/// Production validator: 5s timeout, a rejection status → 400 "key rejected
/// by provider", anything else unreachable/unexpected → 503 "validation
/// unavailable". The key is never included in any message.
pub struct ReqwestKeyValidator {
    /// Tests only: send every check to this origin (a local fake vendor)
    /// instead of the vendor's host. Not settable outside tests.
    origin: Option<String>,
}

impl ReqwestKeyValidator {
    pub fn new() -> Self {
        Self { origin: None }
    }

    #[cfg(test)]
    pub(crate) fn with_origin(origin: impl Into<String>) -> Self {
        Self { origin: Some(origin.into()) }
    }

    fn target(&self, url: &str) -> String {
        match &self.origin {
            Some(origin) => {
                let path_start = url.find("://").map(|i| i + 3).and_then(|i| url[i..].find('/').map(|j| i + j));
                format!("{}{}", origin.trim_end_matches('/'), path_start.map(|i| &url[i..]).unwrap_or(""))
            }
            None => url.to_string(),
        }
    }
}

impl Default for ReqwestKeyValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl KeyValidator for ReqwestKeyValidator {
    async fn validate(&self, provider_id: &str, api_key: &str) -> Result<Vec<String>, ApiError> {
        let check = key_check(provider_id)
            .ok_or_else(|| ApiError::BadRequest(format!("Unknown BYOK provider: {provider_id:?}.")))?;
        let client = crate::net_client::client();
        let url = self.target(&check.url);
        let request = match check.auth {
            KeyAuth::Bearer => client.get(url).bearer_auth(api_key),
            KeyAuth::Anthropic => client
                .get(url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01"),
            KeyAuth::QueryKey => client.get(url).query(&[("key", api_key)]),
            KeyAuth::ElevenLabs => client.get(url).header("xi-api-key", api_key),
        };
        let response = request.timeout(Duration::from_secs(5)).send().await.map_err(|_| {
            ApiError::ServiceUnavailable(
                "Key validation is required but the provider could not be reached — please retry.".to_string(),
            )
        })?;
        let status = response.status();
        if status.is_success() {
            // A body we cannot read still means the key works; only the list is lost.
            let body: serde_json::Value = response.json().await.unwrap_or(serde_json::Value::Null);
            return Ok(models_from_check(provider_id, &body));
        }
        let rejected = status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
            || (check.rejects_with_400 && status == reqwest::StatusCode::BAD_REQUEST);
        if rejected {
            return Err(ApiError::BadRequest(KEY_REJECTED_MESSAGE.to_string()));
        }
        Err(ApiError::ServiceUnavailable(
            "Key validation is required but the provider returned an unexpected status — please retry."
                .to_string(),
        ))
    }
}

/// One stored BYOK key, API-facing (masked; never the plaintext).
#[derive(Debug, Clone, serde::Serialize)]
pub struct InferenceKeyInfo {
    pub provider_id: String,
    pub masked: String,
    pub status: String,
    pub last_validated_at: Option<DateTime<Utc>>,
    /// The vendor's chat models seen at the last check (providers in
    /// [`LISTS_VENDOR_MODELS`]; empty otherwise), ids without a prefix.
    pub models: Vec<String>,
}

#[derive(Debug, FromRow)]
struct KeyRow {
    provider_id: String,
    key_ciphertext: Vec<u8>,
    key_nonce: Vec<u8>,
    status: String,
    last_validated_at: Option<DateTime<Utc>>,
    models: sqlx::types::Json<Vec<String>>,
}

pub struct InferenceKeyService {
    db: PgPool,
    cipher: Arc<CredentialCipher>,
    validator: Arc<dyn KeyValidator>,
}

impl InferenceKeyService {
    pub fn new(db: PgPool, cipher: Arc<CredentialCipher>) -> Self {
        Self {
            db,
            cipher,
            validator: Arc::new(ReqwestKeyValidator::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_validator(db: PgPool, cipher: Arc<CredentialCipher>, validator: Arc<dyn KeyValidator>) -> Self {
        Self { db, cipher, validator }
    }

    /// Validate the key against the provider, then encrypt and upsert it.
    /// Unknown providers are a 400; validation failures carry no key material.
    pub async fn upsert_and_validate(
        &self,
        user_id: &str,
        provider_id: &str,
        plaintext_key: &str,
    ) -> Result<InferenceKeyInfo, ApiError> {
        if !is_key_provider(provider_id) {
            return Err(ApiError::BadRequest(format!(
                "Unknown BYOK provider: {:?}. Supported: {}.",
                provider_id,
                key_provider_ids().join(", ")
            )));
        }
        let plaintext_key = plaintext_key.trim();
        if plaintext_key.is_empty() {
            return Err(ApiError::BadRequest("The API key must not be empty.".to_string()));
        }

        let listed = self.validator.validate(provider_id, plaintext_key).await?;
        let models = if LISTS_VENDOR_MODELS.contains(&provider_id) { listed } else { Vec::new() };

        let (ciphertext, nonce) = encrypt_parts(&self.cipher, plaintext_key)?;
        sqlx::query(
            r#"
            INSERT INTO user_inference_keys (
                user_id, provider_id, key_ciphertext, key_nonce, status, last_validated_at, updated_at, models
            ) VALUES ($1, $2, $3, $4, 'active', NOW(), NOW(), $5)
            ON CONFLICT (user_id, provider_id) DO UPDATE SET
                key_ciphertext = excluded.key_ciphertext,
                key_nonce = excluded.key_nonce,
                status = 'active',
                last_validated_at = NOW(),
                updated_at = NOW(),
                models = excluded.models
            "#,
        )
        .bind(user_id)
        .bind(provider_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(sqlx::types::Json(&models))
        .execute(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        Ok(InferenceKeyInfo {
            provider_id: provider_id.to_string(),
            masked: mask_key(plaintext_key),
            status: "active".to_string(),
            last_validated_at: Some(Utc::now()),
            models,
        })
    }

    /// Decrypt a user's active key for one provider (hot path: BYOK routing).
    pub async fn get_decrypted(
        &self,
        user_id: &str,
        provider_id: &str,
    ) -> Result<Option<String>, ApiError> {
        let row: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT key_ciphertext, key_nonce
            FROM user_inference_keys
            WHERE user_id = $1 AND provider_id = $2 AND status = 'active'
            "#,
        )
        .bind(user_id)
        .bind(provider_id)
        .fetch_optional(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;
        row.map(|(ciphertext, nonce)| decrypt_parts(&self.cipher, &ciphertext, &nonce))
            .transpose()
    }

    /// Ask each provider whether the user's stored key still works (the same
    /// `GET {base}/models` as at save time). `Ok(true)` accepted, `Ok(false)`
    /// rejected (401/403), `Err(())` the provider could not be reached or
    /// answered something else. Nothing is stored or returned but verdicts.
    pub async fn verdicts_for_user(&self, user_id: &str) -> Vec<(String, Result<bool, ()>)> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT provider_id FROM user_inference_keys WHERE user_id = $1 AND status = 'active' ORDER BY provider_id",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();
        let calls = rows.into_iter().map(|(provider_id,)| async move {
            let verdict = match (
                is_key_provider(&provider_id),
                self.get_decrypted(user_id, &provider_id).await,
            ) {
                (true, Ok(Some(key))) => match self.validator.validate(&provider_id, &key).await {
                    Ok(_) => Ok(true),
                    Err(ApiError::BadRequest(_)) => Ok(false),
                    Err(_) => Err(()),
                },
                _ => Err(()),
            };
            (provider_id, verdict)
        });
        futures::future::join_all(calls).await
    }

    /// Cheap existence check for the BYOK routing hot path (no decrypt).
    pub async fn has_key(&self, user_id: &str, provider_id: &str) -> Result<bool, ApiError> {
        sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM user_inference_keys
                WHERE user_id = $1 AND provider_id = $2 AND status = 'active'
            )
            "#,
        )
        .bind(user_id)
        .bind(provider_id)
        .fetch_one(&self.db)
        .await
        .map_err(ApiError::DatabaseError)
    }

    /// List a user's keys, masked. Decrypts each row in memory to compute the
    /// fingerprint — keys are never logged or returned.
    pub async fn list_for_user(&self, user_id: &str) -> Result<Vec<InferenceKeyInfo>, ApiError> {
        let rows = sqlx::query_as::<_, KeyRow>(
            r#"
            SELECT provider_id, key_ciphertext, key_nonce, status, last_validated_at, models
            FROM user_inference_keys
            WHERE user_id = $1
            ORDER BY provider_id
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;
        rows.into_iter()
            .map(|row| {
                let masked = decrypt_parts(&self.cipher, &row.key_ciphertext, &row.key_nonce)
                    .map(|key| mask_key(&key))
                    .unwrap_or_else(|_| "…".to_string());
                Ok(InferenceKeyInfo {
                    provider_id: row.provider_id,
                    masked,
                    status: row.status,
                    last_validated_at: row.last_validated_at,
                    models: row.models.0,
                })
            })
            .collect()
    }

    /// Check a key with the vendor without storing it (the voice OpenAI key
    /// reuses the OpenAI check).
    pub async fn check_only(&self, provider_id: &str, plaintext_key: &str) -> Result<(), ApiError> {
        self.validator.validate(provider_id, plaintext_key.trim()).await.map(|_| ())
    }

    /// Delete a key. Returns false when nothing existed.
    pub async fn delete(&self, user_id: &str, provider_id: &str) -> Result<bool, ApiError> {
        let result = sqlx::query(
            "DELETE FROM user_inference_keys WHERE user_id = $1 AND provider_id = $2",
        )
        .bind(user_id)
        .bind(provider_id)
        .execute(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS user_inference_keys CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE user_inference_keys (
                user_id TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                key_ciphertext BYTEA NOT NULL,
                key_nonce BYTEA NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                last_validated_at TIMESTAMPTZ,
                models JSONB NOT NULL DEFAULT '[]'::jsonb,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (user_id, provider_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    struct StubValidator(Result<(), &'static str>);

    #[async_trait::async_trait]
    impl KeyValidator for StubValidator {
        async fn validate(&self, provider_id: &str, _api_key: &str) -> Result<Vec<String>, ApiError> {
            match &self.0 {
                Ok(()) => Ok(vec![format!("{provider_id}-model-1")]),
                Err("reject") => Err(ApiError::BadRequest(
                    "Key rejected by provider — check the key and try again.".to_string(),
                )),
                Err(_) => Err(ApiError::ServiceUnavailable(
                    "Key validation is required but the provider could not be reached — please retry."
                        .to_string(),
                )),
            }
        }
    }

    fn test_service(db: PgPool, validator: Result<(), &'static str>) -> InferenceKeyService {
        InferenceKeyService::with_validator(
            db,
            Arc::new(CredentialCipher::new("test cipher key material")),
            Arc::new(StubValidator(validator)),
        )
    }

    #[tokio::test]
    async fn encryption_roundtrip_through_the_real_cipher() {
        let pool = test_pool().await;
        let service = test_service(pool, Ok(()));
        let plaintext = "sk-together-abc123def456";

        let info = service
            .upsert_and_validate("user_1", "together", plaintext)
            .await
            .unwrap();
        assert_eq!(info.masked, "sk-…f456");
        assert_eq!(info.status, "active");
        assert!(info.last_validated_at.is_some());

        let decrypted = service.get_decrypted("user_1", "together").await.unwrap();
        assert_eq!(decrypted.as_deref(), Some(plaintext));

        // Nothing plaintext-shaped sits in the table.
        let (ciphertext, nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT key_ciphertext, key_nonce FROM user_inference_keys WHERE user_id = 'user_1'",
        )
        .fetch_one(service_pool(&service))
        .await
        .unwrap();
        assert!(!ciphertext.windows(plaintext.len()).any(|w| w == plaintext.as_bytes()));
        assert_eq!(nonce.len(), NONCE_LEN);
    }

    fn service_pool(service: &InferenceKeyService) -> &PgPool {
        &service.db
    }

    #[tokio::test]
    async fn upsert_replaces_existing_key_and_validation_failures_store_nothing() {
        let pool = test_pool().await;
        let service = test_service(pool, Ok(()));
        service
            .upsert_and_validate("user_1", "groq", "gsk_first_key_xxxxx")
            .await
            .unwrap();
        service
            .upsert_and_validate("user_1", "groq", "gsk_second_key_yyy")
            .await
            .unwrap();
        assert_eq!(
            service.get_decrypted("user_1", "groq").await.unwrap().as_deref(),
            Some("gsk_second_key_yyy"),
            "upsert replaces the stored key"
        );
        let rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_inference_keys WHERE user_id = 'user_1'",
        )
        .fetch_one(service_pool(&service))
        .await
        .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn rejected_or_unreachable_validation_stores_nothing() {
        let pool = test_pool().await;

        let rejected = test_service(pool.clone(), Err("reject"));
        let error = rejected
            .upsert_and_validate("user_1", "groq", "gsk_bad_key_xxxxxxx")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("rejected by provider"), "{error}");

        let unreachable = test_service(pool.clone(), Err("down"));
        let error = unreachable
            .upsert_and_validate("user_1", "groq", "gsk_some_key_xxxxxx")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("could not be reached"), "{error}");

        assert!(!unreachable.has_key("user_1", "groq").await.unwrap());
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_inference_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0, "failed validation stores nothing");
    }

    #[tokio::test]
    async fn list_masks_and_delete_removes() {
        let pool = test_pool().await;
        let service = test_service(pool, Ok(()));
        service
            .upsert_and_validate("user_1", "groq", "gsk_live_abcdef9999")
            .await
            .unwrap();
        service
            .upsert_and_validate("user_1", "deepinfra", "di_key_12345abcd")
            .await
            .unwrap();

        let keys = service.list_for_user("user_1").await.unwrap();
        assert_eq!(keys.len(), 2);
        let groq = keys.iter().find(|k| k.provider_id == "groq").unwrap();
        assert_eq!(groq.masked, "gsk…9999");
        let deepinfra = keys.iter().find(|k| k.provider_id == "deepinfra").unwrap();
        assert_eq!(deepinfra.masked, "di_…abcd");
        assert!(
            serde_json::to_string(&keys).unwrap().find("abcdef9999").is_none(),
            "plaintext never appears in the API shape"
        );

        assert!(service.delete("user_1", "groq").await.unwrap());
        assert!(!service.delete("user_1", "groq").await.unwrap(), "second delete is false");
        assert!(!service.has_key("user_1", "groq").await.unwrap());
        assert!(service.has_key("user_1", "deepinfra").await.unwrap());
    }

    #[tokio::test]
    async fn unknown_provider_is_a_400() {
        let pool = test_pool().await;
        let service = test_service(pool, Ok(()));
        let error = service
            .upsert_and_validate("user_1", "mistral", "sk-mistral-xxxxxxxx")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Unknown BYOK provider"), "{error}");
        // A vendor's model list is saved for the providers that have no
        // catalog aliases, and replaced on the next check.
        let info = service.upsert_and_validate("user_1", "anthropic", "sk-ant-xxxxxxxxxx").await.unwrap();
        assert_eq!(info.models, vec!["anthropic-model-1"]);
        let listed = service.list_for_user("user_1").await.unwrap();
        assert_eq!(listed[0].models, vec!["anthropic-model-1"]);
        let groq = service.upsert_and_validate("user_1", "groq", "gsk_xxxxxxxxxxxxxx").await.unwrap();
        assert!(groq.models.is_empty(), "OpenAI-compatible hosts keep catalog aliases instead");
    }

    #[test]
    fn masked_fingerprint_format() {
        assert_eq!(mask_key("sk-abc1234f9c"), "sk-…4f9c");
        assert_eq!(mask_key("gsk_live_abcdef9999"), "gsk…9999");
        assert_eq!(mask_key("short"), "…", "short keys mask to a bare ellipsis");
        assert_eq!(mask_key("ünïcode-key"), "…", "non-ASCII masks safely");
    }

    #[test]
    fn byok_routing_decision() {
        assert!(should_route_byok(true, true, true));
        assert!(!should_route_byok(false, true, true), "master switch off");
        assert!(!should_route_byok(true, false, true), "provider not BYOK-registered");
        assert!(!should_route_byok(true, true, false), "no stored key");
    }

    #[test]
    fn direct_models_need_a_known_prefix() {
        assert!(is_key_provider("anthropic"));
        assert!(!is_key_provider("elevenlabs"), "the voice key is not an inference key");
        assert_eq!(direct_model("xai/grok-4"), Some(("xai", "grok-4")));
        assert_eq!(
            direct_model("openrouter/anthropic/claude-sonnet-5.5"),
            Some(("openrouter", "anthropic/claude-sonnet-5.5"))
        );
        assert_eq!(direct_model("meta-llama/llama-3.1-8b-instruct"), None);
        assert_eq!(direct_model("xai/"), None);
        assert_eq!(direct_model("gpt-5"), None);
    }

    /// The real validator against a local fake vendor: each provider's path,
    /// auth placement, rejection status and model list (contract section 4).
    #[tokio::test]
    async fn validator_sends_each_vendor_its_own_check() {
        use axum::{extract::Query, http::HeaderMap, http::StatusCode, routing::get, Json, Router};
        use serde_json::json;
        use std::collections::HashMap;
        const GOOD: &str = "good-key-123456";
        fn bearer_ok(headers: &HeaderMap) -> bool {
            headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer good-key-123456")
        }
        let app = Router::new()
            .route(
                "/v1/models",
                get(|headers: HeaderMap| async move {
                    // Anthropic (x-api-key + version) and xAI (Bearer) share this path.
                    let anthropic = headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(GOOD)
                        && headers.get("anthropic-version").is_some();
                    if anthropic {
                        (StatusCode::OK, Json(json!({ "data": [{ "id": "claude-opus-5-5" }, { "id": "claude-sonnet-5-5" }] })))
                    } else if bearer_ok(&headers) {
                        (StatusCode::OK, Json(json!({ "data": [{ "id": "grok-4" }, { "id": "grok-4" }] })))
                    } else if headers.get("x-api-key").is_some() {
                        (StatusCode::UNAUTHORIZED, Json(json!({})))
                    } else {
                        // xAI answers a bad key with 400.
                        (StatusCode::BAD_REQUEST, Json(json!({})))
                    }
                }),
            )
            .route(
                "/v1beta/models",
                get(|Query(query): Query<HashMap<String, String>>| async move {
                    if query.get("key").map(String::as_str) == Some(GOOD) {
                        (StatusCode::OK, Json(json!({ "models": [
                            { "name": "models/gemini-2.5-pro", "supportedGenerationMethods": ["generateContent", "countTokens"] },
                            { "name": "models/text-embedding-004", "supportedGenerationMethods": ["embedContent"] },
                        ]})))
                    } else {
                        (StatusCode::BAD_REQUEST, Json(json!({})))
                    }
                }),
            )
            .route(
                "/api/v1/key",
                get(|headers: HeaderMap| async move {
                    if bearer_ok(&headers) {
                        (StatusCode::OK, Json(json!({ "data": { "label": "sk-or-…" } })))
                    } else {
                        (StatusCode::UNAUTHORIZED, Json(json!({})))
                    }
                }),
            )
            .route(
                "/v1/user",
                get(|headers: HeaderMap| async move {
                    if headers.get("xi-api-key").and_then(|v| v.to_str().ok()) == Some(GOOD) {
                        StatusCode::OK
                    } else {
                        StatusCode::UNAUTHORIZED
                    }
                }),
            )
            .route("/inference/v1/models", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let validator = ReqwestKeyValidator::with_origin(origin);

        let expected: [(&str, &[&str]); 5] = [
            ("anthropic", &["claude-opus-5-5", "claude-sonnet-5-5"]),
            ("google", &["gemini-2.5-pro"]),
            ("xai", &["grok-4"]),
            ("openrouter", &[]),
            ("elevenlabs", &[]),
        ];
        for (provider, models) in expected {
            let listed = validator.validate(provider, GOOD).await.unwrap_or_else(|e| panic!("{provider}: {e}"));
            assert_eq!(listed, models, "{provider} model list");
            let error = validator.validate(provider, "bad-key-000000").await.unwrap_err();
            assert!(matches!(error, ApiError::BadRequest(_)), "{provider} rejects a bad key: {error}");
        }
        // A vendor error that is not about the key is "could not tell", not a rejection.
        let error = validator.validate("fireworks", GOOD).await.unwrap_err();
        assert!(matches!(error, ApiError::ServiceUnavailable(_)), "{error}");
    }
}
