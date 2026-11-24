//! Transparent at-rest encryption for connector tokens.
//!
//! Tokens are sealed with AES-256-GCM when an encryption key is configured —
//! `ALLTERNIT_ENCRYPTION_KEY` (override) or the platform's canonical
//! `ENCRYPTION_KEY` (config.rs::encryption_key / company.json) — and stored as
//! drawn from a UUID v4 (already a dependency), the key is SHA-256 of the env
//! value (any passphrase → 32 bytes). With no key set (local dev), tokens are
//! stored with a `plain:` prefix so the boundary is explicit and legacy
//! un-prefixed rows keep decrypting. `open` is fail-closed: a ciphertext that
//! does not decrypt returns empty (unusable) rather than leaking bytes.
//!
//! Reuses crates already in Cargo.toml: `aes-gcm`, `sha2`, `base64`, `uuid`.
//!
//! [`Keyring`] adds key ids and rotation without changing [`seal`]/[`open`]:
//! it writes `enc:v2:<key id>:<nonce>:<ct>` and reads that plus the old
//! `enc:v1:` form, which carries no key id and is read as key id
//! [`LEGACY_KEY_ID`] (every key in the ring is tried). [`open`] also reads a
//! v2 value when the process key is the one that sealed it.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use sha2::{Digest, Sha256};

const ENC_PREFIX: &str = "enc:v1:";
/// `enc:v2:<key id>:<nonce b64>:<ciphertext b64>`, written by [`Keyring`].
const ENC_V2_PREFIX: &str = "enc:v2:";
const PLAIN_PREFIX: &str = "plain:";

const PLATFORM_KEY_FILE: &str = "connector-encryption.key";

fn derive(passphrase: &str) -> [u8; 32] {
    let digest = Sha256::digest(passphrase.as_bytes());
    let mut k = [0u8; 32];
    k.copy_from_slice(&digest);
    k
}

