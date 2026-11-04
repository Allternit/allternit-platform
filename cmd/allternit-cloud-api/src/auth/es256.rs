//! ES256 JWTs for managed connector auth (docs/MANAGED_AUTH_MAP.md WP1/WP4).
//!
//! Hand-rolled like `dataplane_jwt` (three base64url segments, one
//! aws-lc-rs call), but ES256: vendor authorization servers rarely accept
//! EdDSA. One P-256 key signs both the org ID-JAGs (`routes::managed_auth`)
//! and the edge authorization server's access tokens (`routes::edge_auth`);
//! the two never mix because `iss`, `aud` and `typ` differ and each verifier
//! checks all three.
//!
//! - **Key:** PEM (PKCS#8 `PRIVATE KEY` or SEC1 `EC PRIVATE KEY`) in env
//!   `ALLTERNIT_IDJAG_ES256_PEM`; literal `\n` escapes are accepted. Unset or
//!   unparsable ⇒ `None`, and every caller fails closed (503
//!   `idjag_not_configured`). Generate one with
//!   `openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt`.
//! - **kid:** base64url SHA-256 of the uncompressed public point, so a new
//!   key changes `kid` and verifiers refetch the JWKS.
//! - **Verify:** [`verify_with_jwks`] checks an ES256 or RS256 JWT against a
//!   JWK set (outside issuers such as Okta sign RS256). Never `none`, never HMAC.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{self, EcdsaKeyPair, KeyPair as _, UnparsedPublicKey};
use base64::{engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use sha2::Digest as _;

pub const PEM_ENV: &str = "ALLTERNIT_IDJAG_ES256_PEM";

pub struct Es256Key {
    pair: EcdsaKeyPair,
    kid: String,
}

impl Es256Key {
    pub fn from_pem(pem: &str) -> Result<Self, String> {
        let pem = pem.replace("\\n", "\n");
        let body: String = pem.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("-----")).collect();
        let der = STANDARD.decode(body.as_bytes()).map_err(|_| "the ES256 key is not valid PEM".to_string())?;
        let pair = EcdsaKeyPair::from_private_key_der(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &der).map_err(|e| format!("the ES256 key was rejected: {e}"))?;
        let kid = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(pair.public_key().as_ref()));
        Ok(Self { pair, kid })
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The public JWK (`kty: EC`, `crv: P-256`).
    pub fn jwk(&self) -> Value {
        let point = self.pair.public_key().as_ref();
        json!({
            "kty": "EC",
            "crv": "P-256",
            "use": "sig",
            "alg": "ES256",
            "kid": self.kid,
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
        })
    }

    pub fn jwks(&self) -> Value {
        json!({ "keys": [self.jwk()] })
    }

    /// A compact JWS over `claims` with header `{alg: ES256, typ, kid}`.
    pub fn sign(&self, typ: &str, claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "ES256", "typ": typ, "kid": self.kid }).to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let input = format!("{header}.{payload}");
        let sig = self.pair.sign(&SystemRandom::new(), input.as_bytes()).expect("P-256 signing does not fail");
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
    }
}

/// The signing key from `ALLTERNIT_IDJAG_ES256_PEM`; `None` = not configured
/// (callers answer 503). Parsed per call so a missing key fails closed everywhere.
pub fn key_from_env() -> Option<Es256Key> {
    let pem = std::env::var(PEM_ENV).ok().filter(|v| !v.trim().is_empty())?;
    match Es256Key::from_pem(&pem) {
        Ok(k) => Some(k),
        Err(e) => {
            tracing::error!("{PEM_ENV}: {e}");
            None
        }
    }
}

/// `(header, claims)` without checking the signature. Only for routing
/// (which issuer, which key); every decision uses [`verify_with_jwks`].
pub fn decode_unverified(token: &str) -> Option<(Value, Value)> {
    let mut parts = token.split('.');
    let (h, p, _s) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(h).ok()?).ok()?;
    let claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(p).ok()?).ok()?;
    Some((header, claims))
}

