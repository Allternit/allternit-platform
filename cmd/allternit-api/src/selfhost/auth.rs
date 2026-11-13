//! Authentication and roles for the selfhost API.
//!
//! | Credential | Header | Accepted on |
//! |---|---|---|
//! | Server API key `ask_...` | `Authorization: Bearer` | Client routes, by role |
//! | OIDC id token | `Authorization: Bearer` | Client routes, role from a claim |
//! | Environment key `aek_...` | `Authorization: Bearer` | Its own environment's work routes only |
//! | Session secret | `X-Session-Secret` | Its own work item's tool call routes only |
//! | Internal token | `Authorization: Bearer` | `/internal/*`, from loopback only |
//! | Setup token | `X-Allternit-Self-Hosted-Token` or Bearer | `POST /v1/api_keys` while no key exists |
//!
//! Roles: `viewer` may only read (GET/HEAD). `developer` may do everything
//! except key administration. `admin` may do everything.
//!
//! Keys and secrets are stored as SHA-256 hashes only.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method};
use axum::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;

use super::egress_guard::EgressClient;
use super::settings::OidcSettings;
use super::store::{hash_secret, Role, WorkItem};
use super::{ApiError, SelfHostState};
use crate::auth::constant_time_eq;

pub const SESSION_SECRET_HEADER: &str = "x-session-secret";
/// Same header name the hosted API's self-hosted wizard uses.
pub const SETUP_TOKEN_HEADER: &str = "x-allternit-self-hosted-token";

pub const API_KEY_PREFIX: &str = "ask";
pub const ENV_KEY_PREFIX: &str = "aek";

/// Don't write `last_used_at` more often than this per key.
const TOUCH_EVERY_SECS: i64 = 60;

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// First characters of a key, shown in key lists: `ask_1a2b3c4d`.
pub fn key_prefix(secret: &str) -> String {
    secret.chars().take(12).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerKind {
    ApiKey { key_id: String },
    Oidc,
}

/// A person or client signed in with a server API key or OIDC.
#[derive(Debug, Clone)]
pub struct Caller {
    pub subject: String,
    pub role: Role,
    pub kind: CallerKind,
}

/// Can `role` call `method`? Key administration needs admin on top.
pub fn role_allows(role: Role, method: &Method) -> bool {
    match role {
        Role::Admin | Role::Developer => true,
        Role::Viewer => method == Method::GET || method == Method::HEAD,
    }
}

/// Resolve the bearer of a client route to a [`Caller`].
pub async fn authenticate_user(st: &SelfHostState, headers: &HeaderMap) -> Result<Caller, ApiError> {
    let Some(token) = bearer(headers) else {
        if headers.contains_key(SESSION_SECRET_HEADER) {
            return Err(ApiError::unauthorized(
                "A session secret only works on its own tool call routes.",
            ));
        }
        return Err(ApiError::unauthorized(
            "Send a server API key or an OIDC token in the Authorization header.",
        ));
    };
    if token.starts_with("aek_") {
        return Err(ApiError::forbidden(
            "forbidden",
            "An environment key only works on its environment's worker routes.",
        ));
    }
    if token.starts_with("ask_") {
        let now = Utc::now();
        let key = st
            .store
            .find_api_key_by_hash(&hash_secret(token))
            .await?
            .filter(|k| k.is_active(now))
            .ok_or_else(|| ApiError::unauthorized("The API key is not valid."))?;
        if key
            .last_used_at
            .map_or(true, |t| now - t > Duration::seconds(TOUCH_EVERY_SECS))
        {
            let store = st.store.clone();
            let id = key.id.clone();
            tokio::spawn(async move {
                let _ = store.touch_api_key(&id, now).await;
            });
        }
        return Ok(Caller {
            subject: format!("api_key:{}", key.id),
            role: key.role,
            kind: CallerKind::ApiKey { key_id: key.id },
        });
    }
    match &st.oidc {
        Some(v) if token.matches('.').count() == 2 => v.verify(token).await,
        _ => Err(ApiError::unauthorized("The credential is not valid.")),
    }
}

/// Does the request carry the setup token? Only meaningful while the key
/// table is empty; callers check that.
pub fn setup_token_matches(st: &SelfHostState, headers: &HeaderMap) -> bool {
    let Some(expected) = st.setup_token.as_deref() else {
        return false;
    };
    let provided = headers
        .get(SETUP_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .or_else(|| bearer(headers));
    provided.is_some_and(|p| constant_time_eq(p, expected))
}

// ─── Extractors ─────────────────────────────────────────────────────────────

/// Any signed-in client. Non-read methods need `developer` or `admin`.
pub struct User(pub Caller);

#[async_trait]
impl FromRequestParts<SelfHostState> for User {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &SelfHostState) -> Result<Self, ApiError> {
        let caller = authenticate_user(st, &parts.headers).await?;
        if !role_allows(caller.role, &parts.method) {
            return Err(ApiError::forbidden(
                "forbidden",
                "The viewer role can only read.",
            ));
        }
        Ok(User(caller))
    }
}

/// An admin client (key administration).
pub struct Admin(pub Caller);

#[async_trait]
impl FromRequestParts<SelfHostState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &SelfHostState) -> Result<Self, ApiError> {
        let caller = authenticate_user(st, &parts.headers).await?;
        if caller.role != Role::Admin {
            return Err(ApiError::forbidden("forbidden", "This needs the admin role."));
        }
        Ok(Admin(caller))
    }
}

