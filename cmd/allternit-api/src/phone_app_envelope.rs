//! iCTRL protocol v1 envelope + the app-phone pairing crypto, Rust side.
//!
//! Byte-compatible with `ictrl-fork` `iCTRLKit/SecureEnvelope.swift` (and the
//! Desktop's `ictrl-client.ts`). With K the 32-byte session key:
//!
//! ```text
//! tok        = first 4 bytes of SHA-256(K)            (hex on the JSON wire)
//! K_enc_req  = HMAC(K, "ictrl-v1|enc-req"  || nonce)
//! K_enc_resp = HMAC(K, "ictrl-v1|enc-resp" || nonce)
//! K_mac      = HMAC(K, "ictrl-v1|mac"      || nonce)
//! keystream(Kx) = HMAC(Kx, nonce || be32(0)) || HMAC(Kx, nonce || be32(1)) || ...
//! ct = pt XOR keystream(K_enc_req),  tag = HMAC(K_mac, "v1" || tok4 || be64(ts) || nonce || ct)
//! wire: {"v":1,"tok":"<8 hex>","ts":<unix s>,"nonce":"<32 hex>","ct":"<base64>","tag":"<64 hex>"}
//! ```
//!
//! Contract decisions (SPEC "Decisions fixed after the iOS build"): every
//! sealed message, in **both** directions, uses this **request**
//! construction; the node never uses iCTRL's response form. Control frames
//! are the envelope JSON as a WebSocket text frame (plaintext UTF-8 JSON).
//! An ingest frame is one broadcast packet's envelope JSON as UTF-8 bytes in a
//! binary frame; its plaintext is the raw packet bytes.
//!
//! `|ts - now| <= 120 s`, and a nonce seen inside that window is refused
//! ([`ReplayGuard`]). The MAC is checked before the replay cache is touched.
//! Because both directions share one construction, the node records the
//! nonces it seals in the same guard, so a node frame reflected back at the
//! node is refused as a replay.
//!
//! Pairing: X25519 between the phone's and the node's public keys, then
//! `K = HKDF-SHA256(ikm = shared secret, salt = phoneId (UTF-8, canonical
//! lowercase UUID), info = "allternit-app-phone v1", L = 32)`. The 6-digit
//! code both screens show is `be32(SHA-256(appPub || nodePub)[0..4]) mod
//! 1_000_000`, zero-padded.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use curve25519_dalek::montgomery::MontgomeryPoint;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

const LABEL_MAC: &[u8] = b"ictrl-v1|mac";
const LABEL_ENC_REQ: &[u8] = b"ictrl-v1|enc-req";
pub const TIMESTAMP_WINDOW_SECS: i64 = 120;
pub const HKDF_INFO: &[u8] = b"allternit-app-phone v1";
const TAG_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    Malformed,
    WrongKey,
    BadTag,
    Stale,
    Replay,
    NotUtf8,
}

impl EnvelopeError {
    pub fn code(self) -> &'static str {
        match self {
            EnvelopeError::Malformed => "bad_envelope",
            EnvelopeError::WrongKey => "unknown_token",
            EnvelopeError::BadTag => "bad_tag",
            EnvelopeError::Stale => "stale_timestamp",
            EnvelopeError::Replay => "replay",
            EnvelopeError::NotUtf8 => "bad_tag",
        }
    }
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

