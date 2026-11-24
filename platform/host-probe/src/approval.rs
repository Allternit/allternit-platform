//! Checks the approval ids the cloud issues for `POST /v1/host/actions`.
//!
//! Format (cloud-api `computers_host::mint_approval`, platform #2772):
//! `ap1.<jti>.<exp>.<utag>.<sig>`
//!
//! - `jti`: 32 lowercase hex, the single-use id
//! - `exp`: unix seconds (issued + 120)
//! - `utag`: first 16 hex of `sha256_hex(user_id)`; only the cloud checks it
//! - `sig`: lowercase hex `HMAC-SHA256(key, "ap1\n<jti>\n<exp>\n<utag>\n<runtime_id>\n<type>\n<target>")`
//!
//! `key` is this runtime's relay key: lowercase `sha256_hex(device_token)`,
//! used as its ASCII bytes. It is the same key the cloud signs relayed
//! requests with (`runtime_devices.credential_hash`), so the node already has
//! it. `runtime_id` is the cloud's id for this runtime.
//!
//! [`RelayKeyApproval`] checks the format, the signature (constant time),
//! the binding to this runtime, the action and the target, and the expiry.
//! Single use is the engine's job: it calls
//! [`crate::changes::ChangeStore::claim_approval`] with the whole id after the
//! verifier passes, so a replayed id is refused there.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::actions::{ActionType, ApprovalVerifier};

/// Prefix and version of the only approval format the node accepts.
pub const APPROVAL_PREFIX: &str = "ap1";

/// Lowercase hex `sha256(device_token)`: the relay key the approval HMAC is
/// keyed with (same derivation as allternit-api `relay_auth`).
pub fn relay_key_from_device_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Why an approval id was refused. The message goes back in the 403 body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalReject {
    Format,
    Signature,
    Expired,
    /// This node has no relay key or runtime id (it is not paired).
    NotPaired,
}

impl ApprovalReject {
    pub fn message(self) -> &'static str {
        match self {
            ApprovalReject::Format => "approval_id is not a cloud approval (format)",
            ApprovalReject::Signature => {
                "approval_id was not issued for this computer, action and target (signature)"
            }
            ApprovalReject::Expired => "approval_id has expired; approve the action again",
            ApprovalReject::NotPaired => {
                "this computer is not paired, so it cannot check approvals"
            }
        }
    }
}

/// The cloud-issued approval check, keyed by this runtime's relay key.
pub struct RelayKeyApproval {
    relay_key: String,
    runtime_id: String,
    clock: fn() -> i64,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl RelayKeyApproval {
    /// `relay_key` = lowercase `sha256_hex(device_token)` (see
    /// [`relay_key_from_device_token`]); `runtime_id` = this runtime's cloud id.
    pub fn new(relay_key: impl Into<String>, runtime_id: impl Into<String>) -> Self {
        RelayKeyApproval {
            relay_key: relay_key.into(),
            runtime_id: runtime_id.into(),
            clock: unix_now,
        }
    }

    /// Replace the clock (tests).
    pub fn with_clock(mut self, clock: fn() -> i64) -> Self {
        self.clock = clock;
        self
    }

    /// Check one id against this runtime, `action` and `target` at `now`.
    pub fn check(
        &self,
        approval_id: &str,
        action: &str,
        target: &str,
        now: i64,
    ) -> Result<(), ApprovalReject> {
        if self.relay_key.is_empty() || self.runtime_id.is_empty() {
            return Err(ApprovalReject::NotPaired);
        }
        let parts: Vec<&str> = approval_id.split('.').collect();
        let [prefix, jti, exp, utag, sig] = parts.as_slice() else {
            return Err(ApprovalReject::Format);
        };
        let lower_hex = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if *prefix != APPROVAL_PREFIX
            || jti.len() != 32
            || !lower_hex(jti)
            || utag.len() != 16
            || !lower_hex(utag)
            || sig.len() != 64
            || !lower_hex(sig)
            || exp.is_empty()
            || !exp.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(ApprovalReject::Format);
        }
        let exp: i64 = exp.parse().map_err(|_| ApprovalReject::Format)?;
        let sig_bytes = hex::decode(sig).map_err(|_| ApprovalReject::Format)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(self.relay_key.as_bytes())
            .expect("hmac takes any key length");
        mac.update(
            format!(
                "{APPROVAL_PREFIX}\n{jti}\n{exp}\n{utag}\n{}\n{action}\n{target}",
                self.runtime_id
            )
            .as_bytes(),
        );
        // Constant-time compare.
        mac.verify_slice(&sig_bytes)
            .map_err(|_| ApprovalReject::Signature)?;
        if exp < now {
            return Err(ApprovalReject::Expired);
        }
        Ok(())
    }
}

impl ApprovalVerifier for RelayKeyApproval {
    fn verify(&self, approval_id: &str, action: ActionType, target: &str) -> Result<(), String> {
        self.check(approval_id, action.as_str(), target, (self.clock)())
            .map_err(|r| r.message().to_string())
    }
}

/// Refuses every approval. The engine's default, so nothing runs until the
/// mounting runtime plugs in [`RelayKeyApproval`].
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyApprovals;

impl ApprovalVerifier for DenyApprovals {
    fn verify(&self, _: &str, _: ActionType, _: &str) -> Result<(), String> {
        Err(ApprovalReject::NotPaired.message().to_string())
    }
}