/// A worker holding an environment key. The handler must still check that
/// the environment in the path (or of the work item) is this one, with
/// [`EnvGrant::require_environment`].
#[derive(Debug, Clone)]
pub struct EnvGrant {
    pub environment_id: String,
    pub key_id: String,
}

impl EnvGrant {
    pub fn require_environment(&self, environment_id: &str) -> Result<(), ApiError> {
        if self.environment_id == environment_id {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "forbidden",
                "This environment key belongs to another environment.",
            ))
        }
    }
}

pub struct EnvKey(pub EnvGrant);

#[async_trait]
impl FromRequestParts<SelfHostState> for EnvKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &SelfHostState) -> Result<Self, ApiError> {
        let Some(token) = bearer(&parts.headers) else {
            return Err(ApiError::unauthorized("Send the environment key as a Bearer token."));
        };
        if !token.starts_with("aek_") {
            return Err(ApiError::forbidden(
                "forbidden",
                "Worker routes take an environment key (aek_...).",
            ));
        }
        let now = Utc::now();
        let key = st
            .store
            .find_environment_key_by_hash(&hash_secret(token))
            .await?
            .filter(|k| k.is_active(now))
            .ok_or_else(|| ApiError::unauthorized("The environment key is not valid."))?;
        if key
            .last_used_at
            .map_or(true, |t| now - t > Duration::seconds(TOUCH_EVERY_SECS))
        {
            let store = st.store.clone();
            let id = key.id.clone();
            tokio::spawn(async move {
                let _ = store.touch_environment_key(&id, now).await;
            });
        }
        Ok(EnvKey(EnvGrant {
            environment_id: key.environment_id,
            key_id: key.id,
        }))
    }
}

/// A worker holding the session secret of a live lease.
#[derive(Debug, Clone)]
pub struct SessionGrant {
    pub work_id: String,
    pub session_id: String,
    pub environment_id: String,
    pub lease_generation: i64,
}

impl SessionGrant {
    fn from_item(w: WorkItem) -> Self {
        Self {
            work_id: w.id,
            session_id: w.session_id,
            environment_id: w.environment_id,
            lease_generation: w.lease_generation,
        }
    }

    pub fn require_session(&self, session_id: &str) -> Result<(), ApiError> {
        if self.session_id == session_id {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "forbidden",
                "This session secret belongs to another session.",
            ))
        }
    }

    pub fn require_work(&self, work_id: &str) -> Result<(), ApiError> {
        if self.work_id == work_id {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "forbidden",
                "This session secret belongs to another work item.",
            ))
        }
    }
}

