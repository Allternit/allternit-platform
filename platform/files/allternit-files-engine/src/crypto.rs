//! Files key hierarchy (SPEC section 13) and sealed chunks for client caches and replicas.
//!
//! FRK (32 bytes) -> HKDF-SHA256 subkeys:
//! - `files-cache/1/<device_id>`: one device's offline cache ([`EncryptedCache`])
//! - `files-replica/1/<replica_id>`: replica chunks (step 15)
//! - `files-snapshot/1`: reserved
//!
//! A sealed chunk is XChaCha20-Poly1305 with a random 24-byte nonce and an
//! [`EncryptedChunkHeader`]. The header is bound to the ciphertext as AAD, and opening
//! re-checks the plaintext blake3, so a swapped, edited or truncated blob fails.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use allternit_files_core::EncryptedChunkHeader;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

use crate::chunker::{hash_bytes, is_hash};
use crate::error::{FilesError, Result};

pub const CIPHER: &str = "xchacha20poly1305";
pub const SNAPSHOT_INFO: &str = "files-snapshot/1";

pub fn cache_info(device_id: &str) -> String {
    format!("files-cache/1/{device_id}")
}

pub fn replica_info(replica_id: &str) -> String {
    format!("files-replica/1/{replica_id}")
}

/// The user's Files root key. Never leaves a device unwrapped.
#[derive(Clone)]
pub struct FilesRootKey {
    key_id: String,
    key: [u8; 32],
}

impl std::fmt::Debug for FilesRootKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesRootKey").field("key_id", &self.key_id).finish_non_exhaustive()
    }
}

impl FilesRootKey {
    pub fn generate(key_id: impl Into<String>) -> Self {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        Self { key_id: key_id.into(), key }
    }

    pub fn from_bytes(key_id: impl Into<String>, key: [u8; 32]) -> Self {
        Self { key_id: key_id.into(), key }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// HKDF-SHA256 (no salt) with `info` as the label.
    pub fn subkey(&self, info: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        Hkdf::<Sha256>::new(None, &self.key)
            .expand(info.as_bytes(), &mut out)
            .expect("32 bytes is a valid HKDF-SHA256 length");
        out
    }
}

/// Seals and opens chunks under one subkey.
#[derive(Clone)]
pub struct ChunkSealer {
    key_id: String,
    key: [u8; 32],
}

impl ChunkSealer {
    pub fn for_cache(frk: &FilesRootKey, device_id: &str) -> Self {
        Self { key_id: frk.key_id.clone(), key: frk.subkey(&cache_info(device_id)) }
    }

    pub fn for_replica(frk: &FilesRootKey, replica_id: &str) -> Self {
        Self { key_id: frk.key_id.clone(), key: frk.subkey(&replica_info(replica_id)) }
    }

    /// keyed blake3(subkey, plaintext hash bytes): the name a sealed chunk is stored
    /// under, so the host never sees plaintext hashes.
    pub fn locator(&self, plaintext_hash: &str) -> Result<String> {
        let raw = hex::decode(plaintext_hash)
            .ok()
            .filter(|b| b.len() == 32)
            .ok_or_else(|| FilesError::InvalidRequest(format!("{plaintext_hash:?} is not a blake3 hex hash")))?;
        Ok(blake3::keyed_hash(&self.key, &raw).to_hex().to_string())
    }

    fn aad(h: &EncryptedChunkHeader) -> Vec<u8> {
        format!("afc1|{}|{}|{}|{}|{}", h.v, h.cipher, h.key_id, h.locator, h.plaintext_size).into_bytes()
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<(EncryptedChunkHeader, Vec<u8>)> {
        let plaintext_hash = hash_bytes(plaintext);
        let mut nonce = [0u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let header = EncryptedChunkHeader {
            v: 1,
            cipher: CIPHER.to_string(),
            key_id: self.key_id.clone(),
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            locator: self.locator(&plaintext_hash)?,
            plaintext_size: plaintext.len() as u64,
        };
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &Self::aad(&header) })
            .map_err(|_| FilesError::Internal("seal failed".into()))?;
        Ok((header, ct))
    }

