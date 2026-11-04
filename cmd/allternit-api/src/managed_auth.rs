//! Managed connector authorization — runtime side (ID-JAG, SEP-990).
//!
//! An org admin turns on managed sign-in for a connector in cloud-api. A member
//! then connects with no sign-in screen:
//!
//! 1. `GET  <cloud>/api/v1/managed-auth/configs` — the configs this user may use.
//! 2. `POST <cloud>/api/v1/managed-auth/id-jag {connector_id, org_id?}` — cloud-api
//!    checks membership, role and verified email, signs an ID-JAG with the org's
//!    Allternit issuer key, and returns it with the vendor client credentials.
//! 3. RFC 7523 JWT-bearer grant at the vendor `token_endpoint`
//!    (`grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`, `assertion=<ID-JAG>`,
//!    client auth `client_secret_basic`), ported from gizzi `xaa.ts`
//!    (`exchangeJwtAuthGrant`).
//!
//! Both cloud calls use this runtime's device credential (`Authorization: Bearer
//! <device token>` + `x-allternit-runtime-id`), the same one the `runtime-devices/me/*`
//! calls use. The device token belongs to the user this runtime is paired as, so a
//! managed grant is refused for anyone else.
//!
//! Tokens are stored twice on purpose, written together by [`store`]:
//! `connector_connections` (auth_type `managed`, the connector's state and the
//! REST execute path) and an `mcp_connectors` + `mcp_oauth_sessions` pair for the
//! vendor's MCP server, which is what the chat-turn proxy and the MCP edge serve.
//! Expiry or a 401 triggers a new ID-JAG and a new grant with no UI.
//!
//! Never logged: the ID-JAG, the client secret, access or refresh tokens.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::relay_auth::RelaySecret;

pub const CONFIGS_PATH: &str = "/api/v1/managed-auth/configs";
pub const IDJAG_PATH: &str = "/api/v1/managed-auth/id-jag";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const CLOUD_TIMEOUT: Duration = Duration::from_secs(15);
/// The configs lookup runs before every plain connect, so a cloud outage must
/// not hold up the normal flow for long.
const CONFIGS_TIMEOUT: Duration = Duration::from_secs(5);
/// After a failed configs lookup, plain connects skip it for this long.
const CONFIGS_FAILURE_BACKOFF: Duration = Duration::from_secs(60);
const CONFIG_TTL: Duration = Duration::from_secs(60);
/// Lifetime assumed when the vendor's token response has no `expires_in`, so the
/// token is still renewed on a schedule (gizzi `xaa.ts` does the same).
const DEFAULT_EXPIRES_IN: i64 = 3600;
/// `mcp_oauth_sessions.state` prefix of a managed session.
pub const SESSION_STATE_PREFIX: &str = "managed:";

/// One enabled managed config the user may use. No secrets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedConfig {
    pub org_id: String,
    pub connector_id: String,
    pub as_issuer: String,
    pub token_endpoint: String,
    pub resource: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// cloud-api's answer to `POST /id-jag`. `Debug` never prints the assertion or secret.
#[derive(Clone, Deserialize)]
pub struct IdJag {
    pub id_jag: String,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

impl std::fmt::Debug for IdJag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdJag")
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("has_secret", &self.client_secret.is_some())
            .finish()
    }
}

/// A refusal the UI can show as is: `code` for logic, `message` in plain words.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedError {
    pub code: String,
    pub message: String,
}

