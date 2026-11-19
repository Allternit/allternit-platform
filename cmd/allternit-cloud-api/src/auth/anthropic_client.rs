//! Allternit as a client of Anthropic, for "Link your Claude organization"
//! (CONTRACT-oauth-linked-billing-2026-11.md, Part B, section B1).
//!
//! One dedicated ES256 key (env `ALLTERNIT_ANTHROPIC_CLIENT_ES256_PEM`, never
//! the ID-JAG key) signs two kinds of JWT under our own issuer
//! `https://api.allternit.com/oauth2/anthropic-client`, whose JWKS and
//! discovery document `routes::anthropic_client_issuer` serves:
//!
//! 1. **Client assertions** (`private_key_jwt`) for the auth.claude.com PAR,
//!    token and revoke calls. Per Anthropic's OAuth reference these are not
//!    RFC 7523 client-id claims: `iss` = our issuer, `sub` = [`SUBJECT`]
//!    (matches the subject pattern on the OAuth app), `aud` = exactly one
//!    value (`ALLTERNIT_ANTHROPIC_CLIENT_AUD`), 300 s life, unique `jti`,
//!    header `kid`. Sent as body fields ([`AnthropicClient::client_assertion_fields`]),
//!    never in a query string.
//! 2. **The Workload Identity Federation JWT**, exchanged at
//!    `POST {API}/v1/oauth/token` (JSON body) for the operator token
//!    `sk-ant-oat01-…`. Anthropic states no fixed `aud` for it: the federation
//!    rule may match an exact audience, so ours comes from
//!    `ALLTERNIT_ANTHROPIC_WIF_AUD`. Its life is 1800 s because the minted
//!    token lives at most twice the JWT's remaining life: 1800 s lets a
//!    3600 s rule give a full hour. Each exchange mints a new JWT (a `jti`
//!    is single use at Anthropic).
//!
//! [`OperatorTokens`] caches the operator token in memory: one exchange at a
//! time (the mutex is held across the exchange), re-exchanged 120 s before it
//! expires, dropped by [`OperatorTokens::invalidate`] after a 401. The token
//! is never logged: [`OperatorToken`]'s `Debug` is redacted.
//!
//! The feature is off unless `ALLTERNIT_CLAUDE_LINK=1` and every variable in
//! [`REQUIRED_ENV`] is set and the PEM parses ([`is_configured`]); otherwise
//! it reports `not_available_yet`. The JWKS routes need only the PEM, so the
//! issuer can be registered at Anthropic before the switch is turned on.
//!
//! The `kid` is [`Es256Key`]'s: base64url SHA-256 of the uncompressed public
//! point, the same value in the header and in the served JWKS.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::auth::es256::Es256Key;

/// `1` turns the feature on; anything else (or unset) keeps it off.
pub const LINK_ENV: &str = "ALLTERNIT_CLAUDE_LINK";
/// The signing key (PKCS#8 or SEC1 PEM; literal `\n` escapes accepted).
pub const PEM_ENV: &str = "ALLTERNIT_ANTHROPIC_CLIENT_ES256_PEM";
/// The public base our issuer lives under (default `https://api.allternit.com`).
pub const ISSUER_BASE_ENV: &str = "ALLTERNIT_ANTHROPIC_CLIENT_ISSUER_BASE";
/// The single `aud` on the OAuth app's Client authentication tab.
pub const CLIENT_AUD_ENV: &str = "ALLTERNIT_ANTHROPIC_CLIENT_AUD";
/// The `aud` the federation rule expects on the WIF JWT.
pub const WIF_AUD_ENV: &str = "ALLTERNIT_ANTHROPIC_WIF_AUD";
pub const FEDERATION_RULE_ID_ENV: &str = "ALLTERNIT_ANTHROPIC_FEDERATION_RULE_ID";
pub const ORGANIZATION_ID_ENV: &str = "ALLTERNIT_ANTHROPIC_ORGANIZATION_ID";
pub const SERVICE_ACCOUNT_ID_ENV: &str = "ALLTERNIT_ANTHROPIC_SERVICE_ACCOUNT_ID";
pub const WORKSPACE_ID_ENV: &str = "ALLTERNIT_ANTHROPIC_WORKSPACE_ID";
/// Our OAuth app at platform.claude.com: its client id and app id.
pub const OAUTH_CLIENT_ID_ENV: &str = "ALLTERNIT_ANTHROPIC_OAUTH_CLIENT_ID";
pub const OAUTH_APP_ID_ENV: &str = "ALLTERNIT_ANTHROPIC_OAUTH_APP_ID";
/// Base URLs; tests point them at a fake server.
pub const API_BASE_ENV: &str = "ALLTERNIT_ANTHROPIC_API_BASE";
pub const AUTH_BASE_ENV: &str = "ALLTERNIT_ANTHROPIC_AUTH_BASE";

