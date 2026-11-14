//! GitHub Copilot on the person's own GitHub account (CONTRACT-connections-2026-11
//! sections 9.5 and 9.11, brain runner step 1.5).
//!
//! - Sign-in is the GitHub App user-token flow of `allternit-copilot-connect`:
//!   `connect` hands out the authorize URL with a signed, single-use `state`;
//!   the App's callback exchanges the code for a `ghu_` user token, which is
//!   stored sealed with `CredentialCipher` together with its refresh token.
//! - A chat turn swaps the user token for a short-lived Copilot token
//!   (`/copilot_internal/v2/token`) and streams `POST <api>/chat/completions`
//!   from the Copilot API. Person-sent chat only: the route checks D7 first
//!   and only role + text reach Copilot (no tools).
//! - Disconnect and account erase revoke the grant at GitHub, then delete it.
//!
//! The row is on only when `ALLTERNIT_COPILOT_CLIENT_ID` and
//! `ALLTERNIT_COPILOT_CLIENT_SECRET` are set and the credential cipher is
//! configured; otherwise it stays the section 3 "Coming soon." row.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant};

use allternit_cloud_core::CredentialCipher;
use axum::http::StatusCode;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;

use crate::error::ApiError;
use crate::services::brain_runners::{ChatMessage, TurnEvent, TurnStream};

pub const DEFAULT_CALLBACK: &str = "https://api.allternit.com/api/v1/connections/copilot/callback";
pub const DEFAULT_RETURN: &str = "https://ai.allternit.com/connections?connected=copilot";
/// Return URLs a client may ask for when it starts the connect.
const RETURN_PREFIXES: [&str; 4] = ["https://ai.allternit.com/", "https://m.allternit.com/", "allternit://", "gizzi://"];
const STATE_MINUTES: i64 = 10;
/// D7 pace (same defaults as the plan rows): 20 turns in 5 minutes, one at a time.
const TURNS_PER_WINDOW: usize = 20;
const TURN_WINDOW: StdDuration = StdDuration::from_secs(300);
const USER_AGENT: &str = "Allternit-Cloud/1.0";

/// Server settings for the GitHub App. No `Debug`: it holds the client secret.
#[derive(Clone)]
pub struct CopilotConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub return_url: String,
    /// `https://github.com` (authorize, token exchange).
    pub github_base: String,
    /// `https://api.github.com` (user, Copilot token, grant revoke).
    pub api_base: String,
    /// `Copilot-Integration-Id` header, sent only when set.
    pub integration_id: Option<String>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl CopilotConfig {
    pub fn from_env() -> Option<Self> {
        Some(Self {
            client_id: env("ALLTERNIT_COPILOT_CLIENT_ID")?,
            client_secret: env("ALLTERNIT_COPILOT_CLIENT_SECRET")?,
            redirect_url: env("ALLTERNIT_COPILOT_REDIRECT_URL").unwrap_or_else(|| DEFAULT_CALLBACK.to_string()),
            return_url: env("ALLTERNIT_COPILOT_RETURN_URL").unwrap_or_else(|| DEFAULT_RETURN.to_string()),
            github_base: "https://github.com".to_string(),
            api_base: "https://api.github.com".to_string(),
            integration_id: env("ALLTERNIT_COPILOT_INTEGRATION_ID"),
        })
    }
}

#[cfg(test)]
tokio::task_local! {
    /// Tests point the service at a fake GitHub (or at nothing: `None`).
    pub(crate) static TEST_CONFIG: Option<CopilotConfig>;
}

fn config() -> Option<CopilotConfig> {
    #[cfg(test)]
    if let Ok(cfg) = TEST_CONFIG.try_with(Clone::clone) {
        return cfg;
    }
    CopilotConfig::from_env()
}

/// The service, or `None` when the App or the cipher is not configured
/// (the row then stays "Coming soon.").
pub fn service(state: &crate::ApiState) -> Option<CopilotService> {
    let cfg = config()?;
    let cipher = state.credential_cipher.clone()?;
    Some(CopilotService { db: state.db.clone(), cipher, cfg, http: http() })
}

