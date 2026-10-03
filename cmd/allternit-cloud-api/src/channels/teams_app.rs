//! Microsoft Teams shared app (ao-teams).
//!
//! One single-tenant Azure Bot in Allternit's tenant + one multi-tenant Entra
//! app registration (Microsoft blocks new multi-tenant Azure Bot registrations
//! since 2025-07-31); one Teams app package is installed per customer tenant.
//!
//! * Inbound: the Azure Bot's messaging endpoint `POST /channels/teams/messages`.
//!   The Bot Framework JWT is validated **at the edge** (OpenID metadata from
//!   login.botframework.com, issuer `https://api.botframework.com`, audience =
//!   the bot's app id, 5-minute skew, `serviceurl` claim match), conversation
//!   references are stored from every activity, and the activity is queued for
//!   the owning user's runtime (delivery over the runtime relay, waking it).
//! * Outbound: `POST /api/v1/channels/teams/send` replies through a stored
//!   conversation reference, using a token from Allternit's own tenant token
//!   endpoint. Whether a single-tenant bot can send proactively into another
//!   tenant is UNVERIFIED (two-tenant spike pending), so proactive send is a
//!   separate code path behind `TEAMS_PROACTIVE_SEND`, off by default.
//! * Connect: `POST /api/v1/channels/teams/connect` mints a Microsoft sign-in
//!   URL (multi-tenant `common`, delegated); the callback exchanges the code
//!   and records (tenant, user). Optional admin paths proxy Graph with the
//!   admin's own delegated token: catalog upload and per-user install.
//!
//! Secrets (APP_ID / APP_PASSWORD / TENANT_ID) live only in cloud-api env
//! vars; they never reach a runtime, a log, or Postgres. When unset, every
//! route answers 503 `{error:"teams_not_configured"}`.

use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::auth::resolve_user_scoped;
use crate::routes::channel_inbound::{backoff_secs, classify, Delivery, QUEUED_AT_HEADER};
use crate::routes::runtime_relay::{relay_signed_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

/// Where the runtime receives queued Teams-app activities (trusted relay only).
pub(crate) const RUNTIME_PATH: &str = "/webhooks/teams-app";
/// Still sent so a runtime that predates signed relays keeps working through a
/// staggered rollout. Updated runtimes ignore it: the owner is the signed one.
const USER_HEADER: &str = "x-allternit-user-id";

const OPENID_URL: &str = "https://login.botframework.com/v1/.well-known/openidconfiguration";
const BOT_ISSUER: &str = "https://api.botframework.com";
const BOT_SCOPE: &str = "https://api.botframework.com/.default";
const ENTRA_COMMON_TOKEN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const ENTRA_COMMON_KEYS: &str = "https://login.microsoftonline.com/common/discovery/v2.0/keys";
const ENTRA_AUTHORIZE: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
/// 5-minute clock skew, per the Bot Framework validation contract.
const LEEWAY_SECS: u64 = 300;
/// A queued activity is retried for at most 24 hours, like channel_inbound.
const GIVE_UP_AFTER_HOURS: i64 = 24;
const WORKER_INTERVAL: Duration = Duration::from_secs(5);
const STATE_TTL_MINUTES: i64 = 10;

// ------------------------------------------------------------------- config

#[derive(Debug, Clone)]
pub(crate) struct TeamsConfig {
    pub app_id: String,
    pub app_password: String,
    /// None = the classic `botframework.com` token tenant (multi-tenant bot);
    /// Some = Allternit's own tenant (single-tenant bot).
    pub tenant_id: Option<String>,
    /// Whether proactive send into stored conversation refs is allowed.
    /// Off by default: the two-tenant proactive spike has not been run.
    pub proactive: bool,
    /// Where the connect callback redirects the browser afterwards.
    pub app_url: String,
}

impl TeamsConfig {
    fn from_env() -> Option<Self> {
        let app_id = std::env::var("APP_ID").ok().filter(|s| !s.is_empty())?;
        let app_password = std::env::var("APP_PASSWORD").ok().filter(|s| !s.is_empty())?;
        Some(TeamsConfig {
            app_id,
            app_password,
            tenant_id: std::env::var("TENANT_ID").ok().filter(|s| !s.is_empty()),
            proactive: matches!(
                std::env::var("TEAMS_PROACTIVE_SEND").unwrap_or_default().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            app_url: std::env::var("ALLTERNIT_APP_URL")
                .unwrap_or_else(|_| "https://app.allternit.com".to_string())
                .trim_end_matches('/')
                .to_string(),
        })
    }
}

fn config() -> Option<&'static TeamsConfig> {
    static CONFIG: OnceLock<Option<TeamsConfig>> = OnceLock::new();
    CONFIG.get_or_init(TeamsConfig::from_env).as_ref()
}

fn public_base() -> String {
    std::env::var("ALLTERNIT_CLOUD_API_URL")
        .unwrap_or_else(|_| "https://api.allternit.com".to_string())
        .trim_end_matches('/')
        .to_string()
}

fn not_configured() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "teams_not_configured" })),
    )
        .into_response()
}

// ---------------------------------------------------------------- http seam