fn keystream_xor(key: &[u8; 32], nonce: &[u8], data: &mut [u8]) {
    for (counter, chunk) in data.chunks_mut(32).enumerate() {
        let block = hmac(key, &[nonce, &(counter as u32).to_be_bytes()]);
        for (byte, k) in chunk.iter_mut().zip(block.iter()) {
            *byte ^= k;
        }
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A paired phone's session key.
#[derive(Clone)]
pub struct EnvelopeKey {
    key: [u8; 32],
    tok4: [u8; 4],
}

impl EnvelopeKey {
    pub fn new(key: [u8; 32]) -> Self {
        let digest = Sha256::digest(key);
        let mut tok4 = [0u8; 4];
        tok4.copy_from_slice(&digest[..4]);
        Self { key, tok4 }
    }

    pub fn from_hex(hex_key: &str) -> Option<Self> {
        let bytes = hex::decode(hex_key).ok()?;
        let key: [u8; 32] = bytes.try_into().ok()?;
        Some(Self::new(key))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.key)
    }

    pub fn token_id(&self) -> String {
        hex::encode(self.tok4)
    }

    fn verify_request(
        &self,
        tok4: &[u8],
        ts: i64,
        nonce: &[u8],
        ct: &[u8],
        tag: &[u8],
        now: i64,
        replay: &mut ReplayGuard,
    ) -> Result<Vec<u8>, EnvelopeError> {
        if !ct_eq(tok4, &self.tok4) {
            return Err(EnvelopeError::WrongKey);
        }
        let k_mac = hmac(&self.key, &[LABEL_MAC, nonce]);
        let expected = hmac(&k_mac, &[b"v1", tok4, &(ts as u64).to_be_bytes(), nonce, ct]);
        if !ct_eq(&expected, tag) {
            return Err(EnvelopeError::BadTag);
        }
        if (ts - now).abs() > TIMESTAMP_WINDOW_SECS {
            return Err(EnvelopeError::Stale);
        }
        let nonce16: [u8; 16] = nonce.try_into().map_err(|_| EnvelopeError::Malformed)?;
        replay.check(nonce16, ts, now)?;
        let k_enc = hmac(&self.key, &[LABEL_ENC_REQ, nonce]);
        let mut pt = ct.to_vec();
        keystream_xor(&k_enc, nonce, &mut pt);
        Ok(pt)
    }

    /// Open an envelope JSON and return the raw plaintext bytes (an ingest
    /// packet: the frame is the envelope JSON as UTF-8).
    pub fn open_bytes(
        &self,
        envelope_json: &[u8],
        now: i64,
        replay: &mut ReplayGuard,
    ) -> Result<Vec<u8>, EnvelopeError> {
        let value: serde_json::Value =
            serde_json::from_slice(envelope_json).map_err(|_| EnvelopeError::Malformed)?;
        if value["v"].as_i64() != Some(1) {
            return Err(EnvelopeError::Malformed);
        }
        let field = |name: &str| value[name].as_str().ok_or(EnvelopeError::Malformed);
        let ts = value["ts"].as_i64().filter(|ts| *ts >= 0).ok_or(EnvelopeError::Malformed)?;
        let tok4 = hex::decode(field("tok")?).map_err(|_| EnvelopeError::Malformed)?;
        let nonce = hex::decode(field("nonce")?).map_err(|_| EnvelopeError::Malformed)?;
        let tag = hex::decode(field("tag")?).map_err(|_| EnvelopeError::Malformed)?;
        let ct = STANDARD.decode(field("ct")?).map_err(|_| EnvelopeError::Malformed)?;
        if tok4.len() != 4 || nonce.len() != 16 || tag.len() != TAG_LEN {
            return Err(EnvelopeError::Malformed);
        }
        self.verify_request(&tok4, ts, &nonce, &ct, &tag, now, replay)
    }

    /// Open a control frame; the plaintext must be UTF-8.
    pub fn open_text(
        &self,
        text: &str,
        now: i64,
        replay: &mut ReplayGuard,
    ) -> Result<String, EnvelopeError> {
        let pt = self.open_bytes(text.as_bytes(), now, replay)?;
        String::from_utf8(pt).map_err(|_| EnvelopeError::NotUtf8)
    }

    /// Seal a node → phone message with the request construction under a
    /// fresh nonce, recording the nonce in `replay` (see module docs).
    pub fn seal(&self, plaintext: &str, now: i64, replay: &mut ReplayGuard) -> String {
        let mut nonce = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut nonce);
        let _ = replay.check(nonce, now, now);
        self.seal_with(plaintext.as_bytes(), now, &nonce)
    }

    fn seal_with(&self, plaintext: &[u8], ts: i64, nonce: &[u8; 16]) -> String {
        let k_mac = hmac(&self.key, &[LABEL_MAC, nonce]);
        let k_enc = hmac(&self.key, &[LABEL_ENC_REQ, nonce]);
        let mut ct = plaintext.to_vec();
        keystream_xor(&k_enc, nonce, &mut ct);
        let tag = hmac(&k_mac, &[b"v1", &self.tok4, &(ts as u64).to_be_bytes(), nonce, &ct]);
        serde_json::json!({
            "v": 1,
            "tok": self.token_id(),
            "ts": ts,
            "nonce": hex::encode(nonce),
            "ct": STANDARD.encode(&ct),
            "tag": hex::encode(tag),
        })
        .to_string()
    }
}

/// Nonces seen inside the timestamp window, for one phone key. Time-based like
/// the Swift `seenNonces`: a nonce only needs remembering while its timestamp
/// could still pass the window check.
#[derive(Default)]
pub struct ReplayGuard {
    seen: HashMap<[u8; 16], i64>,
    inserts: usize,
}

/// Backstop against a flood (video + audio is ~120 packets/s, ~30k per window).
const REPLAY_CAPACITY: usize = 200_000;