fn http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .connect_timeout(StdDuration::from_secs(10))
                .build()
                .unwrap_or_default()
        })
        .clone()
}

/// A chat-turn refusal for the route to send as `{"error":{code,message}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
    fn not_connected() -> Self {
        Self::new(StatusCode::FORBIDDEN, "plan_not_connected", "Connect Copilot in Connections first.")
    }
    fn could_not_start() -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "could_not_start", "Could not start. Try again")
    }
    fn login_expired() -> Self {
        Self::new(StatusCode::CONFLICT, "login_expired", "Sign-in expired")
    }
}

/// What the row shows.
#[derive(Debug, Clone, PartialEq)]
pub enum GrantState {
    Off,
    Connecting,
    On { login: Option<String>, models: Vec<String> },
    Expired,
}

pub struct CopilotService {
    db: PgPool,
    cipher: Arc<CredentialCipher>,
    cfg: CopilotConfig,
    http: reqwest::Client,
}

type HmacSha256 = Hmac<Sha256>;

fn mac(secret: &str, payload: &str) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac takes any key");
    mac.update(b"copilot-state|");
    mac.update(payload.as_bytes());
    mac
}

pub fn sign_state(secret: &str, user_id: &str, nonce: &str, expires: i64) -> String {
    let payload = URL_SAFE_NO_PAD.encode(format!("{user_id}|{expires}|{nonce}"));
    let sig = hex::encode(mac(secret, &payload).finalize().into_bytes());
    format!("{payload}.{sig}")
}

/// (user, nonce) when the signature holds and it has not expired.
pub fn verify_state(secret: &str, state: &str, now: i64) -> Option<(String, String)> {
    let (payload, sig) = state.split_once('.')?;
    mac(secret, payload).verify_slice(&hex::decode(sig).ok()?).ok()?;
    let decoded = String::from_utf8(URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    let mut parts = decoded.splitn(3, '|');
    let (user, exp, nonce) = (parts.next()?, parts.next()?, parts.next()?);
    (exp.parse::<i64>().ok()? >= now).then(|| (user.to_string(), nonce.to_string()))
}

/// `url` with `connected=copilot` or `error=<code>` added.
fn with_outcome(url: &str, error: Option<&str>) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    match error {
        None if url.contains("connected=copilot") => url.to_string(),
        None => format!("{url}{sep}connected=copilot"),
        Some(code) => format!("{url}{sep}error={code}"),
    }
}

/// The user token's lifetimes from a GitHub token reply.
struct Tokens {
    access: String,
    access_expires: Option<DateTime<Utc>>,
    refresh: Option<String>,
    refresh_expires: Option<DateTime<Utc>>,
}

fn parse_tokens(body: &Value) -> Result<Tokens, String> {
    if let Some(error) = body.get("error").and_then(Value::as_str) {
        return Err(error.to_string());
    }
    let access = body.get("access_token").and_then(Value::as_str).ok_or("no_token")?.to_string();
    let at = |key: &str| body.get(key).and_then(Value::as_i64).map(|secs| Utc::now() + Duration::seconds(secs));
    Ok(Tokens {
        access,
        access_expires: at("expires_in"),
        refresh: body.get("refresh_token").and_then(Value::as_str).map(str::to_string),
        refresh_expires: at("refresh_token_expires_in"),
    })
}

/// A short-lived Copilot token per account, reused until a minute before it ends.
fn copilot_tokens() -> &'static Mutex<HashMap<String, (String, String, DateTime<Utc>)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (String, String, DateTime<Utc>)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

#[derive(Default)]
struct Pace {
    recent: VecDeque<Instant>,
    in_flight: bool,
}

fn pace() -> &'static Mutex<HashMap<String, Pace>> {
    static PACE: OnceLock<Mutex<HashMap<String, Pace>>> = OnceLock::new();
    PACE.get_or_init(Default::default)
}

