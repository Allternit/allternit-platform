//! Ed25519 receipt signing keys and JWKS export.
//!
//! Private key material lives only in a 0600 file (hex-encoded 32-byte seed)
//! and is never logged, serialized into receipts, or included in Debug output.

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const SIGNING_KEY_ENV: &str = "ALLTERNIT_RECEIPT_SIGNING_KEY";
pub const DOMAIN_TAG: &str = "allternit.receipt.v1";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub kid: String,
    #[serde(rename = "use")]
    pub use_: String,
    pub alg: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

impl Jwks {
    pub fn find(&self, kid: &str) -> Option<VerifyingKey> {
        let k = self.keys.iter().find(|k| k.kid == kid)?;
        let raw = B64.decode(&k.x).ok()?;
        VerifyingKey::from_bytes(&raw.try_into().ok()?).ok()
    }
}

pub fn kid_for(vk: &VerifyingKey) -> String {
    format!("rk-{}", &hex::encode(Sha256::digest(vk.as_bytes()))[..16])
}

pub fn jwk_for(vk: &VerifyingKey) -> Jwk {
    Jwk {
        kty: "OKP".into(),
        crv: "Ed25519".into(),
        x: B64.encode(vk.as_bytes()),
        kid: kid_for(vk),
        use_: "sig".into(),
        alg: "EdDSA".into(),
    }
}

/// Signer holding the private key. Debug deliberately omits key bytes.
pub struct ReceiptSigner {
    key: SigningKey,
    kid: String,
}

impl std::fmt::Debug for ReceiptSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReceiptSigner {{ kid: {} }}", self.kid)
    }
}

impl ReceiptSigner {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let key = SigningKey::from_bytes(&seed);
        let kid = kid_for(&key.verifying_key());
        Self { key, kid }
    }

    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        Self::from_seed(seed)
    }

    /// Load from `path`; if absent and `create_if_missing`, generate with 0600 perms.
    pub fn load_or_create(path: &Path, create_if_missing: bool) -> Result<Self> {
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading signing key {}", path.display()))?;
            let raw = hex::decode(text.trim()).map_err(|_| anyhow!("signing key file is not hex"))?;
            let seed: [u8; 32] = raw.try_into().map_err(|_| anyhow!("signing key must be 32 bytes"))?;
            return Ok(Self::from_seed(seed));
        }
        if !create_if_missing {
            bail!("receipt signing key not found and generation is disabled");
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let s = Self::generate();
        write_secret(path, &hex::encode(s.key.to_bytes()))?;
        Ok(s)
    }

    /// Resolve key path from `ALLTERNIT_RECEIPT_SIGNING_KEY` or `default_path`.
    /// Generation is only allowed outside production (`ALLTERNIT_ENV=production`).
    pub fn from_env_or_default(default_path: &Path) -> Result<Self> {
        let path = std::env::var_os(SIGNING_KEY_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| default_path.to_path_buf());
        let prod = std::env::var("ALLTERNIT_ENV").map(|v| v == "production").unwrap_or(false);
        Self::load_or_create(&path, !prod)
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }
    pub fn verifying_key(&self) -> VerifyingKey {
        self.key.verifying_key()
    }
    pub fn jwk(&self) -> Jwk {
        jwk_for(&self.key.verifying_key())
    }
    pub fn sign(&self, msg: &[u8]) -> String {
        B64.encode(self.key.sign(msg).to_bytes())
    }
}

pub fn verify_sig(vk: &VerifyingKey, msg: &[u8], sig_b64: &str) -> bool {
    let Ok(raw) = B64.decode(sig_b64) else { return false };
    let Ok(arr) = <[u8; 64]>::try_from(raw) else { return false };
    vk.verify(msg, &Signature::from_bytes(&arr)).is_ok()
}

fn write_secret(path: &Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(content.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_generated_0600_and_reloads() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("k/receipt.key");
        let a = ReceiptSigner::load_or_create(&p, true).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let b = ReceiptSigner::load_or_create(&p, false).unwrap();
        assert_eq!(a.kid(), b.kid());
        assert!(!format!("{a:?}").contains(&std::fs::read_to_string(&p).unwrap()));
        assert!(ReceiptSigner::load_or_create(&d.path().join("none"), false).is_err());
    }

    #[test]
    fn sign_verify_and_jwks() {
        let s = ReceiptSigner::generate();
        let jwks = Jwks { keys: vec![s.jwk()] };
        let sig = s.sign(b"msg");
        assert!(verify_sig(&jwks.find(s.kid()).unwrap(), b"msg", &sig));
        assert!(!verify_sig(&jwks.find(s.kid()).unwrap(), b"msg2", &sig));
        assert!(!serde_json::to_string(&jwks).unwrap().contains("\"d\""));
    }
}
