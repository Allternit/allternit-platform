//! Vendor (browser-session) accounts on a cloud computer (migration V221).
//!
//! A vendor account's browser (Grok, ChatGPT, Claude, Gemini, Copilot: wire lane
//! `ui_bridge`) normally runs on the owner's Desktop, so a sleeping Mac takes the vendor
//! bots offline. An account can instead have a **host**: `this_device` (default) or a paired
//! `cloud_computer` runtime, whose subscription gateway keeps a persistent browser profile for
//! the account.
//!
//! * **Dispatch.** [`HostRoutedTransport`] wraps the runner's [`AaiTransport`]. A call for an
//!   account hosted on a cloud computer goes to that runtime's gateway over the cloud relay
//!   (`POST /api/v1/runtime-devices/:id/proxy`, which wakes a sleeping computer). Everything
//!   else, and every call made on the cloud computer itself, passes through untouched.
//! * **Sign-in.** The person signs in on the cloud computer themselves; we only open the
//!   vendor's login page in its browser and hand back the viewer. Passwords and cookies never
//!   pass through here. [`sign_in`] / [`check`] then health-check the session like Subscriptions.
//! * **Moving host** never copies a session: the account drops to `needs_sign_in` (and its
//!   connection state to EXPIRED, which cascades dependent bots to NEEDS_AUTH) until the person
//!   signs in on the new host and a check passes.
//! * **Inert without a cloud.** Nothing here runs for `this_device` accounts, and a cloud host
//!   needs `ALLTERNIT_CLOUD_API_URL` (defaults to api.allternit.com) plus the caller's own bearer
//!   or `ALLTERNIT_CLOUD_API_TOKEN`; without a token calls fail with `AUTH_REQUIRED`.
//!
//! Host state: `ready` | `needs_sign_in` | `signing_in`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_gateway_routes::{
    apply_account_state, audit, connection_path, get_account, now, rows, s, ApiErr,
};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::gateway_runner::{forward_error, AaiError, AaiTransport};
use crate::AppState;

pub const HOST_THIS_DEVICE: &str = "this_device";
pub const HOST_CLOUD: &str = "cloud_computer";
const READY: &str = "ready";
const NEEDS_SIGN_IN: &str = "needs_sign_in";
const SIGNING_IN: &str = "signing_in";

/// The gateway path (on the cloud computer) every vendor call is relayed to.
const AAI_PATH: &str = "/api/v1/subscriptions/gateway/aai/call";
const ACCOUNTS_PATH: &str = "/api/v1/subscriptions/gateway/v1/accounts";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/provider-accounts/:id/host", post(move_host_h))
        .route("/provider-accounts/:id/host/sign-in", post(sign_in_h))
        .route("/provider-accounts/:id/host/check", post(check_h))
        .route("/provider-accounts/:id/host/status", get(status_h))
}

// ---------------------------------------------------------------- cloud relay

/// Who the cloud call is made as: the caller's own bearer when there is one (a request from
/// the app), else `ALLTERNIT_CLOUD_API_TOKEN` (background turns).
#[derive(Clone, Debug, Default)]
pub struct Auth {
    pub bearer: Option<String>,
}

impl Auth {
    pub fn from_headers(h: &HeaderMap) -> Self {
        let bearer = h
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_string)
            .filter(|t| !t.is_empty());
        Auth { bearer }
    }
    fn token(&self) -> Option<String> {
        self.bearer.clone().or_else(|| std::env::var("ALLTERNIT_CLOUD_API_TOKEN").ok().filter(|t| !t.is_empty()))
    }
}

#[derive(Debug)]
pub enum RelayErr {
    /// No bearer to call the cloud with.
    NoAuth,
    /// The cloud did not answer, or answered something unreadable.
    Unreachable(String),
}

#[derive(Debug, Clone)]
pub struct RelayReply {
    pub status: u16,
    pub body: Value,
}

/// A runtime as the cloud lists it (`GET /api/v1/runtime-devices`).
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeInfo {
    pub id: String,
    pub name: String,
    pub runtime_type: String,
    pub status: String,
    pub last_seen_at: Option<String>,
    pub wakeable: bool,
    pub provisioned_instance_id: Option<String>,
}

/// The cloud API as the vendor-host code uses it. Production = [`HttpCloudRelay`]; tests fake it.
#[async_trait]
pub trait CloudRelay: Send + Sync {
    async fn runtime(&self, auth: &Auth, runtime_id: &str) -> Result<Option<RuntimeInfo>, RelayErr>;
    /// One request to the runtime through the relay. The cloud wakes a sleeping cloud computer
    /// first; a computer that cannot be reached answers 503 `runtime_warming` / `runtime_offline`.
    async fn proxy(&self, auth: &Auth, runtime_id: &str, method: &str, path: &str, body: Option<Value>) -> Result<RelayReply, RelayErr>;
}

pub struct HttpCloudRelay {
    base: String,
    http: reqwest::Client,
}

impl HttpCloudRelay {
    pub fn from_env() -> Self {
        let base = std::env::var("ALLTERNIT_CLOUD_API_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "https://api.allternit.com".into());
        // 30 s wake wait + the relay's own 90 s ceiling: never hang a turn longer than that.
        let http = reqwest::Client::builder().timeout(Duration::from_secs(125)).build().unwrap_or_default();
        HttpCloudRelay { base: base.trim_end_matches('/').to_string(), http }
    }
}

#[async_trait]
impl CloudRelay for HttpCloudRelay {
    async fn runtime(&self, auth: &Auth, runtime_id: &str) -> Result<Option<RuntimeInfo>, RelayErr> {
        let token = auth.token().ok_or(RelayErr::NoAuth)?;
        let resp = self
            .http
            .get(format!("{}/api/v1/runtime-devices", self.base))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| RelayErr::Unreachable(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED || resp.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(RelayErr::NoAuth);
        }
        if !resp.status().is_success() {
            return Err(RelayErr::Unreachable(format!("cloud answered {}", resp.status())));
        }
        let v: Value = resp.json().await.map_err(|e| RelayErr::Unreachable(e.to_string()))?;
        Ok(parse_runtimes(&v).into_iter().find(|r| r.id == runtime_id))
    }

    async fn proxy(&self, auth: &Auth, runtime_id: &str, method: &str, path: &str, body: Option<Value>) -> Result<RelayReply, RelayErr> {
        let token = auth.token().ok_or(RelayErr::NoAuth)?;
        let payload = json!({
            "method": method,
            "path": path,
            "headers": { "content-type": "application/json" },
            "body": body.map(|b| b.to_string()).unwrap_or_default(),
            "bodyEncoding": "utf8",
        });
        let resp = self
            .http
            .post(format!("{}/api/v1/runtime-devices/{}/proxy", self.base, urlencoding::encode(runtime_id)))
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| RelayErr::Unreachable(e.to_string()))?;
        let status = resp.status().as_u16();
        if status == 401 {
            return Err(RelayErr::NoAuth);
        }
        let bytes = resp.bytes().await.map_err(|e| RelayErr::Unreachable(e.to_string()))?;
        Ok(RelayReply { status, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) })
    }
}