impl ManagedError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }

    /// From cloud-api's `{ "error": { "code", "message" } }`. Known codes get
    /// Allternit's own wording; an unknown one keeps the cloud's message.
    pub fn from_cloud(status: u16, body: &Value) -> Self {
        let code = body["error"]["code"].as_str().or_else(|| body["error"].as_str()).unwrap_or("").to_string();
        let message = match code.as_str() {
            "not_configured" => "Your organization hasn't turned on managed sign-in for this connector.".to_string(),
            "not_member" => "You're not a member of the organization that manages this connector.".to_string(),
            "role_not_allowed" => {
                "Your role in the organization isn't allowed to use managed sign-in for this connector. Ask an org admin to add your role.".to_string()
            }
            "email_unverified" => "Verify the email address on your Allternit account, then try again.".to_string(),
            "idjag_not_configured" => "Managed sign-in isn't available on Allternit right now. Try again later.".to_string(),
            _ => body["error"]["message"]
                .as_str()
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Allternit couldn't start managed sign-in (HTTP {status}).")),
        };
        let code = if code.is_empty() { format!("cloud_http_{status}") } else { code };
        Self { code, message }
    }

    /// The JSON a connect/refresh answers with. `fallback` offers the normal
    /// sign-in as a separate, explicit action — never taken automatically.
    pub fn to_json(&self, connector_id: &str, fallback: bool) -> Value {
        let mut v = json!({
            "error": "managed_auth_failed",
            "code": self.code,
            "message": self.message,
            "connector": connector_id,
            "auth_type": "managed",
        });
        if fallback {
            v["fallback"] = json!({ "via": "oauth2", "label": "Connect with sign-in instead" });
        }
        v
    }
}

// ─── cloud-api ───────────────────────────────────────────────────────────────

/// The two cloud-api calls, behind a seam so tests need no network.
#[async_trait]
pub trait Cloud: Send + Sync {
    /// The user this runtime's device credential belongs to.
    fn paired_owner(&self) -> Option<String>;
    async fn get(&self, path: &str) -> Result<(u16, Value), ManagedError>;
    async fn post(&self, path: &str, body: &Value) -> Result<(u16, Value), ManagedError>;
}

/// Production: the runtime device token, as `runtime-devices/me/*` calls use it.
pub struct DeviceCloud {
    secret: Arc<dyn RelaySecret>,
    base: String,
}

fn not_paired() -> ManagedError {
    ManagedError::new("not_paired", "This computer isn't signed in to Allternit, so managed sign-in can't run. Sign in, then try again.")
}

impl DeviceCloud {
    fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder, ManagedError> {
        let token = self.secret.device_token().ok_or_else(not_paired)?;
        let mut req = reqwest::Client::new()
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token)
            .timeout(CLOUD_TIMEOUT);
        if let Some(runtime_id) = self.secret.runtime_id() {
            req = req.header(crate::relay_auth::RUNTIME_ID_HEADER, runtime_id);
        }
        Ok(req)
    }

    async fn send(req: reqwest::RequestBuilder) -> Result<(u16, Value), ManagedError> {
        let res = req
            .send()
            .await
            .map_err(|_| ManagedError::new("cloud_unreachable", "Allternit couldn't reach its servers to start managed sign-in. Check your connection and try again."))?;
        let status = res.status().as_u16();
        Ok((status, res.json::<Value>().await.unwrap_or(Value::Null)))
    }
}

#[async_trait]
impl Cloud for DeviceCloud {
    fn paired_owner(&self) -> Option<String> {
        self.secret.paired_owner()
    }
    async fn get(&self, path: &str) -> Result<(u16, Value), ManagedError> {
        Self::send(self.request(reqwest::Method::GET, path)?.timeout(CONFIGS_TIMEOUT)).await
    }
    async fn post(&self, path: &str, body: &Value) -> Result<(u16, Value), ManagedError> {
        Self::send(self.request(reqwest::Method::POST, path)?.json(body)).await
    }
}

#[cfg(test)]
thread_local! {
    static TEST_CLOUD: std::cell::RefCell<Option<Arc<dyn Cloud>>> = const { std::cell::RefCell::new(None) };
}

/// Test seam: `cloud()` answers with `fake` on this thread until the guard drops.
#[cfg(test)]
pub(crate) fn set_test_cloud(fake: Arc<dyn Cloud>) -> impl Drop {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_CLOUD.with(|c| *c.borrow_mut() = None);
        }
    }
    TEST_CLOUD.with(|c| *c.borrow_mut() = Some(fake));
    Reset
}

pub fn cloud() -> Arc<dyn Cloud> {
    #[cfg(test)]
    if let Some(fake) = TEST_CLOUD.with(|c| c.borrow().clone()) {
        return fake;
    }
    static CLOUD: OnceLock<Arc<dyn Cloud>> = OnceLock::new();
    CLOUD
        .get_or_init(|| Arc::new(DeviceCloud { secret: crate::relay_auth::process_secret(), base: crate::phone_sync::cloud_base() }))
        .clone()
}

