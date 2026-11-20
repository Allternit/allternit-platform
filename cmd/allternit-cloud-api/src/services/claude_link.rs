//! Link your Claude organization: the link flow
//! (CONTRACT-oauth-linked-billing-2026-11.md, Part B, section B2).
//!
//! - **Profile.** One Anthropic user profile (`uprof_…`) per account, made with
//!   the operator token at `POST {API}/v1/user_profiles` and reused on every
//!   re-link (`anthropic_link_profiles`). No passthrough: Allternit is not a
//!   reseller.
//! - **Start.** A PKCE verifier (86 chars) sealed with the credential cipher
//!   and a single-use `state` go into `anthropic_link_requests`; the pushed
//!   authorization request (`POST {AUTH}/v1/oauth/par`, JSON, client
//!   assertion) answers a `request_uri` the browser takes to
//!   `{AUTH}/oauth/authorize?client_id=…&request_uri=…`.
//! - **Callback.** The request is taken by `state` under `FOR UPDATE` (single
//!   use, 15 minutes), the code is exchanged at once at
//!   `POST {AUTH}/v1/oauth/token`, and `connection_id` plus the sealed refresh
//!   token are stored. The access token is never stored. The browser always
//!   gets a URL back: the return URL with `connected=claude-org` or
//!   `error=<code>`.
//! - **Status.** `GET {API}/v1/user_profiles/{id}?oauth_app_id=<our OAuth
//!   client id>` (Anthropic's example passes the client id there) with
//!   `anthropic-beta: user-profiles-search-2026-08-30`, cached 60 s per user.
//!   The answer is read from `connection_status.type` (`connected` /
//!   `disconnected` / `not_connected`, with `last_revoked_reason.type` when
//!   disconnected); older guesses at the shape stay as fallbacks. Anthropic
//!   states no organization name or spend limit: `org_name` is kept only if a
//!   field carries one, and `spend_limit` comes only from our own last linked
//!   request ([`note_spend_limit`]).
//! - **Unlink / erase.** Revoke at `POST {AUTH}/v1/oauth/revoke` by
//!   `connection_id` (best effort), then the connection row goes; the profile
//!   row stays so a re-link reuses it.
//!
//! The refresh token is never refreshed on a timer: billing uses the operator
//! token (contract B3). [`ClaudeLink::refresh_connection`] exists for later use
//! only.
//!
//! Billing (`routes::model_router::anthropic_linked`) reads the link with
//! [`linked_profile`], from the DB by the caller's user id.
//!
//! Every API call sends `anthropic-version` from [`anthropic_version`]
//! (`2026-10-21`, the version Anthropic's user profile and Linked billing
//! examples use; env `ALLTERNIT_ANTHROPIC_VERSION` overrides it). Profile
//! create sends no beta header and an empty body (no passthrough fields).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant};

use allternit_cloud_core::CredentialCipher;
use axum::http::StatusCode;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rand::RngCore;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::auth::anthropic_client::{operator_tokens, AnthropicClient, AssertionEndpoint, OperatorToken};
use crate::error::ApiError;
use crate::services::inference_keys::{decrypt_parts, encrypt_parts};

pub const REDIRECT_ENV: &str = "ALLTERNIT_CLAUDE_LINK_REDIRECT_URI";
pub const DEFAULT_REDIRECT: &str = "https://api.allternit.com/api/v1/claude-link/callback";
/// Where the callback sends the browser when the request named no return URL.
pub const DEFAULT_RETURN: &str = "https://ai.allternit.com/connections";
/// `connected=<this>` on success (the connect kit row id).
pub const CONNECTED: &str = "claude-org";
/// Return URLs a client may ask for. Same allow-list as Copilot
/// (`services::copilot::RETURN_PREFIXES`, connections contract 9.5); keep the two equal.
const RETURN_PREFIXES: [&str; 4] = ["https://ai.allternit.com/", "https://m.allternit.com/", "allternit://", "gizzi://"];
pub const SCOPE: &str = "billing:attribution offline_access";
pub const STATUS_BETA: &str = "user-profiles-search-2026-08-30";
/// Overrides the `anthropic-version` header on every Anthropic API call.
pub const ANTHROPIC_VERSION_ENV: &str = "ALLTERNIT_ANTHROPIC_VERSION";
/// The `anthropic-version` Anthropic's user profile and Linked billing examples use.
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2026-10-21";
const STATUS_TTL: StdDuration = StdDuration::from_secs(60);
const RELINK_WINDOW_DAYS: i64 = 14;
/// Anthropic's refresh token life when the reply does not say (contract B3: 1 year).
const DEFAULT_REFRESH_SECS: i64 = 365 * 24 * 3600;
/// `request_uri` life when the PAR reply does not say (contract B2: 600 s).
const DEFAULT_PAR_SECS: i64 = 600;
const HTTP_TIMEOUT: StdDuration = StdDuration::from_secs(15);

