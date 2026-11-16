//! Files drive capability tokens (contracts/files-drive/SPEC.md section 8).
//!
//! ```text
//! <prefix>.<base64url(claims JSON)>.<base64url(Ed25519 signature over "<prefix>." + payload part)>
//! ```
//!
//! This module signs and checks guest tokens (`fgt1`, [`GuestCapabilityClaims`]).
//! Wake tickets (`awt1`) use the same key and the same format.
//!
//! - **Key.** A 32-byte Ed25519 seed, base64, in `ALLTERNIT_FILES_TOKEN_SEED`
//!   (from the deploy's secret store, never the repo). The `kid` is
//!   `ft-` + the first 16 base64url characters of SHA-256(public key), the same
//!   derivation the step 13 wake-ticket signer uses, so one seed gives both
//!   token kinds one `kid`.
//! - **No silent key.** When the seed is unset or not 32 bytes, [`token_key`]
//!   is `None` and every Files sharing route answers `feature_disabled`. This
//!   module never generates a key and keeps it: a per-process key would make
//!   every guest token die on restart and differ between cloud-api replicas.
//! - Runtimes fetch the public key from `GET /api/v1/computer-files/token-keys`
//!   and verify offline (`allternit-api` `files_v2::shares`).

use std::sync::OnceLock;

use allternit_files_core::GuestCapabilityClaims;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use ed25519_dalek::{Signature, Signer, SigningKey};
use serde::Serialize;
use sha2::Digest as _;

/// base64 of the 32-byte Ed25519 seed.
pub const SEED_ENV: &str = "ALLTERNIT_FILES_TOKEN_SEED";
/// Guest token prefix (`shares.json` pins it).
pub const GUEST_TOKEN_PREFIX: &str = "fgt1";
/// A guest session lasts at most an hour (SPEC section 8).
pub const GUEST_TOKEN_MAX_SECS: u64 = 3600;
/// Clock skew allowed on `iat`.
const SKEW_SECS: u64 = 60;

pub struct FilesTokenKey {
    signing: SigningKey,
    kid: String,
}

impl FilesTokenKey {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(seed);
        let digest = sha2::Sha256::digest(signing.verifying_key().to_bytes());
        let kid = format!("ft-{}", &URL_SAFE_NO_PAD.encode(digest)[..16]);
        Self { signing, kid }
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn public_key_b64url(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.signing.verifying_key().to_bytes())
    }

    /// `<prefix>.<payload>.<sig>` over any claims body.
    pub fn sign<T: Serialize>(&self, prefix: &str, claims: &T) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap_or_default());
        let signed = format!("{prefix}.{payload}");
        let signature = URL_SAFE_NO_PAD.encode(self.signing.sign(signed.as_bytes()).to_bytes());
        format!("{signed}.{signature}")
    }

    /// The first gateway check: format, `kid`, signature, `typ`, `v`, `iat`
    /// and `exp`. The share's revocation state is the caller's half.
    pub fn verify_guest(&self, token: &str, now: u64) -> Result<GuestCapabilityClaims, TokenError> {
        let mut parts = token.trim().split('.');
        let (Some(prefix), Some(payload), Some(sig), None) = (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(TokenError::Invalid);
        };
        if prefix != GUEST_TOKEN_PREFIX {
            return Err(TokenError::Invalid);
        }
        let sig = URL_SAFE_NO_PAD
            .decode(sig)
            .ok()
            .and_then(|b| Signature::from_slice(&b).ok())
            .ok_or(TokenError::Invalid)?;
        self.signing
            .verifying_key()
            .verify_strict(format!("{prefix}.{payload}").as_bytes(), &sig)
            .map_err(|_| TokenError::Invalid)?;
        // Only signed bytes are parsed.
        let claims: GuestCapabilityClaims = URL_SAFE_NO_PAD
            .decode(payload)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or(TokenError::Invalid)?;
        if claims.kid != self.kid || claims.typ != "files_guest" || claims.v != 1 {
            return Err(TokenError::Invalid);
        }
        if claims.iat > now + SKEW_SECS || claims.exp > claims.iat + GUEST_TOKEN_MAX_SECS {
            return Err(TokenError::Invalid);
        }
        if claims.exp <= now {
            return Err(TokenError::Expired(Box::new(claims)));
        }
        Ok(claims)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenError {
    /// Malformed, not ours (signature, `kid`), wrong `typ`/`v`, or claiming
    /// more than an hour. Names nobody.
    Invalid,
    /// Signed by us but past `exp`. Carries the claims so the denial can be
    /// audited against the share and guest.
    Expired(Box<GuestCapabilityClaims>),
}

/// The key from a seed value; `None` for unset, not base64, or not 32 bytes.
pub fn parse_seed(raw: Option<&str>) -> Option<FilesTokenKey> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let seed: [u8; 32] = STANDARD.decode(raw).ok()?.as_slice().try_into().ok()?;
    Some(FilesTokenKey::from_seed(&seed))
}

