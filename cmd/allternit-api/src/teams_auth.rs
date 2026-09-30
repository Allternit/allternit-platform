//! Microsoft Teams (Bot Framework) auth, behind the injected `HttpSend` seam:
//!
//! * inbound: validate the `Authorization: Bearer <jwt>` on an activity
//!   against the Bot Framework OpenID metadata -> JWKS (cached, refetched when
//!   an unknown `kid` shows up so key rotation works), checking signature
//!   (RS256 only), issuer, audience (= the bot's app id), expiry / not-before,
//!   and that the `serviceurl` claim matches the activity's `serviceUrl`;
//! * outbound: client-credentials access token for the bot's app id/password,
//!   cached until shortly before it expires.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde_json::Value;

use crate::channel_transports::HttpSend;

pub const OPENID_URL: &str = "https://login.botframework.com/v1/.well-known/openidconfiguration";
pub const ISSUER: &str = "https://api.botframework.com";
pub const TOKEN_URL: &str = "https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token";
pub const TOKEN_SCOPE: &str = "https://api.botframework.com/.default";

struct Jwks {
    keys: Vec<Value>,
    fetched: Instant,
}

pub struct TeamsAuth {
    http: Arc<dyn HttpSend>,
    app_id: String,
    app_password: String,
    pub openid_url: String,
    pub token_url: String,
    /// A JWKS this old is refetched even when the key is present.
    pub jwks_ttl: Duration,
    /// An unknown `kid` triggers a refetch, but not more often than this.
    pub min_refetch: Duration,
    jwks: Mutex<Option<Jwks>>,
    token: Mutex<Option<(String, Instant)>>,
}

impl TeamsAuth {
    pub fn new(http: Arc<dyn HttpSend>, app_id: &str, app_password: &str) -> Self {
        TeamsAuth {
            http,
            app_id: app_id.into(),
            app_password: app_password.into(),
            openid_url: OPENID_URL.into(),
            token_url: TOKEN_URL.into(),
            jwks_ttl: Duration::from_secs(24 * 3600),
            min_refetch: Duration::from_secs(300),
            jwks: Mutex::new(None),
            token: Mutex::new(None),
        }
    }

    async fn fetch_jwks(&self) -> Result<Vec<Value>, String> {
        let meta = self.http.get_json(&self.openid_url).await?;
        if meta.status != 200 {
            return Err(format!("Bot Framework OpenID metadata returned {}", meta.status));
        }
        let uri = meta.body["jwks_uri"].as_str().ok_or("OpenID metadata has no jwks_uri")?.to_string();
        let jwks = self.http.get_json(&uri).await?;
        if jwks.status != 200 {
            return Err(format!("Bot Framework JWKS returned {}", jwks.status));
        }
        let keys = jwks.body["keys"].as_array().cloned().ok_or("JWKS has no keys")?;
        *self.jwks.lock().unwrap() = Some(Jwks { keys: keys.clone(), fetched: Instant::now() });
        Ok(keys)
    }

    async fn key_for(&self, kid: &str) -> Result<Value, String> {
        let find = |keys: &[Value]| keys.iter().find(|k| k["kid"] == kid && k["kty"] == "RSA").cloned();
        let (cached, age) = {
            let g = self.jwks.lock().unwrap();
            match g.as_ref() {
                Some(j) => (find(&j.keys), Some(j.fetched.elapsed())),
                None => (None, None),
            }
        };
        if let (Some(k), Some(age)) = (&cached, age) {
            if age < self.jwks_ttl {
                return Ok(k.clone());
            }
        }
        // Missing key or stale cache: refetch (rate-limited by min_refetch when we already hold keys).
        if cached.is_none() && age.map_or(false, |a| a < self.min_refetch) {
            return Err("unknown signing key".into());
        }
        match self.fetch_jwks().await {
            Ok(keys) => find(&keys).ok_or_else(|| "unknown signing key".to_string()),
            Err(e) => cached.ok_or(e),
        }
    }