/// The device credential speaks for one user. Never mint for anyone else.
fn check_owner(cloud: &dyn Cloud, user_id: &str) -> Result<(), ManagedError> {
    match cloud.paired_owner() {
        None => Err(not_paired()),
        Some(owner) if owner == user_id => Ok(()),
        Some(_) => Err(ManagedError::new(
            "other_account",
            "This computer is signed in to Allternit as a different account, so managed sign-in can't run for you here.",
        )),
    }
}

/// Configs this user may use, cached for a minute per owner.
pub async fn configs(cloud: &dyn Cloud, user_id: &str) -> Result<Vec<ManagedConfig>, ManagedError> {
    check_owner(cloud, user_id)?;
    static CACHE: OnceLock<Mutex<Option<(String, Instant, Vec<ManagedConfig>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((owner, at, list)) = cache.lock().unwrap().as_ref() {
        if owner == user_id && at.elapsed() < CONFIG_TTL {
            return Ok(list.clone());
        }
    }
    let (status, body) = cloud.get(CONFIGS_PATH).await?;
    if status != 200 {
        return Err(ManagedError::from_cloud(status, &body));
    }
    let list: Vec<ManagedConfig> = body
        .get("configs")
        .and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| serde_json::from_value(c.clone()).ok()).collect())
        .unwrap_or_default();
    *cache.lock().unwrap() = Some((user_id.to_string(), Instant::now(), list.clone()));
    Ok(list)
}

/// The config for `connector_id` (first match, or the named org's).
pub fn pick_config<'a>(list: &'a [ManagedConfig], connector_id: &str, org_id: Option<&str>) -> Option<&'a ManagedConfig> {
    list.iter().find(|c| c.connector_id == connector_id && org_id.map_or(true, |o| c.org_id == o))
}

/// RFC 3986 syntax normalisation for comparing resource identifiers.
pub fn same_resource(a: &str, b: &str) -> bool {
    let norm = |s: &str| url::Url::parse(s).map(|u| u.to_string()).unwrap_or_else(|_| s.to_string()).trim_end_matches('/').to_string();
    norm(a) == norm(b)
}

/// Whether managed sign-in applies to this connector for this user. `None` when
/// it doesn't, or when that can't be known right now (unpaired, cloud down): the
/// caller then runs its normal flow, because nothing managed was attempted.
pub async fn applicable_config(cloud: &dyn Cloud, user_id: &str, connector_id: &str) -> Option<ManagedConfig> {
    applicable(cloud, user_id, |c| c.connector_id == connector_id).await
}

/// The managed config for an MCP server the user added by URL ("Custom MCP server").
pub async fn applicable_for_server(cloud: &dyn Cloud, user_id: &str, server_url: &str) -> Option<ManagedConfig> {
    applicable(cloud, user_id, |c| same_resource(&c.resource, server_url)).await
}

async fn applicable(cloud: &dyn Cloud, user_id: &str, matches: impl Fn(&ManagedConfig) -> bool) -> Option<ManagedConfig> {
    static LAST_FAILURE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let last_failure = LAST_FAILURE.get_or_init(|| Mutex::new(None));
    if cloud.paired_owner().as_deref() != Some(user_id) {
        return None;
    }
    if last_failure.lock().unwrap().is_some_and(|at| at.elapsed() < CONFIGS_FAILURE_BACKOFF) {
        return None;
    }
    match configs(cloud, user_id).await {
        Ok(list) => list.into_iter().find(|c| matches(c)),
        Err(e) => {
            warn!(code = %e.code, "managed auth: configs unavailable; normal connect flow");
            *last_failure.lock().unwrap() = Some(Instant::now());
            None
        }
    }
}

pub async fn request_id_jag(cloud: &dyn Cloud, user_id: &str, connector_id: &str, org_id: Option<&str>) -> Result<IdJag, ManagedError> {
    check_owner(cloud, user_id)?;
    let mut body = json!({ "connector_id": connector_id });
    if let Some(org) = org_id {
        body["org_id"] = json!(org);
    }
    let (status, resp) = cloud.post(IDJAG_PATH, &body).await?;
    if status != 200 {
        return Err(ManagedError::from_cloud(status, &resp));
    }
    serde_json::from_value::<IdJag>(resp)
        .ok()
        .filter(|j| !j.id_jag.is_empty() && !j.token_endpoint.is_empty() && !j.client_id.is_empty())
        .ok_or_else(|| ManagedError::new("cloud_bad_response", "Allternit's servers sent an incomplete managed sign-in answer. Try again later."))
}