pub(crate) fn parse_runtimes(v: &Value) -> Vec<RuntimeInfo> {
    let g = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).map(str::to_string);
    v.get("runtimes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(RuntimeInfo {
                        id: g(r, "id")?,
                        name: g(r, "name").unwrap_or_default(),
                        runtime_type: g(r, "runtimeType").unwrap_or_default(),
                        status: g(r, "status").unwrap_or_else(|| "offline".into()),
                        last_seen_at: g(r, "lastSeenAt"),
                        wakeable: r.get("wakeable").and_then(Value::as_bool).unwrap_or(false),
                        provisioned_instance_id: g(r, "provisionedInstanceId"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A cloud computer is a runtime the cloud can wake (provisioned or hosted).
fn is_cloud_computer(r: &RuntimeInfo) -> bool {
    r.wakeable || r.provisioned_instance_id.is_some()
}

/// True on a cloud computer itself (the image sets it): its own accounts always run locally.
fn on_cloud_computer() -> bool {
    matches!(std::env::var("ALLTERNIT_PROVISIONED").as_deref(), Ok("1") | Ok("true"))
}

// ---------------------------------------------------------------- host row

#[derive(Debug, Clone)]
pub struct HostRow {
    pub account_id: String,
    pub vendor: String,
    pub auth_type: String,
    pub state: String,
    pub kind: String,
    pub runtime_id: Option<String>,
    pub host_state: String,
    pub remote_account_id: Option<String>,
    pub last_seen_at: Option<String>,
}

pub fn load_host(db: &DbHandle, owner: &str, account_id: &str) -> Result<Option<HostRow>, rusqlite::Error> {
    let conn = db.connect()?;
    conn.query_row(
        "SELECT id, vendor, auth_type, state, host_kind, host_runtime_id, host_state, host_remote_account_id, host_last_seen_at
         FROM provider_account_bindings WHERE id = ?1 AND owner = ?2",
        params![account_id, owner],
        |r| {
            Ok(HostRow {
                account_id: r.get(0)?,
                vendor: r.get(1)?,
                auth_type: r.get(2)?,
                state: r.get(3)?,
                kind: r.get(4)?,
                runtime_id: r.get(5)?,
                host_state: r.get(6)?,
                remote_account_id: r.get(7)?,
                last_seen_at: r.get(8)?,
            })
        },
    )
    .optional()
}

impl HostRow {
    fn is_cloud(&self) -> bool {
        self.kind == HOST_CLOUD && self.runtime_id.is_some()
    }
    /// The id of this account inside the cloud computer's gateway.
    fn remote_id(&self) -> String {
        self.remote_account_id.clone().unwrap_or_else(|| self.account_id.clone())
    }
}

/// The gateway's provider id for an Agent Gateway vendor library id.
pub(crate) fn gateway_provider(vendor: &str) -> String {
    match vendor {
        "openai" | "chatgpt" => "chatgpt",
        "anthropic" | "claude" => "claude",
        "xai" | "grok" => "grok",
        "google" | "gemini" => "google",
        "microsoft" | "copilot" => "microsoft",
        other => other,
    }
    .to_string()
}

fn touch_seen(db: &DbHandle, owner: &str, account_id: &str) {
    if let Ok(conn) = db.connect() {
        let _ = conn.execute(
            "UPDATE provider_account_bindings SET host_last_seen_at = ?1 WHERE id = ?2 AND owner = ?3",
            params![now(), account_id, owner],
        );
    }
}

// ---------------------------------------------------------------- dispatch

/// Sends vendor calls for cloud-hosted accounts to their cloud computer; passes the rest to `inner`.
pub struct HostRoutedTransport {
    db: DbHandle,
    inner: Arc<dyn AaiTransport>,
    relay: Arc<dyn CloudRelay>,
}

impl HostRoutedTransport {
    pub fn new(db: DbHandle, inner: Arc<dyn AaiTransport>) -> Self {
        Self::with_relay(db, inner, Arc::new(HttpCloudRelay::from_env()))
    }
    pub fn with_relay(db: DbHandle, inner: Arc<dyn AaiTransport>, relay: Arc<dyn CloudRelay>) -> Self {
        Self { db, inner, relay }
    }
}

/// A relay failure as the runner understands it.
fn relay_error(e: RelayErr) -> AaiError {
    match e {
        RelayErr::NoAuth => AaiError::new("AUTH_REQUIRED", "sign in to Allternit again so your cloud computer can be reached"),
        RelayErr::Unreachable(d) => {
            let mut x = AaiError::new("GATEWAY_UNAVAILABLE", format!("the Allternit cloud could not be reached ({d})"));
            x.retryable = true;
            x
        }
    }
}

/// A non-2xx relay answer as the runner understands it.
fn relay_status_error(status: u16, body: &Value) -> AaiError {
    match body["error"].as_str() {
        Some("runtime_warming") => {
            let mut e = AaiError::new("GATEWAY_UNAVAILABLE", "your cloud computer is waking up; this will retry shortly");
            e.retryable = true;
            e.retry_after_ms = Some(body["retryAfterSeconds"].as_u64().unwrap_or(20) * 1000);
            e
        }
        Some("runtime_offline") => {
            let mut e = AaiError::new("GATEWAY_OFFLINE", "your cloud computer is offline");
            e.retryable = true;
            e
        }
        _ => forward_error(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), body),
    }
}

#[async_trait]
impl AaiTransport for HostRoutedTransport {
    async fn call(&self, owner: &str, op: &str, binding: &Value, input: Value) -> Result<Value, AaiError> {
        self.call_cred(owner, op, binding, None, input).await
    }
    async fn append_transcript(&self, session_id: &str, text: &str, metadata: Value) -> Result<(), String> {
        self.inner.append_transcript(session_id, text, metadata).await
    }
    async fn call_cred(&self, owner: &str, op: &str, binding: &Value, credential: Option<&Value>, input: Value) -> Result<Value, AaiError> {
        let host = match binding["accountBindingId"].as_str().filter(|_| !on_cloud_computer()) {
            Some(aid) => load_host(&self.db, owner, aid).map_err(|_| AaiError::new("INTERNAL", "could not read the provider account"))?,
            None => None,
        };
        let Some(host) = host.filter(HostRow::is_cloud) else {
            return self.inner.call_cred(owner, op, binding, credential, input).await;
        };
        // Never run a turn on a host the person has not signed in on: no session, and no reason to wake it.
        if host.host_state != READY {
            return Err(AaiError::new("AUTH_REQUIRED", "sign in to this account on your cloud computer first"));
        }
        let runtime_id = host.runtime_id.clone().unwrap_or_default();
        let mut body = json!({ "op": op, "binding": binding, "input": input });
        if let Some(c) = credential {
            body["credential"] = c.clone();
        }
        let reply = self.relay.proxy(&Auth::default(), &runtime_id, "POST", AAI_PATH, Some(body)).await.map_err(relay_error)?;
        if !(200..300).contains(&reply.status) {
            return Err(relay_status_error(reply.status, &reply.body));
        }
        touch_seen(&self.db, owner, &host.account_id);
        match reply.body["ok"].as_bool() {
            Some(true) => Ok(reply.body["value"].clone()),
            Some(false) => Err(AaiError {
                code: reply.body["error"]["code"].as_str().unwrap_or("UNKNOWN").to_string(),
                retryable: reply.body["error"]["retryable"].as_bool().unwrap_or(false),
                retry_after_ms: reply.body["error"]["retryAfterMs"].as_u64(),
                human_message: reply.body["error"]["humanMessage"].as_str().unwrap_or("the vendor gateway reported an error").to_string(),
            }),
            None => Err(AaiError::new("BAD_GATEWAY_REPLY", "the cloud computer's gateway sent an unreadable reply")),
        }
    }
}

// ---------------------------------------------------------------- status

/// The host block on `GET /provider-accounts/:id`.
pub async fn host_status(relay: &dyn CloudRelay, auth: &Auth, host: &HostRow) -> Value {
    let mut out = json!({
        "kind": host.kind,
        "runtimeId": host.runtime_id,
        "state": host.host_state,
        "needsSignIn": host.host_state != READY,
        "online": Value::Null,
        "lastSeen": host.last_seen_at,
        "wakeable": Value::Null,
    });
    if !host.is_cloud() {
        // The runtime answering this request is the host.
        out["online"] = json!(true);
        out["lastSeen"] = json!(now());
        return out;
    }
    match relay.runtime(auth, host.runtime_id.as_deref().unwrap_or_default()).await {
        Ok(Some(r)) => {
            out["online"] = json!(r.status == "online");
            out["wakeable"] = json!(is_cloud_computer(&r) && r.wakeable);
            out["name"] = json!(r.name);
            if let Some(seen) = r.last_seen_at {
                out["lastSeen"] = json!(seen);
            }
        }
        Ok(None) => {
            out["online"] = json!(false);
            out["error"] = json!("runtime_not_found");
        }
        Err(RelayErr::NoAuth) => out["error"] = json!("cloud_auth_required"),
        Err(RelayErr::Unreachable(_)) => out["error"] = json!("cloud_unreachable"),
    }
    out
}

pub async fn get_account_with_host(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Path(aid): Path<String>,
) -> Response {
    get_account_with_host_using(&state.db, &HttpCloudRelay::from_env(), &Auth::from_headers(&headers), &user.user_id, &aid).await
}

pub(crate) async fn get_account_with_host_using(db: &DbHandle, relay: &dyn CloudRelay, auth: &Auth, owner: &str, aid: &str) -> Response {
    let (db2, owner2, aid2) = (db.clone(), owner.to_string(), aid.to_string());
    let loaded = tokio::task::spawn_blocking(move || -> Result<(Value, Vec<Value>, HostRow), ApiErr> {
        let conn = db2.connect()?;
        let account = get_account(&conn, &owner2, &aid2)?;
        let deps = rows(&conn, "SELECT bot_id, state FROM bot_execution_bindings WHERE account_binding_id = ?1 AND owner = ?2", &[&aid2, &owner2])?;
        let host = load_host(&db2, &owner2, &aid2)?.ok_or_else(|| ApiErr::nf("account not found"))?;
        Ok((account, deps, host))
    })
    .await;
    match loaded {
        Ok(Ok((account, deps, host))) => {
            let status = host_status(relay, auth, &host).await;
            (StatusCode::OK, Json(json!({ "account": account, "dependentBots": deps, "host": status }))).into_response()
        }
        Ok(Err(e)) => e.response(),
        Err(_) => ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, "database error").response(),
    }
}