/// Mint an id the way the cloud does. Tests only (the node never mints).
#[cfg(test)]
pub(crate) fn mint_for_test(
    relay_key: &str,
    runtime_id: &str,
    action: &str,
    target: &str,
    jti: &str,
    exp: i64,
) -> String {
    let utag = "0123456789abcdef";
    let mut mac = Hmac::<Sha256>::new_from_slice(relay_key.as_bytes()).unwrap();
    mac.update(
        format!("{APPROVAL_PREFIX}\n{jti}\n{exp}\n{utag}\n{runtime_id}\n{action}\n{target}")
            .as_bytes(),
    );
    format!(
        "{APPROVAL_PREFIX}.{jti}.{exp}.{utag}.{}",
        hex::encode(mac.finalize().into_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const JTI: &str = "0123456789abcdef0123456789abcdef";

    fn key() -> String {
        relay_key_from_device_token("allternit_runtime_test_token")
    }

    fn v() -> RelayKeyApproval {
        RelayKeyApproval::new(key(), "rt_box").with_clock(|| NOW)
    }

    #[test]
    fn a_cloud_minted_id_passes() {
        let id = mint_for_test(
            &key(),
            "rt_box",
            "service.restart",
            "nginx.service",
            JTI,
            NOW + 120,
        );
        assert!(id.len() <= 129, "fits the cloud's length bound");
        assert!(crate::changes::safe_file_name(&id), "fits the change store");
        assert_eq!(
            v().check(&id, "service.restart", "nginx.service", NOW),
            Ok(())
        );
        assert_eq!(
            v().verify(&id, ActionType::ServiceRestart, "nginx.service"),
            Ok(())
        );
    }

    #[test]
    fn wrong_signature_fails() {
        let id = mint_for_test(
            &key(),
            "rt_box",
            "service.restart",
            "nginx.service",
            JTI,
            NOW + 120,
        );
        let mut tampered = id.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == '0' { '1' } else { '0' });
        assert_eq!(
            v().check(&tampered, "service.restart", "nginx.service", NOW),
            Err(ApprovalReject::Signature)
        );
        // Signed with another runtime's key.
        let other = mint_for_test(
            &relay_key_from_device_token("another_token"),
            "rt_box",
            "service.restart",
            "nginx.service",
            JTI,
            NOW + 120,
        );
        assert_eq!(
            v().check(&other, "service.restart", "nginx.service", NOW),
            Err(ApprovalReject::Signature)
        );
        // Changing exp breaks the signature too.
        let moved = id.replace(&format!(".{}.", NOW + 120), &format!(".{}.", NOW + 9999));
        assert_eq!(
            v().check(&moved, "service.restart", "nginx.service", NOW),
            Err(ApprovalReject::Signature)
        );
    }

    #[test]
    fn wrong_type_target_or_runtime_fails() {
        let id = mint_for_test(
            &key(),
            "rt_box",
            "service.restart",
            "nginx.service",
            JTI,
            NOW + 120,
        );
        assert_eq!(
            v().check(&id, "service.stop", "nginx.service", NOW),
            Err(ApprovalReject::Signature)
        );
        assert_eq!(
            v().check(&id, "service.restart", "sshd.service", NOW),
            Err(ApprovalReject::Signature)
        );
        let other_runtime = RelayKeyApproval::new(key(), "rt_other").with_clock(|| NOW);
        assert_eq!(
            other_runtime.check(&id, "service.restart", "nginx.service", NOW),
            Err(ApprovalReject::Signature)
        );
    }

    #[test]
    fn expired_fails() {
        let id = mint_for_test(&key(), "rt_box", "port.free", "8080", JTI, NOW - 1);
        assert_eq!(
            v().check(&id, "port.free", "8080", NOW),
            Err(ApprovalReject::Expired)
        );
        // exp == now is still valid (the cloud rejects exp < now too).
        let id = mint_for_test(&key(), "rt_box", "port.free", "8080", JTI, NOW);
        assert_eq!(v().check(&id, "port.free", "8080", NOW), Ok(()));
    }

    #[test]
    fn malformed_ids_fail_on_format() {
        for id in [
            "",
            "ap-stop",
            "ap1",
            "ap1.a.b.c.d",
            "ap2.0123456789abcdef0123456789abcdef.1800000120.0123456789abcdef.00",
            "ap1.0123456789ABCDEF0123456789abcdef.1800000120.0123456789abcdef.0000000000000000000000000000000000000000000000000000000000000000",
            "ap1.0123456789abcdef0123456789abcdef.-1.0123456789abcdef.0000000000000000000000000000000000000000000000000000000000000000",
            "ap1.0123456789abcdef0123456789abcdef.1800000120.0123456789abcdef.0000000000000000000000000000000000000000000000000000000000000000.x",
        ] {
            assert_eq!(
                v().check(id, "port.free", "8080", NOW),
                Err(ApprovalReject::Format),
                "{id}"
            );
        }
    }

    #[test]
    fn an_unpaired_node_refuses_everything() {
        let id = mint_for_test(&key(), "rt_box", "port.free", "8080", JTI, NOW + 60);
        let unpaired = RelayKeyApproval::new("", "rt_box");
        assert_eq!(
            unpaired.check(&id, "port.free", "8080", NOW),
            Err(ApprovalReject::NotPaired)
        );
        assert!(DenyApprovals
            .verify(&id, ActionType::PortFree, "8080")
            .is_err());
    }
}
