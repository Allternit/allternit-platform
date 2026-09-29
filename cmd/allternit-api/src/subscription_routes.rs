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

use crate::auth::AuthUser;
use crate::computer_routes::{error_response, fetch_computer, ComputerKind, ComputerStatus};
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
    let action_id = format!("ha_{}", uuid::Uuid::new_v4().simple());
    let conn = db.connect()?;
    conn.execute(
        "INSERT INTO subs_human_actions (action_id, user_id, surface, expires_at) \
         VALUES (?1, ?2, ?3, datetime('now', ?4))",
        params![action_id, user_id, surface, format!("+{HUMAN_ACTION_TTL_SECS} seconds")],
    )?;
    let expires_at: String = conn.query_row(
        "SELECT expires_at FROM subs_human_actions WHERE action_id = ?1",
        params![action_id],
        |row| row.get(0),
    )?;
    Ok((action_id, expires_at))
}

/// Consume `action_id` for one task submission. True when it belongs to the
/// user, has not expired, and is unused — or was used by a retry of the same
/// submission (same idempotency key).
fn consume_human_action(
    db: &crate::db::DbHandle,
    user_id: &str,
    action_id: &str,
    idempotency_key: Option<&str>,
) -> rusqlite::Result<bool> {
    let changed = db.connect()?.execute(
        "UPDATE subs_human_actions \
         SET consumed_at = COALESCE(consumed_at, datetime('now')), idempotency_key = COALESCE(idempotency_key, ?3) \
         WHERE action_id = ?1 AND user_id = ?2 AND expires_at > datetime('now') \
           AND (consumed_at IS NULL OR (idempotency_key IS NOT NULL AND idempotency_key = ?3))",
        params![action_id, user_id, idempotency_key],
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

/// D15: the fabric runtime never runs on the desktop the user works on (the
/// host computer the app registered), and a bot's computer belongs to the bot.
fn not_a_sessions_computer(computer: &crate::computer_routes::ComputerResponse) -> Option<&'static str> {
    if computer.kind == ComputerKind::Local && computer.provider == "host" {
        return Some("this is the desktop you work on; subscriptions run only on a separate Sessions computer");
    }
    if computer.bot_id.is_some() {
        return Some("a bot's computer can't be the Sessions computer");
    }
    None
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
    if let Some(detail) = not_a_sessions_computer(&computer) {
        return coded(StatusCode::CONFLICT, json!({"error": "sessions_computer_not_allowed", "detail": detail}));
    }
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

/// gizzi's always-ask permission class for fabric work nobody sent from the
/// chat composer: a task an agent/tool/bot prepared, or a provider question
/// from a running task. Approving one is the human act (D16).
pub const SUBSCRIPTION_PERMISSION: &str = "subscription";
/// Longest provider answer relayed from a question card.
const MAX_ANSWER_CHARS: usize = 8000;

/// Whether a `cowork_approvals` content payload is a D16 subscription card.
pub(crate) fn is_subscription_card(content: &Value) -> bool {
    content.pointer("/details/actionType").and_then(|v| v.as_str()) == Some(SUBSCRIPTION_PERMISSION)
}

#[derive(Debug, PartialEq)]
pub(crate) enum CardReplyRefusal {
    /// A provider question was approved without an answer to send.
    AnswerRequired,
}

/// The body relayed to gizzi's `POST /permission/:id/reply` for a decided
/// approval row. For an approved subscription card this is where the human
/// action is minted (`approval.confirm`) — server-side, at the moment a
/// person approved — and handed to the pending fabric task along with the
/// person's answer to a provider question. Agents never see a way to mint.
pub(crate) fn permission_reply_body(
    db: &crate::db::DbHandle,
    user_id: &str,
    content: &Value,
    reply: &str,
    answer: Option<&str>,
) -> Result<Value, Result<CardReplyRefusal, rusqlite::Error>> {
    if reply != "once" || !is_subscription_card(content) {
        return Ok(json!({ "reply": reply }));
    }
    let answer = answer.map(str::trim).filter(|a| !a.is_empty());
    let question = content.pointer("/subscription/kind").and_then(|v| v.as_str()) == Some("question");
    if question && answer.is_none() {
        return Err(Ok(CardReplyRefusal::AnswerRequired));
    }
    let (action_id, _) = mint_human_action(db, user_id, "approval.confirm").map_err(Err)?;
    let mut body = json!({ "reply": "once", "humanAction": action_id });
    if let Some(answer) = answer {
        body["answer"] = json!(answer.chars().take(MAX_ANSWER_CHARS).collect::<String>());
    }
    Ok(body)
}

/// Surfaces only allternit-api mints, in process, where the human act
/// happened: the chat bridge on a send (`chat.send`) and the approval
/// decision route when a person approves a subscription card
/// (`approval.confirm`). Nobody can ask for these over HTTP.
pub(crate) const SERVER_ONLY_SURFACES: &[&str] = &["chat.send", "approval.confirm"];

/// POST /subscriptions/human-actions — for UI surfaces that start a fabric
/// task directly (composer tools, Settings). The chat send path and approval
/// cards mint theirs inside allternit-api. An agent runtime (a cloud-issued
/// runtime-device token) can never mint one: agents only *prepare* fabric
/// tasks, a person confirms them on an approval card (D16).
async fn post_human_action(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<HumanActionRequest>,
) -> Response {
    let surface = body.surface.trim();
    if surface.is_empty() || surface.len() > 64 {
        return error_response(StatusCode::BAD_REQUEST, "surface must be 1-64 characters");
    }
    if SERVER_ONLY_SURFACES.contains(&surface) {
        return coded(
            StatusCode::FORBIDDEN,
            json!({"error": "human_action_surface_reserved", "detail": format!("{surface} actions are created by the platform where the person acted")}),
        );
    }
    if crate::connector_routes::device_token_from_headers(&headers).is_some() {
        return coded(
            StatusCode::FORBIDDEN,
            json!({"error": "human_action_agent_caller", "detail": "agent runtimes cannot create human actions; ask the person to confirm on an approval card"}),
        );
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
                json!({"error": "human_action_invalid", "detail": "the human action is unknown, expired, another user's, or already used"}),
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
    if !consume_human_action(db, user_id, action_id, idempotency_key).map_err(Err)? {
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
        let app = router().layer(Extension(user())).with_state(state.clone());
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
    async fn d15_this_desktop_and_bot_computers_cannot_be_bound() {
        let (app, state, seen) = setup().await;
        state
            .db
            .connect()
            .unwrap()
            .execute_batch(&format!(
                "INSERT INTO agents (id, user_id, name, model, provider, status, type)
                 VALUES ('bot-1', '{USER}', 'Bot', 'allternit-fast', 'allternit', 'idle', 'worker');
                 INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, name, billing_source)
                 VALUES ('computer-mac', 'local', 'host', 'running', 'user', '{USER}', 'Studio', 'free');
                 INSERT INTO computers (id, kind, provider, status, owner_type, owner_id, bot_id, name, billing_source)
                 VALUES ('computer-bot', 'cloud_desktop', 'incus', 'running', 'user', '{USER}', 'bot-1', 'Bot box', 'credits');"
            ))
            .unwrap();
        for id in ["computer-mac", "computer-bot"] {
            let (status, body) = send(
                &app,
                "PUT",
                "/subscriptions/binding",
                &[],
                json!({"computer_id": id, "token": "gw-token"}),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT, "{id}: {body}");
            assert_eq!(body["error"], "sessions_computer_not_allowed", "{id}");
        }
        // Refused before any request reaches a gateway, and nothing stored.
        assert!(seen.lock().unwrap().is_empty());
        let (_, body) = send(&app, "GET", "/subscriptions/binding", &[], Value::Null).await;
        assert_eq!(body["bound"], false);
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

        let (_, minted) = send(&app, "POST", "/subscriptions/human-actions", &[], json!({"surface": "composer.tool"})).await;
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
    async fn approving_a_subscription_card_mints_the_human_action_server_side() {
        let (app, state, seen) = setup().await;
        bind(&app).await;
        let (status, _) = send(&app, "POST", "/subscriptions/disclosure/ack", &[], json!({"provider": "chatgpt", "version": DISCLOSURE_VERSION})).await;
        assert_eq!(status, StatusCode::OK);

        let send_card = json!({"details": {"actionType": "subscription"}, "subscription": {"kind": "send", "provider": "chatgpt"}});
        // Ordinary tool cards relay the plain reply.
        let plain = permission_reply_body(&state.db, USER, &json!({"details": {"actionType": "bash"}}), "once", None).unwrap();
        assert_eq!(plain, json!({"reply": "once"}));
        // Rejecting a subscription card mints nothing.
        let rejected = permission_reply_body(&state.db, USER, &send_card, "reject", None).unwrap();
        assert_eq!(rejected, json!({"reply": "reject"}));

        // Approving mints an approval.confirm action the forwarder accepts once.
        let body = permission_reply_body(&state.db, USER, &send_card, "once", None).unwrap();
        let action = body["humanAction"].as_str().unwrap().to_string();
        let surface: String = state
            .db
            .connect()
            .unwrap()
            .query_row("SELECT surface FROM subs_human_actions WHERE action_id = ?1", params![action], |r| r.get(0))
            .unwrap();
        assert_eq!(surface, "approval.confirm");
        let (status, _) = send(&app, "POST", "/subscriptions/gateway/v1/tasks", &[(HUMAN_ACTION_HEADER, &action)], task()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(seen.lock().unwrap().last().unwrap().4["initiated_by"]["action_id"], json!(action));

        // A provider question needs the person's answer; it is relayed trimmed.
        let question = json!({"details": {"actionType": "subscription"}, "subscription": {"kind": "question", "provider": "chatgpt"}});
        assert!(matches!(
            permission_reply_body(&state.db, USER, &question, "once", Some("  ")),
            Err(Ok(CardReplyRefusal::AnswerRequired))
        ));
        let answered = permission_reply_body(&state.db, USER, &question, "once", Some(" Yes, continue ")).unwrap();
        assert_eq!(answered["answer"], "Yes, continue");
        assert!(answered["humanAction"].as_str().unwrap().starts_with("ha_"));
    }

    #[tokio::test]
    async fn agents_and_reserved_surfaces_cannot_mint_human_actions() {
        let (app, _state, _seen) = setup().await;
        // A UI surface may ask for one.
        let (status, body) = send(&app, "POST", "/subscriptions/human-actions", &[], json!({"surface": "composer.tool"})).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert!(body["action_id"].as_str().unwrap().starts_with("ha_"));
        // chat.send / approval.confirm are minted in process only.
        for surface in SERVER_ONLY_SURFACES {
            let (status, body) = send(&app, "POST", "/subscriptions/human-actions", &[], json!({"surface": surface})).await;
            assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("human_action_surface_reserved")));
        }
        // An agent runtime's device token never mints one.
        let (status, body) = send(
            &app,
            "POST",
            "/subscriptions/human-actions",
            &[("authorization", "Bearer allternit_runtime_abc")],
            json!({"surface": "composer.tool"}),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::FORBIDDEN, Some("human_action_agent_caller")));
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