async fn status_h(state: State<Arc<AppState>>, user: Extension<AuthUser>, headers: HeaderMap, path: Path<String>) -> Response {
    get_account_with_host(state, user, headers, path).await
}

// ---------------------------------------------------------------- move host

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveBody {
    host: String,
    runtime_id: Option<String>,
}

/// Where an account's connection state goes when its session can no longer be trusted
/// because the browser moved: a connected account must be signed in again.
fn demoted(state: &str) -> Option<&'static str> {
    match state {
        "CONNECTED" | "DEGRADED" => Some("EXPIRED"),
        "AUTHENTICATING" | "VERIFYING" => Some("AUTH_FAILED"),
        _ => None,
    }
}

pub async fn move_host(db: &DbHandle, relay: &dyn CloudRelay, auth: &Auth, owner: &str, aid: &str, body: MoveBody) -> Result<Value, ApiErr> {
    let load = |db: &DbHandle| load_host(db, owner, aid).map_err(ApiErr::from)?.ok_or_else(|| ApiErr::nf("account not found"));
    let current = load(db)?;
    let unknown = || ApiErr::bad("host must be 'this_device' or 'cloud_computer'");
    let (kind, runtime) = match body.host.as_str() {
        HOST_THIS_DEVICE => {
            if body.runtime_id.is_some() {
                return Err(ApiErr::bad("runtimeId is only for host 'cloud_computer'"));
            }
            (HOST_THIS_DEVICE, None)
        }
        HOST_CLOUD => {
            let rid = body.runtime_id.clone().filter(|r| !r.trim().is_empty()).ok_or_else(|| ApiErr::bad("runtimeId is required for host 'cloud_computer'"))?;
            (HOST_CLOUD, Some(rid))
        }
        _ => return Err(unknown()),
    };
    if current.auth_type != "browser_session" {
        return Err(ApiErr::new(StatusCode::CONFLICT, "only browser-session accounts can run on a cloud computer"));
    }
    if current.kind == kind && current.runtime_id == runtime {
        return Ok(json!({ "moved": false, "host": host_status(relay, auth, &current).await }));
    }
    if let Some(rid) = &runtime {
        match relay.runtime(auth, rid).await {
            Ok(Some(r)) if is_cloud_computer(&r) => {}
            Ok(Some(_)) => return Err(ApiErr::new(StatusCode::CONFLICT, "that runtime is not a cloud computer")),
            Ok(None) => return Err(ApiErr::nf("cloud computer not found")),
            Err(RelayErr::NoAuth) => return Err(ApiErr::new(StatusCode::UNAUTHORIZED, "sign in to Allternit to pick a cloud computer")),
            Err(RelayErr::Unreachable(_)) => return Err(ApiErr::new(StatusCode::BAD_GATEWAY, "the Allternit cloud could not be reached")),
        }
    }
    // Sign the old cloud host out first (best effort): its saved browser session must not outlive the move.
    let mut cleanup = "none";
    if current.is_cloud() {
        let old = current.runtime_id.clone().unwrap_or_default();
        let path = format!("{ACCOUNTS_PATH}/{}", urlencoding::encode(&current.remote_id()));
        cleanup = match relay.proxy(auth, &old, "DELETE", &path, None).await {
            Ok(r) if (200..300).contains(&r.status) || r.status == 404 => "signed_out",
            _ => "skipped",
        };
    }
    let (db2, owner2, aid2, from_kind, from_rt, cleanup) =
        (db.clone(), owner.to_string(), aid.to_string(), current.kind.clone(), current.runtime_id.clone(), cleanup);
    let cur_state = current.state.clone();
    tokio::task::spawn_blocking(move || -> Result<(), ApiErr> {
        let conn = db2.connect()?;
        let t = now();
        conn.execute(
            "UPDATE provider_account_bindings SET host_kind = ?1, host_runtime_id = ?2, host_state = ?3, host_remote_account_id = NULL,
                host_last_seen_at = NULL, host_changed_at = ?4, updated_at = ?4 WHERE id = ?5 AND owner = ?6",
            params![kind, runtime, NEEDS_SIGN_IN, t, aid2, owner2],
        )?;
        if let Some(to) = demoted(&cur_state) {
            apply_account_state(&db2, &conn, &owner2, &aid2, &cur_state, to, false, json!({ "reason": "host moved: sign in again on the new host" }))?;
        }
        audit(
            &conn,
            &owner2,
            &aid2,
            "host.moved",
            Some(&from_kind),
            Some(kind),
            json!({ "fromRuntimeId": from_rt, "toRuntimeId": runtime, "oldHostSession": cleanup, "needsSignIn": true }),
        );
        Ok(())
    })
    .await
    .map_err(|_| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, "database error"))??;
    let after = load(db)?;
    Ok(json!({
        "moved": true,
        "needsSignIn": true,
        "oldHostSession": cleanup,
        "host": host_status(relay, auth, &after).await,
    }))
}

