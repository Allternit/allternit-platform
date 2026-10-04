//! Agent Gateway bindings (`provider_account_bindings`,
//! `bot_execution_bindings`, `remote_thread_bindings`,
//! `channel_conversation_bindings`, `vendor_pack_registry`,
//! `vendor_pack_gaps`, `connection_audit`; migration V198). Spec:
//! `Research/specs/agent-gateway.md` ("Data model", "Vendor Packs", "Terms,
//! safety and failure handling") and `channel-packs.md`.
//!
//! * **Owner-scoped.** Every row carries `owner`; other users get 404.
//! * **Secrets by reference only.** Accounts hold `secretRef` / `sessionRef`
//!   on write; reads only say `hasSecretRef` / `hasSessionRef`.
//! * **State machines are server-side.** Connection, execution-binding and
//!   remote-thread states move only along the tables below; an invalid move is
//!   `409` with the allowed list.
//! * **Revocation cascade.** An account going `REVOKED` / `EXPIRED` moves every
//!   dependent execution binding to `NEEDS_AUTH` (threads are never deleted).
//! * **Ledger.** Execution-binding state changes land on the bot ledger with
//!   `bot_id`; remote-thread open/close also carry `thread_id`.
//! * **Frozen at open.** A remote thread binding's `lane` and
//!   `capabilitySnapshot` cannot change after create (409).
//! * JSON is camelCase; `*_json` columns surface without the suffix
//!   (`capabilities`, `health`, `scopes`).

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use rusqlite::types::ValueRef;
use rusqlite::{params, Connection, OptionalExtension, ToSql};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::bot_event_routes::{append_event, verify_bot_ownership, ActorBody, AppendEventBody};
use crate::db::DbHandle;
use crate::AppState;

// ---------------------------------------------------------------- state machines

pub const CONNECTION_STATES: &[&str] = &[
    "DISCONNECTED", "CONSENT_REQUIRED", "AUTHENTICATING", "VERIFYING", "CONNECTED", "DEGRADED", "AUTH_FAILED",
    "EXPIRED", "REVOKED", "BLOCKED",
];
pub const EXEC_STATES: &[&str] = &["UNBOUND", "BOUND", "READY", "DEGRADED", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED"];
pub const REMOTE_STATES: &[&str] = &["UNBOUND", "OPENING", "ACTIVE", "HANDOFF_PENDING", "CLOSED"];
pub const CHANNEL_SYNC_STATES: &[&str] = &["LIVE", "DELAYED", "RECONNECTING", "DEGRADED", "DISCONNECTED"];
pub const GAP_SEVERITIES: &[&str] = &["visual_parity", "functional", "data_loss"];
pub const GAP_STATUSES: &[&str] = &["open", "resolved", "wontfix"];

/// Mirrors `CONNECTION_TRANSITIONS` in subscription-fabric-contracts (WP1).
pub fn connection_next(from: &str) -> &'static [&'static str] {
    match from {
        "DISCONNECTED" => &["CONSENT_REQUIRED"],
        "CONSENT_REQUIRED" => &["AUTHENTICATING", "DISCONNECTED"],
        "AUTHENTICATING" => &["VERIFYING", "AUTH_FAILED", "DISCONNECTED"],
        "VERIFYING" => &["CONNECTED", "DEGRADED", "AUTH_FAILED", "DISCONNECTED"],
        "CONNECTED" => &["EXPIRED", "REVOKED", "BLOCKED", "DEGRADED", "DISCONNECTED"],
        "DEGRADED" => &["CONNECTED", "EXPIRED", "REVOKED", "BLOCKED", "DISCONNECTED"],
        "AUTH_FAILED" => &["CONSENT_REQUIRED", "AUTHENTICATING", "DISCONNECTED"],
        "EXPIRED" => &["AUTHENTICATING", "DISCONNECTED"],
        "REVOKED" => &["CONSENT_REQUIRED", "DISCONNECTED"],
        "BLOCKED" => &["AUTHENTICATING", "DISCONNECTED"],
        _ => &[],
    }
}

/// Mirrors `EXECUTION_TRANSITIONS` (WP1).
pub fn exec_next(from: &str) -> &'static [&'static str] {
    match from {
        "UNBOUND" => &["BOUND"],
        "BOUND" => &["READY", "NEEDS_AUTH", "FAILED", "UNBOUND", "DISABLED"],
        "READY" => &["DEGRADED", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED", "BOUND"],
        "DEGRADED" => &["READY", "NEEDS_AUTH", "PAUSED", "DISABLED", "FAILED"],
        "NEEDS_AUTH" => &["BOUND", "READY", "DISABLED", "FAILED"],
        "PAUSED" => &["READY", "DISABLED", "BOUND"],
        "DISABLED" => &["BOUND", "UNBOUND"],
        "FAILED" => &["BOUND", "UNBOUND"],
        _ => &[],
    }
}

/// Mirrors `REMOTE_THREAD_TRANSITIONS` (WP1).
pub fn remote_next(from: &str) -> &'static [&'static str] {
    match from {
        "UNBOUND" => &["OPENING"],
        // OPENING -> UNBOUND: the open failed before any vendor context existed; the next turn retries it.
        "OPENING" => &["ACTIVE", "UNBOUND", "CLOSED"],
        "ACTIVE" => &["HANDOFF_PENDING", "CLOSED"],
        "HANDOFF_PENDING" => &["ACTIVE", "CLOSED"],
        _ => &[],
    }
}

/// `Ok(true)` = a real move, `Ok(false)` = same state (no-op).
fn check_transition(kind: &str, all: &[&str], next: fn(&str) -> &'static [&'static str], from: &str, to: &str) -> Result<bool, ApiErr> {
    if !all.contains(&to) {
        return Err(ApiErr::bad(format!("unknown {kind} state '{to}'")));
    }
    if from == to {
        return Ok(false);
    }
    let allowed = next(from);
    if allowed.contains(&to) {
        Ok(true)
    } else {
        Err(ApiErr(
            StatusCode::CONFLICT,
            json!({ "error": format!("invalid {kind} transition {from} -> {to}"), "from": from, "to": to, "allowed": allowed }),
        ))
    }
}

/// Parity of a vendor pack from its gaps' (severity, status) pairs: full = no
/// open gaps; blocked = any open data_loss or functional gap; partial = every
/// open gap is visual_parity.
pub fn parity_of(gaps: &[(String, String)]) -> &'static str {
    let open = || gaps.iter().filter(|(_, st)| st == "open");
    if open().any(|(sev, _)| sev == "data_loss" || sev == "functional") {
        "blocked"
    } else if open().next().is_some() {
        "partial"
    } else {
        "full"
    }
}

// ---------------------------------------------------------------- plumbing

#[derive(Debug)]
pub struct ApiErr(StatusCode, Value);

impl ApiErr {
    pub(crate) fn new(s: StatusCode, m: impl Into<String>) -> Self {
        ApiErr(s, json!({ "error": m.into() }))
    }
    pub(crate) fn response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
    pub(crate) fn bad(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub(crate) fn nf(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, m)
    }
}

impl From<rusqlite::Error> for ApiErr {
    fn from(e: rusqlite::Error) -> Self {
        warn!(error = %e, "agent gateway DB error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    }
}

pub(crate) type Api<T> = Result<T, ApiErr>;
pub(crate) type Reply = Api<(StatusCode, Value)>;

pub(crate) fn ok(v: Value) -> Reply {
    Ok((StatusCode::OK, v))
}

pub(crate) async fn run<F>(state: &Arc<AppState>, f: F) -> Response
where
    F: FnOnce(&DbHandle) -> Reply + Send + 'static,
{
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || f(&db)).await {
        Ok(Ok((s, v))) => (s, Json(v)).into_response(),
        Ok(Err(ApiErr(s, v))) => (s, Json(v)).into_response(),
        Err(e) => {
            warn!(error = %e, "agent gateway task panicked");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "database error" }))).into_response()
        }
    }
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub(crate) fn id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

fn camel(s: &str) -> String {
    let s = s.strip_suffix("_json").unwrap_or(s);
    let mut out = String::new();
    let mut up = false;
    for c in s.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

const BOOL_COLS: &[&str] = &["has_secret_ref", "has_session_ref", "bidirectional", "read_only", "enabled", "fallback_used"];

/// Run a query and return each row as a camelCase JSON object.
pub(crate) fn rows(conn: &Connection, sql: &str, p: &[&dyn ToSql]) -> rusqlite::Result<Vec<Value>> {
    let mut st = conn.prepare(sql)?;
    let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    let out = st.query_map(p, |r| {
        let mut m = Map::new();
        for (i, n) in names.iter().enumerate() {
            let json_col = n.ends_with("_json") || n == "capability_snapshot";
            let v = match r.get_ref(i)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(x) if BOOL_COLS.contains(&n.as_str()) => Value::Bool(x != 0),
                ValueRef::Integer(x) => json!(x),
                ValueRef::Real(x) => json!(x),
                ValueRef::Text(t) => {
                    let s = String::from_utf8_lossy(t).to_string();
                    if json_col {
                        serde_json::from_str(&s).unwrap_or(Value::String(s))
                    } else {
                        Value::String(s)
                    }
                }
                ValueRef::Blob(_) => Value::Null,
            };
            m.insert(camel(n), v);
        }
        Ok(Value::Object(m))
    })?;
    out.collect()
}

pub(crate) fn one(conn: &Connection, sql: &str, p: &[&dyn ToSql]) -> rusqlite::Result<Option<Value>> {
    Ok(rows(conn, sql, p)?.into_iter().next())
}

pub(crate) fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

const ACCT_COLS: &str = "id, owner, vendor, auth_type, external_account_id, display_name, workspace, \
    (secret_ref IS NOT NULL) AS has_secret_ref, (session_ref IS NOT NULL) AS has_session_ref, scopes_json, \
    restricted_bot_id, state, verified_at, expires_at, created_at, updated_at, \
    host_kind, host_runtime_id, host_state, host_remote_account_id, host_last_seen_at, host_changed_at";
pub(crate) const EXEC_COLS: &str = "id, owner, bot_id, type, mode, vendor, adapter_id, account_binding_id, preferred_lane, \
    external_agent_id, external_agent_name, external_agent_avatar, capabilities_json, health_json, state, created_at, updated_at, directing_bot_id";
pub(crate) const REMOTE_COLS: &str = "id, owner, thread_id, generation, bot_id, execution_binding_id, external_context_id, \
    external_task_id, continuation_token, sync_cursor, last_remote_event_id, capability_snapshot, lane, state, \
    created_at, updated_at, closed_at";
const CHAN_COLS: &str = "id, owner, thread_id, provider, account_binding_id, external_workspace_id, external_channel_id, \
    external_conversation_id, external_thread_id, canonical_url, bidirectional, read_only, posting_identity_id, \
    last_inbound_cursor, last_outbound_cursor, sync_state, channel_name, workspace_name, created_at, updated_at";
const GAP_COLS: &str = "id, owner, vendor, capability, surface, fallback_used, severity, status, first_seen_at, \
    last_seen_at, occurrences, sample_ref";

pub(crate) fn get_account(conn: &Connection, owner: &str, aid: &str) -> Api<Value> {
    one(conn, &format!("SELECT {ACCT_COLS} FROM provider_account_bindings WHERE id = ?1 AND owner = ?2"), &[&aid, &owner])?
        .ok_or_else(|| ApiErr::nf("account not found"))
}

/// The thread's bot id, if the caller owns the thread.
fn thread_bot(conn: &Connection, owner: &str, thread_id: &str) -> Api<String> {
    conn.query_row("SELECT bot_id FROM bot_threads WHERE id = ?1 AND user_id = ?2", params![thread_id, owner], |r| r.get(0))
        .optional()?
        .ok_or_else(|| ApiErr::nf("thread not found"))
}

fn require_account(conn: &Connection, owner: &str, account_id: &Option<String>) -> Api<()> {
    match account_id {
        Some(a) => get_account(conn, owner, a).map(|_| ()).map_err(|_| ApiErr::bad("accountBindingId not found")),
        None => Ok(()),
    }
}