/// Required (besides [`LINK_ENV`] and [`PEM_ENV`]) for the feature to be on.
pub const REQUIRED_ENV: [&str; 8] = [
    CLIENT_AUD_ENV,
    WIF_AUD_ENV,
    FEDERATION_RULE_ID_ENV,
    ORGANIZATION_ID_ENV,
    SERVICE_ACCOUNT_ID_ENV,
    WORKSPACE_ID_ENV,
    OAUTH_CLIENT_ID_ENV,
    OAUTH_APP_ID_ENV,
];

const DEFAULT_ISSUER_BASE: &str = "https://api.allternit.com";
const DEFAULT_API_BASE: &str = "https://api.anthropic.com";
const DEFAULT_AUTH_BASE: &str = "https://auth.claude.com";
pub const ISSUER_PATH: &str = "/oauth2/anthropic-client";
/// `sub` of every JWT we sign here; the OAuth app's subject pattern and the
/// federation rule's `subject_prefix` must match it.
pub const SUBJECT: &str = "allternit-cloud-api";
pub const CLIENT_ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
pub const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
/// Client assertion life (Anthropic allows up to 3600 s).
pub const ASSERTION_TTL_SECS: i64 = 300;
/// WIF JWT life (Anthropic allows up to 3600 s); see the module comment.
pub const WIF_TTL_SECS: i64 = 1800;
/// The operator token is re-exchanged this long before it expires.
pub const REFRESH_BEFORE_EXPIRY: Duration = Duration::from_secs(120);
/// Used when the exchange response has no `expires_in` (Anthropic's default rule life).
const DEFAULT_TOKEN_LIFETIME_SECS: u64 = 3600;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(15);
/// The reason code a surface shows while the feature is off.
pub const NOT_AVAILABLE_YET: &str = "not_available_yet";

/// `{base}/oauth2/anthropic-client`.
pub fn issuer() -> String {
    issuer_with(&env_lookup)
}

fn issuer_with(lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let base = lookup(ISSUER_BASE_ENV).map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| !v.is_empty());
    format!("{}{ISSUER_PATH}", base.as_deref().unwrap_or(DEFAULT_ISSUER_BASE))
}

/// The signing key alone (for the JWKS routes); `None` when the PEM is unset
/// or does not parse. Independent of [`LINK_ENV`].
pub fn signing_key() -> Option<Es256Key> {
    let pem = env_lookup(PEM_ENV)?;
    match Es256Key::from_pem(&pem) {
        Ok(key) => Some(key),
        Err(e) => {
            tracing::error!("{PEM_ENV}: {e}");
            None
        }
    }
}

/// The feature is on: [`LINK_ENV`] is `1`, every required variable is set and
/// the PEM parses. When false, surfaces report [`NOT_AVAILABLE_YET`].
pub fn is_configured() -> bool {
    AnthropicClient::from_env().is_ok()
}

fn env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Why the feature is off: the names of what is missing (never values).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Link your Claude organization is not set up on this server (missing: {})", .missing.join(", "))]
pub struct NotConfigured {
    pub missing: Vec<&'static str>,
}

impl NotConfigured {
    pub fn code(&self) -> &'static str {
        NOT_AVAILABLE_YET
    }
}

/// Which auth.claude.com call a client assertion is for. The claims are the
/// same for all three (the OAuth app has one audience); the kind only marks
/// the `jti`, so a rejected assertion can be traced to its call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssertionEndpoint {
    Par,
    Token,
    Revoke,
}