fn key_bytes() -> Option<[u8; 32]> {
    static K: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    // Return a cached key when we have one. Critically, do NOT cache a negative
    // result: the first-run wizard may generate the runtime key mid-process, and
    // a cached None would pin this process to plaintext until restart. Re-probe
    // the sources whenever no key is cached yet.
    if let Some(k) = K.get() {
        return Some(*k);
    }
    // The signed desktop process provides `ALLTERNIT_ENCRYPTION_KEY`. Headless
    // runtimes fall back to a mode-0600 runtime file; this process never talks
    // to OS Keychain.
    let raw = std::env::var("ALLTERNIT_ENCRYPTION_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("ENCRYPTION_KEY")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .or_else(read_runtime_key)?;
    let k = derive(&raw);
    let _ = K.set(k); // ignore Err if another thread raced us
    Some(*K.get().unwrap())
}

fn runtime_key_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("ALLTERNIT_PLATFORM_KEY_FILE") {
        if !path.is_empty() {
            return path.into();
        }
    }
    let directory = std::env::var("ALLTERNIT_DATA_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::data_dir().map(|path| path.join("allternit")))
        .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/allternit"));
    directory.join(PLATFORM_KEY_FILE)
}

fn read_runtime_key() -> Option<String> {
    std::fs::read_to_string(runtime_key_path())
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Ensure an encryption key exists. Desktop supplies it from Electron
/// `safeStorage`; headless/VPS mode creates a private local runtime key.
pub fn ensure_platform_key() -> bool {
    if encryption_enabled() {
        return true;
    }
    // Generate 256 bits of randomness as a 64-char hex passphrase (two UUID v4s).
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    let passphrase = hex::encode(bytes);
    let path = runtime_key_path();
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            use std::io::Write;
            if file.write_all(passphrase.as_bytes()).is_err() {
                let _ = std::fs::remove_file(&path);
                return false;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return false,
    }
    key_bytes().is_some()
}

pub fn encryption_enabled() -> bool {
    key_bytes().is_some()
}

pub fn seal(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    let key = match key_bytes() {
        Some(k) => k,
        None => return format!("{}{}", PLAIN_PREFIX, plain),
    };
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let nonce_bytes = uuid::Uuid::new_v4().as_bytes()[..12].to_vec();
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, plain.as_bytes()) {
        Ok(ct) => format!(
            "{}{}:{}",
            ENC_PREFIX,
            base64::engine::general_purpose::STANDARD.encode(&nonce_bytes),
            base64::engine::general_purpose::STANDARD.encode(&ct)
        ),
        Err(_) => format!("{}{}", PLAIN_PREFIX, plain), // should not happen; never store broken ciphertext
    }
}

pub fn open(stored: &str) -> String {
    if stored.is_empty() {
        return String::new();
    }
    if let Some(rest) = stored.strip_prefix(PLAIN_PREFIX) {
        return rest.to_string();
    }
    if let Some(rest) = stored.strip_prefix(ENC_V2_PREFIX) {
        // Key-id format from a [`Keyring`]. The process key opens it only when
        // it is the key that sealed it.
        let Some(key) = key_bytes() else {
            return String::new();
        };
        let Some((kid, body)) = rest.split_once(':') else {
            return String::new();
        };
        if kid != key_id_for_bytes(&key) {
            return String::new();
        }
        return decrypt_body(&key, body).unwrap_or_default();
    }
    if let Some(rest) = stored.strip_prefix(ENC_PREFIX) {
        let key = match key_bytes() {
            Some(k) => k,
            None => return String::new(), // ciphertext but no key configured → unusable, fail closed
        };
        let mut parts = rest.splitn(2, ':');
        let (Some(n_b64), Some(ct_b64)) = (parts.next(), parts.next()) else {
            return String::new();
        };
        let (Ok(nonce_bytes), Ok(ct)) = (
            base64::engine::general_purpose::STANDARD.decode(n_b64),
            base64::engine::general_purpose::STANDARD.decode(ct_b64),
        ) else {
            return String::new();
        };
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        return match cipher.decrypt(Nonce::from_slice(&nonce_bytes), ct.as_ref()) {
            Ok(pt) => String::from_utf8(pt).unwrap_or_default(),
            Err(_) => String::new(),
        };
    }
    // Legacy un-prefixed plaintext row.
    stored.to_string()
}

// ─── Key ids and rotation ───────────────────────────────────────────────────

/// The key id reported for `enc:v1:` values, which carry none.
pub const LEGACY_KEY_ID: &str = "legacy";

/// Stable id of a derived key: the first 16 hex characters of
/// `SHA-256("allternit-key-id:" || key)`. It names the key without revealing
/// it, and the same passphrase always gets the same id.
fn key_id_for_bytes(key: &[u8; 32]) -> String {
    let mut h = <Sha256 as Digest>::new();
    h.update(b"allternit-key-id:");
    h.update(key);
    hex::encode(&h.finalize()[..8])
}

/// The key id a passphrase gets in a [`Keyring`].
pub fn key_id_for(passphrase: &str) -> String {
    key_id_for_bytes(&derive(passphrase))
}

fn decrypt_body(key: &[u8; 32], body: &str) -> Option<String> {
    let (n_b64, ct_b64) = body.split_once(':')?;
    let nonce_bytes = base64::engine::general_purpose::STANDARD.decode(n_b64).ok()?;
    let ct = base64::engine::general_purpose::STANDARD.decode(ct_b64).ok()?;
    if nonce_bytes.len() != 12 {
        return None;
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let pt = cipher.decrypt(Nonce::from_slice(&nonce_bytes), ct.as_ref()).ok()?;
    String::from_utf8(pt).ok()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyringError {
    #[error("the encryption key is empty")]
    EmptyKey,
    #[error("value is not sealed (no enc:v1: or enc:v2: prefix)")]
    NotSealed,
    #[error("value was sealed with key {0}, which is not in the keyring")]
    UnknownKey(String),
    #[error("value does not decrypt with key {0}")]
    Decrypt(String),
    #[error("encryption failed")]
    Encrypt,
}

/// A current key plus earlier keys that may still open old values.
///
/// [`Keyring::seal`] always uses the current key. [`Keyring::open`] picks the
/// key by the value's key id, so values sealed before a rotation stay
/// readable until `admin rotate-key` re-seals them.
#[derive(Clone)]
pub struct Keyring {
    current: (String, [u8; 32]),
    previous: Vec<(String, [u8; 32])>,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ids only, never key bytes.
        f.debug_struct("Keyring")
            .field("current", &self.current.0)
            .field("previous", &self.previous.iter().map(|(k, _)| k).collect::<Vec<_>>())
            .finish()
    }
}

impl Keyring {
    /// Keys are passphrases, derived like [`seal`]'s key (SHA-256). Empty
    /// previous entries and repeats of the current key are dropped.
    pub fn new<S: AsRef<str>>(current: &str, previous: &[S]) -> Result<Self, KeyringError> {
        if current.is_empty() {
            return Err(KeyringError::EmptyKey);
        }
        let cur = derive(current);
        let cur_id = key_id_for_bytes(&cur);
        let mut prev: Vec<(String, [u8; 32])> = Vec::new();
        for p in previous {
            let p = p.as_ref();
            if p.is_empty() {
                continue;
            }
            let k = derive(p);
            let id = key_id_for_bytes(&k);
            if id != cur_id && !prev.iter().any(|(i, _)| *i == id) {
                prev.push((id, k));
            }
        }
        Ok(Self {
            current: (cur_id, cur),
            previous: prev,
        })
    }

    pub fn current_key_id(&self) -> &str {
        &self.current.0
    }

    /// Current first, then previous keys in the order given.
    pub fn key_ids(&self) -> Vec<String> {
        std::iter::once(self.current.0.clone())
            .chain(self.previous.iter().map(|(k, _)| k.clone()))
            .collect()
    }

    fn key(&self, id: &str) -> Option<&[u8; 32]> {
        if self.current.0 == id {
            return Some(&self.current.1);
        }
        self.previous.iter().find(|(k, _)| k == id).map(|(_, b)| b)
    }

    /// The key id a sealed value names: its v2 id, [`LEGACY_KEY_ID`] for v1,
    /// None when it is not sealed.
    pub fn key_id_of(sealed: &str) -> Option<String> {
        if let Some(rest) = sealed.strip_prefix(ENC_V2_PREFIX) {
            return rest.split_once(':').map(|(k, _)| k.to_string());
        }
        if sealed.starts_with(ENC_PREFIX) {
            return Some(LEGACY_KEY_ID.to_string());
        }
        None
    }

    /// True unless the value is v2 under the current key.
    pub fn needs_rotation(&self, sealed: &str) -> bool {
        Self::key_id_of(sealed).as_deref() != Some(self.current_key_id())
    }

    /// Seal with the current key: `enc:v2:<key id>:<nonce>:<ct>`.
    pub fn seal(&self, plain: &str) -> Result<String, KeyringError> {
        use rand::RngCore;
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.current.1));
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_bytes())
            .map_err(|_| KeyringError::Encrypt)?;
        let b64 = &base64::engine::general_purpose::STANDARD;
        Ok(format!(
            "{ENC_V2_PREFIX}{}:{}:{}",
            self.current.0,
            b64.encode(nonce_bytes),
            b64.encode(ct)
        ))
    }

    /// Open a v2 value by its key id, or a v1 value (key id
    /// [`LEGACY_KEY_ID`]) with whichever key in the ring opens it. Fails
    /// closed: never returns the stored bytes as if they were plaintext.
    pub fn open(&self, sealed: &str) -> Result<String, KeyringError> {
        if let Some(rest) = sealed.strip_prefix(ENC_V2_PREFIX) {
            let (kid, body) = rest.split_once(':').ok_or(KeyringError::NotSealed)?;
            let key = self
                .key(kid)
                .ok_or_else(|| KeyringError::UnknownKey(kid.to_string()))?;
            return decrypt_body(key, body).ok_or_else(|| KeyringError::Decrypt(kid.to_string()));
        }
        if let Some(body) = sealed.strip_prefix(ENC_PREFIX) {
            let keys = std::iter::once(&self.current.1).chain(self.previous.iter().map(|(_, k)| k));
            for key in keys {
                if let Some(pt) = decrypt_body(key, body) {
                    return Ok(pt);
                }
            }
            return Err(KeyringError::Decrypt(LEGACY_KEY_ID.to_string()));
        }
        Err(KeyringError::NotSealed)
    }

    /// Open with any key, then seal with the current one.
    pub fn reseal(&self, sealed: &str) -> Result<String, KeyringError> {
        self.seal(&self.open(sealed)?)
    }
}

