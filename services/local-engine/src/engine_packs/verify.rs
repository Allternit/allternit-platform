//! Engine pack manifest, signatures and file hashes.
//!
//! Same rules as the Desktop runtime package client
//! (`surfaces/allternit-desktop/src/main/runtime-package.ts`): Ed25519
//! signatures over the exact bytes of `latest.json` and the manifest, and a
//! sha256 + size for every file. Ported here (not left to Desktop) because
//! local-engine is the process that spawns the binary, and it also runs
//! without Desktop (Gizzi CLI, Linux cloud computers): the spawner has to be
//! the one that checks.

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path};

/// The runtime signing key (`RUNTIME_PUBLIC_KEY` in runtime-package.ts),
/// SubjectPublicKeyInfo DER, base64. The private half stays with the release
/// operator (`~/.config/allternit/runtime-signing-ed25519.pem`).
pub const RUNTIME_PUBLIC_KEY_SPKI: &str = "MCowBQYDK2VwAyEAfFzE4gMEyDUK/Tb40TsLV5hXAYcD0Lro6gqWijht7ZU=";

/// One file of a pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
}

/// `pack.json`: the runtime manifest (`version`, `build`, `platform`,
/// `shellApi`, `files`) plus the engine fields (spec/on-device/engine-pack-spike.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackManifest {
    #[serde(default)]
    pub format: u32,
    #[serde(default)]
    pub kind: String,
    pub pack_id: String,
    /// `llamacpp` or `mlx`.
    pub engine: String,
    pub backend: String,
    pub version: String,
    pub build: u64,
    /// Node style: `darwin-arm64`, `win32-x64`, `linux-x64`.
    pub platform: String,
    #[serde(rename = "shellApi", default)]
    pub shell_api: u32,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub min_os: String,
    pub entrypoint: String,
    #[serde(default)]
    pub source_commit: String,
    pub files: BTreeMap<String, FileEntry>,
}

/// `engines/<pack_id>/latest.json`: a signed pointer to the newest manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedEntry {
    #[serde(default)]
    pub format: u32,
    #[serde(default)]
    pub pack_id: Option<String>,
    pub version: String,
    pub build: u64,
    /// Manifest path relative to `engines/<pack_id>/`.
    pub manifest: String,
    #[serde(rename = "manifestSha256")]
    pub manifest_sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("signature invalid")]
    BadSignature,
    #[error("manifest is not valid JSON: {0}")]
    BadManifest(String),
    #[error("manifest is for {found}, this machine is {expected}")]
    WrongPlatform { expected: String, found: String },
    #[error("unsafe path in manifest: {0}")]
    UnsafePath(String),
    #[error("{0}: missing")]
    Missing(String),
    #[error("{0}: size or sha256 mismatch")]
    Mismatch(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Node-style platform id of this machine (`process.platform-process.arch`).
pub fn platform_id() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// Raw 32-byte key from SPKI DER base64 (12-byte prefix) or a bare base64 key.
pub fn parse_public_key(text: &str) -> Option<VerifyingKey> {
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    let der = base64::engine::general_purpose::STANDARD.decode(body.trim()).ok()?;
    let raw: [u8; 32] = match der.len() {
        32 => der.try_into().ok()?,
        44 => der[12..].try_into().ok()?,
        _ => return None,
    };
    VerifyingKey::from_bytes(&raw).ok()
}

/// True when `signature_b64` is a valid signature of `data` by any key.
pub fn verify_signature(data: &[u8], signature_b64: &str, keys: &[VerifyingKey]) -> bool {
    let Ok(sig) = base64::engine::general_purpose::STANDARD.decode(signature_b64.trim()) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(&sig) else { return false };
    keys.iter().any(|k| k.verify(data, &sig).is_ok())
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// A manifest-relative path that stays inside the pack dir.
pub fn safe_rel(rel: &str) -> Result<&Path, VerifyError> {
    let p = Path::new(rel);
    let ok = !rel.is_empty()
        && !rel.contains('\\')
        && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(p)
    } else {
        Err(VerifyError::UnsafePath(rel.to_string()))
    }
}

/// Parse signed manifest bytes: signature, JSON, platform, paths, entrypoint.
pub fn parse_signed_manifest(
    raw: &[u8],
    signature_b64: &str,
    keys: &[VerifyingKey],
    platform: &str,
) -> Result<PackManifest, VerifyError> {
    if !verify_signature(raw, signature_b64, keys) {
        return Err(VerifyError::BadSignature);
    }
    let m: PackManifest = serde_json::from_slice(raw).map_err(|e| VerifyError::BadManifest(e.to_string()))?;
    if m.platform != platform {
        return Err(VerifyError::WrongPlatform { expected: platform.into(), found: m.platform });
    }
    let version_ok = !m.version.is_empty()
        && m.version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let id_ok = !m.pack_id.is_empty() && m.pack_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !version_ok || !id_ok {
        return Err(VerifyError::BadManifest("bad pack_id or version".into()));
    }
    for (rel, f) in &m.files {
        safe_rel(rel)?;
        if f.sha256.len() != 64 || !f.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(VerifyError::BadManifest(format!("bad sha256 for {rel}")));
        }
    }
    if !m.files.contains_key(&m.entrypoint) {
        return Err(VerifyError::BadManifest("entrypoint is not a listed file".into()));
    }
    Ok(m)
}

