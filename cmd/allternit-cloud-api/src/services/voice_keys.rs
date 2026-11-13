//! Voice keys saved to the account, and the QR handoff relay.
//!
//! **Account keys** (`user_voice_keys`, migration 128): when a user turns on
//! "Save to my Allternit account", their OpenAI or ElevenLabs voice key is
//! stored here so their other signed-in devices pick it up. Keys are sealed
//! with the platform credential cipher (`ALLTERNIT_CREDENTIALS_KEY`) through
//! the same nonce/ciphertext split as `services::inference_keys`. Without the
//! cipher (dev only: production refuses to boot without it, see `main.rs`) the
//! key is stored plaintext with an empty nonce, the same dev story as provider
//! tokens. Unlike inference keys these come back decrypted, to the owner's
//! signed-in session only (the routes refuse API tokens).
//!
//! **Handoff** (`voice_key_handoffs`): the relay for the device-to-device QR
//! handoff. The sender creates a session id, the receiver posts its one-time
//! P-256 public key once, and the sender reads it back. A row lives 120 s; an
//! expired row is not found. The API key itself never reaches the server.
//!
//! Key material never appears in errors or logs: validation messages are
//! static, and [`VoiceKey`]'s `Debug` redacts the key.

use std::sync::Arc;

use allternit_cloud_core::CredentialCipher;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Serialize;
use sqlx::{FromRow, PgPool};

use crate::error::ApiError;
use crate::services::inference_keys::{decrypt_parts, encrypt_parts};

/// Vendors whose voice keys can be saved.
pub const VOICE_PROVIDERS: &[&str] = &["openai", "elevenlabs"];
/// Longest accepted API key, in characters, after trimming.
pub const MAX_API_KEY_CHARS: usize = 512;
/// Longest accepted ElevenLabs voice id / label, in characters.
pub const MAX_VOICE_ID_CHARS: usize = 128;
pub const MAX_LABEL_CHARS: usize = 128;
/// Longest accepted receiver public key (base64 / base64url text).
pub const MAX_PUBKEY_CHARS: usize = 200;
/// Handoff lifetime.
pub const HANDOFF_TTL_SECS: i64 = 120;
/// Live handoffs one user may hold at once (a stuck client can't fill the table).
pub const MAX_LIVE_HANDOFFS: i64 = 20;

/// One saved voice key, decrypted, for its owner.
#[derive(Clone, Serialize)]
pub struct VoiceKey {
    pub provider: String,
    pub api_key: String,
    pub voice_id: Option<String>,
    pub label: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for VoiceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceKey")
            .field("provider", &self.provider)
            .field("api_key", &"<redacted>")
            .field("voice_id", &self.voice_id)
            .field("label", &self.label)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// A handoff as the owner sees it.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Handoff {
    pub sid: String,
    pub receiver_pubkey: Option<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct KeyRow {
    provider: String,
    key_ciphertext: Vec<u8>,
    key_nonce: Vec<u8>,
    voice_id: Option<String>,
    label: Option<String>,
    updated_at: DateTime<Utc>,
}

pub fn is_voice_provider(provider: &str) -> bool {
    VOICE_PROVIDERS.contains(&provider)
}

/// Trim an optional field; empty means unset. Over `max` chars is a 400.
fn optional_field(value: Option<&str>, max: usize, name: &str) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(ApiError::BadRequest(format!("{name} must be at most {max} characters.")));
    }
    Ok(Some(value.to_string()))
}

pub struct VoiceKeyService {
    db: PgPool,
    cipher: Option<Arc<CredentialCipher>>,
}

impl VoiceKeyService {
    pub fn new(db: PgPool, cipher: Option<Arc<CredentialCipher>>) -> Self {
        Self { db, cipher }
    }

    fn seal(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
        match &self.cipher {
            Some(cipher) => encrypt_parts(cipher, plaintext),
            None => {
                tracing::warn!("Storing a voice key PLAINTEXT (ALLTERNIT_CREDENTIALS_KEY unset)");
                Ok((plaintext.as_bytes().to_vec(), Vec::new()))
            }
        }
    }

