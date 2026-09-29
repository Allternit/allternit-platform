//! Subscription Fabric forwarder (SURFACES_PLAN §3 step 1).
//!
//! The subscription gateway runs only on the user's Sessions computer (D15).
//! This module is the one way the platform reaches it:
//!
//! - `/subscriptions/binding` records which of the user's computers is the
//!   Sessions computer, its gateway guest port, and the gateway token (sealed
//!   at rest). The token never leaves allternit-api again — gizzi and the UI
//!   call `/subscriptions/gateway/*` and the token is added here.
//! - `/subscriptions/disclosure` serves the provider-terms disclosure and
//!   records acknowledgements (D16 part 1).
//! - `/subscriptions/human-actions` mints single-use ids at the surface where
//!   a human sent or confirmed something (D16 part 2).
//! - `/subscriptions/gateway/*path` forwards to the gateway's `/v1/*`. Task
//!   submission (`POST v1/tasks`) is checked first: a pinned provider, a
//!   current disclosure acknowledgement for it, and an unused human action,
//!   which is stamped into the task as `initiated_by`.

use axum::{
    body::Bytes,
    extract::{Extension, Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::auth::{AuthUser, CallerKind};
use crate::computer_routes::{error_response, fetch_computer, ComputerStatus};
use crate::AppState;

/// Bump when the disclosure text changes: every user acknowledges again.
pub const DISCLOSURE_VERSION: i64 = 1;
const DEFAULT_GATEWAY_PORT: u16 = 7788;
/// How long a minted human action stays usable.
const HUMAN_ACTION_TTL_SECS: i64 = 600;
/// Plain requests (task JSON, artifact downloads). Event streams are exempt.
const GATEWAY_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const GATEWAY_BODY_LIMIT: usize = 10 * 1024 * 1024;
pub const HUMAN_ACTION_HEADER: &str = "x-allternit-human-action";

pub(crate) struct ProviderDisclosure {
    pub(crate) provider: &'static str,
    pub(crate) name: &'static str,
    company: &'static str,
}

const PROVIDERS: &[ProviderDisclosure] = &[
    ProviderDisclosure { provider: "chatgpt", name: "ChatGPT", company: "OpenAI" },
    ProviderDisclosure { provider: "claude", name: "Claude", company: "Anthropic" },
    ProviderDisclosure { provider: "kimi", name: "Kimi", company: "Moonshot AI" },
];

pub(crate) fn provider_disclosure(provider: &str) -> Option<&'static ProviderDisclosure> {
    PROVIDERS.iter().find(|p| p.provider == provider)
}

/// Register 1 (plain, no guarantees). Versioned by DISCLOSURE_VERSION.
fn disclosure_text(p: &ProviderDisclosure) -> String {
    format!(
        "Using your {name} subscription through Allternit means Allternit operates your {name} \
account in a browser on your Sessions computer, on your behalf. {company}'s terms restrict \
automated or programmatic use of personal subscriptions, and {company} can limit or suspend \
accounts that break them. Allternit cannot promise {company} will allow this use.\n\n\
To keep you in control: every {name} task starts only when you press send or confirm it, and \
any question {name} asks during a task comes to you to answer. Agents, bots and schedules can \
prepare a {name} task, but it runs only after you confirm it.\n\n\
Your {name} login stays on your Sessions computer. This is not legal advice; review {company}'s \
terms before continuing.",
        name = p.name,
        company = p.company,
    )
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/subscriptions/status", get(get_status))
        .route("/subscriptions/binding", get(get_binding).put(put_binding).delete(delete_binding))
        .route("/subscriptions/disclosure", get(get_disclosure))
        .route("/subscriptions/disclosure/ack", post(ack_disclosure))
        .route("/subscriptions/human-actions", post(post_human_action))
        .route("/subscriptions/gateway/*path", any(gateway_forward))
        .route("/subscriptions/mcp", post(crate::subscription_mcp::handle_rpc))
}

// ── Storage ──────────────────────────────────────────────────────────────────

struct Binding {
    computer_id: String,
    guest_port: u16,
    token: String,
}

fn load_binding(db: &crate::db::DbHandle, user_id: &str) -> rusqlite::Result<Option<Binding>> {
    let conn = db.connect()?;
    let row = conn
        .query_row(
            "SELECT computer_id, guest_port, token_sealed FROM subs_gateway_bindings WHERE user_id = ?1",
            params![user_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)),
        )
        .optional()?;
    Ok(row.map(|(computer_id, port, sealed)| Binding {
        computer_id,
        guest_port: port as u16,
        token: crate::token_crypto::open(&sealed),
    }))
}