/// Resolve `X-Session-Secret`. The secret works only while its lease is live:
/// a new claim, a release, a completion or an expired lease ends it.
pub async fn session_grant(st: &SelfHostState, headers: &HeaderMap) -> Result<SessionGrant, ApiError> {
    let secret = headers
        .get(SESSION_SECRET_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::unauthorized("Send the session secret in X-Session-Secret."))?;
    let item = st
        .store
        .find_work_by_secret_hash(&hash_secret(secret), Utc::now())
        .await?
        .ok_or_else(|| ApiError::unauthorized("The session secret is not valid or its lease ended."))?;
    Ok(SessionGrant::from_item(item))
}

pub struct SessionSecret(pub SessionGrant);

#[async_trait]
impl FromRequestParts<SelfHostState> for SessionSecret {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &SelfHostState) -> Result<Self, ApiError> {
        Ok(SessionSecret(session_grant(st, &parts.headers).await?))
    }
}

/// The agent loop child, on `/internal/*`, from a loopback address, with the
/// per-boot internal token. Needs the server to be served with connect info.
pub struct Internal;

#[async_trait]
impl FromRequestParts<SelfHostState> for Internal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, st: &SelfHostState) -> Result<Self, ApiError> {
        if !parts.uri.path().starts_with("/internal/") {
            return Err(ApiError::forbidden("forbidden", "Not an internal route."));
        }
        let loopback = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .is_some_and(|ConnectInfo(addr)| addr.ip().is_loopback());
        if !loopback {
            return Err(ApiError::forbidden(
                "forbidden",
                "Internal routes only answer loopback callers.",
            ));
        }
        match bearer(&parts.headers) {
            Some(t) if constant_time_eq(t, &st.internal_token) => Ok(Internal),
            _ => Err(ApiError::unauthorized("The internal token is not valid.")),
        }
    }
}

// ─── OIDC ───────────────────────────────────────────────────────────────────

const JWKS_TTL: StdDuration = StdDuration::from_secs(600);
/// A token with an unknown `kid` forces a refresh at most this often.
const JWKS_MIN_REFRESH: StdDuration = StdDuration::from_secs(60);
const CLOCK_LEEWAY_SECS: i64 = 60;

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kty: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default, rename = "use")]
    use_: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwkSet {
    keys: Vec<Jwk>,
}

struct KeyCache {
    fetched_at: Instant,
    keys: Vec<Jwk>,
}

/// Validates id tokens from the configured issuer. Signing keys come
/// through the egress client, so the issuer must be on the allowlist (the
/// settings put it there).
pub struct OidcVerifier {
    cfg: OidcSettings,
    http: EgressClient,
    cache: RwLock<Option<KeyCache>>,
}

fn oidc_err(message: impl Into<String>) -> ApiError {
    ApiError::unauthorized(message)
}

fn b64(s: &str) -> Result<Vec<u8>, ApiError> {
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|_| oidc_err("The token is not valid."))
}

impl OidcVerifier {
    pub fn new(cfg: OidcSettings, http: EgressClient) -> Self {
        Self {
            cfg,
            http,
            cache: RwLock::new(None),
        }
    }

