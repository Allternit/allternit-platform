//! Artifact ids: `art_` + a ULID (26 Crockford base32 chars: 48-bit
//! millisecond timestamp, then 80 random bits), so ids sort by creation time.
//! Clients may supply their own id (idempotent create, deterministic legacy
//! ids `art_legacy_<source>_<localId>`); [`is_valid_client_id`] bounds them.

use rand::RngCore;

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub const PREFIX: &str = "art_";
pub const MAX_ID_LEN: usize = 128;

/// A new ULID string (26 chars).
pub fn ulid() -> String {
    let millis = chrono::Utc::now().timestamp_millis().max(0) as u128 & ((1u128 << 48) - 1);
    let mut random = [0u8; 10];
    rand::thread_rng().fill_bytes(&mut random);
    let mut value = millis << 80;
    for (i, byte) in random.iter().enumerate() {
        value |= (*byte as u128) << (8 * (9 - i));
    }
    let mut out = [0u8; 26];
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = 5 * (25 - i);
        *slot = CROCKFORD[((value >> shift) & 0x1f) as usize];
    }
    String::from_utf8(out.to_vec()).expect("ascii")
}

pub fn new_artifact_id() -> String {
    format!("{PREFIX}{}", ulid())
}

/// Client-supplied ids: `art_` prefix, then 1+ of `[A-Za-z0-9_-]`, at most
/// 128 chars in total.
pub fn is_valid_client_id(id: &str) -> bool {
    id.len() <= MAX_ID_LEN
        && id.strip_prefix(PREFIX).is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulids_are_26_crockford_chars_and_time_ordered() {
        let a = new_artifact_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = new_artifact_id();
        assert_eq!(a.len(), 4 + 26);
        assert!(a[4..].bytes().all(|c| CROCKFORD.contains(&c)));
        assert!(a < b, "{a} should sort before {b}");
        assert!(is_valid_client_id(&a));
    }

    #[test]
    fn client_ids() {
        assert!(is_valid_client_id("art_legacy_canvas_123e4567-e89b"));
        assert!(!is_valid_client_id("art_"));
        assert!(!is_valid_client_id("doc_123"));
        assert!(!is_valid_client_id("art_a/b"));
        assert!(!is_valid_client_id(&format!("art_{}", "x".repeat(200))));
    }
}