pub(crate) fn acknowledged_version(db: &crate::db::DbHandle, user_id: &str, provider: &str) -> rusqlite::Result<Option<i64>> {
    db.connect()?
        .query_row(
            "SELECT MAX(version) FROM subs_disclosure_acks WHERE user_id = ?1 AND provider = ?2",
            params![user_id, provider],
            |row| row.get(0),
        )
}

/// Mint a single-use human action for `user_id` at `surface` (e.g.
/// `chat.send`, `approval.confirm`). Returns `(action_id, expires_at)`.
/// Call this only where a person actually acted.
pub(crate) fn mint_human_action(
    db: &crate::db::DbHandle,
    user_id: &str,
    surface: &str,
) -> rusqlite::Result<(String, String)> {
    mint_action(db, user_id, surface, None)
}

/// Mint a human action that can start only the task the person saw:
/// `task_digest` is [`task_digest`] of the confirmed card's task. The
/// forwarder refuses it for any other capability, provider, prompt or
/// options. Use this for approval cards.
pub(crate) fn mint_bound_human_action(
    db: &crate::db::DbHandle,
    user_id: &str,
    surface: &str,
    task_digest: &str,
) -> rusqlite::Result<(String, String)> {
    mint_action(db, user_id, surface, Some(task_digest))
}

fn mint_action(
    db: &crate::db::DbHandle,
    user_id: &str,
    surface: &str,
    task_digest: Option<&str>,
) -> rusqlite::Result<(String, String)> {
    let action_id = format!("ha_{}", uuid::Uuid::new_v4().simple());
    let conn = db.connect()?;
    conn.execute(
        "INSERT INTO subs_human_actions (action_id, user_id, surface, expires_at, task_digest) \
         VALUES (?1, ?2, ?3, datetime('now', ?4), ?5)",
        params![action_id, user_id, surface, format!("+{HUMAN_ACTION_TTL_SECS} seconds"), task_digest],
    )?;
    let expires_at: String = conn.query_row(
        "SELECT expires_at FROM subs_human_actions WHERE action_id = ?1",
        params![action_id],
        |row| row.get(0),
    )?;
    Ok((action_id, expires_at))
}

/// The task a person confirmed, as one digest: SHA-256 over the canonical
/// JSON of `[capability, provider, prompt, options]` (object keys sorted at
/// every level; missing/null options count as `{}`). An approval card binds
/// its human action to this, and the forwarder recomputes it from the body
/// actually submitted.
pub(crate) fn task_digest(capability: &str, provider: &str, prompt: &str, options: Option<&Value>) -> String {
    use sha2::{Digest, Sha256};
    let options = match options {
        None | Some(Value::Null) => json!({}),
        Some(v) => v.clone(),
    };
    let canonical = canonical_json(&json!([capability, provider, prompt, options]));
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

/// JSON text with object keys sorted at every level, independent of the
/// `serde_json` map ordering feature.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical_json(&map[k])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// Consume `action_id` for one task submission. True when it belongs to the
/// user, has not expired, is unused — or was used by a retry of the same
/// submission (same idempotency key) — and, when it was minted on an approval
/// card, `task_digest` matches the task that card showed.
fn consume_human_action(
    db: &crate::db::DbHandle,
    user_id: &str,
    action_id: &str,
    idempotency_key: Option<&str>,
    task_digest: &str,
) -> rusqlite::Result<bool> {
    let changed = db.connect()?.execute(
        "UPDATE subs_human_actions \
         SET consumed_at = COALESCE(consumed_at, datetime('now')), idempotency_key = COALESCE(idempotency_key, ?3) \
         WHERE action_id = ?1 AND user_id = ?2 AND expires_at > datetime('now') \
           AND (task_digest IS NULL OR task_digest = ?4) \
           AND (consumed_at IS NULL OR (idempotency_key IS NOT NULL AND idempotency_key = ?3))",
        params![action_id, user_id, idempotency_key, task_digest],
    )?;
    Ok(changed == 1)
}

fn db_error(e: impl std::fmt::Display) -> Response {
    warn!(error = %e, "subscription route database error");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "database error")
}