pub(crate) fn audit(conn: &Connection, owner: &str, account_id: &str, event: &str, from: Option<&str>, to: Option<&str>, detail: Value) {
    let r = conn.execute(
        "INSERT INTO connection_audit (id, owner, account_binding_id, event, from_state, to_state, actor, detail_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![id("aud"), owner, account_id, event, from, to, owner, detail.to_string(), now()],
    );
    if let Err(e) = r {
        warn!(error = %e, "failed to write connection_audit");
    }
}

/// Record an event on the bot ledger, optionally tagged with a thread.
fn ledger(db: &DbHandle, owner: &str, bot_id: &str, thread_id: Option<&str>, event_type: &str, payload: Value) {
    let key = format!("gw:{event_type}:{}", uuid::Uuid::new_v4());
    let body = AppendEventBody {
        event_type: event_type.to_string(),
        actor: ActorBody { r#type: "user".into(), id: owner.to_string() },
        payload,
        occurred_at: None,
        session_id: None,
        goal_id: None,
        wih_id: None,
        task_id: None,
        run_id: None,
        idempotency_key: Some(key.clone()),
    };
    match append_event(db, bot_id, &body, &now()) {
        Ok(_) => {
            if let (Some(t), Ok(conn)) = (thread_id, db.connect()) {
                let _ = conn.execute(
                    "UPDATE bot_events SET thread_id = ?1 WHERE bot_id = ?2 AND idempotency_key = ?3",
                    params![t, bot_id, key],
                );
            }
        }
        Err(e) => warn!(bot = %bot_id, error = %e, "failed to ledger gateway event"),
    }
}

/// Move an execution binding (already validated) and ledger it.
pub(crate) fn set_exec_state(db: &DbHandle, conn: &Connection, owner: &str, binding: &Value, to: &str, cause: &str) -> rusqlite::Result<()> {
    let (bid, bot_id, from) = (s(binding, "id"), s(binding, "botId"), s(binding, "state"));
    conn.execute("UPDATE bot_execution_bindings SET state = ?1, updated_at = ?2 WHERE id = ?3", params![to, now(), bid])?;
    ledger(
        db,
        owner,
        &bot_id,
        None,
        "gateway.execution_binding.state_changed",
        json!({ "bindingId": bid, "botId": bot_id, "from": from, "to": to, "cause": cause }),
    );
    Ok(())
}

// ---------------------------------------------------------------- router

pub fn agent_gateway_router() -> Router<Arc<AppState>> {
    let g = Router::new()
        .route("/provider-accounts", post(create_account).get(list_accounts))
        .route("/provider-accounts/:id/secret", post(set_secret).delete(clear_secret))
        .route("/provider-accounts/:id/agents", get(list_agents))
        .route("/execution-bindings", get(list_exec))
        .route("/provider-accounts/:id", get(crate::gateway_vendor_host::get_account_with_host).patch(patch_account).delete(delete_account))
        .route("/provider-accounts/sync-subscriptions", post(crate::subscription_sync::sync_route))
        .route("/bots/:bot_id/execution-binding", put(put_exec).get(get_exec).patch(patch_exec))
        .route("/threads/:thread_id/remote-bindings", post(create_remote).get(list_remote))
        .route("/remote-bindings/:id", patch(patch_remote))
        .route("/threads/:thread_id/channel-bindings", post(create_channel).get(list_channel))
        .route("/channel-bindings/:id", patch(patch_channel))
        .route("/vendor-packs/:vendor/gaps", post(record_gap).get(list_gaps))
        .route("/vendor-packs/:vendor/parity", get(parity))
        .route("/vendor-pack-gaps/:id", patch(patch_gap))
        .route("/bots/:bot_id/vendor-memory", get(vendor_memory_h))
        .route("/bots/:bot_id/vendor-memory/:record_id/promote", post(promote_memory_h))
        // Vendor accounts on a cloud computer (V221): move host, sign in there, check.
        .merge(crate::gateway_vendor_host::routes());
    Router::new().nest("/gateway", g)
}

// ---------------------------------------------------------------- provider accounts

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAccount {
    vendor: String,
    auth_type: String,
    external_account_id: Option<String>,
    display_name: Option<String>,
    workspace: Option<String>,
    secret_ref: Option<String>,
    session_ref: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    restricted_bot_id: Option<String>,
    expires_at: Option<String>,
}

const AUTH_TYPES: &[&str] =
    &["oauth", "browser_session", "api_key", "desktop_session", "local_endpoint", "channel_oauth", "mcp_plugin"];

async fn create_account(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(b): Json<CreateAccount>) -> Response {
    if let Some(bot) = &b.restricted_bot_id {
        if !verify_bot_ownership(&state, &user.user_id, bot).await {
            return err_resp(StatusCode::FORBIDDEN, "bot not found or access denied");
        }
    }
    let owner = user.user_id;
    run(&state, move |db| {
        if b.vendor.trim().is_empty() || !AUTH_TYPES.contains(&b.auth_type.as_str()) {
            return Err(ApiErr::bad("vendor required and authType must be one of the known auth types"));
        }
        let conn = db.connect()?;
        let aid = id("acct");
        let t = now();
        conn.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, workspace,
                secret_ref, session_ref, scopes_json, restricted_bot_id, state, expires_at, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'DISCONNECTED',?12,?13,?13)",
            params![aid, owner, b.vendor, b.auth_type, b.external_account_id, b.display_name, b.workspace, b.secret_ref,
                    b.session_ref, json!(b.scopes).to_string(), b.restricted_bot_id, b.expires_at, t],
        )?;
        audit(&conn, &owner, &aid, "account.created", None, Some("DISCONNECTED"), json!({ "vendor": b.vendor, "authType": b.auth_type }));
        Ok((StatusCode::CREATED, json!({ "account": get_account(&conn, &owner, &aid)? })))
    })
    .await
}

#[derive(Deserialize)]
struct AccountFilter {
    vendor: Option<String>,
    state: Option<String>,
}

async fn list_accounts(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Query(q): Query<AccountFilter>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let v = rows(
            &conn,
            &format!("SELECT {ACCT_COLS} FROM provider_account_bindings WHERE owner = ?1 AND (?2 IS NULL OR vendor = ?2) AND (?3 IS NULL OR state = ?3) ORDER BY created_at"),
            &[&user.user_id, &q.vendor, &q.state],
        )?;
        ok(json!({ "accounts": v }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchAccount {
    state: Option<String>,
    display_name: Option<String>,
    workspace: Option<String>,
    external_account_id: Option<String>,
    expires_at: Option<String>,
    verified_at: Option<String>,
    /// Free-text reason recorded in connection_audit.
    reason: Option<String>,
}

async fn patch_account(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(aid): Path<String>, Json(b): Json<PatchAccount>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        let acct = get_account(&conn, &owner, &aid)?;
        let from = s(&acct, "state");
        let mut moved = false;
        if let Some(to) = &b.state {
            moved = check_transition("connection", CONNECTION_STATES, connection_next, &from, to)?;
        }
        let t = now();
        conn.execute(
            "UPDATE provider_account_bindings SET
                display_name = COALESCE(?1, display_name), workspace = COALESCE(?2, workspace),
                external_account_id = COALESCE(?3, external_account_id), expires_at = COALESCE(?4, expires_at),
                verified_at = COALESCE(?5, verified_at), updated_at = ?6 WHERE id = ?7 AND owner = ?8",
            params![b.display_name, b.workspace, b.external_account_id, b.expires_at, b.verified_at, t, aid, owner],
        )?;
        if moved {
            let to = b.state.as_deref().unwrap();
            apply_account_state(db, &conn, &owner, &aid, &from, to, b.verified_at.is_some(), json!({ "reason": b.reason }))?;
        }
        ok(json!({ "account": get_account(&conn, &owner, &aid)? }))
    })
    .await
}

/// One already-checked hop: state, verified_at on CONNECTED, audit, and the
/// NEEDS_AUTH cascade. Shared by PATCH and the subscription sync so both
/// leave the same trail.
pub(crate) fn apply_account_state(
    db: &DbHandle,
    conn: &Connection,
    owner: &str,
    aid: &str,
    from: &str,
    to: &str,
    verified_given: bool,
    detail: Value,
) -> rusqlite::Result<()> {
    let t = now();
    conn.execute("UPDATE provider_account_bindings SET state = ?1, updated_at = ?2 WHERE id = ?3 AND owner = ?4", params![to, t, aid, owner])?;
    if to == "CONNECTED" && !verified_given {
        // Verified only after the probe passed: CONNECTED is only reachable via VERIFYING.
        conn.execute("UPDATE provider_account_bindings SET verified_at = ?1 WHERE id = ?2", params![t, aid])?;
    }
    audit(conn, owner, aid, "state.changed", Some(from), Some(to), detail);
    if to == "CONNECTED" {
        // A verified session means no sign-in is owed on whichever host runs it.
        conn.execute("UPDATE provider_account_bindings SET host_state = 'ready' WHERE id = ?1 AND owner = ?2", params![aid, owner])?;
    }
    if to == "REVOKED" || to == "EXPIRED" {
        cascade_needs_auth(db, conn, owner, aid, &format!("account {}", to.to_lowercase()))?;
    }
    Ok(())
}

/// Shortest legal hop list from `from` to `to` (excluding `from`), or None.
pub(crate) fn connection_path(from: &str, to: &str) -> Option<Vec<&'static str>> {
    if from == to {
        return Some(vec![]);
    }
    let mut prev: std::collections::HashMap<&'static str, &'static str> = std::collections::HashMap::new();
    let start = CONNECTION_STATES.iter().copied().find(|x| *x == from)?;
    let mut queue = std::collections::VecDeque::from([start]);
    while let Some(cur) = queue.pop_front() {
        for &n in connection_next(cur) {
            if n == start || prev.contains_key(n) {
                continue;
            }
            prev.insert(n, cur);
            if n == to {
                let mut path = vec![n];
                let mut at = n;
                while let Some(&p) = prev.get(at) {
                    if p == start {
                        break;
                    }
                    path.push(p);
                    at = p;
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(n);
        }
    }
    None
}

/// Accounts linked to a Subscriptions login (`externalAccountId = subsfab:<id>`):
/// (account id, state, subscription login id).
pub(crate) fn subscription_linked_accounts(conn: &Connection, owner: &str) -> rusqlite::Result<Vec<(String, String, String)>> {
    let mut st = conn.prepare(
        "SELECT id, state, substr(external_account_id, 9) FROM provider_account_bindings \
         WHERE owner = ?1 AND external_account_id LIKE 'subsfab:%' ORDER BY id",
    )?;
    let rows = st.query_map(params![owner], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

/// Owners with at least one Subscriptions-linked account (the sync's work list).
pub(crate) fn subscription_linked_owners(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT DISTINCT owner FROM provider_account_bindings WHERE external_account_id LIKE 'subsfab:%'")?;
    let rows = st.query_map([], |r| r.get(0))?;
    rows.collect()
}

/// Every dependent execution binding -> NEEDS_AUTH. Bindings whose current
/// state can't move there (already NEEDS_AUTH, UNBOUND, DISABLED, FAILED) stay.
fn cascade_needs_auth(db: &DbHandle, conn: &Connection, owner: &str, account_id: &str, cause: &str) -> rusqlite::Result<Vec<String>> {
    let deps = rows(conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE account_binding_id = ?1 AND owner = ?2"), &[&account_id, &owner])?;
    let mut moved = vec![];
    for d in deps {
        if exec_next(&s(&d, "state")).contains(&"NEEDS_AUTH") {
            set_exec_state(db, conn, owner, &d, "NEEDS_AUTH", cause)?;
            moved.push(s(&d, "botId"));
        }
    }
    Ok(moved)
}

#[derive(Deserialize)]
struct DeleteQ {
    #[serde(default)]
    force: bool,
}

async fn delete_account(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(aid): Path<String>, Query(q): Query<DeleteQ>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        let acct = get_account(&conn, &owner, &aid)?;
        let deps = rows(&conn, "SELECT bot_id, state FROM bot_execution_bindings WHERE account_binding_id = ?1 AND owner = ?2", &[&aid, &owner])?;
        if !deps.is_empty() && !q.force {
            return Err(ApiErr(
                StatusCode::CONFLICT,
                json!({ "error": "account is used by bots; pass ?force=true to disconnect them", "dependentBots": deps }),
            ));
        }
        let moved = cascade_needs_auth(db, &conn, &owner, &aid, "account deleted")?;
        conn.execute("UPDATE bot_execution_bindings SET account_binding_id = NULL WHERE account_binding_id = ?1 AND owner = ?2", params![aid, owner])?;
        conn.execute("DELETE FROM provider_account_bindings WHERE id = ?1 AND owner = ?2", params![aid, owner])?;
        audit(&conn, &owner, &aid, "account.deleted", Some(&s(&acct, "state")), None, json!({ "force": q.force, "dependentBots": moved }));
        ok(json!({ "deleted": true, "dependentBots": moved }))
    })
    .await
}

// ---------------------------------------------------------------- provider keys, discovery, bindings list

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretBody {
    api_key: String,
}

/// Seal a per-user provider key with `token_crypto` (AES-256-GCM, the same
/// mechanism `aci_credentials` uses for user-owned secrets). STRICT: with no
/// encryption key available nothing is stored, never a `plain:` fallback.
pub(crate) fn seal_strict(plain: &str) -> Option<String> {
    if !crate::token_crypto::ensure_platform_key() {
        return None;
    }
    let sealed = crate::token_crypto::seal(plain);
    sealed.starts_with("enc:v1:").then_some(sealed)
}

async fn set_secret(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(aid): Path<String>, Json(b): Json<SecretBody>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        get_account(&conn, &owner, &aid)?;
        let key = b.api_key.trim();
        if key.is_empty() {
            return Err(ApiErr::bad("apiKey is required"));
        }
        let sealed = seal_strict(key).ok_or_else(|| ApiErr::new(StatusCode::SERVICE_UNAVAILABLE, "no encryption key is configured; the key was not stored"))?;
        conn.execute("UPDATE provider_account_bindings SET secret_ref = ?1, updated_at = ?2 WHERE id = ?3 AND owner = ?4", params![sealed, now(), aid, owner])?;
        audit(&conn, &owner, &aid, "secret_set", None, None, json!({}));
        ok(json!({ "account": get_account(&conn, &owner, &aid)? }))
    })
    .await
}

async fn clear_secret(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(aid): Path<String>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        get_account(&conn, &owner, &aid)?;
        conn.execute("UPDATE provider_account_bindings SET secret_ref = NULL, updated_at = ?1 WHERE id = ?2 AND owner = ?3", params![now(), aid, owner])?;
        audit(&conn, &owner, &aid, "secret_cleared", None, None, json!({}));
        ok(json!({ "account": get_account(&conn, &owner, &aid)? }))
    })
    .await
}

