//! Shared auth for requests cloud-api relays to this runtime, signed with the
//! runtime's device token. Used by `voice_calls` and any other relayed
//! envelope (discord-app, ...).
//!
//! Headers: `x-allternit-runtime-sig: v1=<hex HMAC-SHA256(relay_key,
//! "<ts>.<METHOD>.<path>.<hex sha256(body)>")>`, `x-allternit-runtime-ts` (unix
//! seconds, ±300 s) and `x-allternit-owner` (must be the owner this runtime is
//! paired as). Unsigned requests are never accepted; with no device token
//! available the extractor answers 503.
//!
//! The HMAC key is `relay_key = sha256_hex(device_token)`: the ASCII bytes of
//! the lowercase hex digest, not the raw token. cloud-api only stores that
//! digest (`credential_hash`), so it can sign without the raw token.
//!
//! The secret comes from a [`RelaySecret`]. Production uses
//! [`EnvOrFileRelaySecret`]: env `ALLTERNIT_RUNTIME_DEVICE_TOKEN` +
//! `ALLTERNIT_RUNTIME_OWNER_ID` first, else the `runtime-identity.json` that
//! allternit-node and agent-daemon share. Routes using [`RelayedAuth`] must
//! carry an `Extension<Arc<dyn RelaySecret>>` layer.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

pub const SIG_HEADER: &str = "x-allternit-runtime-sig";
pub const TS_HEADER: &str = "x-allternit-runtime-ts";
pub const OWNER_HEADER: &str = "x-allternit-owner";
/// Allowed clock skew between cloud-api and this runtime, either direction.
pub const MAX_SKEW_SECS: i64 = 300;
const MAX_BODY: usize = 1024 * 1024;

const ENV_TOKEN: &str = "ALLTERNIT_RUNTIME_DEVICE_TOKEN";
const ENV_OWNER: &str = "ALLTERNIT_RUNTIME_OWNER_ID";
const ENV_IDENTITY_PATH: &str = "ALLTERNIT_RUNTIME_IDENTITY_PATH";

// ---------------------------------------------------------------- relay secret

/// The HMAC key for relayed requests: lowercase hex `sha256(device_token)`.
pub fn relay_key_from_device_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// What this runtime verifies relayed requests with: the device token cloud-api
/// issued when it paired, and the owner it paired as.
pub trait RelaySecret: Send + Sync {
    fn device_token(&self) -> Option<String>;
    fn paired_owner(&self) -> Option<String>;
    /// `(relay_key, owner)`, the key already derived from the device token by
    /// [`relay_key_from_device_token`]. Read together so a rotating file can't
    /// be seen half-updated.
    fn credentials(&self) -> Option<(String, String)> {
        Some((relay_key_from_device_token(&self.device_token()?), self.paired_owner()?))
    }
}

/// A fixed token + owner. For tests and embedders that already hold both.
pub struct StaticRelaySecret {
    pub token: String,
    pub owner: String,
}

impl RelaySecret for StaticRelaySecret {
    fn device_token(&self) -> Option<String> {
        Some(self.token.clone())
    }
    fn paired_owner(&self) -> Option<String> {
        Some(self.owner.clone())
    }
}

/// No credentials: signed requests answer 503.
pub struct UnconfiguredRelaySecret;

impl RelaySecret for UnconfiguredRelaySecret {
    fn device_token(&self) -> Option<String> {
        None
    }
    fn paired_owner(&self) -> Option<String> {
        None
    }
}

#[derive(Deserialize)]
struct Identity {
    #[serde(rename = "deviceToken", default)]
    device_token: String,
    #[serde(rename = "userId", default)]
    user_id: String,
    #[serde(rename = "expiresAt", default)]
    expires_at: Option<String>,
}

impl Identity {
    /// Token + owner, unless either is empty or `expiresAt` has passed (an
    /// unparseable expiry fails closed; an absent/empty one counts as fresh,
    /// as in allternit-node).
    fn raw_credentials(&self, now: chrono::DateTime<chrono::Utc>) -> Option<(String, String)> {
        if self.device_token.is_empty() || self.user_id.is_empty() {
            return None;
        }
        if let Some(raw) = self.expires_at.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let expiry = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
            if expiry.with_timezone(&chrono::Utc) <= now {
                return None;
            }
        }
        Some((self.device_token.clone(), self.user_id.clone()))
    }
}

type EnvLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Production secret: env first, else the shared identity file, re-read when
/// its mtime/size changes (allternit-node rewrites it when the token rotates).
pub struct EnvOrFileRelaySecret {
    env: EnvLookup,
    cache: Mutex<Option<((SystemTime, u64), Option<Arc<Identity>>)>>,
}