impl ReplayGuard {
    fn check(&mut self, nonce: [u8; 16], ts: i64, now: i64) -> Result<(), EnvelopeError> {
        self.inserts += 1;
        if self.inserts % 4096 == 0 || self.seen.len() >= REPLAY_CAPACITY {
            self.seen
                .retain(|_, seen_ts| (*seen_ts - now).abs() <= TIMESTAMP_WINDOW_SECS);
        }
        if self.seen.len() >= REPLAY_CAPACITY {
            return Err(EnvelopeError::Replay);
        }
        if self.seen.insert(nonce, ts).is_some() {
            return Err(EnvelopeError::Replay);
        }
        Ok(())
    }
}

// ── Pairing ──────────────────────────────────────────────────────────────────

/// The node's half of one pairing attempt.
pub struct NodeKeyPair {
    secret: [u8; 32],
    pub public: [u8; 32],
}

impl NodeKeyPair {
    pub fn generate() -> Self {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        let public = MontgomeryPoint::mul_base_clamped(secret).to_bytes();
        Self { secret, public }
    }

    /// X25519 + HKDF → the session key. `None` for a low-order phone key (an
    /// all-zero shared secret).
    pub fn derive_session_key(&self, app_pub: &[u8; 32], phone_id: &str) -> Option<EnvelopeKey> {
        let shared = MontgomeryPoint(*app_pub).mul_clamped(self.secret).to_bytes();
        if shared.iter().all(|b| *b == 0) {
            return None;
        }
        Some(EnvelopeKey::new(session_key_from_shared(&shared, phone_id)))
    }
}

pub fn session_key_from_shared(shared: &[u8; 32], phone_id: &str) -> [u8; 32] {
    let hk = hkdf::Hkdf::<Sha256>::new(Some(phone_id.as_bytes()), shared);
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm).expect("32 bytes is a valid HKDF length");
    okm
}