/// Check an installed pack dir: `pack.json` + `pack.json.sig` signed, and
/// every listed file present with the right size and sha256. Called before
/// every spawn, so a binary changed on disk after install never runs.
pub fn verify_installed(dir: &Path, keys: &[VerifyingKey], platform: &str) -> Result<PackManifest, VerifyError> {
    let raw = std::fs::read(dir.join("pack.json"))?;
    let sig = std::fs::read_to_string(dir.join("pack.json.sig"))?;
    let m = parse_signed_manifest(&raw, &sig, keys, platform)?;
    for (rel, f) in &m.files {
        let abs = dir.join(safe_rel(rel)?);
        let meta = std::fs::metadata(&abs).map_err(|_| VerifyError::Missing(rel.clone()))?;
        if meta.len() != f.size || sha256_file(&abs)? != f.sha256 {
            return Err(VerifyError::Mismatch(rel.clone()));
        }
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn sign(k: &SigningKey, data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(k.sign(data).to_bytes())
    }

    fn write_pack(dir: &Path, k: &SigningKey, platform: &str) -> Vec<u8> {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/llama-server"), b"#!/bin/sh\necho hi\n").unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            "bin/llama-server".to_string(),
            FileEntry { sha256: sha256_file(&dir.join("bin/llama-server")).unwrap(), size: 18 },
        );
        let m = PackManifest {
            format: 1, kind: "engine-pack".into(), pack_id: "llamacpp-metal-macos-arm64".into(),
            engine: "llamacpp".into(), backend: "metal".into(), version: "b1".into(), build: 1,
            platform: platform.into(), shell_api: 1, os: "macos".into(), arch: "arm64".into(),
            min_os: "13.0".into(), entrypoint: "bin/llama-server".into(), source_commit: String::new(), files,
        };
        let raw = serde_json::to_vec(&m).unwrap();
        std::fs::write(dir.join("pack.json"), &raw).unwrap();
        std::fs::write(dir.join("pack.json.sig"), sign(k, &raw)).unwrap();
        raw
    }

    #[test]
    fn production_key_parses() {
        assert!(parse_public_key(RUNTIME_PUBLIC_KEY_SPKI).is_some());
        let pem = format!("-----BEGIN PUBLIC KEY-----\n{RUNTIME_PUBLIC_KEY_SPKI}\n-----END PUBLIC KEY-----\n");
        assert_eq!(parse_public_key(&pem), parse_public_key(RUNTIME_PUBLIC_KEY_SPKI));
    }

    #[test]
    fn installed_pack_passes_only_untouched_and_signed_by_a_trusted_key() {
        let tmp = tempfile::tempdir().unwrap();
        let k = key();
        let trusted = [k.verifying_key()];
        write_pack(tmp.path(), &k, "darwin-arm64");
        assert!(verify_installed(tmp.path(), &trusted, "darwin-arm64").is_ok());

        // Another key (e.g. the production key against a dev-signed pack).
        let other = [SigningKey::from_bytes(&[9u8; 32]).verifying_key()];
        assert!(matches!(verify_installed(tmp.path(), &other, "darwin-arm64"), Err(VerifyError::BadSignature)));
        // Wrong platform.
        assert!(matches!(verify_installed(tmp.path(), &trusted, "linux-x64"), Err(VerifyError::WrongPlatform { .. })));
        // Same size, different bytes: a swapped binary must not run.
        std::fs::write(tmp.path().join("bin/llama-server"), b"#!/bin/sh\necho no\n").unwrap();
        assert!(matches!(verify_installed(tmp.path(), &trusted, "darwin-arm64"), Err(VerifyError::Mismatch(_))));
        std::fs::remove_file(tmp.path().join("bin/llama-server")).unwrap();
        assert!(matches!(verify_installed(tmp.path(), &trusted, "darwin-arm64"), Err(VerifyError::Missing(_))));
    }

    #[test]
    fn manifest_paths_cannot_escape_the_pack() {
        for bad in ["../x", "/etc/passwd", "a/../../b", "", "a\\b"] {
            assert!(safe_rel(bad).is_err(), "{bad}");
        }
        assert!(safe_rel("mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib").is_ok());
    }
}