/// `enc:v1:` with an explicit passphrase, the format [`seal`] writes. For
/// tests and migration tools that must produce legacy ciphertext.
pub fn seal_v1_with(passphrase: &str, plain: &str) -> Result<String, KeyringError> {
    use rand::RngCore;
    let key = derive(passphrase);
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plain.as_bytes())
        .map_err(|_| KeyringError::Encrypt)?;
    let b64 = &base64::engine::general_purpose::STANDARD;
    Ok(format!("{ENC_PREFIX}{}:{}", b64.encode(nonce_bytes), b64.encode(ct)))
}

#[cfg(test)]
mod keyring_tests {
    use super::*;

    #[test]
    fn seals_with_current_key_id() {
        let ring = Keyring::new("new-key", &["old-key"]).unwrap();
        let sealed = ring.seal("hush").unwrap();
        assert!(sealed.starts_with(&format!("enc:v2:{}:", ring.current_key_id())), "{sealed}");
        assert_eq!(ring.open(&sealed).unwrap(), "hush");
        assert!(!ring.needs_rotation(&sealed));
        assert_eq!(ring.current_key_id(), key_id_for("new-key"));
    }

    #[test]
    fn opens_previous_key_ciphertext() {
        let old = Keyring::new("old-key", &[] as &[&str]).unwrap();
        let sealed = old.seal("from before").unwrap();
        let rotated = Keyring::new("new-key", &["old-key"]).unwrap();
        assert!(rotated.needs_rotation(&sealed));
        assert_eq!(rotated.open(&sealed).unwrap(), "from before");
        let resealed = rotated.reseal(&sealed).unwrap();
        assert!(!rotated.needs_rotation(&resealed));
        // Once the old key is dropped, the old value no longer opens.
        let only_new = Keyring::new("new-key", &[] as &[&str]).unwrap();
        assert_eq!(
            only_new.open(&sealed),
            Err(KeyringError::UnknownKey(key_id_for("old-key")))
        );
        assert_eq!(only_new.open(&resealed).unwrap(), "from before");
    }