/// Clears the account's in-flight mark when the turn's stream ends or drops.
struct TurnGuard(String);

impl Drop for TurnGuard {
    fn drop(&mut self) {
        if let Ok(mut pace) = pace().lock() {
            if let Some(entry) = pace.get_mut(&self.0) {
                entry.in_flight = false;
            }
        }
    }
}

fn begin_turn(user_id: &str) -> Result<TurnGuard, Refusal> {
    let mut pace = pace().lock().map_err(|_| Refusal::could_not_start())?;
    let entry = pace.entry(user_id.to_string()).or_default();
    let now = Instant::now();
    while entry.recent.front().is_some_and(|at| now.duration_since(*at) > TURN_WINDOW) {
        entry.recent.pop_front();
    }
    if entry.in_flight {
        return Err(Refusal::new(StatusCode::CONFLICT, "plan_turn_in_flight", "Wait for the reply before sending another message"));
    }
    if entry.recent.len() >= TURNS_PER_WINDOW {
        return Err(Refusal::new(StatusCode::TOO_MANY_REQUESTS, "plan_rate", "Too many messages. Wait a few minutes"));
    }
    entry.recent.push_back(now);
    entry.in_flight = true;
    Ok(TurnGuard(user_id.to_string()))
}

impl CopilotService {
    fn seal(&self, plain: &str) -> Result<String, ApiError> {
        self.cipher.encrypt(plain).map_err(|e| ApiError::Internal(format!("seal copilot token: {e}")))
    }

    fn open(&self, sealed: &str) -> Result<String, ApiError> {
        self.cipher.decrypt(sealed).map_err(|e| ApiError::Internal(format!("open copilot token: {e}")))
    }

    /// The GitHub authorize URL for this account. Stores a fresh single-use
    /// nonce (a new connect replaces an older one) and the return URL.
    pub async fn authorize_url(&self, user_id: &str, return_to: Option<&str>) -> Result<(String, DateTime<Utc>), ApiError> {
        let return_to = match return_to.map(str::trim).filter(|v| !v.is_empty()) {
            Some(url) if RETURN_PREFIXES.iter().any(|p| url.starts_with(p)) => url.to_string(),
            Some(_) => return Err(ApiError::BadRequest("return_to must be an Allternit app or site URL.".to_string())),
            None => self.cfg.return_url.clone(),
        };
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let expires = Utc::now() + Duration::minutes(STATE_MINUTES);
        sqlx::query(
            "INSERT INTO copilot_grants (user_id, pending_nonce, pending_return, pending_expires_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id) DO UPDATE SET pending_nonce = $2, pending_return = $3, pending_expires_at = $4, updated_at = now()",
        )
        .bind(user_id)
        .bind(&nonce)
        .bind(&return_to)
        .bind(expires)
        .execute(&self.db)
        .await?;
        let state = sign_state(&self.cfg.client_secret, user_id, &nonce, expires.timestamp());
        let url = reqwest::Url::parse_with_params(
            &format!("{}/login/oauth/authorize", self.cfg.github_base),
            &[("client_id", self.cfg.client_id.as_str()), ("redirect_uri", self.cfg.redirect_url.as_str()), ("state", state.as_str())],
        )
        .map_err(|e| ApiError::Internal(e.to_string()))?;
        Ok((url.to_string(), expires))
    }