    pub async fn verify(&self, token: &str) -> Result<Caller, ApiError> {
        let mut parts = token.split('.');
        let (h, p, s) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(h), Some(p), Some(s), None) => (h, p, s),
            _ => return Err(oidc_err("The token is not valid.")),
        };
        let header: Value =
            serde_json::from_slice(&b64(h)?).map_err(|_| oidc_err("The token is not valid."))?;
        let alg = header.get("alg").and_then(Value::as_str).unwrap_or("");
        let kid = header.get("kid").and_then(Value::as_str);
        let sig = b64(s)?;
        let message = format!("{h}.{p}");

        let key = self.key_for(kid, alg).await?;
        verify_signature(alg, &key, message.as_bytes(), &sig)?;

        let claims: Value =
            serde_json::from_slice(&b64(p)?).map_err(|_| oidc_err("The token is not valid."))?;
        self.check_claims(&claims)?;
        let subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .ok_or_else(|| oidc_err("The token has no subject."))?
            .to_string();
        let role = self.role_from(&claims).ok_or_else(|| {
            ApiError::forbidden("forbidden", "Your account has no role on this server.")
        })?;
        Ok(Caller {
            subject: format!("oidc:{subject}"),
            role,
            kind: CallerKind::Oidc,
        })
    }

    fn check_claims(&self, c: &Value) -> Result<(), ApiError> {
        let now = Utc::now().timestamp();
        let exp = c
            .get("exp")
            .and_then(Value::as_i64)
            .ok_or_else(|| oidc_err("The token has no expiry."))?;
        if exp + CLOCK_LEEWAY_SECS < now {
            return Err(oidc_err("The token has expired."));
        }
        if let Some(nbf) = c.get("nbf").and_then(Value::as_i64) {
            if nbf - CLOCK_LEEWAY_SECS > now {
                return Err(oidc_err("The token is not valid yet."));
            }
        }
        let iss = c.get("iss").and_then(Value::as_str).unwrap_or("");
        if iss.trim_end_matches('/') != self.cfg.issuer.trim_end_matches('/') {
            return Err(oidc_err("The token is from another issuer."));
        }
        let aud_ok = match c.get("aud") {
            Some(Value::String(a)) => self.cfg.audiences.iter().any(|x| x == a),
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .any(|a| self.cfg.audiences.iter().any(|x| x == a)),
            _ => false,
        };
        if !aud_ok {
            return Err(oidc_err("The token is for another audience."));
        }
        Ok(())
    }

    /// The highest role any claim value maps to, else the default role.
    fn role_from(&self, c: &Value) -> Option<Role> {
        let values: Vec<&str> = match c.get(&self.cfg.role_claim) {
            Some(Value::String(s)) => vec![s.as_str()],
            Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        values
            .iter()
            .filter_map(|v| self.cfg.role_map.get(*v).copied())
            .max()
            .or_else(|| self.cfg.default_role.role())
    }

    async fn key_for(&self, kid: Option<&str>, alg: &str) -> Result<Jwk, ApiError> {
        let kty = match alg {
            "RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512" => "RSA",
            "ES256" | "ES384" => "EC",
            _ => return Err(oidc_err("The token uses an algorithm this server does not accept.")),
        };
        let pick = |keys: &[Jwk]| -> Option<Jwk> {
            let usable = keys
                .iter()
                .filter(|k| k.kty == kty && k.use_.as_deref().map_or(true, |u| u == "sig"));
            match kid {
                Some(kid) => usable.into_iter().find(|k| k.kid.as_deref() == Some(kid)).cloned(),
                None => {
                    let all: Vec<&Jwk> = usable.collect();
                    (all.len() == 1).then(|| all[0].clone())
                }
            }
        };
        {
            let cache = self.cache.read().await;
            if let Some(c) = cache.as_ref() {
                if c.fetched_at.elapsed() < JWKS_TTL {
                    if let Some(k) = pick(&c.keys) {
                        return Ok(k);
                    }
                }
            }
        }
        let mut cache = self.cache.write().await;
        // Expired (past the TTL) or holding no key for this token: refetch,
        // but not more than once per JWKS_MIN_REFRESH (another task may have
        // refetched while we waited for the lock).
        let stale = cache
            .as_ref()
            .map_or(true, |c| c.fetched_at.elapsed() >= JWKS_MIN_REFRESH);
        if stale {
            let keys = self.fetch_keys().await?;
            *cache = Some(KeyCache {
                fetched_at: Instant::now(),
                keys,
            });
        }
        cache
            .as_ref()
            .and_then(|c| pick(&c.keys))
            .ok_or_else(|| oidc_err("The token was signed with an unknown key."))
    }

    async fn jwks_uri(&self) -> Result<String, ApiError> {
        if let Some(u) = &self.cfg.jwks_uri {
            return Ok(u.clone());
        }
        let disco = format!(
            "{}/.well-known/openid-configuration",
            self.cfg.issuer.trim_end_matches('/')
        );
        let doc: Value = self.get_json(&disco).await?;
        let uri = doc
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::unavailable("oidc_unavailable", "The issuer has no jwks_uri."))?
            .to_string();
        // Discovery may only point at the issuer's own host. Another host
        // must be set as auth.oidc.jwks_uri, which also allowlists it.
        let same_host = url::Url::parse(&uri).ok().map(|u| (u.host_str().map(str::to_string), u.port_or_known_default()))
            == url::Url::parse(&self.cfg.issuer).ok().map(|u| (u.host_str().map(str::to_string), u.port_or_known_default()));
        if !same_host {
            return Err(ApiError::unavailable(
                "oidc_unavailable",
                "The issuer's jwks_uri is on another host. Set auth.oidc.jwks_uri in the settings.",
            ));
        }
        Ok(uri)
    }

    async fn fetch_keys(&self) -> Result<Vec<Jwk>, ApiError> {
        let uri = self.jwks_uri().await?;
        let set: JwkSet = self.get_json(&uri).await?;
        Ok(set.keys)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, ApiError> {
        let req = self.http.get(url).map_err(|e| {
            tracing::warn!(error = %e, "OIDC request refused by the egress guard");
            ApiError::unavailable("oidc_unavailable", "The identity provider is not reachable.")
        })?;
        let res = req.send().await.map_err(|e| {
            tracing::warn!(error = %e, "OIDC request failed");
            ApiError::unavailable("oidc_unavailable", "The identity provider is not reachable.")
        })?;
        if !res.status().is_success() {
            return Err(ApiError::unavailable(
                "oidc_unavailable",
                format!("The identity provider answered {}.", res.status().as_u16()),
            ));
        }
        res.json::<T>().await.map_err(|_| {
            ApiError::unavailable("oidc_unavailable", "The identity provider sent bad JSON.")
        })
    }
}

