//! Standard Webhooks (https://www.standardwebhooks.com) signing and
//! verification — the format MCP Events requires for webhook delivery, and
//! the format Clerk/Svix already send us.
//!
//! ```text
//! webhook-id:        <message id, unique per event, stable across retries>
//! webhook-timestamp: <unix seconds>
//! webhook-signature: v1,<base64 HMAC-SHA256(key, "<id>.<timestamp>.<body>")>
//! ```
//!
//! Secrets are `whsec_` + base64 of 24–64 random bytes. For MCP Events the
//! *client* supplies the secret on `events/subscribe`; we never mint it.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const SECRET_PREFIX: &str = "whsec_";
pub const MIN_KEY_BYTES: usize = 24;
pub const MAX_KEY_BYTES: usize = 64;
/// Receivers reject timestamps further than this from now.
pub const TOLERANCE_SECS: i64 = 5 * 60;

pub const HEADER_ID: &str = "webhook-id";
pub const HEADER_TIMESTAMP: &str = "webhook-timestamp";
pub const HEADER_SIGNATURE: &str = "webhook-signature";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    MissingPrefix,
    NotBase64,
    BadLength(usize),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::MissingPrefix => write!(f, "secret must start with {SECRET_PREFIX}"),
            SecretError::NotBase64 => write!(f, "secret is not valid base64 after {SECRET_PREFIX}"),
            SecretError::BadLength(n) => write!(f, "secret decodes to {n} bytes; must be {MIN_KEY_BYTES}-{MAX_KEY_BYTES}"),
        }
    }
}

/// Decode a `whsec_…` secret to its key bytes, enforcing the 24–64 byte
/// range MCP Events requires.
pub fn parse_secret(secret: &str) -> Result<Vec<u8>, SecretError> {
    let b64 = secret.strip_prefix(SECRET_PREFIX).ok_or(SecretError::MissingPrefix)?;
    let key = B64.decode(b64).map_err(|_| SecretError::NotBase64)?;
    if !(MIN_KEY_BYTES..=MAX_KEY_BYTES).contains(&key.len()) {
        return Err(SecretError::BadLength(key.len()));
    }
    Ok(key)
}

/// `v1,<base64 signature>` for one message.
pub fn sign(key: &[u8], msg_id: &str, timestamp: i64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg_id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("v1,{}", B64.encode(mac.finalize().into_bytes()))
}

/// The three headers for one delivery. Callers add `Content-Type` and any
/// transport-specific headers (MCP Events adds `X-MCP-Subscription-Id`).
pub fn headers(key: &[u8], msg_id: &str, timestamp: i64, body: &[u8]) -> [(&'static str, String); 3] {
    [
        (HEADER_ID, msg_id.to_string()),
        (HEADER_TIMESTAMP, timestamp.to_string()),
        (HEADER_SIGNATURE, sign(key, msg_id, timestamp, body)),
    ]
}

/// Signature header for a key rotation window: one `v1,…` per key,
/// space-separated (receivers accept any match).
pub fn sign_all(keys: &[&[u8]], msg_id: &str, timestamp: i64, body: &[u8]) -> String {
    keys.iter().map(|k| sign(k, msg_id, timestamp, body)).collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    BadTimestamp,
    Expired,
    NoMatchingSignature,
}

/// Verify a received message. `signature_header` may hold several
/// space-separated signatures (key rotation); any `v1` match passes.
pub fn verify(
    key: &[u8],
    msg_id: &str,
    timestamp_header: &str,
    signature_header: &str,
    body: &[u8],
    now_unix: i64,
) -> Result<(), VerifyError> {
    let ts: i64 = timestamp_header.trim().parse().map_err(|_| VerifyError::BadTimestamp)?;
    if (now_unix - ts).abs() > TOLERANCE_SECS {
        return Err(VerifyError::Expired);
    }
    let expected = sign(key, msg_id, ts, body);
    let expected_sig = expected.as_bytes();
    let ok = signature_header
        .split(' ')
        .filter(|s| s.starts_with("v1,"))
        .any(|s| constant_time_eq(s.as_bytes(), expected_sig));
    if ok { Ok(()) } else { Err(VerifyError::NoMatchingSignature) }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference vector from the Standard Webhooks / Svix docs.
    const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    const MSG_ID: &str = "msg_p5jXN8AQM9LWM0D4loKWxJek";
    const TS: i64 = 1614265330;
    const BODY: &str = r#"{"test": 2432232314}"#;
    const SIG: &str = "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=";

    #[test]
    fn reference_vector() {
        let key = parse_secret(SECRET).unwrap();
        assert_eq!(sign(&key, MSG_ID, TS, BODY.as_bytes()), SIG);
        assert!(verify(&key, MSG_ID, &TS.to_string(), SIG, BODY.as_bytes(), TS + 10).is_ok());
    }

    #[test]
    fn verify_rejects_tamper_expiry_and_accepts_rotation_list() {
        let key = parse_secret(SECRET).unwrap();
        let ts = TS.to_string();
        assert_eq!(verify(&key, MSG_ID, &ts, SIG, b"{}", TS), Err(VerifyError::NoMatchingSignature));
        assert_eq!(verify(&key, MSG_ID, &ts, SIG, BODY.as_bytes(), TS + 301), Err(VerifyError::Expired));
        assert_eq!(verify(&key, MSG_ID, "x", SIG, BODY.as_bytes(), TS), Err(VerifyError::BadTimestamp));
        let multi = format!("v1,AAAA {SIG}");
        assert!(verify(&key, MSG_ID, &ts, &multi, BODY.as_bytes(), TS).is_ok());
    }

    #[test]
    fn dual_signature_verifies_under_either_key() {
        let old = parse_secret(SECRET).unwrap();
        let new = vec![9u8; 32];
        let both = sign_all(&[&new, &old], MSG_ID, TS, BODY.as_bytes());
        assert_eq!(both.split(' ').count(), 2);
        assert!(verify(&old, MSG_ID, &TS.to_string(), &both, BODY.as_bytes(), TS).is_ok());
        assert!(verify(&new, MSG_ID, &TS.to_string(), &both, BODY.as_bytes(), TS).is_ok());
    }

    #[test]
    fn secret_rules() {
        assert_eq!(parse_secret("abc"), Err(SecretError::MissingPrefix));
        assert_eq!(parse_secret("whsec_!!"), Err(SecretError::NotBase64));
        let short = format!("whsec_{}", B64.encode([0u8; 16]));
        assert_eq!(parse_secret(&short), Err(SecretError::BadLength(16)));
        let long = format!("whsec_{}", B64.encode([0u8; 65]));
        assert_eq!(parse_secret(&long), Err(SecretError::BadLength(65)));
        let ok = format!("whsec_{}", B64.encode([7u8; 32]));
        assert_eq!(parse_secret(&ok).unwrap().len(), 32);
    }
}