// ─── RFC 7523 JWT-bearer grant at the vendor ─────────────────────────────────

fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "the app".into())
}

/// RFC 6749 §2.3.1: id and secret are form-urlencoded before Basic encoding.
fn form_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// ID-JAG → access token. HTTPS only (an http endpoint would get the assertion and
/// client secret in the clear), except for private hosts when explicitly allowed
/// for local test servers. The returned token set carries `obtained_at` and an
/// `expires_in`.
pub async fn jwt_bearer_grant(jag: &IdJag, scopes: &[String], allow_private: bool) -> Result<Value, ManagedError> {
    let host = host_of(&jag.token_endpoint);
    let refused = |why: String| ManagedError::new("vendor_refused", format!("{host} refused managed sign-in: {why}"));
    let parsed = url::Url::parse(&jag.token_endpoint).map_err(|_| ManagedError::new("vendor_endpoint_invalid", "The connector's token address isn't a valid URL. Ask an org admin to check the managed sign-in settings."))?;
    if parsed.scheme() != "https" && !allow_private {
        return Err(ManagedError::new(
            "vendor_endpoint_insecure",
            "The connector's token address doesn't use HTTPS, so Allternit won't send it a sign-in. Ask an org admin to check the managed sign-in settings.",
        ));
    }
    let client = crate::mcp_apps::guarded_client(&jag.token_endpoint, allow_private)
        .await
        .map_err(|e| ManagedError::new("vendor_endpoint_forbidden", format!("Allternit can't reach {host}: {}", e.message)))?;

    let mut form: Vec<(&str, String)> = vec![("grant_type", JWT_BEARER_GRANT.into()), ("assertion", jag.id_jag.clone())];
    if !scopes.is_empty() {
        form.push(("scope", scopes.join(" ")));
    }
    let mut req = client.post(&jag.token_endpoint).header("Accept", "application/json");
    match jag.client_secret.as_deref().filter(|s| !s.is_empty()) {
        Some(secret) => req = req.basic_auth(form_encode(&jag.client_id), Some(form_encode(secret))),
        None => form.push(("client_id", jag.client_id.clone())),
    }
    let res = req
        .form(&form)
        .send()
        .await
        .map_err(|_| ManagedError::new("vendor_unreachable", format!("Allternit couldn't reach {host} to finish managed sign-in. Try again later.")))?;
    let status = res.status().as_u16();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        // Only the OAuth error fields: never echo the body (a misbehaving server
        // may reflect the assertion or secret).
        let code = body["error"].as_str().unwrap_or("error");
        let secret = jag.client_secret.as_deref().unwrap_or("\u{0}");
        let desc = body["error_description"]
            .as_str()
            .filter(|d| !d.contains(&jag.id_jag) && !d.contains(secret))
            .map(|d| format!(" ({})", d.chars().take(160).collect::<String>()))
            .unwrap_or_default();
        return Err(refused(format!("HTTP {status}, {code}{desc}")));
    }
    let access = body["access_token"].as_str().unwrap_or("");
    if access.is_empty() {
        return Err(refused("no access token in its answer".into()));
    }
    let mut tokens = body.clone();
    if tokens.get("expires_in").and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok())).is_none() {
        tokens["expires_in"] = json!(DEFAULT_EXPIRES_IN);
    }
    tokens["obtained_at"] = json!(chrono::Utc::now().timestamp());
    Ok(tokens)
}

/// A fresh token set for `cfg`: new ID-JAG, then the grant.
pub async fn grant(cloud: &dyn Cloud, user_id: &str, cfg: &ManagedConfig, allow_private: bool) -> Result<Value, ManagedError> {
    let jag = request_id_jag(cloud, user_id, &cfg.connector_id, Some(&cfg.org_id)).await?;
    let tokens = jwt_bearer_grant(&jag, &cfg.scopes, allow_private).await?;
    info!(connector_id = %cfg.connector_id, org_id = %cfg.org_id, "managed auth: access token granted");
    Ok(tokens)
}

