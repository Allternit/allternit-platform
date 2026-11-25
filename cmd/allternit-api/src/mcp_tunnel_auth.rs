//! MCP tunnel security — mTLS + OAuth validation helpers.
//!
//! Each tunnel policy has a `direction` (plan D3, Apps + MCP 2026):
//!
//! - `outbound`: an admin MCP tunnel to an externally hosted MCP server. The
//!   gateway (mTLS terminator) in front of `/mcp/server` asserts the
//!   client-certificate thumbprint and the OAuth issuer/audience in headers,
//!   and [`require_tunnel_auth`] compares them with the policy offline. Those
//!   headers are only believed when the request also carries the internal
//!   service token (`mcp_server_routes::handle_rpc` rejects them otherwise),
//!   so a policy that pins a certificate or issuer can only be satisfied
//!   through the gateway. A missing policy row stays fail-open for existing
//!   attachments.
//! - `inbound_share`: a share link that lets someone outside reach a local
//!   port or MCP server through the share gate. Nothing in the request is
//!   trusted as asserted: [`require_inbound_share_auth`] verifies the bearer
//!   JWT's RS256 signature against the issuer's JWKS (cached, refetched on an
//!   unknown `kid` at a bounded rate — `auth::JwksManager`), then checks
//!   `iss`, `aud`, `exp` and `nbf` from the verified claims. A missing policy
//!   row is fail-closed (plan D10: MCP shares always require auth).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::auth::{AuthConfig, JwksManager};
use crate::db::DbHandle;

/// Which way traffic crosses the tunnel (column `mcp_tunnel_auth.direction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelDirection {
    /// Admin tunnel to an external MCP server; header-asserted checks.
    Outbound,
    /// Share link into this machine; JWT signature verified against JWKS.
    InboundShare,
}

impl TunnelDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            TunnelDirection::Outbound => "outbound",
            TunnelDirection::InboundShare => "inbound_share",
        }
    }

    /// Parse the stored value. Anything unknown is treated as
    /// `inbound_share`, the stricter direction (fail closed).
    pub fn from_db(value: &str) -> Self {
        match value.trim() {
            "outbound" => TunnelDirection::Outbound,
            _ => TunnelDirection::InboundShare,
        }
    }
}

/// Per-tunnel security policy. At least one of `client_cert_pem` or
/// `oauth_issuer` should be set for an `outbound` policy to be meaningful; an
/// `inbound_share` policy always requires a verified token.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTunnelAuth {
    pub tunnel_id: String,
    pub client_cert_pem: Option<String>,
    pub oauth_issuer: Option<String>,
    pub audience: Option<String>,
    pub direction: TunnelDirection,
    /// JWKS endpoint for `inbound_share` token verification. None =
    /// `<oauth_issuer>/.well-known/jwks.json`.
    pub jwks_url: Option<String>,
}

impl McpTunnelAuth {
    /// True when the policy requires a client-certificate thumbprint check.
    pub fn requires_mtls(&self) -> bool {
        self.client_cert_pem.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
    }

    /// True when the policy requires an OAuth issuer (and optionally audience)
    /// check. Always true for `inbound_share`.
    pub fn requires_oauth(&self) -> bool {
        self.direction == TunnelDirection::InboundShare
            || self.oauth_issuer.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false)
    }
}

/// Load the tunnel auth policy for `tunnel_id`, if any.
pub fn load_tunnel_auth(
    db: &DbHandle,
    tunnel_id: &str,
) -> Result<Option<McpTunnelAuth>, rusqlite::Error> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(
        "SELECT tunnel_id, client_cert_pem, oauth_issuer, audience, direction, jwks_url
         FROM mcp_tunnel_auth WHERE tunnel_id = ?1",
    )?;
    let row = stmt
        .query_row(params![tunnel_id], |row| {
            let direction: Option<String> = row.get(4)?;
            Ok(McpTunnelAuth {
                tunnel_id: row.get(0)?,
                client_cert_pem: row.get(1)?,
                oauth_issuer: row.get(2)?,
                audience: row.get(3)?,
                direction: TunnelDirection::from_db(direction.as_deref().unwrap_or("outbound")),
                jwks_url: row.get(5)?,
            })
        })
        .optional()?;
    Ok(row)
}

/// Compute a SHA-256 hex thumbprint of the first certificate found in a PEM
/// bundle. The thumbprint is the hex-encoded SHA-256 digest of the certificate
/// DER bytes (uppercase, colon-free), matching `openssl x509 -sha256 -fingerprint`.
///
/// Errors are returned as strings for easy use from route handlers.
pub fn client_cert_thumbprint(pem: &str) -> Result<String, String> {
    let mut cursor = std::io::Cursor::new(pem.trim().as_bytes());
    let certs = rustls_pemfile::certs(&mut cursor)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to parse client certificate PEM: {e}"))?;

    let cert = certs
        .into_iter()
        .next()
        .ok_or_else(|| "No certificate found in PEM".to_string())?;

    let mut hasher = Sha256::new();
    hasher.update(&cert);
    Ok(hex::encode(hasher.finalize()).to_uppercase())
}