fn coded(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

/// 403 for a human act (minting or spending a human action, approving a
/// subscription card) attempted with an agent's delegated credential.
pub(crate) fn person_required() -> Response {
    coded(
        StatusCode::FORBIDDEN,
        json!({
            "error": "person_required",
            "detail": "only a person signed in to the Allternit app can start or approve a subscription task; agents can only prepare one",
        }),
    )
}

// ── Status / binding ─────────────────────────────────────────────────────────

async fn get_status(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let binding = match load_binding(&state.db, &user.user_id) {
        Ok(b) => b,
        Err(e) => return db_error(e),
    };
    let mut computer_json = Value::Null;
    if let Some(b) = &binding {
        computer_json = match fetch_computer(&state, &user, &b.computer_id).await {
            Ok(Some(c)) => json!({
                "id": c.id,
                "name": c.name,
                "running": c.status == ComputerStatus::Running,
            }),
            Ok(None) => json!({"id": b.computer_id, "missing": true}),
            Err(response) => return response,
        };
    }
    let disclosure = match disclosure_json(&state, &user.user_id) {
        Ok(d) => d,
        Err(e) => return db_error(e),
    };
    Json(json!({
        "bound": binding.is_some(),
        "guest_port": binding.as_ref().map(|b| b.guest_port),
        "token_set": binding.as_ref().map(|b| !b.token.is_empty()).unwrap_or(false),
        "computer": computer_json,
        "disclosure": disclosure,
    }))
    .into_response()
}

async fn get_binding(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    match load_binding(&state.db, &user.user_id) {
        Ok(Some(b)) => Json(json!({
            "bound": true,
            "computer_id": b.computer_id,
            "guest_port": b.guest_port,
            "token_set": !b.token.is_empty(),
        }))
        .into_response(),
        Ok(None) => Json(json!({"bound": false})).into_response(),
        Err(e) => db_error(e),
    }
}

#[derive(Debug, Deserialize)]
struct PutBindingRequest {
    computer_id: String,
    guest_port: Option<u16>,
    token: String,
}

/// PUT /subscriptions/binding — bind one of the caller's computers as their
/// Sessions computer. The token is checked against the live gateway before
/// it is stored, so a typo cannot leave a binding that fails every call.
async fn put_binding(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<PutBindingRequest>,
) -> Response {
    let token = body.token.trim().to_string();
    if token.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "token is required");
    }
    let guest_port = body.guest_port.unwrap_or(DEFAULT_GATEWAY_PORT);
    if guest_port == 0 {
        return error_response(StatusCode::BAD_REQUEST, "guest_port must be 1-65535");
    }
    let computer = match fetch_computer(&state, &user, &body.computer_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "computer not found"),
        Err(response) => return response,
    };
    if computer.status != ComputerStatus::Running {
        return error_response(StatusCode::CONFLICT, "computer is not running");
    }
    let probe = crate::computer_ws::forward_to_guest(
        &state,
        &computer,
        guest_port,
        "v1/accounts",
        None,
        Method::GET,
        &gateway_headers(&HeaderMap::new(), &token),
        Bytes::new(),
        GATEWAY_REQUEST_TIMEOUT,
    )
    .await;
    match probe.status() {
        s if s.is_success() => {}
        StatusCode::UNAUTHORIZED => {
            return coded(StatusCode::BAD_REQUEST, json!({"error": "gateway_token_rejected"}))
        }
        s => {
            return coded(
                StatusCode::BAD_GATEWAY,
                json!({"error": "gateway_unreachable", "status": s.as_u16()}),
            )
        }
    }
    let result = state.db.connect().and_then(|conn| {
        conn.execute(
            "INSERT INTO subs_gateway_bindings (user_id, computer_id, guest_port, token_sealed) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(user_id) DO UPDATE SET computer_id = excluded.computer_id, \
               guest_port = excluded.guest_port, token_sealed = excluded.token_sealed, \
               updated_at = CURRENT_TIMESTAMP",
            params![user.user_id, computer.id, guest_port as i64, crate::token_crypto::seal(&token)],
        )
    });
    if let Err(e) = result {
        return db_error(e);
    }
    crate::computer_audit::log_computer_access(
        &state.db,
        &computer.id,
        &user.user_id,
        crate::computer_audit::KIND_SUBS_BINDING,
        &format!("bound as Sessions computer, gateway port {guest_port}"),
    );
    Json(json!({"bound": true, "computer_id": computer.id, "guest_port": guest_port, "token_set": true}))
        .into_response()
}

async fn delete_binding(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    match state.db.connect().and_then(|conn| {
        conn.execute("DELETE FROM subs_gateway_bindings WHERE user_id = ?1", params![user.user_id])
    }) {
        Ok(_) => Json(json!({"bound": false})).into_response(),
        Err(e) => db_error(e),
    }
}

