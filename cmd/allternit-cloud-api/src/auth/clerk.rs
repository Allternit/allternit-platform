//! Clerk verification for human-facing cloud control-plane actions.
//!
//! Clerk proves who approved a runtime. The runtime receives a separate device
//! credential; the Clerk JWT is never handed to or stored by the runtime.

use axum::http::{header, HeaderMap};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::ApiError;

/// Extracts `kid` from a JWT header without jsonwebtoken's Header
/// deserialization, which rejects Clerk's numeric `oiat` claim under v10.
fn jwt_header_kid(token: &str) -> Result<String, ApiError> {
    let part = token
        .split('.')
        .next()
        .ok_or_else(|| ApiError::Unauthorized("Invalid Clerk token".to_string()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| ApiError::Unauthorized("Invalid Clerk token".to_string()))?;
    let header: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::Unauthorized("Invalid Clerk token".to_string()))?;
    header
        .get("kid")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::Unauthorized("Clerk token has no key id".to_string()))
}

const DEFAULT_CLERK_ISSUER: &str = "https://clerk.allternit.com";
const DEFAULT_CLERK_JWKS_URL: &str = "https://clerk.allternit.com/.well-known/jwks.json";
const JWKS_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug)]
pub struct ClerkUser {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub image_url: Option<String>,
    pub organization_id: Option<String>,
    /// Role in the active Clerk organization (`admin`, `org:admin`,
    /// `org:member`, ...). Session token v2 carries it as `o.rol`, v1 as
    /// `org_role`. `None` outside an org or for non-Clerk callers.
    pub org_role: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

struct CachedKeys {
    fetched_at: Instant,
    keys: HashMap<String, Jwk>,
}

struct ClerkVerifier {
    client: reqwest::Client,
    cache: RwLock<Option<CachedKeys>>,
    issuers: Vec<String>,
    jwks_url: String,
}

impl ClerkVerifier {
    fn from_env() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("failed to create Clerk HTTP client"),
            cache: RwLock::new(None),
            // Comma-separated. Include the first-party proxy issuer
            // (https://allternit.com/__clerk) so browser session JWTs verify.
            issuers: {
                let raw = std::env::var("CLERK_ISSUER")
                    .unwrap_or_else(|_| DEFAULT_CLERK_ISSUER.to_string());
                let mut list: Vec<String> = raw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let proxy = "https://allternit.com/__clerk";
                if !list.iter().any(|s| s == proxy) {
                    list.push(proxy.to_string());
                }
                list
            },
            jwks_url: std::env::var("CLERK_JWKS_URL")
                .unwrap_or_else(|_| DEFAULT_CLERK_JWKS_URL.to_string()),
        }
    }

    async fn key(&self, kid: &str) -> Result<Jwk, ApiError> {
        {
            let cache = self.cache.read().await;
            if let Some(cache) = cache.as_ref() {
                if cache.fetched_at.elapsed() < JWKS_TTL {
                    if let Some(key) = cache.keys.get(kid) {
                        return Ok(key.clone());
                    }
                }
            }
        }

        let response = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|error| ApiError::Unauthorized(format!("Clerk JWKS unavailable: {error}")))?;
        if !response.status().is_success() {
            return Err(ApiError::Unauthorized("Clerk JWKS unavailable".to_string()));
        }
        let jwks = response
            .json::<JwksResponse>()
            .await
            .map_err(|error| ApiError::Unauthorized(format!("Invalid Clerk JWKS: {error}")))?;
        let keys = jwks
            .keys
            .into_iter()
            .map(|key| (key.kid.clone(), key))
            .collect::<HashMap<_, _>>();
        let key = keys
            .get(kid)
            .cloned()
            .ok_or_else(|| ApiError::Unauthorized("Clerk signing key not found".to_string()))?;
        *self.cache.write().await = Some(CachedKeys {
            fetched_at: Instant::now(),
            keys,
        });
        Ok(key)
    }

    async fn verify(&self, token: &str) -> Result<ClerkUser, ApiError> {
        // Parse the JWT header manually: jsonwebtoken v10's Header struct
        // flattens non-standard claims into `HashMap<String, String> extras`,
        // so any non-string custom claim (Clerk's numeric `oiat`) fails
        // decode_header AND decode() for every Clerk token. We therefore
        // verify RS256 + issuer + expiry manually instead.
        let kid = jwt_header_kid(token)?;
        let jwk = self.key(&kid).await?;
        let claims = verify_rs256(token, &jwk, &self.issuers)?;

        let organization_id = claims
            .get("o")
            .and_then(|o| o.get("id"))
            .and_then(|v| v.as_str())
            .or_else(|| claims.get("org_id").and_then(|v| v.as_str()))
            .map(str::to_owned);
        let org_role = claims
            .get("o")
            .and_then(|o| o.get("rol"))
            .and_then(|v| v.as_str())
            .or_else(|| claims.get("org_role").and_then(|v| v.as_str()))
            .map(str::to_owned);

        Ok(ClerkUser {
            id: claims
                .get("sub")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::Unauthorized("Clerk token has no subject".to_string()))?
                .to_string(),
            email: str_claim(&claims, "email").or_else(|| str_claim(&claims, "email_address")),
            name: str_claim(&claims, "name"),
            image_url: str_claim(&claims, "image_url"),
            organization_id,
            org_role,
        })
    }
}