/// Every outbound HTTP call this module makes (JWKS, token endpoints, the
/// Bot Connector reply, Graph), behind one trait so tests run fully offline.
#[async_trait]
pub(crate) trait TeamsHttp: Send + Sync {
    async fn get_json(&self, url: &str) -> Result<(u16, Value), String>;
    async fn post_json(&self, url: &str, headers: &[(&str, &str)], body: &Value) -> Result<(u16, Value), String>;
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String>;
    async fn post_bytes(&self, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<(u16, Value), String>;
}

pub(crate) struct ReqwestTeamsHttp;

#[async_trait]
impl TeamsHttp for ReqwestTeamsHttp {
    async fn get_json(&self, url: &str) -> Result<(u16, Value), String> {
        let resp = reqwest::Client::new().get(url).timeout(Duration::from_secs(15)).send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        Ok((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
    }
    async fn post_json(&self, url: &str, headers: &[(&str, &str)], body: &Value) -> Result<(u16, Value), String> {
        let mut r = reqwest::Client::new().post(url).timeout(Duration::from_secs(15)).json(body);
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        let resp = r.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        Ok((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
    }
    async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String> {
        let resp = reqwest::Client::new().post(url).timeout(Duration::from_secs(15)).form(form).send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        Ok((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
    }
    async fn post_bytes(&self, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<(u16, Value), String> {
        let mut r = reqwest::Client::new().post(url).timeout(Duration::from_secs(30)).body(body.to_vec());
        for (k, v) in headers {
            r = r.header(*k, *v);
        }
        let resp = r.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        Ok((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
    }
}

// ------------------------------------------------------------------ jwt edge

fn jwt_header_kid(token: &str) -> Result<String, String> {
    let header_b64 = token.split('.').next().ok_or("malformed token")?;
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).map_err(|_| "malformed token header")?)
        .map_err(|_| "malformed token header")?;
    header.get("kid").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "token has no kid".to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Who may have issued the token.
#[derive(Debug, PartialEq)]
enum IssuerCheck {
    /// Exact issuer string (Bot Framework).
    Exact(&'static str),
    /// Entra v2 issuer: `https://login.microsoftonline.com/{tenant}/v2.0`.
    EntraV2,
}

/// RS256 verify + issuer + audience + expiry/not-before (5-minute skew),
/// returning the raw claims. Mirrors `auth::clerk::verify_rs256` (jsonwebtoken
/// v10 cannot be used — see there) with an audience check added.
fn verify_rs256(
    token: &str,
    n_b64: &str,
    e_b64: &str,
    issuer: IssuerCheck,
    audience: &str,
) -> Result<Value, String> {
    let mut parts = token.split('.');
    let (header_b64, payload_b64, signature_b64) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => (h, p, s),
        _ => return Err("malformed token".into()),
    };
    let signature = URL_SAFE_NO_PAD.decode(signature_b64).map_err(|_| "malformed signature")?;
    let n = URL_SAFE_NO_PAD.decode(n_b64).map_err(|_| "bad signing key")?;
    let e = URL_SAFE_NO_PAD.decode(e_b64).map_err(|_| "bad signing key")?;
    let public_key = aws_lc_rs::signature::RsaPublicKeyComponents { n: &n, e: &e };
    public_key
        .verify(
            &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
            format!("{header_b64}.{payload_b64}").as_bytes(),
            &signature,
        )
        .map_err(|_| "invalid signature")?;

    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| "malformed payload")?)
        .map_err(|_| "malformed payload")?;
    let iss = claims.get("iss").and_then(Value::as_str).unwrap_or("");
    let iss_ok = match issuer {
        IssuerCheck::Exact(want) => iss == want,
        IssuerCheck::EntraV2 => iss.starts_with("https://login.microsoftonline.com/") && iss.ends_with("/v2.0"),
    };
    if !iss_ok {
        return Err("invalid issuer".into());
    }
    let aud_ok = claims.get("aud").and_then(Value::as_str).map(|aud| aud == audience).unwrap_or(false);
    if !aud_ok {
        return Err("invalid audience".into());
    }
    let now = now_secs();
    match claims.get("exp").and_then(Value::as_u64) {
        Some(exp) if now <= exp + LEEWAY_SECS => {}
        _ => return Err("invalid or expired token".into()),
    }
    if let Some(nbf) = claims.get("nbf").and_then(Value::as_u64) {
        if now + LEEWAY_SECS < nbf {
            return Err("token not yet valid".into());
        }
    }
    Ok(claims)
}

/// JWKS cache keyed by `kid`, refetched on unknown kid (rate-limited). The
/// keys URL is either fixed (Entra common discovery) or discovered once from
/// an OpenID metadata document (Bot Framework).
struct JwksCache {
    http: Arc<dyn TeamsHttp>,
    keys_url: Mutex<Option<String>>,
    openid_url: Option<String>,
    keys: tokio::sync::RwLock<Option<(Instant, HashMap<String, (String, String)>)>>,
    min_refetch: Duration,
}

impl JwksCache {
    fn fixed(http: Arc<dyn TeamsHttp>, keys_url: &str, min_refetch: Duration) -> Self {
        JwksCache {
            http,
            keys_url: Mutex::new(Some(keys_url.into())),
            openid_url: None,
            keys: tokio::sync::RwLock::new(None),
            min_refetch,
        }
    }

    fn discovering(http: Arc<dyn TeamsHttp>, openid_url: &str, min_refetch: Duration) -> Self {
        JwksCache {
            http,
            keys_url: Mutex::new(None),
            openid_url: Some(openid_url.into()),
            keys: tokio::sync::RwLock::new(None),
            min_refetch,
        }
    }

    async fn resolved_url(&self) -> Result<String, String> {
        if let Some(url) = self.keys_url.lock().unwrap().clone() {
            return Ok(url);
        }
        let openid = self.openid_url.as_ref().ok_or("no JWKS url configured")?;
        let (status, meta) = self.http.get_json(openid).await?;
        if status != 200 {
            return Err(format!("OpenID metadata returned {status}"));
        }
        let uri = meta["jwks_uri"].as_str().ok_or("OpenID metadata has no jwks_uri")?.to_string();
        *self.keys_url.lock().unwrap() = Some(uri.clone());
        Ok(uri)
    }

    async fn fetch(&self) -> Result<HashMap<String, (String, String)>, String> {
        let url = self.resolved_url().await?;
        let (status, body) = self.http.get_json(&url).await?;
        if status != 200 {
            return Err(format!("JWKS endpoint returned {status}"));
        }
        let keys: HashMap<String, (String, String)> = body
            .get("keys")
            .and_then(Value::as_array)
            .map(|keys| {
                keys.iter()
                    .filter(|k| k["kty"] == "RSA")
                    .filter_map(|k| match (k["kid"].as_str(), k["n"].as_str(), k["e"].as_str()) {
                        (Some(kid), Some(n), Some(e)) => Some((kid.to_string(), (n.to_string(), e.to_string()))),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        *self.keys.write().await = Some((Instant::now(), keys.clone()));
        Ok(keys)
    }

    async fn key(&self, kid: &str) -> Result<(String, String), String> {
        let (cached, age) = {
            let g = self.keys.read().await;
            match g.as_ref() {
                Some((at, keys)) => (keys.get(kid).cloned(), Some(at.elapsed())),
                None => (None, None),
            }
        };
        if let (Some(k), Some(age)) = (&cached, age) {
            if age < Duration::from_secs(24 * 3600) {
                return Ok(k.clone());
            }
        }
        if cached.is_none() && age.map_or(false, |a| a < self.min_refetch) {
            return Err("unknown signing key".into());
        }
        match self.fetch().await {
            Ok(keys) => keys.get(kid).cloned().ok_or_else(|| "unknown signing key".to_string()),
            Err(e) => cached.ok_or(e),
        }
    }
}

/// Bot Framework edge validation: OpenID metadata -> JWKS -> RS256 + issuer +
/// audience + skew + `serviceurl` claim match against the activity's
/// `serviceUrl`.
pub(crate) struct BotFrameworkAuth {
    app_id: String,
    jwks: JwksCache,
}

impl BotFrameworkAuth {
    pub(crate) fn for_bot_framework(http: Arc<dyn TeamsHttp>, app_id: &str) -> Self {
        BotFrameworkAuth { app_id: app_id.into(), jwks: JwksCache::discovering(http, OPENID_URL, Duration::from_secs(300)) }
    }

    pub(crate) async fn validate(&self, authorization: &str, activity: &Value) -> Result<Value, String> {
        let jwt = authorization.strip_prefix("Bearer ").ok_or("missing bearer token")?.trim();
        if jwt_header_alg(jwt)? != "RS256" {
            return Err("unsupported token algorithm".into());
        }
        let kid = jwt_header_kid(jwt)?;
        let (n, e) = self.jwks.key(&kid).await?;
        let claims = verify_rs256(jwt, &n, &e, IssuerCheck::Exact(BOT_ISSUER), &self.app_id)?;
        if let (Some(claim), Some(svc)) = (claims["serviceurl"].as_str(), activity["serviceUrl"].as_str()) {
            if claim.trim_end_matches('/') != svc.trim_end_matches('/') {
                return Err("serviceUrl does not match the token".into());
            }
        }
        Ok(claims)
    }
}

fn jwt_header_alg(token: &str) -> Result<String, String> {
    let header_b64 = token.split('.').next().ok_or("malformed token")?;
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_b64).map_err(|_| "malformed token header")?)
        .map_err(|_| "malformed token header")?;
    header.get("alg").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "token has no alg".to_string())
}

fn production_auth(cfg: &TeamsConfig) -> Arc<BotFrameworkAuth> {
    static AUTH: OnceLock<Mutex<HashMap<String, Arc<BotFrameworkAuth>>>> = OnceLock::new();
    AUTH.get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(cfg.app_id.clone())
        .or_insert_with(|| Arc::new(BotFrameworkAuth::for_bot_framework(Arc::new(ReqwestTeamsHttp), &cfg.app_id)))
        .clone()
}

// ------------------------------------------------------------- bot token out

/// Client-credentials token for the Bot Connector, from Allternit's own
/// tenant (single-tenant bot) or `botframework.com` (multi-tenant).
struct TeamsTokenCache {
    http: Arc<dyn TeamsHttp>,
    app_id: String,
    app_password: String,
    tenant: String,
    token: Mutex<Option<(String, Instant)>>,
}

impl TeamsTokenCache {
    fn new(http: Arc<dyn TeamsHttp>, cfg: &TeamsConfig) -> Self {
        TeamsTokenCache {
            http,
            app_id: cfg.app_id.clone(),
            app_password: cfg.app_password.clone(),
            tenant: cfg.tenant_id.clone().unwrap_or_else(|| "botframework.com".to_string()),
            token: Mutex::new(None),
        }
    }

    async fn access_token(&self) -> Result<String, ApiError> {
        if let Some((t, exp)) = self.token.lock().unwrap().as_ref() {
            if Instant::now() < *exp {
                return Ok(t.clone());
            }
        }
        let url = format!("https://login.microsoftonline.com/{}/oauth2/v2.0/token", self.tenant);
        let (status, body) = self
            .http
            .post_form(
                &url,
                &[
                    ("grant_type", "client_credentials"),
                    ("client_id", &self.app_id),
                    ("client_secret", &self.app_password),
                    ("scope", BOT_SCOPE),
                ],
            )
            .await
            .map_err(|e| ApiError::Internal(format!("Teams token endpoint unreachable: {e}")))?;
        if status != 200 {
            return Err(ApiError::Internal(format!("Teams token endpoint returned {status}")));
        }
        let t = body["access_token"].as_str().ok_or_else(|| ApiError::Internal("token response has no access_token".into()))?.to_string();
        let ttl = body["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
        *self.token.lock().unwrap() = Some((t.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(t)
    }
}

fn production_tokens(cfg: &TeamsConfig) -> Arc<TeamsTokenCache> {
    static TOKENS: OnceLock<Mutex<HashMap<String, Arc<TeamsTokenCache>>>> = OnceLock::new();
    TOKENS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(cfg.app_id.clone())
        .or_insert_with(|| Arc::new(TeamsTokenCache::new(Arc::new(ReqwestTeamsHttp), cfg)))
        .clone()
}

// ---------------------------------------------------------------- activities

fn s_of(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(|x| x.as_str().map(str::to_string))
}

/// The Bot Framework tenant an activity belongs to.
fn tenant_of(activity: &Value) -> Option<String> {
    s_of(activity, "/channelData/tenant/id").or_else(|| s_of(activity, "/conversation/tenantId"))
}

fn conversation_of(activity: &Value) -> Option<String> {
    s_of(activity, "/conversation/id")
}

/// Whether this activity type carries a chat event the runtime should see.
fn is_routable_activity(activity: &Value) -> bool {
    matches!(
        activity["type"].as_str(),
        Some("message") | Some("messageUpdate") | Some("messageDelete") | Some("messageReaction")
    )
}

/// Build the Bot Connector reply activity. The answering bot's name rides in
/// an Adaptive Card header so a shared connection's chats can tell bots apart.
fn build_reply_activity(text: &str, bot_name: Option<&str>, reply_to_id: Option<&str>) -> Value {
    let mut activity = match bot_name {
        Some(name) if !name.is_empty() => json!({
            "type": "message",
            "attachments": [{
                "contentType": "application/vnd.microsoft.card.adaptive",
                "content": {
                    "type": "AdaptiveCard",
                    "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                    "version": "1.4",
                    "body": [
                        { "type": "Container", "style": "emphasis", "items": [
                            { "type": "TextBlock", "text": name, "weight": "Bolder", "size": "Medium" }
                        ] },
                        { "type": "TextBlock", "text": text, "wrap": true }
                    ]
                }
            }]
        }),
        _ => json!({ "type": "message", "text": text }),
    };
    if let Some(id) = reply_to_id.filter(|s| !s.is_empty()) {
        activity["replyToId"] = json!(id);
    }
    activity
}

/// Percent-encode for a query string (unreserved characters kept).
fn pct_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// The Microsoft sign-in URL for the connect flow (multi-tenant, delegated).
fn connect_url(app_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "{ENTRA_AUTHORIZE}?client_id={}&response_type=code&response_mode=query&scope=openid%20profile&redirect_uri={}&state={}",
        pct_encode(app_id),
        pct_encode(redirect_uri),
        pct_encode(state),
    )
}

// ------------------------------------------------------------------- routes

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/channels/teams/messages", post(messages_h))
        .route("/channels/teams/callback", get(callback_h))
        .route("/api/v1/channels/teams/send", post(send_h))
        .route("/api/v1/channels/teams/connect", post(connect_h))
        .route("/api/v1/channels/teams/register", post(register_h))
        .route("/api/v1/channels/teams/catalog-upload", post(catalog_upload_h))
        .route("/api/v1/channels/teams/install-app", post(install_app_h))
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.starts_with("Bearer "))
        .map(str::to_string)
}

/// The runtime calls these routes in the background with its paired runtime
/// device token; the connect wizard calls them in a browser with a Clerk
/// session. Accept whichever credential authenticated.
async fn resolve_caller(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    match resolve_user_scoped(&state.db, headers, "compute").await {
        Ok(user) => Ok(user.id),
        Err(session_err) => {
            let Some(token) = crate::routes::runtime_pairing::device_token_from_headers(headers) else {
                return Err(session_err);
            };
            let device = crate::routes::runtime_pairing::runtime_device_for_token(&state.db, token, None).await?;
            Ok(device.user_id)
        }
    }
}

/// The Azure Bot messaging endpoint. Always acks 200 after the edge check so
/// Azure does not retry-storm; delivery to the runtime is the queue's job.
async fn messages_h(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(cfg) = config() else { return not_configured() };
    let Ok(activity) = serde_json::from_slice::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_json" }))).into_response();
    };
    let Some(authz) = bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing_token" }))).into_response();
    };
    let auth = production_auth(cfg);
    if let Err(err) = auth.validate(&authz, &activity).await {
        tracing::warn!("teams app: rejected activity: {err}");
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "invalid_token" }))).into_response();
    }
    let tenant = match tenant_of(&activity) {
        Some(t) => t,
        None => {
            tracing::warn!("teams app: activity without a tenant; dropped");
            return Json(json!({ "ok": true })).into_response();
        }
    };
    // Refresh the conversation reference on every activity (serviceUrl moves).
    if let (Some(conv), Some(service_url)) = (conversation_of(&activity), s_of(&activity, "/serviceUrl")) {
        if let Err(e) = sqlx::query(
            "INSERT INTO teams_conversation_refs (tenant_id, conversation_id, service_url, name, conversation_type)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (tenant_id, conversation_id) DO UPDATE
             SET service_url = EXCLUDED.service_url,
                 name = COALESCE(EXCLUDED.name, teams_conversation_refs.name),
                 conversation_type = COALESCE(EXCLUDED.conversation_type, teams_conversation_refs.conversation_type),
                 updated_at = now()",
        )
        .bind(&tenant)
        .bind(&conv)
        .bind(&service_url)
        .bind(s_of(&activity, "/conversation/name"))
        .bind(s_of(&activity, "/conversation/conversationType"))
        .execute(&state.db)
        .await
        {
            tracing::warn!("teams app: conversation ref upsert failed: {e}");
        }
    }
    if !is_routable_activity(&activity) {
        return Json(json!({ "ok": true })).into_response();
    }
    let known: Option<(String,)> = sqlx::query_as("SELECT id FROM teams_installs WHERE tenant_id = $1")
        .bind(&tenant)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    if known.is_none() {
        tracing::warn!(%tenant, "teams app: activity for a tenant that never connected; dropped");
        return Json(json!({ "ok": true })).into_response();
    }
    if let Err(e) = sqlx::query("INSERT INTO teams_app_queue (tenant_id, conversation_id, activity) VALUES ($1, $2, $3)")
        .bind(&tenant)
        .bind(conversation_of(&activity).unwrap_or_default())
        .bind(&activity)
        .execute(&state.db)
        .await
    {
        tracing::warn!("teams app: queue insert failed: {e}");
    }
    Json(json!({ "ok": true })).into_response()
}

/// Whether a send request must be refused as proactive while the proactive
/// path is off (the default until the two-tenant spike proves it works).
fn proactive_refused(proactive_configured: bool, requested: Option<bool>) -> bool {
    requested == Some(true) && !proactive_configured
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    conversation_id: String,
    tenant_id: Option<String>,
    text: String,
    bot_name: Option<String>,
    reply_to_id: Option<String>,
    proactive: Option<bool>,
}

/// Reply to a stored conversation reference. Proactive send is a separate,
/// flagged path (TEAMS_PROACTIVE_SEND) and refuses by default.
async fn send_h(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SendBody>) -> Result<Response, ApiError> {
    let Some(cfg) = config() else { return Ok(not_configured()) };
    let user = resolve_caller(&state, &headers).await?;
    if proactive_refused(cfg.proactive, body.proactive) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "teams_proactive_disabled", "message": "Proactive send needs the two-tenant spike; it stays off until then." })),
        )
            .into_response());
    }
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT r.service_url, r.tenant_id FROM teams_conversation_refs r
          JOIN teams_installs i ON i.tenant_id = r.tenant_id
         WHERE r.conversation_id = $1 AND i.user_id = $2
         ORDER BY r.updated_at DESC LIMIT 1",
    )
    .bind(&body.conversation_id)
    .bind(&user)
    .fetch_optional(&state.db)
    .await?;
    let (service_url, _tenant) = row.ok_or_else(|| ApiError::NotFound("conversation reference not found".to_string()))?;
    let tokens = production_tokens(cfg);
    let token = tokens.access_token().await?;
    let activity = build_reply_activity(&body.text, body.bot_name.as_deref(), body.reply_to_id.as_deref());
    let url = format!("{}/v3/conversations/{}/activities", service_url.trim_end_matches('/'), body.conversation_id);
    let (status, resp) = ReqwestTeamsHttp
        .post_json(&url, &[("Authorization", &format!("Bearer {token}"))], &activity)
        .await
        .map_err(|e| ApiError::Internal(format!("Bot Connector unreachable: {e}")))?;
    if !(200..300).contains(&status) {
        return Err(ApiError::Internal(format!("Bot Connector returned {status}")));
    }
    Ok(Json(json!({ "id": resp["id"].as_str().unwrap_or_default(), "ok": true })).into_response())
}