/// Normalize an issuer string for comparison. Trims whitespace and strips a
/// trailing slash so `https://issuer.example` and `https://issuer.example/`
/// compare equal.
fn normalize_issuer(issuer: &str) -> String {
    issuer.trim().trim_end_matches('/').to_lowercase()
}

/// Validate that `token_issuer` matches the configured `allowed_issuer`.
/// Returns true when both normalized values are equal.
pub fn validate_oauth_issuer(token_issuer: &str, allowed_issuer: &str) -> bool {
    normalize_issuer(token_issuer) == normalize_issuer(allowed_issuer)
}

/// Validate that `token_audience` matches the configured `audience`, if an
/// audience is configured. An unset audience means any audience is accepted.
pub fn validate_oauth_audience(token_audience: Option<&str>, allowed_audience: Option<&str>) -> bool {
    match allowed_audience {
        None => true,
        Some(allowed) if allowed.trim().is_empty() => true,
        Some(allowed) => token_audience.map(|a| a.trim()) == Some(allowed.trim()),
    }
}

/// Result of validating a request against a tunnel auth policy.
#[derive(Debug, Clone, PartialEq)]
pub enum TunnelAuthResult {
    Allowed,
    MissingCert,
    CertMismatch,
    MissingIssuer,
    IssuerMismatch,
    AudienceMismatch,
    /// `inbound_share`: no bearer token on the request.
    MissingToken,
    /// `inbound_share`: the token failed signature, lifetime or format checks.
    InvalidToken,
    /// The policy's direction does not match the path that checked it.
    DirectionMismatch,
    /// `inbound_share`: no policy row for the tunnel (fail closed).
    MissingPolicy,
    /// `inbound_share`: the policy's JWKS URL is not https (or loopback http).
    JwksUrlInvalid,
}

impl TunnelAuthResult {
    /// Human-readable reason for logging or error responses.
    pub fn reason(&self) -> &'static str {
        match self {
            TunnelAuthResult::Allowed => "allowed",
            TunnelAuthResult::MissingCert => "missing client certificate",
            TunnelAuthResult::CertMismatch => "client certificate thumbprint mismatch",
            TunnelAuthResult::MissingIssuer => "missing OAuth issuer",
            TunnelAuthResult::IssuerMismatch => "OAuth issuer mismatch",
            TunnelAuthResult::AudienceMismatch => "OAuth audience mismatch",
            TunnelAuthResult::MissingToken => "missing bearer token",
            TunnelAuthResult::InvalidToken => "invalid bearer token",
            TunnelAuthResult::DirectionMismatch => "tunnel policy direction mismatch",
            TunnelAuthResult::MissingPolicy => "no inbound share policy",
            TunnelAuthResult::JwksUrlInvalid => "tunnel JWKS URL must be https",
        }
    }

    pub fn is_allowed(&self) -> bool {
        matches!(self, TunnelAuthResult::Allowed)
    }
}

/// Validate an incoming MCP tunnel request against the stored policy.
///
/// - `client_thumbprint`: SHA-256 thumbprint presented by the mTLS terminator
///   (or extracted from a PEM in a header). None when the request did not
///   present a client certificate.
/// - `token_issuer`: `iss` claim from the OAuth bearer token, if available.
/// - `token_audience`: `aud` claim from the OAuth bearer token, if available.
///
/// The function is fail-closed: any configured check that is not satisfied
/// returns the corresponding failure variant. An `inbound_share` policy is
/// never satisfied here (`DirectionMismatch`): its issuer and audience must
/// come from a verified token, not from headers — use
/// [`require_inbound_share_auth`].
pub fn validate_tunnel_request(
    auth: &McpTunnelAuth,
    client_thumbprint: Option<&str>,
    token_issuer: Option<&str>,
    token_audience: Option<&str>,
) -> TunnelAuthResult {
    if auth.direction != TunnelDirection::Outbound {
        return TunnelAuthResult::DirectionMismatch;
    }
    if let Some(failure) = check_mtls(auth, client_thumbprint) {
        return failure;
    }

    if auth.requires_oauth() {
        let allowed_issuer = match auth.oauth_issuer.as_deref() {
            Some(issuer) if !issuer.trim().is_empty() => issuer,
            _ => return TunnelAuthResult::MissingIssuer,
        };
        match token_issuer {
            None => return TunnelAuthResult::MissingIssuer,
            Some(issuer) if !validate_oauth_issuer(issuer, allowed_issuer) => {
                return TunnelAuthResult::IssuerMismatch;
            }
            _ => {}
        }
        if !validate_oauth_audience(token_audience, auth.audience.as_deref()) {
            return TunnelAuthResult::AudienceMismatch;
        }
    }

    TunnelAuthResult::Allowed
}