/// Check the signature of an ES256 or RS256 JWT against a JWK set and return
/// `(header, claims)`. The key is picked by `kid`; a token without `kid`
/// is only accepted when the set holds exactly one key. Claims are NOT
/// checked here (exp, aud, ...): callers do that.
pub fn verify_with_jwks(token: &str, jwks: &Value) -> Result<(Value, Value), String> {
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err("malformed token".into());
    };
    let header: Value = URL_SAFE_NO_PAD.decode(h).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or("malformed header")?;
    let alg = header["alg"].as_str().unwrap_or_default();
    let keys = jwks["keys"].as_array().cloned().unwrap_or_default();
    let key = match header["kid"].as_str() {
        Some(kid) => keys.iter().find(|k| k["kid"].as_str() == Some(kid)),
        None if keys.len() == 1 => keys.first(),
        None => None,
    }
    .ok_or("signing key not found")?;
    let sig = URL_SAFE_NO_PAD.decode(s).map_err(|_| "malformed signature")?;
    let input = format!("{h}.{p}");
    let b64 = |name: &str| key[name].as_str().and_then(|v| URL_SAFE_NO_PAD.decode(v).ok());
    let ok = match (alg, key["kty"].as_str()) {
        ("ES256", Some("EC")) if key["crv"].as_str() == Some("P-256") => {
            let (Some(x), Some(y)) = (b64("x"), b64("y")) else { return Err("bad EC key".into()) };
            if x.len() != 32 || y.len() != 32 {
                return Err("bad EC key".into());
            }
            let mut point = vec![4u8];
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, &point).verify(input.as_bytes(), &sig).is_ok()
        }
        ("RS256", Some("RSA")) => {
            let (Some(n), Some(e)) = (b64("n"), b64("e")) else { return Err("bad RSA key".into()) };
            signature::RsaPublicKeyComponents { n: &n, e: &e }.verify(&signature::RSA_PKCS1_2048_8192_SHA256, input.as_bytes(), &sig).is_ok()
        }
        _ => return Err(format!("unsupported algorithm {alg:?}")),
    };
    if !ok {
        return Err("signature mismatch".into());
    }
    let claims: Value = URL_SAFE_NO_PAD.decode(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or("malformed payload")?;
    Ok((header, claims))
}

/// `aud` (string or array) contains `want` (trailing slashes ignored).
pub fn has_audience(claims: &Value, want: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_string();
    match &claims["aud"] {
        Value::String(s) => norm(s) == norm(want),
        Value::Array(a) => a.iter().filter_map(Value::as_str).any(|s| norm(s) == norm(want)),
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod testing {
    /// A fresh P-256 key as PKCS#8 PEM.
    pub fn pem() -> String {
        use base64::Engine as _;
        let doc = aws_lc_rs::signature::EcdsaKeyPair::generate_pkcs8(&aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &aws_lc_rs::rand::SystemRandom::new()).unwrap();
        format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", base64::engine::general_purpose::STANDARD.encode(doc.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_token_verifies_against_its_own_jwks_and_not_another_keys_or_a_tampered_payload() {
        let key = Es256Key::from_pem(&testing::pem()).unwrap();
        let other = Es256Key::from_pem(&testing::pem()).unwrap();
        let token = key.sign("oauth-id-jag+jwt", &json!({ "sub": "u1" }));
        let (header, claims) = verify_with_jwks(&token, &key.jwks()).unwrap();
        assert_eq!((header["typ"].as_str(), header["alg"].as_str(), claims["sub"].as_str()), (Some("oauth-id-jag+jwt"), Some("ES256"), Some("u1")));
        assert!(verify_with_jwks(&token, &other.jwks()).is_err(), "kid differs");
        let mut forged = other.jwk();
        forged["kid"] = json!(key.kid());
        assert_eq!(verify_with_jwks(&token, &json!({ "keys": [forged] })).unwrap_err(), "signature mismatch");
        let parts: Vec<&str> = token.split('.').collect();
        let tampered = format!("{}.{}.{}", parts[0], URL_SAFE_NO_PAD.encode(json!({ "sub": "admin" }).to_string()), parts[2]);
        assert!(verify_with_jwks(&tampered, &key.jwks()).is_err());
    }

    #[test]
    fn none_and_hmac_tokens_are_refused_and_escaped_newlines_in_the_env_pem_parse() {
        let key = Es256Key::from_pem(&testing::pem().replace('\n', "\\n")).unwrap();
        let h = URL_SAFE_NO_PAD.encode(json!({ "alg": "none", "kid": key.kid() }).to_string());
        let p = URL_SAFE_NO_PAD.encode(json!({ "sub": "u1" }).to_string());
        assert!(verify_with_jwks(&format!("{h}.{p}."), &key.jwks()).is_err());
        let h = URL_SAFE_NO_PAD.encode(json!({ "alg": "HS256", "kid": key.kid() }).to_string());
        assert!(verify_with_jwks(&format!("{h}.{p}.AAAA"), &key.jwks()).is_err());
    }
}