/// Adapter for an account connected a particular way, when it differs from the vendor's default.
pub(crate) fn adapter_for_auth(vendor: &str, auth_type: &str) -> Option<&'static str> {
    match (vendor, auth_type) {
        ("anthropic", "browser_session") => Some("claude-subscription"),
        ("anthropic", "api_key") => Some("claude-managed-agents"),
        // ChatGPT browser sign-in is chat (dots need a plan with dots; ChatGPT.app is the dots lane).
        ("openai", "browser_session") => Some("chatgpt-subscription"),
        ("openai", "desktop_session") => Some("chatgpt-dots"),
        ("kimi", "browser_session") => Some("kimi-subscription"),
        // Google/Microsoft browser logins run on their web adapters (the
        // gemini-web / copilot-web pools the accounts sign in through).
        ("google", "browser_session") => Some("gemini-subscription"),
        ("microsoft", "browser_session") => Some("copilot-subscription"),
        ("hermes", _) => Some("hermes"),
        _ => None,
    }
}

/// `agent.list` for an account's vendor adapter, via a transient binding.
pub(crate) async fn discover_agents(db: &DbHandle, tx: &dyn crate::gateway_runner::AaiTransport, owner: &str, aid: &str) -> Result<Vec<Value>, (StatusCode, String, String)> {
    let acct = {
        let conn = db.connect().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL".to_string(), "database error".to_string()))?;
        get_account(&conn, owner, aid).map_err(|ApiErr(s, v)| (s, "NOT_FOUND".to_string(), v["error"].as_str().unwrap_or("error").to_string()))?
    };
    let mut binding = json!({ "type": "vendor", "vendor": s(&acct, "vendor"), "accountBindingId": aid });
    // The vendor id alone picks the vendor's default adapter; the way the account was connected can pick another
    // (a Claude browser login runs on claude-subscription, not the desktop app).
    if let Some(adapter) = adapter_for_auth(&s(&acct, "vendor"), &s(&acct, "authType")) {
        binding["adapterId"] = json!(adapter);
    }
    let v = crate::gateway_runner::vcall(db, tx, owner, "agent.list", &binding, json!({})).await.map_err(map_aai_err)?;
    let list = v["agents"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|a| {
            // The AAI host lists `{agentId, displayName}`; older adapters used `externalAgentId|id` and `name`.
            let ext = a["agentId"].as_str().or_else(|| a["externalAgentId"].as_str()).or_else(|| a["id"].as_str())?;
            let name = a["displayName"].as_str().or_else(|| a["name"].as_str()).unwrap_or(ext);
            let mut o = json!({ "externalAgentId": ext, "name": name });
            for field in ["kind", "kindLabel"] {
                if let Some(label) = a[field].as_str() {
                    o[field] = json!(label);
                }
            }
            if let Some(d) = a["description"].as_str() {
                o["description"] = json!(d);
            }
            if let Some(u) = a["avatarUrl"].as_str().filter(|u| valid_avatar(u)) {
                o["avatarUrl"] = json!(u);
            }
            Some(o)
        })
        .collect())
}

async fn list_agents(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(aid): Path<String>) -> Response {
    let tx = crate::gateway_runner::transport(&state);
    match discover_agents(&state.db, tx.as_ref(), &user.user_id, &aid).await {
        Ok(agents) => {
            // Discovery is the sync point for vendor identity: bots bound to one of
            // these agents pick up its current name and avatar.
            if let Ok(conn) = state.db.connect() {
                refresh_agent_identity(&conn, &user.user_id, &aid, &agents);
            }
            (StatusCode::OK, Json(json!({ "agents": agents }))).into_response()
        }
        Err((status, code, msg)) => (status, Json(json!({ "error": msg, "code": code }))).into_response(),
    }
}

/// Copy each discovered agent's name and avatar onto the bindings that use it.
pub(crate) fn refresh_agent_identity(conn: &Connection, owner: &str, aid: &str, agents: &[Value]) {
    for a in agents {
        let Some(ext) = a["externalAgentId"].as_str() else { continue };
        let _ = conn.execute(
            "UPDATE bot_execution_bindings SET external_agent_name = COALESCE(?1, external_agent_name),
                external_agent_avatar = COALESCE(?2, external_agent_avatar), updated_at = ?3
             WHERE owner = ?4 AND account_binding_id = ?5 AND external_agent_id = ?6",
            params![a["name"].as_str(), a["avatarUrl"].as_str(), now(), owner, aid, ext],
        );
    }
}

async fn list_exec(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let bindings = rows(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE owner = ?1 ORDER BY created_at"), &[&user.user_id])?;
        ok(json!({ "bindings": bindings }))
    })
    .await
}

// ---------------------------------------------------------------- execution bindings

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutExec {
    r#type: Option<String>,
    mode: Option<String>,
    vendor: Option<String>,
    adapter_id: Option<String>,
    account_binding_id: Option<String>,
    preferred_lane: Option<String>,
    external_agent_id: Option<String>,
    external_agent_name: Option<String>,
    external_agent_avatar: Option<String>,
    /// The bot that directs this vendor bot (set on deploy).
    directing_bot_id: Option<String>,
    capabilities: Option<Value>,
    health: Option<Value>,
}

/// Largest inline avatar we keep (the data URI string, not the decoded bytes).
const MAX_AVATAR_LEN: usize = 256 * 1024;

/// A vendor agent avatar we store and hand to the browser: an https URL, or an
/// inline raster data URI. Never fetched server-side (no SSRF surface); never SVG
/// (scriptable).
pub(crate) fn valid_avatar(v: &str) -> bool {
    if v.len() > MAX_AVATAR_LEN {
        return false;
    }
    if let Some(rest) = v.strip_prefix("https://") {
        return !rest.is_empty() && !v.chars().any(|c| c.is_whitespace() || c == '"' || c == '<' || c == '>');
    }
    ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/webp;base64,", "data:image/gif;base64,"]
        .iter()
        .any(|p| v.starts_with(p) && v[p.len()..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='))
}

async fn put_exec(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>, Json(b): Json<PutExec>) -> Response {
    if !verify_bot_ownership(&state, &user.user_id, &bot_id).await {
        return err_resp(StatusCode::FORBIDDEN, "bot not found or access denied");
    }
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        let ty = b.r#type.clone().unwrap_or_else(|| "vendor".into());
        let mode = b.mode.clone().unwrap_or_else(|| "hosted".into());
        if !["allternit", "vendor"].contains(&ty.as_str()) || !["native", "hosted", "linked", "mirror"].contains(&mode.as_str()) {
            return Err(ApiErr::bad("type must be allternit|vendor and mode native|hosted|linked|mirror"));
        }
        require_account(&conn, &owner, &b.account_binding_id)?;
        // An account restricted to one bot can't back any other bot.
        if let Some(aid) = &b.account_binding_id {
            let restricted: Option<String> = conn
                .query_row("SELECT restricted_bot_id FROM provider_account_bindings WHERE id = ?1 AND owner = ?2", params![aid, owner], |r| r.get(0))
                .optional()?
                .flatten();
            if let Some(rb) = restricted {
                if rb != bot_id {
                    return Err(ApiErr::bad("accountBindingId is restricted to another bot"));
                }
            }
        }
        let caps = b.capabilities.as_ref().map(|v| v.to_string());
        let health = b.health.as_ref().map(|v| v.to_string());
        if let Some(a) = b.external_agent_avatar.as_deref() {
            if !a.is_empty() && !valid_avatar(a) {
                return Err(ApiErr::bad("externalAgentAvatar must be an https URL or a png/jpeg/webp/gif data URI under 256 KB"));
            }
        }
        let agent_name = b.external_agent_name.as_deref().map(|n| n.chars().take(120).collect::<String>());
        let agent_avatar = b.external_agent_avatar.clone().filter(|a| !a.is_empty());
        let existing = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2"), &[&bot_id, &owner])?;
        let (status, bid) = match existing {
            Some(e) => {
                // Rebind: config changes, bot_id and state stay.
                conn.execute(
                    "UPDATE bot_execution_bindings SET type = ?1, mode = ?2, vendor = ?3, adapter_id = ?4, account_binding_id = ?5,
                        preferred_lane = ?6, external_agent_id = ?7, capabilities_json = COALESCE(?8, capabilities_json),
                        health_json = COALESCE(?9, health_json), updated_at = ?10,
                        external_agent_name = ?12, external_agent_avatar = ?13 WHERE id = ?11",
                    params![ty, mode, b.vendor, b.adapter_id, b.account_binding_id, b.preferred_lane, b.external_agent_id, caps, health, now(), s(&e, "id"), agent_name, agent_avatar],
                )?;
                (StatusCode::OK, s(&e, "id"))
            }
            None => {
                let bid = id("xb");
                conn.execute(
                    "INSERT INTO bot_execution_bindings (id, owner, bot_id, type, mode, vendor, adapter_id, account_binding_id,
                        preferred_lane, external_agent_id, capabilities_json, health_json, state, created_at, updated_at,
                        external_agent_name, external_agent_avatar)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'UNBOUND',?13,?13,?14,?15)",
                    params![bid, owner, bot_id, ty, mode, b.vendor, b.adapter_id, b.account_binding_id, b.preferred_lane,
                            b.external_agent_id, caps.unwrap_or_else(|| "{}".into()), health.unwrap_or_else(|| "{}".into()), now(),
                            agent_name, agent_avatar],
                )?;
                let row = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE id = ?1"), &[&bid])?.unwrap();
                set_exec_state(db, &conn, &owner, &row, "BOUND", "binding created")?;
                (StatusCode::CREATED, bid)
            }
        };
        if let Some(d) = b.directing_bot_id.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            let mine: i64 = conn.query_row("SELECT COUNT(*) FROM agents WHERE id = ?1 AND user_id = ?2 AND id <> ?3", params![d, owner, bot_id], |r| r.get(0))?;
            if mine == 0 {
                return Err(ApiErr::bad("directingBotId must be one of your own bots, other than this vendor bot"));
            }
            conn.execute("UPDATE bot_execution_bindings SET directing_bot_id = ?1 WHERE id = ?2", params![d, bid])?;
            // The connector reads the same relation (whose phone and mailbox the vendor bot uses).
            conn.execute(
                "INSERT INTO vendor_bot_connectors (vendor_bot_id, owner, directing_bot_id, created_at, updated_at) VALUES (?1,?2,?3,?4,?4)
                 ON CONFLICT(vendor_bot_id) DO UPDATE SET directing_bot_id = excluded.directing_bot_id, updated_at = excluded.updated_at WHERE owner = excluded.owner",
                params![bot_id, owner, d, now()],
            )?;
        }
        let row = one(&conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE id = ?1"), &[&bid])?;
        Ok((status, json!({ "binding": row })))
    })
    .await
}