    #[test]
    fn opens_legacy_v1_ciphertext() {
        let legacy = seal_v1_with("old-key", "v1 secret").unwrap();
        assert_eq!(Keyring::key_id_of(&legacy).as_deref(), Some(LEGACY_KEY_ID));
        let ring = Keyring::new("new-key", &["old-key"]).unwrap();
        assert!(ring.needs_rotation(&legacy));
        assert_eq!(ring.open(&legacy).unwrap(), "v1 secret");
        let wrong = Keyring::new("other", &[] as &[&str]).unwrap();
        assert_eq!(wrong.open(&legacy), Err(KeyringError::Decrypt(LEGACY_KEY_ID.into())));
    }

    #[test]
    fn refuses_unsealed_values() {
        let ring = Keyring::new("k", &[] as &[&str]).unwrap();
        assert_eq!(ring.open("plain:abc"), Err(KeyringError::NotSealed));
        assert_eq!(ring.open("abc"), Err(KeyringError::NotSealed));
        assert!(Keyring::new("", &[] as &[&str]).is_err());
    }

    #[test]
    fn debug_never_prints_key_bytes() {
        let ring = Keyring::new("secret-passphrase", &[] as &[&str]).unwrap();
        let s = format!("{ring:?}");
        assert!(s.contains(ring.current_key_id()));
        assert!(!s.contains("secret-passphrase"));
    }
}