// ─── storage ─────────────────────────────────────────────────────────────────

/// Session metadata of a managed MCP session; what a re-grant needs.
pub fn session_metadata(user_id: &str, cfg: &ManagedConfig) -> Value {
    json!({
        "tokenEndpoint": cfg.token_endpoint,
        "resource": cfg.resource,
        "managed": {
            "userId": user_id,
            "connectorId": cfg.connector_id,
            "orgId": cfg.org_id,
            "asIssuer": cfg.as_issuer,
            "scopes": cfg.scopes,
        },
    })
}

/// The config a managed session was granted under, from its metadata.
pub fn config_from_session(metadata: &Value) -> Option<(String, ManagedConfig)> {
    let m = metadata.get("managed")?;
    let s = |k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Some((
        s("userId")?,
        ManagedConfig {
            org_id: s("orgId")?,
            connector_id: s("connectorId")?,
            as_issuer: s("asIssuer").unwrap_or_default(),
            token_endpoint: metadata.get("tokenEndpoint").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            resource: metadata.get("resource").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            scopes: m.get("scopes").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default(),
        },
    ))
}

fn expires_at_rfc3339(tokens: &Value) -> Option<String> {
    let at = crate::mcp_apps::token_expiry(tokens, None)?;
    chrono::DateTime::from_timestamp(at, 0).map(|t| t.to_rfc3339())
}

/// Write a granted token set: the `connector_connections` row (the default account
/// of this connector, keyed `managed:<org>`), and the MCP connector + session for
/// the vendor's MCP server (`cfg.resource`). Returns the `mcp_connectors` id.
pub fn store(conn: &rusqlite::Connection, user_id: &str, display_name: &str, cfg: &ManagedConfig, tokens: &Value) -> rusqlite::Result<String> {
    store_for(conn, user_id, None, display_name, cfg, tokens)
}

/// [`store`] onto a known `mcp_connectors` row (a server the user added by URL).
pub fn store_for(conn: &rusqlite::Connection, user_id: &str, mcp_connector_id: Option<&str>, display_name: &str, cfg: &ManagedConfig, tokens: &Value) -> rusqlite::Result<String> {
    let existing = match mcp_connector_id {
        Some(id) => conn.query_row("SELECT id FROM mcp_connectors WHERE id = ?1 AND user_id = ?2", params![id, user_id], |r| r.get(0)),
        None => conn.query_row(
            "SELECT id FROM mcp_connectors WHERE user_id = ?1 AND url = ?2 ORDER BY created_at ASC LIMIT 1",
            params![user_id, cfg.resource],
            |r| r.get(0),
        ),
    };
    let mcp_id: String = match existing {
        Ok(id) => {
            conn.execute("UPDATE mcp_connectors SET enabled = 1, updated_at = CURRENT_TIMESTAMP WHERE id = ?1", params![id])?;
            id
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO mcp_connectors (id, user_id, name, name_id, url, type, enabled) VALUES (?1, ?2, ?3, ?4, ?5, 'http', 1)",
                params![id, user_id, display_name, crate::mcp_apps::normalize_name(&cfg.connector_id), cfg.resource],
            )?;
            id
        }
        Err(e) => return Err(e),
    };
    let sealed_tokens = crate::token_crypto::seal(&tokens.to_string());
    let metadata = session_metadata(user_id, cfg).to_string();
    conn.execute(
        "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, tokens, metadata, is_authenticated)
         VALUES (?1, ?2, ?3, ?4, ?5, 1)
         ON CONFLICT(state) DO UPDATE SET tokens = excluded.tokens, metadata = excluded.metadata,
           is_authenticated = 1, updated_at = CURRENT_TIMESTAMP",
        params![uuid::Uuid::new_v4().to_string(), mcp_id, format!("{SESSION_STATE_PREFIX}{mcp_id}"), sealed_tokens, metadata],
    )?;

    let access = crate::token_crypto::seal(tokens["access_token"].as_str().unwrap_or(""));
    let refresh = tokens["refresh_token"].as_str().filter(|t| !t.is_empty()).map(crate::token_crypto::seal);
    let row_meta = json!({ "managed": { "org_id": cfg.org_id, "resource": cfg.resource, "mcp_connector_id": mcp_id } }).to_string();
    let account_key = format!("managed:{}", cfg.org_id.to_lowercase());
    conn.execute(
        "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, access_token, refresh_token, expires_at, scopes, metadata, account_key, is_default)
         VALUES (?1, ?2, ?3, 'managed', 'connected', ?4, ?5, ?6, ?7, ?8, ?9, 0)
         ON CONFLICT(connector_id, user_id, account_key) DO UPDATE SET
           auth_type='managed', status='connected', access_token=excluded.access_token, refresh_token=excluded.refresh_token,
           expires_at=excluded.expires_at, scopes=excluded.scopes, metadata=excluded.metadata, updated_at=CURRENT_TIMESTAMP",
        params![
            uuid::Uuid::new_v4().to_string(),
            cfg.connector_id,
            user_id,
            access,
            refresh,
            expires_at_rfc3339(tokens),
            cfg.scopes.join(" "),
            row_meta,
            account_key
        ],
    )?;
    // Managed wins: it is the account reads that name none use.
    conn.execute(
        "UPDATE connector_connections SET is_default = CASE WHEN account_key = ?3 THEN 1 ELSE 0 END WHERE connector_id = ?1 AND user_id = ?2",
        params![cfg.connector_id, user_id, account_key],
    )?;
    Ok(mcp_id)
}