/// The process key, read once from [`SEED_ENV`]. `None`: sharing is off.
pub fn token_key() -> Option<&'static FilesTokenKey> {
    static KEY: OnceLock<Option<FilesTokenKey>> = OnceLock::new();
    KEY.get_or_init(|| {
        let key = parse_seed(std::env::var(SEED_ENV).ok().as_deref());
        if key.is_none() {
            tracing::warn!("{SEED_ENV} unset or invalid: Files sharing answers feature_disabled");
        }
        key
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_files_core::ShareRole;

    fn claims(key: &FilesTokenKey, now: u64) -> GuestCapabilityClaims {
        GuestCapabilityClaims {
            v: 1,
            typ: "files_guest".into(),
            share_id: "sh_1".into(),
            guest_id: "gst_1".into(),
            owner_user_id: "user_a".into(),
            computer_id: "rd_a".into(),
            root_id: "rt_1".into(),
            item_id: "it_1".into(),
            role: ShareRole::ViewDownload,
            guest_wake: false,
            passcode_ok: true,
            iat: now,
            exp: now + 600,
            kid: key.kid().to_string(),
        }
    }

    #[test]
    fn no_seed_means_no_key() {
        assert!(parse_seed(None).is_none());
        assert!(parse_seed(Some("")).is_none());
        assert!(parse_seed(Some("not base64!")).is_none());
        // 31 bytes is not a seed.
        assert!(parse_seed(Some(&STANDARD.encode([1u8; 31]))).is_none());
        let key = parse_seed(Some(&STANDARD.encode([1u8; 32]))).expect("32-byte seed");
        assert!(key.kid().starts_with("ft-"));
        assert_eq!(key.kid().len(), 19);
    }

    #[test]
    fn guest_token_round_trips_and_refuses_tampering() {
        let key = FilesTokenKey::from_seed(&[7u8; 32]);
        let now = 1_800_000_000;
        let c = claims(&key, now);
        let token = key.sign(GUEST_TOKEN_PREFIX, &c);
        assert_eq!(key.verify_guest(&token, now).unwrap(), c);

        // Payload swapped to an edit role, signature kept.
        let mut edit = c.clone();
        edit.role = ShareRole::Edit;
        let edit_token = key.sign(GUEST_TOKEN_PREFIX, &edit);
        let forged = format!(
            "fgt1.{}.{}",
            edit_token.split('.').nth(1).unwrap(),
            token.split('.').nth(2).unwrap()
        );
        assert_eq!(key.verify_guest(&forged, now), Err(TokenError::Invalid));
        // Another key's signature.
        let other = FilesTokenKey::from_seed(&[8u8; 32]);
        let mut theirs = c.clone();
        theirs.kid = key.kid().to_string();
        assert_eq!(key.verify_guest(&other.sign(GUEST_TOKEN_PREFIX, &theirs), now), Err(TokenError::Invalid));
        // A wake-ticket prefix is not a guest token.
        assert_eq!(key.verify_guest(&token.replacen("fgt1.", "awt1.", 1), now), Err(TokenError::Invalid));
        // Expired, and longer than an hour.
        assert!(matches!(key.verify_guest(&token, now + 600), Err(TokenError::Expired(_))));
        let mut long = c.clone();
        long.exp = long.iat + GUEST_TOKEN_MAX_SECS + 1;
        assert_eq!(key.verify_guest(&key.sign(GUEST_TOKEN_PREFIX, &long), now), Err(TokenError::Invalid));
    }
}
