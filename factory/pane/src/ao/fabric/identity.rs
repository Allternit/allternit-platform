//! Node identity: one Ed25519 keypair per `--session ao` state dir, owned by
//! ao alone (spike D1 — never reads gizzi-code/agent-daemon/Desktop identity
//! files). Keypair is reused across re-pairs; only the device token rotates.
//! Storage: `~/.agent-orchestrator/fabric/identity.json`, mode 0600, atomic
//! temp-write + rename (pattern from gizzi `pairing.ts:119-129`).

use std::path::PathBuf;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::wire::RuntimeSessionResponse;

const IDENTITY_DIR: &str = ".agent-orchestrator/fabric";
const IDENTITY_FILE: &str = "identity.json";

/// Proactive rotation window: rotate when the token expires within 7 days
/// (agent-daemon `ROTATION_SKEW_MS`, index.ts:13).
pub(crate) const ROTATION_SKEW: std::time::Duration =
    std::time::Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NodeIdentity {
    pub runtime_id: Option<String>,
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    pub device_token: Option<String>,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Raw 32-byte Ed25519 public key, base64url no-pad (what the server
    /// stores; it enforces length at runtime_pairing.rs:1195-1205).
    pub public_key: String,
    /// Raw 32-byte secret seed, base64url no-pad (agent-daemon stores PEM;
    /// raw bytes are the Rust-native equivalent and stay 0600 on disk).
    pub private_key: String,
}

impl NodeIdentity {
    pub(crate) fn identity_path() -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        home.join(IDENTITY_DIR).join(IDENTITY_FILE)
    }

    /// Generate a fresh keypair (no pairing yet). Used on first `pair`.
    pub(crate) fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        let signing = SigningKey::from_bytes(&seed);
        Self::from_signing_key(signing)
    }

    fn from_signing_key(signing: SigningKey) -> Self {
        let verifying: VerifyingKey = (&signing).into();
        NodeIdentity {
            runtime_id: None,
            user_id: None,
            user_email: None,
            organization_id: None,
            device_token: None,
            expires_at: None,
            capabilities: Vec::new(),
            public_key: URL_SAFE_NO_PAD.encode(verifying.to_bytes()),
            private_key: URL_SAFE_NO_PAD.encode(signing.to_bytes()),
        }
    }

    pub(crate) fn signing_key(&self) -> Result<SigningKey, String> {
        let bytes = URL_SAFE_NO_PAD
            .decode(&self.private_key)
            .map_err(|err| format!("identity private key is not valid base64url: {err}"))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "identity private key is not 32 bytes".to_string())?;
        Ok(SigningKey::from_bytes(&seed))
    }

    /// Proof of possession: Ed25519 signature over
    /// `allternit-runtime-pairing:{pairingId}:{challenge}` (runtime_pairing.rs:1207-1209),
    /// base64url no-pad.
    pub(crate) fn pairing_signature(&self, pairing_id: &str, challenge: &str) -> Result<String, String> {
        let message = format!("allternit-runtime-pairing:{pairing_id}:{challenge}");
        let signing = self.signing_key()?;
        Ok(URL_SAFE_NO_PAD.encode(signing.sign(message.as_bytes()).to_bytes()))
    }

    pub(crate) fn is_paired(&self) -> bool {
        self.runtime_id.is_some() && self.device_token.is_some()
    }

    /// Record a successful exchange/rotate: replaces token + expiry, keeps
    /// the keypair (spike: keypair reused across re-pairs, token rotates).
    pub(crate) fn apply_session(&mut self, session: &RuntimeSessionResponse) {
        self.runtime_id = Some(session.runtime_id.clone());
        self.user_id = Some(session.user_id.clone());
        self.user_email = Some(session.user_email.clone());
        self.organization_id = session.organization_id.clone();
        self.device_token = Some(session.device_token.clone());
        self.expires_at = Some(session.expires_at.clone());
        self.capabilities = session.capabilities.clone();
    }

    pub(crate) fn require_paired(&self) -> Result<(), String> {
        if self.is_paired() {
            Ok(())
        } else {
            Err("node is not paired — run `ao fabric pair` first".to_string())
        }
    }

    pub(crate) fn runtime_id(&self) -> Result<&str, String> {
        self.runtime_id.as_deref().ok_or_else(|| "not paired".to_string())
    }

    pub(crate) fn user_id(&self) -> Result<&str, String> {
        self.user_id.as_deref().ok_or_else(|| "not paired".to_string())
    }

    pub(crate) fn user_email(&self) -> Result<&str, String> {
        self.user_email.as_deref().ok_or_else(|| "not paired".to_string())
    }

    pub(crate) fn device_token(&self) -> Result<&str, String> {
        self.device_token.as_deref().ok_or_else(|| "not paired".to_string())
    }

    /// Seconds until token expiry; `None` if unpaired or the timestamp does
    /// not parse (server emits RFC 3339).
    pub(crate) fn seconds_to_expiry(&self) -> Option<i64> {
        let expires_at = self.expires_at.as_deref()?;
        let expiry = time::OffsetDateTime::parse(
            expires_at,
            &time::format_description::well_known::Rfc3339,
        )
        .ok()?;
        let now = time::OffsetDateTime::now_utc();
        Some((expiry - now).whole_seconds())
    }

    /// True when paired and the token expires inside the rotation skew
    /// (agent-daemon `rotateIfNeeded`, index.ts:227-237).
    pub(crate) fn needs_rotation(&self) -> bool {
        self.seconds_to_expiry()
            .map(|seconds| seconds < ROTATION_SKEW.as_secs() as i64)
            .unwrap_or(false)
    }

    /// Human-readable key fingerprint: sha256 of the raw public key,
    /// lowercase hex. Used by `ao fabric status`.
    pub(crate) fn fingerprint(&self) -> String {
        let Ok(bytes) = URL_SAFE_NO_PAD.decode(&self.public_key) else {
            return "<invalid public key>".to_string();
        };
        let digest = Sha256::digest(&bytes);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Load the identity file; a corrupt file is renamed aside (exactly like
    /// agent-daemon `loadIdentity`, index.ts:46-60) so it is replaced, not
    /// silently ignored.
    pub(crate) fn load() -> Option<Self> {
        let path = Self::identity_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return None;
        };
        match serde_json::from_str::<NodeIdentity>(&text) {
            Ok(identity) if identity.public_key.len() >= 32 && !identity.private_key.is_empty() => {
                Some(identity)
            }
            _ => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis())
                    .unwrap_or(0);
                let _ = std::fs::rename(&path, path.with_extension(format!("invalid-{stamp}.json")));
                None
            }
        }
    }

    /// Atomic 0600 write (temp file in the same dir + rename).
    pub(crate) fn save(&self) -> std::io::Result<()> {
        let path = Self::identity_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let temp = path.with_extension("tmp");
        {
            use std::io::Write as _;
            #[cfg(unix)]
            use std::os::unix::fs::OpenOptionsExt;
            let mut open = std::fs::OpenOptions::new();
            open.create(true).truncate(true).write(true);
            #[cfg(unix)]
            open.mode(0o600);
            let mut file = open.open(&temp)?;
            serde_json::to_writer_pretty(std::io::BufWriter::new(&mut file), self)
                .map_err(std::io::Error::other)?;
            file.flush()?;
        }
        std::fs::rename(&temp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identity_signs_and_verifies() {
        let identity = NodeIdentity::generate();
        let signature = identity.pairing_signature("pr_123", "challenge-abc").unwrap();
        let signing = identity.signing_key().unwrap();
        let verifying: VerifyingKey = (&signing).into();
        let sig_bytes = URL_SAFE_NO_PAD.decode(&signature).unwrap();
        let signature = ed25519_dalek::Signature::from_slice(&sig_bytes).unwrap();
        verifying
            .verify_strict(
                b"allternit-runtime-pairing:pr_123:challenge-abc",
                &signature,
            )
            .unwrap();
        // Wrong message must not verify.
        assert!(verifying
            .verify_strict(b"allternit-runtime-pairing:pr_123:tampered", &signature)
            .is_err());
    }

    #[test]
    fn public_key_is_32_bytes_base64url() {
        let identity = NodeIdentity::generate();
        let bytes = URL_SAFE_NO_PAD.decode(&identity.public_key).unwrap();
        assert_eq!(bytes.len(), 32);
        assert!(!identity.public_key.contains('='));
    }

    #[test]
    fn rotation_window_uses_seven_day_skew() {
        let mut identity = NodeIdentity::generate();
        let soon = (time::OffsetDateTime::now_utc() + time::Duration::days(3))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        identity.expires_at = Some(soon);
        assert!(identity.needs_rotation());

        let later = (time::OffsetDateTime::now_utc() + time::Duration::days(30))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        identity.expires_at = Some(later);
        assert!(!identity.needs_rotation());
    }

    #[test]
    fn apply_session_keeps_keypair_and_rotates_token() {
        let mut identity = NodeIdentity::generate();
        let public_before = identity.public_key.clone();
        let private_before = identity.private_key.clone();
        let session = RuntimeSessionResponse {
            runtime_id: "rt_x".into(),
            user_id: "u1".into(),
            user_email: "e@example.com".into(),
            organization_id: None,
            device_token: "allternit_runtime_a".into(),
            token_type: Some("Bearer".into()),
            expires_at: "2026-12-01T00:00:00Z".into(),
            capabilities: vec!["runtime:connect".into()],
        };
        identity.apply_session(&session);
        assert!(identity.is_paired());
        assert_eq!(identity.public_key, public_before);
        assert_eq!(identity.private_key, private_before);
        let session_b = RuntimeSessionResponse {
            device_token: "allternit_runtime_b".into(),
            ..session
        };
        identity.apply_session(&session_b);
        assert_eq!(identity.device_token.as_deref(), Some("allternit_runtime_b"));
    }
}