/// Remove what [`store`] wrote for this connector (its managed rows only).
pub fn forget(conn: &rusqlite::Connection, user_id: &str, connector_id: &str) -> rusqlite::Result<()> {
    let mcp_ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT json_extract(metadata, '$.managed.mcp_connector_id') FROM connector_connections
             WHERE connector_id = ?1 AND user_id = ?2 AND auth_type = 'managed'",
        )?;
        let ids = stmt.query_map(params![connector_id, user_id], |r| r.get::<_, Option<String>>(0))?.filter_map(|r| r.ok().flatten()).collect();
        ids
    };
    for id in mcp_ids {
        conn.execute("DELETE FROM mcp_oauth_sessions WHERE mcp_connector_id = ?1 AND state = ?2", params![id, format!("{SESSION_STATE_PREFIX}{id}")])?;
        // The MCP connector goes too unless another (non-managed) session still signs it in.
        conn.execute(
            "DELETE FROM mcp_connectors WHERE id = ?1 AND user_id = ?2
               AND NOT EXISTS (SELECT 1 FROM mcp_oauth_sessions s WHERE s.mcp_connector_id = ?1)",
            params![id, user_id],
        )?;
    }
    Ok(())
}

/// Re-grant for a stored managed config and write both copies. The new token set.
pub async fn regrant(
    state: &Arc<crate::AppState>,
    cloud: &dyn Cloud,
    user_id: &str,
    mcp_connector_id: Option<&str>,
    display_name: &str,
    cfg: &ManagedConfig,
    allow_private: bool,
) -> Result<Value, ManagedError> {
    let tokens = grant(cloud, user_id, cfg, allow_private).await?;
    let (db, uid, mcp_id, name, cfg2, t) =
        (state.db.clone(), user_id.to_string(), mcp_connector_id.map(str::to_string), display_name.to_string(), cfg.clone(), tokens.clone());
    let saved = tokio::task::spawn_blocking(move || db.connect().and_then(|c| store_for(&c, &uid, mcp_id.as_deref(), &name, &cfg2, &t))).await;
    if !matches!(saved, Ok(Ok(_))) {
        warn!(connector_id = %cfg.connector_id, "managed auth: re-granted token could not be stored");
    }
    Ok(tokens)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::{routing::post, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) struct FakeCloud {
        pub owner: Option<String>,
        pub configs: Value,
        pub idjag: (u16, Value),
        pub idjag_bodies: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl Cloud for FakeCloud {
        fn paired_owner(&self) -> Option<String> {
            self.owner.clone()
        }
        async fn get(&self, _path: &str) -> Result<(u16, Value), ManagedError> {
            Ok((200, self.configs.clone()))
        }
        async fn post(&self, _path: &str, body: &Value) -> Result<(u16, Value), ManagedError> {
            self.idjag_bodies.lock().unwrap().push(body.clone());
            Ok(self.idjag.clone())
        }
    }

    #[derive(Clone, Default)]
    struct AsLog {
        forms: Arc<Mutex<Vec<(Option<String>, String)>>>,
        calls: Arc<AtomicUsize>,
    }

    /// A vendor AS token endpoint that accepts assertion `jag-ok` from client `c1:s1`.
    async fn vendor_as(answer: Option<Value>) -> (String, AsLog) {
        let log = AsLog::default();
        let l = log.clone();
        let app = Router::new().route(
            "/token",
            post(move |headers: axum::http::HeaderMap, body: String| {
                let l = l.clone();
                let answer = answer.clone();
                async move {
                    l.calls.fetch_add(1, Ordering::SeqCst);
                    let auth = headers.get("authorization").and_then(|h| h.to_str().ok()).map(str::to_string);
                    l.forms.lock().unwrap().push((auth.clone(), body.clone()));
                    let basic = format!("Basic {}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "c1:s%2B1"));
                    let ok = body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer") && body.contains("assertion=jag-ok") && auth.as_deref() == Some(basic.as_str());
                    if !ok {
                        return (axum::http::StatusCode::BAD_REQUEST, axum::Json(json!({ "error": "invalid_grant", "error_description": "echo jag-bad s+1" }))).into_response();
                    }
                    axum::Json(answer.unwrap_or_else(|| json!({ "access_token": "at-1", "token_type": "Bearer" }))).into_response()
                }
            }),
        );
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/token"), log)
    }

    fn jag(endpoint: &str, assertion: &str) -> IdJag {
        IdJag { id_jag: assertion.into(), token_endpoint: endpoint.into(), client_id: "c1".into(), client_secret: Some("s+1".into()), expires_in: Some(300) }
    }

    #[tokio::test]
    async fn the_grant_sends_the_assertion_with_basic_client_auth_and_fills_expiry() {
        let (url, log) = vendor_as(None).await;
        let tokens = jwt_bearer_grant(&jag(&url, "jag-ok"), &["read".into(), "write".into()], true).await.unwrap();
        assert_eq!(tokens["access_token"], "at-1");
        assert_eq!(tokens["expires_in"], DEFAULT_EXPIRES_IN, "no expires_in still renews on a schedule");
        assert!(tokens["obtained_at"].as_i64().unwrap() > 0);
        let (_, form) = log.forms.lock().unwrap()[0].clone();
        assert!(form.contains("scope=read+write"), "{form}");
        assert!(!form.contains("client_secret"), "secret goes in the Basic header, not the body: {form}");
    }

    #[tokio::test]
    async fn a_refusal_is_plain_and_never_echoes_the_answer_body() {
        let (url, _) = vendor_as(None).await;
        let err = jwt_bearer_grant(&jag(&url, "jag-bad"), &[], true).await.unwrap_err();
        assert_eq!(err.code, "vendor_refused");
        assert!(err.message.contains("invalid_grant") && err.message.contains("127.0.0.1"), "{}", err.message);
        // The vendor's error_description echoed the assertion and secret: it is dropped.
        assert!(!err.message.contains("jag-bad") && !err.message.contains("s+1"), "{}", err.message);
    }

    #[tokio::test]
    async fn plain_http_token_endpoints_are_refused_before_anything_is_sent() {
        let (url, log) = vendor_as(None).await;
        let err = jwt_bearer_grant(&jag(&url, "jag-ok"), &[], false).await.unwrap_err();
        assert_eq!(err.code, "vendor_endpoint_insecure");
        assert_eq!(log.calls.load(Ordering::SeqCst), 0, "the assertion never left");
    }

    #[test]
    fn cloud_refusals_map_to_plain_words() {
        for (code, needle) in [
            ("not_configured", "hasn't turned on"),
            ("not_member", "not a member"),
            ("role_not_allowed", "role"),
            ("email_unverified", "Verify the email"),
            ("idjag_not_configured", "isn't available"),
        ] {
            let e = ManagedError::from_cloud(403, &json!({ "error": { "code": code, "message": "internal detail" } }));
            assert_eq!(e.code, code);
            assert!(e.message.contains(needle), "{code}: {}", e.message);
        }
        let unknown = ManagedError::from_cloud(500, &json!({}));
        assert_eq!(unknown.code, "cloud_http_500");
        let j = unknown.to_json("linear", true);
        assert_eq!(j["fallback"]["label"], "Connect with sign-in instead");
        assert!(ManagedError::from_cloud(500, &json!({})).to_json("x", false).get("fallback").is_none());
    }

    fn cfg() -> ManagedConfig {
        ManagedConfig {
            org_id: "org_A".into(),
            connector_id: "linear".into(),
            as_issuer: "https://as.example".into(),
            token_endpoint: "https://as.example/token".into(),
            resource: "https://mcp.example/mcp".into(),
            scopes: vec!["read".into()],
        }
    }

    #[tokio::test]
    async fn the_device_credential_only_ever_speaks_for_its_owner() {
        let fake = |owner: Option<&str>| FakeCloud {
            owner: owner.map(str::to_string),
            configs: json!({ "configs": [cfg()] }),
            idjag: (200, json!({ "id_jag": "j", "token_endpoint": "https://as.example/token", "client_id": "c" })),
            idjag_bodies: Mutex::new(Vec::new()),
        };
        let other = fake(Some("user-b"));
        assert_eq!(request_id_jag(&other, "user-a", "linear", None).await.unwrap_err().code, "other_account");
        assert!(other.idjag_bodies.lock().unwrap().is_empty(), "no ID-JAG asked for someone else");
        assert_eq!(request_id_jag(&fake(None), "user-a", "linear", None).await.unwrap_err().code, "not_paired");
        assert!(applicable_config(&other, "user-a", "linear").await.is_none());
        let mine = fake(Some("user-a"));
        assert_eq!(request_id_jag(&mine, "user-a", "linear", Some("org_A")).await.unwrap().client_id, "c");
        assert_eq!(mine.idjag_bodies.lock().unwrap()[0], json!({ "connector_id": "linear", "org_id": "org_A" }));
    }

    #[tokio::test]
    async fn store_writes_both_copies_sealed_makes_managed_the_default_and_forget_undoes_it() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::beta_session_routes::tests::test_app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        // An existing sign-in account for the same connector.
        conn.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account_key, is_default) VALUES ('old', 'linear', 'user-a', 'oauth2', 'connected', 'me@x', 1)",
            [],
        )
        .unwrap();
        let tokens = json!({ "access_token": "at-secret", "refresh_token": "rt-secret", "expires_in": 600, "obtained_at": 1_000 });
        let mcp_id = store(&conn, "user-a", "Linear", &cfg(), &tokens).unwrap();
        // A second grant lands on the same rows.
        assert_eq!(store(&conn, "user-a", "Linear", &cfg(), &tokens).unwrap(), mcp_id);

        let (auth, access, default_key): (String, String, String) = conn
            .query_row("SELECT auth_type, access_token, account_key FROM connector_connections WHERE connector_id='linear' AND user_id='user-a' AND is_default=1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((auth.as_str(), default_key.as_str()), ("managed", "managed:org_a"));
        assert!(!access.contains("at-secret") && crate::token_crypto::open(&access) == "at-secret");
        let (url, sessions): (String, i64) = conn
            .query_row("SELECT c.url, (SELECT COUNT(*) FROM mcp_oauth_sessions s WHERE s.mcp_connector_id = c.id) FROM mcp_connectors c WHERE c.id = ?1", params![mcp_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((url.as_str(), sessions), ("https://mcp.example/mcp", 1));
        let (stored, meta): (String, String) = conn.query_row("SELECT tokens, metadata FROM mcp_oauth_sessions WHERE mcp_connector_id = ?1", params![mcp_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert!(!stored.contains("rt-secret"));
        let (uid, back) = config_from_session(&serde_json::from_str(&meta).unwrap()).unwrap();
        assert_eq!((uid.as_str(), back), ("user-a", cfg()));

        forget(&conn, "user-a", "linear").unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM mcp_connectors WHERE id = ?1", params![mcp_id], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }
}
