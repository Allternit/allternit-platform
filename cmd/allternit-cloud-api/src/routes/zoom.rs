//! Zoom, cloud side (docs/meetings/ZOOM.md sections 3–7; CONTRACT.md
//! "Change 2026-11-13: Zoom cloud side"). Gizzi joins Zoom calls through
//! the Meeting SDK with an on-behalf-of (OBF) token from the user's own
//! connected Zoom account, so the user connects Zoom once here.
//!
//! - `GET /api/v1/zoom/oauth/start?return_to=` (User): `200 { url }`, the
//!   Zoom consent URL with a signed `state` `{u, r, n, exp 10 min}`.
//! - `GET /api/v1/zoom/oauth/callback?code=&state=` (state-verified):
//!   exchanges the code, checks the OBF endpoint once, stores the tokens
//!   encrypted (`credential_cipher`) in `zoom_connections`, then `302` to
//!   `return_to` with `?zoom=connected|error`.
//! - `GET /api/v1/zoom/connection` / `DELETE` (User): status only, never
//!   tokens; delete revokes best effort and drops the row.
//! - `POST /api/v1/zoom/webhook` (Zoom signature): `x-zm-signature` =
//!   `v0=` + hex HMAC-SHA256(`v0:<ts>:<body>`) with
//!   `ZOOM_WEBHOOK_SECRET_TOKEN`, timestamp within 5 minutes.
//!   `endpoint.url_validation` is answered; `app_deauthorized` deletes the
//!   user's row and ends their active Zoom sessions (`left_by_user`).
//! - `POST /api/v1/meeting-sessions/:sid/zoom-tokens` (Pod): a fresh OBF
//!   token, an SDK JWT signed here (the client secret never reaches a pod),
//!   and a local-recording join token when Zoom gives one.
//!
//! Refresh tokens rotate on every refresh, so a refresh runs inside a
//! transaction holding `SELECT … FOR UPDATE` on the row and writes the new
//! refresh token before commit: two pods refreshing at once never lose it.
//! `invalid_grant` sets `needs_reauth` (the page shows "Reconnect Zoom").
//!
//! Env: `ZOOM_CLIENT_ID`, `ZOOM_CLIENT_SECRET`, `ZOOM_WEBHOOK_SECRET_TOKEN`,
//! optional `ZOOM_OAUTH_REDIRECT_URI` (default the registered
//! `https://api.allternit.com/api/v1/zoom/oauth/callback`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::meeting_pod::pod_session;
use super::meeting_state::State as SessionState;
use super::meetings::{caller, control, fetch_session, ts, MeetingError, Result};
use crate::ApiState;

type HmacSha256 = Hmac<Sha256>;

pub const DEFAULT_REDIRECT_URI: &str = "https://api.allternit.com/api/v1/zoom/oauth/callback";
const DEFAULT_RETURN_TO: &str = "https://ai.allternit.com/settings/meetings";
const OAUTH_BASE: &str = "https://zoom.us";
const API_BASE: &str = "https://api.zoom.us/v2";
const STATE_TTL_SECS: i64 = 600;
/// Refresh the cached access token when less than this remains.
const REFRESH_MARGIN: chrono::Duration = chrono::Duration::minutes(5);
/// Zoom webhook timestamps older (or newer) than this are refused.
pub const WEBHOOK_WINDOW_SECS: i64 = 300;
/// SDK JWT lifetime (Zoom: at least 1800 s, at most 48 h) and clock-skew backdate.
const SDK_JWT_TTL_SECS: i64 = 2 * 3600;
const SDK_JWT_BACKDATE_SECS: i64 = 30;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/zoom/oauth/start", get(oauth_start))
        .route("/api/v1/zoom/oauth/callback", get(oauth_callback))
        .route("/api/v1/zoom/connection", get(get_connection).delete(delete_connection))
        .route("/api/v1/zoom/webhook", post(webhook))
        .route("/api/v1/meeting-sessions/:sid/zoom-tokens", post(zoom_tokens))
}

