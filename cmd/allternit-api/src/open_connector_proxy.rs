//! Proxy client for the vendored open-connector sidecar (`services/open-connector`).
//!
//! The sidecar is a self-hosted OAuth connector gateway (Apache-2.0, see
//! `services/open-connector/PROVENANCE.md`) bound to 127.0.0.1 only — it is never
//! reachable externally. All browser traffic reaches it through the Rust API:
//! this module is the only place that talks to it.
//!
//! Per-user isolation is enforced HERE, not by the sidecar (which is a local
//! single-user server): every call passes the Allternit `user_id` as
//! `connectionName` / `x-oo-connector-alias`, so one user's credentials are
//! invisible to every other user.
//!
//! Auth toward the sidecar mirrors its own middleware
//! (`services/open-connector/src/server/api/auth.ts`):
//! - `/api/*` routes require the admin bearer token.
//! - `/v1/*` routes require the runtime bearer token.
//! - `/health` and `/oauth/callback` are public.
//!
//! Tokens are shared between this process and the sidecar at launch time by
//! `scripts/dev-stack-watch.cjs` (operator-provided env, or ephemeral per dev
//! session). Env reads follow the config.rs convention
//! (`std::env::var(...).ok().filter(|s| !s.is_empty())`). The sidecar is the
//! token vault for every connector it handles — nothing in this module ever
//! persists a credential in Rust's own database.

use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const ALIASES_JSON: &str = include_str!("../assets/connector_id_aliases.json");

/// Hand-curated Allternit-catalog-id -> open-connector-provider-id mappings.
/// Loaded once from `assets/connector_id_aliases.json`; missing entries mean
/// the catalog id is served honestly as `connectable:false`.
fn aliases() -> &'static HashMap<String, String> {
    static A: OnceLock<HashMap<String, String>> = OnceLock::new();
    A.get_or_init(|| {
        let raw: Value = serde_json::from_str(ALIASES_JSON).unwrap_or_else(|_| json!({}));
        raw.as_object()
            .map(|o| {
                o.iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Reverse lookup: sidecar provider id -> Allternit catalog id. Used when the
/// sidecar reports live connections under its own spelling.
/// When a sidecar id maps to multiple Allternit ids (e.g. "monday" serves both
/// legacy "monday" and aliased "mondaymcp"), prefer the exact-spelling match.
fn reverse_aliases() -> &'static HashMap<String, String> {
    static R: OnceLock<HashMap<String, String>> = OnceLock::new();
    R.get_or_init(|| {
        let mut map: HashMap<String, String> = HashMap::new();
        for (allternit_id, sidecar_id) in aliases().iter() {
            // First pass: only exact matches, so they can't be overwritten.
            if allternit_id == sidecar_id {
                map.insert(sidecar_id.clone(), allternit_id.clone());
            }
        }
        for (allternit_id, sidecar_id) in aliases().iter() {
            // Second pass: fill in aliases for sidecar ids that don't already
            // have an exact-spelling Allternit id.
            map.entry(sidecar_id.clone()).or_insert_with(|| allternit_id.clone());
        }
        map
    })
}

/// Map an Allternit catalog connector id to the sidecar provider id that
/// actually implements the same service. Returns the original id when no alias
/// exists (the sidecar may still lack the provider, which is reported honestly).
pub fn sidecar_id(allternit_id: &str) -> String {
    aliases()
        .get(allternit_id)
        .cloned()
        .unwrap_or_else(|| allternit_id.to_string())
}

/// Map a sidecar provider id back to an Allternit catalog id when the sidecar
/// reports a live connection. Returns `None` only for unmapped sidecar-only
/// providers; callers should fall back to the sidecar id in that case.
pub fn allternit_id(sidecar_id: &str) -> Option<String> {
    reverse_aliases().get(sidecar_id).cloned()
}

/// Fallback env file written by `dev/scripts/start-connector-sidecar.sh`.
/// When the sidecar tokens are not present in the process environment, the
/// proxy reads them from this file so `allternit-api` can talk to a sidecar
/// that was started independently (e.g. by the desktop main process or the
/// dev stack script). The path itself can be overridden via env var.
fn env_file_path() -> String {
    std::env::var("ALLTERNIT_CONNECTOR_SIDECAR_ENV_FILE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/tmp/allternit-connector-sidecar.env".to_string())
}

fn env_file_values() -> &'static HashMap<String, String> {
    static CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let path = env_file_path();
        let contents = std::fs::read_to_string(&path).unwrap_or_default();
        let mut map = HashMap::new();
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.to_string(), v.to_string());
            }
        }
        map
    })
}