    /// Opens a sealed chunk and checks it is the plaintext named by `plaintext_hash`.
    pub fn open(&self, header: &EncryptedChunkHeader, ciphertext: &[u8], plaintext_hash: &str) -> Result<Vec<u8>> {
        if header.v != 1 || header.cipher != CIPHER {
            return Err(FilesError::IntegrityFailed(format!("unknown sealed chunk format v{} {}", header.v, header.cipher)));
        }
        if header.key_id != self.key_id {
            return Err(FilesError::KeyUnavailable(format!("chunk sealed with key {}", header.key_id)));
        }
        if header.locator != self.locator(plaintext_hash)? {
            return Err(FilesError::IntegrityFailed(format!("sealed chunk is not {plaintext_hash}")));
        }
        let nonce = URL_SAFE_NO_PAD
            .decode(&header.nonce)
            .ok()
            .filter(|n| n.len() == 24)
            .ok_or_else(|| FilesError::IntegrityFailed("bad nonce".into()))?;
        let cipher = XChaCha20Poly1305::new((&self.key).into());
        let pt = cipher
            .decrypt(XNonce::from_slice(&nonce), Payload { msg: ciphertext, aad: &Self::aad(header) })
            .map_err(|_| FilesError::IntegrityFailed(format!("sealed chunk {plaintext_hash} failed authentication")))?;
        if pt.len() as u64 != header.plaintext_size || hash_bytes(&pt) != plaintext_hash {
            return Err(FilesError::IntegrityFailed(format!("sealed chunk {plaintext_hash} has the wrong content")));
        }
        Ok(pt)
    }
}

/// A device's offline chunk cache, sealed with `files-cache/1/<device_id>`.
/// Blob layout: `u32 BE header length | header JSON | ciphertext`, file name = locator.
pub struct EncryptedCache {
    dir: PathBuf,
    sealer: ChunkSealer,
}

impl EncryptedCache {
    pub fn open(dir: impl Into<PathBuf>, frk: &FilesRootKey, device_id: &str) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, sealer: ChunkSealer::for_cache(frk, device_id) })
    }

    fn path(&self, plaintext_hash: &str) -> Result<PathBuf> {
        if !is_hash(plaintext_hash) {
            return Err(FilesError::InvalidRequest(format!("{plaintext_hash:?} is not a blake3 hex hash")));
        }
        Ok(self.dir.join(self.sealer.locator(plaintext_hash)?))
    }

    pub fn put(&self, plaintext: &[u8]) -> Result<String> {
        let hash = hash_bytes(plaintext);
        let (header, ct) = self.sealer.seal(plaintext)?;
        let header_json = serde_json::to_vec(&header).map_err(|e| FilesError::Internal(e.to_string()))?;
        let path = self.path(&hash)?;
        let tmp = path.with_extension(format!("tmp{}", rand::random::<u32>()));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&(header_json.len() as u32).to_be_bytes())?;
            f.write_all(&header_json)?;
            f.write_all(&ct)?;
        }
        fs::rename(tmp, path)?;
        Ok(hash)
    }

    /// `Ok(None)` when not cached; `integrity_failed` when the blob was changed.
    pub fn get(&self, plaintext_hash: &str) -> Result<Option<Vec<u8>>> {
        let blob = match fs::read(self.path(plaintext_hash)?) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let bad = || FilesError::IntegrityFailed("cache blob is damaged".into());
        if blob.len() < 4 {
            return Err(bad());
        }
        let hlen = u32::from_be_bytes(blob[..4].try_into().expect("4 bytes")) as usize;
        let header_bytes = blob.get(4..4 + hlen).ok_or_else(bad)?;
        let header: EncryptedChunkHeader = serde_json::from_slice(header_bytes).map_err(|_| bad())?;
        self.sealer.open(&header, &blob[4 + hlen..], plaintext_hash).map(Some)
    }

    pub fn remove(&self, plaintext_hash: &str) -> Result<()> {
        match fs::remove_file(self.path(plaintext_hash)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}