fn load_exec(conn: &Connection, owner: &str, bot_id: &str) -> Api<Value> {
    one(conn, &format!("SELECT {EXEC_COLS} FROM bot_execution_bindings WHERE bot_id = ?1 AND owner = ?2"), &[&bot_id, &owner])?
        .ok_or_else(|| ApiErr::nf("execution binding not found"))
}

async fn get_exec(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        ok(json!({ "binding": load_exec(&conn, &user.user_id, &bot_id)? }))
    })
    .await
}

#[derive(Deserialize)]
struct PatchExec {
    state: Option<String>,
    health: Option<Value>,
    reason: Option<String>,
}

async fn patch_exec(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>, Json(b): Json<PatchExec>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        let e = load_exec(&conn, &owner, &bot_id)?;
        if let Some(h) = &b.health {
            conn.execute("UPDATE bot_execution_bindings SET health_json = ?1, updated_at = ?2 WHERE id = ?3", params![h.to_string(), now(), s(&e, "id")])?;
        }
        if let Some(to) = &b.state {
            if check_transition("execution binding", EXEC_STATES, exec_next, &s(&e, "state"), to)? {
                set_exec_state(db, &conn, &owner, &e, to, b.reason.as_deref().unwrap_or("manual"))?;
            }
        }
        ok(json!({ "binding": load_exec(&conn, &owner, &bot_id)? }))
    })
    .await
}

// ---------------------------------------------------------------- remote thread bindings

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRemote {
    generation: i64,
    execution_binding_id: Option<String>,
    external_context_id: Option<String>,
    external_task_id: Option<String>,
    lane: Option<String>,
    capability_snapshot: Option<Value>,
}

async fn create_remote(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(thread_id): Path<String>, Json(b): Json<CreateRemote>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        let bot_id = thread_bot(&conn, &owner, &thread_id)?;
        if b.generation < 1 {
            return Err(ApiErr::bad("generation must be >= 1"));
        }
        if let Some(x) = &b.execution_binding_id {
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM bot_execution_bindings WHERE id = ?1 AND owner = ?2", params![x, owner], |r| r.get(0))?;
            if n == 0 {
                return Err(ApiErr::bad("executionBindingId not found"));
            }
        }
        let dup: i64 = conn.query_row("SELECT COUNT(*) FROM remote_thread_bindings WHERE thread_id = ?1 AND generation = ?2", params![thread_id, b.generation], |r| r.get(0))?;
        if dup > 0 {
            return Err(ApiErr::new(StatusCode::CONFLICT, "a binding for this thread generation already exists"));
        }
        let rid = id("rtb");
        let snap = b.capability_snapshot.unwrap_or_else(|| json!({})).to_string();
        conn.execute(
            "INSERT INTO remote_thread_bindings (id, owner, thread_id, generation, bot_id, execution_binding_id, external_context_id,
                external_task_id, capability_snapshot, lane, state, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'UNBOUND',?11,?11)",
            params![rid, owner, thread_id, b.generation, bot_id, b.execution_binding_id, b.external_context_id, b.external_task_id, snap, b.lane, now()],
        )?;
        let row = one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1"), &[&rid])?;
        Ok((StatusCode::CREATED, json!({ "binding": row })))
    })
    .await
}

async fn list_remote(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(thread_id): Path<String>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        thread_bot(&conn, &user.user_id, &thread_id)?;
        let v = rows(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE thread_id = ?1 AND owner = ?2 ORDER BY generation"), &[&thread_id, &user.user_id])?;
        ok(json!({ "bindings": v }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchRemote {
    state: Option<String>,
    sync_cursor: Option<String>,
    last_remote_event_id: Option<String>,
    external_context_id: Option<String>,
    external_task_id: Option<String>,
    continuation_token: Option<String>,
    // Immutable after create: presence is rejected.
    lane: Option<Value>,
    capability_snapshot: Option<Value>,
}

async fn patch_remote(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(rid): Path<String>, Json(b): Json<PatchRemote>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        let cur = one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1 AND owner = ?2"), &[&rid, &owner])?
            .ok_or_else(|| ApiErr::nf("remote binding not found"))?;
        if b.lane.is_some() || b.capability_snapshot.is_some() {
            return Err(ApiErr::new(StatusCode::CONFLICT, "lane and capabilitySnapshot are frozen at create and cannot be changed"));
        }
        let from = s(&cur, "state");
        let moved = match &b.state {
            Some(to) => check_transition("remote thread", REMOTE_STATES, remote_next, &from, to)?,
            None => false,
        };
        let t = now();
        conn.execute(
            "UPDATE remote_thread_bindings SET sync_cursor = COALESCE(?1, sync_cursor), last_remote_event_id = COALESCE(?2, last_remote_event_id),
                external_context_id = COALESCE(?3, external_context_id), external_task_id = COALESCE(?4, external_task_id),
                continuation_token = COALESCE(?5, continuation_token), updated_at = ?6 WHERE id = ?7",
            params![b.sync_cursor, b.last_remote_event_id, b.external_context_id, b.external_task_id, b.continuation_token, t, rid],
        )?;
        if moved {
            let to = b.state.as_deref().unwrap();
            conn.execute(
                "UPDATE remote_thread_bindings SET state = ?1, closed_at = CASE WHEN ?1 = 'CLOSED' THEN ?2 ELSE closed_at END WHERE id = ?3",
                params![to, t, rid],
            )?;
            let kind = match to {
                "ACTIVE" if from == "OPENING" => Some("gateway.remote_thread.opened"),
                "CLOSED" => Some("gateway.remote_thread.closed"),
                _ => None,
            };
            if let Some(k) = kind {
                ledger(
                    db,
                    &owner,
                    &s(&cur, "botId"),
                    Some(&s(&cur, "threadId")),
                    k,
                    json!({ "bindingId": rid, "threadId": s(&cur, "threadId"), "generation": cur["generation"], "lane": cur["lane"], "from": from, "to": to }),
                );
            }
        }
        let row = one(&conn, &format!("SELECT {REMOTE_COLS} FROM remote_thread_bindings WHERE id = ?1"), &[&rid])?;
        ok(json!({ "binding": row }))
    })
    .await
}

// ---------------------------------------------------------------- channel conversation bindings

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateChannel {
    provider: String,
    account_binding_id: Option<String>,
    external_workspace_id: Option<String>,
    external_channel_id: Option<String>,
    external_conversation_id: String,
    external_thread_id: Option<String>,
    canonical_url: Option<String>,
    bidirectional: Option<bool>,
    read_only: Option<bool>,
    posting_identity_id: Option<String>,
    channel_name: Option<String>,
    workspace_name: Option<String>,
}

async fn create_channel(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(thread_id): Path<String>, Json(b): Json<CreateChannel>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        thread_bot(&conn, &owner, &thread_id)?;
        require_account(&conn, &owner, &b.account_binding_id)?;
        if b.provider.trim().is_empty() || b.external_conversation_id.trim().is_empty() {
            return Err(ApiErr::bad("provider and externalConversationId are required"));
        }
        let cid = id("chb");
        conn.execute(
            "INSERT INTO channel_conversation_bindings (id, owner, thread_id, provider, account_binding_id, external_workspace_id,
                external_channel_id, external_conversation_id, external_thread_id, canonical_url, bidirectional, read_only,
                posting_identity_id, channel_name, workspace_name, sync_state, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?15,?16,'LIVE',?14,?14)",
            params![cid, owner, thread_id, b.provider, b.account_binding_id, b.external_workspace_id, b.external_channel_id,
                    b.external_conversation_id, b.external_thread_id, b.canonical_url, b.bidirectional.unwrap_or(true) as i64,
                    b.read_only.unwrap_or(false) as i64, b.posting_identity_id, now(), b.channel_name, b.workspace_name],
        )?;
        let row = one(&conn, &format!("SELECT {CHAN_COLS} FROM channel_conversation_bindings WHERE id = ?1"), &[&cid])?;
        Ok((StatusCode::CREATED, json!({ "binding": row })))
    })
    .await
}

async fn list_channel(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(thread_id): Path<String>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        thread_bot(&conn, &user.user_id, &thread_id)?;
        let v = rows(&conn, &format!("SELECT {CHAN_COLS} FROM channel_conversation_bindings WHERE thread_id = ?1 AND owner = ?2 ORDER BY created_at"), &[&thread_id, &user.user_id])?;
        ok(json!({ "bindings": v }))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchChannel {
    sync_state: Option<String>,
    last_inbound_cursor: Option<String>,
    last_outbound_cursor: Option<String>,
    read_only: Option<bool>,
    channel_name: Option<String>,
    workspace_name: Option<String>,
}

async fn patch_channel(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(cid): Path<String>, Json(b): Json<PatchChannel>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        one(&conn, "SELECT id FROM channel_conversation_bindings WHERE id = ?1 AND owner = ?2", &[&cid, &owner])?
            .ok_or_else(|| ApiErr::nf("channel binding not found"))?;
        if let Some(st) = &b.sync_state {
            if !CHANNEL_SYNC_STATES.contains(&st.as_str()) {
                return Err(ApiErr::bad(format!("syncState must be one of {CHANNEL_SYNC_STATES:?}")));
            }
        }
        conn.execute(
            "UPDATE channel_conversation_bindings SET sync_state = COALESCE(?1, sync_state),
                last_inbound_cursor = COALESCE(?2, last_inbound_cursor), last_outbound_cursor = COALESCE(?3, last_outbound_cursor),
                read_only = COALESCE(?4, read_only), channel_name = COALESCE(?7, channel_name),
                workspace_name = COALESCE(?8, workspace_name), updated_at = ?5 WHERE id = ?6",
            params![b.sync_state, b.last_inbound_cursor, b.last_outbound_cursor, b.read_only.map(|x| x as i64), now(), cid, b.channel_name, b.workspace_name],
        )?;
        let row = one(&conn, &format!("SELECT {CHAN_COLS} FROM channel_conversation_bindings WHERE id = ?1"), &[&cid])?;
        ok(json!({ "binding": row }))
    })
    .await
}

// ---------------------------------------------------------------- vendor pack gaps

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordGap {
    capability: String,
    surface: String,
    severity: Option<String>,
    fallback_used: Option<bool>,
    sample_ref: Option<String>,
}

const SURFACES: &[&str] = &["transcript", "composer", "activity", "card", "computer", "approval"];

async fn record_gap(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(vendor): Path<String>, Json(b): Json<RecordGap>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let owner = user.user_id;
        let sev = b.severity.clone().unwrap_or_else(|| "visual_parity".into());
        if !GAP_SEVERITIES.contains(&sev.as_str()) {
            return Err(ApiErr::bad(format!("severity must be one of {GAP_SEVERITIES:?}")));
        }
        let surface_kind = b.surface.split('.').next().unwrap_or_default();
        if b.capability.trim().is_empty() || !SURFACES.contains(&surface_kind) {
            return Err(ApiErr::bad(format!("capability required; surface must start with one of {SURFACES:?}")));
        }
        let t = now();
        let existing = one(&conn, &format!("SELECT {GAP_COLS} FROM vendor_pack_gaps WHERE owner = ?1 AND vendor = ?2 AND capability = ?3 AND surface = ?4"), &[&owner, &vendor, &b.capability, &b.surface])?;
        let (status, gid) = match existing {
            Some(g) => {
                // A resolved gap that shows up again is open again; wontfix stays.
                conn.execute(
                    "UPDATE vendor_pack_gaps SET occurrences = occurrences + 1, last_seen_at = ?1,
                        status = CASE WHEN status = 'resolved' THEN 'open' ELSE status END,
                        severity = ?2, sample_ref = COALESCE(?3, sample_ref) WHERE id = ?4",
                    params![t, sev, b.sample_ref, s(&g, "id")],
                )?;
                (StatusCode::OK, s(&g, "id"))
            }
            None => {
                let gid = id("gap");
                conn.execute(
                    "INSERT INTO vendor_pack_gaps (id, owner, vendor, capability, surface, fallback_used, severity, status,
                        first_seen_at, last_seen_at, occurrences, sample_ref)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,'open',?8,?8,1,?9)",
                    params![gid, owner, vendor, b.capability, b.surface, b.fallback_used.unwrap_or(true) as i64, sev, t, b.sample_ref],
                )?;
                (StatusCode::CREATED, gid)
            }
        };
        let row = one(&conn, &format!("SELECT {GAP_COLS} FROM vendor_pack_gaps WHERE id = ?1"), &[&gid])?;
        Ok((status, json!({ "gap": row })))
    })
    .await
}