// ---------------------------------------------------------------------------
// Config and the Zoom API seam
// ---------------------------------------------------------------------------

/// The Zoom app's credentials. No `Debug`: it holds the client secret.
#[derive(Clone)]
pub struct ZoomConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

impl ZoomConfig {
    pub fn from_env() -> Option<Self> {
        let get = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Some(Self {
            client_id: get("ZOOM_CLIENT_ID")?,
            client_secret: get("ZOOM_CLIENT_SECRET")?,
            redirect_uri: get("ZOOM_OAUTH_REDIRECT_URI").unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_string()),
        })
    }
}

fn webhook_secret_from_env() -> Option<String> {
    std::env::var("ZOOM_WEBHOOK_SECRET_TOKEN").ok().filter(|v| !v.trim().is_empty())
}

/// One OAuth token response.
#[derive(Debug, Clone)]
pub struct ZoomTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub scopes: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ZoomCallError {
    /// The refresh token (or code) is no longer valid: the user reconnects.
    #[error("invalid_grant")]
    InvalidGrant,
    #[error("zoom returned {0}")]
    Status(u16),
    #[error("zoom request failed: {0}")]
    Http(String),
}

/// Seam for Zoom's OAuth and REST endpoints. Production is [`ZoomHttp`];
/// tests substitute a fake (no real Zoom, no real credentials).
#[async_trait]
pub trait ZoomApi: Send + Sync {
    async fn exchange_code(&self, code: &str) -> std::result::Result<ZoomTokens, ZoomCallError>;
    async fn refresh(&self, refresh_token: &str) -> std::result::Result<ZoomTokens, ZoomCallError>;
    /// `GET /v2/users/me/token?type=onbehalf` (lives 2 h).
    async fn obf_token(&self, access_token: &str) -> std::result::Result<String, ZoomCallError>;
    /// `GET /v2/meetings/{n}/jointoken/local_recording` (lives 120 s).
    async fn local_recording_token(&self, access_token: &str, meeting_number: &str)
        -> std::result::Result<String, ZoomCallError>;
    /// `POST /oauth/revoke`.
    async fn revoke(&self, access_token: &str) -> std::result::Result<(), ZoomCallError>;
}

pub struct ZoomHttp {
    config: ZoomConfig,
    client: reqwest::Client,
}

impl ZoomHttp {
    pub fn new(config: ZoomConfig) -> Self {
        Self { config, client: crate::net_client::client() }
    }

    fn basic(&self) -> String {
        format!("Basic {}", STANDARD.encode(format!("{}:{}", self.config.client_id, self.config.client_secret)))
    }