/// Convenience helper used by route handlers: load the tunnel auth policy and,
/// if one exists, validate the request. Returns `Ok(None)` when no policy is
/// configured (fail-open for backward compatibility) and `Err(result)` when a
/// policy is configured but the request fails validation.
pub fn require_tunnel_auth(
    db: &DbHandle,
    tunnel_id: Option<&str>,
    client_thumbprint: Option<&str>,
    token_issuer: Option<&str>,
    token_audience: Option<&str>,
) -> Result<Option<McpTunnelAuth>, TunnelAuthResult> {
    let tunnel_id = match tunnel_id {
        Some(id) if !id.trim().is_empty() => id,
        _ => return Ok(None),
    };

    let auth = match load_tunnel_auth(db, tunnel_id) {
        Ok(Some(auth)) => auth,
        Ok(None) => return Ok(None),
        Err(_) => return Err(TunnelAuthResult::MissingCert),
    };

    let result = validate_tunnel_request(&auth, client_thumbprint, token_issuer, token_audience);
    if result.is_allowed() {
        Ok(Some(auth))
    } else {
        Err(result)
    }
}

/// The mTLS half of a policy check: `None` when the policy needs no client
/// certificate or the presented thumbprint matches.
fn check_mtls(auth: &McpTunnelAuth, client_thumbprint: Option<&str>) -> Option<TunnelAuthResult> {
    if !auth.requires_mtls() {
        return None;
    }
    let expected = match auth.client_cert_pem.as_deref() {
        Some(pem) => match client_cert_thumbprint(pem) {
            Ok(thp) => thp,
            Err(_) => return Some(TunnelAuthResult::CertMismatch),
        },
        None => return Some(TunnelAuthResult::MissingCert),
    };
    match client_thumbprint {
        None => Some(TunnelAuthResult::MissingCert),
        Some(thp) if thp.trim().to_uppercase() != expected => Some(TunnelAuthResult::CertMismatch),
        _ => None,
    }
}

// ─── inbound_share: verified bearer JWT ─────────────────────────────────────

/// Clock skew allowed on `nbf`, matching the 60 s `exp` leeway in `auth`.
const NBF_LEEWAY_SECS: u64 = 60;
/// Distinct JWKS endpoints kept in [`tunnel_jwks`]; past this the map is
/// cleared (a handful of issuers is the real-world count).
const MAX_TUNNEL_JWKS: usize = 64;

/// The JWKS endpoint for an `inbound_share` policy: the explicit `jwks_url`,
/// else `<oauth_issuer>/.well-known/jwks.json`. Only https, or http to a
/// loopback host (local issuers and tests), is accepted.
pub fn inbound_jwks_url(auth: &McpTunnelAuth) -> Result<String, TunnelAuthResult> {
    let url = match auth.jwks_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(url) => url.to_string(),
        None => {
            let issuer = auth
                .oauth_issuer
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or(TunnelAuthResult::MissingIssuer)?;
            format!("{}/.well-known/jwks.json", issuer.trim_end_matches('/'))
        }
    };
    let parsed = url::Url::parse(&url).map_err(|_| TunnelAuthResult::JwksUrlInvalid)?;
    let loopback = match parsed.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match parsed.scheme() {
        "https" => Ok(url),
        "http" if loopback => Ok(url),
        _ => Err(TunnelAuthResult::JwksUrlInvalid),
    }
}

/// Process-wide JWKS cache, one [`JwksManager`] per endpoint, so keys are
/// fetched once per TTL (and on rotation) rather than per request.
pub fn tunnel_jwks(jwks_url: &str) -> Arc<JwksManager> {
    static MANAGERS: OnceLock<Mutex<HashMap<String, Arc<JwksManager>>>> = OnceLock::new();
    let mut managers = MANAGERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(manager) = managers.get(jwks_url) {
        return manager.clone();
    }
    if managers.len() >= MAX_TUNNEL_JWKS {
        managers.clear();
    }
    let config = AuthConfig { clerk_jwks_url: jwks_url.to_string(), clerk_issuer: String::new() };
    let manager = Arc::new(JwksManager::new(&config));
    managers.insert(jwks_url.to_string(), manager.clone());
    manager
}

/// True when the `aud` claim (a string or an array of strings) contains the
/// policy audience. An unset policy audience accepts any token audience.
fn claims_audience_ok(claims: &serde_json::Value, allowed: Option<&str>) -> bool {
    let allowed = match allowed.map(str::trim).filter(|a| !a.is_empty()) {
        None => return true,
        Some(a) => a,
    };
    match claims.get("aud") {
        Some(serde_json::Value::String(aud)) => validate_oauth_audience(Some(aud), Some(allowed)),
        Some(serde_json::Value::Array(auds)) => auds
            .iter()
            .filter_map(|a| a.as_str())
            .any(|aud| validate_oauth_audience(Some(aud), Some(allowed))),
        _ => false,
    }
}