fn env_or_file(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env_file_values().get(key).cloned())
}

/// Base URL of the sidecar. `dev-stack-watch.cjs` always sets this; the
/// fallback matches its default port.
pub(crate) fn sidecar_url() -> String {
    env_or_file("ALLTERNIT_CONNECTOR_SIDECAR_URL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8014".to_string())
}

fn admin_token() -> Option<String> {
    env_or_file("ALLTERNIT_CONNECTOR_SIDECAR_ADMIN_TOKEN")
        .filter(|s| !s.is_empty())
}

fn runtime_token() -> Option<String> {
    env_or_file("ALLTERNIT_CONNECTOR_SIDECAR_RUNTIME_TOKEN")
        .filter(|s| !s.is_empty())
}

fn client() -> &'static Client {
    static C: OnceLock<Client> = OnceLock::new();
    C.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Allternit-Connector-Proxy/1.0")
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

/// Failure talking to the sidecar. `unreachable` means the process is down or
/// still starting — callers should degrade gracefully (e.g. mark sidecar-backed
/// connectors `connectable:false`) rather than 500 the whole endpoint.
#[derive(Debug)]
pub struct ProxyError {
    pub status: u16,
    pub message: String,
    /// True when the sidecar could not be reached at all (connection refused,
    /// timeout, DNS). False when it answered with an error status/body.
    pub unreachable: bool,
}

impl ProxyError {
    fn down(err: impl std::fmt::Display) -> Self {
        ProxyError {
            status: 502,
            message: format!("open-connector sidecar unreachable: {}", err),
            unreachable: true,
        }
    }
    fn bad_status(status: u16, body: &str) -> Self {
        ProxyError {
            status,
            message: format!("open-connector sidecar returned {}: {}", status, body),
            unreachable: false,
        }
    }
    fn decode(err: impl std::fmt::Display) -> Self {
        ProxyError {
            status: 502,
            message: format!("open-connector sidecar response not JSON: {}", err),
            unreachable: false,
        }
    }
}

/// GET a JSON endpoint, passing the admin bearer token when configured.
async fn admin_get(path: &str) -> Result<Value, ProxyError> {
    let url = format!("{}{}", sidecar_url(), path);
    let mut req = client().get(&url);
    if let Some(tok) = admin_token() {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::bad_status(status, &text));
    }
    serde_json::from_str(&text).map_err(ProxyError::decode)
}