async fn move_host_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, headers: HeaderMap, Path(aid): Path<String>, Json(b): Json<MoveBody>) -> Response {
    match move_host(&state.db, &HttpCloudRelay::from_env(), &Auth::from_headers(&headers), &user.user_id, &aid, b).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => e.response(),
    }
}

// ---------------------------------------------------------------- sign-in + check

fn require_cloud(h: &HostRow) -> Result<(), ApiErr> {
    if h.is_cloud() {
        Ok(())
    } else {
        Err(ApiErr::new(StatusCode::CONFLICT, "this account runs on this device; move it to a cloud computer first"))
    }
}

/// Maps a relay failure on a user-driven call to an HTTP error.
fn http_relay_error(e: RelayErr) -> ApiErr {
    match e {
        RelayErr::NoAuth => ApiErr::new(StatusCode::UNAUTHORIZED, "sign in to Allternit so your cloud computer can be reached"),
        RelayErr::Unreachable(_) => ApiErr::new(StatusCode::BAD_GATEWAY, "the Allternit cloud could not be reached"),
    }
}

fn http_reply_error(r: &RelayReply) -> ApiErr {
    match r.body["error"].as_str() {
        Some("runtime_warming") => {
            ApiErr::new(StatusCode::SERVICE_UNAVAILABLE, format!("your cloud computer is waking up; try again in {}s", r.body["retryAfterSeconds"].as_u64().unwrap_or(20)))
        }
        Some("runtime_offline") => ApiErr::new(StatusCode::SERVICE_UNAVAILABLE, "your cloud computer is offline"),
        other => ApiErr::new(StatusCode::BAD_GATEWAY, format!("the cloud computer's gateway answered {}{}", r.status, other.map(|o| format!(": {o}")).unwrap_or_default())),
    }
}

fn set_host_state(db: &DbHandle, owner: &str, aid: &str, host_state: &str, remote_id: Option<&str>, seen: bool) -> Result<(), ApiErr> {
    let conn = db.connect()?;
    conn.execute(
        "UPDATE provider_account_bindings SET host_state = ?1, host_remote_account_id = COALESCE(?2, host_remote_account_id),
            host_last_seen_at = CASE WHEN ?3 THEN ?4 ELSE host_last_seen_at END, updated_at = ?4 WHERE id = ?5 AND owner = ?6",
        params![host_state, remote_id, seen, now(), aid, owner],
    )?;
    Ok(())
}

/// Hops the connection state toward `target` along legal transitions, auditing each.
fn advance_state(db: &DbHandle, owner: &str, aid: &str, target: &str) -> Result<(), ApiErr> {
    let conn = db.connect()?;
    let from = s(&get_account(&conn, owner, aid)?, "state");
    let Some(path) = connection_path(&from, target) else { return Ok(()) };
    let mut at = from;
    for to in path {
        apply_account_state(db, &conn, owner, aid, &at, to, false, json!({ "reason": "cloud computer sign-in" }))?;
        at = to.to_string();
    }
    Ok(())
}

/// Opens the vendor's login page in the cloud computer's browser. The person signs in there
/// themselves; this never sees a password or a cookie. Returns the login state and the
/// viewer descriptor of the cloud computer (its existing viewing session).
pub async fn sign_in(db: &DbHandle, relay: &dyn CloudRelay, auth: &Auth, owner: &str, aid: &str) -> Result<Value, ApiErr> {
    let host = load_host(db, owner, aid).map_err(ApiErr::from)?.ok_or_else(|| ApiErr::nf("account not found"))?;
    require_cloud(&host)?;
    let rid = host.runtime_id.clone().unwrap_or_default();
    let remote = host.remote_id();
    let status_path = format!("{ACCOUNTS_PATH}/{}/status", urlencoding::encode(&remote));
    let existing = relay.proxy(auth, &rid, "GET", &status_path, None).await.map_err(http_relay_error)?;
    let login = if existing.status == 404 {
        // First time on this host: create the gateway account and open its login window in one call.
        let body = json!({ "provider": gateway_provider(&host.vendor), "account_id": remote, "login": true });
        relay.proxy(auth, &rid, "POST", ACCOUNTS_PATH, Some(body)).await.map_err(http_relay_error)?
    } else if (200..300).contains(&existing.status) {
        let path = format!("{ACCOUNTS_PATH}/{}/login", urlencoding::encode(&remote));
        relay.proxy(auth, &rid, "POST", &path, None).await.map_err(http_relay_error)?
    } else {
        return Err(http_reply_error(&existing));
    };
    if !(200..300).contains(&login.status) {
        return Err(http_reply_error(&login));
    }
    let (db2, owner2, aid2, remote2) = (db.clone(), owner.to_string(), aid.to_string(), remote.clone());
    tokio::task::spawn_blocking(move || -> Result<(), ApiErr> {
        set_host_state(&db2, &owner2, &aid2, SIGNING_IN, Some(&remote2), true)?;
        advance_state(&db2, &owner2, &aid2, "AUTHENTICATING")
    })
    .await
    .map_err(|_| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, "database error"))??;
    let info = relay.runtime(auth, &rid).await.ok().flatten();
    Ok(json!({
        "state": "login_window_open",
        "login": login.body.get("login").cloned().unwrap_or_else(|| login.body.clone()),
        "viewer": {
            "kind": "cloud_computer",
            "runtimeId": rid,
            "provisionedInstanceId": info.as_ref().and_then(|r| r.provisioned_instance_id.clone()),
            "name": info.as_ref().map(|r| r.name.clone()),
            "hint": "Open this cloud computer's viewer and sign in to the vendor in its browser window. Then check the session.",
        },
        "needsSignIn": true,
    }))
}

