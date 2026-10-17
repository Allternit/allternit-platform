//! The saved Apple session, so a re-sign needs neither the password nor 2FA.
//!
//! Stored per Apple ID in the Keychain (storage.rs) as JSON:
//!   - the Xcode app token + its expiry and the account's adsid: enough to
//!     talk to Apple's developer services directly;
//!   - the GrandSlam session data (`spd`, binary plist) so an expired Xcode
//!     token can be refreshed without the password;
//!   - the team id chosen last time.
//! The anisette machine state and the signing key live in the same Keychain
//! service under isideload's own keys; keeping the anisette identity is what
//! makes Apple treat this Mac as already verified (no 2FA on later logins).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop, Clone, PartialEq, Eq)]
pub struct StoredSession {
    pub version: u8,
    pub adsid: String,
    pub xcode_token: String,
    /// Milliseconds since the Unix epoch.
    pub token_expiry_ms: u64,
    /// base64 of the binary-plist `spd`.
    pub spd: Option<String>,
    pub team_id: Option<String>,
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSession")
            .field("token_expiry_ms", &self.token_expiry_ms)
            .field("team_id", &self.team_id)
            .finish_non_exhaustive()
    }
}

pub const SESSION_VERSION: u8 = 1;
/// Don't start a sign with a token that lapses within this margin.
pub const TOKEN_MARGIN_MS: u64 = 10 * 60 * 1000;

/// Keychain account name for an Apple ID's session. Hashed, so the Keychain
/// item list doesn't spell out the Apple ID.
pub fn session_key(apple_id: &str) -> String {
    let normalized = apple_id.trim().to_lowercase();
    format!("session/{}", hex::encode(Sha256::digest(normalized.as_bytes())))
}

/// Apple's app-token `expiry` is milliseconds; tolerate seconds too.
pub fn expiry_to_ms(raw: u64) -> u64 {
    if raw < 100_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw
    }
}

pub fn token_is_fresh(expiry_ms: u64, now_ms: u64) -> bool {
    expiry_ms > now_ms.saturating_add(TOKEN_MARGIN_MS)
}

impl StoredSession {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// None for anything unreadable or from another version (then we log in again).
    pub fn from_json(text: &str) -> Option<StoredSession> {
        let s: StoredSession = serde_json::from_str(text).ok()?;
        (s.version == SESSION_VERSION && !s.adsid.is_empty()).then_some(s)
    }

    pub fn token_fresh(&self, now_ms: u64) -> bool {
        !self.xcode_token.is_empty() && token_is_fresh(self.token_expiry_ms, now_ms)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// RFC 3339 (UTC, seconds) from a SystemTime, for `expires_at`.
pub fn rfc3339(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn sample() -> StoredSession {
        StoredSession {
            version: SESSION_VERSION,
            adsid: "000123-45-abcdef".into(),
            xcode_token: "AAAA-token".into(),
            token_expiry_ms: 2_000_000_000_000,
            spd: Some("YnBsaXN0".into()),
            team_id: Some("ABCDE12345".into()),
        }
    }

    #[test]
    fn session_round_trips() {
        let s = sample();
        assert_eq!(StoredSession::from_json(&s.to_json()).unwrap(), s);
    }

    #[test]
    fn rejects_garbage_and_old_versions() {
        assert!(StoredSession::from_json("").is_none());
        assert!(StoredSession::from_json("{}").is_none());
        let mut s = sample();
        s.version = 0;
        assert!(StoredSession::from_json(&s.to_json()).is_none());
    }

    #[test]
    fn debug_hides_tokens() {
        let dbg = format!("{:?}", sample());
        assert!(!dbg.contains("AAAA-token") && !dbg.contains("YnBsaXN0") && !dbg.contains("000123"), "{dbg}");
    }

    #[test]
    fn zeroize_clears_secrets() {
        let mut s = sample();
        s.zeroize();
        assert!(s.xcode_token.is_empty() && s.adsid.is_empty() && s.spd.is_none());
    }

    #[test]
    fn key_is_per_apple_id_case_insensitive_and_hashed() {
        assert_eq!(session_key("Me@Example.com "), session_key("me@example.com"));
        assert_ne!(session_key("a@example.com"), session_key("b@example.com"));
        assert!(!session_key("me@example.com").contains("example"));
    }

    #[test]
    fn expiry_and_freshness() {
        assert_eq!(expiry_to_ms(1_800_000_000), 1_800_000_000_000);
        assert_eq!(expiry_to_ms(1_800_000_000_000), 1_800_000_000_000);
        let now = 1_000_000_000_000;
        assert!(token_is_fresh(now + TOKEN_MARGIN_MS + 1, now));
        assert!(!token_is_fresh(now + TOKEN_MARGIN_MS, now));
        assert!(!token_is_fresh(now - 1, now));
        let mut s = sample();
        s.xcode_token.clear();
        assert!(!s.token_fresh(0));
    }

    #[test]
    fn formats_rfc3339() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(1_792_756_800)), "2026-10-23T12:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
    }
}