/// A refusal the routes send as `{"error":{"code","message"}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl LinkError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }
    pub fn not_available() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, crate::auth::anthropic_client::NOT_AVAILABLE_YET, "Not available yet")
    }
    fn could_not_start() -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "could_not_start", "Could not reach Claude. Try again")
    }
    fn internal(detail: impl std::fmt::Display) -> Self {
        tracing::error!(%detail, "claude link: internal error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "Something went wrong. Try again")
    }
}

impl From<sqlx::Error> for LinkError {
    fn from(e: sqlx::Error) -> Self {
        Self::internal(e)
    }
}

impl From<ApiError> for LinkError {
    fn from(e: ApiError) -> Self {
        Self::internal(e)
    }
}

// ─── what step 10 reads ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Linked,
    /// Linked, but the refresh token ends within 14 days.
    RelinkNeeded,
    Disconnected,
}

/// The caller's link as stored. `None` from [`linked_profile`] means unlinked.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedProfile {
    /// The `anthropic-user-profile-id` header value.
    pub user_profile_id: String,
    pub connection_id: Option<String>,
    pub state: LinkState,
    pub linked_at: DateTime<Utc>,
    pub refresh_expires_at: DateTime<Utc>,
}

/// The `anthropic-version` header value: [`ANTHROPIC_VERSION_ENV`] when set,
/// else [`DEFAULT_ANTHROPIC_VERSION`].
pub fn anthropic_version() -> String {
    std::env::var(ANTHROPIC_VERSION_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_VERSION.to_string())
}

fn relink_due(refresh_expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    refresh_expires_at - now < Duration::days(RELINK_WINDOW_DAYS)
}

/// The caller's linked Claude organization, read from the DB by user id (never
/// from the request). DB only: no Anthropic call, no feature check.
pub async fn linked_profile(db: &PgPool, user_id: &str) -> Result<Option<LinkedProfile>, sqlx::Error> {
    let row: Option<(String, Option<String>, String, Option<String>, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT user_profile_id, connection_id, status, last_error, linked_at, refresh_expires_at FROM anthropic_connections WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(|(user_profile_id, connection_id, status, last_error, linked_at, refresh_expires_at)| {
        let state = match (status.as_str(), last_error.as_deref()) {
            // Anthropic said `not_connected` (see `status`): unlinked.
            ("disconnected", Some("not_connected")) => return None,
            ("disconnected", _) => LinkState::Disconnected,
            _ if relink_due(refresh_expires_at, Utc::now()) => LinkState::RelinkNeeded,
            _ => LinkState::Linked,
        };
        Some(LinkedProfile { user_profile_id, connection_id, state, linked_at, refresh_expires_at })
    }))
}

// ─── status ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/claude-link`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkStatus {
    /// `unlinked` | `linked` | `relink_needed` | `disconnected`.
    pub state: &'static str,
    pub org_name: Option<String>,
    pub linked_at: Option<DateTime<Utc>>,
    pub relink_by: Option<DateTime<Utc>>,
    pub spend_limit_reached: Option<bool>,
}

impl LinkStatus {
    fn unlinked() -> Self {
        Self { state: "unlinked", org_name: None, linked_at: None, relink_by: None, spend_limit_reached: None }
    }

    pub fn to_json(&self) -> Value {
        let iso = |t: &Option<DateTime<Utc>>| t.map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true));
        json!({
            "state": self.state,
            "org_name": self.org_name,
            "linked_at": iso(&self.linked_at),
            "relink_by": iso(&self.relink_by),
            "spend_limit": self.spend_limit_reached.map(|reached| json!({ "reached": reached })),
        })
    }
}

fn status_cache() -> &'static Mutex<HashMap<String, (Instant, LinkStatus)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, LinkStatus)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached_status(user_id: &str) -> Option<LinkStatus> {
    let cache = status_cache().lock().ok()?;
    cache.get(user_id).filter(|(at, _)| at.elapsed() < STATUS_TTL).map(|(_, s)| s.clone())
}

fn cache_status(user_id: &str, status: &LinkStatus) {
    if let Ok(mut cache) = status_cache().lock() {
        cache.insert(user_id.to_string(), (Instant::now(), status.clone()));
    }
}

fn forget_status(user_id: &str) {
    if let Ok(mut cache) = status_cache().lock() {
        cache.remove(user_id);
    }
}