fn str_claim(claims: &serde_json::Value, key: &str) -> Option<String> {
    claims.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

/// Verifies an RS256 JWT against a JWKS key and enforces issuer + expiry
/// (60s leeway), returning the raw claims. See `verify` for why jsonwebtoken
/// v10 cannot be used on this path.
fn verify_rs256(token: &str, jwk: &Jwk, issuers: &[String]) -> Result<serde_json::Value, ApiError> {
    let unauthorized = |msg: &str| ApiError::Unauthorized(msg.to_string());
    let mut parts = token.split('.');
    let (header_b64, payload_b64, signature_b64) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => (h, p, s),
        _ => return Err(unauthorized("Malformed Clerk token")),
    };
    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| unauthorized("Malformed Clerk signature"))?;
    let n = URL_SAFE_NO_PAD
        .decode(&jwk.n)
        .map_err(|_| unauthorized("Invalid Clerk signing key"))?;
    let e = URL_SAFE_NO_PAD
        .decode(&jwk.e)
        .map_err(|_| unauthorized("Invalid Clerk signing key"))?;
    let public_key = aws_lc_rs::signature::RsaPublicKeyComponents { n: &n, e: &e };
    public_key
        .verify(
            &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{header_b64}.{payload_b64}").as_bytes(),
            &signature,
        )
        .map_err(|_| unauthorized("Invalid Clerk signature"))?;

    let claims: serde_json::Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|_| unauthorized("Malformed Clerk payload"))?,
    )
    .map_err(|_| unauthorized("Malformed Clerk payload"))?;

    let iss = claims.get("iss").and_then(|v| v.as_str()).unwrap_or("");
    if !issuers.iter().any(|allowed| allowed == iss) {
        return Err(unauthorized("Invalid Clerk issuer"));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match claims.get("exp").and_then(|v| v.as_u64()) {
        Some(exp) if now <= exp + 60 => {}
        _ => return Err(unauthorized("Invalid or expired Clerk session")),
    }
    Ok(claims)
}

fn verifier() -> &'static Arc<ClerkVerifier> {
    static VERIFIER: OnceLock<Arc<ClerkVerifier>> = OnceLock::new();
    VERIFIER.get_or_init(|| Arc::new(ClerkVerifier::from_env()))
}

pub async fn user_from_headers(headers: &HeaderMap) -> Result<ClerkUser, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::Unauthorized("A Clerk session is required".to_string()))?;
    verifier().verify(token).await
}

/// Verify a Clerk JWT (signature, issuer, expiry) and return its raw claims,
/// for callers that check `aud` / `scope` themselves (the public MCP edge).
pub async fn verified_claims(token: &str) -> Result<serde_json::Value, ApiError> {
    let v = verifier();
    let kid = jwt_header_kid(token)?;
    let jwk = v.key(&kid).await?;
    verify_rs256(token, &jwk, &v.issuers)
}

/// Verify a raw Clerk session JWT (no header extraction). Used by the run
/// WebSocket, where the token may arrive via a query param or the
/// Sec-WebSocket-Protocol header rather than Authorization.
pub async fn user_from_token(token: &str) -> Result<ClerkUser, ApiError> {
    verifier().verify(token).await
}