    /// The App's redirect. Returns the URL to send the browser to: the
    /// client's return URL, with `?error=<code>` on failure.
    pub async fn callback(&self, code: Option<&str>, state: Option<&str>, github_error: Option<&str>) -> String {
        let Some((user, nonce)) = state.and_then(|s| verify_state(&self.cfg.client_secret, s, Utc::now().timestamp())) else {
            return with_outcome(&self.cfg.return_url, Some("bad_state"));
        };
        // Single use: the nonce is taken whatever happens next.
        let taken: Option<(Option<String>,)> = sqlx::query_as(
            "UPDATE copilot_grants SET pending_nonce = NULL, pending_expires_at = NULL, updated_at = now()
             WHERE user_id = $1 AND pending_nonce = $2 AND pending_expires_at > now()
             RETURNING pending_return",
        )
        .bind(&user)
        .bind(&nonce)
        .fetch_optional(&self.db)
        .await
        .unwrap_or(None);
        let Some((return_to,)) = taken else {
            return with_outcome(&self.cfg.return_url, Some("bad_state"));
        };
        let return_to = return_to.unwrap_or_else(|| self.cfg.return_url.clone());
        if github_error.is_some() {
            return with_outcome(&return_to, Some("denied"));
        }
        let Some(code) = code.filter(|c| !c.is_empty()) else {
            return with_outcome(&return_to, Some("denied"));
        };
        match self.finish(&user, code).await {
            Ok(()) => with_outcome(&return_to, None),
            Err(error) => {
                tracing::warn!(%error, "copilot callback failed");
                with_outcome(&return_to, Some(error))
            }
        }
    }

    async fn finish(&self, user_id: &str, code: &str) -> Result<(), &'static str> {
        let body = self
            .http
            .post(format!("{}/login/oauth/access_token", self.cfg.github_base))
            .header("accept", "application/json")
            .json(&json!({
                "client_id": self.cfg.client_id,
                "client_secret": self.cfg.client_secret,
                "code": code,
                "redirect_uri": self.cfg.redirect_url,
            }))
            .send()
            .await
            .map_err(|_| "could_not_start")?
            .json::<Value>()
            .await
            .map_err(|_| "could_not_start")?;
        let tokens = parse_tokens(&body).map_err(|_| "exchange_failed")?;
        let login = self.github_login(&tokens.access).await;
        // Only accounts with Copilot are stored; any other grant is given back.
        let models = match self.copilot_token(&tokens.access).await {
            Ok((token, api, _)) => self.list_models(&token, &api).await.unwrap_or_default(),
            Err(_) => {
                let _ = self.revoke_token(&tokens.access).await;
                return Err("no_copilot");
            }
        };
        let sealed_access = self.seal(&tokens.access).map_err(|_| "could_not_start")?;
        let sealed_refresh = match &tokens.refresh {
            Some(refresh) => Some(self.seal(refresh).map_err(|_| "could_not_start")?),
            None => None,
        };
        sqlx::query(
            "UPDATE copilot_grants SET github_login = $2, sealed_access = $3, access_expires_at = $4,
                 sealed_refresh = $5, refresh_expires_at = $6, models = $7, needs_reason = NULL, updated_at = now()
             WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(login)
        .bind(sealed_access)
        .bind(tokens.access_expires)
        .bind(sealed_refresh)
        .bind(tokens.refresh_expires)
        .bind(json!(models))
        .execute(&self.db)
        .await
        .map_err(|_| "could_not_start")?;
        copilot_tokens().lock().map(|mut c| c.remove(user_id)).ok();
        Ok(())
    }