/// Activity ids in the path tolerate `:` and `@` unencoded by the Connector;
/// keep them literal (Teams conversation ids look like `19:…@thread.tacv2`).

/// Mint the Microsoft sign-in URL for the connect wizard step.
async fn connect_h(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let Some(cfg) = config() else { return Ok(not_configured()) };
    let user = resolve_user_scoped(&state.db, &headers, "compute").await?;
    let state_id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO teams_app_states (state, user_id, expires_at) VALUES ($1, $2, now() + make_interval(mins => $3))")
        .bind(&state_id)
        .bind(&user.id)
        .bind(STATE_TTL_MINUTES as f64)
        .execute(&state.db)
        .await?;
    let redirect_uri = format!("{}/channels/teams/callback", public_base());
    Ok(Json(json!({
        "url": connect_url(&cfg.app_id, &redirect_uri, &state_id),
        "state": state_id,
        "expiresInSeconds": STATE_TTL_MINUTES * 60,
    }))
    .into_response())
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: String,
    state: String,
}

/// OAuth callback: exchange the code, validate the id_token, record the
/// (tenant, user) install, redirect the browser to the workspace.
async fn callback_h(State(state): State<Arc<ApiState>>, Query(q): Query<CallbackQuery>) -> Response {
    let Some(cfg) = config() else { return not_configured() };
    let redirect = |params: &str| {
        let sep = if cfg.app_url.contains('?') { '&' } else { '?' };
        (
            StatusCode::SEE_OTHER,
            [("location", format!("{}{sep}{params}", cfg.app_url))],
        )
            .into_response()
    };
    let row: Option<(String,)> = sqlx::query_as("DELETE FROM teams_app_states WHERE state = $1 AND expires_at > now() RETURNING user_id")
        .bind(&q.state)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    let Some((user_id,)) = row else {
        return redirect("teams=error&reason=expired_state");
    };
    let redirect_uri = format!("{}/channels/teams/callback", public_base());
    let (status, tokens) = match ReqwestTeamsHttp
        .post_form(
            ENTRA_COMMON_TOKEN,
            &[
                ("grant_type", "authorization_code"),
                ("code", &q.code),
                ("client_id", &cfg.app_id),
                ("client_secret", &cfg.app_password),
                ("redirect_uri", &redirect_uri),
                ("scope", "openid profile"),
            ],
        )
        .await
    {
        Ok(r) => r,
        Err(_) => return redirect("teams=error&reason=token_exchange"),
    };
    if status != 200 {
        return redirect("teams=error&reason=token_exchange");
    }
    let id_token = match tokens["id_token"].as_str() {
        Some(t) => t,
        None => return redirect("teams=error&reason=no_id_token"),
    };
    let claims = match validate_entra_id_token(Arc::new(ReqwestTeamsHttp), &cfg.app_id, id_token).await {
        Ok(c) => c,
        Err(_) => return redirect("teams=error&reason=bad_id_token"),
    };
    let tenant = match claims["tid"].as_str().or_else(|| entra_tenant_from_iss(claims["iss"].as_str())) {
        Some(t) => t.to_string(),
        None => return redirect("teams=error&reason=no_tenant"),
    };
    let installed_by = claims["oid"].as_str().or_else(|| claims["preferred_username"].as_str()).unwrap_or_default();
    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query(
        "INSERT INTO teams_installs (id, tenant_id, user_id, installed_by) VALUES ($1, $2, $3, $4)
         ON CONFLICT (tenant_id) DO UPDATE SET user_id = EXCLUDED.user_id, installed_by = EXCLUDED.installed_by, updated_at = now()",
    )
    .bind(&id)
    .bind(&tenant)
    .bind(&user_id)
    .bind(installed_by)
    .execute(&state.db)
    .await
    {
        tracing::warn!("teams app: install upsert failed: {e}");
    }
    tracing::info!(%tenant, %user_id, "teams app: tenant connected");
    redirect(&format!("teams=connected&tenant={}", pct_encode(&tenant)))
}