impl EnvOrFileRelaySecret {
    /// Raw `(device_token, owner)`; the HMAC key is derived from it.
    fn raw_credentials(&self) -> Option<(String, String)> {
        if let (Some(token), Some(owner)) = (self.env_nonempty(ENV_TOKEN), self.env_nonempty(ENV_OWNER)) {
            return Some((token, owner));
        }
        self.file_identity()?.raw_credentials(chrono::Utc::now())
    }

    pub fn from_process_env() -> Self {
        Self::with_env(Box::new(|k| std::env::var(k).ok()))
    }

    pub fn with_env(env: EnvLookup) -> Self {
        Self { env, cache: Mutex::new(None) }
    }

    fn env_nonempty(&self, key: &str) -> Option<String> {
        (self.env)(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    }

    fn identity_path(&self) -> Option<PathBuf> {
        if let Some(p) = self.env_nonempty(ENV_IDENTITY_PATH) {
            return Some(PathBuf::from(p));
        }
        let home = self.env_nonempty("HOME")?;
        Some(PathBuf::from(home).join(".config").join("allternit").join("runtime-identity.json"))
    }

    fn file_identity(&self) -> Option<Arc<Identity>> {
        let path = self.identity_path()?;
        let meta = std::fs::metadata(&path).ok()?;
        let key = (meta.modified().ok()?, meta.len());
        let mut cache = self.cache.lock().unwrap();
        if let Some((k, ident)) = cache.as_ref() {
            if *k == key {
                return ident.clone();
            }
        }
        let ident = std::fs::read_to_string(&path).ok().and_then(|raw| serde_json::from_str::<Identity>(&raw).ok()).map(Arc::new);
        *cache = Some((key, ident.clone()));
        ident
    }
}

impl RelaySecret for EnvOrFileRelaySecret {
    fn device_token(&self) -> Option<String> {
        self.raw_credentials().map(|c| c.0)
    }
    fn paired_owner(&self) -> Option<String> {
        self.raw_credentials().map(|c| c.1)
    }
    fn credentials(&self) -> Option<(String, String)> {
        self.raw_credentials().map(|(token, owner)| (relay_key_from_device_token(&token), owner))
    }
}

// ---------------------------------------------------------------- signature

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// Missing or malformed signature headers, bad signature, stale ts, wrong owner.
    Unauthorized(&'static str),
    /// A signature was presented but this runtime has no token to check it with.
    NotConfigured,
}

pub fn body_sha256_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// The hex signature for `(ts, method, path, body)`, keyed with the derived
/// `relay_key` (see [`relay_key_from_device_token`]). Cloud-api computes the same.
pub fn sign_relay(relay_key: &str, ts: i64, method: &str, path: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(relay_key.as_bytes()).expect("hmac takes any key length");
    mac.update(format!("{ts}.{method}.{path}.{}", body_sha256_hex(body)).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify one relayed request; returns the owner it acts for. The compare is
/// constant-time (`Mac::verify_slice`).
pub fn verify_relay(
    secret: &dyn RelaySecret,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
    now: i64,
) -> Result<String, AuthError> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty());
    let sig = header(SIG_HEADER).ok_or(AuthError::Unauthorized("missing signature"))?;
    let ts = header(TS_HEADER).ok_or(AuthError::Unauthorized("missing timestamp"))?;
    let owner = header(OWNER_HEADER).ok_or(AuthError::Unauthorized("missing owner"))?;
    let sig_hex = sig.strip_prefix("v1=").ok_or(AuthError::Unauthorized("unsupported signature version"))?;
    let sig_bytes = hex::decode(sig_hex).map_err(|_| AuthError::Unauthorized("malformed signature"))?;
    let ts: i64 = ts.parse().map_err(|_| AuthError::Unauthorized("malformed timestamp"))?;
    let Some((relay_key, paired)) = secret.credentials() else {
        return Err(AuthError::NotConfigured);
    };
    if (now - ts).abs() > MAX_SKEW_SECS {
        return Err(AuthError::Unauthorized("stale timestamp"));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(relay_key.as_bytes()).expect("hmac takes any key length");
    mac.update(format!("{ts}.{method}.{path}.{}", body_sha256_hex(body)).as_bytes());
    mac.verify_slice(&sig_bytes).map_err(|_| AuthError::Unauthorized("bad signature"))?;
    if owner != paired {
        return Err(AuthError::Unauthorized("owner does not match this runtime"));
    }
    Ok(owner.to_string())
}

/// The three signature headers cloud-api sends for `(method, path, body)`,
/// signed with `device_token`. For tests and embedders that act as the relay.
pub fn signed_headers(device_token: &str, owner: &str, method: &str, path: &str, body: &[u8]) -> [(&'static str, String); 3] {
    let ts = unix_now();
    let sig = sign_relay(&relay_key_from_device_token(device_token), ts, method, path, body);
    [(SIG_HEADER, format!("v1={sig}")), (TS_HEADER, ts.to_string()), (OWNER_HEADER, owner.to_string())]
}

/// A POST to `path` the way cloud-api relays it. `signed_as` is the
/// `(device_token, owner)` to sign with; `None` sends it unsigned.
pub fn relayed_post(path: &str, body: &[u8], signed_as: Option<(&str, &str)>) -> axum::http::Request<axum::body::Body> {
    let mut request = axum::http::Request::builder().method("POST").uri(path).header("content-type", "application/json");
    if let Some((token, owner)) = signed_as {
        for (k, v) in signed_headers(token, owner, "POST", path, body) {
            request = request.header(k, v);
        }
    }
    request.body(axum::body::Body::from(body.to_vec())).unwrap()
}

/// The process-wide production secret (env, else the shared identity file).
/// One instance, so its identity-file cache is shared by every relayed route.
pub fn process_secret() -> Arc<dyn RelaySecret> {
    static SECRET: std::sync::OnceLock<Arc<dyn RelaySecret>> = std::sync::OnceLock::new();
    SECRET.get_or_init(|| Arc::new(process_env_secret())).clone()
}

#[cfg(not(test))]
fn process_env_secret() -> EnvOrFileRelaySecret {
    EnvOrFileRelaySecret::from_process_env()
}

/// Unit tests never read the developer's real paired identity
/// (`~/.config/allternit/runtime-identity.json`): on a paired Mac it made
/// "is this runtime paired?" checks true and tests depend on the machine.
/// An explicit identity path or device-token env still works.
#[cfg(test)]
fn process_env_secret() -> EnvOrFileRelaySecret {
    EnvOrFileRelaySecret::with_env(Box::new(|k| if k == "HOME" { None } else { std::env::var(k).ok() }))
}

/// Layer that gives [`RelayedAuth`] handlers their secret.
pub fn secret_layer(secret: Arc<dyn RelaySecret>) -> axum::Extension<Arc<dyn RelaySecret>> {
    axum::Extension(secret)
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn fail(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// Extractor: reads the raw body, verifies the relay signature over it, and
/// yields the owner plus the verified bytes.
pub struct RelayedAuth {
    pub owner: String,
    pub body: bytes::Bytes,
}

impl RelayedAuth {
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, Response> {
        serde_json::from_slice(&self.body).map_err(|e| fail(StatusCode::BAD_REQUEST, &format!("invalid body: {e}")))
    }
}

#[async_trait]
impl<S: Send + Sync> FromRequest<S> for RelayedAuth {
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let secret = parts.extensions.get::<Arc<dyn RelaySecret>>().cloned().ok_or_else(|| fail(StatusCode::SERVICE_UNAVAILABLE, "relay not configured"))?;
        let body = axum::body::to_bytes(Body::new(body), MAX_BODY).await.map_err(|_| fail(StatusCode::PAYLOAD_TOO_LARGE, "body too large"))?;
        let path = parts.extensions.get::<axum::extract::OriginalUri>().map(|u| u.0.path().to_string()).unwrap_or_else(|| parts.uri.path().to_string());
        match verify_relay(secret.as_ref(), &parts.headers, parts.method.as_str(), &path, &body, unix_now()) {
            Ok(owner) => Ok(Self { owner, body }),
            Err(AuthError::Unauthorized(why)) => Err(fail(StatusCode::UNAUTHORIZED, why)),
            Err(AuthError::NotConfigured) => Err(fail(StatusCode::SERVICE_UNAVAILABLE, "relay not configured")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const TOKEN: &str = "allternit_runtime_test_token";
    const OWNER: &str = "user-a";

    struct Secret(Option<(&'static str, &'static str)>);
    impl RelaySecret for Secret {
        fn device_token(&self) -> Option<String> {
            self.0.map(|s| s.0.to_string())
        }
        fn paired_owner(&self) -> Option<String> {
            self.0.map(|s| s.1.to_string())
        }
    }

    fn headers(sig: &str, ts: i64, owner: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(SIG_HEADER, sig.parse().unwrap());
        h.insert(TS_HEADER, ts.to_string().parse().unwrap());
        h.insert(OWNER_HEADER, owner.parse().unwrap());
        h
    }

    /// Signed the way cloud-api does: keyed with `sha256_hex(token)`.
    fn signed(token: &str, ts: i64, owner: &str, method: &str, path: &str, body: &[u8]) -> HeaderMap {
        headers(&format!("v1={}", sign_relay(&relay_key_from_device_token(token), ts, method, path, body)), ts, owner)
    }

    /// Same, but keyed with the raw token (the old scheme).
    fn signed_raw(token: &str, ts: i64, owner: &str, method: &str, path: &str, body: &[u8]) -> HeaderMap {
        headers(&format!("v1={}", sign_relay(token, ts, method, path, body)), ts, owner)
    }

    // Pinned so cloud-api can reproduce the scheme byte for byte.
    const KAT_KEY: &str = "c8963414bf6c4c869eeac5f8a057c3dc574d422f1b108397b66f67bab3d2f981";
    const KAT_SIG: &str = "34edb38cb1c7839397d5993a055972d2f352b42164bcdfd935264cdcae1d1576";

    #[test]
    fn known_answer_vector_pins_the_key_and_signature() {
        let key = relay_key_from_device_token("tok-123");
        assert_eq!(key, KAT_KEY);
        assert_eq!(sign_relay(&key, 1_700_000_000, "POST", "/api/v1/voice/calls", b"{}"), KAT_SIG);
    }

    #[test]
    fn a_raw_token_signature_is_rejected() {
        let (secret, now, body) = (Secret(Some((TOKEN, OWNER))), unix_now(), br#"{"a":1}"#);
        let path = "/api/v1/voice/calls";
        let h = signed_raw(TOKEN, now, OWNER, "POST", path, body);
        assert_eq!(verify_relay(&secret, &h, "POST", path, body, now), Err(AuthError::Unauthorized("bad signature")));
    }

    #[test]
    fn verifier_accepts_a_good_signature_and_returns_the_owner() {
        let (secret, now, body) = (Secret(Some((TOKEN, OWNER))), unix_now(), br#"{"a":1}"#);
        let h = signed(TOKEN, now, OWNER, "POST", "/api/v1/voice/calls", body);
        assert_eq!(verify_relay(&secret, &h, "POST", "/api/v1/voice/calls", body, now), Ok(OWNER.to_string()));
    }

    #[test]
    fn verifier_rejects_bad_stale_future_wrong_owner_and_tampered() {
        let (secret, now, body) = (Secret(Some((TOKEN, OWNER))), unix_now(), br#"{"a":1}"#);
        let path = "/api/v1/voice/calls";
        let bad = |r: Result<String, AuthError>| matches!(r, Err(AuthError::Unauthorized(_)));
        // wrong key
        assert!(bad(verify_relay(&secret, &signed("other", now, OWNER, "POST", path, body), "POST", path, body, now)));
        // stale and future timestamps (validly signed for their own ts)
        assert!(bad(verify_relay(&secret, &signed(TOKEN, now - 301, OWNER, "POST", path, body), "POST", path, body, now)));
        assert!(bad(verify_relay(&secret, &signed(TOKEN, now + 301, OWNER, "POST", path, body), "POST", path, body, now)));
        assert!(verify_relay(&secret, &signed(TOKEN, now - 299, OWNER, "POST", path, body), "POST", path, body, now).is_ok());
        // wrong owner header, validly signed
        assert!(bad(verify_relay(&secret, &signed(TOKEN, now, "user-b", "POST", path, body), "POST", path, body, now)));
        // body, method and path tamper
        let h = signed(TOKEN, now, OWNER, "POST", path, body);
        assert!(bad(verify_relay(&secret, &h, "POST", path, br#"{"a":2}"#, now)));
        assert!(bad(verify_relay(&secret, &h, "DELETE", path, body, now)));
        assert!(bad(verify_relay(&secret, &h, "POST", "/api/v1/voice/calls/x/turn", body, now)));
        // missing headers, wrong version, non-hex, bad ts
        assert!(bad(verify_relay(&secret, &HeaderMap::new(), "POST", path, body, now)));
        let sig = sign_relay(&relay_key_from_device_token(TOKEN), now, "POST", path, body);
        assert!(bad(verify_relay(&secret, &headers(&format!("v2={sig}"), now, OWNER), "POST", path, body, now)));
        assert!(bad(verify_relay(&secret, &headers("v1=zz", now, OWNER), "POST", path, body, now)));
        let mut h = headers(&format!("v1={sig}"), now, OWNER);
        h.insert(TS_HEADER, "soon".parse().unwrap());
        assert!(bad(verify_relay(&secret, &h, "POST", path, body, now)));
    }

    #[test]
    fn verifier_without_a_device_token_never_accepts() {
        let (now, body) = (unix_now(), b"{}");
        let h = signed(TOKEN, now, OWNER, "POST", "/p", body);
        assert_eq!(verify_relay(&Secret(None), &h, "POST", "/p", body, now), Err(AuthError::NotConfigured));
        // Unsigned stays a 401-class error even when unconfigured.
        assert!(matches!(verify_relay(&Secret(None), &HeaderMap::new(), "POST", "/p", body, now), Err(AuthError::Unauthorized(_))));
    }


    // ---------------------------------------------------------- EnvOrFileRelaySecret

    fn secret_with(env: &[(&str, &str)]) -> EnvOrFileRelaySecret {
        let map: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        EnvOrFileRelaySecret::with_env(Box::new(move |k| map.get(k).cloned()))
    }

    fn write_identity(path: &std::path::Path, token: &str, user: &str, expires: Option<&str>, mtime_secs: u64) {
        let exp = expires.map(|e| format!(r#","expiresAt":"{e}""#)).unwrap_or_default();
        std::fs::write(path, format!(r#"{{"runtimeId":"rt_1","deviceToken":"{token}","userId":"{user}"{exp}}}"#)).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs)).unwrap();
    }

    fn future() -> String {
        (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339()
    }

    #[test]
    fn env_wins_over_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        write_identity(&path, "file-token", "file-owner", None, 1_000);
        let s = secret_with(&[(ENV_TOKEN, "env-token"), (ENV_OWNER, "env-owner"), (ENV_IDENTITY_PATH, path.to_str().unwrap())]);
        assert_eq!(s.credentials(), Some((relay_key_from_device_token("env-token"), "env-owner".into())));
        // Both env vars are required; one alone falls through to the file.
        let s = secret_with(&[(ENV_TOKEN, "env-token"), (ENV_IDENTITY_PATH, path.to_str().unwrap())]);
        assert_eq!(s.credentials(), Some((relay_key_from_device_token("file-token"), "file-owner".into())));
    }

    #[test]
    fn reads_the_identity_file_and_the_default_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        write_identity(&path, "tok", "user-1", Some(&future()), 1_000);
        let s = secret_with(&[(ENV_IDENTITY_PATH, path.to_str().unwrap())]);
        assert_eq!((s.device_token(), s.paired_owner()), (Some("tok".into()), Some("user-1".into())));
        assert_eq!(s.credentials(), Some((relay_key_from_device_token("tok"), "user-1".into())));

        let cfg = dir.path().join(".config").join("allternit");
        std::fs::create_dir_all(&cfg).unwrap();
        write_identity(&cfg.join("runtime-identity.json"), "home-tok", "home-user", Some(""), 1_000);
        let s = secret_with(&[("HOME", dir.path().to_str().unwrap())]);
        assert_eq!(s.credentials(), Some((relay_key_from_device_token("home-tok"), "home-user".into())));
    }

    #[test]
    fn a_rewritten_file_is_picked_up_on_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        write_identity(&path, "old-token", "user-1", None, 1_000);
        let s = secret_with(&[(ENV_IDENTITY_PATH, path.to_str().unwrap())]);
        assert_eq!(s.device_token().as_deref(), Some("old-token"));
        // Same length, new mtime.
        write_identity(&path, "new-token", "user-1", None, 2_000);
        assert_eq!(s.device_token().as_deref(), Some("new-token"));
    }

    #[test]
    fn expired_missing_or_corrupt_identity_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let s = secret_with(&[(ENV_IDENTITY_PATH, path.to_str().unwrap())]);
        assert_eq!(s.credentials(), None, "missing file");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(s.credentials(), None, "corrupt file");
        write_identity(&path, "tok", "u", Some("2020-01-01T00:00:00Z"), 3_000);
        assert_eq!(s.credentials(), None, "expired");
        write_identity(&path, "tok", "u", Some("garbage"), 4_000);
        assert_eq!(s.credentials(), None, "unparseable expiry fails closed");
        write_identity(&path, "tok", "", None, 5_000);
        assert_eq!(s.credentials(), None, "no owner");
        // Nothing configured at all (no HOME either).
        assert_eq!(secret_with(&[]).credentials(), None);
    }
}