#[derive(Deserialize)]
struct GapFilter {
    status: Option<String>,
}

async fn list_gaps(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(vendor): Path<String>, Query(q): Query<GapFilter>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let v = rows(
            &conn,
            &format!("SELECT {GAP_COLS} FROM vendor_pack_gaps WHERE owner = ?1 AND vendor = ?2 AND (?3 IS NULL OR status = ?3) ORDER BY last_seen_at DESC"),
            &[&user.user_id, &vendor, &q.status],
        )?;
        ok(json!({ "gaps": v }))
    })
    .await
}

#[derive(Deserialize)]
struct PatchGap {
    status: Option<String>,
    severity: Option<String>,
}

async fn patch_gap(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(gid): Path<String>, Json(b): Json<PatchGap>) -> Response {
    run(&state, move |db| {
        if b.status.is_none() && b.severity.is_none() {
            return Err(ApiErr::bad("nothing to update: send status and/or severity"));
        }
        if let Some(st) = &b.status {
            if !GAP_STATUSES.contains(&st.as_str()) {
                return Err(ApiErr::bad(format!("status must be one of {GAP_STATUSES:?}")));
            }
        }
        if let Some(sev) = &b.severity {
            if !GAP_SEVERITIES.contains(&sev.as_str()) {
                return Err(ApiErr::bad(format!("severity must be one of {GAP_SEVERITIES:?}")));
            }
        }
        let conn = db.connect()?;
        let n = conn.execute(
            "UPDATE vendor_pack_gaps SET status = COALESCE(?1, status), severity = COALESCE(?2, severity) WHERE id = ?3 AND owner = ?4",
            params![b.status, b.severity, gid, user.user_id],
        )?;
        if n == 0 {
            return Err(ApiErr::nf("gap not found"));
        }
        ok(json!({ "gap": one(&conn, &format!("SELECT {GAP_COLS} FROM vendor_pack_gaps WHERE id = ?1"), &[&gid])? }))
    })
    .await
}

async fn parity(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(vendor): Path<String>) -> Response {
    run(&state, move |db| {
        let conn = db.connect()?;
        let gaps = rows(&conn, "SELECT severity, status FROM vendor_pack_gaps WHERE owner = ?1 AND vendor = ?2", &[&user.user_id, &vendor])?;
        let pairs: Vec<(String, String)> = gaps.iter().map(|g| (s(g, "severity"), s(g, "status"))).collect();
        let open = pairs.iter().filter(|(_, st)| st == "open").count();
        let blocking = pairs.iter().filter(|(sev, st)| st == "open" && (sev == "data_loss" || sev == "functional")).count();
        ok(json!({ "vendor": vendor, "parity": parity_of(&pairs), "openGaps": open, "blockingGaps": blocking }))
    })
    .await
}

// ---------------------------------------------------------------- vendor memory
// Vendor memory is a separate partition (authority "vendor"): it is read through
// `agent.memory`, never merged into Allternit memory silently, and only an
// explicit human promote copies a record into a native note (`memory.promoted`).

type GwErr = (StatusCode, String, String);

fn map_aai_err(e: crate::gateway_runner::AaiError) -> GwErr {
    // `aaiErrorCodeSchema` (subscription-fabric-contracts agent.ts), plus the local
    // transport's own INTERNAL. Anything else is an unknown vendor failure.
    let status = match e.code.as_str() {
        "AUTH_REQUIRED" | "AUTH_REVOKED" => StatusCode::UNAUTHORIZED,
        "RATE_LIMITED" => StatusCode::TOO_MANY_REQUESTS,
        "LANE_BLOCKED" | "POLICY_DENIED" => StatusCode::FORBIDDEN,
        "CONTEXT_NOT_FOUND" => StatusCode::NOT_FOUND,
        "CONTEXT_BUSY" | "APPROVAL_REQUIRED" | "SYNC_CONFLICT" => StatusCode::CONFLICT,
        "UNSUPPORTED" => StatusCode::NOT_IMPLEMENTED,
        "VENDOR_UNAVAILABLE" | "ADAPTER_DRIFT" | "GATEWAY_OFFLINE" => StatusCode::SERVICE_UNAVAILABLE,
        "INTERNAL" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_GATEWAY,
    };
    (status, e.code, e.human_message)
}

/// The vendor-memory view for a vendor-backed bot. 404 for native or unknown bots.
pub(crate) async fn read_vendor_memory(db: &DbHandle, tx: &dyn crate::gateway_runner::AaiTransport, owner: &str, bot_id: &str) -> Result<Value, GwErr> {
    let nf = || (StatusCode::NOT_FOUND, "NOT_FOUND".to_string(), "vendor-backed bot not found".to_string());
    let bx = {
        let conn = db.connect().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL".to_string(), "database error".to_string()))?;
        load_exec(&conn, owner, bot_id).map_err(|_| nf())?
    };
    if s(&bx, "type") != "vendor" {
        return Err(nf());
    }
    let vendor = s(&bx, "vendor");
    let caps = bx["capabilities"].clone();
    if caps.pointer("/memory/opaque").and_then(Value::as_bool).unwrap_or(false) || crate::gateway_placement::has_capability(&caps, "memory.opaque") {
        return Ok(json!({ "vendor": vendor, "authority": "vendor", "observability": "opaque", "promotable": false,
            "reason": "this vendor keeps memory it does not let Allternit read" }));
    }
    let binding = json!({ "type": "vendor", "vendor": vendor, "accountBindingId": bx["accountBindingId"], "externalAgentId": bx["externalAgentId"] });
    let v = match crate::gateway_runner::vcall(db, tx, owner, "agent.memory", &binding, json!({ "op": "read" })).await {
        Ok(v) => v,
        Err(e) if e.code == "UNSUPPORTED" => {
            return Ok(json!({ "vendor": vendor, "authority": "vendor", "observability": "unavailable", "promotable": false, "reason": e.human_message }));
        }
        Err(e) => return Err(map_aai_err(e)),
    };
    if v["opaque"].as_bool().unwrap_or(false) {
        return Ok(json!({ "vendor": vendor, "authority": "vendor", "observability": "opaque", "promotable": false,
            "reason": "this vendor keeps memory it does not let Allternit read" }));
    }
    let list = v["records"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default();
    let records: Vec<Value> = list
        .iter()
        .filter_map(|r| {
            let text = r["text"].as_str().or_else(|| r["content"].as_str())?;
            let rid = r["id"].as_str().or_else(|| r["remoteRef"].as_str())?;
            let mut o = json!({ "id": rid, "scope": r["scope"].as_str().unwrap_or("bot"), "text": text });
            if let Some(x) = r["remoteRef"].as_str() {
                o["remoteRef"] = json!(x);
            }
            if let Some(x) = r["updatedAt"].as_str() {
                o["updatedAt"] = json!(x);
            }
            Some(o)
        })
        .collect();
    Ok(json!({ "vendor": vendor, "authority": "vendor", "observability": "readable", "records": records, "promotable": true }))
}

/// Explicit, human-driven copy of one vendor record into a native note.
pub(crate) async fn promote_vendor_memory(db: &DbHandle, tx: &dyn crate::gateway_runner::AaiTransport, owner: &str, bot_id: &str, record_id: &str, scope: &str) -> Result<Value, GwErr> {
    if !["bot", "project", "thread"].contains(&scope) {
        return Err((StatusCode::BAD_REQUEST, "BAD_REQUEST".into(), "scope must be bot|project|thread".into()));
    }
    let view = read_vendor_memory(db, tx, owner, bot_id).await?;
    if view["observability"] != "readable" {
        return Err((StatusCode::CONFLICT, "NOT_PROMOTABLE".into(), view["reason"].as_str().unwrap_or("vendor memory is not readable").to_string()));
    }
    let rec = view["records"].as_array().and_then(|a| a.iter().find(|r| r["id"] == record_id).cloned())
        .ok_or((StatusCode::NOT_FOUND, "NOT_FOUND".to_string(), "vendor memory record not found".to_string()))?;
    let vendor = s(&view, "vendor");
    let remote_ref = rec["remoteRef"].as_str().unwrap_or(record_id).to_string();
    let text = s(&rec, "text");
    let conn = db.connect().map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL".to_string(), "database error".to_string()))?;
    let ref_tag = format!("remoteRef:{remote_ref}");
    let tags = json!(["source:vendor", format!("vendor:{vendor}"), ref_tag, format!("bot:{bot_id}"), format!("scope:{scope}")]);
    // Promoting the same record twice never duplicates it.
    let dup: Option<String> = conn
        .query_row(
            "SELECT id FROM memory_notes WHERE user_id = ?1 AND tags LIKE ?2 AND tags LIKE ?3",
            params![owner, format!("%\"{ref_tag}\"%"), format!("%\"bot:{bot_id}\"%")],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL".to_string(), e.to_string()))?;
    if let Some(nid) = dup {
        return Ok(json!({ "noteId": nid, "promoted": false, "alreadyPromoted": true }));
    }
    let nid = format!("mn_{}", uuid::Uuid::new_v4().simple());
    let title: String = text.chars().take(80).collect();
    conn.execute(
        "INSERT INTO memory_notes (id, user_id, note_type, title, content, tags, entity_id) VALUES (?1, ?2, 'general', ?3, ?4, ?5, NULL)",
        params![nid, owner, title, text, tags.to_string()],
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL".to_string(), e.to_string()))?;
    ledger(db, owner, bot_id, None, "memory.promoted", json!({
        "source": "vendor", "vendor": vendor, "remoteRef": remote_ref, "recordId": record_id, "scope": scope, "noteId": nid, "authority": "allternit", "actor": "human" }));
    Ok(json!({ "noteId": nid, "promoted": true }))
}

fn gw_err(e: GwErr) -> Response {
    (e.0, Json(json!({ "error": e.2, "code": e.1 }))).into_response()
}

async fn vendor_memory_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path(bot_id): Path<String>) -> Response {
    let tx = crate::gateway_runner::transport(&state);
    match read_vendor_memory(&state.db, tx.as_ref(), &user.user_id, &bot_id).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => gw_err(e),
    }
}

#[derive(Deserialize)]
struct PromoteBody {
    scope: String,
}

async fn promote_memory_h(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Path((bot_id, record_id)): Path<(String, String)>, Json(b): Json<PromoteBody>) -> Response {
    let tx = crate::gateway_runner::transport(&state);
    match promote_vendor_memory(&state.db, tx.as_ref(), &user.user_id, &bot_id, &record_id, &b.scope).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(e) => gw_err(e),
    }
}