fn entra_tenant_from_iss(iss: Option<&str>) -> Option<&str> {
    let rest = iss?.strip_prefix("https://login.microsoftonline.com/")?;
    rest.trim_end_matches("/v2.0").split('/').next().filter(|s| !s.is_empty())
}

async fn validate_entra_id_token(http: Arc<dyn TeamsHttp>, app_id: &str, token: &str) -> Result<Value, String> {
    if jwt_header_alg(token)? != "RS256" {
        return Err("unsupported token algorithm".into());
    }
    let kid = jwt_header_kid(token)?;
    let cache = JwksCache::fixed(http, ENTRA_COMMON_KEYS, Duration::ZERO);
    let (n, e) = cache.key(&kid).await?;
    verify_rs256(token, &n, &e, IssuerCheck::EntraV2, app_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterBody {
    runtime_id: String,
}

/// Bind the caller's installs to the runtime that should receive their Teams
/// activities (the Desktop/cloud computer registers itself on boot).
async fn register_h(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<RegisterBody>) -> Result<Response, ApiError> {
    let Some(_cfg) = config() else { return Ok(not_configured()) };
    let user = resolve_caller(&state, &headers).await?;
    let owns: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(&body.runtime_id)
    .bind(&user)
    .fetch_optional(&state.db)
    .await?;
    if owns.is_none() {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    let result = sqlx::query("UPDATE teams_installs SET runtime_id = $1, updated_at = now() WHERE user_id = $2")
        .bind(&body.runtime_id)
        .bind(&user)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true, "installs": result.rows_affected() })).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogUploadBody {
    graph_token: String,
    package_base64: String,
}

/// Optional admin path: publish the Teams app package to the admin's tenant
/// catalog with the admin's own delegated Graph token.
async fn catalog_upload_h(State(_state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<CatalogUploadBody>) -> Result<Response, ApiError> {
    let Some(_cfg) = config() else { return Ok(not_configured()) };
    let _ = resolve_user_scoped(&_state.db, &headers, "compute").await?;
    let package = URL_SAFE_NO_PAD
        .decode(&body.package_base64)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&body.package_base64))
        .map_err(|_| ApiError::BadRequest("package_base64 is not valid base64".to_string()))?;
    let (status, resp) = ReqwestTeamsHttp
        .post_bytes(
            &format!("{GRAPH}/appCatalogs/teamsApps"),
            &[("Authorization", &format!("Bearer {}", body.graph_token)), ("Content-Type", "application/zip")],
            &package,
        )
        .await
        .map_err(|e| ApiError::Internal(format!("Graph unreachable: {e}")))?;
    let ok = (200..300).contains(&status);
    Ok((if ok { StatusCode::OK } else { StatusCode::BAD_GATEWAY }, Json(json!({ "graphStatus": status, "graphBody": resp }))).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallAppBody {
    graph_token: String,
    user_id: Option<String>,
    app_id: Option<String>,
}

/// Optional admin path: install the published app for one user with the
/// admin's delegated Graph token.
async fn install_app_h(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<InstallAppBody>) -> Result<Response, ApiError> {
    let Some(cfg) = config() else { return Ok(not_configured()) };
    let _ = resolve_user_scoped(&state.db, &headers, "compute").await?;
    let user = body.user_id.unwrap_or_else(|| "me".to_string());
    let app = body.app_id.unwrap_or_else(|| cfg.app_id.clone());
    let payload = json!({
        "teamsApp@odata.bind": format!("{GRAPH}/appCatalogs/teamsApps/{app}"),
    });
    let (status, resp) = ReqwestTeamsHttp
        .post_json(
            &format!("{GRAPH}/users/{}/teamwork/installedApps", user),
            &[("Authorization", &format!("Bearer {}", body.graph_token))],
            &payload,
        )
        .await
        .map_err(|e| ApiError::Internal(format!("Graph unreachable: {e}")))?;
    let ok = (200..300).contains(&status);
    Ok((if ok { StatusCode::OK } else { StatusCode::BAD_GATEWAY }, Json(json!({ "graphStatus": status, "graphBody": resp }))).into_response())
}

// ------------------------------------------------------------------- worker

/// Start the background delivery loop and the 7-day cleanup.
pub fn start_teams_app_worker(state: Arc<ApiState>) {
    tokio::spawn(async move {
        let mut ticks: u64 = 0;
        loop {
            if let Err(error) = deliver_due(&state).await {
                tracing::warn!("teams app worker: {error}");
            }
            ticks += 1;
            if ticks % 720 == 0 {
                let _ = sqlx::query(
                    "DELETE FROM teams_app_queue WHERE (delivered_at IS NOT NULL AND delivered_at < now() - interval '7 days')
                        OR (dead_at IS NOT NULL AND dead_at < now() - interval '7 days')",
                )
                .execute(&state.db)
                .await;
            }
            tokio::time::sleep(WORKER_INTERVAL).await;
        }
    });
}

async fn deliver_due(state: &Arc<ApiState>) -> Result<(), ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM teams_app_queue
          WHERE delivered_at IS NULL AND dead_at IS NULL AND next_attempt_at <= now()
            AND (locked_until IS NULL OR locked_until < now())
          LIMIT 50",
    )
    .fetch_all(&state.db)
    .await?;
    for (id,) in rows {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = deliver_one(&state, id).await {
                tracing::warn!(%id, "teams app delivery failed: {error}");
            }
        });
    }
    Ok(())
}

