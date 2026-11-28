//! `ApprovalTokenV1` (CONTRACT.md 3.2): Ed25519, kernel-signed, bound to the `sha256:<hex>` of the
//! exact action payload, single use, TTL 300 s by default and 900 s at most (section 12.1).
//!
//! `signature` is over the canonical (RFC 8785) JSON of every other field except `token`;
//! `token` is `base64url(canonical JSON of those fields plus signature)` and is what travels.
//! Signing and canonical JSON reuse the factory engine's receipt signer and JCS, so there is one
//! Ed25519 key format (32-byte seed) and one canonicaliser in allternit-api. Where the seed lives
//! per host (OS keystore through Desktop, or a 0600 file on headless servers) is
//! [`super::keystore`].

use std::path::Path;
use std::sync::{Arc, OnceLock};

use allternit_authority::{ActionClass, ApprovalTokenV1, AuthorityError, ErrorCode};
use allternit_factory_engine::receipts::jcs;
use allternit_factory_engine::receipts::sign::{verify_sig, ReceiptSigner};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};

pub const TOKEN_SCHEMA_ID: &str = "allternit.os.ApprovalTokenV1";
pub const SCHEMA_VERSION: &str = "1.0.0";
/// CONTRACT.md 12.1.
pub const DEFAULT_TTL_SECS: u64 = 300;
/// CONTRACT.md 12.1: hard cap, whatever the request asks for.
pub const MAX_TTL_SECS: u64 = 900;
pub use super::keystore::{key_path, SIGNING_KEY_ENV};

/// The signed fields, in the order CONTRACT.md 3.2 lists them (canonical JSON sorts them anyway).
const SIGNED_FIELDS: [&str; 12] = [
    "schema_id",
    "schema_version",
    "token_id",
    "request_id",
    "action_class",
    "action_hash",
    "principal",
    "approved_by",
    "issued_at",
    "expires_at",
    "single_use",
    "key_id",
];

/// TTL from the request, default 300 s, clamped to 1..=900 s.
pub fn clamp_ttl(requested: Option<u64>) -> u64 {
    requested.unwrap_or(DEFAULT_TTL_SECS).clamp(1, MAX_TTL_SECS)
}

/// Timestamps on the wire: RFC 3339, UTC, whole seconds.
pub fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