    /// Send, retrying once on 429 after `Retry-After` (capped at 10 s).
    async fn send(&self, build: impl Fn() -> reqwest::RequestBuilder) -> std::result::Result<(u16, Value), ZoomCallError> {
        for attempt in 0..2 {
            let response = build()
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .map_err(|e| ZoomCallError::Http(e.without_url().to_string()))?;
            let status = response.status().as_u16();
            if status == 429 && attempt == 0 {
                let wait = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(1)
                    .min(10);
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            let body = response.json::<Value>().await.unwrap_or(Value::Null);
            return Ok((status, body));
        }
        Err(ZoomCallError::Status(429))
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> std::result::Result<ZoomTokens, ZoomCallError> {
        let url = format!("{OAUTH_BASE}/oauth/token");
        let (status, body) = self
            .send(|| self.client.post(&url).header(header::AUTHORIZATION, self.basic()).form(form))
            .await?;
        if body.get("error").and_then(Value::as_str) == Some("invalid_grant")
            || body.get("reason").and_then(Value::as_str).is_some_and(|r| r.contains("Invalid Token"))
        {
            return Err(ZoomCallError::InvalidGrant);
        }
        if !(200..300).contains(&status) {
            return Err(ZoomCallError::Status(status));
        }
        tokens_from_response(&body).ok_or(ZoomCallError::Status(status))
    }
}

fn tokens_from_response(body: &Value) -> Option<ZoomTokens> {
    Some(ZoomTokens {
        access_token: body.get("access_token")?.as_str()?.to_string(),
        refresh_token: body.get("refresh_token")?.as_str()?.to_string(),
        expires_in: body.get("expires_in").and_then(Value::as_i64).unwrap_or(3600),
        scopes: body
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("")
            .split([' ', ','])
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    })
}

#[async_trait]
impl ZoomApi for ZoomHttp {
    async fn exchange_code(&self, code: &str) -> std::result::Result<ZoomTokens, ZoomCallError> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.config.redirect_uri),
        ])
        .await
    }

    async fn refresh(&self, refresh_token: &str) -> std::result::Result<ZoomTokens, ZoomCallError> {
        self.token_request(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token)]).await
    }

    async fn obf_token(&self, access_token: &str) -> std::result::Result<String, ZoomCallError> {
        let url = format!("{API_BASE}/users/me/token?type=onbehalf");
        let (status, body) = self.send(|| self.client.get(&url).bearer_auth(access_token)).await?;
        if !(200..300).contains(&status) {
            return Err(ZoomCallError::Status(status));
        }
        body.get("token").and_then(Value::as_str).map(str::to_string).ok_or(ZoomCallError::Status(status))
    }

    async fn local_recording_token(
        &self,
        access_token: &str,
        meeting_number: &str,
    ) -> std::result::Result<String, ZoomCallError> {
        let url = format!("{API_BASE}/meetings/{meeting_number}/jointoken/local_recording");
        let (status, body) = self.send(|| self.client.get(&url).bearer_auth(access_token)).await?;
        if !(200..300).contains(&status) {
            return Err(ZoomCallError::Status(status));
        }
        body.get("token").and_then(Value::as_str).map(str::to_string).ok_or(ZoomCallError::Status(status))
    }

    async fn revoke(&self, access_token: &str) -> std::result::Result<(), ZoomCallError> {
        let url = format!("{OAUTH_BASE}/oauth/revoke");
        let (status, _) = self
            .send(|| {
                self.client
                    .post(&url)
                    .header(header::AUTHORIZATION, self.basic())
                    .form(&[("token", access_token)])
            })
            .await?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(ZoomCallError::Status(status))
        }
    }
}

fn not_configured() -> MeetingError {
    MeetingError::new(StatusCode::SERVICE_UNAVAILABLE, "zoom_not_configured", "Zoom is not set up on this server.")
}

fn not_connected(message: &str) -> MeetingError {
    MeetingError::new(StatusCode::CONFLICT, "zoom_not_connected", message.to_string())
}

// ---------------------------------------------------------------------------
// Token sealing (credential_cipher, like provider_tokens)
// ---------------------------------------------------------------------------

fn seal(cipher: Option<&allternit_cloud_core::CredentialCipher>, plaintext: &str) -> String {
    match cipher {
        Some(cipher) => cipher.encrypt(plaintext).unwrap_or_else(|_| plaintext.to_string()),
        None => {
            tracing::warn!("Storing Zoom tokens PLAINTEXT (ALLTERNIT_CREDENTIALS_KEY unset)");
            plaintext.to_string()
        }
    }
}

fn open(cipher: Option<&allternit_cloud_core::CredentialCipher>, stored: &str) -> String {
    match cipher {
        Some(cipher) => cipher.decrypt(stored).unwrap_or_else(|_| stored.to_string()),
        None => stored.to_string(),
    }
}

/// Zoom OAuth access tokens are JWTs whose payload carries the Zoom user id
/// as `uid`. Read it (no verification needed: the token came straight from
/// Zoom over TLS) so the deauthorization webhook can find the row.
fn zoom_user_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("uid").and_then(Value::as_str).map(str::to_string)
}