impl AssertionEndpoint {
    fn as_str(self) -> &'static str {
        match self {
            Self::Par => "par",
            Self::Token => "token",
            Self::Revoke => "revoke",
        }
    }
}

/// Everything needed to talk to Anthropic as Allternit. Built per use from
/// the environment ([`AnthropicClient::from_env`]), so a missing variable
/// fails closed everywhere.
pub struct AnthropicClient {
    key: Es256Key,
    issuer: String,
    client_aud: String,
    wif_aud: String,
    federation_rule_id: String,
    organization_id: String,
    service_account_id: String,
    workspace_id: String,
    oauth_client_id: String,
    oauth_app_id: String,
    api_base: String,
    auth_base: String,
}

impl AnthropicClient {
    /// From the process environment.
    pub fn from_env() -> Result<Self, NotConfigured> {
        Self::from_lookup(&env_lookup)
    }

    /// From any name → value lookup (tests pass a map). Empty values count as missing.
    pub fn from_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Self, NotConfigured> {
        let get = |name: &str| lookup(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let mut missing: Vec<&'static str> = Vec::new();
        if get(LINK_ENV).as_deref() != Some("1") {
            missing.push(LINK_ENV);
        }
        let key = match get(PEM_ENV).map(|pem| Es256Key::from_pem(&pem)) {
            Some(Ok(key)) => Some(key),
            Some(Err(e)) => {
                tracing::error!("{PEM_ENV}: {e}");
                missing.push(PEM_ENV);
                None
            }
            None => {
                missing.push(PEM_ENV);
                None
            }
        };
        let mut values: Vec<String> = Vec::with_capacity(REQUIRED_ENV.len());
        for name in REQUIRED_ENV {
            match get(name) {
                Some(v) => values.push(v),
                None => {
                    missing.push(name);
                    values.push(String::new());
                }
            }
        }
        let (Some(key), true) = (key, missing.is_empty()) else {
            return Err(NotConfigured { missing });
        };
        let base = |name: &str, default: &str| get(name).map(|v| v.trim_end_matches('/').to_string()).unwrap_or_else(|| default.to_string());
        let mut values = values.into_iter();
        let mut next = || values.next().unwrap_or_default();
        Ok(Self {
            key,
            issuer: issuer_with(lookup),
            client_aud: next(),
            wif_aud: next(),
            federation_rule_id: next(),
            organization_id: next(),
            service_account_id: next(),
            workspace_id: next(),
            oauth_client_id: next(),
            oauth_app_id: next(),
            api_base: base(API_BASE_ENV, DEFAULT_API_BASE),
            auth_base: base(AUTH_BASE_ENV, DEFAULT_AUTH_BASE),
        })
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    /// The JWKS this client's assertions verify against.
    pub fn jwks(&self) -> Value {
        self.key.jwks()
    }
    pub fn kid(&self) -> &str {
        self.key.kid()
    }
    pub fn oauth_client_id(&self) -> &str {
        &self.oauth_client_id
    }
    pub fn oauth_app_id(&self) -> &str {
        &self.oauth_app_id
    }
    /// `https://api.anthropic.com` unless overridden; no trailing slash.
    pub fn api_base(&self) -> &str {
        &self.api_base
    }
    /// `https://auth.claude.com` unless overridden; no trailing slash.
    pub fn auth_base(&self) -> &str {
        &self.auth_base
    }

    /// A `private_key_jwt` client assertion for one auth.claude.com call.
    pub fn client_assertion(&self, endpoint: AssertionEndpoint) -> String {
        self.mint(&self.client_aud, ASSERTION_TTL_SECS, endpoint.as_str())
    }

    /// The three body fields that authenticate a PAR, token or revoke call:
    /// `client_id`, `client_assertion_type`, `client_assertion`.
    pub fn client_assertion_fields(&self, endpoint: AssertionEndpoint) -> [(&'static str, String); 3] {
        [
            ("client_id", self.oauth_client_id.clone()),
            ("client_assertion_type", CLIENT_ASSERTION_TYPE.to_string()),
            ("client_assertion", self.client_assertion(endpoint)),
        ]
    }

    /// A fresh Workload Identity Federation JWT (one per exchange).
    pub fn wif_jwt(&self) -> String {
        self.mint(&self.wif_aud, WIF_TTL_SECS, "wif")
    }

    fn mint(&self, aud: &str, ttl: i64, purpose: &str) -> String {
        let iat = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": self.issuer,
            "sub": SUBJECT,
            "aud": aud,
            "iat": iat,
            "exp": iat + ttl,
            "jti": format!("{purpose}-{}", uuid::Uuid::new_v4()),
        });
        self.key.sign("JWT", &claims)
    }

    /// The WIF exchange body (`POST {API}/v1/oauth/token`, JSON).
    fn exchange_body(&self) -> Value {
        json!({
            "grant_type": JWT_BEARER,
            "assertion": self.wif_jwt(),
            "federation_rule_id": self.federation_rule_id,
            "organization_id": self.organization_id,
            "service_account_id": self.service_account_id,
            "workspace_id": self.workspace_id,
        })
    }
}

// ─── the operator token ───────────────────────────────────────────────────────

/// The operator token (`sk-ant-oat01-…`) and when it expires. `Debug` never
/// shows the token.
#[derive(Clone)]
pub struct OperatorToken {
    secret: String,
    expires_at: Instant,
}

impl OperatorToken {
    /// The value for `Authorization: Bearer …`. Never log it.
    pub fn bearer(&self) -> &str {
        &self.secret
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    fn fresh_at(&self, now: Instant) -> bool {
        now + REFRESH_BEFORE_EXPIRY < self.expires_at
    }
}

impl std::fmt::Debug for OperatorToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperatorToken").field("secret", &"[redacted]").field("expires_at", &self.expires_at).finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OperatorTokenError {
    /// Anthropic refused the exchange (wrong rule, issuer not registered, ...).
    #[error("the Anthropic token exchange was refused with HTTP {status}")]
    Refused { status: u16 },
    #[error("the Anthropic token exchange did not complete: {0}")]
    Transport(String),
    #[error("the Anthropic token exchange returned no token")]
    BadResponse,
}

#[derive(Deserialize)]
struct ExchangeResponse {
    access_token: Option<String>,
    expires_in: Option<u64>,
}

/// The in-memory operator token cache. One exchange at a time.
pub struct OperatorTokens {
    http: reqwest::Client,
    cached: Mutex<Option<OperatorToken>>,
}

impl Default for OperatorTokens {
    fn default() -> Self {
        Self::new()
    }
}

impl OperatorTokens {
    pub fn new() -> Self {
        let http = reqwest::Client::builder().timeout(EXCHANGE_TIMEOUT).build().unwrap_or_default();
        Self { http, cached: Mutex::new(None) }
    }