    /// Validate an activity's `Authorization` header value. Returns the claims.
    pub async fn validate(&self, authorization: &str, activity: &Value) -> Result<Value, String> {
        let jwt = authorization.strip_prefix("Bearer ").ok_or("missing bearer token")?.trim();
        let header = decode_header(jwt).map_err(|_| "malformed token")?;
        if header.alg != Algorithm::RS256 {
            return Err("unsupported token algorithm".into());
        }
        let kid = header.kid.ok_or("token has no kid")?;
        let jwk = self.key_for(&kid).await?;
        let key = DecodingKey::from_rsa_components(jwk["n"].as_str().ok_or("bad JWK")?, jwk["e"].as_str().ok_or("bad JWK")?).map_err(|_| "bad JWK")?;
        let mut v = Validation::new(Algorithm::RS256);
        v.set_audience(&[self.app_id.as_str()]);
        v.set_issuer(&[ISSUER]);
        v.leeway = 300;
        v.validate_nbf = true;
        v.set_required_spec_claims(&["exp", "aud", "iss"]);
        let claims = decode::<Value>(jwt, &key, &v).map_err(|e| format!("invalid Teams token: {e}"))?.claims;
        if let (Some(claim), Some(svc)) = (claims["serviceurl"].as_str(), activity["serviceUrl"].as_str()) {
            if claim.trim_end_matches('/') != svc.trim_end_matches('/') {
                return Err("serviceUrl does not match the token".into());
            }
        }
        Ok(claims)
    }