/// Health-checks the account's session on the cloud computer, as Subscriptions does: a ready
/// session connects the account and clears needs_sign_in; a login still pending leaves it signing in.
pub async fn check(db: &DbHandle, relay: &dyn CloudRelay, auth: &Auth, owner: &str, aid: &str) -> Result<Value, ApiErr> {
    let host = load_host(db, owner, aid).map_err(ApiErr::from)?.ok_or_else(|| ApiErr::nf("account not found"))?;
    require_cloud(&host)?;
    let rid = host.runtime_id.clone().unwrap_or_default();
    let path = format!("{ACCOUNTS_PATH}/{}/status", urlencoding::encode(&host.remote_id()));
    let reply = relay.proxy(auth, &rid, "GET", &path, None).await.map_err(http_relay_error)?;
    let health = if reply.status == 404 {
        None // never signed in on this host
    } else if (200..300).contains(&reply.status) {
        Some(reply.body["session_health"].as_str().unwrap_or("unknown").to_string())
    } else {
        return Err(http_reply_error(&reply));
    };
    let target = health.as_deref().map(|h| crate::subscription_sync::health_target(Some(h)));
    let (db2, owner2, aid2, h2) = (db.clone(), owner.to_string(), aid.to_string(), health.clone());
    tokio::task::spawn_blocking(move || -> Result<(), ApiErr> {
        match h2.as_deref() {
            Some("ready") => {
                set_host_state(&db2, &owner2, &aid2, READY, None, true)?;
                // Connected only through the legal hops; apply_account_state also clears needs_sign_in.
                advance_state(&db2, &owner2, &aid2, "CONNECTED")?;
            }
            Some("auth_required") | None => {
                let st = load_host(&db2, &owner2, &aid2)?.map(|h| h.host_state).unwrap_or_default();
                // Keep `signing_in` while a login window is open; otherwise the person still owes a sign-in.
                set_host_state(&db2, &owner2, &aid2, if st == SIGNING_IN { SIGNING_IN } else { NEEDS_SIGN_IN }, None, h2.is_some())?;
            }
            Some(_) => {
                // A challenge, restriction or outage: not usable, and not fixed by signing in blindly.
                set_host_state(&db2, &owner2, &aid2, NEEDS_SIGN_IN, None, true)?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| ApiErr::new(StatusCode::INTERNAL_SERVER_ERROR, "database error"))??;
    let after = load_host(db, owner, aid).map_err(ApiErr::from)?.ok_or_else(|| ApiErr::nf("account not found"))?;
    Ok(json!({
        "sessionHealth": health,
        "connection": target,
        "host": host_status(relay, auth, &after).await,
    }))
}

async fn sign_in_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, headers: HeaderMap, Path(aid): Path<String>) -> Response {
    match sign_in(&state.db, &HttpCloudRelay::from_env(), &Auth::from_headers(&headers), &user.user_id, &aid).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => e.response(),
    }
}

async fn check_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, headers: HeaderMap, Path(aid): Path<String>) -> Response {
    match check(&state.db, &HttpCloudRelay::from_env(), &Auth::from_headers(&headers), &user.user_id, &aid).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => e.response(),
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    /// Fake cloud: a runtime list and scripted proxy answers per "METHOD path"; records every call.
    #[derive(Default)]
    struct FakeCloud {
        runtimes: Mutex<Vec<RuntimeInfo>>,
        replies: Mutex<HashMap<String, VecDeque<RelayReply>>>,
        calls: Mutex<Vec<(String, String, String, Option<Value>)>>,
    }

    impl FakeCloud {
        fn cloud_computer(&self, id: &str, status: &str) {
            self.runtimes.lock().unwrap().push(RuntimeInfo {
                id: id.into(),
                name: "Allternit cloud computer".into(),
                runtime_type: "provisioned".into(),
                status: status.into(),
                last_seen_at: Some("2026-10-03T00:00:00Z".into()),
                wakeable: true,
                provisioned_instance_id: Some(format!("pi_{id}")),
            });
        }
        fn desktop(&self, id: &str) {
            self.runtimes.lock().unwrap().push(RuntimeInfo {
                id: id.into(),
                name: "Mac".into(),
                runtime_type: "desktop".into(),
                status: "online".into(),
                last_seen_at: None,
                wakeable: false,
                provisioned_instance_id: None,
            });
        }
        fn reply(&self, key: &str, status: u16, body: Value) {
            self.replies.lock().unwrap().entry(key.into()).or_default().push_back(RelayReply { status, body });
        }
        fn calls_to(&self, method: &str, path: &str) -> usize {
            self.calls.lock().unwrap().iter().filter(|c| c.1 == method && c.2 == path).count()
        }
        fn total(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl CloudRelay for FakeCloud {
        async fn runtime(&self, _a: &Auth, id: &str) -> Result<Option<RuntimeInfo>, RelayErr> {
            Ok(self.runtimes.lock().unwrap().iter().find(|r| r.id == id).cloned())
        }
        async fn proxy(&self, _a: &Auth, rid: &str, method: &str, path: &str, body: Option<Value>) -> Result<RelayReply, RelayErr> {
            self.calls.lock().unwrap().push((rid.into(), method.into(), path.into(), body));
            let key = format!("{method} {path}");
            let mut m = self.replies.lock().unwrap();
            Ok(m.get_mut(&key).and_then(VecDeque::pop_front).unwrap_or(RelayReply { status: 200, body: json!({}) }))
        }
    }

    #[derive(Default)]
    struct Inner {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AaiTransport for Inner {
        async fn call(&self, _o: &str, op: &str, _b: &Value, _i: Value) -> Result<Value, AaiError> {
            self.calls.lock().unwrap().push(op.into());
            Ok(json!({ "via": "inner" }))
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-vh-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await
    }

    fn seed(st: &Arc<AppState>, id: &str, auth: &str, state: &str, host: Option<(&str, &str)>) {
        let conn = st.db.connect().unwrap();
        conn.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, state, host_kind, host_runtime_id, host_state, created_at, updated_at)
             VALUES (?1, 'user-a', 'openai', ?2, ?3, ?4, ?5, ?6, 't', 't')",
            params![id, auth, state, host.map_or(HOST_THIS_DEVICE, |_| HOST_CLOUD), host.map(|h| h.0), host.map_or(READY, |h| h.1)],
        )
        .unwrap();
    }

    fn seed_bot(st: &Arc<AppState>, account: &str) {
        let conn = st.db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES ('bot-1', 'user-a', 'b', 'm', 'p', 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO bot_execution_bindings (id, owner, bot_id, type, account_binding_id, state, created_at, updated_at) VALUES ('eb-1', 'user-a', 'bot-1', 'vendor', ?1, 'READY', 't', 't')",
            params![account],
        )
        .unwrap();
    }

    fn col(st: &Arc<AppState>, id: &str, c: &str) -> Option<String> {
        st.db.connect().unwrap().query_row(&format!("SELECT {c} FROM provider_account_bindings WHERE id = ?1"), params![id], |r| r.get(0)).unwrap()
    }

    fn binding(account: &str) -> Value {
        json!({ "id": "eb-1", "botId": "bot-1", "type": "vendor", "vendor": "openai", "accountBindingId": account, "state": "READY" })
    }

    fn mv(host: &str, rt: Option<&str>) -> MoveBody {
        MoveBody { host: host.into(), runtime_id: rt.map(str::to_string) }
    }

    // ---- dispatch routing by host

    #[tokio::test]
    async fn dispatch_follows_the_accounts_host() {
        let st = setup("dispatch").await;
        seed(&st, "acct-local", "browser_session", "CONNECTED", None);
        seed(&st, "acct-cloud", "browser_session", "CONNECTED", Some(("rt-1", READY)));
        let cloud = Arc::new(FakeCloud::default());
        cloud.cloud_computer("rt-1", "online");
        cloud.reply(&format!("POST {AAI_PATH}"), 200, json!({ "ok": true, "value": { "via": "cloud" } }));
        let inner = Arc::new(Inner::default());
        let tx = HostRoutedTransport::with_relay(st.db.clone(), inner.clone(), cloud.clone());

        let v = tx.call("user-a", "agent.list", &binding("acct-local"), json!({})).await.unwrap();
        assert_eq!(v["via"], "inner");
        assert_eq!(cloud.total(), 0, "a this_device account never touches the cloud");

        let v = tx.call("user-a", "agent.context.message", &binding("acct-cloud"), json!({ "text": "hi" })).await.unwrap();
        assert_eq!(v["via"], "cloud");
        assert_eq!(inner.calls.lock().unwrap().len(), 1, "the cloud account did not use the local gateway");
        let (rid, method, path, body) = cloud.calls.lock().unwrap()[0].clone();
        assert_eq!((rid.as_str(), method.as_str(), path.as_str()), ("rt-1", "POST", AAI_PATH));
        let body = body.unwrap();
        assert_eq!((body["op"].as_str(), body["input"]["text"].as_str()), (Some("agent.context.message"), Some("hi")));
        assert!(col(&st, "acct-cloud", "host_last_seen_at").is_some());

        // A binding with no account, or someone else's account, is not a cloud call.
        let v = tx.call("user-b", "agent.list", &binding("acct-cloud"), json!({})).await.unwrap();
        assert_eq!(v["via"], "inner");
        let v = tx.call("user-a", "agent.list", &json!({ "type": "vendor" }), json!({})).await.unwrap();
        assert_eq!(v["via"], "inner");
        assert_eq!(cloud.total(), 1);
    }

    #[tokio::test]
    async fn dispatch_refuses_an_unsigned_host_without_waking_it() {
        let st = setup("unsigned").await;
        seed(&st, "acct-cloud", "browser_session", "EXPIRED", Some(("rt-1", NEEDS_SIGN_IN)));
        let cloud = Arc::new(FakeCloud::default());
        let tx = HostRoutedTransport::with_relay(st.db.clone(), Arc::new(Inner::default()), cloud.clone());
        let e = tx.call("user-a", "agent.context.message", &binding("acct-cloud"), json!({})).await.unwrap_err();
        assert_eq!(e.code, "AUTH_REQUIRED");
        assert_eq!(cloud.total(), 0);
    }

    #[tokio::test]
    async fn dispatch_goes_through_the_relay_when_the_runtime_is_asleep_and_maps_wake_answers() {
        let st = setup("wake").await;
        seed(&st, "acct-cloud", "browser_session", "CONNECTED", Some(("rt-1", READY)));
        let cloud = Arc::new(FakeCloud::default());
        // Asleep: the relay is what wakes it, so we must still send the request.
        cloud.cloud_computer("rt-1", "offline");
        let key = format!("POST {AAI_PATH}");
        cloud.reply(&key, 503, json!({ "error": "runtime_warming", "retryAfterSeconds": 30 }));
        cloud.reply(&key, 503, json!({ "error": "runtime_offline" }));
        cloud.reply(&key, 200, json!({ "ok": false, "error": { "code": "RATE_LIMITED", "retryable": true, "retryAfterMs": 5000, "humanMessage": "slow down" } }));
        cloud.reply(&key, 200, json!({ "ok": true, "value": { "woke": true } }));
        let tx = HostRoutedTransport::with_relay(st.db.clone(), Arc::new(Inner::default()), cloud.clone());
        let b = binding("acct-cloud");

        let e = tx.call("user-a", "agent.events", &b, json!({})).await.unwrap_err();
        assert_eq!((e.code.as_str(), e.retryable, e.retry_after_ms), ("GATEWAY_UNAVAILABLE", true, Some(30_000)));
        let e = tx.call("user-a", "agent.events", &b, json!({})).await.unwrap_err();
        assert_eq!((e.code.as_str(), e.retryable), ("GATEWAY_OFFLINE", true));
        let e = tx.call("user-a", "agent.events", &b, json!({})).await.unwrap_err();
        assert_eq!((e.code.as_str(), e.retry_after_ms, e.human_message.as_str()), ("RATE_LIMITED", Some(5000), "slow down"));
        assert!(col(&st, "acct-cloud", "host_last_seen_at").is_some(), "a vendor-level failure still proves the host answered");
        let v = tx.call("user-a", "agent.events", &b, json!({})).await.unwrap();
        assert_eq!(v["woke"], true);
        assert_eq!(cloud.calls_to("POST", AAI_PATH), 4);
    }

    // ---- host move state machine

    #[tokio::test]
    async fn moving_to_a_cloud_computer_requires_a_new_sign_in_and_never_copies_the_session() {
        let st = setup("move").await;
        seed(&st, "acct-1", "browser_session", "CONNECTED", None);
        seed_bot(&st, "acct-1");
        let cloud = FakeCloud::default();
        cloud.cloud_computer("rt-1", "online");
        cloud.desktop("rt-mac");
        let a = Auth::default();

        let r = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("cloud_computer", Some("rt-1"))).await.unwrap();
        assert_eq!((r["moved"].as_bool(), r["needsSignIn"].as_bool()), (Some(true), Some(true)));
        assert_eq!(r["host"]["kind"], "cloud_computer");
        assert_eq!(r["host"]["needsSignIn"], true);
        assert_eq!(col(&st, "acct-1", "host_runtime_id").as_deref(), Some("rt-1"));
        assert_eq!(col(&st, "acct-1", "host_state").as_deref(), Some(NEEDS_SIGN_IN));
        assert_eq!(col(&st, "acct-1", "state").as_deref(), Some("EXPIRED"));
        let bot_state: String = st.db.connect().unwrap().query_row("SELECT state FROM bot_execution_bindings WHERE id = 'eb-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(bot_state, "NEEDS_AUTH", "dependent bots wait for the new sign-in");
        assert_eq!(cloud.total(), 0, "moving from this device copies nothing and calls nothing");

        // Same destination again: nothing changes.
        let r = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("cloud_computer", Some("rt-1"))).await.unwrap();
        assert_eq!(r["moved"], false);

        // Not a cloud computer / unknown runtime / bad shapes / other owner.
        let e = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("cloud_computer", Some("rt-mac"))).await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::CONFLICT);
        let e = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("cloud_computer", Some("rt-nope"))).await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::NOT_FOUND);
        let e = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("cloud_computer", None)).await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::BAD_REQUEST);
        let e = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("this_device", Some("rt-1"))).await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::BAD_REQUEST);
        let e = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("mars", None)).await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::BAD_REQUEST);
        let e = move_host(&st.db, &cloud, &a, "user-b", "acct-1", mv("this_device", None)).await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::NOT_FOUND);
        assert_eq!(col(&st, "acct-1", "host_runtime_id").as_deref(), Some("rt-1"), "refused moves change nothing");
    }

    #[tokio::test]
    async fn moving_between_hosts_signs_the_old_cloud_session_out_and_back_to_this_device() {
        let st = setup("move2").await;
        seed(&st, "acct-1", "browser_session", "CONNECTED", Some(("rt-1", READY)));
        let cloud = FakeCloud::default();
        cloud.cloud_computer("rt-1", "online");
        cloud.cloud_computer("rt-2", "online");
        let a = Auth::default();

        let r = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("cloud_computer", Some("rt-2"))).await.unwrap();
        assert_eq!(r["oldHostSession"], "signed_out");
        assert_eq!(cloud.calls_to("DELETE", &format!("{ACCOUNTS_PATH}/acct-1")), 1);
        assert_eq!(cloud.calls.lock().unwrap()[0].0, "rt-1", "the old host is the one signed out");
        assert_eq!(col(&st, "acct-1", "host_state").as_deref(), Some(NEEDS_SIGN_IN));
        assert_eq!(col(&st, "acct-1", "state").as_deref(), Some("EXPIRED"));

        // The old host is unreachable on the way back: the move still happens, and says the cleanup was skipped.
        cloud.reply(&format!("DELETE {ACCOUNTS_PATH}/acct-1"), 503, json!({ "error": "runtime_offline" }));
        let r = move_host(&st.db, &cloud, &a, "user-a", "acct-1", mv("this_device", None)).await.unwrap();
        assert_eq!((r["moved"].as_bool(), r["oldHostSession"].as_str()), (Some(true), Some("skipped")));
        assert_eq!(col(&st, "acct-1", "host_kind").as_deref(), Some(HOST_THIS_DEVICE));
        assert_eq!(col(&st, "acct-1", "host_runtime_id"), None);
        assert_eq!(col(&st, "acct-1", "host_state").as_deref(), Some(NEEDS_SIGN_IN));
        let audits: i64 = st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM connection_audit WHERE account_binding_id = 'acct-1' AND event = 'host.moved'", [], |r| r.get(0)).unwrap();
        assert_eq!(audits, 2);
    }

    #[tokio::test]
    async fn only_browser_session_accounts_can_move() {
        let st = setup("authtype").await;
        seed(&st, "acct-key", "api_key", "CONNECTED", None);
        let cloud = FakeCloud::default();
        cloud.cloud_computer("rt-1", "online");
        let e = move_host(&st.db, &cloud, &Auth::default(), "user-a", "acct-key", mv("cloud_computer", Some("rt-1"))).await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::CONFLICT);
    }

    // ---- sign-in and health check

    #[tokio::test]
    async fn sign_in_opens_the_login_on_the_cloud_computer_then_check_connects_it() {
        let st = setup("signin").await;
        seed(&st, "acct-1", "browser_session", "EXPIRED", Some(("rt-1", NEEDS_SIGN_IN)));
        let cloud = FakeCloud::default();
        cloud.cloud_computer("rt-1", "online");
        let a = Auth::default();
        let status = format!("GET {ACCOUNTS_PATH}/acct-1/status");
        cloud.reply(&status, 404, json!({ "error": "account_not_found" }));
        cloud.reply(&format!("POST {ACCOUNTS_PATH}"), 201, json!({ "account_id": "acct-1", "login": { "state": "waiting" } }));

        let r = sign_in(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap();
        assert_eq!(r["state"], "login_window_open");
        assert_eq!(r["login"]["state"], "waiting");
        assert_eq!((r["viewer"]["kind"].as_str(), r["viewer"]["runtimeId"].as_str(), r["viewer"]["provisionedInstanceId"].as_str()), (Some("cloud_computer"), Some("rt-1"), Some("pi_rt-1")));
        let create = cloud.calls.lock().unwrap().iter().find(|c| c.1 == "POST").unwrap().3.clone().unwrap();
        assert_eq!((create["provider"].as_str(), create["account_id"].as_str(), create["login"].as_bool()), (Some("chatgpt"), Some("acct-1"), Some(true)));
        assert!(create.get("password").is_none() && create.get("cookies").is_none());
        assert_eq!(col(&st, "acct-1", "host_state").as_deref(), Some(SIGNING_IN));
        assert_eq!(col(&st, "acct-1", "state").as_deref(), Some("AUTHENTICATING"));
        assert_eq!(col(&st, "acct-1", "host_remote_account_id").as_deref(), Some("acct-1"));

        // Still waiting on the person: the check keeps it signing in.
        cloud.reply(&status, 200, json!({ "account_id": "acct-1", "session_health": "auth_required" }));
        let r = check(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap();
        assert_eq!((r["sessionHealth"].as_str(), r["host"]["state"].as_str(), r["host"]["needsSignIn"].as_bool()), (Some("auth_required"), Some(SIGNING_IN), Some(true)));
        assert_eq!(col(&st, "acct-1", "state").as_deref(), Some("AUTHENTICATING"));

        // Signed in: connected, nothing owed.
        cloud.reply(&status, 200, json!({ "account_id": "acct-1", "session_health": "ready" }));
        let r = check(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap();
        assert_eq!((r["host"]["state"].as_str(), r["host"]["needsSignIn"].as_bool()), (Some(READY), Some(false)));
        assert_eq!(col(&st, "acct-1", "state").as_deref(), Some("CONNECTED"));
        assert!(col(&st, "acct-1", "verified_at").is_some());
        assert_eq!(col(&st, "acct-1", "host_state").as_deref(), Some(READY));

        // Signing in again later reopens the login on the existing gateway account, never recreating it.
        cloud.reply(&status, 200, json!({ "account_id": "acct-1", "session_health": "ready" }));
        let posts_before = cloud.calls_to("POST", ACCOUNTS_PATH);
        sign_in(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap();
        assert_eq!(cloud.calls_to("POST", ACCOUNTS_PATH), posts_before);
        assert_eq!(cloud.calls_to("POST", &format!("{ACCOUNTS_PATH}/acct-1/login")), 1);
    }

    #[tokio::test]
    async fn check_reports_a_host_that_was_never_signed_in_and_a_blocked_one() {
        let st = setup("check").await;
        seed(&st, "acct-1", "browser_session", "EXPIRED", Some(("rt-1", NEEDS_SIGN_IN)));
        let cloud = FakeCloud::default();
        let a = Auth::default();
        let status = format!("GET {ACCOUNTS_PATH}/acct-1/status");
        cloud.reply(&status, 404, json!({}));
        let r = check(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap();
        assert_eq!((r["sessionHealth"].is_null(), r["host"]["state"].as_str()), (true, Some(NEEDS_SIGN_IN)));
        cloud.reply(&status, 200, json!({ "session_health": "challenge_presented" }));
        let r = check(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap();
        assert_eq!((r["connection"].as_str(), r["host"]["needsSignIn"].as_bool()), (Some("BLOCKED"), Some(true)));
        assert_ne!(col(&st, "acct-1", "state").as_deref(), Some("CONNECTED"));
        // A sleeping/offline host is reported, not swallowed.
        cloud.reply(&status, 503, json!({ "error": "runtime_offline" }));
        let e = check(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap_err();
        assert_eq!(e.response().status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn sign_in_and_check_need_a_cloud_host() {
        let st = setup("localonly").await;
        seed(&st, "acct-1", "browser_session", "CONNECTED", None);
        let cloud = FakeCloud::default();
        let a = Auth::default();
        assert_eq!(sign_in(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap_err().response().status(), StatusCode::CONFLICT);
        assert_eq!(check(&st.db, &cloud, &a, "user-a", "acct-1").await.unwrap_err().response().status(), StatusCode::CONFLICT);
        assert_eq!(sign_in(&st.db, &cloud, &a, "user-b", "acct-1").await.unwrap_err().response().status(), StatusCode::NOT_FOUND);
        assert_eq!(cloud.total(), 0);
    }

    // ---- status

    #[tokio::test]
    async fn status_reports_host_online_last_seen_and_needs_sign_in() {
        let st = setup("status").await;
        seed(&st, "acct-local", "browser_session", "CONNECTED", None);
        seed(&st, "acct-up", "browser_session", "CONNECTED", Some(("rt-1", READY)));
        seed(&st, "acct-asleep", "browser_session", "EXPIRED", Some(("rt-2", NEEDS_SIGN_IN)));
        seed(&st, "acct-gone", "browser_session", "CONNECTED", Some(("rt-gone", READY)));
        let cloud = FakeCloud::default();
        cloud.cloud_computer("rt-1", "online");
        cloud.cloud_computer("rt-2", "offline");
        let a = Auth::default();
        async fn status(st: &Arc<AppState>, cloud: &FakeCloud, a: &Auth, id: &str) -> Value {
            let h = load_host(&st.db, "user-a", id).unwrap().unwrap();
            host_status(cloud, a, &h).await
        }
        let local = status(&st, &cloud, &a, "acct-local").await;
        assert_eq!((local["kind"].as_str(), local["online"].as_bool(), local["needsSignIn"].as_bool()), (Some("this_device"), Some(true), Some(false)));
        let up = status(&st, &cloud, &a, "acct-up").await;
        assert_eq!((up["kind"].as_str(), up["runtimeId"].as_str(), up["online"].as_bool(), up["needsSignIn"].as_bool(), up["wakeable"].as_bool()), (Some("cloud_computer"), Some("rt-1"), Some(true), Some(false), Some(true)));
        assert_eq!(up["lastSeen"], "2026-10-03T00:00:00Z");
        let asleep = status(&st, &cloud, &a, "acct-asleep").await;
        assert_eq!((asleep["online"].as_bool(), asleep["needsSignIn"].as_bool(), asleep["wakeable"].as_bool()), (Some(false), Some(true), Some(true)));
        let gone = status(&st, &cloud, &a, "acct-gone").await;
        assert_eq!((gone["online"].as_bool(), gone["error"].as_str()), (Some(false), Some("runtime_not_found")));
    }

    #[tokio::test]
    async fn get_account_carries_the_host_block_and_account_json_has_host_columns() {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        let st = setup("get").await;
        seed(&st, "acct-1", "browser_session", "CONNECTED", None);
        let cloud = FakeCloud::default();
        let resp = get_account_with_host_using(&st.db, &cloud, &Auth::default(), "user-a", "acct-1").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!((v["host"]["kind"].as_str(), v["account"]["hostKind"].as_str(), v["account"]["hostState"].as_str()), (Some("this_device"), Some("this_device"), Some("ready")));
        let resp = get_account_with_host_using(&st.db, &cloud, &Auth::default(), "user-b", "acct-1").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // The real router builds without a route overlap and validates bodies before touching the cloud.
        let app = crate::agent_gateway_routes::agent_gateway_router().with_state(st.clone());
        let user = AuthUser { user_id: "user-a".into(), email: None, name: None, avatar_url: None, tenant_id: None, organization_id: None, organization_role: None, organization_slug: None };
        let req = Request::builder()
            .method("POST")
            .uri("/gateway/provider-accounts/acct-1/host")
            .header("content-type", "application/json")
            .extension(user)
            .body(Body::from(json!({ "host": "cloud_computer" }).to_string()))
            .unwrap();
        use tower::ServiceExt;
        assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn gateway_provider_names_and_runtime_parsing() {
        assert_eq!(gateway_provider("openai"), "chatgpt");
        assert_eq!(gateway_provider("anthropic"), "claude");
        assert_eq!(gateway_provider("xai"), "grok");
        assert_eq!(gateway_provider("kimi"), "kimi");
        let v = json!({ "runtimes": [{ "id": "r1", "name": "N", "runtimeType": "cloud", "status": "online", "lastSeenAt": "t", "wakeable": true, "provisionedInstanceId": "pi_1" }, { "name": "no id" }] });
        let r = parse_runtimes(&v);
        assert_eq!(r.len(), 1);
        assert!(is_cloud_computer(&r[0]));
        assert_eq!(Auth::from_headers(&HeaderMap::new()).bearer, None);
    }
}
