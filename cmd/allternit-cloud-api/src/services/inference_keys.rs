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

/// BYOK-capable providers: OpenAI-compatible chat/completions hosts only, as
/// (provider_id, base URL). Anthropic is excluded (messages API, different
/// protocol); OpenRouter is excluded from BYOK v1 (its per-token pricing
/// extras differ from the shared-router path).
pub const BYOK_PROVIDERS: &[(&str, &str)] = &[
    ("together", "https://api.together.xyz/v1"),
    ("fireworks", "https://api.fireworks.ai/inference/v1"),
    ("deepinfra", "https://api.deepinfra.com/v1/openai"),
    ("groq", "https://api.groq.com/openai/v1"),
    ("openai", "https://api.openai.com/v1"),
    ("deepseek", "https://api.deepseek.com"),
    ("kimi", "https://api.moonshot.ai/v1"),
];

/// Base URL for a BYOK-registered provider, if any.
pub fn byok_base_url(provider_id: &str) -> Option<&'static str> {
    BYOK_PROVIDERS
        .iter()
        .find(|(id, _)| *id == provider_id)
        .map(|(_, base_url)| *base_url)
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
fn encrypt_parts(cipher: &CredentialCipher, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
    let encoded = cipher
        .encrypt(plaintext)
        .map_err(|_| ApiError::Internal("Failed to encrypt the inference key".to_string()))?;
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
fn decrypt_parts(
    cipher: &CredentialCipher,
    ciphertext: &[u8],
    nonce: &[u8],
) -> Result<String, ApiError> {
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(ciphertext);
    let encoded = format!("{}{}", CIPHER_VERSION_PREFIX, BASE64.encode(blob));
    cipher
        .decrypt(&encoded)
        .map_err(|_| ApiError::Internal("Failed to decrypt the inference key".to_string()))
}

/// The BYOK key-validation call, abstracted so tests can stub it (the crate's
/// existing pattern for outbound provider calls, e.g. the Stripe checkout
/// trait).
#[async_trait::async_trait]
pub trait KeyValidator: Send + Sync {
    /// Cheap authenticated GET to `{base_url}/models`. Ok = key accepted.
    async fn validate(&self, base_url: &str, api_key: &str) -> Result<(), ApiError>;
}

/// Production validator: 5s timeout, 401/403 → 400 "key rejected by provider",
/// anything else unreachable/unexpected → 502-style "validation unavailable".
/// The key is never included in any message.
pub struct ReqwestKeyValidator;

#[async_trait::async_trait]
impl KeyValidator for ReqwestKeyValidator {
    async fn validate(&self, base_url: &str, api_key: &str) -> Result<(), ApiError> {
        let response = reqwest::Client::new()
            .get(format!("{}/models", base_url.trim_end_matches('/')))
            .bearer_auth(api_key)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| {
                ApiError::ServiceUnavailable(
                    "Key validation is required but the provider could not be reached — please retry."
                        .to_string(),
                )
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ApiError::BadRequest(
                "Key rejected by provider — check the key and try again.".to_string(),
            ));
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
}

#[derive(Debug, FromRow)]
struct KeyRow {
    provider_id: String,
    key_ciphertext: Vec<u8>,
    key_nonce: Vec<u8>,
    status: String,
    last_validated_at: Option<DateTime<Utc>>,
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
            validator: Arc::new(ReqwestKeyValidator),
        }
    }

    #[cfg(test)]
    fn with_validator(db: PgPool, cipher: Arc<CredentialCipher>, validator: Arc<dyn KeyValidator>) -> Self {
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
        let base_url = byok_base_url(provider_id).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Unknown BYOK provider: {:?}. Supported: {}.",
                provider_id,
                BYOK_PROVIDERS
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        if plaintext_key.trim().is_empty() {
            return Err(ApiError::BadRequest("The API key must not be empty.".to_string()));
        }

        self.validator.validate(base_url, plaintext_key).await?;

        let (ciphertext, nonce) = encrypt_parts(&self.cipher, plaintext_key)?;
        sqlx::query(
            r#"
            INSERT INTO user_inference_keys (
                user_id, provider_id, key_ciphertext, key_nonce, status, last_validated_at, updated_at
            ) VALUES ($1, $2, $3, $4, 'active', NOW(), NOW())
            ON CONFLICT (user_id, provider_id) DO UPDATE SET
                key_ciphertext = excluded.key_ciphertext,
                key_nonce = excluded.key_nonce,
                status = 'active',
                last_validated_at = NOW(),
                updated_at = NOW()
            "#,
        )
        .bind(user_id)
        .bind(provider_id)
        .bind(ciphertext)
        .bind(nonce)
        .execute(&self.db)
        .await
        .map_err(ApiError::DatabaseError)?;

        Ok(InferenceKeyInfo {
            provider_id: provider_id.to_string(),
            masked: mask_key(plaintext_key),
            status: "active".to_string(),
            last_validated_at: Some(Utc::now()),
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
                byok_base_url(&provider_id),
                self.get_decrypted(user_id, &provider_id).await,
            ) {
                (Some(base), Ok(Some(key))) => match self.validator.validate(base, &key).await {
                    Ok(()) => Ok(true),
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
            SELECT provider_id, key_ciphertext, key_nonce, status, last_validated_at
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
                })
            })
            .collect()
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
        async fn validate(&self, _base_url: &str, _api_key: &str) -> Result<(), ApiError> {
            match &self.0 {
                Ok(()) => Ok(()),
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
            .upsert_and_validate("user_1", "anthropic", "sk-ant-xxxxxxxxxx")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Unknown BYOK provider"), "{error}");
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
    fn byok_registry_excludes_anthropic_and_openrouter() {
        assert!(byok_base_url("anthropic").is_none());
        assert!(byok_base_url("openrouter").is_none());
        for (provider, _) in BYOK_PROVIDERS {
            assert!(byok_base_url(provider).is_some());
        }
    }
}