// ---------------------------------------------------------------------------
// Fresh access token under SELECT … FOR UPDATE
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum TokenError {
    /// No row for the user.
    NotConnected,
    /// The row is `needs_reauth` / `revoked`, or Zoom refused the refresh.
    NeedsReauth,
    /// Zoom or the network failed; nothing changed.
    Upstream,
    Db,
}

impl From<TokenError> for MeetingError {
    fn from(error: TokenError) -> Self {
        match error {
            TokenError::NotConnected => not_connected("Connect your Zoom account first. Gizzi joins Zoom calls on your behalf."),
            TokenError::NeedsReauth => {
                let mut e = not_connected("Reconnect Zoom. Your Zoom connection expired or was removed.");
                e.extra.insert("needs_reauth".to_string(), Value::Bool(true));
                e
            }
            TokenError::Upstream => MeetingError::new(StatusCode::BAD_GATEWAY, "zoom_unavailable", "Zoom did not answer. Try again."),
            TokenError::Db => MeetingError::internal(),
        }
    }
}

#[derive(FromRow)]
struct TokenRow {
    encrypted_refresh_token: String,
    encrypted_access_token: Option<String>,
    access_expires_at: Option<DateTime<Utc>>,
    status: String,
}

/// The user's Zoom access token, refreshed when less than 5 minutes remain.
/// The refresh runs while this transaction holds the row lock, and the new
/// refresh token is written before commit, so concurrent callers queue and
/// each one uses the newest refresh token (Zoom rotates it every refresh).
pub async fn fresh_access_token(
    db: &PgPool,
    cipher: Option<&allternit_cloud_core::CredentialCipher>,
    zoom: &dyn ZoomApi,
    user_id: &str,
) -> std::result::Result<String, TokenError> {
    let db_err = |e: sqlx::Error| {
        tracing::error!("zoom: database error: {e}");
        TokenError::Db
    };
    let mut tx = db.begin().await.map_err(db_err)?;
    let row = sqlx::query_as::<_, TokenRow>(
        "SELECT encrypted_refresh_token, encrypted_access_token, access_expires_at, status \
         FROM zoom_connections WHERE user_id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?
    .ok_or(TokenError::NotConnected)?;
    if row.status != "active" {
        return Err(TokenError::NeedsReauth);
    }
    if let (Some(access), Some(expires)) = (&row.encrypted_access_token, row.access_expires_at) {
        if expires > Utc::now() + REFRESH_MARGIN {
            tx.commit().await.map_err(db_err)?;
            return Ok(open(cipher, access));
        }
    }
    let refresh_token = open(cipher, &row.encrypted_refresh_token);
    match zoom.refresh(&refresh_token).await {
        Ok(tokens) => {
            sqlx::query(
                "UPDATE zoom_connections SET encrypted_refresh_token = $2, encrypted_access_token = $3, \
                     access_expires_at = $4, scopes = CASE WHEN cardinality($5::text[]) > 0 THEN $5 ELSE scopes END, \
                     zoom_user_id = COALESCE($6, zoom_user_id), updated_at = now() \
                 WHERE user_id = $1",
            )
            .bind(user_id)
            .bind(seal(cipher, &tokens.refresh_token))
            .bind(seal(cipher, &tokens.access_token))
            .bind(Utc::now() + chrono::Duration::seconds(tokens.expires_in.max(0)))
            .bind(&tokens.scopes)
            .bind(zoom_user_id(&tokens.access_token))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(tokens.access_token)
        }
        Err(ZoomCallError::InvalidGrant) => {
            sqlx::query("UPDATE zoom_connections SET status = 'needs_reauth', updated_at = now() WHERE user_id = $1")
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Err(TokenError::NeedsReauth)
        }
        Err(error) => {
            tracing::warn!(%user_id, %error, "zoom: token refresh failed");
            Err(TokenError::Upstream)
        }
    }
}

// ---------------------------------------------------------------------------
// SDK JWT (ZOOM.md section 7)
// ---------------------------------------------------------------------------

/// Meeting SDK JWT, HS256 with the client secret, server-side only.
/// `iat` is back-dated 30 s for clock skew; `exp` = `tokenExp` = 2 h after.
pub fn sign_sdk_jwt(config: &ZoomConfig, now: i64) -> String {
    let iat = now - SDK_JWT_BACKDATE_SECS;
    let exp = iat + SDK_JWT_TTL_SECS;
    let claims = json!({ "appKey": config.client_id, "iat": iat, "exp": exp, "tokenExp": exp });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(config.client_secret.as_bytes()),
    )
    .expect("HS256 signing cannot fail")
}