    fn open(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<String, ApiError> {
        if nonce.is_empty() {
            // Stored by a dev deployment without the cipher.
            return String::from_utf8(ciphertext.to_vec())
                .map_err(|_| ApiError::Internal("Failed to read the stored key".to_string()));
        }
        let cipher = self.cipher.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("voice_keys_not_configured".to_string())
        })?;
        decrypt_parts(cipher, ciphertext, nonce)
    }

    /// The owner's saved keys, decrypted. A row that no longer decrypts (the
    /// cipher key was rotated) is skipped rather than failing the whole list.
    pub async fn list(&self, user_id: &str) -> Result<Vec<VoiceKey>, ApiError> {
        let rows = sqlx::query_as::<_, KeyRow>(
            "SELECT provider, key_ciphertext, key_nonce, voice_id, label, updated_at \
             FROM user_voice_keys WHERE user_id = $1 ORDER BY provider",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await?;
        let mut keys = Vec::with_capacity(rows.len());
        for row in rows {
            match self.open(&row.key_ciphertext, &row.key_nonce) {
                Ok(api_key) => keys.push(VoiceKey {
                    provider: row.provider,
                    api_key,
                    voice_id: row.voice_id,
                    label: row.label,
                    updated_at: row.updated_at,
                }),
                Err(_) => tracing::warn!(provider = %row.provider, "voice key no longer decrypts; skipped"),
            }
        }
        Ok(keys)
    }

    /// Save (or replace) the owner's key for one provider.
    pub async fn upsert(
        &self,
        user_id: &str,
        provider: &str,
        api_key: &str,
        voice_id: Option<&str>,
        label: Option<&str>,
    ) -> Result<VoiceKey, ApiError> {
        if !is_voice_provider(provider) {
            return Err(ApiError::BadRequest(
                "Unknown voice provider. Supported: openai, elevenlabs.".to_string(),
            ));
        }
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(ApiError::BadRequest("The API key must not be empty.".to_string()));
        }
        if api_key.chars().count() > MAX_API_KEY_CHARS {
            return Err(ApiError::BadRequest(format!(
                "The API key must be at most {MAX_API_KEY_CHARS} characters."
            )));
        }
        let voice_id = optional_field(voice_id, MAX_VOICE_ID_CHARS, "voice_id")?;
        let label = optional_field(label, MAX_LABEL_CHARS, "label")?;

        let (ciphertext, nonce) = self.seal(api_key)?;
        let updated_at: DateTime<Utc> = sqlx::query_scalar(
            r#"
            INSERT INTO user_voice_keys (user_id, provider, key_ciphertext, key_nonce, voice_id, label, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, NOW())
            ON CONFLICT (user_id, provider) DO UPDATE SET
                key_ciphertext = excluded.key_ciphertext,
                key_nonce = excluded.key_nonce,
                voice_id = excluded.voice_id,
                label = excluded.label,
                updated_at = NOW()
            RETURNING updated_at
            "#,
        )
        .bind(user_id)
        .bind(provider)
        .bind(ciphertext)
        .bind(nonce)
        .bind(&voice_id)
        .bind(&label)
        .fetch_one(&self.db)
        .await?;

        Ok(VoiceKey {
            provider: provider.to_string(),
            api_key: api_key.to_string(),
            voice_id,
            label,
            updated_at,
        })
    }

    /// Delete every saved key of the owner (sync turned off). Returns the count.
    pub async fn delete_all(&self, user_id: &str) -> Result<u64, ApiError> {
        let result = sqlx::query("DELETE FROM user_voice_keys WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(result.rows_affected())
    }

    /// Start a handoff: a fresh 256-bit url-safe session id, live 120 s.
    /// Expired rows (anyone's) are pruned first.
    pub async fn create_handoff(&self, user_id: &str) -> Result<Handoff, ApiError> {
        sqlx::query("DELETE FROM voice_key_handoffs WHERE expires_at <= NOW()")
            .execute(&self.db)
            .await?;
        let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM voice_key_handoffs WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&self.db)
            .await?;
        if live >= MAX_LIVE_HANDOFFS {
            return Err(ApiError::TooManyRequests(
                "Too many open key handoffs; wait two minutes and try again.".to_string(),
            ));
        }
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let sid = URL_SAFE_NO_PAD.encode(bytes);
        let handoff = sqlx::query_as::<_, Handoff>(
            "INSERT INTO voice_key_handoffs (sid, user_id, expires_at) \
             VALUES ($1, $2, NOW() + make_interval(secs => $3)) \
             RETURNING sid, receiver_pubkey, expires_at",
        )
        .bind(&sid)
        .bind(user_id)
        .bind(HANDOFF_TTL_SECS as f64)
        .fetch_one(&self.db)
        .await?;
        Ok(handoff)
    }

    /// Record the receiver's public key, once. 404 when the handoff is
    /// missing, expired or someone else's; 409 when a receiver is already set.
    pub async fn set_receiver(&self, user_id: &str, sid: &str, pubkey: &str) -> Result<(), ApiError> {
        let pubkey = pubkey.trim();
        let valid_chars = pubkey
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'));
        if pubkey.is_empty() || pubkey.len() > MAX_PUBKEY_CHARS || !valid_chars {
            return Err(ApiError::BadRequest(format!(
                "pubkey must be base64 or base64url text of at most {MAX_PUBKEY_CHARS} characters."
            )));
        }
        let updated = sqlx::query(
            "UPDATE voice_key_handoffs SET receiver_pubkey = $3 \
             WHERE sid = $1 AND user_id = $2 AND expires_at > NOW() AND receiver_pubkey IS NULL",
        )
        .bind(sid)
        .bind(user_id)
        .bind(pubkey)
        .execute(&self.db)
        .await?;
        if updated.rows_affected() == 1 {
            return Ok(());
        }
        match self.get_handoff(user_id, sid).await? {
            Some(_) => Err(ApiError::Conflict("This handoff already has a receiver.".to_string())),
            None => Err(ApiError::NotFound("Handoff not found or expired.".to_string())),
        }
    }

    /// The owner's live handoff, or `None` when missing/expired/not theirs.
    pub async fn get_handoff(&self, user_id: &str, sid: &str) -> Result<Option<Handoff>, ApiError> {
        Ok(sqlx::query_as::<_, Handoff>(
            "SELECT sid, receiver_pubkey, expires_at FROM voice_key_handoffs \
             WHERE sid = $1 AND user_id = $2 AND expires_at > NOW()",
        )
        .bind(sid)
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> PgPool {
        let db = crate::routes::test_support::test_pool().await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/128_voice_keys.sql").replace("public.", ""))
            .execute(&db)
            .await
            .expect("128 applies");
        db
    }

    fn sealed(db: PgPool) -> VoiceKeyService {
        VoiceKeyService::new(db, Some(Arc::new(CredentialCipher::new("test cipher key material"))))
    }

    #[tokio::test]
    async fn upsert_list_delete_roundtrip_seals_the_key() {
        let db = pool().await;
        let service = sealed(db.clone());
        let saved = service
            .upsert("user_a", "elevenlabs", "  sk_test_123  ", Some("21m00Tcm4TlvDq8ikWAM"), Some("Studio"))
            .await
            .unwrap();
        assert_eq!(saved.api_key, "sk_test_123", "trimmed");
        service.upsert("user_a", "openai", "sk-openai-1", None, Some("  ")).await.unwrap();
        service.upsert("user_a", "openai", "sk-openai-2", None, None).await.unwrap();

        let keys = service.list("user_a").await.unwrap();
        let summary: Vec<_> = keys
            .iter()
            .map(|k| (k.provider.as_str(), k.api_key.as_str(), k.voice_id.as_deref(), k.label.as_deref()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("elevenlabs", "sk_test_123", Some("21m00Tcm4TlvDq8ikWAM"), Some("Studio")),
                ("openai", "sk-openai-2", None, None),
            ]
        );
        assert!(!format!("{keys:?}").contains("sk_test_123"), "Debug redacts the key");

        let (ciphertext, nonce): (Vec<u8>, Vec<u8>) =
            sqlx::query_as("SELECT key_ciphertext, key_nonce FROM user_voice_keys WHERE provider = 'elevenlabs'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(nonce.len(), 12);
        assert!(!ciphertext.windows(11).any(|w| w == b"sk_test_123"), "no plaintext at rest");

        assert_eq!(service.delete_all("user_a").await.unwrap(), 2);
        assert!(service.list("user_a").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dev_without_cipher_stores_plaintext_and_reads_it_back() {
        let service = VoiceKeyService::new(pool().await, None);
        service.upsert("user_a", "openai", "sk-dev", None, None).await.unwrap();
        assert_eq!(service.list("user_a").await.unwrap()[0].api_key, "sk-dev");
    }

    #[tokio::test]
    async fn provider_and_length_validation() {
        let service = sealed(pool().await);
        for provider in ["anthropic", "OpenAI", ""] {
            let error = service.upsert("user_a", provider, "k", None, None).await.unwrap_err();
            assert!(matches!(error, ApiError::BadRequest(_)), "{provider}: {error}");
        }
        let error = service.upsert("user_a", "openai", "   ", None, None).await.unwrap_err();
        assert!(matches!(error, ApiError::BadRequest(_)), "{error}");

        let longest = "k".repeat(MAX_API_KEY_CHARS);
        service.upsert("user_a", "openai", &longest, None, None).await.unwrap();
        let too_long = "k".repeat(MAX_API_KEY_CHARS + 1);
        let error = service.upsert("user_a", "openai", &too_long, None, None).await.unwrap_err();
        assert!(error.to_string().contains("512"), "{error}");
        assert!(!error.to_string().contains("kkkk"), "the key is not echoed");

        let long_voice = "v".repeat(MAX_VOICE_ID_CHARS + 1);
        assert!(service.upsert("user_a", "elevenlabs", "k", Some(&long_voice), None).await.is_err());
        let long_label = "l".repeat(MAX_LABEL_CHARS + 1);
        assert!(service.upsert("user_a", "elevenlabs", "k", None, Some(&long_label)).await.is_err());
        assert_eq!(service.list("user_a").await.unwrap().len(), 1, "rejected saves store nothing");
    }

    #[tokio::test]
    async fn another_user_cannot_read_or_delete_the_key() {
        let service = sealed(pool().await);
        service.upsert("user_a", "elevenlabs", "sk_a", None, None).await.unwrap();
        assert!(service.list("user_b").await.unwrap().is_empty());
        assert_eq!(service.delete_all("user_b").await.unwrap(), 0);
        assert_eq!(service.list("user_a").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn handoff_receiver_is_set_once_and_expires() {
        let db = pool().await;
        let service = sealed(db.clone());
        let handoff = service.create_handoff("user_a").await.unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(&handoff.sid).unwrap().len(), 32, "256-bit sid");
        let ttl = (handoff.expires_at - Utc::now()).num_seconds();
        assert!((110..=120).contains(&ttl), "ttl {ttl}");
        assert_eq!(service.get_handoff("user_a", &handoff.sid).await.unwrap().unwrap().receiver_pubkey, None);

        // Another user can neither see nor claim it.
        assert!(service.get_handoff("user_b", &handoff.sid).await.unwrap().is_none());
        let error = service.set_receiver("user_b", &handoff.sid, "BPubB").await.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)), "{error}");

        let error = service.set_receiver("user_a", &handoff.sid, "not base64!").await.unwrap_err();
        assert!(matches!(error, ApiError::BadRequest(_)), "{error}");
        let error = service.set_receiver("user_a", &handoff.sid, &"A".repeat(MAX_PUBKEY_CHARS + 1)).await.unwrap_err();
        assert!(matches!(error, ApiError::BadRequest(_)), "{error}");

        service.set_receiver("user_a", &handoff.sid, "BPubB_-x").await.unwrap();
        let error = service.set_receiver("user_a", &handoff.sid, "BOther").await.unwrap_err();
        assert!(matches!(error, ApiError::Conflict(_)), "{error}");
        assert_eq!(
            service.get_handoff("user_a", &handoff.sid).await.unwrap().unwrap().receiver_pubkey.as_deref(),
            Some("BPubB_-x"),
            "the first receiver stays"
        );

        sqlx::query("UPDATE voice_key_handoffs SET expires_at = NOW() - interval '1 second'")
            .execute(&db)
            .await
            .unwrap();
        assert!(service.get_handoff("user_a", &handoff.sid).await.unwrap().is_none());
        let error = service.set_receiver("user_a", &handoff.sid, "BPubB").await.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)), "{error}");

        // The next create prunes the expired row.
        service.create_handoff("user_a").await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM voice_key_handoffs").fetch_one(&db).await.unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn a_receiver_on_an_unknown_sid_is_404() {
        let service = sealed(pool().await);
        let error = service.set_receiver("user_a", "nope", "BPub").await.unwrap_err();
        assert!(matches!(error, ApiError::NotFound(_)), "{error}");
    }
}