/// Deliver one queued activity to the install's registered runtime, oldest
/// first, retrying with backoff for 24 hours.
async fn deliver_one(state: &Arc<ApiState>, id: i64) -> Result<(), ApiError> {
    use base64::engine::general_purpose::STANDARD;
    let claimed: Option<(i64, String, String, Value, chrono::DateTime<chrono::Utc>, i32)> = sqlx::query_as(
        "UPDATE teams_app_queue SET locked_until = now() + interval '3 minutes', attempts = attempts + 1
          WHERE id = $1 AND delivered_at IS NULL AND dead_at IS NULL
            AND next_attempt_at <= now() AND (locked_until IS NULL OR locked_until < now())
          RETURNING id, tenant_id, conversation_id, activity, received_at, attempts",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((_, tenant, _, activity, received_at, attempts)) = claimed else { return Ok(()) };
    let install: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT user_id, runtime_id FROM teams_installs WHERE tenant_id = $1",
    )
    .bind(&tenant)
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id, Some(runtime_id))) = install else {
        return retry_or_dead(state, id, received_at, attempts, None, "no runtime registered for this tenant").await;
    };
    let outcome = relay_signed_request_to_runtime_with(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &user_id,
        &runtime_id,
        RelayRequest {
            method: "POST".to_string(),
            path: RUNTIME_PATH.to_string(),
            headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
            body: STANDARD.encode(serde_json::to_vec(&activity).unwrap_or_default()),
            body_encoding: "base64".to_string(),
        },
        &[],
        HashMap::from([
            (USER_HEADER.to_string(), user_id.clone()),
            (QUEUED_AT_HEADER.to_string(), received_at.timestamp().to_string()),
        ]),
    )
    .await;
    let (status, error) = match &outcome {
        Ok(response) => (Some(response.status().as_u16()), None),
        Err(error) => (None, Some(error.to_string())),
    };
    if status.map(classify) == Some(Delivery::Done) {
        sqlx::query("UPDATE teams_app_queue SET delivered_at = now(), locked_until = NULL, last_status = $2 WHERE id = $1")
            .bind(id)
            .bind(status.map(i32::from))
            .execute(&state.db)
            .await?;
        return Ok(());
    }
    retry_or_dead(state, id, received_at, attempts, status, error.as_deref().unwrap_or("delivery failed")).await
}