/// True if the sidecar answers `/health`. Cheap liveness probe for callers
/// that need to decide whether sidecar-backed connectors are usable right now.
pub async fn is_reachable() -> bool {
    let url = format!("{}/health", sidecar_url());
    client()
        .get(&url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Returns the list of services that have an OAuth client configured on the
/// sidecar (`GET /api/oauth/configs`). Used by the connectors setup-status
/// endpoint to tell the UI which first-party OAuth connectors are ready.
pub(crate) async fn configured_oauth_services() -> Result<Vec<String>, ProxyError> {
    let raw = admin_get("/api/oauth/configs").await?;
    let list = raw.as_array().cloned().unwrap_or_default();
    let mut services = Vec::new();
    for entry in list {
        let configured = entry
            .get("configured")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !configured {
            continue;
        }
        if let Some(service) = entry.get("service").and_then(|v| v.as_str()) {
            services.push(service.to_string());
        }
    }
    Ok(services)
}

/// Slim per-provider view used by the connector catalog merge. Deliberately
/// tiny: the full catalog is ~39MB across 1,000+ providers, so the hot path
/// extracts only what `connector_routes::merge` needs and caches that.
#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub auth_types: Vec<String>,
    /// `execution.locallyExecutableActionCount` — actions this build can run.
    pub executable_actions: i64,
    pub display_name: String,
    /// `homepageUrl` from the provider definition — present on all 1,063
    /// vendored providers (`iconUrl` is defined on the type but never
    /// actually populated by any provider, so it's not worth carrying here).
    /// The frontend derives a real logo from this domain via a favicon
    /// service rather than Allternit hosting/curating per-connector art.
    pub homepage_url: Option<String>,
}

/// Sidecar `GET /api/providers`, reduced to a slim per-service map and cached
/// for 60s. The catalog only changes when the sidecar restarts. Keys are
/// **Allternit catalog ids** (after applying reverse aliases), so callers in
/// `connector_routes.rs` can look providers up with the original catalog id.
pub async fn provider_summaries() -> Result<Arc<HashMap<String, ProviderSummary>>, ProxyError> {
    static CACHE: OnceLock<
        tokio::sync::Mutex<Option<(Instant, Arc<HashMap<String, ProviderSummary>>)>>,
    > = OnceLock::new();
    let lock = CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    {
        let guard = lock.lock().await;
        if let Some((at, v)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return Ok(v.clone());
            }
        }
    }
    let raw = admin_get("/api/providers").await?;
    let mut map = HashMap::new();
    if let Some(arr) = raw.as_array() {
        for p in arr {
            let service = p.get("service").and_then(|v| v.as_str()).unwrap_or("");
            if service.is_empty() {
                continue;
            }
            let auth_types: Vec<String> = p
                .get("authTypes")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let executable_actions = p
                .get("execution")
                .and_then(|e| e.get("locallyExecutableActionCount"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let display_name = p
                .get("displayName")
                .and_then(|v| v.as_str())
                .unwrap_or(service)
                .to_string();
            let homepage_url = p
                .get("homepageUrl")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // Expose under the Allternit catalog spelling when one exists.
            // Also keep the sidecar's own spelling so an Allternit id that
            // happens to equal the sidecar service id is still reachable
            // (e.g. legacy "monday" alongside aliased "mondaymcp" -> "monday").
            let keys: std::collections::HashSet<String> = [
                allternit_id(service).unwrap_or_else(|| service.to_string()),
                service.to_string(),
            ]
            .into_iter()
            .collect();
            for key in keys {
                map.insert(
                    key,
                    ProviderSummary {
                        auth_types: auth_types.clone(),
                        executable_actions,
                        display_name: display_name.clone(),
                        homepage_url: homepage_url.clone(),
                    },
                );
            }
        }
    }
    let map = Arc::new(map);
    let mut guard = lock.lock().await;
    *guard = Some((Instant::now(), map.clone()));
    Ok(map)
}

/// Sidecar `GET /api/providers` (full catalog), cached briefly in-memory.
/// Heavy (~39MB parsed) — prefer `provider_summaries()` on hot paths; this is
/// for callers that genuinely need the raw catalog.
pub async fn list_providers() -> Result<Value, ProxyError> {
    static CACHE: OnceLock<tokio::sync::Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    let lock = CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    {
        let guard = lock.lock().await;
        if let Some((at, v)) = guard.as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return Ok(v.clone());
            }
        }
    }
    let fresh = admin_get("/api/providers").await?;
    let mut guard = lock.lock().await;
    *guard = Some((Instant::now(), fresh.clone()));
    Ok(fresh)
}

/// Sidecar `GET /api/providers/:service`, cached per service for 10 minutes.
/// `service` is the **Allternit catalog id**; it is resolved to the sidecar
/// provider id for the HTTP call, while the cache is keyed by the Allternit
/// id so callers never need to know the sidecar spelling.
pub async fn get_provider(service: &str) -> Result<Value, ProxyError> {
    static CACHE: OnceLock<tokio::sync::Mutex<HashMap<String, (Instant, Value)>>> = OnceLock::new();
    let lock = CACHE.get_or_init(Default::default);
    {
        let guard = lock.lock().await;
        if let Some((at, v)) = guard.get(service) {
            if at.elapsed() < Duration::from_secs(600) {
                return Ok(v.clone());
            }
        }
    }
    let sidecar_service = sidecar_id(service);
    let fresh = admin_get(&format!("/api/providers/{}", urlencoding(&sidecar_service))).await?;
    let mut guard = lock.lock().await;
    guard.insert(service.to_string(), (Instant::now(), fresh.clone()));
    Ok(fresh)
}