    /// The cached token while it has more than 120 s left; otherwise one new
    /// exchange. Concurrent callers wait for that one exchange.
    pub async fn get(&self, client: &AnthropicClient) -> Result<OperatorToken, OperatorTokenError> {
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.as_ref().filter(|t| t.fresh_at(Instant::now())) {
            return Ok(token.clone());
        }
        let token = self.exchange(client).await?;
        *cached = Some(token.clone());
        Ok(token)
    }

    /// After Anthropic answered 401 to `rejected`: drop it so the next
    /// [`get`](Self::get) exchanges again. A token another caller already
    /// replaced is left alone.
    pub async fn invalidate(&self, rejected: &OperatorToken) {
        let mut cached = self.cached.lock().await;
        if cached.as_ref().is_some_and(|t| t.secret == rejected.secret) {
            *cached = None;
        }
    }

    async fn exchange(&self, client: &AnthropicClient) -> Result<OperatorToken, OperatorTokenError> {
        let started = Instant::now();
        let response = self
            .http
            .post(format!("{}/v1/oauth/token", client.api_base()))
            .json(&client.exchange_body())
            .send()
            .await
            .map_err(|e| OperatorTokenError::Transport(e.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            // The body is not logged: it is Anthropic's, and we only need the status.
            tracing::warn!(status = status.as_u16(), "Anthropic operator token exchange refused");
            return Err(OperatorTokenError::Refused { status: status.as_u16() });
        }
        let body: ExchangeResponse = response.json().await.map_err(|_| OperatorTokenError::BadResponse)?;
        let secret = body.access_token.filter(|t| !t.is_empty()).ok_or(OperatorTokenError::BadResponse)?;
        let lifetime = Duration::from_secs(body.expires_in.unwrap_or(DEFAULT_TOKEN_LIFETIME_SECS));
        Ok(OperatorToken { secret, expires_at: started + lifetime })
    }
}

/// The process-wide cache the link and provider code use.
pub fn operator_tokens() -> &'static OperatorTokens {
    static TOKENS: OnceLock<OperatorTokens> = OnceLock::new();
    TOKENS.get_or_init(OperatorTokens::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::es256;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;
    use tower::ServiceExt;

    const AUD: &str = "https://auth.claude.com/oauth/client/aud-test";
    const WIF_AUD: &str = "https://api.anthropic.com";

    fn full_env(pem: &str, api_base: &str) -> HashMap<&'static str, String> {
        let mut env: HashMap<&'static str, String> = HashMap::new();
        env.insert(LINK_ENV, "1".into());
        env.insert(PEM_ENV, pem.to_string());
        env.insert(CLIENT_AUD_ENV, AUD.into());
        env.insert(WIF_AUD_ENV, WIF_AUD.into());
        env.insert(FEDERATION_RULE_ID_ENV, "fdrl_test".into());
        env.insert(ORGANIZATION_ID_ENV, "00000000-0000-0000-0000-000000000001".into());
        env.insert(SERVICE_ACCOUNT_ID_ENV, "svac_test".into());
        env.insert(WORKSPACE_ID_ENV, "wrkspc_test".into());
        env.insert(OAUTH_CLIENT_ID_ENV, "client_test".into());
        env.insert(OAUTH_APP_ID_ENV, "oapp_test".into());
        env.insert(API_BASE_ENV, api_base.to_string());
        env
    }