async fn retry_or_dead(
    state: &Arc<ApiState>,
    id: i64,
    received_at: chrono::DateTime<chrono::Utc>,
    attempts: i32,
    status: Option<u16>,
    error: &str,
) -> Result<(), ApiError> {
    let give_up = chrono::Utc::now() - received_at > chrono::Duration::hours(GIVE_UP_AFTER_HOURS);
    sqlx::query(
        "UPDATE teams_app_queue
            SET locked_until = NULL, last_status = $2, last_error = $3,
                next_attempt_at = now() + make_interval(secs => $4),
                dead_at = CASE WHEN $5 THEN now() ELSE NULL END
          WHERE id = $1",
    )
    .bind(id)
    .bind(status.map(i32::from))
    .bind(error)
    .bind(backoff_secs(attempts) as f64)
    .bind(give_up)
    .execute(&state.db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Test RSA key (2048-bit, PKCS#8 DER base64) generated for this module.
    const TEST_KEY_PKCS8_B64: &str = "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC7pkZzGB1dKWyJTQbHp27kjibf/zmxkUoI/Fo22Rx/uTbuRjj8l11nfbovikc4TbwaAPPSjliyHLiVHgqWvEcyg5f7d9k5eiLhRgR8eUesJM4cAux/xJllZeZI4LbE5KTwHEwWcjLW9IjwCMLkRp9B72M2QVvX4fftwjKc9SdOsAVASzqH25u03gXfrjTBDNUTLCCnxHIx/13sTOZUpuMgl1PYg6LnyPbzQcr8aI5K6KM2VZHR9zfbbTnNnm27J+3iykOwZGvFeB4lQfqNFJDI64ZHRqxbKT4kQMgDQXWndIyLXchV7DSwibwhgrq9yC1l2o39byQ6V4aKs7K9T4e7AgMBAAECggEBAKvUerZx4pGomQaxTgANRfJsaRS8tavwCwdnbYTEEuCyTaarDwvd314hNxjJWqYoKJL3RE4OoxGWUz/ZHoEhL2EGN2nCOVv0h3QZlYoV1JfBrCriUoY9FOpRASrC+xoY9SAaCWKUeBF2It8KZsx6DuNlvke0WdG5zjodlhM8Oh5GZxrKTt/fSCwkG8f5tZ9HCH8W+H3eWv3ShVWR6WmIGZaHTBK3vnDaIPOPJ4OyA3piMiMZLAuwLknNQyzIl5AqaFdGnOZvPWM9iOA5QNq9yCAEP21YlGrz4cQNbnZdwhg7RSkQobMEy4AWrjWpWgHlYmZr2Ho5aVOWWFFk6zEYkqECgYEA42/AiPE4fK7NVV7OUKQm7R0s1NDPUHL4qwb8nQXrbSPkiCk+uEBuWoOzyG4PfNfgv2EALpB7qKk3VW1clY1JXlAwu405oAhd9BTRynyL0x1Vz4oZ6jJd2ewn/+cNnTY8qt39z5p79bitpm5XStwsL1R9Q3k6iVyPARhyxvZZJsUCgYEA0zdXg489VrQb85iElCrWAQ1Kdi/labOAhFjIrAVWNhdQNcrcfAwTbruwxZOupNcMaNX8IixBR6iUia0TtDTkGq6jnYwbB7j13sprKqN0X0klM3tLKFppxQCPMJ+bDyN451c/vpCmdlWuZYmSQvlY0CsjLadUptLLw4GoDH703H8CgYAIqJV84SoUXRdGG8DhAw7UUhsF0xlBZ88du7tcQwBufEJUCLXxj8pjucXbaI1AZHuS7Z9zJl7+0cpgfzRaITHc3FKuoTbDZ+4jv7Mo8Urlc6VzrD9GUjqOdFYlgOdcx9bRbngeMjRr62b5AhRirjUkbCQEsJXQ6uwqz4J3HqPQPQKBgQCNzjoTObgU3hdmFJ/uWlQToLi9YKrFrJ48PN99npei+UQA2ZHuNq1VSu4DuIMoaLkMKZ590viBA822IV15P4ll4Jo4zDfZl3R7f6szlUosLw+q4Lw0+37HpPh9zKpuH4KszwcdCkC4cg4EXbi0/nOCT3Pu0skit6PWPtZ0jUgYQQKBgChL9fAWLGgWJRr9Cmw7mB1qAlyiR/29S0Bvn8vPAUm1nnx5NliX9oi/OQ2GlRgBIkkK8HzPcRA6IP9B8BFW1W2zjoUssn0tCjmvP5ppgLCnZftzeLj3Yz3xscUNIFlyZmICudxysDInqGyejgM0hjKFu7hHxPw7RARXkTqFaTUR";
    const TEST_KEY_N: &str = "u6ZGcxgdXSlsiU0Gx6du5I4m3_85sZFKCPxaNtkcf7k27kY4_JddZ326L4pHOE28GgDz0o5Yshy4lR4KlrxHMoOX-3fZOXoi4UYEfHlHrCTOHALsf8SZZWXmSOC2xOSk8BxMFnIy1vSI8AjC5EafQe9jNkFb1-H37cIynPUnTrAFQEs6h9ubtN4F3640wQzVEywgp8RyMf9d7EzmVKbjIJdT2IOi58j280HK_GiOSuijNlWR0fc32205zZ5tuyft4spDsGRrxXgeJUH6jRSQyOuGR0asWyk-JEDIA0F1p3SMi13IVew0sIm8IYK6vcgtZdqN_W8kOleGirOyvU-Huw";
    const TEST_KID: &str = "k1";

    #[derive(Default)]
    struct FakeHttp {
        gets: StdMutex<Vec<String>>,
        forms: StdMutex<Vec<(String, Vec<(String, String)>)>>,
        replies: StdMutex<HashMap<String, Value>>,
    }

    #[async_trait]
    impl TeamsHttp for FakeHttp {
        async fn get_json(&self, url: &str) -> Result<(u16, Value), String> {
            self.gets.lock().unwrap().push(url.to_string());
            Ok((200, self.replies.lock().unwrap().get(url).cloned().unwrap_or(Value::Null)))
        }
        async fn post_json(&self, _url: &str, _headers: &[(&str, &str)], _body: &Value) -> Result<(u16, Value), String> {
            Err("unused".into())
        }
        async fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String> {
            self.forms.lock().unwrap().push((url.to_string(), form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()));
            Ok((200, self.replies.lock().unwrap().get(url).cloned().unwrap_or(Value::Null)))
        }
        async fn post_bytes(&self, _url: &str, _headers: &[(&str, &str)], _body: &[u8]) -> Result<(u16, Value), String> {
            Err("unused".into())
        }
    }

    fn idp() -> Arc<FakeHttp> {
        let idp = Arc::new(FakeHttp::default());
        idp.replies.lock().unwrap().insert(
            OPENID_URL.into(),
            json!({ "jwks_uri": "https://idp.test/keys" }),
        );
        idp.replies.lock().unwrap().insert(
            "https://idp.test/keys".into(),
            json!({ "keys": [{ "kty": "RSA", "kid": TEST_KID, "n": TEST_KEY_N, "e": "AQAB", "use": "sig" }] }),
        );
        idp
    }

    fn sign_rs256(payload: &Value) -> String {
        let der = base64::engine::general_purpose::STANDARD.decode(TEST_KEY_PKCS8_B64).unwrap();
        let key_pair = aws_lc_rs::signature::RsaKeyPair::from_pkcs8(&der).unwrap();
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let header = URL_SAFE_NO_PAD.encode(format!("{{\"alg\":\"RS256\",\"kid\":\"{TEST_KID}\",\"typ\":\"JWT\"}}"));
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        let message = format!("{header}.{claims}");
        let mut sig = vec![0u8; key_pair.public_modulus_len()];
        key_pair.sign(&aws_lc_rs::signature::RSA_PKCS1_SHA256, &rng, message.as_bytes(), &mut sig).unwrap();
        format!("Bearer {message}.{}", URL_SAFE_NO_PAD.encode(sig))
    }

    fn good_claims(app: &str) -> Value {
        json!({ "iss": BOT_ISSUER, "aud": app, "exp": now_secs() + 600, "nbf": now_secs() - 10, "serviceurl": "https://smba.test/amer/" })
    }

    fn activity() -> Value {
        json!({ "serviceUrl": "https://smba.test/amer" })
    }

    #[tokio::test]
    async fn validates_bot_framework_jwt_at_the_edge() {
        let http = idp();
        let auth = BotFrameworkAuth::for_bot_framework(http.clone(), "app-1");
        assert!(auth.validate(&sign_rs256(&good_claims("app-1")), &activity()).await.is_ok());

        let mut bad_aud = good_claims("app-1");
        bad_aud["aud"] = json!("app-2");
        assert!(auth.validate(&sign_rs256(&bad_aud), &activity()).await.is_err());

        let mut bad_iss = good_claims("app-1");
        bad_iss["iss"] = json!("https://evil.example");
        assert!(auth.validate(&sign_rs256(&bad_iss), &activity()).await.is_err());

        let mut expired = good_claims("app-1");
        expired["exp"] = json!(now_secs() - 3600);
        assert!(auth.validate(&sign_rs256(&expired), &activity()).await.is_err());

        let mut future = good_claims("app-1");
        future["nbf"] = json!(now_secs() + 3600);
        assert!(auth.validate(&sign_rs256(&future), &activity()).await.is_err());

        let mut wrong_svc = good_claims("app-1");
        wrong_svc["serviceurl"] = json!("https://elsewhere.test/");
        let err = auth.validate(&sign_rs256(&wrong_svc), &activity()).await.unwrap_err();
        assert!(err.contains("serviceUrl"), "{err}");

        assert!(auth.validate("Bearer not.a.jwt", &activity()).await.is_err());
        assert!(auth.validate("HMAC abc", &activity()).await.is_err());
        // OpenID metadata + JWKS were fetched once and then cached.
        assert_eq!(http.gets.lock().unwrap().len(), 2);
    }

    #[test]
    fn reads_the_tenant_and_routable_types() {
        let a = json!({ "channelData": { "tenant": { "id": "t-channel" } }, "conversation": { "tenantId": "t-conv" } });
        assert_eq!(tenant_of(&a).as_deref(), Some("t-channel"));
        let b = json!({ "conversation": { "id": "19:x", "tenantId": "t-conv" } });
        assert_eq!(tenant_of(&b).as_deref(), Some("t-conv"));
        assert_eq!(tenant_of(&json!({ "conversation": { "id": "19:x" } })), None);
        for ty in ["message", "messageUpdate", "messageDelete", "messageReaction"] {
            assert!(is_routable_activity(&json!({ "type": ty })), "{ty}");
        }
        for ty in ["conversationUpdate", "installationUpdate", "typing"] {
            assert!(!is_routable_activity(&json!({ "type": ty })), "{ty}");
        }
    }

    #[test]
    fn reply_activity_carries_the_bot_name_in_an_adaptive_card_header() {
        let plain = build_reply_activity("hi", None, None);
        assert_eq!(plain, json!({ "type": "message", "text": "hi" }));
        let named = build_reply_activity("hi", Some("Scout"), Some("1700000000123"));
        assert_eq!(named["type"], "message");
        assert!(named.get("text").is_none());
        let card = &named["attachments"][0];
        assert_eq!(card["contentType"], "application/vnd.microsoft.card.adaptive");
        assert_eq!(card["content"]["type"], "AdaptiveCard");
        assert_eq!(card["content"]["version"], "1.4");
        assert_eq!(card["content"]["body"][0]["items"][0]["text"], "Scout");
        assert_eq!(card["content"]["body"][0]["style"], "emphasis");
        assert_eq!(card["content"]["body"][1]["text"], "hi");
        assert_eq!(named["replyToId"], "1700000000123");
    }

    #[test]
    fn connect_url_is_the_multitenant_delegated_signin() {
        let url = connect_url("app-1", "https://api.allternit.com/channels/teams/callback", "st ate");
        assert!(url.starts_with(ENTRA_AUTHORIZE), "{url}");
        assert!(url.contains("client_id=app-1"), "{url}");
        assert!(url.contains("scope=openid%20profile"), "{url}");
        assert!(url.contains("redirect_uri=https%3A%2F%2Fapi.allternit.com%2Fchannels%2Fteams%2Fcallback"), "{url}");
        assert!(url.contains("state=st%20ate"), "{url}");
    }

    #[tokio::test]
    async fn bot_token_uses_allternits_tenant_and_caches() {
        let http = Arc::new(FakeHttp::default());
        http.replies.lock().unwrap().insert(
            "https://login.microsoftonline.com/tenant-1/oauth2/v2.0/token".into(),
            json!({ "access_token": "tok-1", "expires_in": 3600 }),
        );
        let cfg = TeamsConfig {
            app_id: "app-1".into(),
            app_password: "pw".into(),
            tenant_id: Some("tenant-1".into()),
            proactive: false,
            app_url: "https://app.allternit.com".into(),
        };
        let cache = TeamsTokenCache::new(http.clone(), &cfg);
        assert_eq!(cache.access_token().await.unwrap(), "tok-1");
        assert_eq!(cache.access_token().await.unwrap(), "tok-1");
        let forms = http.forms.lock().unwrap();
        assert_eq!(forms.len(), 1, "token cached until shortly before expiry");
        assert_eq!(forms[0].0, "https://login.microsoftonline.com/tenant-1/oauth2/v2.0/token");
        assert!(forms[0].1.contains(&("scope".to_string(), BOT_SCOPE.to_string())));
        assert!(forms[0].1.contains(&("client_secret".to_string(), "pw".to_string())));
    }

    #[tokio::test]
    async fn multitenant_bot_falls_back_to_the_botframework_tenant() {
        let http = Arc::new(FakeHttp::default());
        http.replies.lock().unwrap().insert(
            "https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token".into(),
            json!({ "access_token": "tok-2", "expires_in": 3600 }),
        );
        let cfg = TeamsConfig {
            app_id: "app-1".into(),
            app_password: "pw".into(),
            tenant_id: None,
            proactive: false,
            app_url: "https://app.allternit.com".into(),
        };
        let cache = TeamsTokenCache::new(http, &cfg);
        assert_eq!(cache.access_token().await.unwrap(), "tok-2");
    }

    #[test]
    fn proactive_send_is_a_separate_path_and_off_by_default() {
        assert!(proactive_refused(false, Some(true)), "off by default");
        assert!(!proactive_refused(true, Some(true)), "flagged on");
        assert!(!proactive_refused(false, None), "reactive replies always allowed");
        assert!(!proactive_refused(false, Some(false)));
    }

    #[test]
    fn entra_v2_issuer_carries_the_tenant() {
        assert_eq!(
            entra_tenant_from_iss(Some("https://login.microsoftonline.com/72f988bf-86f1-41af-91ab-2d7cd011db47/v2.0")),
            Some("72f988bf-86f1-41af-91ab-2d7cd011db47")
        );
        assert_eq!(entra_tenant_from_iss(Some("https://evil.example/tenant/v2.0")), None);
        assert_eq!(entra_tenant_from_iss(None), None);
    }
}