/// Verify an `inbound_share` request against `auth` with keys from `jwks`.
///
/// `bearer` is the raw token or the whole `Authorization` header value.
/// Returns the verified claims. Checks, in order: direction, mTLS (when the
/// policy pins a certificate), token present, RS256 signature by the JWKS key
/// named in the header `kid`, `iss` == policy issuer, `exp` (60 s leeway),
/// `nbf` (60 s leeway), `aud` contains the policy audience when one is set.
pub async fn verify_inbound_bearer(
    auth: &McpTunnelAuth,
    jwks: &JwksManager,
    client_thumbprint: Option<&str>,
    bearer: Option<&str>,
) -> Result<serde_json::Value, TunnelAuthResult> {
    if auth.direction != TunnelDirection::InboundShare {
        return Err(TunnelAuthResult::DirectionMismatch);
    }
    if let Some(failure) = check_mtls(auth, client_thumbprint) {
        return Err(failure);
    }
    let allowed_issuer = auth
        .oauth_issuer
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(TunnelAuthResult::MissingIssuer)?;
    let token = bearer
        .map(str::trim_start)
        .map(|t| match t.get(..7) {
            Some(prefix) if prefix.eq_ignore_ascii_case("bearer ") => t[7..].trim(),
            _ => t.trim(),
        })
        .filter(|t| !t.is_empty())
        .ok_or(TunnelAuthResult::MissingToken)?;

    let claims = crate::auth::verify_rs256_with_jwks(jwks, token, |iss| {
        validate_oauth_issuer(iss, allowed_issuer)
    })
    .await
    .map_err(|e| {
        warn!(tunnel_id = %auth.tunnel_id, error = %e, "inbound share token rejected");
        TunnelAuthResult::InvalidToken
    })?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Some(nbf) = claims.get("nbf") {
        match nbf.as_u64() {
            Some(nbf) if nbf <= now + NBF_LEEWAY_SECS => {}
            _ => return Err(TunnelAuthResult::InvalidToken),
        }
    }
    if !claims_audience_ok(&claims, auth.audience.as_deref()) {
        return Err(TunnelAuthResult::AudienceMismatch);
    }
    Ok(claims)
}