/// Pod identity for a Zoom launch: a fresh OBF token for the session owner
/// plus an SDK JWT. The client secret stays here (`sdkSecretRef: jwt:…`).
pub async fn zoom_identity(
    db: &PgPool,
    cipher: Option<&allternit_cloud_core::CredentialCipher>,
    zoom: &dyn ZoomApi,
    config: &ZoomConfig,
    owner: &str,
) -> std::result::Result<Value, TokenError> {
    let access = fresh_access_token(db, cipher, zoom, owner).await?;
    let obf = zoom.obf_token(&access).await.map_err(|error| {
        tracing::warn!(%owner, %error, "zoom: OBF token fetch failed");
        TokenError::Upstream
    })?;
    Ok(json!({
        "kind": "zoom_obf",
        "obfToken": obf,
        "sdkKey": config.client_id,
        "sdkSecretRef": format!("jwt:{}", sign_sdk_jwt(config, Utc::now().timestamp())),
    }))
}

/// The meeting number from a Zoom `url_key` (`zoom:<digits>`); personal
/// rooms (`zoom:my:<name>`) have none.
pub fn meeting_number(url_key: &str) -> Option<&str> {
    url_key
        .strip_prefix("zoom:")
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

// ---------------------------------------------------------------------------
// Signed OAuth state
// ---------------------------------------------------------------------------

fn state_mac(secret: &str) -> HmacSha256 {
    // Domain-separated from the client secret's other use (SDK JWTs).
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(b"allternit-zoom-oauth-state:");
    mac
}

/// `base64url(json{u, r, n, exp}).base64url(hmac)`.
pub fn sign_state(secret: &str, user_id: &str, return_to: &str, now: i64) -> String {
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let payload = json!({ "u": user_id, "r": return_to, "n": URL_SAFE_NO_PAD.encode(nonce), "exp": now + STATE_TTL_SECS });
    let body = URL_SAFE_NO_PAD.encode(payload.to_string());
    let mut mac = state_mac(secret);
    mac.update(body.as_bytes());
    format!("{body}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// `(user_id, return_to)` from a valid, unexpired state.
pub fn verify_state(secret: &str, state: &str, now: i64) -> Option<(String, String)> {
    let (body, sig) = state.split_once('.')?;
    let mut mac = state_mac(secret);
    mac.update(body.as_bytes());
    mac.verify_slice(&URL_SAFE_NO_PAD.decode(sig).ok()?).ok()?;
    let payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
    if now > payload.get("exp")?.as_i64()? {
        return None;
    }
    Some((payload.get("u")?.as_str()?.to_string(), payload.get("r")?.as_str()?.to_string()))
}

/// Where the callback may send the browser: our web/PWA origins, the app's
/// `allternit://` scheme, and localhost for dev. Anything else falls back to
/// the default (no open redirect).
pub fn safe_return_to(raw: Option<&str>) -> String {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return DEFAULT_RETURN_TO.to_string();
    };
    if raw.starts_with("allternit://") {
        return raw.to_string();
    }
    let Ok(url) = url::Url::parse(raw) else {
        return DEFAULT_RETURN_TO.to_string();
    };
    let host = url.host_str().unwrap_or("");
    let ok = match url.scheme() {
        "https" => host == "allternit.com" || host.ends_with(".allternit.com"),
        "http" => host == "localhost" || host == "127.0.0.1",
        _ => false,
    };
    if ok && url.username().is_empty() && url.password().is_none() {
        raw.to_string()
    } else {
        DEFAULT_RETURN_TO.to_string()
    }
}

fn with_param(return_to: &str, key: &str, value: &str) -> String {
    let sep = if return_to.contains('?') { '&' } else { '?' };
    let (base, fragment) = match return_to.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (return_to, None),
    };
    let sep = if base.contains('?') { '&' } else { sep };
    match fragment {
        Some(fragment) => format!("{base}{sep}{key}={value}#{fragment}"),
        None => format!("{base}{sep}{key}={value}"),
    }
}

// ---------------------------------------------------------------------------
// OAuth routes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StartQuery {
    return_to: Option<String>,
}