    async fn github_login(&self, user_token: &str) -> Option<String> {
        let body: Value = self
            .http
            .get(format!("{}/user", self.cfg.api_base))
            .bearer_auth(user_token)
            .header("accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        body.get("login").and_then(Value::as_str).map(str::to_string)
    }

    /// (Copilot token, Copilot API base, expiry) for a GitHub user token.
    /// `Err(true)` when GitHub says the token is bad (sign-in expired).
    async fn copilot_token(&self, user_token: &str) -> Result<(String, String, DateTime<Utc>), bool> {
        let response = self
            .http
            .get(format!("{}/copilot_internal/v2/token", self.cfg.api_base))
            .header("authorization", format!("token {user_token}"))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|_| false)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(true);
        }
        let body: Value = response.error_for_status().map_err(|_| false)?.json().await.map_err(|_| false)?;
        let token = body.get("token").and_then(Value::as_str).ok_or(false)?.to_string();
        let api = body
            .pointer("/endpoints/api")
            .and_then(Value::as_str)
            .unwrap_or("https://api.githubcopilot.com")
            .trim_end_matches('/')
            .to_string();
        let expires = body
            .get("expires_at")
            .and_then(Value::as_i64)
            .and_then(|at| DateTime::<Utc>::from_timestamp(at, 0))
            .unwrap_or_else(|| Utc::now() + Duration::minutes(10));
        Ok((token, api, expires))
    }

    fn copilot_request(&self, builder: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
        let builder = builder.bearer_auth(token).header("editor-version", USER_AGENT);
        match &self.cfg.integration_id {
            Some(id) => builder.header("copilot-integration-id", id),
            None => builder,
        }
    }

    /// Chat models the account can pick, as bare model ids.
    pub async fn list_models(&self, token: &str, api: &str) -> Result<Vec<String>, ApiError> {
        let body: Value = self
            .copilot_request(self.http.get(format!("{api}/models")), token)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| ApiError::ServiceUnavailable(format!("copilot models: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::ServiceUnavailable(format!("copilot models: {e}")))?;
        let mut models: Vec<String> = body
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m.pointer("/capabilities/type").and_then(Value::as_str).unwrap_or("chat") == "chat")
            .filter(|m| m.get("model_picker_enabled").and_then(Value::as_bool) != Some(false))
            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        models.dedup();
        Ok(models)
    }

    /// The row's state for this account.
    pub async fn grant_state(&self, user_id: &str) -> Result<GrantState, ApiError> {
        let row: Option<(Option<String>, Option<String>, Value, Option<String>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT github_login, sealed_access, models, needs_reason, pending_expires_at FROM copilot_grants WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?;
        Ok(match row {
            None => GrantState::Off,
            Some((_, Some(_), _, Some(_), _)) => GrantState::Expired,
            Some((login, Some(_), models, None, _)) => GrantState::On {
                login,
                models: serde_json::from_value::<Vec<String>>(models).unwrap_or_default(),
            },
            Some((_, None, _, _, Some(pending))) if pending > Utc::now() => GrantState::Connecting,
            Some(_) => GrantState::Off,
        })
    }

    /// A user token that is good now, refreshed and re-sealed when it is
    /// about to end. `Err` is the refusal for the chat route.
    async fn user_token(&self, user_id: &str) -> Result<String, Refusal> {
        let row: Option<(Option<String>, Option<DateTime<Utc>>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT sealed_access, access_expires_at, sealed_refresh, needs_reason FROM copilot_grants WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|_| Refusal::could_not_start())?;
        let Some((Some(sealed), expires, refresh, needs)) = row else {
            return Err(Refusal::not_connected());
        };
        if needs.is_some() {
            return Err(Refusal::login_expired());
        }
        if expires.map_or(true, |at| at > Utc::now() + Duration::minutes(2)) {
            return self.open(&sealed).map_err(|_| Refusal::could_not_start());
        }
        let Some(refresh) = refresh.and_then(|r| self.open(&r).ok()) else {
            self.mark_expired(user_id).await;
            return Err(Refusal::login_expired());
        };
        let body: Value = self
            .http
            .post(format!("{}/login/oauth/access_token", self.cfg.github_base))
            .header("accept", "application/json")
            .json(&json!({
                "client_id": self.cfg.client_id,
                "client_secret": self.cfg.client_secret,
                "grant_type": "refresh_token",
                "refresh_token": refresh,
            }))
            .send()
            .await
            .map_err(|_| Refusal::could_not_start())?
            .json()
            .await
            .map_err(|_| Refusal::could_not_start())?;
        let tokens = match parse_tokens(&body) {
            Ok(tokens) => tokens,
            Err(_) => {
                self.mark_expired(user_id).await;
                return Err(Refusal::login_expired());
            }
        };
        let sealed_access = self.seal(&tokens.access).map_err(|_| Refusal::could_not_start())?;
        let sealed_refresh = tokens.refresh.as_deref().map(|r| self.seal(r)).transpose().map_err(|_| Refusal::could_not_start())?;
        sqlx::query(
            "UPDATE copilot_grants SET sealed_access = $2, access_expires_at = $3,
                 sealed_refresh = COALESCE($4, sealed_refresh), refresh_expires_at = COALESCE($5, refresh_expires_at), updated_at = now()
             WHERE user_id = $1",
        )
        .bind(user_id)
        .bind(sealed_access)
        .bind(tokens.access_expires)
        .bind(sealed_refresh)
        .bind(tokens.refresh_expires)
        .execute(&self.db)
        .await
        .map_err(|_| Refusal::could_not_start())?;
        Ok(tokens.access)
    }

    async fn mark_expired(&self, user_id: &str) {
        let _ = sqlx::query("UPDATE copilot_grants SET needs_reason = 'login_expired', updated_at = now() WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.db)
            .await;
        copilot_tokens().lock().map(|mut c| c.remove(user_id)).ok();
    }

    async fn session_token(&self, user_id: &str) -> Result<(String, String), Refusal> {
        if let Some((token, api, expires)) = copilot_tokens().lock().ok().and_then(|c| c.get(user_id).cloned()) {
            if expires > Utc::now() + Duration::minutes(1) {
                return Ok((token, api));
            }
        }
        let user_token = self.user_token(user_id).await?;
        match self.copilot_token(&user_token).await {
            Ok((token, api, expires)) => {
                copilot_tokens().lock().map(|mut c| c.insert(user_id.to_string(), (token.clone(), api.clone(), expires))).ok();
                Ok((token, api))
            }
            Err(true) => {
                self.mark_expired(user_id).await;
                Err(Refusal::login_expired())
            }
            Err(false) => Err(Refusal::new(
                StatusCode::CONFLICT,
                "vendor_limited",
                "GitHub did not allow Copilot for this account. Check your Copilot plan.",
            )),
        }
    }

    /// One streamed chat turn on the person's Copilot. `messages` carry only
    /// role and text (the route drops tools); history is replayed each turn.
    pub async fn turn(&self, user_id: &str, model: &str, messages: Vec<ChatMessage>) -> Result<TurnStream, Refusal> {
        if let Some(bad) = messages.iter().find(|m| !matches!(m.role.as_str(), "system" | "user" | "assistant")) {
            return Err(Refusal::new(StatusCode::BAD_REQUEST, "plan_no_tools", format!("Copilot chats can't carry a {} message.", bad.role)));
        }
        let guard = begin_turn(user_id)?;
        let (token, api) = self.session_token(user_id).await?;
        let body = json!({
            "model": model,
            "stream": true,
            "messages": messages.iter().map(|m| json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
        });
        let response = self
            .copilot_request(self.http.post(format!("{api}/chat/completions")), &token)
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|_| Refusal::could_not_start())?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            tracing::warn!(%status, "copilot chat refused");
            return Err(match status.as_u16() {
                401 => {
                    copilot_tokens().lock().map(|mut c| c.remove(user_id)).ok();
                    Refusal::login_expired()
                }
                402 | 403 | 429 => Refusal::new(StatusCode::CONFLICT, "vendor_limited", vendor_message(&text)),
                400 | 404 | 422 => Refusal::new(StatusCode::BAD_REQUEST, "model_not_available", "Copilot does not offer that model for this account."),
                _ => Refusal::could_not_start(),
            });
        }

        struct Turn {
            response: reqwest::Response,
            buffer: String,
            pending: VecDeque<TurnEvent>,
            usage: (Option<u32>, Option<u32>),
            finished: bool,
            _guard: TurnGuard,
        }
        let turn = Turn { response, buffer: String::new(), pending: VecDeque::new(), usage: (None, None), finished: false, _guard: guard };
        let stream = futures::stream::unfold(turn, |mut turn| async move {
            loop {
                if let Some(event) = turn.pending.pop_front() {
                    return Some((event, turn));
                }
                if turn.finished {
                    return None;
                }
                match turn.response.chunk().await {
                    Ok(Some(bytes)) => {
                        turn.buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(end) = turn.buffer.find('\n') {
                            let line: String = turn.buffer.drain(..=end).collect();
                            let Some(data) = line.trim().strip_prefix("data:").map(str::trim) else { continue };
                            if data == "[DONE]" {
                                turn.finished = true;
                                let (input_tokens, output_tokens) = turn.usage;
                                turn.pending.push_back(TurnEvent::Done { input_tokens, output_tokens });
                                break;
                            }
                            let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue };
                            if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
                                let n = |k: &str| usage.get(k).and_then(Value::as_u64).map(|v| v as u32);
                                turn.usage = (n("prompt_tokens"), n("completion_tokens"));
                            }
                            for choice in chunk.get("choices").and_then(Value::as_array).into_iter().flatten() {
                                if let Some(text) = choice.pointer("/delta/content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                                    turn.pending.push_back(TurnEvent::Delta(text.to_string()));
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        turn.finished = true;
                        let (input_tokens, output_tokens) = turn.usage;
                        turn.pending.push_back(TurnEvent::Done { input_tokens, output_tokens });
                    }
                    Err(_) => {
                        turn.finished = true;
                        turn.pending.push_back(TurnEvent::Error { code: "could_not_start", message: "Could not start. Try again".to_string() });
                    }
                }
            }
        });
        Ok(Box::pin(stream))
    }

    async fn revoke_token(&self, user_token: &str) -> Result<(), ApiError> {
        let response = self
            .http
            .delete(format!("{}/applications/{}/grant", self.cfg.api_base, self.cfg.client_id))
            .basic_auth(&self.cfg.client_id, Some(&self.cfg.client_secret))
            .header("accept", "application/vnd.github+json")
            .json(&json!({ "access_token": user_token }))
            .send()
            .await
            .map_err(|e| ApiError::ServiceUnavailable(format!("copilot revoke: {e}")))?;
        // 404/422: GitHub no longer knows the token; it is gone either way.
        match response.status().as_u16() {
            200..=299 | 404 | 422 => Ok(()),
            status => Err(ApiError::ServiceUnavailable(format!("copilot revoke: GitHub answered {status}"))),
        }
    }

    /// Disconnect: revoke the grant at GitHub (best effort), then delete it.
    pub async fn disconnect(&self, user_id: &str) -> Result<(), ApiError> {
        let sealed: Option<Option<String>> = sqlx::query_scalar("SELECT sealed_access FROM copilot_grants WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&self.db)
            .await?;
        if let Some(token) = sealed.flatten().and_then(|s| self.open(&s).ok()) {
            if let Err(error) = self.revoke_token(&token).await {
                tracing::warn!(%error, "copilot revoke failed; deleting the stored grant anyway");
            }
        }
        sqlx::query("DELETE FROM copilot_grants WHERE user_id = $1").bind(user_id).execute(&self.db).await?;
        copilot_tokens().lock().map(|mut c| c.remove(user_id)).ok();
        Ok(())
    }
}

fn vendor_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
        .filter(|m| !m.is_empty() && m.len() < 300)
        .unwrap_or_else(|| "GitHub limited Copilot for this account".to_string())
}

/// Account erase: revoke the grant at GitHub while the App is configured.
/// The row itself is removed by `account_erase::ERASE`.
pub async fn revoke_for_erase(state: &crate::ApiState, user_id: &str) {
    let Some(service) = service(state) else { return };
    if let Err(error) = service.disconnect(user_id).await {
        // Table missing (migration 142 not applied) or GitHub down: the ERASE
        // row delete still runs; GitHub drops unused grants on its own.
        tracing::warn!(%error, "copilot grant revoke on erase failed");
    }
}

/// A fake GitHub + Copilot API for tests: one local server for both bases.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use axum::extract::State;
    use axum::routing::{delete, get, post};
    use axum::{Json, Router};

    #[derive(Default)]
    pub struct Calls {
        pub token_bodies: Vec<Value>,
        pub chat_bodies: Vec<Value>,
        pub revoked: Vec<String>,
    }

    pub struct Fake {
        pub base: String,
        pub calls: Arc<Mutex<Calls>>,
    }

    impl Fake {
        pub fn config(&self) -> CopilotConfig {
            CopilotConfig {
                client_id: "Iv_test".to_string(),
                client_secret: "test-secret".to_string(),
                redirect_url: DEFAULT_CALLBACK.to_string(),
                return_url: DEFAULT_RETURN.to_string(),
                github_base: self.base.clone(),
                api_base: self.base.clone(),
                integration_id: Some("allternit-test".to_string()),
            }
        }
    }

    type S = (Arc<Mutex<Calls>>, String);

    pub async fn start() -> Fake {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Calls::default()));
        let app = Router::new()
            .route(
                "/login/oauth/access_token",
                post(|State((calls, _)): State<S>, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().token_bodies.push(body.clone());
                    if body["code"] == "bad" {
                        return Json(json!({ "error": "bad_verification_code" }));
                    }
                    Json(json!({ "access_token": "ghu_fake_user_token", "expires_in": 28800,
                                 "refresh_token": "ghr_fake_refresh", "refresh_token_expires_in": 15897600, "token_type": "bearer" }))
                }),
            )
            .route("/user", get(|| async { Json(json!({ "login": "octo" })) }))
            .route(
                "/copilot_internal/v2/token",
                get(|State((_, base)): State<S>| async move {
                    Json(json!({ "token": "cop_session_token", "expires_at": Utc::now().timestamp() + 1500, "endpoints": { "api": base } }))
                }),
            )
            .route(
                "/models",
                get(|| async {
                    Json(json!({ "data": [
                        { "id": "gpt-5", "capabilities": { "type": "chat" }, "model_picker_enabled": true },
                        { "id": "claude-sonnet-4.5", "capabilities": { "type": "chat" } },
                        { "id": "text-embedding-3-small", "capabilities": { "type": "embeddings" } }
                    ]}))
                }),
            )
            .route(
                "/chat/completions",
                post(|State((calls, _)): State<S>, headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().chat_bodies.push(body);
                    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer cop_session_token") {
                        return (axum::http::StatusCode::UNAUTHORIZED, String::new());
                    }
                    let sse = [
                        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"Hi"}}]}"#,
                        r#"data: {"choices":[{"index":0,"delta":{"content":" there"}}]}"#,
                        r#"data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#,
                        "data: [DONE]",
                    ]
                    .join("\n\n");
                    (axum::http::StatusCode::OK, format!("{sse}\n\n"))
                }),
            )
            .route(
                "/applications/:client_id/grant",
                delete(|State((calls, _)): State<S>, Json(body): Json<Value>| async move {
                    calls.lock().unwrap().revoked.push(body["access_token"].as_str().unwrap_or_default().to_string());
                    axum::http::StatusCode::NO_CONTENT
                }),
            )
            .with_state((calls.clone(), base.clone()));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Fake { base, calls }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_signed_bound_and_expires() {
        let now = Utc::now().timestamp();
        let state = sign_state("secret", "user_1", "nonce_1", now + 600);
        assert_eq!(verify_state("secret", &state, now), Some(("user_1".to_string(), "nonce_1".to_string())));
        assert_eq!(verify_state("other", &state, now), None, "another secret");
        assert_eq!(verify_state("secret", &state, now + 601), None, "expired");
        let (payload, sig) = state.split_once('.').unwrap();
        let forged = URL_SAFE_NO_PAD.encode(format!("user_2|{}|nonce_1", now + 600));
        assert_eq!(verify_state("secret", &format!("{forged}.{sig}"), now), None, "{payload}");
    }
}