fn verify_signature(alg: &str, key: &Jwk, message: &[u8], sig: &[u8]) -> Result<(), ApiError> {
    use aws_lc_rs::signature as sigs;
    let bad = || oidc_err("The token signature is not valid.");
    match alg {
        "RS256" | "RS384" | "RS512" | "PS256" | "PS384" | "PS512" => {
            let n = b64(key.n.as_deref().ok_or_else(bad)?)?;
            let e = b64(key.e.as_deref().ok_or_else(bad)?)?;
            let params: &sigs::RsaParameters = match alg {
                "RS256" => &sigs::RSA_PKCS1_2048_8192_SHA256,
                "RS384" => &sigs::RSA_PKCS1_2048_8192_SHA384,
                "RS512" => &sigs::RSA_PKCS1_2048_8192_SHA512,
                "PS256" => &sigs::RSA_PSS_2048_8192_SHA256,
                "PS384" => &sigs::RSA_PSS_2048_8192_SHA384,
                _ => &sigs::RSA_PSS_2048_8192_SHA512,
            };
            sigs::RsaPublicKeyComponents { n: &n, e: &e }
                .verify(params, message, sig)
                .map_err(|_| bad())
        }
        "ES256" | "ES384" => {
            let (want_crv, verify_alg): (&str, &'static sigs::EcdsaVerificationAlgorithm) =
                if alg == "ES256" {
                    ("P-256", &sigs::ECDSA_P256_SHA256_FIXED)
                } else {
                    ("P-384", &sigs::ECDSA_P384_SHA384_FIXED)
                };
            if key.crv.as_deref() != Some(want_crv) {
                return Err(bad());
            }
            let x = b64(key.x.as_deref().ok_or_else(bad)?)?;
            let y = b64(key.y.as_deref().ok_or_else(bad)?)?;
            let mut point = Vec::with_capacity(1 + x.len() + y.len());
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            sigs::UnparsedPublicKey::new(verify_alg, point)
                .verify(message, sig)
                .map_err(|_| bad())
        }
        _ => Err(bad()),
    }
}

/// Keeps `Arc<OidcVerifier>` cheap to share in state.
pub type SharedOidc = Arc<OidcVerifier>;