async fn oauth_start(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<StartQuery>,
) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    let config = ZoomConfig::from_env().ok_or_else(not_configured)?;
    let return_to = safe_return_to(query.return_to.as_deref());
    Ok(Json(json!({ "url": authorize_url(&config, &caller.id, &return_to, Utc::now().timestamp()) })).into_response())
}

pub fn authorize_url(config: &ZoomConfig, user_id: &str, return_to: &str, now: i64) -> String {
    format!(
        "{OAUTH_BASE}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&state={}",
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&config.redirect_uri),
        urlencoding::encode(&sign_state(&config.client_secret, user_id, return_to, now)),
    )
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn oauth_callback(State(state): State<Arc<ApiState>>, Query(query): Query<CallbackQuery>) -> Result<Response> {
    let raw_state = query
        .state
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| MeetingError::invalid("state is required."))?;
    let config = ZoomConfig::from_env().ok_or_else(not_configured)?;
    let (user_id, return_to) = verify_state(&config.client_secret, raw_state, Utc::now().timestamp())
        .ok_or_else(|| MeetingError::invalid("state is invalid or expired. Start connecting Zoom again."))?;
    let return_to = safe_return_to(Some(&return_to));
    let outcome = match (query.error.as_deref(), query.code.as_deref().filter(|c| !c.is_empty())) {
        (None, Some(code)) => {
            connect(&state.db, state.credential_cipher.as_deref(), &ZoomHttp::new(config), &user_id, code).await
        }
        _ => false,
    };
    let value = if outcome { "connected" } else { "error" };
    Ok(Redirect::to(&with_param(&return_to, "zoom", value)).into_response())
}

/// Exchange the code, check the OBF endpoint once (token discarded) and
/// upsert the connection. Returns whether the user is now connected.
pub async fn connect(
    db: &PgPool,
    cipher: Option<&allternit_cloud_core::CredentialCipher>,
    zoom: &dyn ZoomApi,
    user_id: &str,
    code: &str,
) -> bool {
    let tokens = match zoom.exchange_code(code).await {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!(%user_id, %error, "zoom: code exchange failed");
            return false;
        }
    };
    if let Err(error) = zoom.obf_token(&tokens.access_token).await {
        tracing::warn!(%user_id, %error, "zoom: OBF check after connect failed");
        return false;
    }
    let stored = sqlx::query(
        "INSERT INTO zoom_connections (user_id, zoom_user_id, encrypted_refresh_token, encrypted_access_token, \
             access_expires_at, scopes, status) \
         VALUES ($1, $2, $3, $4, $5, $6, 'active') \
         ON CONFLICT (user_id) DO UPDATE SET zoom_user_id = EXCLUDED.zoom_user_id, \
             encrypted_refresh_token = EXCLUDED.encrypted_refresh_token, \
             encrypted_access_token = EXCLUDED.encrypted_access_token, \
             access_expires_at = EXCLUDED.access_expires_at, scopes = EXCLUDED.scopes, \
             status = 'active', updated_at = now()",
    )
    .bind(user_id)
    .bind(zoom_user_id(&tokens.access_token))
    .bind(seal(cipher, &tokens.refresh_token))
    .bind(seal(cipher, &tokens.access_token))
    .bind(Utc::now() + chrono::Duration::seconds(tokens.expires_in.max(0)))
    .bind(&tokens.scopes)
    .execute(db)
    .await;
    match stored {
        Ok(_) => true,
        Err(error) => {
            tracing::error!(%user_id, "zoom: storing connection failed: {error}");
            false
        }
    }
}