/// Per user: when our last linked request was answered
/// `linked_organization_spend_limit_reached` (`Some`), or that a linked
/// request went through since (`None`). In memory only: Anthropic states no
/// spend limit, so this is the only source, and a restart forgets it.
fn spend_notes() -> &'static Mutex<HashMap<String, Option<DateTime<Utc>>>> {
    static NOTES: OnceLock<Mutex<HashMap<String, Option<DateTime<Utc>>>>> = OnceLock::new();
    NOTES.get_or_init(Default::default)
}

/// Record a linked request's outcome for the status `spend_limit` field:
/// `reached` after a 402 spend-limit refusal, not reached after a request
/// Anthropic billed. A change drops the cached status so the next read shows it.
pub fn note_spend_limit(user_id: &str, reached: bool) {
    let Ok(mut notes) = spend_notes().lock() else { return };
    let changed = match (notes.get(user_id), reached) {
        (Some(Some(_)), true) | (Some(None), false) => false,
        _ => true,
    };
    if changed {
        notes.insert(user_id.to_string(), reached.then(Utc::now));
        drop(notes);
        forget_status(user_id);
    }
}

/// `Some(reached)` once a linked request of this user has been answered.
fn spend_limit_note(user_id: &str) -> Option<bool> {
    spend_notes().lock().ok()?.get(user_id).map(Option::is_some)
}

/// What Anthropic says about the profile's connection to our app. Anthropic
/// documents `connection_status.type` (`connected` / `disconnected` /
/// `not_connected`), `connection_status.last_revoked_reason.type` and a list
/// of connections under `oauth_connections.data`. `connection_status.type` is
/// read first; the older guesses (a `status` word on the profile or on a
/// connection entry) are fallbacks only.
#[derive(Debug, Default, PartialEq)]
struct RemoteStatus {
    status: Option<String>,
    org_name: Option<String>,
    revoked_reason: Option<String>,
}

fn parse_remote(body: &Value, our_ids: &[&str]) -> RemoteStatus {
    let lists = [body.pointer("/oauth_connections/data"), body.get("oauth_connections"), body.get("connections")];
    let entry = lists.into_iter().flatten().filter_map(Value::as_array).find_map(|list| {
        let ours = |c: &&Value| {
            ["oauth_app_id", "oauth_client_id", "client_id"]
                .iter()
                .any(|key| c.get(*key).and_then(Value::as_str).is_some_and(|id| our_ids.contains(&id)))
        };
        list.iter().find(ours).or_else(|| (list.len() == 1).then(|| &list[0]))
    });
    let text = |pointers: &[&str]| -> Option<String> {
        [Some(body), entry].into_iter().flatten().find_map(|obj| {
            pointers.iter().find_map(|p| obj.pointer(p).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
        })
    };
    let documented = text(&["/connection_status/type"]);
    RemoteStatus {
        status: documented.or_else(|| text(&["/status", "/connection_status", "/connection/status"])),
        org_name: text(&["/organization/name", "/organization_name", "/org_name", "/connection/organization/name"]),
        revoked_reason: text(&[
            "/connection_status/last_revoked_reason/type",
            "/last_revoked_reason/type",
            "/connection/last_revoked_reason/type",
        ]),
    }
}

#[derive(sqlx::FromRow)]
struct ConnectionRow {
    user_profile_id: String,
    connection_id: Option<String>,
    org_name: Option<String>,
    status: String,
    linked_at: DateTime<Utc>,
    refresh_expires_at: DateTime<Utc>,
    last_error: Option<String>,
}

impl ConnectionRow {
    /// The status from the row alone (Anthropic unreachable).
    fn local_status(&self) -> LinkStatus {
        let state = match (self.status.as_str(), self.last_error.as_deref()) {
            ("disconnected", Some("not_connected")) => return LinkStatus::unlinked(),
            ("disconnected", _) => "disconnected",
            _ if relink_due(self.refresh_expires_at, Utc::now()) => "relink_needed",
            _ => "linked",
        };
        self.with_state(state)
    }

    fn with_state(&self, state: &'static str) -> LinkStatus {
        LinkStatus {
            state,
            org_name: self.org_name.clone(),
            linked_at: Some(self.linked_at),
            relink_by: (state != "disconnected").then_some(self.refresh_expires_at),
            spend_limit_reached: None,
        }
    }
}

// ─── the service ──────────────────────────────────────────────────────────────

/// Everything the link routes need. Built per request by [`service`].
pub struct ClaudeLink {
    db: PgPool,
    cipher: Arc<CredentialCipher>,
    client: AnthropicClient,
    http: reqwest::Client,
    redirect_uri: String,
}

fn http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().timeout(HTTP_TIMEOUT).build().unwrap_or_default()).clone()
}