fn err_resp(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-gw-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = crate::test_helpers::app_state(&dir).await;
        let conn = state.db.connect().unwrap();
        for b in ["bot-1", "bot-2"] {
            conn.execute(
                "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1, 'user-a', 'b', 'm', 'p', 1, '{}')",
                params![b],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, title, last_activity_at, created_at, updated_at) VALUES ('th-1', 'user-a', 'bot-1', 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        state
    }

    async fn call(state: &Arc<AppState>, method: &str, uri: &str, u: &str, b: Option<Value>) -> (StatusCode, Value) {
        let app = agent_gateway_router().with_state(state.clone());
        let mut req = Request::builder().method(method).uri(format!("/gateway{uri}")).extension(user(u));
        let body = match b {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = app.oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn account(st: &Arc<AppState>) -> String {
        let (s, v) = call(st, "POST", "/provider-accounts", "user-a", Some(json!({"vendor": "openai", "authType": "browser_session", "sessionRef": "vault://x"}))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert_eq!(v["account"]["hasSessionRef"], true);
        assert!(v["account"].get("sessionRef").is_none(), "refs are never echoed");
        v["account"]["id"].as_str().unwrap().to_string()
    }

    async fn connect(st: &Arc<AppState>, aid: &str) {
        for to in ["CONSENT_REQUIRED", "AUTHENTICATING", "VERIFYING", "CONNECTED"] {
            let (s, v) = call(st, "PATCH", &format!("/provider-accounts/{aid}"), "user-a", Some(json!({"state": to}))).await;
            assert_eq!(s, StatusCode::OK, "{to}: {v}");
        }
    }

    async fn bind(st: &Arc<AppState>, bot: &str, aid: &str) -> Value {
        let (s, v) = call(st, "PUT", &format!("/bots/{bot}/execution-binding"), "user-a", Some(json!({"vendor": "openai", "accountBindingId": aid}))).await;
        assert!(s == StatusCode::CREATED || s == StatusCode::OK, "{v}");
        v["binding"].clone()
    }

    #[test]
    fn connection_machine() {
        assert_eq!(connection_next("DISCONNECTED"), &["CONSENT_REQUIRED"]);
        assert!(connection_next("CONNECTED").contains(&"REVOKED"));
        assert!(!connection_next("DISCONNECTED").contains(&"CONNECTED"));
        assert!(connection_next("REVOKED").contains(&"CONSENT_REQUIRED"));
        assert!(!connection_next("REVOKED").contains(&"AUTHENTICATING"));
        assert!(check_transition("connection", CONNECTION_STATES, connection_next, "DISCONNECTED", "CONNECTED").is_err());
        assert_eq!(check_transition("connection", CONNECTION_STATES, connection_next, "CONNECTED", "CONNECTED").unwrap(), false);
        assert!(check_transition("connection", CONNECTION_STATES, connection_next, "CONNECTED", "NOPE").is_err());
        for st in CONNECTION_STATES {
            assert!(!connection_next(st).contains(st));
        }
    }

    #[test]
    fn exec_and_remote_machines() {
        assert!(exec_next("BOUND").contains(&"READY"));
        assert!(!exec_next("UNBOUND").contains(&"READY"));
        assert!(!exec_next("BOUND").contains(&"DEGRADED"));
        assert!(exec_next("READY").contains(&"NEEDS_AUTH"));
        assert!(exec_next("FAILED").contains(&"BOUND"));
        assert_eq!(remote_next("UNBOUND"), &["OPENING"]);
        assert!(remote_next("ACTIVE").contains(&"HANDOFF_PENDING"));
        assert!(remote_next("CLOSED").is_empty());
        for st in EXEC_STATES {
            assert!(exec_next(st).iter().all(|n| EXEC_STATES.contains(n)));
        }
    }

    #[test]
    fn parity_rules() {
        let g = |sev: &str, st: &str| (sev.to_string(), st.to_string());
        assert_eq!(parity_of(&[]), "full");
        assert_eq!(parity_of(&[g("data_loss", "resolved"), g("visual_parity", "wontfix")]), "full");
        assert_eq!(parity_of(&[g("visual_parity", "open"), g("visual_parity", "open")]), "partial");
        assert_eq!(parity_of(&[g("visual_parity", "open"), g("functional", "open")]), "blocked");
        assert_eq!(parity_of(&[g("visual_parity", "open"), g("data_loss", "open")]), "blocked");
    }

    #[tokio::test]
    async fn invalid_transition_is_409_with_allowed_list() {
        let st = setup("inv").await;
        let aid = account(&st).await;
        let (s, v) = call(&st, "PATCH", &format!("/provider-accounts/{aid}"), "user-a", Some(json!({"state": "CONNECTED"}))).await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["allowed"], json!(["CONSENT_REQUIRED"]));
        let (s, _) = call(&st, "GET", &format!("/provider-accounts/{aid}"), "user-b", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "owner-scoped");
        connect(&st, &aid).await;
        let (_, v) = call(&st, "GET", &format!("/provider-accounts/{aid}"), "user-a", None).await;
        assert!(v["account"]["verifiedAt"].is_string());
    }

    #[tokio::test]
    async fn revocation_cascades_to_needs_auth_and_audits() {
        let st = setup("rev").await;
        let aid = account(&st).await;
        connect(&st, &aid).await;
        let b1 = bind(&st, "bot-1", &aid).await;
        bind(&st, "bot-2", &aid).await;
        assert_eq!(b1["state"], "BOUND");
        let (s, _) = call(&st, "PATCH", "/bots/bot-1/execution-binding", "user-a", Some(json!({"state": "READY"}))).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = call(&st, "PATCH", &format!("/provider-accounts/{aid}"), "user-a", Some(json!({"state": "REVOKED"}))).await;
        assert_eq!(s, StatusCode::OK);
        for bot in ["bot-1", "bot-2"] {
            let (_, v) = call(&st, "GET", &format!("/bots/{bot}/execution-binding"), "user-a", None).await;
            assert_eq!(v["binding"]["state"], "NEEDS_AUTH", "{bot}");
            assert_eq!(v["binding"]["botId"], bot, "bot id never changes");
        }
        let conn = st.db.connect().unwrap();
        let audits: i64 = conn.query_row("SELECT COUNT(*) FROM connection_audit WHERE account_binding_id = ?1", params![aid], |r| r.get(0)).unwrap();
        assert_eq!(audits, 6, "created + 5 state changes");
        let evs: i64 = conn.query_row("SELECT COUNT(*) FROM bot_events WHERE event_type = 'gateway.execution_binding.state_changed' AND bot_id = 'bot-1'", [], |r| r.get(0)).unwrap();
        assert_eq!(evs, 3, "bound, ready, needs_auth");
    }

    #[tokio::test]
    async fn subscription_sync_follows_the_login_and_moves_bots() {
        use crate::subscription_sync::apply_snapshot;
        use std::collections::HashMap;
        let st = setup("subsync").await;
        let aid = account(&st).await;
        connect(&st, &aid).await;
        let (s, _) = call(&st, "PATCH", &format!("/provider-accounts/{aid}"), "user-a", Some(json!({"externalAccountId": "subsfab:login-1"}))).await;
        assert_eq!(s, StatusCode::OK);
        bind(&st, "bot-1", &aid).await;
        let snap = |h: Option<&str>| h.map(|h| HashMap::from([("login-1".to_string(), h.to_string())])).unwrap_or_default();
        let state_of = |st: &Arc<AppState>| st.db.connect().unwrap().query_row("SELECT state FROM provider_account_bindings WHERE id = ?1", params![aid], |r| r.get::<_, String>(0)).unwrap();

        // Healthy: nothing to do.
        assert!(apply_snapshot(&st.db, "user-a", &snap(Some("ready"))).unwrap().is_empty());
        // Signed out on the Sessions computer: EXPIRED, bot needs auth.
        apply_snapshot(&st.db, "user-a", &snap(Some("auth_required"))).unwrap();
        assert_eq!(state_of(&st), "EXPIRED");
        let (_, v) = call(&st, "GET", "/bots/bot-1/execution-binding", "user-a", None).await;
        assert_eq!(v["binding"]["state"], "NEEDS_AUTH");
        // Signed in again: back to CONNECTED through legal hops.
        let moved = apply_snapshot(&st.db, "user-a", &snap(Some("ready"))).unwrap();
        assert_eq!(moved, vec![(aid.clone(), "EXPIRED".to_string(), "CONNECTED".to_string())]);
        assert_eq!(state_of(&st), "CONNECTED");
        // A verification check blocks; a removed login revokes (from CONNECTED only: BLOCKED can't be revoked directly, so it stays).
        apply_snapshot(&st.db, "user-a", &snap(Some("challenge_presented"))).unwrap();
        assert_eq!(state_of(&st), "BLOCKED");
        apply_snapshot(&st.db, "user-a", &snap(Some("ready"))).unwrap();
        apply_snapshot(&st.db, "user-a", &snap(None)).unwrap();
        assert_eq!(state_of(&st), "REVOKED");
        // Revoked is the person's to redo: the sync never re-consents.
        assert!(apply_snapshot(&st.db, "user-a", &snap(Some("ready"))).unwrap().is_empty());
        assert_eq!(state_of(&st), "REVOKED");
        let conn = st.db.connect().unwrap();
        let synced: i64 = conn.query_row("SELECT COUNT(*) FROM connection_audit WHERE account_binding_id = ?1 AND detail_json LIKE '%subscription sync%'", params![aid], |r| r.get(0)).unwrap();
        assert!(synced >= 5, "every sync hop is audited ({synced})");
    }

    #[tokio::test]
    async fn restricted_account_backs_only_its_bot() {
        let st = setup("restr").await;
        let (s, v) = call(&st, "POST", "/provider-accounts", "user-a", Some(json!({"vendor": "openai", "authType": "api_key", "secretRef": "vault://k", "restrictedBotId": "bot-1"}))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        let aid = v["account"]["id"].as_str().unwrap().to_string();
        bind(&st, "bot-1", &aid).await;
        let (s, _) = call(&st, "PUT", "/bots/bot-2/execution-binding", "user-a", Some(json!({"vendor": "openai", "accountBindingId": aid}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_with_dependents_is_409_unless_forced() {
        let st = setup("del").await;
        let aid = account(&st).await;
        bind(&st, "bot-1", &aid).await;
        let (s, v) = call(&st, "DELETE", &format!("/provider-accounts/{aid}"), "user-a", None).await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["dependentBots"][0]["botId"], "bot-1");
        let (s, _) = call(&st, "DELETE", &format!("/provider-accounts/{aid}?force=true"), "user-a", None).await;
        assert_eq!(s, StatusCode::OK);
        let (_, v) = call(&st, "GET", "/bots/bot-1/execution-binding", "user-a", None).await;
        assert_eq!(v["binding"]["state"], "NEEDS_AUTH");
        assert!(v["binding"]["accountBindingId"].is_null());
        let (s, _) = call(&st, "GET", &format!("/provider-accounts/{aid}"), "user-a", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn exec_binding_needs_owned_bot() {
        let st = setup("own").await;
        let (s, _) = call(&st, "PUT", "/bots/bot-1/execution-binding", "user-b", Some(json!({"vendor": "openai"}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn remote_binding_lifecycle_and_frozen_lane() {
        let st = setup("rtb").await;
        let (s, v) = call(&st, "POST", "/threads/th-1/remote-bindings", "user-a", Some(json!({"generation": 1, "lane": "official", "capabilitySnapshot": {"steer": true}}))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        let rid = v["binding"]["id"].as_str().unwrap().to_string();
        assert_eq!(v["binding"]["capabilitySnapshot"]["steer"], true);
        let (s, _) = call(&st, "POST", "/threads/th-1/remote-bindings", "user-a", Some(json!({"generation": 1}))).await;
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, _) = call(&st, "POST", "/threads/th-1/remote-bindings", "user-b", Some(json!({"generation": 2}))).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let uri = format!("/remote-bindings/{rid}");
        for body in [json!({"lane": "ui_bridge"}), json!({"capabilitySnapshot": {}})] {
            let (s, _) = call(&st, "PATCH", &uri, "user-a", Some(body)).await;
            assert_eq!(s, StatusCode::CONFLICT);
        }
        let (s, v) = call(&st, "PATCH", &uri, "user-a", Some(json!({"state": "ACTIVE"}))).await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(v["allowed"], json!(["OPENING"]));
        for to in ["OPENING", "ACTIVE"] {
            let (s, _) = call(&st, "PATCH", &uri, "user-a", Some(json!({"state": to, "syncCursor": "c1"}))).await;
            assert_eq!(s, StatusCode::OK);
        }
        let (s, v) = call(&st, "PATCH", &uri, "user-a", Some(json!({"state": "CLOSED"}))).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["binding"]["syncCursor"], "c1");
        assert!(v["binding"]["closedAt"].is_string());
        let (_, v) = call(&st, "GET", "/threads/th-1/remote-bindings", "user-a", None).await;
        assert_eq!(v["bindings"].as_array().unwrap().len(), 1);
        let conn = st.db.connect().unwrap();
        let types: Vec<String> = conn
            .prepare("SELECT event_type FROM bot_events WHERE thread_id = 'th-1' ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(types, vec!["gateway.remote_thread.opened", "gateway.remote_thread.closed"]);
    }

    #[tokio::test]
    async fn channel_bindings_create_list_patch() {
        let st = setup("chan").await;
        let (s, v) = call(&st, "POST", "/threads/th-1/channel-bindings", "user-a", Some(json!({"provider": "slack", "externalConversationId": "C1:1.2", "readOnly": true}))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert_eq!(v["binding"]["readOnly"], true);
        assert_eq!(v["binding"]["bidirectional"], true);
        let cid = v["binding"]["id"].as_str().unwrap();
        let (s, v) = call(&st, "PATCH", &format!("/channel-bindings/{cid}"), "user-a", Some(json!({"syncState": "DELAYED", "lastInboundCursor": "9"}))).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["binding"]["syncState"], "DELAYED");
        let (s, _) = call(&st, "PATCH", &format!("/channel-bindings/{cid}"), "user-a", Some(json!({"syncState": "BOGUS"}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (_, v) = call(&st, "GET", "/threads/th-1/channel-bindings", "user-a", None).await;
        assert_eq!(v["bindings"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn gap_upsert_and_parity() {
        let st = setup("gap").await;
        let (_, v) = call(&st, "GET", "/vendor-packs/openai/parity", "user-a", None).await;
        assert_eq!(v["parity"], "full");
        let gap = json!({"capability": "dot.card.foo", "surface": "transcript", "severity": "visual_parity"});
        let (s, v) = call(&st, "POST", "/vendor-packs/openai/gaps", "user-a", Some(gap.clone())).await;
        assert_eq!(s, StatusCode::CREATED);
        let gid = v["gap"]["id"].as_str().unwrap().to_string();
        let (s, v) = call(&st, "POST", "/vendor-packs/openai/gaps", "user-a", Some(gap)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["gap"]["occurrences"], 2);
        assert_eq!(v["gap"]["id"], gid.as_str());
        let (_, v) = call(&st, "GET", "/vendor-packs/openai/parity", "user-a", None).await;
        assert_eq!(v["parity"], "partial");
        let (_, v) = call(&st, "POST", "/vendor-packs/openai/gaps", "user-a", Some(json!({"capability": "dot.card.bar", "surface": "card", "severity": "data_loss"}))).await;
        let dl = v["gap"]["id"].as_str().unwrap().to_string();
        let (_, v) = call(&st, "GET", "/vendor-packs/openai/parity", "user-a", None).await;
        assert_eq!(v["parity"], "blocked");
        let (s, _) = call(&st, "PATCH", &format!("/vendor-pack-gaps/{dl}"), "user-a", Some(json!({"status": "resolved"}))).await;
        assert_eq!(s, StatusCode::OK);
        let (_, v) = call(&st, "GET", "/vendor-packs/openai/parity", "user-a", None).await;
        assert_eq!(v["parity"], "partial");
        let (_, v) = call(&st, "GET", "/vendor-packs/openai/gaps?status=open", "user-a", None).await;
        assert_eq!(v["gaps"].as_array().unwrap().len(), 1);
        let (_, v) = call(&st, "GET", "/vendor-packs/openai/parity", "user-b", None).await;
        assert_eq!(v["parity"], "full", "gaps are owner-scoped");
    }

    // ---- provider keys, discovery, bindings list

    struct Disco {
        creds: std::sync::Mutex<Vec<Option<Value>>>,
        calls: std::sync::Mutex<usize>,
    }
    #[async_trait::async_trait]
    impl crate::gateway_runner::AaiTransport for Disco {
        async fn call(&self, _o: &str, op: &str, _b: &Value, _i: Value) -> Result<Value, crate::gateway_runner::AaiError> {
            *self.calls.lock().unwrap() += 1;
            assert_eq!(op, "agent.list");
            Ok(json!({ "agents": [{ "id": "a1", "name": "Alpha", "description": "d", "avatarUrl": "https://x/a.png" }, { "externalAgentId": "a2" }] }))
        }
        async fn call_cred(&self, o: &str, op: &str, b: &Value, c: Option<&Value>, i: Value) -> Result<Value, crate::gateway_runner::AaiError> {
            self.creds.lock().unwrap().push(c.cloned());
            self.call(o, op, b, i).await
        }
    }

    async fn key_account(st: &Arc<AppState>, auth: &str) -> String {
        let (s, v) = call(st, "POST", "/provider-accounts", "user-a", Some(json!({"vendor": "openai", "authType": auth}))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v["account"]["id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn secret_is_sealed_owner_checked_and_never_echoed() {
        std::env::set_var("ALLTERNIT_ENCRYPTION_KEY", "unit-test-encryption-key");
        let st = setup("secret").await;
        let aid = key_account(&st, "api_key").await;
        let k = "sk-LEAK-CHECK-9999";
        let (s, v) = call(&st, "POST", &format!("/provider-accounts/{aid}/secret"), "user-a", Some(json!({"apiKey": k}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["account"]["hasSecretRef"], true);
        assert!(!v.to_string().contains(k));
        let (_, g) = call(&st, "GET", &format!("/provider-accounts/{aid}"), "user-a", None).await;
        assert!(!g.to_string().contains(k));
        let conn = st.db.connect().unwrap();
        let sealed: String = conn.query_row("SELECT secret_ref FROM provider_account_bindings WHERE id=?1", params![aid], |r| r.get(0)).unwrap();
        assert!(sealed.starts_with("enc:v1:") && !sealed.contains(k));
        assert_eq!(crate::token_crypto::open(&sealed), k);
        let audits: String = conn.query_row("SELECT group_concat(event || detail_json) FROM connection_audit WHERE account_binding_id=?1", params![aid], |r| r.get(0)).unwrap();
        assert!(audits.contains("secret_set") && !audits.contains(k));
        let bot_events: String = conn.query_row("SELECT COALESCE(group_concat(payload), '') FROM bot_events", [], |r| r.get(0)).unwrap();
        assert!(!bot_events.contains(k));
        // Another user can neither set nor clear it.
        let (s, _) = call(&st, "POST", &format!("/provider-accounts/{aid}/secret"), "user-b", Some(json!({"apiKey": "x"}))).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = call(&st, "DELETE", &format!("/provider-accounts/{aid}/secret"), "user-b", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _) = call(&st, "POST", &format!("/provider-accounts/{aid}/secret"), "user-a", Some(json!({"apiKey": "  "}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, v) = call(&st, "DELETE", &format!("/provider-accounts/{aid}/secret"), "user-a", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["account"]["hasSecretRef"], false);
    }

    #[tokio::test]
    async fn discovery_uses_a_transient_binding_and_attaches_the_key_only_when_present() {
        std::env::set_var("ALLTERNIT_ENCRYPTION_KEY", "unit-test-encryption-key");
        let st = setup("disco").await;
        let with_key = key_account(&st, "api_key").await;
        let (s, _) = call(&st, "POST", &format!("/provider-accounts/{with_key}/secret"), "user-a", Some(json!({"apiKey": "sk-disco"}))).await;
        assert_eq!(s, StatusCode::OK);
        let d = Disco { creds: Default::default(), calls: Default::default() };
        let agents = discover_agents(&st.db, &d, "user-a", &with_key).await.unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0], json!({ "externalAgentId": "a1", "name": "Alpha", "description": "d", "avatarUrl": "https://x/a.png" }));
        assert_eq!(agents[1]["externalAgentId"], "a2");
        assert_eq!(d.creds.lock().unwrap()[0].as_ref().unwrap()["apiKey"], "sk-disco");

        // browser-session account: no credential is sent.
        let plain = account(&st).await;
        discover_agents(&st.db, &d, "user-a", &plain).await.unwrap();
        assert!(d.creds.lock().unwrap()[1].is_none());

        // api_key account without a key: AUTH_REQUIRED before any call is made.
        let empty = key_account(&st, "api_key").await;
        let before = *d.calls.lock().unwrap();
        let (status, code, _) = discover_agents(&st.db, &d, "user-a", &empty).await.unwrap_err();
        assert_eq!((status, code.as_str()), (StatusCode::UNAUTHORIZED, "AUTH_REQUIRED"));
        assert_eq!(*d.calls.lock().unwrap(), before);
        // Someone else's account is invisible.
        assert_eq!(discover_agents(&st.db, &d, "user-b", &with_key).await.unwrap_err().0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn vendor_agent_name_and_avatar_are_validated_stored_and_refreshed_by_discovery() {
        let st = setup("identity").await;
        let aid = account(&st).await;
        let put = |body: Value| { let st = st.clone(); async move { call(&st, "PUT", "/bots/bot-1/execution-binding", "user-a", Some(body)).await } };
        let base = |extra: Value| { let mut b = json!({"vendor": "openai", "accountBindingId": aid, "externalAgentId": "a1"}); b.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone()); b };
        // Rejected: svg (scriptable), plain http, oversized.
        for bad in ["data:image/svg+xml;base64,PHN2Zz4=", "http://x/a.png", "javascript:alert(1)", "https://x/a b.png"] {
            let (s, v) = put(base(json!({"externalAgentAvatar": bad}))).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {v}");
        }
        let (s, _) = put(base(json!({"externalAgentAvatar": format!("data:image/png;base64,{}", "A".repeat(300 * 1024))}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // Accepted: inline png, stored with the name.
        let (s, v) = put(base(json!({"externalAgentName": "Nova", "externalAgentAvatar": "data:image/png;base64,iVBORw0KGgo="}))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert_eq!(v["binding"]["externalAgentName"], "Nova");
        assert_eq!(v["binding"]["externalAgentAvatar"], "data:image/png;base64,iVBORw0KGgo=");
        // Discovery refreshes the bound agent's identity; unsafe avatars from an adapter are dropped.
        let conn = st.db.connect().unwrap();
        refresh_agent_identity(&conn, "user-a", &aid, &[json!({"externalAgentId": "a1", "name": "Alpha", "avatarUrl": "https://x/a.png"})]);
        let (_, v) = call(&st, "GET", "/bots/bot-1/execution-binding", "user-a", None).await;
        assert_eq!((v["binding"]["externalAgentName"].as_str(), v["binding"]["externalAgentAvatar"].as_str()), (Some("Alpha"), Some("https://x/a.png")));
        // Another owner's refresh touches nothing.
        refresh_agent_identity(&conn, "user-b", &aid, &[json!({"externalAgentId": "a1", "name": "Mallory"})]);
        let (_, v) = call(&st, "GET", "/bots/bot-1/execution-binding", "user-a", None).await;
        assert_eq!(v["binding"]["externalAgentName"], "Alpha");
        assert!(valid_avatar("https://cdn.example/a.webp") && !valid_avatar("data:image/png;base64,<script>"));
    }

    #[test]
    fn discovery_uses_the_adapter_the_account_was_connected_with() {
        assert_eq!(adapter_for_auth("anthropic", "browser_session"), Some("claude-subscription"));
        assert_eq!(adapter_for_auth("anthropic", "api_key"), Some("claude-managed-agents"));
        assert_eq!(adapter_for_auth("anthropic", "desktop_session"), None);
        assert_eq!(adapter_for_auth("openai", "browser_session"), Some("chatgpt-subscription"));
        assert_eq!(adapter_for_auth("openai", "desktop_session"), Some("chatgpt-dots"));
        assert_eq!(adapter_for_auth("kimi", "browser_session"), Some("kimi-subscription"));
        assert_eq!(adapter_for_auth("google", "browser_session"), Some("gemini-subscription"));
        assert_eq!(adapter_for_auth("google", "api_key"), None);
        assert_eq!(adapter_for_auth("microsoft", "browser_session"), Some("copilot-subscription"));
        assert_eq!(adapter_for_auth("microsoft", "desktop_session"), None);
        assert_eq!(adapter_for_auth("hermes", "local_endpoint"), Some("hermes"));
        assert_eq!(adapter_for_auth("grok", "desktop_session"), None);
    }

    #[tokio::test]
    async fn execution_bindings_list_returns_only_the_owners() {
        let st = setup("list").await;
        let aid = account(&st).await;
        connect(&st, &aid).await;
        bind(&st, "bot-1", &aid).await;
        bind(&st, "bot-2", &aid).await;
        let (s, v) = call(&st, "GET", "/execution-bindings", "user-a", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["bindings"].as_array().unwrap().len(), 2);
        let (_, other) = call(&st, "GET", "/execution-bindings", "user-b", None).await;
        assert_eq!(other["bindings"].as_array().unwrap().len(), 0);
    }

    // ---- vendor memory

    struct Mem(Result<Value, (&'static str, &'static str)>);
    #[async_trait::async_trait]
    impl crate::gateway_runner::AaiTransport for Mem {
        async fn call(&self, _o: &str, op: &str, _b: &Value, _i: Value) -> Result<Value, crate::gateway_runner::AaiError> {
            assert_eq!(op, "agent.memory");
            self.0.clone().map_err(|(c, m)| crate::gateway_runner::AaiError::new(c, m))
        }
    }

    fn events(st: &Arc<AppState>, ty: &str) -> i64 {
        st.db.connect().unwrap().query_row("SELECT COUNT(*) FROM bot_events WHERE bot_id='bot-1' AND event_type=?1", params![ty], |r| r.get(0)).unwrap()
    }

    async fn bound(tag: &str) -> Arc<AppState> {
        let st = setup(tag).await;
        let aid = account(&st).await;
        connect(&st, &aid).await;
        bind(&st, "bot-1", &aid).await;
        st
    }

    #[tokio::test]
    async fn vendor_memory_readable_opaque_unavailable_and_native_404() {
        let st = bound("vmem").await;
        let ok = Mem(Ok(json!({ "records": [{ "id": "r1", "scope": "bot", "text": "likes tea", "remoteRef": "vm-1", "updatedAt": "2026-01-01" }, { "text": "no id" }] })));
        let v = read_vendor_memory(&st.db, &ok, "user-a", "bot-1").await.unwrap();
        assert_eq!((v["authority"].as_str(), v["observability"].as_str(), v["promotable"].as_bool()), (Some("vendor"), Some("readable"), Some(true)));
        assert_eq!(v["records"].as_array().unwrap().len(), 1);
        assert_eq!(v["records"][0]["remoteRef"], "vm-1");
        // Opaque via reported flag and via the capability snapshot (no records either way).
        let v = read_vendor_memory(&st.db, &Mem(Ok(json!({ "opaque": true }))), "user-a", "bot-1").await.unwrap();
        assert_eq!((v["observability"].as_str(), v["promotable"].as_bool()), (Some("opaque"), Some(false)));
        assert!(v.get("records").is_none());
        // The capability snapshot is set when binding (PUT); PATCH only moves state/health.
        let aid: String = st.db.connect().unwrap().query_row("SELECT account_binding_id FROM bot_execution_bindings WHERE bot_id='bot-1'", [], |r| r.get(0)).unwrap();
        let (s, v) = call(&st, "PUT", "/bots/bot-1/execution-binding", "user-a", Some(json!({"vendor": "openai", "accountBindingId": aid, "capabilities": {"memory": {"opaque": true}}}))).await;
        assert!(s == StatusCode::OK || s == StatusCode::CREATED, "{v}");
        let v = read_vendor_memory(&st.db, &Mem(Err(("INTERNAL", "must not be called"))), "user-a", "bot-1").await.unwrap();
        assert_eq!(v["observability"], "opaque");
        // Unsupported -> unavailable with a reason.
        let v = read_vendor_memory(&st.db, &Mem(Err(("UNSUPPORTED", "no memory op"))), "user-a", "bot-2").await.unwrap_err();
        assert_eq!(v.0, StatusCode::NOT_FOUND, "unbound/native bot");
        let st2 = bound("vmem2").await;
        let v = read_vendor_memory(&st2.db, &Mem(Err(("UNSUPPORTED", "no memory op"))), "user-a", "bot-1").await.unwrap();
        assert_eq!((v["observability"].as_str(), v["reason"].as_str()), (Some("unavailable"), Some("no memory op")));
    }

    #[tokio::test]
    async fn promote_creates_one_native_note_with_provenance_and_one_event() {
        let st = bound("vprom").await;
        let m = Mem(Ok(json!({ "records": [{ "id": "r1", "scope": "bot", "text": "likes tea", "remoteRef": "vm-1" }] })));
        let (s0, _, _) = promote_vendor_memory(&st.db, &m, "user-a", "bot-1", "r1", "galaxy").await.unwrap_err();
        assert_eq!(s0, StatusCode::BAD_REQUEST);
        assert_eq!(promote_vendor_memory(&st.db, &m, "user-a", "bot-1", "nope", "bot").await.unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(events(&st, "memory.promoted"), 0, "reading never promotes");
        let v = promote_vendor_memory(&st.db, &m, "user-a", "bot-1", "r1", "project").await.unwrap();
        assert_eq!(v["promoted"], true);
        let again = promote_vendor_memory(&st.db, &m, "user-a", "bot-1", "r1", "project").await.unwrap();
        assert_eq!(again["alreadyPromoted"], true);
        let c = st.db.connect().unwrap();
        let (n, tags, content): (i64, String, String) = c
            .query_row("SELECT COUNT(*), MAX(tags), MAX(content) FROM memory_notes WHERE user_id='user-a'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((n, content.as_str()), (1, "likes tea"));
        assert!(tags.contains("source:vendor") && tags.contains("vendor:openai") && tags.contains("remoteRef:vm-1"), "{tags}");
        assert_eq!(events(&st, "memory.promoted"), 1);
        // Opaque memory can never be promoted.
        let o = Mem(Ok(json!({ "opaque": true })));
        assert_eq!(promote_vendor_memory(&st.db, &o, "user-a", "bot-1", "r1", "bot").await.unwrap_err().0, StatusCode::CONFLICT);
    }

    // ---- contract fixes (docs/gateway/CONTRACT_AUDIT.md R1, R3, R7, R8, R9), real TS wire shapes

    /// `agent.list` as the AAI host returns it: a bare array of `{agentId, displayName, vendor, state}`
    /// (services/subscription-gateway/test/chatgpt-dots-adapter.test.ts).
    struct HostList;
    #[async_trait::async_trait]
    impl crate::gateway_runner::AaiTransport for HostList {
        async fn call(&self, _o: &str, _op: &str, _b: &Value, _i: Value) -> Result<Value, crate::gateway_runner::AaiError> {
            Ok(json!([
                { "agentId": "agent_1:nova-dot", "displayName": "Nova", "vendor": "openai", "state": "ready", "kind": "gpt", "kindLabel": "GPT", "avatarUrl": "https://example.invalid/nova.png" },
                { "agentId": "agent_1:atlas-dot", "displayName": "Atlas", "vendor": "openai", "state": "ready" }
            ]))
        }
    }

    #[tokio::test]
    async fn r1_discovery_reads_the_hosts_agent_id_and_display_name() {
        let st = setup("r1").await;
        let aid = account(&st).await;
        let agents = discover_agents(&st.db, &HostList, "user-a", &aid).await.unwrap();
        assert_eq!(agents, vec![json!({ "externalAgentId": "agent_1:nova-dot", "name": "Nova", "kind": "gpt", "kindLabel": "GPT", "avatarUrl": "https://example.invalid/nova.png" }), json!({ "externalAgentId": "agent_1:atlas-dot", "name": "Atlas" })]);
    }

    #[test]
    fn r3_every_contract_error_code_maps_to_a_status() {
        let st = |c: &str| map_aai_err(crate::gateway_runner::AaiError::new(c, "m")).0;
        assert_eq!(st("AUTH_REVOKED"), StatusCode::UNAUTHORIZED);
        assert_eq!(st("POLICY_DENIED"), StatusCode::FORBIDDEN);
        assert_eq!(st("CONTEXT_NOT_FOUND"), StatusCode::NOT_FOUND);
        assert_eq!(st("CONTEXT_BUSY"), StatusCode::CONFLICT);
        assert_eq!(st("UNSUPPORTED"), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(st("VENDOR_UNAVAILABLE"), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(st("UNKNOWN"), StatusCode::BAD_GATEWAY);
        let (_, code, msg) = map_aai_err(crate::gateway_runner::AaiError::new("RATE_LIMITED", "slow down"));
        assert_eq!((code.as_str(), msg.as_str()), ("RATE_LIMITED", "slow down"));
    }

    #[tokio::test]
    async fn r7_channel_bindings_carry_display_names() {
        let st = setup("r7").await;
        let (s, v) = call(&st, "POST", "/threads/th-1/channel-bindings", "user-a", Some(json!({"provider": "slack", "externalConversationId": "C1:1.2", "externalChannelId": "C1", "channelName": "#ops", "workspaceName": "Acme"}))).await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        assert_eq!((v["binding"]["channelName"].as_str(), v["binding"]["workspaceName"].as_str()), (Some("#ops"), Some("Acme")));
        let cid = v["binding"]["id"].as_str().unwrap();
        let (_, v) = call(&st, "PATCH", &format!("/channel-bindings/{cid}"), "user-a", Some(json!({"channelName": "#ops-2"}))).await;
        assert_eq!((v["binding"]["channelName"].as_str(), v["binding"]["workspaceName"].as_str()), (Some("#ops-2"), Some("Acme")));
        let (_, v) = call(&st, "POST", "/threads/th-1/channel-bindings", "user-a", Some(json!({"provider": "slack", "externalConversationId": "C2:1"}))).await;
        assert!(v["binding"]["channelName"].is_null(), "names are optional");
    }

    #[tokio::test]
    async fn r8_accounts_filter_by_state() {
        let st = setup("r8").await;
        let a = account(&st).await;
        connect(&st, &a).await;
        account(&st).await;
        let (_, v) = call(&st, "GET", "/provider-accounts?state=CONNECTED", "user-a", None).await;
        let ids: Vec<&str> = v["accounts"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec![a.as_str()]);
        let (_, v) = call(&st, "GET", "/provider-accounts?vendor=openai", "user-a", None).await;
        assert_eq!(v["accounts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn r9_gap_patch_takes_severity_and_optional_status() {
        let st = setup("r9").await;
        let (_, v) = call(&st, "POST", "/vendor-packs/openai/gaps", "user-a", Some(json!({"capability": "c", "surface": "card", "severity": "visual_parity"}))).await;
        let gid = v["gap"]["id"].as_str().unwrap().to_string();
        let (s, v) = call(&st, "PATCH", &format!("/vendor-pack-gaps/{gid}"), "user-a", Some(json!({"severity": "data_loss"}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!((v["gap"]["severity"].as_str(), v["gap"]["status"].as_str()), (Some("data_loss"), Some("open")));
        let (s, _) = call(&st, "PATCH", &format!("/vendor-pack-gaps/{gid}"), "user-a", Some(json!({"severity": "cosmetic"}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(&st, "PATCH", &format!("/vendor-pack-gaps/{gid}"), "user-a", Some(json!({}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn vendor_memory_reads_the_contract_record_shape() {
        // memoryRecordSchema: {id?, text?, scope, source, vendor?, remoteRef?, authority, promotable}
        let st = bound("vmemc").await;
        let m = Mem(Ok(json!([{ "text": "prefers metric", "scope": "bot", "source": "vendor", "vendor": "openai", "remoteRef": "mem_9", "authority": "vendor", "promotable": true }])));
        let v = read_vendor_memory(&st.db, &m, "user-a", "bot-1").await.unwrap();
        assert_eq!(v["records"][0]["id"], "mem_9");
        assert_eq!(v["records"][0]["text"], "prefers metric");
    }

    #[tokio::test]
    async fn vendor_memory_is_owner_scoped() {
        let st = bound("vown").await;
        let m = Mem(Ok(json!({ "records": [{ "id": "r1", "text": "x" }] })));
        assert_eq!(read_vendor_memory(&st.db, &m, "user-b", "bot-1").await.unwrap_err().0, StatusCode::NOT_FOUND);
        assert_eq!(promote_vendor_memory(&st.db, &m, "user-b", "bot-1", "r1", "bot").await.unwrap_err().0, StatusCode::NOT_FOUND);
        let (s, _) = call(&st, "GET", "/bots/bot-1/vendor-memory", "user-b", None).await;
        assert!(s == StatusCode::NOT_FOUND, "{s}");
    }
}