async fn get_connection(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    let row: Option<(String, DateTime<Utc>)> =
        sqlx::query_as("SELECT status, created_at FROM zoom_connections WHERE user_id = $1")
            .bind(&caller.id)
            .fetch_optional(&state.db)
            .await?;
    Ok(Json(match row {
        Some((status, created_at)) => json!({
            "connected": status == "active",
            "status": status,
            "connected_at": ts(created_at),
        }),
        None => json!({ "connected": false, "status": null, "connected_at": null }),
    })
    .into_response())
}

async fn delete_connection(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    let access: Option<Option<String>> =
        sqlx::query_scalar("SELECT encrypted_access_token FROM zoom_connections WHERE user_id = $1")
            .bind(&caller.id)
            .fetch_optional(&state.db)
            .await?;
    if let (Some(Some(sealed)), Some(config)) = (access, ZoomConfig::from_env()) {
        let token = open(state.credential_cipher.as_deref(), &sealed);
        if let Err(error) = ZoomHttp::new(config).revoke(&token).await {
            tracing::warn!(user_id = %caller.id, %error, "zoom: revoke failed (row deleted anyway)");
        }
    }
    sqlx::query("DELETE FROM zoom_connections WHERE user_id = $1")
        .bind(&caller.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Webhook (ZOOM.md section 6)
// ---------------------------------------------------------------------------

fn hmac_hex(secret: &str, message: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(message);
    hex::encode(mac.finalize().into_bytes())
}

/// `x-zm-signature` check: `v0=` + hex HMAC-SHA256(`v0:<ts>:<body>`), in
/// constant time, with the timestamp (seconds) within 5 minutes of `now`.
pub fn verify_signature(secret: &str, timestamp: &str, body: &[u8], signature: &str, now: i64) -> bool {
    let Ok(at) = timestamp.trim().parse::<i64>() else {
        return false;
    };
    if (now - at).abs() > WEBHOOK_WINDOW_SECS {
        return false;
    }
    let Some(given) = signature.trim().strip_prefix("v0=").and_then(|h| hex::decode(h).ok()) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(format!("v0:{}:", timestamp.trim()).as_bytes());
    mac.update(body);
    mac.verify_slice(&given).is_ok()
}

async fn webhook(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Result<Response> {
    handle_webhook(&state.db, webhook_secret_from_env().as_deref(), &headers, &body, Utc::now().timestamp()).await
}

pub(crate) async fn handle_webhook(
    db: &PgPool,
    secret: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
    now: i64,
) -> Result<Response> {
    let header_str = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let (Some(signature), Some(timestamp)) = (header_str("x-zm-signature"), header_str("x-zm-request-timestamp")) else {
        return Err(MeetingError::unauthorized());
    };
    let secret = secret.ok_or_else(not_configured)?;
    if !verify_signature(secret, timestamp, body, signature, now) {
        return Err(MeetingError::unauthorized());
    }
    let event: Value = serde_json::from_slice(body).map_err(|_| MeetingError::invalid("Body must be JSON."))?;
    match event.get("event").and_then(Value::as_str) {
        Some("endpoint.url_validation") => {
            let plain = event
                .pointer("/payload/plainToken")
                .and_then(Value::as_str)
                .ok_or_else(|| MeetingError::invalid("payload.plainToken is required."))?;
            Ok(Json(json!({ "plainToken": plain, "encryptedToken": hmac_hex(secret, plain.as_bytes()) })).into_response())
        }
        Some("app_deauthorized") => {
            let zoom_user = event
                .pointer("/payload/user_id")
                .and_then(Value::as_str)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| MeetingError::invalid("payload.user_id is required."))?;
            let ended = deauthorize(db, zoom_user).await?;
            Ok(Json(json!({ "ok": true, "ended_sessions": ended })).into_response())
        }
        _ => Ok(Json(json!({ "ok": true })).into_response()),
    }
}

/// The user removed our app in Zoom: delete their tokens and end their
/// active Zoom sessions (`left_by_user`). Returns how many were ended.
pub(crate) async fn deauthorize(db: &PgPool, zoom_user: &str) -> Result<usize> {
    let owners: Vec<String> = sqlx::query_scalar("DELETE FROM zoom_connections WHERE zoom_user_id = $1 RETURNING user_id")
        .bind(zoom_user)
        .fetch_all(db)
        .await?;
    let mut ended = 0;
    for owner in owners {
        tracing::info!(user_id = %owner, "zoom: app deauthorized, connection deleted");
        let sids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM meeting_sessions WHERE user_id = $1 AND platform = 'zoom' AND state <> 'ended'",
        )
        .bind(&owner)
        .fetch_all(db)
        .await?;
        for sid in sids {
            let Some(session) = fetch_session(db, sid).await? else { continue };
            if session.state() == SessionState::Ended {
                continue;
            }
            match control(db, session, "leave", None).await {
                Ok(_) => ended += 1,
                Err(error) => tracing::warn!(%sid, code = error.code, "zoom: ending session after deauthorization failed"),
            }
        }
    }
    Ok(ended)
}

// ---------------------------------------------------------------------------
// POST /api/v1/meeting-sessions/:sid/zoom-tokens (Pod)
// ---------------------------------------------------------------------------

async fn zoom_tokens(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(sid): Path<String>,
) -> Result<Response> {
    let session = pod_session(&state.db, &headers, &sid).await?;
    if session.state() == SessionState::Ended {
        return Err(MeetingError::not_active());
    }
    if session.platform != "zoom" {
        return Err(MeetingError::invalid("This session is not a Zoom meeting."));
    }
    // Connection first, so a missing one is a 409 even before Zoom is set up.
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM zoom_connections WHERE user_id = $1")
        .bind(&session.user_id)
        .fetch_optional(&state.db)
        .await?;
    match status.as_deref() {
        None => return Err(TokenError::NotConnected.into()),
        Some("active") => {}
        Some(_) => return Err(TokenError::NeedsReauth.into()),
    }
    let config = ZoomConfig::from_env().ok_or_else(not_configured)?;
    let zoom = ZoomHttp::new(config.clone());
    let body = mint_tokens(&state.db, state.credential_cipher.as_deref(), &zoom, &config, &session.user_id, &session.url_key).await?;
    Ok(Json(body).into_response())
}

/// `{ obf_token, sdk_jwt, app_privilege_token | null }` for a session owner.
pub(crate) async fn mint_tokens(
    db: &PgPool,
    cipher: Option<&allternit_cloud_core::CredentialCipher>,
    zoom: &dyn ZoomApi,
    config: &ZoomConfig,
    owner: &str,
    url_key: &str,
) -> Result<Value> {
    let access = fresh_access_token(db, cipher, zoom, owner).await?;
    let obf = zoom.obf_token(&access).await.map_err(|error| {
        tracing::warn!(%owner, %error, "zoom: OBF token fetch failed");
        MeetingError::from(TokenError::Upstream)
    })?;
    // Works only when the user hosts the meeting; on any error the sidecar
    // asks the host for recording permission instead.
    let privilege = match meeting_number(url_key) {
        Some(number) => zoom.local_recording_token(&access, number).await.ok(),
        None => None,
    };
    Ok(json!({
        "obf_token": obf,
        "sdk_jwt": sign_sdk_jwt(config, Utc::now().timestamp()),
        "app_privilege_token": privilege,
    }))
}

#[cfg(test)]
#[path = "zoom_tests.rs"]
mod tests;