/// Drop all cached sidecar data (e.g. after the sidecar restarts).
pub async fn invalidate_caches() {
    static PROVIDERS: OnceLock<tokio::sync::Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    static PER_SERVICE: OnceLock<tokio::sync::Mutex<HashMap<String, (Instant, Value)>>> =
        OnceLock::new();
    if let Some(lock) = PROVIDERS.get() {
        *lock.lock().await = None;
    }
    if let Some(lock) = PER_SERVICE.get() {
        lock.lock().await.clear();
    }
}

/// This user's live sidecar connections: `GET /v1/apps` (runtime scope),
/// filtered client-side to `alias == user_id`. The sidecar is single-user and
/// returns every connection; the filter is what preserves per-user isolation.
/// Returns (Allternit catalog id, account_label) pairs, with sidecar service
/// ids reverse-mapped to Allternit catalog ids so `connector_routes.rs` can
/// match them against the catalog without leaking the sidecar spelling.
pub async fn list_user_connections(user_id: &str) -> Result<Vec<(String, String)>, ProxyError> {
    let url = format!("{}/v1/apps", sidecar_url());
    let mut req = client().get(&url);
    if let Some(tok) = runtime_token() {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::bad_status(status, &text));
    }
    let env: Value = serde_json::from_str(&text).map_err(ProxyError::decode)?;
    let out = env
        .get("data")
        .and_then(|d| d.as_array())
        .map(|apps| {
            apps.iter()
                .filter(|a| a.get("alias").and_then(|v| v.as_str()) == Some(user_id))
                .filter_map(|a| {
                    let service = a.get("service").and_then(|v| v.as_str()).unwrap_or("");
                    if service.is_empty() {
                        return None;
                    }
                    let label = a
                        .get("accountLabel")
                        .and_then(|v| v.as_str())
                        .or_else(|| a.get("displayName").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .to_string();
                    // Report the connection under the Allternit catalog spelling.
                    let key = allternit_id(service).unwrap_or_else(|| service.to_string());
                    Some((key, label))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(out)
}

/// Sidecar `POST /api/oauth/authorizations {service, connectionName: user_id}`.
/// `service` is the Allternit catalog id and is resolved to the sidecar
/// provider id internally. Returns the sidecar's authorization object (contains
/// `authorizationUrl`). No state is persisted in Rust by this call — the
/// sidecar tracks the pending authorization and completes it via
/// `proxy_oauth_callback`.
pub async fn start_oauth(service: &str, user_id: &str) -> Result<Value, ProxyError> {
    let sidecar_service = sidecar_id(service);
    let url = format!("{}/api/oauth/authorizations", sidecar_url());
    let mut req = client().post(&url).json(&json!({
        "service": sidecar_service,
        "connectionName": user_id,
    }));
    if let Some(tok) = admin_token() {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::bad_status(status, &text));
    }
    serde_json::from_str(&text).map_err(ProxyError::decode)
}

/// Sidecar `PUT /api/connections/:service` with `connectionName: user_id`.
/// `service` is the Allternit catalog id and is resolved to the sidecar
/// provider id internally. `body` is the caller-supplied credential payload
/// (e.g. `{authType: "api_key", values: {apiKey: ...}}` or
/// `{authType: "no_auth"}`) — the secret lands only in the sidecar's own
/// encrypted SQLite, never in Rust's DB.
pub async fn upsert_credential(
    service: &str,
    user_id: &str,
    body: Value,
) -> Result<Value, ProxyError> {
    let sidecar_service = sidecar_id(service);
    let mut payload = body;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("connectionName".to_string(), json!(user_id));
    }
    let url = format!(
        "{}/api/connections/{}",
        sidecar_url(),
        urlencoding(&sidecar_service)
    );
    let mut req = client().put(&url).json(&payload);
    if let Some(tok) = admin_token() {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::bad_status(status, &text));
    }
    serde_json::from_str(&text).map_err(ProxyError::decode)
}

/// Sidecar `DELETE /api/connections/:service?connectionName=<user_id>`
/// (connectionName as query param, per the sidecar's `readConnectionName`
/// resolution order in connect-server.ts). `service` is the Allternit catalog
/// id and is resolved to the sidecar provider id internally. Keeps this a
/// bodyless DELETE.
pub async fn disconnect(service: &str, user_id: &str) -> Result<Value, ProxyError> {
    let sidecar_service = sidecar_id(service);
    let url = format!(
        "{}/api/connections/{}?connectionName={}",
        sidecar_url(),
        urlencoding(&sidecar_service),
        urlencoding(user_id)
    );
    let mut req = client().delete(&url);
    if let Some(tok) = admin_token() {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::bad_status(status, &text));
    }
    serde_json::from_str(&text).map_err(ProxyError::decode)
}