    /// Outbound client-credentials token (cached).
    pub async fn access_token(&self) -> Result<String, String> {
        if let Some((t, exp)) = self.token.lock().unwrap().as_ref() {
            if Instant::now() < *exp {
                return Ok(t.clone());
            }
        }
        let resp = self
            .http
            .post_form(
                &self.token_url,
                vec![
                    ("grant_type".into(), "client_credentials".into()),
                    ("client_id".into(), self.app_id.clone()),
                    ("client_secret".into(), self.app_password.clone()),
                    ("scope".into(), TOKEN_SCOPE.into()),
                ],
            )
            .await?;
        if resp.status != 200 {
            return Err(format!("Teams token endpoint returned {}", resp.status));
        }
        let t = resp.body["access_token"].as_str().ok_or("token response has no access_token")?.to_string();
        let ttl = resp.body["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
        *self.token.lock().unwrap() = Some((t.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(t)
    }
}

/// One `TeamsAuth` per app id for the process, so the JWKS and token caches survive across requests.
pub fn shared(http: Arc<dyn HttpSend>, app_id: &str, app_password: &str) -> Arc<TeamsAuth> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<TeamsAuth>>>> = OnceLock::new();
    let mut m = CACHE.get_or_init(Default::default).lock().unwrap();
    m.entry(format!("{app_id}:{app_password}")).or_insert_with(|| Arc::new(TeamsAuth::new(http, app_id, app_password))).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::HttpResp;
    use async_trait::async_trait;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    const KEY1: &str = "-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCi+V2Pn8BLlTkI
nZG5poon+Rkas+1I0BQ5N1tGwMf+Frhks8c4Ju0b29A84G521SmZXEj89fXXfY1Y
I2jJmvi8WSwXE/4ICU8tiqCYHGqgvAmIFQUda2JF+USsm8O9AeutiTqxTU8cDAnb
C/+WVvgIkZsv3zB1iZaV4/5xbrJFFO3wzdoZyvzVjT62+XIDO3aZg9lzAa0+bgUp
2AAwVdVRokySxXHtCXg87g1l8NEREDiZNCmL/mAbR3s4JBN/+bqBvxmzNNvy0YW6
aL+xoQ3v32I9Hz8PK4Y8xfcbDqJuEhE6mpRunfIevnfOhkojC5Yfdy8An/MdydVE
iC4H2//tAgMBAAECggEAF72x1nlUHu6XB11E3EGZgKc19ADgASpmt8sLnev5dldC
91CFJfXttpA37pZXITJ+Q9wAufDupjyg+YT2/992eqGW3anI6MzKXv0B1gbVtaKO
1OcS/q0k/MKKMYbjZcZA2d/Sz/9OFezfBqzhx7nVmhH0BG5D9etcJ2u2JjoU+CEP
1bdcm78ClHV3s5OGHO/smCmItBjbtfpB6n57CFaufczTN0EtkQ3A6xvHou6EVYLP
fa52liKgPPxOBoJLiLTj5DJlGGkOSg4Z8mQw9JDtNukNtN6SaUVPjDTpy2MHu6dB
w5ENNf8eLu4r02xwQ+nYaGpCWddbnw2h18sEI00G1QKBgQDUgvIxNAofGe5SyMDZ
tkv2CmQBczEQGw5M8uP6ZcalnqdzzXkCIOpY5ZV5sSUWPqYLM7ctxEvJBQfLvwsV
/GAtUnqdtqXZuwNOl1skOFYbdgasvoNtfCpUiMMTr2KxmpvfV47YzcukH7XacOtn
81mPdM8e7kenCvj93t9/pAAtJwKBgQDEU0CpfAiBnJ8onG4gGsqCrNTmdlj/ogwk
GM2HhpCcsCEHzTLHothb8jWCmy8Eey7Bgfd3RcZQ18GFnspeUKrUuVQpwEZt5yLs
SKuk1rEJYU3K0HdsbvJDYxJ3uYT5ymYKjfDe3Idu5YX64R/CPZUcdcKP3ivaUIJ2
/KEl6+l+ywKBgEkoHvoDQSy9v4ZuJ72K/RFhOFYrcotp1P12bDcKDF19hYXfCTZL
YIFj35Y5/ltvC7g1gGzX84LrIWjphoZ2ECHqD931P5j5wUSv5CdO4Y2ojtmu6A2r
veLGBenj6aTcZaZ4POuzxNPmOrNNRizN4Fn7S0YJn08I/vheXjBRo+HnAoGAdsxN
34D/gLaulJt8BA9SJZrBxactmZqMMDeV/wFNdpMZmafwp18B/zlkaeDPpa2IkG29
uj3NkFPOdbWtjT7Q8KIorI79zzlDJ6mdu8RyIlPwB973tPS5wk8r+KiZL7Hq504W
yDS3+0IGPdaGKjqrhSo5DmeJL7TyDWA3f0Pu6dsCgYB1TZYTgbOV5Jn5Ila/RE1V
gtelVe6xn2dmgYqirOhWlnppQ1zmHACcjTrzpvs5dkrmuXc+5S1cje4ZaF0CYz5E
49qcc5Ts+IlZX1Vgvxt/693zoMTHQb4MBP3CoJcAjIQv28XVR58dah7RWKyKHRM+
HO+PSKaqgKIK8RJjwojL/g==
-----END PRIVATE KEY-----";
    const N1: &str = "ovldj5_AS5U5CJ2RuaaKJ_kZGrPtSNAUOTdbRsDH_ha4ZLPHOCbtG9vQPOBudtUpmVxI_PX1132NWCNoyZr4vFksFxP-CAlPLYqgmBxqoLwJiBUFHWtiRflErJvDvQHrrYk6sU1PHAwJ2wv_llb4CJGbL98wdYmWleP-cW6yRRTt8M3aGcr81Y0-tvlyAzt2mYPZcwGtPm4FKdgAMFXVUaJMksVx7Ql4PO4NZfDRERA4mTQpi_5gG0d7OCQTf_m6gb8ZszTb8tGFumi_saEN799iPR8_DyuGPMX3Gw6ibhIROpqUbp3yHr53zoZKIwuWH3cvAJ_zHcnVRIguB9v_7Q";
    const KEY2: &str = "-----BEGIN PRIVATE KEY-----
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQDreO7gxJN6fTsB
LuIIAmZTsRwE0i5fgVDMyYcwfnhMXbXRaUE4gN5B142VstJfKnfjObkFxFiRIzHK
Gx/SUReMi/DYYkHGQ+71/N1UUk7FgdWvxtpK50SV4jYIPkCrUzkO4GNp+WZ/mP4H
EyTycbjV4dipC6ITz61pIh95xKEL7vzlNeQKl+tX8vZ6FVdlOS/lM4Wz4VhwNh3k
8wrkt7n+SJrVIt24wCNXGX76tB/W2nE0v7fgsdF8qs2W4HFXJgw/65muikId/XrW
bZuR3UI6VyCw1JMswwJqUdZU5bO/NwmS3bdj3ZI1Y+9gMh5+Zn1qPhksUodqdMXn
XMV2hC5vAgMBAAECggEBAJnDwtnsBBI8rMc9AkUQrBmC0jUjuzyKiWTxROKZ85yK
pSw2e2yWWozMYZybxVx3swoiq3vkl6FPRjggNkt0wNf6mi/zGdiKjAT+NtnVVbML
8apzRCEdnF/z9Cy12i0Gj3/zb3fIcPJpyZh9G+dl87lXXdAl1JTzTN4Wdk9h43iS
Oqm2K+6JPTegJkbVDLxoYJcI9r634Pe+T7jZSna1Y1MCVp6NGQvVSi6gMXUO56gu
/vG6n6EExQ4OhHyAbHSEEhoXbbKUrFC97pcb7HpjWcXE267a+okLbgbgBFMdg4Od
8gocluLwA3MLUFrgGulkgETMMop6VTcUzz5A0iQLIgECgYEA/VhNtxjNgj/hItDY
GkV2J2t9Wi+cgCezGAc178WzXJ65j+TTJ6w9rzGB9lvIs7pQr041LWSmOuA+VdKc
FOrO5OSRpOGW1YWSSmL8qRb0TuvXLZemcSF3zLX5Qx2ukkJmPw3LPAlZCHVY4pBU
7clPpYN6JxUKfCugoZnPMAErG68CgYEA7fCt80BKKSeXI6geX/pI5r3AhRpfygA1
Ega+i7MtRL/hfK9BGAa6pbN91ceFue3W+gGR4OmnlUESON4B7sILpZvyR6Ae2/ID
fdme0ZDmfCQh5+0KwoaITVredDwcB3i9O4FGo0R2kvfHE2EBW5/SO740n8OykiPZ
rdf5HIw3CUECgYAahL+9xq7cT2t1hX7ZYSP5BFtesVUkZQNuJHMU4hDgRQ0Pbthb
csASMpB0BS+BAKOpdfpDAiIUT5y2xxDnb5ywgOxt4d80AoNJngrseCaijDU95H3b
obE0kGfDCrxXOvQQ7ZS0eBYFuKLRNGJvcX8QyC5lIOK0FKz/vWXelIC6qQKBgQDi
ZuutlNO7+x7V38cfPhCV9aG1RNY2aCIXR/GRDemaDBYrRLrkeKqqtnKasuHse4Hd
mxbKcqlx3FvRXnVqUJsydoN/Yb1bPRnTavmyoHSfVOYqP6PIWqhhKoCXCwcEmP8+
GpEzExbcWwXCy7+2BgnNrPt3tYc5hQFAaEtxvX00wQKBgQCu9R7edqac9o0TNxup
TEwYgvcm1Q5gHd7ZkAfhCugpmCZdX2RTvMkUFYM11riNutdEsM1JB8ezzq8khX8W
Agzz0dZiwzOv/RmfxdNBxHBp4ZOptMKhB/wn55EyLcysaff9EcjeYHIXG+jVHrtv
Q4zTS1ptokh5sSJ47XyyfEt5PQ==
-----END PRIVATE KEY-----";
    const N2: &str = "63ju4MSTen07AS7iCAJmU7EcBNIuX4FQzMmHMH54TF210WlBOIDeQdeNlbLSXyp34zm5BcRYkSMxyhsf0lEXjIvw2GJBxkPu9fzdVFJOxYHVr8baSudEleI2CD5Aq1M5DuBjaflmf5j-BxMk8nG41eHYqQuiE8-taSIfecShC-785TXkCpfrV_L2ehVXZTkv5TOFs-FYcDYd5PMK5Le5_kia1SLduMAjVxl--rQf1tpxNL-34LHRfKrNluBxVyYMP-uZropCHf161m2bkd1COlcgsNSTLMMCalHWVOWzvzcJkt23Y92SNWPvYDIefmZ9aj4ZLFKHanTF51zFdoQubw";

    /// Serves OpenID metadata + a JWKS holding whichever keys are currently "published".
    #[derive(Default)]
    struct FakeIdp {
        keys: Mutex<Vec<Value>>,
        gets: Mutex<Vec<String>>,
        form: Mutex<Vec<Vec<(String, String)>>>,
    }
    #[async_trait]
    impl HttpSend for FakeIdp {
        async fn post_json(&self, _r: crate::channel_transports::HttpReq) -> Result<HttpResp, String> {
            Err("unused".into())
        }
        async fn get_json(&self, url: &str) -> Result<HttpResp, String> {
            self.gets.lock().unwrap().push(url.to_string());
            if url == OPENID_URL {
                Ok(HttpResp { status: 200, body: json!({ "jwks_uri": "https://idp.test/keys" }) })
            } else {
                Ok(HttpResp { status: 200, body: json!({ "keys": self.keys.lock().unwrap().clone() }) })
            }
        }
        async fn post_form(&self, _url: &str, form: Vec<(String, String)>) -> Result<HttpResp, String> {
            self.form.lock().unwrap().push(form);
            Ok(HttpResp { status: 200, body: json!({ "access_token": "tok-1", "expires_in": 3600 }) })
        }
    }

    fn jwk(kid: &str, n: &str) -> Value {
        json!({ "kty": "RSA", "kid": kid, "n": n, "e": "AQAB", "use": "sig" })
    }
    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }
    fn sign(pem: &str, kid: &str, claims: Value) -> String {
        let mut h = Header::new(Algorithm::RS256);
        h.kid = Some(kid.into());
        format!("Bearer {}", encode(&h, &claims, &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap()).unwrap())
    }
    fn good(app: &str) -> Value {
        json!({ "iss": ISSUER, "aud": app, "exp": now() + 600, "nbf": now() - 10, "serviceurl": "https://smba.test/amer/" })
    }
    fn activity() -> Value {
        json!({ "serviceUrl": "https://smba.test/amer" })
    }

    #[tokio::test]
    async fn validates_signature_issuer_audience_expiry_and_service_url() {
        let idp = Arc::new(FakeIdp::default());
        *idp.keys.lock().unwrap() = vec![jwk("k1", N1)];
        let auth = TeamsAuth::new(idp.clone(), "app-1", "pw");
        assert!(auth.validate(&sign(KEY1, "k1", good("app-1")), &activity()).await.is_ok());
        assert!(auth.validate(&sign(KEY1, "k1", good("app-2")), &activity()).await.is_err());
        let mut c = good("app-1");
        c["iss"] = json!("https://evil.example");
        assert!(auth.validate(&sign(KEY1, "k1", c), &activity()).await.is_err());
        let mut c = good("app-1");
        c["exp"] = json!(now() - 3600);
        assert!(auth.validate(&sign(KEY1, "k1", c), &activity()).await.is_err());
        let mut c = good("app-1");
        c["serviceurl"] = json!("https://elsewhere.test/");
        assert!(auth.validate(&sign(KEY1, "k1", c), &activity()).await.unwrap_err().contains("serviceUrl"));
        // Signed by a different key but claiming kid k1.
        assert!(auth.validate(&sign(KEY2, "k1", good("app-1")), &activity()).await.is_err());
        assert!(auth.validate("Bearer not.a.jwt", &activity()).await.is_err());
        assert!(auth.validate("HMAC abc", &activity()).await.is_err());
        // The metadata + JWKS were fetched once and then cached.
        assert_eq!(idp.gets.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn key_rotation_refetches_on_unknown_kid_but_rate_limits_it() {
        let idp = Arc::new(FakeIdp::default());
        *idp.keys.lock().unwrap() = vec![jwk("k1", N1)];
        let mut auth = TeamsAuth::new(idp.clone(), "app-1", "pw");
        auth.min_refetch = Duration::from_millis(0);
        assert!(auth.validate(&sign(KEY1, "k1", good("app-1")), &activity()).await.is_ok());
        // Microsoft rotates: k2 appears; the unknown kid triggers a refetch.
        *idp.keys.lock().unwrap() = vec![jwk("k1", N1), jwk("k2", N2)];
        assert!(auth.validate(&sign(KEY2, "k2", good("app-1")), &activity()).await.is_ok());
        // With a long min_refetch, an unknown kid does not hammer the IdP.
        auth.min_refetch = Duration::from_secs(3600);
        let before = idp.gets.lock().unwrap().len();
        assert!(auth.validate(&sign(KEY2, "k9", good("app-1")), &activity()).await.is_err());
        assert_eq!(idp.gets.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn outbound_client_credentials_token_is_cached() {
        let idp = Arc::new(FakeIdp::default());
        let auth = TeamsAuth::new(idp.clone(), "app-1", "s3cret");
        assert_eq!(auth.access_token().await.unwrap(), "tok-1");
        assert_eq!(auth.access_token().await.unwrap(), "tok-1");
        let f = idp.form.lock().unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].contains(&("grant_type".to_string(), "client_credentials".to_string())));
        assert!(f[0].contains(&("client_id".to_string(), "app-1".to_string())));
        assert!(f[0].contains(&("scope".to_string(), TOKEN_SCOPE.to_string())));
    }
}