/// Gate for a request arriving through a share link (`inbound_share`).
///
/// Fail-closed throughout: no policy row, a policy of the other direction, a
/// bad JWKS URL or any failed token check is an `Err`. On success returns the
/// policy and the verified token claims (`sub` = who is calling).
pub async fn require_inbound_share_auth(
    db: &DbHandle,
    tunnel_id: &str,
    client_thumbprint: Option<&str>,
    bearer: Option<&str>,
) -> Result<(McpTunnelAuth, serde_json::Value), TunnelAuthResult> {
    let tunnel_id = tunnel_id.trim();
    if tunnel_id.is_empty() {
        return Err(TunnelAuthResult::MissingPolicy);
    }
    let auth = match load_tunnel_auth(db, tunnel_id) {
        Ok(Some(auth)) => auth,
        Ok(None) => return Err(TunnelAuthResult::MissingPolicy),
        Err(e) => {
            warn!(tunnel_id = %tunnel_id, error = %e, "inbound share policy load failed");
            return Err(TunnelAuthResult::MissingPolicy);
        }
    };
    if auth.direction != TunnelDirection::InboundShare {
        return Err(TunnelAuthResult::DirectionMismatch);
    }
    let jwks = tunnel_jwks(&inbound_jwks_url(&auth)?);
    let claims = verify_inbound_bearer(&auth, &jwks, client_thumbprint, bearer).await?;
    Ok((auth, claims))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::DbHandle;
    use std::path::PathBuf;

    fn temp_db() -> (DbHandle, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "allternit_mcp_tunnel_auth_test_{}_{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        // Remove any leftover file so each test starts fresh; migrations run on open.
        let _ = std::fs::remove_file(&path);
        (DbHandle::new(path.clone()).expect("open temp db"), path)
    }

    // Self-signed test certificate generated with:
    //   openssl req -x509 -newkey rsa:2048 -keyout /tmp/key.pem -out /tmp/cert.pem \
    //     -days 1 -nodes -subj "/CN=test-client"
    // SHA-256 fingerprint (colon-free uppercase):
    //   A4A616FD19584FE679C1C4E5AFC8363F1552ED0002864FAB14795A33AA5BB9A6
    pub(crate) const TEST_CERT_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIICqDCCAZACCQCPLNNrnTDMTDANBgkqhkiG9w0BAQsFADAWMRQwEgYDVQQDDAt0
ZXN0LWNsaWVudDAeFw0yNjA4MDkwODQ5NDZaFw0yNjA4MTAwODQ5NDZaMBYxFDAS
BgNVBAMMC3Rlc3QtY2xpZW50MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKC
AQEAyyd6Exp3JVV8V8u0DmOWqgf/Gi2Tx0VqeqEya//ezX8ge5AgzKAjSUZSSuhL
BVVad8A3B9bceivLKoPtEvlnfwIbRZIern515d4/8Npy5VMIlMAbhbYApK0+xY6k
2QiKBGdTstvCJWV2i/CQPnFAeeAoUWK7zEU72VltFcU0JvhASuTiKxoCvwtVmceo
F1wKG8q9EvsndHpp8WOHR3Jdy/rB11G3gyMfthx9UXxkAdakX0CTrcqnHaVMfyPI
KWVYdCN0R3n0z2yMjrwgbnuyaxPECxc0NicBT1y7x7fE7rwvSmbyhsIOlomtKcC3
IY5HjS+YBCgnD/bLEK9/kqZi7QIDAQABMA0GCSqGSIb3DQEBCwUAA4IBAQDKYzC1
8VPsf27oyyPu41Z2o9Xk7vfcXJ/pbW7jm/MOSoNk8APyi/ApxuOeKWzqzOO4xt4W
q2m5otrW54Irti0op4Zt0h1UY37uzJWaiSewphkEBLyEoBabOErHAgJmWLw3EjqJ
Sk7Gv8Bpp6OJeTYKhsyWUjLhFsKgB3szc9ahOjstEwfm6d/DsHGHrMPoIss6hUKk
KL7kGgy676Hiru/FVAE/q/QEyB9aoNbLg38CLDdhFtXlOzgI5QtvQltaievgnQnD
E+1ZWU7/2RqF1IwGwnd691/5x1cumBkc9TCLfYQMwXvij+3p9mawj63UROZCblR6
CkUlufXsZ1DNpNFr
-----END CERTIFICATE-----
"#;

    #[test]
    fn client_cert_thumbprint_matches_openssl() {
        let thumbprint = client_cert_thumbprint(TEST_CERT_PEM).unwrap();
        assert_eq!(
            thumbprint,
            "A4A616FD19584FE679C1C4E5AFC8363F1552ED0002864FAB14795A33AA5BB9A6"
        );
    }

    #[test]
    fn client_cert_thumbprint_rejects_empty_pem() {
        assert!(client_cert_thumbprint("").is_err());
        assert!(client_cert_thumbprint("not a pem").is_err());
    }

    #[test]
    fn validate_oauth_issuer_normalizes_case_and_trailing_slash() {
        assert!(validate_oauth_issuer(
            "https://issuer.example",
            "https://issuer.example/"
        ));
        assert!(validate_oauth_issuer(
            "https://Issuer.Example/",
            "https://issuer.example"
        ));
        assert!(!validate_oauth_issuer(
            "https://other.example",
            "https://issuer.example"
        ));
    }

    #[test]
    fn validate_oauth_audience_behavior() {
        assert!(validate_oauth_audience(Some("aud1"), Some("aud1")));
        assert!(!validate_oauth_audience(Some("aud2"), Some("aud1")));
        assert!(validate_oauth_audience(None, None));
        assert!(validate_oauth_audience(Some("anything"), None));
        assert!(validate_oauth_audience(Some("anything"), Some("")));
    }

    #[test]
    fn validate_tunnel_request_allows_when_no_policy() {
        let auth = McpTunnelAuth {
            tunnel_id: "t1".into(),
            client_cert_pem: None,
            oauth_issuer: None,
            audience: None,
            direction: TunnelDirection::Outbound,
            jwks_url: None,
        };
        assert_eq!(
            validate_tunnel_request(&auth, None, None, None),
            TunnelAuthResult::Allowed
        );
    }

    #[test]
    fn validate_tunnel_request_enforces_mtls() {
        let expected = client_cert_thumbprint(TEST_CERT_PEM).unwrap();
        let auth = McpTunnelAuth {
            tunnel_id: "t1".into(),
            client_cert_pem: Some(TEST_CERT_PEM.into()),
            oauth_issuer: None,
            audience: None,
            direction: TunnelDirection::Outbound,
            jwks_url: None,
        };
        assert_eq!(
            validate_tunnel_request(&auth, Some(&expected), None, None),
            TunnelAuthResult::Allowed
        );
        assert_eq!(
            validate_tunnel_request(&auth, None, None, None),
            TunnelAuthResult::MissingCert
        );
        assert_eq!(
            validate_tunnel_request(&auth, Some("DEADBEEF"), None, None),
            TunnelAuthResult::CertMismatch
        );
    }

    #[test]
    fn validate_tunnel_request_enforces_oauth_issuer_and_audience() {
        let auth = McpTunnelAuth {
            tunnel_id: "t1".into(),
            client_cert_pem: None,
            oauth_issuer: Some("https://issuer.example/".into()),
            audience: Some("allternit-mcp".into()),
            direction: TunnelDirection::Outbound,
            jwks_url: None,
        };
        assert_eq!(
            validate_tunnel_request(
                &auth,
                None,
                Some("https://issuer.example"),
                Some("allternit-mcp")
            ),
            TunnelAuthResult::Allowed
        );
        assert_eq!(
            validate_tunnel_request(&auth, None, None, Some("allternit-mcp")),
            TunnelAuthResult::MissingIssuer
        );
        assert_eq!(
            validate_tunnel_request(
                &auth,
                None,
                Some("https://other.example"),
                Some("allternit-mcp")
            ),
            TunnelAuthResult::IssuerMismatch
        );
        assert_eq!(
            validate_tunnel_request(
                &auth,
                None,
                Some("https://issuer.example"),
                Some("wrong-aud")
            ),
            TunnelAuthResult::AudienceMismatch
        );
    }

    #[test]
    fn validate_tunnel_request_requires_all_configured_checks() {
        let expected = client_cert_thumbprint(TEST_CERT_PEM).unwrap();
        let auth = McpTunnelAuth {
            tunnel_id: "t1".into(),
            client_cert_pem: Some(TEST_CERT_PEM.into()),
            oauth_issuer: Some("https://issuer.example".into()),
            audience: Some("allternit-mcp".into()),
            direction: TunnelDirection::Outbound,
            jwks_url: None,
        };
        // Missing cert short-circuits before OAuth checks.
        assert_eq!(
            validate_tunnel_request(&auth, None, Some("https://issuer.example"), None),
            TunnelAuthResult::MissingCert
        );
        // Cert good but issuer bad.
        assert_eq!(
            validate_tunnel_request(&auth, Some(&expected), Some("https://other.example"), None),
            TunnelAuthResult::IssuerMismatch
        );
        // All good.
        assert_eq!(
            validate_tunnel_request(
                &auth,
                Some(&expected),
                Some("https://issuer.example"),
                Some("allternit-mcp")
            ),
            TunnelAuthResult::Allowed
        );
    }

    #[test]
    fn mcp_tunnel_auth_requires_helpers() {
        let empty_cert = McpTunnelAuth {
            tunnel_id: "t1".into(),
            client_cert_pem: Some("   ".into()),
            oauth_issuer: Some("".into()),
            audience: None,
            direction: TunnelDirection::Outbound,
            jwks_url: None,
        };
        assert!(!empty_cert.requires_mtls());
        assert!(!empty_cert.requires_oauth());

        let real = McpTunnelAuth {
            tunnel_id: "t2".into(),
            client_cert_pem: Some(TEST_CERT_PEM.into()),
            oauth_issuer: Some("https://issuer.example".into()),
            audience: Some("aud".into()),
            direction: TunnelDirection::Outbound,
            jwks_url: None,
        };
        assert!(real.requires_mtls());
        assert!(real.requires_oauth());
    }

    #[test]
    fn load_tunnel_auth_returns_none_for_unknown_tunnel() {
        let (db, _path) = temp_db();
        assert!(load_tunnel_auth(&db, "no-such-tunnel").unwrap().is_none());
    }

    #[test]
    fn load_tunnel_auth_reads_stored_policy() {
        let (db, _path) = temp_db();
        let expected_thumbprint = client_cert_thumbprint(TEST_CERT_PEM).unwrap();

        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO mcp_tunnel_auth (tunnel_id, client_cert_pem, oauth_issuer, audience)
             VALUES (?1, ?2, ?3, ?4)",
            params!["tunnel-1", TEST_CERT_PEM, "https://issuer.example/", "allternit-mcp"],
        )
        .unwrap();
        drop(conn);

        let auth = load_tunnel_auth(&db, "tunnel-1").unwrap().expect("policy exists");
        assert_eq!(auth.tunnel_id, "tunnel-1");
        assert!(auth.requires_mtls());
        assert!(auth.requires_oauth());
        assert_eq!(auth.audience.as_deref(), Some("allternit-mcp"));

        // Full request validation succeeds with the right thumbprint, issuer, and audience.
        let result = require_tunnel_auth(
            &db,
            Some("tunnel-1"),
            Some(&expected_thumbprint),
            Some("https://issuer.example"),
            Some("allternit-mcp"),
        );
        assert!(result.is_ok());

        // Wrong thumbprint fails.
        let result = require_tunnel_auth(
            &db,
            Some("tunnel-1"),
            Some("BAD"),
            Some("https://issuer.example"),
            Some("allternit-mcp"),
        );
        assert_eq!(result.unwrap_err(), TunnelAuthResult::CertMismatch);

        // No tunnel header at all bypasses the check (backward compatibility).
        let result = require_tunnel_auth(&db, None, None, None, None);
        assert!(result.is_ok());
    }

    // ─── inbound_share (D3) + JWT signature check ──────────────────────────

    use crate::auth::{test_rsa_jwk, test_rsa_key_pair, test_seed_rsa_key, test_sign_rs256};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const ISSUER: &str = "https://issuer.example";

    fn inbound_policy(audience: Option<&str>, jwks_url: Option<&str>) -> McpTunnelAuth {
        McpTunnelAuth {
            tunnel_id: "share-1".into(),
            client_cert_pem: None,
            oauth_issuer: Some(format!("{ISSUER}/")),
            audience: audience.map(Into::into),
            direction: TunnelDirection::InboundShare,
            jwks_url: jwks_url.map(Into::into),
        }
    }

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn claims(iss: &str, aud: serde_json::Value, exp_in: i64) -> serde_json::Value {
        json!({"iss": iss, "sub": "user_recipient", "aud": aud, "iat": now(), "exp": now() + exp_in})
    }

    /// A manager whose cache is filled by `test_seed_rsa_key`; its URL is never fetched.
    fn offline_jwks() -> JwksManager {
        JwksManager::new(&AuthConfig {
            clerk_jwks_url: "http://127.0.0.1:9/unused".into(),
            clerk_issuer: String::new(),
        })
    }

    /// Serves `{"keys": [...]}` from `keys` on 127.0.0.1 and counts fetches.
    async fn jwks_server(
        keys: Arc<Mutex<Vec<serde_json::Value>>>,
        hits: Arc<AtomicUsize>,
    ) -> String {
        use axum::{routing::get, Json, Router};
        let app = Router::new().route(
            "/jwks",
            get(move || {
                let keys = keys.clone();
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    let keys = keys.lock().unwrap().clone();
                    Json(json!({ "keys": keys }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/jwks")
    }

    #[tokio::test]
    async fn inbound_share_valid_token_passes() {
        let jwks = offline_jwks();
        let (kid, key) = test_seed_rsa_key(&jwks).await;
        let token = test_sign_rs256(&key, &kid, &claims(ISSUER, json!("allternit-share"), 300));
        let auth = inbound_policy(Some("allternit-share"), None);

        let verified = verify_inbound_bearer(&auth, &jwks, None, Some(&token)).await.expect("valid token");
        assert_eq!(verified["sub"], "user_recipient");
        // The whole Authorization header value is accepted too.
        let header = format!("Bearer {token}");
        assert!(verify_inbound_bearer(&auth, &jwks, None, Some(&header)).await.is_ok());
        // `aud` as an array containing the policy audience.
        let token = test_sign_rs256(&key, &kid, &claims(ISSUER, json!(["other", "allternit-share"]), 300));
        assert!(verify_inbound_bearer(&auth, &jwks, None, Some(&token)).await.is_ok());
    }

    #[tokio::test]
    async fn inbound_share_forged_token_with_right_issuer_is_rejected() {
        let jwks = offline_jwks();
        let (kid, _real_key) = test_seed_rsa_key(&jwks).await;
        // Right issuer, right audience, the real key's kid, but signed by another key.
        let attacker_key = test_rsa_key_pair();
        let forged = test_sign_rs256(&attacker_key, &kid, &claims(ISSUER, json!("allternit-share"), 300));
        let auth = inbound_policy(Some("allternit-share"), None);

        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, Some(&forged)).await.unwrap_err(),
            TunnelAuthResult::InvalidToken
        );
        // An unsigned (alg none) token with the same claims is rejected as well.
        let parts: Vec<&str> = forged.split('.').collect();
        let unsigned = format!("{}.{}.", parts[0], parts[1]);
        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, Some(&unsigned)).await.unwrap_err(),
            TunnelAuthResult::InvalidToken
        );
    }

    #[tokio::test]
    async fn inbound_share_rejects_wrong_issuer_audience_expiry_and_missing_token() {
        let jwks = offline_jwks();
        let (kid, key) = test_seed_rsa_key(&jwks).await;
        let auth = inbound_policy(Some("allternit-share"), None);
        let sign = |c: serde_json::Value| test_sign_rs256(&key, &kid, &c);

        let wrong_iss = sign(claims("https://other.example", json!("allternit-share"), 300));
        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, Some(&wrong_iss)).await.unwrap_err(),
            TunnelAuthResult::InvalidToken
        );
        let wrong_aud = sign(claims(ISSUER, json!("someone-else"), 300));
        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, Some(&wrong_aud)).await.unwrap_err(),
            TunnelAuthResult::AudienceMismatch
        );
        let expired = sign(claims(ISSUER, json!("allternit-share"), -3600));
        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, Some(&expired)).await.unwrap_err(),
            TunnelAuthResult::InvalidToken
        );
        let mut early = claims(ISSUER, json!("allternit-share"), 3600);
        early["nbf"] = json!(now() + 1800);
        let not_yet = sign(early);
        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, Some(&not_yet)).await.unwrap_err(),
            TunnelAuthResult::InvalidToken
        );
        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, None).await.unwrap_err(),
            TunnelAuthResult::MissingToken
        );
        assert_eq!(
            verify_inbound_bearer(&auth, &jwks, None, Some("Bearer ")).await.unwrap_err(),
            TunnelAuthResult::MissingToken
        );
    }

    #[test]
    fn inbound_share_policy_never_passes_on_header_asserted_claims() {
        // Headers claiming the right issuer and audience must not satisfy an
        // inbound_share policy: only a verified token can.
        let auth = inbound_policy(Some("allternit-share"), None);
        assert_eq!(
            validate_tunnel_request(&auth, None, Some(ISSUER), Some("allternit-share")),
            TunnelAuthResult::DirectionMismatch
        );
    }

    #[test]
    fn inbound_jwks_url_derives_from_issuer_and_requires_https() {
        let auth = inbound_policy(None, None);
        assert_eq!(inbound_jwks_url(&auth).unwrap(), "https://issuer.example/.well-known/jwks.json");
        let auth = inbound_policy(None, Some("https://keys.example/jwks"));
        assert_eq!(inbound_jwks_url(&auth).unwrap(), "https://keys.example/jwks");
        let auth = inbound_policy(None, Some("http://127.0.0.1:4000/jwks"));
        assert!(inbound_jwks_url(&auth).is_ok());
        let auth = inbound_policy(None, Some("http://keys.example/jwks"));
        assert_eq!(inbound_jwks_url(&auth).unwrap_err(), TunnelAuthResult::JwksUrlInvalid);
        let mut auth = inbound_policy(None, None);
        auth.oauth_issuer = None;
        assert_eq!(inbound_jwks_url(&auth).unwrap_err(), TunnelAuthResult::MissingIssuer);
    }

    #[tokio::test]
    async fn inbound_share_jwks_refetches_on_rotation_at_a_bounded_rate() {
        let key_a = test_rsa_key_pair();
        let key_b = test_rsa_key_pair();
        let keys = Arc::new(Mutex::new(vec![test_rsa_jwk(&key_a, "kid-a")]));
        let hits = Arc::new(AtomicUsize::new(0));
        let url = jwks_server(keys.clone(), hits.clone()).await;
        let jwks = JwksManager::new(&AuthConfig { clerk_jwks_url: url.clone(), clerk_issuer: String::new() })
            .with_min_refetch_interval(std::time::Duration::from_millis(300));
        let auth = inbound_policy(None, Some(url.as_str()));
        let token_a = test_sign_rs256(&key_a, "kid-a", &claims(ISSUER, json!("x"), 300));
        let token_b = test_sign_rs256(&key_b, "kid-b", &claims(ISSUER, json!("x"), 300));

        assert!(verify_inbound_bearer(&auth, &jwks, None, Some(&token_a)).await.is_ok());
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // Unknown kids right after a fetch do not refetch (bounded rate).
        for i in 0..3 {
            let junk = test_sign_rs256(&key_b, &format!("made-up-{i}"), &claims(ISSUER, json!("x"), 300));
            assert!(verify_inbound_bearer(&auth, &jwks, None, Some(&junk)).await.is_err());
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // The issuer rotates to key B. Once the gap has passed, the unknown
        // kid triggers one refetch and the new key verifies.
        *keys.lock().unwrap() = vec![test_rsa_jwk(&key_b, "kid-b")];
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(verify_inbound_bearer(&auth, &jwks, None, Some(&token_b)).await.is_ok());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        // The retired key A no longer verifies.
        assert!(verify_inbound_bearer(&auth, &jwks, None, Some(&token_a)).await.is_err());
    }

    #[tokio::test]
    async fn require_inbound_share_auth_is_fail_closed_and_verifies_end_to_end() {
        let (db, _path) = temp_db();
        let key = test_rsa_key_pair();
        let keys = Arc::new(Mutex::new(vec![test_rsa_jwk(&key, "kid-e2e")]));
        let url = jwks_server(keys, Arc::new(AtomicUsize::new(0))).await;

        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO mcp_tunnel_auth (tunnel_id, oauth_issuer, audience, direction, jwks_url)
             VALUES ('share-e2e', ?1, 'allternit-share', 'inbound_share', ?2)",
            params![ISSUER, url],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mcp_tunnel_auth (tunnel_id, oauth_issuer) VALUES ('admin-out', ?1)",
            params![ISSUER],
        )
        .unwrap();
        drop(conn);

        let good = test_sign_rs256(&key, "kid-e2e", &claims(ISSUER, json!("allternit-share"), 300));
        let (policy, verified) =
            require_inbound_share_auth(&db, "share-e2e", None, Some(&good)).await.expect("valid token");
        assert_eq!(policy.direction, TunnelDirection::InboundShare);
        assert_eq!(verified["sub"], "user_recipient");

        let forged = test_sign_rs256(&test_rsa_key_pair(), "kid-e2e", &claims(ISSUER, json!("allternit-share"), 300));
        assert_eq!(
            require_inbound_share_auth(&db, "share-e2e", None, Some(&forged)).await.unwrap_err(),
            TunnelAuthResult::InvalidToken
        );
        // No policy row: denied (MCP shares always require auth).
        assert_eq!(
            require_inbound_share_auth(&db, "no-such-share", None, Some(&good)).await.unwrap_err(),
            TunnelAuthResult::MissingPolicy
        );
        // An existing row defaults to outbound and is not an inbound share policy.
        assert_eq!(
            load_tunnel_auth(&db, "admin-out").unwrap().unwrap().direction,
            TunnelDirection::Outbound
        );
        assert_eq!(
            require_inbound_share_auth(&db, "admin-out", None, Some(&good)).await.unwrap_err(),
            TunnelAuthResult::DirectionMismatch
        );
        // And the header path refuses the inbound row.
        assert_eq!(
            require_tunnel_auth(&db, Some("share-e2e"), None, Some(ISSUER), Some("allternit-share")).unwrap_err(),
            TunnelAuthResult::DirectionMismatch
        );
    }
}