/// `None` unless the feature is configured
/// ([`crate::auth::anthropic_client::is_configured`]) and the credential
/// cipher is set (the verifier and refresh token are sealed with it).
pub fn service(state: &crate::ApiState) -> Option<ClaudeLink> {
    let client = AnthropicClient::from_env().ok()?;
    let cipher = state.credential_cipher.clone()?;
    let redirect_uri = std::env::var(REDIRECT_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_REDIRECT.to_string());
    Some(ClaudeLink { db: state.db.clone(), cipher, client, http: http(), redirect_uri })
}

fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// S256: base64url(sha256(verifier)), no padding.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `url` with `connected=claude-org` or `error=<code>` added.
fn with_outcome(url: &str, error: Option<&str>) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    match error {
        None => format!("{url}{sep}connected={CONNECTED}"),
        Some(code) => format!("{url}{sep}error={code}"),
    }
}

/// The callback answer when the feature is off.
pub fn not_configured_return() -> String {
    with_outcome(DEFAULT_RETURN, Some("not_configured"))
}

fn check_return(return_to: Option<&str>) -> Result<Option<String>, LinkError> {
    match return_to.map(str::trim).filter(|v| !v.is_empty()) {
        Some(url) if RETURN_PREFIXES.iter().any(|p| url.starts_with(p)) => Ok(Some(url.to_string())),
        Some(_) => Err(LinkError::new(StatusCode::BAD_REQUEST, "bad_return_to", "return_to must be an Allternit app or site URL.")),
        None => Ok(None),
    }
}

/// The OAuth error code of an auth.claude.com reply: RFC 6749 `error` string,
/// or Anthropic's `{"error":{"type"|"code"|"details":{"error_code"}}}`.
fn oauth_error(body: &Value) -> Option<String> {
    let error = body.get("error")?;
    error
        .as_str()
        .or_else(|| error.pointer("/details/error_code").and_then(Value::as_str))
        .or_else(|| error.get("code").and_then(Value::as_str))
        .or_else(|| error.get("type").and_then(Value::as_str))
        .map(str::to_string)
}

/// A callback `error=` for a refused code exchange.
fn exchange_outcome(status: u16, body: &Value) -> &'static str {
    match oauth_error(body).as_deref() {
        Some("unauthorized_client") => {
            // Owner gate G3: the OAuth app has no default workspace.
            tracing::error!(status, "claude link: code exchange answered unauthorized_client; set a default workspace on the Anthropic OAuth app");
            "not_configured"
        }
        Some("user_profile_already_linked") => "user_profile_already_linked",
        Some("org_not_eligible" | "user_profile_not_billable") => "org_not_eligible",
        other => {
            tracing::warn!(status, error = other.unwrap_or("-"), "claude link: code exchange refused");
            "exchange_failed"
        }
    }
}

enum Taken {
    Missing,
    /// Used or expired; carries its return URL.
    Stale(Option<String>),
    Fresh { user_id: String, verifier_ct: Vec<u8>, verifier_nonce: Vec<u8>, return_to: Option<String> },
}

impl ClaudeLink {
    /// One call to `{API}` with the operator token. A 401 drops the token and
    /// tries once more with a fresh one (it is our token, contract B4).
    async fn api_request(&self, method: reqwest::Method, path: &str, body: Option<&Value>, beta: Option<&str>) -> Result<(u16, Value), String> {
        let tokens = operator_tokens();
        let token = tokens.get(&self.client).await.map_err(|e| e.to_string())?;
        let (status, value) = self.api_send(&token, method.clone(), path, body, beta).await?;
        if status != 401 {
            return Ok((status, value));
        }
        tokens.invalidate(&token).await;
        let token = tokens.get(&self.client).await.map_err(|e| e.to_string())?;
        self.api_send(&token, method, path, body, beta).await
    }

