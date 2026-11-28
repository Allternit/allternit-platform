//! User presence for Desktop approvals (CONTRACT.md section 12.3, G0 answer 3).
//!
//! On Desktop the model's shell runs as the same OS user as the window, so the D16 human proof
//! (the per-launch `desktop:<secret>`) says the request came through the app window, not that a
//! person was at the keyboard. Approving therefore also needs an OS user-presence check, which
//! Electron main runs before it forwards the call (`presence.ts`: Touch ID on macOS, Windows Hello
//! on Windows; refused where neither exists). Main then attaches:
//!
//! ```text
//! x-allternit-presence-proof: v1.<expires unix secs>.<hex HMAC-SHA256>
//! HMAC key  = the per-launch human-proof secret (stdin, shared by Electron main and this process only)
//! HMAC data = "allternit.presence.v1\n<request_id>\napprove\n<expires unix secs>"
//! ```
//!
//! The proof is bound to one approval request, lives at most [`MAX_TTL_SECS`], and the request
//! itself resolves once, so a proof cannot be reused. Deny needs no presence. Clerk-session
//! approvals (web, phone) keep their own auth and are not checked here.

use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const PRESENCE_PROOF_HEADER: &str = "x-allternit-presence-proof";
/// Electron main mints proofs for 60 s; anything claiming a longer life is refused.
pub const MAX_TTL_SECS: i64 = 120;
const DOMAIN: &str = "allternit.presence.v1";

fn mac_hex(secret: &str, request_id: &str, expires: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(format!("{DOMAIN}\n{request_id}\napprove\n{expires}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// The header value for `request_id` (Electron main computes the same thing; used by tests).
pub fn mint(secret: &str, request_id: &str, expires: i64) -> String {
    format!("v1.{expires}.{}", mac_hex(secret, request_id, expires))
}

/// True when `proof` is a live presence proof for approving `request_id` under `secret`.
pub fn verify(secret: &str, proof: &str, request_id: &str, now: i64) -> bool {
    let mut parts = proof.trim().splitn(3, '.');
    let (Some("v1"), Some(exp), Some(mac)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let Ok(expires) = exp.parse::<i64>() else {
        return false;
    };
    if expires < now || expires > now + MAX_TTL_SECS {
        return false;
    }
    crate::auth::constant_time_eq(mac, &mac_hex(secret, request_id, expires))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn a_proof_is_bound_to_its_request_secret_and_time() {
        let now = 1_800_000_000;
        let proof = mint(SECRET, "apr_1", now + 60);
        assert!(verify(SECRET, &proof, "apr_1", now));
        assert!(!verify(SECRET, &proof, "apr_2", now), "other request");
        assert!(!verify("another-secret-of-enough-length-000000", &proof, "apr_1", now), "other launch");
        assert!(!verify(SECRET, &proof, "apr_1", now + 61), "expired");
        assert!(!verify(SECRET, &mint(SECRET, "apr_1", now + 3600), "apr_1", now), "too long-lived");
        assert!(!verify(SECRET, "", "apr_1", now));
        assert!(!verify(SECRET, "v1.notanumber.00", "apr_1", now));
    }

    #[test]
    fn matches_the_desktop_main_process_vector() {
        // Same vector as surfaces/allternit-desktop/src/main/presence.test.ts.
        assert_eq!(
            mint(SECRET, "apr_1", 1_800_000_060),
            "v1.1800000060.f92933da31497666592fd04d78854e07ae0cc0951d689671ae0ee62650baabcb"
        );
    }
}
