//! Signed per-slot release manifest (plan step 31). Pure logic, shared by PID 1 (verifies the
//! manifest of the slot it is about to boot) and `allternit-update` (verifies before writing).
//!
//! `manifest-<slot>.json` + `manifest-<slot>.sig` live on the ESP in `allternit/`. The JSON is
//! signed byte for byte with Ed25519 (raw 64-byte signature file); the public key (raw 32
//! bytes) is baked into the initramfs, so Secure Boot signs it into the UKI, and into the slot
//! image for the updater. The manifest carries the slot's dm-verity root hash, salt and data
//! block count, so a new slot image never needs a new UKI.
//! TODO: anti-rollback (a monotonic version counter in a UEFI variable or the TPM).

use serde_json::Value;

/// Where the raw 32-byte Ed25519 public key sits in the initramfs and in the slot image.
pub const PUBKEY_PATH: &str = "/etc/allternit/update-pub.key";
pub const SCHEMA: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub slot: String,
    pub image_sha256: String,
    pub image_bytes: u64,
    pub verity_root: String,
    pub verity_salt: String,
    pub data_blocks: u64,
    pub uki_sha256: String,
    pub sbom_sha256: String,
}

pub fn verify_signature(manifest: &[u8], sig: &[u8], pubkey: &[u8]) -> Result<(), String> {
    let pk = ed25519_compact::PublicKey::from_slice(pubkey)
        .map_err(|_| format!("public key is {} bytes, want a raw 32-byte Ed25519 key", pubkey.len()))?;
    let sg = ed25519_compact::Signature::from_slice(sig)
        .map_err(|_| format!("signature is {} bytes, want a raw 64-byte Ed25519 signature", sig.len()))?;
    pk.verify(manifest, &sg)
        .map_err(|_| "signature does not match the manifest (wrong key, or the manifest was changed)".to_string())
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn text(v: &Value, key: &str) -> Result<String, String> {
    v[key].as_str().map(str::to_string).ok_or_else(|| format!("manifest has no \"{key}\""))
}

fn sha(v: &Value, key: &str) -> Result<String, String> {
    let s = text(v, key)?.to_ascii_lowercase();
    if is_hex(&s, 64) {
        Ok(s)
    } else {
        Err(format!("manifest \"{key}\" is not 64 hex digits"))
    }
}

fn count(v: &Value, key: &str) -> Result<u64, String> {
    match v[key].as_u64() {
        Some(n) if n > 0 => Ok(n),
        _ => Err(format!("manifest \"{key}\" is not a positive number")),
    }
}

/// Parse the manifest JSON. Does not check the signature: use `verified`.
pub fn parse(manifest: &[u8]) -> Result<Manifest, String> {
    let v: Value = serde_json::from_slice(manifest).map_err(|e| format!("manifest JSON: {e}"))?;
    if v["schema"].as_u64() != Some(SCHEMA) {
        return Err(format!("manifest schema is not {SCHEMA}"));
    }
    let slot = text(&v, "slot")?;
    if slot != "a" && slot != "b" {
        return Err(format!("manifest slot is \"{slot}\", want a or b"));
    }
    let salt = text(&v, "verity_salt")?.to_ascii_lowercase();
    if salt != "-" && (salt.is_empty() || salt.len() % 2 != 0 || !salt.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("manifest \"verity_salt\" is not hex".into());
    }
    Ok(Manifest {
        version: text(&v, "version")?,
        slot,
        image_sha256: sha(&v, "image_sha256")?,
        image_bytes: count(&v, "image_bytes")?,
        verity_root: sha(&v, "verity_root")?,
        verity_salt: salt,
        data_blocks: count(&v, "data_blocks")?,
        uki_sha256: sha(&v, "uki_sha256")?,
        sbom_sha256: sha(&v, "sbom_sha256")?,
    })
}

/// Signature first, then the contents: nothing in an unsigned manifest is believed.
pub fn verified(manifest: &[u8], sig: &[u8], pubkey: &[u8]) -> Result<Manifest, String> {
    verify_signature(manifest, sig, pubkey)?;
    parse(manifest)
}

impl Manifest {
    /// `allternit.verity.<slot>=<root>:<salt>:<blocks>`, the form `verity::for_slot` reads.
    pub fn verity_param(&self) -> String {
        format!("allternit.verity.{}={}:{}:{}", self.slot, self.verity_root, self.verity_salt, self.data_blocks)
    }

    /// The image about to be written must be the one the manifest describes.
    pub fn check_image(&self, sha256: &str, bytes: u64) -> Result<(), String> {
        if bytes != self.image_bytes {
            return Err(format!("image is {bytes} bytes, which does not match the manifest ({})", self.image_bytes));
        }
        if !sha256.eq_ignore_ascii_case(&self.image_sha256) {
            return Err(format!("image sha256 {sha256} does not match the manifest ({})", self.image_sha256));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_compact::{KeyPair, Seed};

    fn json(slot: &str) -> String {
        let h = "ab".repeat(32);
        format!(
            r#"{{"schema":1,"version":"v1","slot":"{slot}","image_sha256":"{h}","image_bytes":8192,"verity_root":"{h}","verity_salt":"0a0b","data_blocks":2,"uki_sha256":"{h}","sbom_sha256":"{h}"}}"#
        )
    }

    fn signed(msg: &str) -> (Vec<u8>, Vec<u8>) {
        let kp = KeyPair::from_seed(Seed::new([7u8; 32]));
        let sig = kp.sk.sign(msg.as_bytes(), None);
        (kp.pk[..].to_vec(), sig[..].to_vec())
    }

    #[test]
    fn accepts_a_signed_manifest() {
        let msg = json("a");
        let (pk, sig) = signed(&msg);
        let m = verified(msg.as_bytes(), &sig, &pk).unwrap();
        assert_eq!(m.slot, "a");
        assert_eq!(m.verity_param(), format!("allternit.verity.a={}:0a0b:2", "ab".repeat(32)));
        assert!(m.check_image(&"AB".repeat(32), 8192).is_ok());
        assert!(m.check_image(&"cd".repeat(32), 8192).is_err());
        assert!(m.check_image(&"ab".repeat(32), 8193).is_err());
    }

    #[test]
    fn refuses_a_changed_manifest_a_wrong_key_and_junk() {
        let msg = json("a");
        let (pk, sig) = signed(&msg);
        let changed = msg.replace("\"v1\"", "\"v2\"");
        assert!(verified(changed.as_bytes(), &sig, &pk).is_err());
        let other = KeyPair::from_seed(Seed::new([9u8; 32]));
        assert!(verified(msg.as_bytes(), &sig, &other.pk[..]).is_err());
        assert!(verified(msg.as_bytes(), &sig[..63], &pk).is_err());
        assert!(verified(msg.as_bytes(), &sig, &pk[..31]).is_err());
        assert!(parse(b"{not json").is_err());
        assert!(parse(json("c").as_bytes()).is_err());
        assert!(parse(json("a").replace("\"schema\":1", "\"schema\":2").as_bytes()).is_err());
    }
}