/// True for `sha256:` + 64 lowercase hex (CONTRACT.md 11.3).
pub fn is_content_hash(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// `sha256:<hex>` of the canonical JSON of an action payload.
pub fn action_hash(payload: &Map<String, Value>) -> anyhow::Result<String> {
    jcs::hash_value(&Value::Object(payload.clone()))
}

/// What a token says once its signature has checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenClaims {
    pub token_id: String,
    pub request_id: String,
    pub action_class: ActionClass,
    pub action_hash: String,
    pub principal: String,
    pub approved_by: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

fn class_str(c: ActionClass) -> String {
    serde_json::to_value(c).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn unsigned_fields(c: &TokenClaims, key_id: &str) -> Value {
    json!({
        "schema_id": TOKEN_SCHEMA_ID,
        "schema_version": SCHEMA_VERSION,
        "token_id": c.token_id,
        "request_id": c.request_id,
        "action_class": class_str(c.action_class),
        "action_hash": c.action_hash,
        "principal": c.principal,
        "approved_by": c.approved_by,
        "issued_at": ts(c.issued_at),
        "expires_at": ts(c.expires_at),
        "single_use": true,
        "key_id": key_id,
    })
}

/// Mint a signed token for `claims`.
pub fn mint(signer: &ReceiptSigner, claims: &TokenClaims) -> Result<ApprovalTokenV1, AuthorityError> {
    let unavailable = |e: anyhow::Error| AuthorityError::kernel_unavailable(format!("token canonical form: {e}"));
    let mut fields = unsigned_fields(claims, signer.kid());
    let signature = signer.sign(jcs::canonicalize(&fields).map_err(unavailable)?.as_bytes());
    fields["signature"] = json!(signature);
    let token = B64.encode(jcs::canonicalize(&fields).map_err(unavailable)?.as_bytes());
    Ok(ApprovalTokenV1 {
        schema_id: TOKEN_SCHEMA_ID.into(),
        schema_version: SCHEMA_VERSION.into(),
        token_id: claims.token_id.clone(),
        request_id: claims.request_id.clone(),
        action_class: claims.action_class,
        action_hash: claims.action_hash.clone(),
        principal: claims.principal.clone(),
        approved_by: claims.approved_by.clone(),
        issued_at: ts(claims.issued_at),
        expires_at: ts(claims.expires_at),
        single_use: true,
        key_id: signer.kid().to_string(),
        signature,
        token,
        redemption_receipt_id: None,
        extensions: None,
    })
}

/// Check a token's signature and shape. Expiry, spend and payload binding are the redeem's job.
/// Any failure is `token_invalid`; the reason is for the receipt, not the caller.
pub fn verify(signer: &ReceiptSigner, token: &str) -> Result<TokenClaims, &'static str> {
    let raw = B64.decode(token.trim()).map_err(|_| "token is not base64url")?;
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(&raw) else {
        return Err("token is not a JSON object");
    };
    let Some(Value::String(signature)) = fields.remove("signature") else {
        return Err("token has no signature");
    };
    if fields.len() != SIGNED_FIELDS.len() || !SIGNED_FIELDS.iter().all(|k| fields.contains_key(*k)) {
        return Err("token fields do not match ApprovalTokenV1");
    }
    let s = |k: &str| fields.get(k).and_then(Value::as_str).map(str::to_string);
    if s("schema_id").as_deref() != Some(TOKEN_SCHEMA_ID) || fields.get("single_use") != Some(&Value::Bool(true)) {
        return Err("not a single-use ApprovalTokenV1");
    }
    // One kernel key today; rotation (several known keys by key_id) is a later step.
    if s("key_id").as_deref() != Some(signer.kid()) {
        return Err("unknown key_id");
    }
    let canon = jcs::canonicalize(&Value::Object(fields.clone())).map_err(|_| "token does not canonicalise")?;
    if !verify_sig(&signer.verifying_key(), canon.as_bytes(), &signature) {
        return Err("bad signature");
    }
    let action_class: ActionClass = fields
        .get("action_class")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or("unknown action_class")?;
    let need = |k: &str| s(k).ok_or("missing field");
    Ok(TokenClaims {
        token_id: need("token_id")?,
        request_id: need("request_id")?,
        action_class,
        action_hash: need("action_hash")?,
        principal: need("principal")?,
        approved_by: need("approved_by")?,
        issued_at: s("issued_at").as_deref().and_then(parse_ts).ok_or("bad issued_at")?,
        expires_at: s("expires_at").as_deref().and_then(parse_ts).ok_or("bad expires_at")?,
    })
}

static SIGNER: OnceLock<Arc<ReceiptSigner>> = OnceLock::new();

/// The kernel's Ed25519 signer, loaded once per process. The key is held by the kernel only.
///
/// CONTRACT.md section 12.3 (G0 answer 3): on Desktop the seed lives in the OS keystore (macOS
/// Keychain, Windows DPAPI, Linux Secret Service), owned by Electron main and handed over on stdin;
/// a same-user process can no longer read it from a file. Headless servers keep the 0600 file.
/// See [`super::keystore`]. Desktop approvals also need a user-presence check
/// ([`super::presence`]). A failure is `kernel_unavailable` (fail closed), never cached.
pub fn kernel_signer(data_dir: &Path) -> Result<Arc<ReceiptSigner>, AuthorityError> {
    if let Some(s) = SIGNER.get() {
        return Ok(s.clone());
    }
    let store = super::keystore::KeyStore::select(data_dir);
    let signer = store
        .load(data_dir)
        .map_err(|e| AuthorityError::new(ErrorCode::KernelUnavailable, format!("kernel signing key: {e}"), None))?;
    let mut first = false;
    let signer = SIGNER
        .get_or_init(|| {
            first = true;
            Arc::new(signer)
        })
        .clone();
    if first {
        tracing::info!(store = %store.describe(), key_id = signer.kid(), "kernel signing key loaded");
    }
    Ok(signer)
}