    fn client_from(env: &HashMap<&'static str, String>) -> AnthropicClient {
        AnthropicClient::from_lookup(&|name: &str| env.get(name).cloned()).expect("configured")
    }

    /// Every variable this module reads, cleared so a test starts from nothing.
    fn clear_env() {
        for name in [LINK_ENV, PEM_ENV, ISSUER_BASE_ENV, API_BASE_ENV, AUTH_BASE_ENV].into_iter().chain(REQUIRED_ENV) {
            std::env::remove_var(name);
        }
    }

    /// GET a path on the real issuer routes: (status, JSON body).
    async fn served(path: &str) -> (u16, Value) {
        let app: axum::Router = crate::routes::anthropic_client_issuer::routes();
        let response = app.oneshot(axum::http::Request::builder().uri(path).body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn assertion_claims_match_contract() {
        clear_env();
        let pem = es256::testing::pem();
        std::env::set_var(PEM_ENV, &pem);
        let (status, jwks) = served("/oauth2/anthropic-client/jwks.json").await;
        clear_env();
        assert_eq!(status, 200);

        let client = client_from(&full_env(&pem, "http://127.0.0.1:9"));
        let first = client.client_assertion(AssertionEndpoint::Token);
        let (header, claims) = es256::verify_with_jwks(&first, &jwks).expect("verifies against the served JWKS");
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"].as_str(), Some(client.kid()), "kid present and in the JWKS");
        assert_eq!(jwks["keys"][0]["kid"].as_str(), Some(client.kid()));
        assert_eq!(claims["iss"], "https://api.allternit.com/oauth2/anthropic-client");
        assert_eq!(claims["sub"], "allternit-cloud-api");
        assert_eq!(claims["aud"], AUD, "exactly one audience, a string, not an array");
        let (iat, exp) = (claims["iat"].as_i64().unwrap(), claims["exp"].as_i64().unwrap());
        assert!(exp > iat && exp - iat <= 3600, "exp - iat = {}", exp - iat);
        assert!((iat - chrono::Utc::now().timestamp()).abs() <= 5);

        let second = client.client_assertion(AssertionEndpoint::Token);
        let (_, again) = es256::verify_with_jwks(&second, &jwks).unwrap();
        assert_ne!(claims["jti"], again["jti"], "every assertion has its own jti");

        let fields = client.client_assertion_fields(AssertionEndpoint::Par);
        assert_eq!(fields[0], ("client_id", "client_test".to_string()));
        assert_eq!(fields[1], ("client_assertion_type", "urn:ietf:params:oauth:client-assertion-type:jwt-bearer".to_string()));
        assert!(es256::verify_with_jwks(&fields[2].1, &jwks).is_ok());

        let (_, wif) = es256::verify_with_jwks(&client.wif_jwt(), &jwks).unwrap();
        assert_eq!((wif["iss"].as_str(), wif["aud"].as_str()), (Some("https://api.allternit.com/oauth2/anthropic-client"), Some(WIF_AUD)));
        assert!(wif["exp"].as_i64().unwrap() - wif["iat"].as_i64().unwrap() <= 3600);
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn assertion_verifies_against_served_jwks() {
        clear_env();
        let pem = es256::testing::pem();
        std::env::set_var(PEM_ENV, &pem);
        std::env::set_var(ISSUER_BASE_ENV, "https://api.example.test/");
        let (status, discovery) = served("/oauth2/anthropic-client/.well-known/openid-configuration").await;
        assert_eq!(status, 200);
        let jwks_path = discovery["jwks_uri"].as_str().unwrap().trim_start_matches("https://api.example.test").to_string();
        let (_, jwks) = served(&jwks_path).await;
        let mut env = full_env(&pem, "http://127.0.0.1:9");
        env.insert(ISSUER_BASE_ENV, "https://api.example.test/".into());
        clear_env();

        let client = client_from(&env);
        assert_eq!(discovery["issuer"].as_str(), Some(client.issuer()), "discovery names the issuer the assertions carry");
        let assertion = client.client_assertion(AssertionEndpoint::Revoke);
        let (_, claims) = es256::verify_with_jwks(&assertion, &jwks).expect("verifies against the JWKS that discovery points to");
        assert_eq!(claims["iss"], "https://api.example.test/oauth2/anthropic-client");

        // A different key (rotation without republishing) does not verify.
        let other = client_from(&full_env(&es256::testing::pem(), "http://127.0.0.1:9"));
        assert!(es256::verify_with_jwks(&other.client_assertion(AssertionEndpoint::Revoke), &jwks).is_err());
    }

    /// A fake `POST /v1/oauth/token`: counts exchanges, keeps the last body,
    /// answers with `expires_in` seconds, and waits `delay_ms` first.
    #[derive(Default)]
    struct FakeAnthropic {
        exchanges: AtomicUsize,
        expires_in: AtomicU64,
        delay_ms: AtomicU64,
        last_body: std::sync::Mutex<Option<Value>>,
    }

    async fn fake_anthropic(expires_in: u64, delay_ms: u64) -> (Arc<FakeAnthropic>, String) {
        let fake = Arc::new(FakeAnthropic::default());
        fake.expires_in.store(expires_in, Ordering::SeqCst);
        fake.delay_ms.store(delay_ms, Ordering::SeqCst);
        let state = fake.clone();
        let app = axum::Router::new().route(
            "/v1/oauth/token",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let fake = state.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(fake.delay_ms.load(Ordering::SeqCst))).await;
                    let n = fake.exchanges.fetch_add(1, Ordering::SeqCst) + 1;
                    *fake.last_body.lock().unwrap() = Some(body);
                    axum::Json(json!({
                        "access_token": format!("sk-ant-oat01-test-{n}"),
                        "token_type": "bearer",
                        "expires_in": fake.expires_in.load(Ordering::SeqCst),
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (fake, format!("http://{addr}"))
    }

    #[tokio::test]
    async fn operator_token_cached_and_refreshed_before_expiry() {
        let (fake, base) = fake_anthropic(3600, 0).await;
        let pem = es256::testing::pem();
        let client = client_from(&full_env(&pem, &base));
        let tokens = OperatorTokens::new();

        let first = tokens.get(&client).await.unwrap();
        assert_eq!(first.bearer(), "sk-ant-oat01-test-1");
        let body = fake.last_body.lock().unwrap().clone().unwrap();
        assert_eq!(body["grant_type"], "urn:ietf:params:oauth:grant-type:jwt-bearer");
        assert_eq!(
            (&body["federation_rule_id"], &body["organization_id"], &body["service_account_id"], &body["workspace_id"]),
            (&json!("fdrl_test"), &json!("00000000-0000-0000-0000-000000000001"), &json!("svac_test"), &json!("wrkspc_test"))
        );
        let (_, wif) = es256::verify_with_jwks(body["assertion"].as_str().unwrap(), &client.jwks()).expect("the assertion is our signed WIF JWT");
        assert_eq!(wif["aud"], WIF_AUD);
        assert!(!format!("{first:?}").contains("sk-ant"), "Debug never shows the token");

        // An hour left: the cache answers.
        assert_eq!(tokens.get(&client).await.unwrap().bearer(), "sk-ant-oat01-test-1");
        assert_eq!(fake.exchanges.load(Ordering::SeqCst), 1);

        // A 401 on that token drops it; the next call exchanges again.
        tokens.invalidate(&first).await;
        let second = tokens.get(&client).await.unwrap();
        assert_eq!(second.bearer(), "sk-ant-oat01-test-2");
        // Invalidating the old token again leaves the new one cached.
        tokens.invalidate(&first).await;
        assert_eq!(tokens.get(&client).await.unwrap().bearer(), "sk-ant-oat01-test-2");
        assert_eq!(fake.exchanges.load(Ordering::SeqCst), 2);

        // A token with under 120 s left is re-exchanged on the next call.
        let (short, base) = fake_anthropic(60, 0).await;
        let client = client_from(&full_env(&pem, &base));
        let tokens = OperatorTokens::new();
        assert_eq!(tokens.get(&client).await.unwrap().bearer(), "sk-ant-oat01-test-1");
        assert_eq!(tokens.get(&client).await.unwrap().bearer(), "sk-ant-oat01-test-2");
        assert_eq!(short.exchanges.load(Ordering::SeqCst), 2);
        let (_, a) = es256::decode_unverified(short.last_body.lock().unwrap().clone().unwrap()["assertion"].as_str().unwrap()).unwrap();
        assert!(a["jti"].as_str().unwrap().starts_with("wif-"), "each exchange signs a new WIF JWT");
    }

    #[tokio::test]
    async fn operator_token_single_flight() {
        let (fake, base) = fake_anthropic(3600, 150).await;
        let client = client_from(&full_env(&es256::testing::pem(), &base));
        let tokens = OperatorTokens::new();
        let results = futures::future::join_all((0..5).map(|_| tokens.get(&client))).await;
        let bearers: Vec<String> = results.into_iter().map(|r| r.unwrap().bearer().to_string()).collect();
        assert_eq!(fake.exchanges.load(Ordering::SeqCst), 1, "five concurrent calls make one exchange");
        assert!(bearers.iter().all(|b| b == "sk-ant-oat01-test-1"), "{bearers:?}");
    }

    #[tokio::test]
    async fn operator_token_refused_exchange_is_an_error_and_caches_nothing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/v1/oauth/token",
            axum::routing::post(|| async { (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({ "error": "invalid_grant" }))) }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = client_from(&full_env(&es256::testing::pem(), &format!("http://{addr}")));
        let tokens = OperatorTokens::new();
        assert!(matches!(tokens.get(&client).await, Err(OperatorTokenError::Refused { status: 401 })));
        assert!(tokens.cached.lock().await.is_none());
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn not_configured_without_env() {
        clear_env();
        assert!(!is_configured());
        let missing = AnthropicClient::from_env().err().unwrap().missing;
        assert!(missing.contains(&LINK_ENV) && missing.contains(&PEM_ENV) && REQUIRED_ENV.iter().all(|n| missing.contains(n)), "{missing:?}");
        assert_eq!(AnthropicClient::from_env().err().unwrap().code(), "not_available_yet");

        let pem = es256::testing::pem();
        let env = full_env(&pem, "https://api.anthropic.com");
        for (name, value) in &env {
            std::env::set_var(name, value);
        }
        assert!(is_configured(), "every variable set and the switch on");

        // Each required variable alone turns it off, and so does the switch.
        for name in REQUIRED_ENV.into_iter().chain([PEM_ENV]) {
            std::env::remove_var(name);
            assert!(!is_configured(), "{name} missing");
            assert_eq!(AnthropicClient::from_env().err().unwrap().missing, vec![name]);
            std::env::set_var(name, &env[name]);
        }
        std::env::set_var(LINK_ENV, "0");
        assert!(!is_configured(), "the switch off");
        std::env::set_var(LINK_ENV, "1");
        std::env::set_var(PEM_ENV, "-----BEGIN PRIVATE KEY-----\nnot a key\n-----END PRIVATE KEY-----");
        assert!(!is_configured(), "a PEM that does not parse");
        clear_env();
    }

    #[tokio::test]
    #[serial_test::serial(anthropic_client_env)]
    async fn jwks_503_without_pem() {
        clear_env();
        for path in ["/oauth2/anthropic-client/jwks.json", "/oauth2/anthropic-client/.well-known/openid-configuration"] {
            let (status, body) = served(path).await;
            assert_eq!((status, body["error"]["code"].as_str()), (503, Some("not_configured")), "{path}");
        }
        // The issuer serves with the PEM alone (registration at Anthropic
        // comes before the switch is turned on).
        std::env::set_var(PEM_ENV, es256::testing::pem());
        let (status, jwks) = served("/oauth2/anthropic-client/jwks.json").await;
        assert_eq!(status, 200);
        assert_eq!(jwks["keys"][0]["kid"].as_str(), signing_key().as_ref().map(|k| k.kid()));
        assert!(jwks["keys"][0].get("d").is_none(), "no private part");
        let (status, discovery) = served("/oauth2/anthropic-client/.well-known/openid-configuration").await;
        assert_eq!(status, 200);
        assert_eq!(
            discovery,
            json!({
                "issuer": "https://api.allternit.com/oauth2/anthropic-client",
                "jwks_uri": "https://api.allternit.com/oauth2/anthropic-client/jwks.json",
            })
        );
        clear_env();
    }

    #[tokio::test]
    async fn migration_151_applies_twice() {
        let db = crate::routes::test_support::test_pool().await;
        // 151 widens inference_usage; a minimal one stands in for 007.
        sqlx::raw_sql("CREATE TABLE inference_usage (id TEXT PRIMARY KEY, created_at TIMESTAMPTZ DEFAULT now())").execute(&db).await.unwrap();
        let sql = include_str!("../../migrations_pg/151_anthropic_links.sql").replace("public.", "");
        sqlx::raw_sql(&sql).execute(&db).await.expect("151 applies");
        sqlx::raw_sql(&sql).execute(&db).await.expect("151 re-applies");

        let checks: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_constraint WHERE conname = 'inference_usage_billed_to_check' AND conrelid = 'inference_usage'::regclass")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(checks, 1, "the billed_to check is added once");
        sqlx::query("INSERT INTO inference_usage (id, billed_to, oauth_app_id, oauth_grant_id) VALUES ('u1', 'claude_org', $1, $2)")
            .bind(uuid::Uuid::new_v4())
            .bind(uuid::Uuid::new_v4())
            .execute(&db)
            .await
            .unwrap();
        assert!(sqlx::query("INSERT INTO inference_usage (id, billed_to) VALUES ('u2', 'someone_else')").execute(&db).await.is_err());

        // Half a sealed token is refused; a link with none is fine.
        assert!(sqlx::query("INSERT INTO anthropic_connections (user_id, user_profile_id, refresh_token_ciphertext) VALUES ('a', 'uprof_a', '\\x01'::bytea)")
            .execute(&db)
            .await
            .is_err());
        sqlx::query("INSERT INTO anthropic_connections (user_id, user_profile_id) VALUES ('a', 'uprof_a')").execute(&db).await.unwrap();
        let (status, expires_ok): (String, bool) =
            sqlx::query_as("SELECT status, refresh_expires_at > now() + INTERVAL '364 days' FROM anthropic_connections WHERE user_id = 'a'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!((status.as_str(), expires_ok), ("linked", true));
        assert!(sqlx::query("UPDATE anthropic_connections SET status = 'paused' WHERE user_id = 'a'").execute(&db).await.is_err());
    }
}