// ── Disclosure (D16 part 1) ──────────────────────────────────────────────────

fn disclosure_json(state: &AppState, user_id: &str) -> rusqlite::Result<Value> {
    let mut providers = Vec::new();
    for p in PROVIDERS {
        let acked = acknowledged_version(&state.db, user_id, p.provider)?;
        providers.push(json!({
            "provider": p.provider,
            "name": p.name,
            "text": disclosure_text(p),
            "acknowledged": acked == Some(DISCLOSURE_VERSION),
            "acknowledged_version": acked,
        }));
    }
    Ok(json!({"version": DISCLOSURE_VERSION, "providers": providers}))
}

async fn get_disclosure(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    match disclosure_json(&state, &user.user_id) {
        Ok(v) => Json(v).into_response(),
        Err(e) => db_error(e),
    }
}

#[derive(Debug, Deserialize)]
struct AckRequest {
    provider: String,
    version: i64,
}

async fn ack_disclosure(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<AckRequest>,
) -> Response {
    if provider_disclosure(&body.provider).is_none() {
        return coded(StatusCode::BAD_REQUEST, json!({"error": "unknown_provider", "provider": body.provider}));
    }
    // Acknowledging an old text does not count as reading the current one.
    if body.version != DISCLOSURE_VERSION {
        return coded(
            StatusCode::CONFLICT,
            json!({"error": "disclosure_version_mismatch", "current_version": DISCLOSURE_VERSION}),
        );
    }
    match state.db.connect().and_then(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO subs_disclosure_acks (user_id, provider, version) VALUES (?1, ?2, ?3)",
            params![user.user_id, body.provider, body.version],
        )
    }) {
        Ok(_) => Json(json!({"provider": body.provider, "version": body.version, "acknowledged": true}))
            .into_response(),
        Err(e) => db_error(e),
    }
}

// ── Human actions (D16 part 2) ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct HumanActionRequest {
    surface: String,
}

/// POST /subscriptions/human-actions — for UI surfaces that start a fabric
/// task directly (composer tools, Settings, approval cards). The chat send
/// path mints its own inside allternit-api.
///
/// Only a person's app session can mint here (`CallerKind::Person`). An agent
/// holding the user's delegated credential (an access token in its MCP
/// config, a runtime device token) is refused: agents prepare tasks, people
/// start them (D16).
async fn post_human_action(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    caller: Option<Extension<CallerKind>>,
    Json(body): Json<HumanActionRequest>,
) -> Response {
    if !CallerKind::is_person(caller.as_ref()) {
        return person_required();
    }
    let surface = body.surface.trim();
    if surface.is_empty() || surface.len() > 64 {
        return error_response(StatusCode::BAD_REQUEST, "surface must be 1-64 characters");
    }
    match mint_human_action(&state.db, &user.user_id, surface) {
        Ok((action_id, expires_at)) => {
            (StatusCode::CREATED, Json(json!({"action_id": action_id, "expires_at": expires_at}))).into_response()
        }
        Err(e) => db_error(e),
    }
}

// ── Forwarder ────────────────────────────────────────────────────────────────

/// Request headers passed to the gateway. Everything else — the caller's own
/// Authorization, cookies, Origin (the gateway rejects any Origin not on its
/// allowlist) — is dropped; the gateway token is added.
const FORWARDED_HEADERS: &[&str] = &["accept", "content-type", "last-event-id", "range", "if-none-match"];

fn gateway_headers(incoming: &HeaderMap, token: &str) -> HeaderMap {
    let mut out = HeaderMap::new();
    for name in FORWARDED_HEADERS {
        for value in incoming.get_all(*name) {
            out.append(HeaderName::from_static(name), value.clone());
        }
    }
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        out.insert(header::AUTHORIZATION, value);
    }
    out
}

/// Why a task submission was refused before reaching the gateway.
#[derive(Debug, PartialEq)]
pub(crate) enum TaskRefusal {
    InvalidJson,
    ProviderRequired,
    UnknownProvider(String),
    DisclosureRequired(String),
    HumanActionRequired,
    HumanActionInvalid,
}