/// The 6-digit code both screens show for one pairing.
pub fn pairing_code(app_pub: &[u8; 32], node_pub: &[u8; 32]) -> String {
    let digest = Sha256::new().chain_update(app_pub).chain_update(node_pub).finalize();
    let n = u32::from_be_bytes(digest[..4].try_into().unwrap()) % 1_000_000;
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seal a request exactly as `SecureEnvelope.swift` / `ictrl-client.ts`
    /// `sealRequest` do, written out from the spec independently of the
    /// opener so a construction slip on either side fails here.
    fn swift_seal(key: &[u8; 32], pt: &[u8], ts: i64, nonce: &[u8; 16]) -> (Vec<u8>, [u8; 4], [u8; 32]) {
        let tok4: [u8; 4] = Sha256::digest(key)[..4].try_into().unwrap();
        let k_enc = hmac(key, &[b"ictrl-v1|enc-req", nonce]);
        let mut ct = Vec::new();
        let mut i: u32 = 0;
        while ct.len() < pt.len() {
            ct.extend_from_slice(&hmac(&k_enc, &[nonce, &i.to_be_bytes()]));
            i += 1;
        }
        let ct: Vec<u8> = pt.iter().zip(ct).map(|(a, b)| a ^ b).collect();
        let k_mac = hmac(key, &[b"ictrl-v1|mac", nonce]);
        let tag = hmac(&k_mac, &[b"v1", &tok4, &(ts as u64).to_be_bytes(), nonce, &ct]);
        (ct, tok4, tag)
    }

    fn envelope_json(tok4: [u8; 4], ts: i64, nonce: &[u8; 16], ct: &[u8], tag: [u8; 32]) -> String {
        serde_json::json!({
            "v": 1, "tok": hex::encode(tok4), "ts": ts, "nonce": hex::encode(nonce),
            "ct": STANDARD.encode(ct), "tag": hex::encode(tag),
        })
        .to_string()
    }

    #[test]
    fn opens_swift_sealed_control_and_ingest_frames_and_refuses_tampering() {
        let key = [7u8; 32];
        let env = EnvelopeKey::new(key);
        let now = 1_800_000_000;
        let body = r#"{"id":"1","result":{"url":"https://example.com/é"}}"#;
        let nonce = [3u8; 16];
        let (ct, tok4, tag) = swift_seal(&key, body.as_bytes(), now, &nonce);
        let control = envelope_json(tok4, now, &nonce, &ct, tag);
        let mut replay = ReplayGuard::default();
        assert_eq!(env.open_text(&control, now, &mut replay).unwrap(), body);
        assert_eq!(env.open_text(&control, now, &mut replay), Err(EnvelopeError::Replay));

        // Ingest: a 100-byte binary packet (crosses keystream blocks); the
        // frame is the envelope JSON as UTF-8.
        let packet: Vec<u8> = (0..100u8).map(|b| b.wrapping_mul(37)).collect();
        let nonce = [9u8; 16];
        let (ct, tok4, tag) = swift_seal(&key, &packet, now - 60, &nonce);
        let frame = envelope_json(tok4, now - 60, &nonce, &ct, tag).into_bytes();
        assert_eq!(env.open_bytes(&frame, now, &mut ReplayGuard::default()).unwrap(), packet);
        assert_eq!(env.open_text(std::str::from_utf8(&frame).unwrap(), now, &mut ReplayGuard::default()), Err(EnvelopeError::NotUtf8));

        let mut flipped_ct = ct.clone();
        flipped_ct[40] ^= 1;
        let flipped = envelope_json(tok4, now - 60, &nonce, &flipped_ct, tag).into_bytes();
        assert_eq!(env.open_bytes(&flipped, now, &mut ReplayGuard::default()), Err(EnvelopeError::BadTag));
        // A forged timestamp breaks the tag; a genuinely old one is stale.
        let moved = envelope_json(tok4, now, &nonce, &ct, tag).into_bytes();
        assert_eq!(env.open_bytes(&moved, now, &mut ReplayGuard::default()), Err(EnvelopeError::BadTag));
        assert_eq!(env.open_bytes(&frame, now + 200, &mut ReplayGuard::default()), Err(EnvelopeError::Stale));
        let other = EnvelopeKey::new([8u8; 32]);
        assert_eq!(other.open_bytes(&frame, now, &mut ReplayGuard::default()), Err(EnvelopeError::WrongKey));
        assert_eq!(env.open_bytes(&frame[..50], now, &mut ReplayGuard::default()), Err(EnvelopeError::Malformed));
    }

    #[test]
    fn node_seal_is_the_request_construction_and_is_not_accepted_back() {
        let key = [5u8; 32];
        let env = EnvelopeKey::new(key);
        let now = 1_800_000_000;
        let mut replay = ReplayGuard::default();
        let plain = r#"{"id":"a","method":"web.back","params":{}}"#;
        let sealed = env.seal(plain, now, &mut replay);
        let v: serde_json::Value = serde_json::from_str(&sealed).unwrap();
        // Re-derive the tag and plaintext independently, as the phone does.
        let nonce: [u8; 16] = hex::decode(v["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
        let ct = STANDARD.decode(v["ct"].as_str().unwrap()).unwrap();
        let (expected_ct, tok4, tag) = swift_seal(&key, plain.as_bytes(), now, &nonce);
        assert_eq!(ct, expected_ct);
        assert_eq!(v["tok"].as_str().unwrap(), hex::encode(tok4));
        assert_eq!(v["tag"].as_str().unwrap(), hex::encode(tag));
        // A phone with its own guard opens it; the same frame reflected at the
        // node's guard is a replay.
        assert_eq!(env.open_text(&sealed, now, &mut ReplayGuard::default()).unwrap(), plain);
        assert_eq!(env.open_text(&sealed, now, &mut replay), Err(EnvelopeError::Replay));
    }

    #[test]
    fn both_sides_derive_the_same_key_and_code() {
        let node = NodeKeyPair::generate();
        let app = NodeKeyPair::generate();
        let phone_id = "8c0f6b4e-1d2a-4f3b-9c5d-6e7f8a9b0c1d";
        let a = node.derive_session_key(&app.public, phone_id).unwrap();
        let b = app.derive_session_key(&node.public, phone_id).unwrap();
        assert_eq!(a.to_hex(), b.to_hex());
        assert_ne!(a.to_hex(), node.derive_session_key(&app.public, "other").unwrap().to_hex());
        assert!(node.derive_session_key(&[0u8; 32], phone_id).is_none());
        let code = pairing_code(&app.public, &node.public);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn x25519_matches_rfc7748_vector() {
        // RFC 7748 §6.1 Alice/Bob.
        let alice_sk: [u8; 32] = hex::decode("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a").unwrap().try_into().unwrap();
        let bob_pk: [u8; 32] = hex::decode("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f").unwrap().try_into().unwrap();
        let alice_pk = MontgomeryPoint::mul_base_clamped(alice_sk).to_bytes();
        assert_eq!(hex::encode(alice_pk), "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        let shared = MontgomeryPoint(bob_pk).mul_clamped(alice_sk).to_bytes();
        assert_eq!(hex::encode(shared), "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
    }
}