/// Sidecar `POST /v1/actions/:actionId` with `x-oo-connector-alias: user_id`.
/// `action_id` is the sidecar's full action id (e.g. `gmail.list_messages`);
/// the caller maps a connector id + tool name to it. Runtime-scope route, so
/// this uses the runtime bearer token.
pub async fn execute(action_id: &str, user_id: &str, input: Value) -> Result<Value, ProxyError> {
    let url = format!("{}/v1/actions/{}", sidecar_url(), urlencoding(action_id));
    let mut req = client()
        .post(&url)
        .header("x-oo-connector-alias", user_id)
        .json(&json!({ "input": input }));
    if let Some(tok) = runtime_token() {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::bad_status(status, &text));
    }
    serde_json::from_str(&text).map_err(ProxyError::decode)
}

/// Sidecar `POST /mcp` — stateless JSON-RPC (no SSE/long-lived session; the
/// sidecar rejects `GET`/`DELETE` on this route by design, matching its own
/// docs). `body` is the caller's raw JSON-RPC request, passed through as-is.
/// `x-oo-connector-alias: user_id` on every call is what keeps `execute_action`
/// (one of the sidecar's built-in MCP tools, alongside `list_apps`,
/// `search_actions`, `get_action_guide`) resolving to the calling Allternit
/// user's own connections instead of some other user's — same per-user
/// isolation discipline as `execute()`, just over MCP instead of `/v1/actions`.
/// Runtime-scope route, so this uses the runtime bearer token.
pub async fn proxy_mcp(user_id: &str, body: Value) -> Result<Value, ProxyError> {
    let url = format!("{}/mcp", sidecar_url());
    // The sidecar's MCP transport (@modelcontextprotocol/sdk Streamable HTTP)
    // requires the client to accept both media types, even for the
    // stateless-JSON-response case actually used here — otherwise it answers
    // 406 regardless of what the original inbound request's Accept header was.
    let mut req = client()
        .post(&url)
        .header("x-oo-connector-alias", user_id)
        .header("accept", "application/json, text/event-stream")
        .json(&body);
    if let Some(tok) = runtime_token() {
        req = req.bearer_auth(tok);
    }
    let resp = req.send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    if !(200..300).contains(&status) {
        return Err(ProxyError::bad_status(status, &text));
    }
    serde_json::from_str(&text).map_err(ProxyError::decode)
}

/// Sidecar `GET /oauth/callback?<query>` — the OAuth provider's redirect
/// target. Pipes the sidecar's HTML completion page back verbatim (status +
/// body); this route is public on the sidecar, no token needed.
pub async fn proxy_oauth_callback(query: &str) -> Result<(u16, String), ProxyError> {
    let url = format!("{}/oauth/callback?{}", sidecar_url(), query);
    let resp = client().get(&url).send().await.map_err(ProxyError::down)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(ProxyError::down)?;
    Ok((status, text))
}

/// Percent-encode a path segment or query value (conservative: alphanumerics
/// + `-_.~`).
fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}