impl TaskRefusal {
    fn into_response(self) -> Response {
        match self {
            TaskRefusal::InvalidJson => coded(StatusCode::BAD_REQUEST, json!({"error": "invalid_task"})),
            TaskRefusal::ProviderRequired => coded(
                StatusCode::BAD_REQUEST,
                json!({"error": "provider_required", "detail": "set routing.provider: disclosure is per provider"}),
            ),
            TaskRefusal::UnknownProvider(p) => {
                coded(StatusCode::BAD_REQUEST, json!({"error": "unknown_provider", "provider": p}))
            }
            TaskRefusal::DisclosureRequired(p) => coded(
                StatusCode::FORBIDDEN,
                json!({"error": "disclosure_required", "provider": p, "version": DISCLOSURE_VERSION}),
            ),
            TaskRefusal::HumanActionRequired => coded(
                StatusCode::FORBIDDEN,
                json!({"error": "human_action_required", "detail": format!("send {HUMAN_ACTION_HEADER} from the surface where a person sent or confirmed this task")}),
            ),
            TaskRefusal::HumanActionInvalid => coded(
                StatusCode::FORBIDDEN,
                json!({"error": "human_action_invalid", "detail": "the human action is unknown, expired, another user's, already used, or was confirmed for a different task"}),
            ),
        }
    }
}