    async fn api_send(&self, token: &OperatorToken, method: reqwest::Method, path: &str, body: Option<&Value>, beta: Option<&str>) -> Result<(u16, Value), String> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.client.api_base()))
            .bearer_auth(token.bearer())
            .header("anthropic-version", anthropic_version());
        if let Some(beta) = beta {
            request = request.header("anthropic-beta", beta);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|e| e.without_url().to_string())?;
        let status = response.status().as_u16();
        Ok((status, response.json::<Value>().await.unwrap_or(Value::Null)))
    }

    /// A JSON POST to `{AUTH}` with the client assertion fields added.
    async fn auth_post(&self, path: &str, endpoint: AssertionEndpoint, mut body: Map<String, Value>) -> Result<(u16, Value), String> {
        for (key, value) in self.client.client_assertion_fields(endpoint) {
            body.insert(key.to_string(), Value::String(value));
        }
        let response = self
            .http
            .post(format!("{}{path}", self.client.auth_base()))
            .json(&Value::Object(body))
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        let status = response.status().as_u16();
        Ok((status, response.json::<Value>().await.unwrap_or(Value::Null)))
    }

    async fn profile_id(&self, user_id: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT user_profile_id FROM anthropic_link_profiles WHERE user_id = $1").bind(user_id).fetch_optional(&self.db).await
    }

    /// The account's Anthropic user profile, made once and reused.
    pub async fn ensure_profile(&self, user_id: &str) -> Result<String, LinkError> {
        if let Some(id) = self.profile_id(user_id).await? {
            return Ok(id);
        }
        // No passthrough fields: Allternit is not a reseller (contract Part B).
        let (status, body) = self.api_request(reqwest::Method::POST, "/v1/user_profiles", Some(&json!({})), None).await.map_err(|e| {
            tracing::warn!(error = %e, "claude link: user profile create did not complete");
            LinkError::could_not_start()
        })?;
        let id = body.get("id").and_then(Value::as_str).filter(|id| (200..300).contains(&status) && !id.is_empty());
        let Some(id) = id else {
            tracing::warn!(status, "claude link: user profile create refused");
            return Err(LinkError::could_not_start());
        };
        // A concurrent start may have stored one first: theirs is kept.
        sqlx::query("INSERT INTO anthropic_link_profiles (user_id, user_profile_id) VALUES ($1, $2) ON CONFLICT (user_id) DO NOTHING")
            .bind(user_id)
            .bind(id)
            .execute(&self.db)
            .await?;
        self.profile_id(user_id).await?.ok_or_else(|| LinkError::internal("profile row missing after insert"))
    }

    /// Start a link: the authorize URL and when its `request_uri` expires.
    pub async fn start(&self, user_id: &str, return_to: Option<&str>) -> Result<(String, DateTime<Utc>), LinkError> {
        let return_to = check_return(return_to)?;
        let user_profile_id = self.ensure_profile(user_id).await?;
        // 64 random bytes: an 86-character verifier (PKCE allows 43 to 128).
        let verifier = random_b64(64);
        let challenge = pkce_challenge(&verifier);
        let state = random_b64(32);
        let (verifier_ct, verifier_nonce) = encrypt_parts(&self.cipher, &verifier)?;
        sqlx::query(
            "INSERT INTO anthropic_link_requests (state, user_id, code_verifier_ciphertext, code_verifier_nonce, return_to) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&state)
        .bind(user_id)
        .bind(&verifier_ct)
        .bind(&verifier_nonce)
        .bind(&return_to)
        .execute(&self.db)
        .await?;

        let mut body = Map::new();
        for (key, value) in [
            ("response_type", "code"),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("scope", SCOPE),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("connection_type", "organization_shared"),
            ("connection_auth_target", "platform"),
            ("user_profile_id", user_profile_id.as_str()),
        ] {
            body.insert(key.to_string(), Value::String(value.to_string()));
        }
        let par = self.auth_post("/v1/oauth/par", AssertionEndpoint::Par, body).await;
        let request_uri = match &par {
            Ok((status, reply)) if (200..300).contains(status) => reply.get("request_uri").and_then(Value::as_str).filter(|v| !v.is_empty()),
            _ => None,
        };
        let Some(request_uri) = request_uri else {
            match &par {
                Ok((status, reply)) => tracing::warn!(status, error = oauth_error(reply).as_deref().unwrap_or("-"), "claude link: PAR refused"),
                Err(e) => tracing::warn!(error = %e, "claude link: PAR did not complete"),
            }
            sqlx::query("DELETE FROM anthropic_link_requests WHERE state = $1").bind(&state).execute(&self.db).await?;
            return Err(LinkError::could_not_start());
        };
        let expires_in = par.as_ref().ok().and_then(|(_, r)| r.get("expires_in").and_then(Value::as_i64)).unwrap_or(DEFAULT_PAR_SECS);
        let url = reqwest::Url::parse_with_params(
            &format!("{}/oauth/authorize", self.client.auth_base()),
            &[("client_id", self.client.oauth_client_id()), ("request_uri", request_uri)],
        )
        .map_err(LinkError::internal)?;
        Ok((url.to_string(), Utc::now() + Duration::seconds(expires_in)))
    }

    /// Take the request named by `state`: single use whatever happens next.
    async fn take_request(&self, state: &str) -> Result<Taken, sqlx::Error> {
        let mut tx = self.db.begin().await?;
        let row: Option<(String, Vec<u8>, Vec<u8>, Option<String>, bool)> = sqlx::query_as(
            "SELECT user_id, code_verifier_ciphertext, code_verifier_nonce, return_to, (used_at IS NULL AND expires_at > now())
             FROM anthropic_link_requests WHERE state = $1 FOR UPDATE",
        )
        .bind(state)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, verifier_ct, verifier_nonce, return_to, usable)) = row else {
            tx.commit().await?;
            return Ok(Taken::Missing);
        };
        if !usable {
            tx.commit().await?;
            return Ok(Taken::Stale(return_to));
        }
        sqlx::query("UPDATE anthropic_link_requests SET used_at = now() WHERE state = $1").bind(state).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Taken::Fresh { user_id, verifier_ct, verifier_nonce, return_to })
    }

    /// The OAuth redirect. Returns the URL to send the browser to.
    pub async fn callback(&self, code: Option<&str>, state: Option<&str>, error: Option<&str>) -> String {
        let Some(state) = state.filter(|s| !s.is_empty()) else {
            return with_outcome(DEFAULT_RETURN, Some("bad_state"));
        };
        let (user_id, verifier_ct, verifier_nonce, return_to) = match self.take_request(state).await {
            Ok(Taken::Fresh { user_id, verifier_ct, verifier_nonce, return_to }) => (user_id, verifier_ct, verifier_nonce, return_to),
            Ok(Taken::Stale(return_to)) => return with_outcome(return_to.as_deref().unwrap_or(DEFAULT_RETURN), Some("bad_state")),
            Ok(Taken::Missing) => return with_outcome(DEFAULT_RETURN, Some("bad_state")),
            Err(e) => {
                tracing::warn!(error = %e, "claude link: request lookup failed");
                return with_outcome(DEFAULT_RETURN, Some("could_not_start"));
            }
        };
        let return_to = return_to.unwrap_or_else(|| DEFAULT_RETURN.to_string());
        if let Some(error) = error.filter(|e| !e.is_empty()) {
            let outcome = match error {
                "access_denied" => "denied",
                "user_profile_already_linked" => "user_profile_already_linked",
                "unauthorized_client" => "not_configured",
                _ => "could_not_start",
            };
            return with_outcome(&return_to, Some(outcome));
        }
        let Some(code) = code.filter(|c| !c.is_empty()) else {
            return with_outcome(&return_to, Some("denied"));
        };
        match self.finish(&user_id, code, &verifier_ct, &verifier_nonce).await {
            Ok(()) => with_outcome(&return_to, None),
            Err(outcome) => with_outcome(&return_to, Some(outcome)),
        }
    }

    async fn finish(&self, user_id: &str, code: &str, verifier_ct: &[u8], verifier_nonce: &[u8]) -> Result<(), &'static str> {
        let verifier = decrypt_parts(&self.cipher, verifier_ct, verifier_nonce).map_err(|_| "exchange_failed")?;
        let mut body = Map::new();
        for (key, value) in [("grant_type", "authorization_code"), ("code", code), ("redirect_uri", self.redirect_uri.as_str()), ("code_verifier", verifier.as_str())] {
            body.insert(key.to_string(), Value::String(value.to_string()));
        }
        let (status, reply) = self.auth_post("/v1/oauth/token", AssertionEndpoint::Token, body).await.map_err(|e| {
            tracing::warn!(error = %e, "claude link: code exchange did not complete");
            "exchange_failed"
        })?;
        if !(200..300).contains(&status) {
            return Err(exchange_outcome(status, &reply));
        }
        let connection_id = reply.get("connection_id").and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or("exchange_failed")?;
        // The access token in `reply` is dropped here: never stored.
        let sealed = match reply.get("refresh_token").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            Some(refresh) => Some(encrypt_parts(&self.cipher, refresh).map_err(|_| "exchange_failed")?),
            None => None,
        };
        let refresh_secs = reply.get("refresh_token_expires_in").and_then(Value::as_i64).filter(|s| *s > 0).unwrap_or(DEFAULT_REFRESH_SECS);
        let (ct, nonce) = sealed.map(|(c, n)| (Some(c), Some(n))).unwrap_or((None, None));
        let stored = sqlx::query(
            "INSERT INTO anthropic_connections
                (user_id, user_profile_id, connection_id, refresh_token_ciphertext, refresh_token_nonce, org_name, status, linked_at, refresh_expires_at, last_checked_at, last_error)
             SELECT p.user_id, p.user_profile_id, $2, $3, $4, NULL, 'linked', now(), now() + make_interval(secs => $5), NULL, NULL
             FROM anthropic_link_profiles p WHERE p.user_id = $1
             ON CONFLICT (user_id) DO UPDATE SET
                user_profile_id = excluded.user_profile_id, connection_id = excluded.connection_id,
                refresh_token_ciphertext = excluded.refresh_token_ciphertext, refresh_token_nonce = excluded.refresh_token_nonce,
                org_name = NULL, status = 'linked', linked_at = excluded.linked_at, refresh_expires_at = excluded.refresh_expires_at,
                last_checked_at = NULL, last_error = NULL",
        )
        .bind(user_id)
        .bind(connection_id)
        .bind(ct)
        .bind(nonce)
        .bind(refresh_secs as f64)
        .execute(&self.db)
        .await;
        match stored {
            Ok(r) if r.rows_affected() == 1 => {
                forget_status(user_id);
                Ok(())
            }
            Ok(_) => {
                tracing::warn!("claude link: no profile row for a finished link");
                Err("exchange_failed")
            }
            Err(e) => {
                tracing::warn!(error = %e, "claude link: storing the link failed");
                Err("exchange_failed")
            }
        }
    }

    async fn connection(&self, user_id: &str) -> Result<Option<ConnectionRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT user_profile_id, connection_id, org_name, status, linked_at, refresh_expires_at, last_error FROM anthropic_connections WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.db)
        .await
    }

    /// The link as Anthropic sees it, cached 60 s per user. Falls back to the
    /// stored status (uncached) when Anthropic cannot be read.
    pub async fn status(&self, user_id: &str) -> Result<LinkStatus, LinkError> {
        if let Some(status) = cached_status(user_id) {
            return Ok(status);
        }
        // The spend limit is our own record (Anthropic states none), on a live link only.
        let noted = |mut status: LinkStatus| {
            if matches!(status.state, "linked" | "relink_needed") {
                status.spend_limit_reached = spend_limit_note(user_id);
            }
            status
        };
        let Some(row) = self.connection(user_id).await? else {
            let status = LinkStatus::unlinked();
            cache_status(user_id, &status);
            return Ok(status);
        };
        // Anthropic's example names the OAuth client id here (`oauth_app_id=YOUR_CLIENT_ID`).
        let path = format!(
            "/v1/user_profiles/{}?oauth_app_id={}",
            urlencoding::encode(&row.user_profile_id),
            urlencoding::encode(self.client.oauth_client_id())
        );
        let body = match self.api_request(reqwest::Method::GET, &path, None, Some(STATUS_BETA)).await {
            Ok((status, body)) if (200..300).contains(&status) => body,
            Ok((status, _)) => {
                tracing::warn!(status, "claude link: status read refused; using the stored status");
                return Ok(noted(row.local_status()));
            }
            Err(e) => {
                tracing::warn!(error = %e, "claude link: status read did not complete; using the stored status");
                return Ok(noted(row.local_status()));
            }
        };
        let remote = parse_remote(&body, &[self.client.oauth_client_id(), self.client.oauth_app_id()]);
        let (status, db_status, last_error) = match remote.status.as_deref() {
            Some("connected") => {
                let state = if relink_due(row.refresh_expires_at, Utc::now()) { "relink_needed" } else { "linked" };
                let mut status = noted(row.with_state(state));
                status.org_name = remote.org_name.clone().or(row.org_name.clone());
                (status, state, None)
            }
            Some("disconnected") => {
                let mut status = row.with_state("disconnected");
                status.org_name = remote.org_name.clone().or(row.org_name.clone());
                (status, "disconnected", remote.revoked_reason.clone())
            }
            Some("not_connected") => (LinkStatus::unlinked(), "disconnected", Some("not_connected".to_string())),
            other => {
                tracing::warn!(status = other.unwrap_or("-"), "claude link: unknown connection status; using the stored status");
                return Ok(noted(row.local_status()));
            }
        };
        sqlx::query("UPDATE anthropic_connections SET status = $2, org_name = $3, last_error = $4, last_checked_at = now() WHERE user_id = $1")
            .bind(user_id)
            .bind(db_status)
            .bind(status.org_name.as_deref().or(row.org_name.as_deref()))
            .bind(last_error)
            .execute(&self.db)
            .await?;
        cache_status(user_id, &status);
        Ok(status)
    }

    /// Revoke at auth.claude.com by `connection_id` (or the profile when the
    /// connection id is unknown). `Err` carries a log line.
    async fn revoke_remote(&self, connection_id: Option<&str>, user_profile_id: &str) -> Result<(), String> {
        let mut body = Map::new();
        match connection_id.filter(|c| !c.is_empty()) {
            Some(id) => body.insert("connection_id".to_string(), Value::String(id.to_string())),
            None => body.insert("user_profile_id".to_string(), Value::String(user_profile_id.to_string())),
        };
        let (status, _) = self.auth_post("/v1/oauth/revoke", AssertionEndpoint::Revoke, body).await?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(format!("auth.claude.com answered {status}"))
        }
    }

    /// `DELETE /api/v1/claude-link`: revoke (best effort), drop the connection
    /// and its sealed token and any pending sign-in. The profile row stays.
    pub async fn unlink(&self, user_id: &str) -> Result<(), LinkError> {
        if let Some(row) = self.connection(user_id).await? {
            if let Err(error) = self.revoke_remote(row.connection_id.as_deref(), &row.user_profile_id).await {
                tracing::warn!(%error, "claude link: revoke failed; removing the link anyway");
            }
        }
        sqlx::query("DELETE FROM anthropic_connections WHERE user_id = $1").bind(user_id).execute(&self.db).await?;
        sqlx::query("DELETE FROM anthropic_link_requests WHERE user_id = $1").bind(user_id).execute(&self.db).await?;
        forget_status(user_id);
        Ok(())
    }

    /// Rotate one connection's refresh token. For re-link and connection-bound
    /// mode later (contract B3): one at a time under the row lock, no retry,
    /// no `scope`. A refused refresh with `invalid_grant` (Anthropic's answer
    /// to a replaced token being used again) marks the link disconnected.
    // No caller yet: billing uses the operator token and nothing refreshes on a timer.
    #[allow(dead_code)]
    pub async fn refresh_connection(&self, connection_id: &str) -> Result<(), LinkError> {
        let mut tx = self.db.begin().await?;
        let row: Option<(String, Option<Vec<u8>>, Option<Vec<u8>>)> = sqlx::query_as(
            "SELECT user_id, refresh_token_ciphertext, refresh_token_nonce FROM anthropic_connections WHERE connection_id = $1 FOR UPDATE",
        )
        .bind(connection_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((user_id, Some(ct), Some(nonce))) = row else {
            return Err(LinkError::new(StatusCode::CONFLICT, "relink_needed", "Link your Claude organization again"));
        };
        let refresh = decrypt_parts(&self.cipher, &ct, &nonce)?;
        let mut body = Map::new();
        body.insert("grant_type".to_string(), Value::String("refresh_token".to_string()));
        body.insert("refresh_token".to_string(), Value::String(refresh));
        let (status, reply) = self.auth_post("/v1/oauth/token", AssertionEndpoint::Token, body).await.map_err(|e| {
            tracing::warn!(error = %e, "claude link: refresh did not complete");
            LinkError::could_not_start()
        })?;
        if !(200..300).contains(&status) {
            let error = oauth_error(&reply);
            if error.as_deref() == Some("invalid_grant") {
                sqlx::query(
                    "UPDATE anthropic_connections SET status = 'disconnected', refresh_token_ciphertext = NULL, refresh_token_nonce = NULL,
                     last_error = 'refresh_token_reused' WHERE connection_id = $1",
                )
                .bind(connection_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                forget_status(&user_id);
                return Err(LinkError::new(StatusCode::UNAUTHORIZED, "claude_link_disconnected", "Your Claude organization is disconnected. Link it again."));
            }
            tracing::warn!(status, error = error.as_deref().unwrap_or("-"), "claude link: refresh refused");
            return Err(LinkError::could_not_start());
        }
        let rotated = reply.get("refresh_token").and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or_else(LinkError::could_not_start)?;
        let (ct, nonce) = encrypt_parts(&self.cipher, rotated)?;
        let refresh_secs = reply.get("refresh_token_expires_in").and_then(Value::as_i64).filter(|s| *s > 0);
        sqlx::query(
            "UPDATE anthropic_connections SET refresh_token_ciphertext = $2, refresh_token_nonce = $3,
             refresh_expires_at = COALESCE(now() + make_interval(secs => $4), refresh_expires_at) WHERE connection_id = $1",
        )
        .bind(connection_id)
        .bind(ct)
        .bind(nonce)
        .bind(refresh_secs.map(|s| s as f64))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Account erase: revoke the link at auth.claude.com while the feature is
/// configured. Best effort: never fails the erase. `account_erase::REVOKE`
/// already disconnected it locally and `ERASE` deletes the rows.
pub async fn revoke_for_erase(state: &crate::ApiState, user_id: &str) {
    let Some(service) = service(state) else { return };
    let row = match service.connection(user_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => {
            // Table missing (migration 151 not applied): nothing to revoke.
            tracing::warn!(%error, "claude link: erase lookup failed");
            return;
        }
    };
    if let Err(error) = service.revoke_remote(row.connection_id.as_deref(), &row.user_profile_id).await {
        tracing::warn!(%error, "claude link: revoke on erase failed");
    }
    forget_status(user_id);
}