/// Check a `POST v1/tasks` body and return it stamped with `initiated_by`.
/// Order: shape → provider → disclosure → human action (consumed last, so a
/// refused submission never burns the action).
pub(crate) fn prepare_task_submission(
    db: &crate::db::DbHandle,
    user_id: &str,
    body: &[u8],
    human_action: Option<&str>,
) -> Result<Vec<u8>, Result<TaskRefusal, rusqlite::Error>> {
    let mut task: Map<String, Value> = serde_json::from_slice(body).map_err(|_| Ok(TaskRefusal::InvalidJson))?;
    let provider = task
        .get("routing")
        .and_then(|r| r.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(Ok(TaskRefusal::ProviderRequired))?;
    if provider_disclosure(&provider).is_none() {
        return Err(Ok(TaskRefusal::UnknownProvider(provider)));
    }
    if acknowledged_version(db, user_id, &provider).map_err(Err)? != Some(DISCLOSURE_VERSION) {
        return Err(Ok(TaskRefusal::DisclosureRequired(provider)));
    }
    let action_id = human_action
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .ok_or(Ok(TaskRefusal::HumanActionRequired))?;
    let idempotency_key = task.get("idempotency_key").and_then(Value::as_str);
    let digest = task_digest(
        task.get("capability").and_then(Value::as_str).unwrap_or_default(),
        &provider,
        task.get("prompt").and_then(Value::as_str).unwrap_or_default(),
        task.get("options"),
    );
    if !consume_human_action(db, user_id, action_id, idempotency_key, &digest).map_err(Err)? {
        return Err(Ok(TaskRefusal::HumanActionInvalid));
    }
    task.insert(
        "initiated_by".into(),
        json!({"kind": "human", "user_id": user_id, "action_id": action_id}),
    );
    task.insert("requester_kind".into(), json!("user"));
    Ok(serde_json::to_vec(&task).expect("json map serializes"))
}

/// ANY /subscriptions/gateway/*path → gateway `/{path}` on the caller's
/// Sessions computer, with the gateway token added server-side.
async fn gateway_forward(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(path): Path<String>,
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    forward(&state, &user, &path, method, &headers, uri.query(), body).await
}

/// The forwarder core, also used in-process (the subscription MCP server and
/// the approval card that runs a prepared task). `path` is the gateway path
/// (`v1/...`); a `POST v1/tasks` is checked and stamped exactly as for HTTP
/// callers.
pub(crate) async fn forward(
    state: &Arc<AppState>,
    user: &AuthUser,
    path: &str,
    method: Method,
    headers: &HeaderMap,
    query: Option<&str>,
    body: Bytes,
) -> Response {
    let path = path.trim_start_matches('/').to_string();
    if !path.starts_with("v1/") || path.split('/').any(|seg| seg == ".." || seg == ".") {
        return error_response(StatusCode::NOT_FOUND, "not a gateway route");
    }
    if body.len() > GATEWAY_BODY_LIMIT {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "request body exceeds 10 MB");
    }
    let binding = match load_binding(&state.db, &user.user_id) {
        Ok(Some(b)) if !b.token.is_empty() => b,
        Ok(_) => {
            return coded(
                StatusCode::CONFLICT,
                json!({"error": "sessions_computer_not_bound", "detail": "bind a Sessions computer: PUT /api/v1/subscriptions/binding"}),
            )
        }
        Err(e) => return db_error(e),
    };
    let computer = match fetch_computer(state, user, &binding.computer_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return coded(StatusCode::CONFLICT, json!({"error": "sessions_computer_missing"})),
        Err(response) => return response,
    };
    if computer.status != ComputerStatus::Running {
        return coded(StatusCode::SERVICE_UNAVAILABLE, json!({"error": "sessions_computer_not_running"}));
    }

    let mut body = body;
    if method == Method::POST && path.trim_end_matches('/') == "v1/tasks" {
        let action = headers.get(HUMAN_ACTION_HEADER).and_then(|v| v.to_str().ok());
        match prepare_task_submission(&state.db, &user.user_id, &body, action) {
            Ok(stamped) => body = Bytes::from(stamped),
            Err(Ok(refusal)) => return refusal.into_response(),
            Err(Err(e)) => return db_error(e),
        }
    }

    let response = crate::computer_ws::forward_to_guest(
        state,
        &computer,
        binding.guest_port,
        &path,
        query,
        method.clone(),
        &gateway_headers(headers, &binding.token),
        body,
        GATEWAY_REQUEST_TIMEOUT,
    )
    .await;
    crate::computer_audit::log_computer_access(
        &state.db,
        &computer.id,
        &user.user_id,
        crate::computer_audit::KIND_SUBS_GATEWAY,
        &format!("{} /{} → {}", method, path, response.status()),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Mutex;
    use tower::ServiceExt;

    const USER: &str = "user-1";

    fn user() -> AuthUser {
        AuthUser {
            user_id: USER.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    /// What the fake gateway saw: (method, path, authorization, origin, body).
    type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Option<String>, Value)>>>;

    /// A fake gateway on a loopback port that records every request.
    async fn fake_gateway() -> (String, Seen) {
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let app = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
            let record = record.clone();
            async move {
                let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(String::from);
                let origin = headers.get("origin").and_then(|v| v.to_str().ok()).map(String::from);
                let json_body = serde_json::from_slice(&body).unwrap_or(Value::Null);
                record.lock().unwrap().push((method.to_string(), uri.path().to_string(), auth.clone(), origin, json_body));
                if auth.as_deref() != Some("Bearer gw-token") {
                    return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"}))).into_response();
                }
                (StatusCode::OK, Json(json!({"ok": true, "path": uri.path()}))).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    async fn setup() -> (Router, Arc<AppState>, Seen) {
        let (url, seen) = fake_gateway().await;
        let temp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn allternit_driver_interface::ExecutionDriver> =
            Arc::new(crate::computer_ws::tests::StubDriver(Some(url)));
        let state = crate::test_helpers::app_state_with_driver(temp.path(), Some(driver)).await;
        std::mem::forget(temp);
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, os, native_id, billing_source)
                 VALUES ('computer-1', 'cloud_desktop', 'incus', 'running', 'user', ?1, 'sessions', 'ubuntu-24.04', 'sandbox-1', 'credits')",
                params![USER],
            )
            .unwrap();
        // The UI's session: a person.
        let app = router()
            .layer(Extension(user()))
            .layer(Extension(CallerKind::Person))
            .with_state(state.clone());
        (app, state, seen)
    }

    async fn send(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: Value) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = if body.is_null() { Body::empty() } else { Body::from(body.to_string()) };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn bind(app: &Router) {
        let (status, body) = send(
            app,
            "PUT",
            "/subscriptions/binding",
            &[],
            json!({"computer_id": "computer-1", "token": "gw-token"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    fn task() -> Value {
        json!({"capability": "chat.create", "prompt": "hi", "routing": {"provider": "chatgpt"}})
    }

    #[tokio::test]
    async fn forwarding_needs_a_bound_sessions_computer() {
        let (app, _state, seen) = setup().await;
        let (status, body) = send(&app, "GET", "/subscriptions/gateway/v1/accounts", &[], Value::Null).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "sessions_computer_not_bound");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn binding_verifies_the_token_and_never_returns_it() {
        let (app, _state, _seen) = setup().await;
        let (status, body) = send(
            &app,
            "PUT",
            "/subscriptions/binding",
            &[],
            json!({"computer_id": "computer-1", "token": "wrong"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "gateway_token_rejected");
        let (status, body) = send(
            &app,
            "PUT",
            "/subscriptions/binding",
            &[],
            json!({"computer_id": "someone-elses", "token": "gw-token"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        bind(&app).await;
        let (_, body) = send(&app, "GET", "/subscriptions/binding", &[], Value::Null).await;
        assert_eq!(body["computer_id"], "computer-1");
        assert_eq!(body["guest_port"], 7788);
        assert!(!body.to_string().contains("gw-token"));
    }

    #[tokio::test]
    async fn reads_forward_with_the_server_side_token_and_no_origin() {
        let (app, _state, seen) = setup().await;
        bind(&app).await;
        let (status, body) = send(
            &app,
            "GET",
            "/subscriptions/gateway/v1/accounts",
            &[("authorization", "Bearer user-jwt"), ("origin", "https://ai.allternit.com")],
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let seen = seen.lock().unwrap();
        let (_, path, auth, origin, _) = seen.last().unwrap();
        assert_eq!(path, "/v1/accounts");
        assert_eq!(auth.as_deref(), Some("Bearer gw-token"));
        assert_eq!(origin, &None);
    }

    #[tokio::test]
    async fn only_gateway_v1_paths_forward() {
        let (app, _state, _seen) = setup().await;
        bind(&app).await;
        for path in ["/subscriptions/gateway/health", "/subscriptions/gateway/v1/../admin"] {
            let (status, _) = send(&app, "GET", path, &[], Value::Null).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn d16_task_needs_provider_disclosure_and_a_human_action() {
        let (app, state, seen) = setup().await;
        bind(&app).await;
        let before = seen.lock().unwrap().len();

        let (status, body) = send(
            &app,
            "POST",
            "/subscriptions/gateway/v1/tasks",
            &[],
            json!({"capability": "chat.create", "prompt": "hi"}),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("provider_required")));

        let (status, body) = send(&app, "POST", "/subscriptions/gateway/v1/tasks", &[], task()).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("disclosure_required")));
        assert_eq!(body["provider"], "chatgpt");

        let (status, _) = send(
            &app,
            "POST",
            "/subscriptions/disclosure/ack",
            &[],
            json!({"provider": "chatgpt", "version": DISCLOSURE_VERSION}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send(&app, "POST", "/subscriptions/gateway/v1/tasks", &[], task()).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("human_action_required")));

        let (_, minted) = send(&app, "POST", "/subscriptions/human-actions", &[], json!({"surface": "chat.send"})).await;
        let action = minted["action_id"].as_str().unwrap().to_string();
        // A caller cannot pick its own stamp: the forwarder overwrites it.
        let mut forged = task();
        forged["initiated_by"] = json!({"kind": "human", "user_id": "someone-else", "action_id": "made-up"});
        let (status, body) = send(
            &app,
            "POST",
            "/subscriptions/gateway/v1/tasks",
            &[(HUMAN_ACTION_HEADER, &action)],
            forged,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), before + 1, "refused submissions never reach the gateway");
            let (_, path, _, _, sent) = seen.last().unwrap();
            assert_eq!(path, "/v1/tasks");
            assert_eq!(sent["initiated_by"], json!({"kind": "human", "user_id": USER, "action_id": action}));
            assert_eq!(sent["requester_kind"], "user");
        }

        // Single use.
        let (status, body) = send(
            &app,
            "POST",
            "/subscriptions/gateway/v1/tasks",
            &[(HUMAN_ACTION_HEADER, &action)],
            task(),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("human_action_invalid")));

        // Another user's action is not usable.
        let (other, _) = mint_human_action(&state.db, "user-2", "chat.send").unwrap();
        let (status, _) = send(&app, "POST", "/subscriptions/gateway/v1/tasks", &[(HUMAN_ACTION_HEADER, &other)], task()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn d16_an_agent_credential_cannot_mint_a_human_action() {
        let (_app, state, _seen) = setup().await;
        for caller in [Some(CallerKind::Agent), None] {
            let mut app = router().layer(Extension(user()));
            if let Some(kind) = caller {
                app = app.layer(Extension(kind));
            }
            let app = app.with_state(state.clone());
            let (status, body) =
                send(&app, "POST", "/subscriptions/human-actions", &[], json!({"surface": "approval.confirm"})).await;
            assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("person_required")), "{caller:?}");
        }
        let minted: i64 = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM subs_human_actions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(minted, 0);
    }

    #[tokio::test]
    async fn d16_a_card_bound_action_starts_only_the_task_the_card_showed() {
        let (_app, state, _seen) = setup().await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO subs_disclosure_acks (user_id, provider, version) VALUES (?1, 'chatgpt', ?2)",
                params![USER, DISCLOSURE_VERSION],
            )
            .unwrap();
        let shown = task_digest("document.create", "chatgpt", "summarise the Q3 numbers", None);
        let (action, _) = mint_bound_human_action(&state.db, USER, "approval.confirm", &shown).unwrap();
        let submit = |prompt: &str, options: Value| {
            json!({"capability": "document.create", "prompt": prompt, "routing": {"provider": "chatgpt"}, "options": options})
                .to_string()
                .into_bytes()
        };
        // A different prompt, capability, provider or options: refused, and
        // the action is not burnt.
        for body in [
            submit("the pasted confidential file", json!({})),
            submit("summarise the Q3 numbers", json!({"format": "pdf"})),
            json!({"capability": "research.deep", "prompt": "summarise the Q3 numbers", "routing": {"provider": "chatgpt"}})
                .to_string()
                .into_bytes(),
        ] {
            assert_eq!(
                prepare_task_submission(&state.db, USER, &body, Some(&action)).unwrap_err().unwrap(),
                TaskRefusal::HumanActionInvalid
            );
        }
        // The task that was shown (options {} and absent are the same).
        assert!(prepare_task_submission(&state.db, USER, &submit("summarise the Q3 numbers", json!({})), Some(&action)).is_ok());
    }

    #[test]
    fn task_digest_is_canonical() {
        let a = task_digest("image.generate", "chatgpt", "a cat", Some(&json!({"size": "1024", "n": 1})));
        let b = task_digest("image.generate", "chatgpt", "a cat", Some(&json!({"n": 1, "size": "1024"})));
        assert_eq!(a, b);
        assert_eq!(task_digest("x", "y", "z", None), task_digest("x", "y", "z", Some(&Value::Null)));
        assert_eq!(task_digest("x", "y", "z", None), task_digest("x", "y", "z", Some(&json!({}))));
        assert_ne!(task_digest("x", "y", "z", None), task_digest("x", "y", "z ", None));
    }

    #[tokio::test]
    async fn a_retry_with_the_same_idempotency_key_reuses_the_action() {
        let (_app, state, _seen) = setup().await;
        state
            .db
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO subs_disclosure_acks (user_id, provider, version) VALUES (?1, 'chatgpt', ?2)",
                params![USER, DISCLOSURE_VERSION],
            )
            .unwrap();
        let (action, _) = mint_human_action(&state.db, USER, "chat.send").unwrap();
        let mut body = task();
        body["idempotency_key"] = json!("msg-1");
        let bytes = body.to_string().into_bytes();
        assert!(prepare_task_submission(&state.db, USER, &bytes, Some(&action)).is_ok());
        assert!(prepare_task_submission(&state.db, USER, &bytes, Some(&action)).is_ok());
        body["idempotency_key"] = json!("msg-2");
        let other = body.to_string().into_bytes();
        assert_eq!(
            prepare_task_submission(&state.db, USER, &other, Some(&action)).unwrap_err().unwrap(),
            TaskRefusal::HumanActionInvalid
        );
    }

    #[tokio::test]
    async fn expired_actions_and_old_disclosure_versions_do_not_count() {
        let (app, state, _seen) = setup().await;
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO subs_disclosure_acks (user_id, provider, version) VALUES (?1, 'chatgpt', ?2)",
            params![USER, DISCLOSURE_VERSION - 1],
        )
        .unwrap();
        let bytes = task().to_string().into_bytes();
        assert_eq!(
            prepare_task_submission(&state.db, USER, &bytes, Some("x")).unwrap_err().unwrap(),
            TaskRefusal::DisclosureRequired("chatgpt".into())
        );
        let (status, _) = send(
            &app,
            "POST",
            "/subscriptions/disclosure/ack",
            &[],
            json!({"provider": "chatgpt", "version": DISCLOSURE_VERSION - 1}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        conn.execute(
            "INSERT INTO subs_disclosure_acks (user_id, provider, version) VALUES (?1, 'chatgpt', ?2)",
            params![USER, DISCLOSURE_VERSION],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO subs_human_actions (action_id, user_id, surface, expires_at) VALUES ('old', ?1, 'chat.send', datetime('now', '-1 seconds'))",
            params![USER],
        )
        .unwrap();
        assert_eq!(
            prepare_task_submission(&state.db, USER, &bytes, Some("old")).unwrap_err().unwrap(),
            TaskRefusal::HumanActionInvalid
        );
    }

    #[tokio::test]
    async fn disclosure_lists_every_provider_with_plain_text() {
        let (app, _state, _seen) = setup().await;
        let (_, body) = send(&app, "GET", "/subscriptions/disclosure", &[], Value::Null).await;
        assert_eq!(body["version"], DISCLOSURE_VERSION);
        let providers = body["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 3);
        for p in providers {
            assert_eq!(p["acknowledged"], false);
            let text = p["text"].as_str().unwrap();
            assert!(text.contains("press send or confirm"));
            assert!(!text.to_lowercase().contains("guarantee"));
        }
    }
}
